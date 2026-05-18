use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Sse, sse::Event};
use axum::routing::get;
use datastar::{
    axum::ReadSignals,
    prelude::{ExecuteScript, PatchElements},
};
use futures_util::stream;
use html_escape::{encode_double_quoted_attribute, encode_text};
use serde::Deserialize;
use tower_http::trace::TraceLayer;

use nix_search_config::AppConfig;
use nix_search_core::{
    CommonDoc, DocumentKind, License, Maintainer, SearchDocument, SourceLinkConfig,
    SourceLinkResolver,
};
use nix_search_index::{
    EntryLookup, EntryLookupResult, IndexStore, SearchHit, SearchIndex, SearchOptions, SearchScope,
};

const DEFAULT_LIMIT: usize = 20;

#[derive(Debug, Clone)]
struct AppState {
    config: Arc<AppConfig>,
    index_path: Arc<PathBuf>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct PageQuery {
    q: Option<String>,

    #[serde(rename = "ref")]
    ref_id: Option<String>,

    kind: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct PageRequest {
    source: Option<String>,
    entry: Option<String>,
    query: PageQuery,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct SearchSignals {
    q: Option<String>,
    source: Option<String>,

    #[serde(rename = "ref")]
    ref_id: Option<String>,

    kind: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct EntryEventQuery {
    source: String,
    entry: String,

    #[serde(rename = "ref")]
    ref_id: Option<String>,

    kind: Option<String>,
}

pub async fn serve(config: AppConfig) -> Result<()> {
    let index_store = IndexStore::new(&config.data.index_dir);
    let index_path = index_store.current_path().with_context(|| {
        format!(
            "failed to locate current index in {}; run `nix-search update` first",
            config.data.index_dir.display()
        )
    })?;

    let addr: SocketAddr =
        config.server.listen.parse().with_context(|| {
            format!("failed to parse listen address {:?}", config.server.listen)
        })?;

    let state = AppState {
        config: Arc::new(config),
        index_path: Arc::new(index_path),
    };

    let app = Router::new()
        .route("/-/health", get(health))
        .route("/-/search/events", get(search_events))
        .route("/-/entry/events", get(entry_events))
        .route("/-/entry/close", get(entry_close_events))
        .route("/", get(root_page))
        .route("/{source}", get(source_page))
        .route("/{source}/{*entry}", get(entry_page))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;

    tracing::info!("serving nix-search web UI at http://{addr}");

    axum::serve(listener, app)
        .await
        .context("web server failed")?;

    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

async fn root_page(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> impl IntoResponse {
    let request = PageRequest {
        source: None,
        entry: None,
        query,
    };

    render_page_response(&state, request)
}

async fn source_page(
    State(state): State<AppState>,
    Path(source): Path<String>,
    Query(query): Query<PageQuery>,
) -> impl IntoResponse {
    let request = PageRequest {
        source: Some(source),
        entry: None,
        query,
    };

    render_page_response(&state, request)
}

async fn entry_page(
    State(state): State<AppState>,
    Path((source, entry)): Path<(String, String)>,
    Query(query): Query<PageQuery>,
) -> impl IntoResponse {
    let entry = decode_path_value(&entry).unwrap_or(entry);

    let request = PageRequest {
        source: Some(source),
        entry: Some(entry),
        query,
    };

    render_page_response(&state, request)
}

fn render_page_response(state: &AppState, request: PageRequest) -> Html<String> {
    match run_page_search(state, &request) {
        Ok(hits) => Html(render_page(state, &request, Some(Ok(&hits)))),
        Err(error) => Html(render_page(
            state,
            &request,
            Some(Err(&format!("{error:#}"))),
        )),
    }
}

async fn search_events(
    State(state): State<AppState>,
    ReadSignals(signals): ReadSignals<SearchSignals>,
) -> impl IntoResponse {
    let request = PageRequest {
        source: signals
            .source
            .clone()
            .and_then(|value| non_empty_owned(value)),
        entry: None,
        query: PageQuery {
            q: signals.q,
            ref_id: signals.ref_id,
            kind: signals.kind,
        },
    };

    let results_html = match run_page_search(&state, &request) {
        Ok(hits) => render_results_container(&request, &hits, &state.config),
        Err(error) => render_error_container(&format!("{error:#}")),
    };

    let event = PatchElements::new(results_html).write_as_axum_sse_event();

    Sse::new(stream::once(async move { Ok::<Event, Infallible>(event) }))
}

async fn entry_events(
    State(state): State<AppState>,
    Query(entry_query): Query<EntryEventQuery>,
    ReadSignals(signals): ReadSignals<SearchSignals>,
) -> impl IntoResponse {
    let entry = decode_path_value(&entry_query.entry).unwrap_or(entry_query.entry);

    let request = PageRequest {
        source: Some(entry_query.source.clone()),
        entry: Some(entry.clone()),
        query: PageQuery {
            q: signals.q,
            ref_id: entry_query.ref_id.or(signals.ref_id),
            kind: entry_query.kind.or(signals.kind),
        },
    };

    let modal_html = render_entry_modal_for_request(&state, &request);
    let next_url = entry_url_for_request(&request, &entry);

    let script = format!(
        r#"
history.pushState(null, "", {});
document.getElementById("entry-modal")?.showModal();
    "#,
        js_string(&next_url)
    );

    let events: Vec<Result<Event, Infallible>> = vec![
        Ok(PatchElements::new(modal_html).write_as_axum_sse_event()),
        Ok(ExecuteScript::new(script).write_as_axum_sse_event()),
    ];

    Sse::new(stream::iter(events))
}

async fn entry_close_events() -> impl IntoResponse {
    let html = r#"<div id="entry-modal-container"></div>"#;

    let script = r#"
const url = new URL(window.location.href);
const parts = url.pathname.split("/").filter(Boolean);

if (parts.length >= 2) {
    url.pathname = "/" + parts[0];
} else {
    url.pathname = "/";
}

history.pushState(null, "", url);
    "#;

    let events: Vec<Result<Event, Infallible>> = vec![
        Ok(PatchElements::new(html).write_as_axum_sse_event()),
        Ok(ExecuteScript::new(script).write_as_axum_sse_event()),
    ];

    Sse::new(stream::iter(events))
}

fn run_page_search(state: &AppState, request: &PageRequest) -> Result<Vec<SearchHit>> {
    let Some(q) = normalized_query(&request.query) else {
        return Ok(Vec::new());
    };

    let index = SearchIndex::open(&*state.index_path).with_context(|| {
        format!(
            "failed to open current search index {}",
            state.index_path.display()
        )
    })?;

    let scopes = state
        .config
        .resolve_search_scopes(
            request.source.as_deref().and_then(non_empty),
            request.query.ref_id.as_deref().and_then(non_empty),
        )
        .context("failed to resolve search scope")?
        .into_iter()
        .map(|scope| SearchScope {
            source: scope.source,
            ref_id: scope.ref_id,
        })
        .collect();

    index
        .search(SearchOptions {
            query: q.to_owned(),
            limit: DEFAULT_LIMIT,
            scopes,
        })
        .context("search failed")
}

fn normalized_query(query: &PageQuery) -> Option<&str> {
    query.q.as_deref().and_then(non_empty)
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();

    if value.is_empty() { None } else { Some(value) }
}

fn non_empty_owned(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

fn render_page(
    state: &AppState,
    request: &PageRequest,
    results: Option<std::result::Result<&[SearchHit], &str>>,
) -> String {
    let q = request.query.q.as_deref().unwrap_or("");
    let source = request.source.as_deref().unwrap_or("");
    let ref_id = request.query.ref_id.as_deref().unwrap_or("");
    let kind = request.query.kind.as_deref().unwrap_or("");

    let results_html = match results {
        Some(Ok(hits)) => render_results_container(request, hits, &state.config),
        Some(Err(error)) => render_error_container(error),
        None => render_empty_results_container(),
    };

    let modal_html = if request.entry.is_some() {
        render_entry_modal_for_request(state, request)
    } else {
        r#"<div id="entry-modal-container"></div>"#.to_owned()
    };

    let form_action = request
        .source
        .as_deref()
        .map(source_path)
        .unwrap_or_else(|| "/".to_owned());

    format!(
        r#"<!doctype html>
   <html lang="en">
   <head>
     <meta charset="utf-8">
     <meta name="viewport" content="width=device-width, initial-scale=1">
     <title>Nix Search</title>
     <script type="module"
 src="https://cdn.jsdelivr.net/gh/starfederation/datastar@main/bundles/datastar.js"></script>
     <style>
       :root {{
         color-scheme: light dark;
         --bg: #0f172a;
         --panel: #111827;
         --text: #e5e7eb;
         --muted: #9ca3af;
         --accent: #38bdf8;
         --border: #374151;
         --danger: #fecaca;
       }}

       body {{
         margin: 0;
         font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
         background: var(--bg);
         color: var(--text);
       }}

       main {{
         max-width: 960px;
         margin: 0 auto;
         padding: 2rem 1rem 4rem;
       }}

       h1 {{
         margin-bottom: 0.25rem;
         font-size: 2rem;
       }}

       .subtitle {{
         color: var(--muted);
         margin-top: 0;
         margin-bottom: 2rem;
       }}

       form.search {{
         display: grid;
         gap: 0.75rem;
         background: var(--panel);
         border: 1px solid var(--border);
         border-radius: 0.75rem;
         padding: 1rem;
         margin-bottom: 1.25rem;
       }}

       .filters {{
         display: grid;
         gap: 0.75rem;
         grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
       }}

       label {{
         display: grid;
         gap: 0.25rem;
         color: var(--muted);
         font-size: 0.875rem;
       }}

       input {{
         box-sizing: border-box;
         width: 100%;
         border: 1px solid var(--border);
         border-radius: 0.5rem;
         background: #030712;
         color: var(--text);
         padding: 0.7rem 0.8rem;
         font: inherit;
       }}

       input[type="search"] {{
         font-size: 1.1rem;
       }}

       button {{
         border: 0;
         border-radius: 0.5rem;
         background: var(--accent);
         color: #082f49;
         font-weight: 700;
         padding: 0.65rem 1rem;
         cursor: pointer;
       }}

       .status, .error {{
         border: 1px solid var(--border);
         border-radius: 0.75rem;
         padding: 1rem;
         color: var(--muted);
         background: var(--panel);
       }}

       .error {{
         color: var(--danger);
         border-color: #7f1d1d;
       }}

       .results {{
         display: grid;
         gap: 0.75rem;
       }}

       .result {{
         border: 1px solid var(--border);
         border-radius: 0.75rem;
         padding: 1rem;
         background: var(--panel);
       }}

       .result h2 {{
         margin: 0 0 0.35rem;
         font-size: 1.15rem;
       }}

       .result h2 code {{
         color: var(--text);
       }}

       .meta {{
         color: var(--muted);
         font-size: 0.875rem;
         margin-bottom: 0.5rem;
       }}

       .summary {{
         margin: 0.5rem 0;
       }}

       a {{
         color: var(--accent);
       }}

       dialog {{
         width: min(900px, calc(100vw - 2rem));
         max-height: calc(100vh - 2rem);
         border: 1px solid var(--border);
         border-radius: 1rem;
         background: var(--panel);
         color: var(--text);
         padding: 0;
       }}

       dialog::backdrop {{
         background: rgb(0 0 0 / 0.65);
       }}

       .entry {{
         padding: 1.25rem;
       }}

       .entry header {{
         display: flex;
         justify-content: space-between;
         align-items: start;
         gap: 1rem;
         border-bottom: 1px solid var(--border);
         padding-bottom: 1rem;
         margin-bottom: 1rem;
       }}

       .entry h2 {{
         margin: 0;
         font-size: 1.35rem;
       }}

       .entry-section {{
         margin-top: 1rem;
       }}

       .entry-section h3 {{
         margin-bottom: 0.35rem;
       }}

       pre {{
         overflow: auto;
         background: #030712;
         border: 1px solid var(--border);
         border-radius: 0.5rem;
         padding: 0.75rem;
       }}

       ul {{
         padding-left: 1.25rem;
       }}
     </style>
   </head>
   <body
     data-signals-q="{q_attr}"
     data-signals-source="{source_attr}"
     data-signals-ref="{ref_attr}"
     data-signals-kind="{kind_attr}"
   >
     <main>
       <h1>Nix Search</h1>
       <p class="subtitle">Search indexed Nix packages and options.</p>

       <form class="search" action="{form_action}" method="get">
         <label>
           Query
           <input
             type="search"
             name="q"
             value="{q_attr}"
             placeholder="git, programs.git.enable, services.nginx..."
             autocomplete="off"
             autofocus
             data-bind-q
             data-on-input__debounce.300ms="@get('/-/search/events')"
           >
         </label>

         <div class="filters">
           <label>
             Ref
             <input
               name="ref"
               value="{ref_attr}"
               placeholder="optional"
               data-bind-ref
               data-on-input__debounce.300ms="@get('/-/search/events')"
             >
           </label>
         </div>

         <button type="submit">Search</button>
       </form>

       {results_html}
       {modal_html}
     </main>
   </body>
   </html>"#,
        q_attr = encode_double_quoted_attribute(q),
        source_attr = encode_double_quoted_attribute(source),
        ref_attr = encode_double_quoted_attribute(ref_id),
        kind_attr = encode_double_quoted_attribute(kind),
        form_action = encode_double_quoted_attribute(&form_action),
    )
}

fn render_results_container(
    request: &PageRequest,
    hits: &[SearchHit],
    config: &AppConfig,
) -> String {
    let Some(q) = normalized_query(&request.query) else {
        return render_empty_results_container();
    };

    if hits.is_empty() {
        return format!(
            r#"<div id="results" class="status">No results for <strong>{}</strong>.</div>"#,
            encode_text(q)
        );
    }

    let mut html = format!(
        r#"<div id="results" class="results" aria-live="polite"><div class="status">{} result{} for
 <strong>{}</strong>.</div>"#,
        hits.len(),
        if hits.len() == 1 { "" } else { "s" },
        encode_text(q),
    );

    for hit in hits {
        html.push_str(&render_hit(request, hit, config));
    }

    html.push_str("</div>");
    html
}

fn render_empty_results_container() -> String {
    r#"<div id="results" class="status">Enter a search query.</div>"#.to_owned()
}

fn render_error_container(error: &str) -> String {
    format!(
        r#"<div id="results" class="error"><strong>Search failed:</strong> {}</div>"#,
        encode_text(error)
    )
}

fn render_hit(request: &PageRequest, hit: &SearchHit, config: &AppConfig) -> String {
    let common = hit.document.common();
    let summary = summary_for_document(&hit.document);
    let source_link = first_source_link(&hit.document, config);

    let entry_url = entry_url(
        &common.source,
        &common.name,
        Some(&common.ref_id),
        Some(common.kind.as_str()),
        request.query.q.as_deref(),
    );

    let entry_events = entry_events_url(
        &common.source,
        &common.name,
        Some(&common.ref_id),
        Some(common.kind.as_str()),
    );

    let mut html = format!(
        r#"<article class="result">
     <h2>
       <a href="{href}" data-on-click__prevent="@get('{events_href}')">
         <code>{name}</code>
       </a>
     </h2>
     <div class="meta">{kind} · {source}/{ref_id} · score {score:.3}</div>"#,
        href = encode_double_quoted_attribute(&entry_url),
        events_href = encode_double_quoted_attribute(&entry_events),
        name = encode_text(&common.name),
        kind = encode_text(common.kind.as_str()),
        source = encode_text(&common.source),
        ref_id = encode_text(&common.ref_id),
        score = hit.score,
    );

    if let Some(summary) = summary {
        html.push_str(&format!(
            r#"
     <p class="summary">{}</p>"#,
            encode_text(summary)
        ));
    }

    if let Some(source_link) = source_link {
        html.push_str(&format!(
            r#"
     <a href="{href}" rel="noreferrer">Source</a>"#,
            href = encode_double_quoted_attribute(&source_link),
        ));
    }

    html.push_str("\n</article>");
    html
}

fn render_entry_modal_for_request(state: &AppState, request: &PageRequest) -> String {
    let Some(source) = request.source.as_deref() else {
        return render_entry_error_modal("Entry source is missing.");
    };

    let Some(entry) = request.entry.as_deref() else {
        return r#"<div id="entry-modal-container"></div>"#.to_owned();
    };

    let ref_id = match resolve_entry_ref(&state.config, source, request.query.ref_id.as_deref()) {
        Ok(ref_id) => ref_id,
        Err(error) => return render_entry_error_modal(&format!("{error:#}")),
    };

    let kind = match parse_document_kind(request.query.kind.as_deref()) {
        Ok(kind) => kind,
        Err(error) => return render_entry_error_modal(&error),
    };

    let index = match SearchIndex::open(&*state.index_path) {
        Ok(index) => index,
        Err(error) => return render_entry_error_modal(&format!("{error:#}")),
    };

    let lookup = EntryLookup {
        source: source.to_owned(),
        ref_id,
        name: entry.to_owned(),
        kind,
    };

    match index.find_entry(lookup) {
        Ok(result) => render_entry_lookup_result(request, result, &state.config),
        Err(error) => render_entry_error_modal(&format!("{error:#}")),
    }
}

fn render_entry_lookup_result(
    request: &PageRequest,
    result: EntryLookupResult,
    config: &AppConfig,
) -> String {
    match result {
        EntryLookupResult::Found(document) => render_entry_modal(&document, config),
        EntryLookupResult::NotFound => render_entry_error_modal("Entry not found."),
        EntryLookupResult::Ambiguous(documents) => {
            render_ambiguous_entry_modal(request, &documents)
        }
    }
}

fn render_entry_modal(document: &SearchDocument, config: &AppConfig) -> String {
    let common = document.common();

    format!(
        r#"<div id="entry-modal-container">
     <dialog id="entry-modal" open>
       <article class="entry">
         <header>
           <div>
             <h2><code>{name}</code></h2>
             <div class="meta">{kind} · {source}/{ref_id}{revision}</div>
           </div>
           <button type="button" data-on-click__prevent="@get('/-/entry/close')"
 onclick="this.closest('dialog')?.close()">Close</button>
         </header>
         {detail}
       </article>
     </dialog>
   </div>"#,
        name = encode_text(&common.name),
        kind = encode_text(common.kind.as_str()),
        source = encode_text(&common.source),
        ref_id = encode_text(&common.ref_id),
        revision = common
            .revision
            .as_deref()
            .map(|revision| format!(" · {}", encode_text(revision)))
            .unwrap_or_default(),
        detail = render_entry_detail(document, config),
    )
}

fn render_entry_error_modal(message: &str) -> String {
    format!(
        r#"<div id="entry-modal-container">
     <dialog id="entry-modal" open>
       <article class="entry">
         <header>
           <h2>Entry</h2>
           <button type="button" data-on-click__prevent="@get('/-/entry/close')"
 onclick="this.closest('dialog')?.close()">Close</button>
         </header>
         <div class="error">{}</div>
       </article>
     </dialog>
   </div>"#,
        encode_text(message)
    )
}

fn render_ambiguous_entry_modal(request: &PageRequest, documents: &[SearchDocument]) -> String {
    let mut list = String::new();

    for document in documents {
        let common = document.common();
        let href = entry_url(
            &common.source,
            &common.name,
            Some(&common.ref_id),
            Some(common.kind.as_str()),
            request.query.q.as_deref(),
        );

        list.push_str(&format!(
            r#"<li><a href="{href}">{kind} · {source}/{ref_id}</a></li>"#,
            href = encode_double_quoted_attribute(&href),
            kind = encode_text(common.kind.as_str()),
            source = encode_text(&common.source),
            ref_id = encode_text(&common.ref_id),
        ));
    }

    format!(
        r#"<div id="entry-modal-container">
     <dialog id="entry-modal" open>
       <article class="entry">
         <header>
           <h2>Multiple entries found</h2>
           <button type="button" data-on-click__prevent="@get('/-/entry/close')"
 onclick="this.closest('dialog')?.close()">Close</button>
         </header>
         <p>Multiple entries have this name. Choose one:</p>
         <ul>{list}</ul>
       </article>
     </dialog>
   </div>"#,
    )
}

fn render_entry_detail(document: &SearchDocument, config: &AppConfig) -> String {
    match document {
        SearchDocument::Option(option) => {
            let mut html = String::new();

            if let Some(description) = &option.description {
                html.push_str(&section(
                    "Description",
                    &format!("<p>{}</p>", encode_text(description)),
                ));
            }

            if let Some(option_type) = &option.option_type {
                html.push_str(&field("Type", option_type));
            }

            if let Some(default) = &option.default {
                html.push_str(&json_section("Default", default));
            }

            if let Some(example) = &option.example {
                html.push_str(&json_section("Example", example));
            }

            if let Some(related_packages) = &option.related_packages {
                html.push_str(&section(
                    "Related packages",
                    &format!("<p>{}</p>", encode_text(related_packages)),
                ));
            }

            let flags = [
                ("Read only", option.read_only),
                ("Internal", option.internal),
                ("Visible", option.visible),
            ]
            .into_iter()
            .filter_map(|(name, value)| value.map(|value| format!("{name}: {value}")))
            .collect::<Vec<_>>();

            if !flags.is_empty() {
                html.push_str(&section(
                    "Flags",
                    &format!(
                        "<ul>{}</ul>",
                        flags
                            .iter()
                            .map(|flag| format!("<li>{}</li>", encode_text(flag)))
                            .collect::<String>()
                    ),
                ));
            }

            if !option.declarations.is_empty() {
                let resolver =
                    source_link_config_for_document(config, &option.common).map(|config| {
                        SourceLinkResolver::new(config, option.common.revision.as_deref())
                    });

                let mut items = String::new();

                for declaration in &option.declarations {
                    let label = encode_text(&declaration.name);

                    if let Some(url) = resolver
                        .as_ref()
                        .and_then(|resolver| resolver.resolve_declaration(declaration))
                    {
                        items.push_str(&format!(
                            r#"<li><a href="{href}" rel="noreferrer">{label}</a></li>"#,
                            href = encode_double_quoted_attribute(&url),
                        ));
                    } else {
                        items.push_str(&format!("<li>{label}</li>"));
                    }
                }

                html.push_str(&section("Declarations", &format!("<ul>{items}</ul>")));
            }

            html
        }

        SearchDocument::Package(package) => {
            let mut html = String::new();

            let mut summary = Vec::new();

            if let Some(pname) = &package.pname {
                summary.push(format!("pname: {}", encode_text(pname)));
            }

            if let Some(version) = &package.version {
                summary.push(format!("version: {}", encode_text(version)));
            }

            if let Some(main_program) = &package.main_program {
                summary.push(format!("main program: {}", encode_text(main_program)));
            }

            if let Some(broken) = package.broken {
                summary.push(format!("broken: {broken}"));
            }

            if !summary.is_empty() {
                html.push_str(&section(
                    "Package",
                    &format!(
                        "<ul>{}</ul>",
                        summary
                            .iter()
                            .map(|item| format!("<li>{item}</li>"))
                            .collect::<String>()
                    ),
                ));
            }

            if let Some(description) = &package.description {
                html.push_str(&section(
                    "Description",
                    &format!("<p>{}</p>", encode_text(description)),
                ));
            }

            if let Some(long_description) = &package.long_description {
                html.push_str(&section(
                    "Long description",
                    &format!("<p>{}</p>", encode_text(long_description)),
                ));
            }

            if !package.homepages.is_empty() {
                html.push_str(&string_links_section("Homepages", &package.homepages));
            }

            if !package.platforms.is_empty() {
                html.push_str(&strings_section("Platforms", &package.platforms));
            }

            if !package.licenses.is_empty() {
                html.push_str(&licenses_section(&package.licenses));
            }

            if !package.maintainers.is_empty() {
                html.push_str(&maintainers_section(&package.maintainers));
            }

            if let Some(position) = &package.position {
                let resolver =
                    source_link_config_for_document(config, &package.common).map(|config| {
                        SourceLinkResolver::new(config, package.common.revision.as_deref())
                    });

                if let Some(url) = resolver
                    .as_ref()
                    .and_then(|resolver| resolver.resolve_package_position(position))
                {
                    html.push_str(&section(
                        "Source",
                        &format!(
                            r#"<p><a href="{href}" rel="noreferrer">{label}</a></p>"#,
                            href = encode_double_quoted_attribute(&url),
                            label = encode_text(position),
                        ),
                    ));
                } else {
                    html.push_str(&field("Source", position));
                }
            }

            html
        }
    }
}

fn section(title: &str, body: &str) -> String {
    format!(
        r#"<section class="entry-section"><h3>{}</h3>{}</section>"#,
        encode_text(title),
        body
    )
}

fn field(name: &str, value: &str) -> String {
    section(name, &format!("<p>{}</p>", encode_text(value)))
}

fn json_section(name: &str, value: &serde_json::Value) -> String {
    let pretty = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());

    section(name, &format!("<pre>{}</pre>", encode_text(&pretty)))
}

fn strings_section(name: &str, values: &[String]) -> String {
    section(
        name,
        &format!(
            "<ul>{}</ul>",
            values
                .iter()
                .map(|value| format!("<li>{}</li>", encode_text(value)))
                .collect::<String>()
        ),
    )
}

fn string_links_section(name: &str, values: &[String]) -> String {
    section(
        name,
        &format!(
            "<ul>{}</ul>",
            values
                .iter()
                .map(|value| {
                    if value.starts_with("http://") || value.starts_with("https://") {
                        format!(
                            r#"<li><a href="{href}" rel="noreferrer">{label}</a></li>"#,
                            href = encode_double_quoted_attribute(value),
                            label = encode_text(value),
                        )
                    } else {
                        format!("<li>{}</li>", encode_text(value))
                    }
                })
                .collect::<String>()
        ),
    )
}

fn licenses_section(licenses: &[License]) -> String {
    section(
        "Licenses",
        &format!(
            "<ul>{}</ul>",
            licenses
                .iter()
                .map(|license| {
                    let label = license
                        .spdx_id
                        .as_deref()
                        .or(license.name.as_deref())
                        .or(license.full_name.as_deref())
                        .unwrap_or("unknown");

                    if let Some(url) = &license.url {
                        format!(
                            r#"<li><a href="{href}" rel="noreferrer">{label}</a></li>"#,
                            href = encode_double_quoted_attribute(url),
                            label = encode_text(label),
                        )
                    } else {
                        format!("<li>{}</li>", encode_text(label))
                    }
                })
                .collect::<String>()
        ),
    )
}

fn maintainers_section(maintainers: &[Maintainer]) -> String {
    section(
        "Maintainers",
        &format!(
            "<ul>{}</ul>",
            maintainers
                .iter()
                .map(|maintainer| {
                    let label = maintainer
                        .name
                        .as_deref()
                        .or(maintainer.github.as_deref())
                        .or(maintainer.email.as_deref())
                        .unwrap_or("unknown");

                    format!("<li>{}</li>", encode_text(label))
                })
                .collect::<String>()
        ),
    )
}

fn summary_for_document(document: &SearchDocument) -> Option<&str> {
    match document {
        SearchDocument::Option(option) => {
            option.description.as_deref().and_then(first_non_empty_line)
        }
        SearchDocument::Package(package) => package
            .description
            .as_deref()
            .and_then(first_non_empty_line),
    }
}

fn first_non_empty_line(value: &str) -> Option<&str> {
    value.lines().map(str::trim).find(|line| !line.is_empty())
}

fn first_source_link(document: &SearchDocument, config: &AppConfig) -> Option<String> {
    let common = document.common();
    let source_links = source_link_config_for_document(config, common)?;
    let resolver = SourceLinkResolver::new(source_links, common.revision.as_deref());

    match document {
        SearchDocument::Option(option) => option
            .declarations
            .iter()
            .find_map(|declaration| resolver.resolve_declaration(declaration)),
        SearchDocument::Package(package) => package
            .position
            .as_deref()
            .and_then(|position| resolver.resolve_package_position(position)),
    }
}

fn source_link_config_for_document<'a>(
    config: &'a AppConfig,
    common: &CommonDoc,
) -> Option<&'a SourceLinkConfig> {
    let source = config.sources.get(&common.source)?;

    let ref_config = source
        .refs
        .iter()
        .find(|ref_config| ref_config.id == common.ref_id)?;

    ref_config.source_links.as_ref()
}

fn resolve_entry_ref(config: &AppConfig, source_id: &str, ref_id: Option<&str>) -> Result<String> {
    if let Some(ref_id) = ref_id.and_then(non_empty) {
        return Ok(ref_id.to_owned());
    }

    let source = config
        .sources
        .get(source_id)
        .with_context(|| format!("unknown source {source_id:?}"))?;

    source
        .default_ref
        .clone()
        .with_context(|| format!("source {source_id:?} has no default ref"))
}

fn parse_document_kind(value: Option<&str>) -> std::result::Result<Option<DocumentKind>, String> {
    match value.and_then(non_empty) {
        None => Ok(None),
        Some("option") => Ok(Some(DocumentKind::Option)),
        Some("package") => Ok(Some(DocumentKind::Package)),
        Some("app") => Ok(Some(DocumentKind::App)),
        Some("service") => Ok(Some(DocumentKind::Service)),
        Some(other) => Err(format!("unknown entry kind {other:?}")),
    }
}

fn source_path(source: &str) -> String {
    format!("/{}", encode_path(source))
}

fn entry_url(
    source: &str,
    entry: &str,
    ref_id: Option<&str>,
    kind: Option<&str>,
    q: Option<&str>,
) -> String {
    let mut url = format!("{}/{}", source_path(source), encode_path(entry));

    let query = query_string([("q", q), ("ref", ref_id), ("kind", kind)]);

    if !query.is_empty() {
        url.push('?');
        url.push_str(&query);
    }

    url
}

fn entry_url_for_request(request: &PageRequest, entry: &str) -> String {
    let source = request.source.as_deref().unwrap_or("");
    entry_url(
        source,
        entry,
        request.query.ref_id.as_deref(),
        request.query.kind.as_deref(),
        request.query.q.as_deref(),
    )
}

fn entry_events_url(source: &str, entry: &str, ref_id: Option<&str>, kind: Option<&str>) -> String {
    let mut url = "/-/entry/events?".to_owned();

    url.push_str(&query_string([
        ("source", Some(source)),
        ("entry", Some(entry)),
        ("ref", ref_id),
        ("kind", kind),
    ]));

    url
}

fn query_string<const N: usize>(pairs: [(&str, Option<&str>); N]) -> String {
    pairs
        .into_iter()
        .filter_map(|(key, value)| {
            let value = value.and_then(non_empty)?;

            Some(format!("{}={}", encode_query(key), encode_query(value)))
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn encode_path(value: &str) -> String {
    urlencoding::encode(value).into_owned()
}

fn encode_query(value: &str) -> String {
    urlencoding::encode(value).into_owned()
}

fn decode_path_value(value: &str) -> Option<String> {
    urlencoding::decode(value)
        .ok()
        .map(|value| value.into_owned())
}

fn js_string(value: &str) -> String {
    serde_json::to_string(value).expect("serializing string cannot fail")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use nix_search_config::AppConfig;
    use nix_search_core::{Declaration, OptionDoc, SearchDocument};

    use super::{AppState, PageQuery, PageRequest, first_source_link, render_page};

    #[test]
    fn escapes_query_in_search_input() {
        let state = test_state();
        let request = test_request(PageQuery {
            q: Some(r#"<script>alert("x")</script>"#.to_owned()),
            ..PageQuery::default()
        });

        let html = render_page(&state, &request, None);

        assert!(!html.contains(r#"<script>alert("x")</script>"#));
        assert!(html.contains("&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt;"));
    }

    #[test]
    fn renders_empty_results_message() {
        let state = test_state();
        let request = test_request(PageQuery::default());

        let html = render_page(&state, &request, None);

        assert!(html.contains("Enter a search query."));
    }

    #[test]
    fn resolves_source_link_when_available() {
        let config = test_config();
        let mut option = OptionDoc::new(
            &nix_search_core::IngestContext {
                source: "fixtures".into(),
                ref_id: "small".into(),
                revision: Some("abc123".into()),
                repo: None,
            },
            "programs.fixture.enable",
        );

        option.declarations.push(Declaration {
            name: "module.nix:4".into(),
            url: None,
        });

        let document = SearchDocument::Option(option);

        assert_eq!(
            first_source_link(&document, &config).as_deref(),
            Some("https://github.com/example/repo/blob/abc123/module.nix#L4")
        );
    }

    fn test_state() -> AppState {
        AppState {
            config: Arc::new(test_config()),
            index_path: Arc::new(PathBuf::from("./data/indexes/missing-test-index")),
        }
    }

    fn test_request(query: PageQuery) -> PageRequest {
        PageRequest {
            source: None,
            entry: None,
            query,
        }
    }

    fn test_config() -> AppConfig {
        nix_search_test_support::app_config("./data/indexes")
    }
}
