use std::convert::Infallible;

use anyhow::{Context, Result};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, Uri, header};
use axum::response::{Html, IntoResponse, Sse, sse::Event};
use datastar::prelude::{ExecuteScript, PatchElements};
use futures_util::stream;
use serde::Deserialize;
use url::Url;

use nixsearch_config::app::AppConfig;
use nixsearch_index::search::{
    EntryLookup, EntryLookupResult, SearchIndex, SearchOptions, SearchResult, SearchScope,
};

use crate::AppState;
use crate::DEFAULT_LIMIT;
use crate::request::{
    PageQuery, PageRequest, PageState, decode_path_value, non_empty, normalized_query,
    page_request_from_public_url, page_state, parse_document_kind, resolve_entry_ref,
    search_scopes_for_state,
};
use crate::scripts::dialog_reconcile_script;
use crate::templates::{self, layout::PageUrls, modal::EntryData};

#[derive(Debug, Clone, Deserialize)]
pub struct StateQuery {
    url: String,
    previous_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MoreQuery {
    url: String,
    offset: usize,
}

pub async fn health() -> &'static str {
    "ok"
}

pub async fn favicon() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "image/x-icon")],
        include_bytes!("../favicon.ico"),
    )
}

pub async fn apple_touch_icon() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "image/png")],
        include_bytes!("../apple-touch-icon.png"),
    )
}

pub async fn root_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(query): Query<PageQuery>,
) -> impl IntoResponse {
    render_full_page_response(
        &state,
        page_urls(&state, &headers, &uri),
        PageRequest {
            source: None,
            entry: None,
            query,
        },
    )
}

pub async fn source_page(
    State(state): State<AppState>,
    Path(source): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    Query(query): Query<PageQuery>,
) -> impl IntoResponse {
    render_full_page_response(
        &state,
        page_urls(&state, &headers, &uri),
        PageRequest {
            source: Some(source),
            entry: None,
            query,
        },
    )
}

pub async fn entry_page(
    State(state): State<AppState>,
    Path((source, entry)): Path<(String, String)>,
    headers: HeaderMap,
    uri: Uri,
    Query(query): Query<PageQuery>,
) -> impl IntoResponse {
    let entry = decode_path_value(&entry).unwrap_or(entry);

    render_full_page_response(
        &state,
        page_urls(&state, &headers, &uri),
        PageRequest {
            source: Some(source),
            entry: Some(entry),
            query,
        },
    )
}

pub async fn state_events(
    State(state): State<AppState>,
    Query(query): Query<StateQuery>,
) -> impl IntoResponse {
    let request = match page_request_from_public_url(&query.url) {
        Ok(request) => request,
        Err(error) => {
            let html = templates::results::render_error(&error).into_string();
            let event = PatchElements::new(html).write_as_axum_sse_event();
            let events: Vec<std::result::Result<Event, Infallible>> = vec![Ok(event)];
            return Sse::new(stream::iter(events));
        }
    };

    let page_state = page_state(&state.config, &request);
    let patch_results = should_patch_results(&state, query.previous_url.as_deref(), &request);
    let needs_search = patch_results && normalized_query(&request.query).is_some();
    let needs_entry = page_state.detail.is_some();
    let index = if needs_search || needs_entry {
        Some(open_search_index(&state).map_err(|error| format!("{error:#}")))
    } else {
        None
    };

    let results_html = if patch_results {
        if normalized_query(&request.query).is_none() {
            Some(templates::home::render(&state, &request, &page_state).into_string())
        } else {
            let search_result = match &index {
                Some(Ok(index)) => run_search_with_index(
                    &state.config,
                    index,
                    &request,
                    &page_state,
                    0,
                    DEFAULT_LIMIT,
                )
                .map_err(|error| format!("{error:#}")),
                Some(Err(error)) => Err(error.clone()),
                None => unreachable!("search result requested without opening the index"),
            };

            Some(match &search_result {
                Ok(result) => templates::results::render(
                    &page_state,
                    &result.hits,
                    result.total,
                    &state.config,
                )
                .into_string(),
                Err(error) => templates::results::render_error(&format!("{error:#}")).into_string(),
            })
        }
    } else {
        None
    };

    let entry = entry_data_from_index(&state.config, &page_state, index.as_ref());
    let modal_html = templates::modal::render(&state.config, &page_state, &entry).into_string();

    let mut events: Vec<std::result::Result<Event, Infallible>> = Vec::new();

    if let Some(results_html) = results_html {
        events.push(Ok(
            PatchElements::new(results_html).write_as_axum_sse_event()
        ));
    }

    events.push(Ok(PatchElements::new(modal_html).write_as_axum_sse_event()));
    events.push(Ok(
        ExecuteScript::new(dialog_reconcile_script()).write_as_axum_sse_event()
    ));

    Sse::new(stream::iter(events))
}

fn should_patch_results(
    state: &AppState,
    previous_url: Option<&str>,
    request: &PageRequest,
) -> bool {
    let Some(previous_url) = previous_url.and_then(non_empty) else {
        return true;
    };

    match page_request_from_public_url(previous_url) {
        Ok(previous_request) => {
            let previous_state = page_state(&state.config, &previous_request);
            let next_state = page_state(&state.config, request);

            previous_state.q != next_state.q
                || previous_state.source_filter != next_state.source_filter
                || previous_state.source_ref != next_state.source_ref
                || previous_state.active_ref_set() != next_state.active_ref_set()
        }
        Err(_) => true,
    }
}

pub async fn more_results(
    State(state): State<AppState>,
    Query(query): Query<MoreQuery>,
) -> impl IntoResponse {
    let request = match page_request_from_public_url(&query.url) {
        Ok(request) => request,
        Err(error) => {
            return Json(serde_json::json!({
                "error": error
            }));
        }
    };

    let search_result = run_search(&state, &request, query.offset, DEFAULT_LIMIT);

    match search_result {
        Ok(result) => {
            let rows_html = templates::results::render_rows_only(
                &request,
                &result.hits,
                &state.config,
                query.offset,
            );
            Json(serde_json::json!({
                "rows": rows_html,
                "total": result.total
            }))
        }
        Err(error) => Json(serde_json::json!({
            "error": format!("{error:#}")
        })),
    }
}

fn render_full_page_response(
    state: &AppState,
    page_urls: PageUrls,
    request: PageRequest,
) -> Html<String> {
    let page_state = page_state(&state.config, &request);
    let page = request.query.page.unwrap_or(1).max(1);
    let offset = (page - 1) * DEFAULT_LIMIT;
    let needs_search = normalized_query(&request.query).is_some();
    let needs_entry = page_state.detail.is_some();
    let index = if needs_search || needs_entry {
        Some(open_search_index(state).map_err(|error| format!("{error:#}")))
    } else {
        None
    };
    let search_result = if needs_search {
        match &index {
            Some(Ok(index)) => run_search_with_index(
                &state.config,
                index,
                &request,
                &page_state,
                offset,
                DEFAULT_LIMIT,
            )
            .map_err(|error| format!("{error:#}")),
            Some(Err(error)) => Err(error.clone()),
            None => unreachable!("search result requested without opening the index"),
        }
    } else {
        Ok(empty_search_result())
    };
    let entry = entry_data_from_index(&state.config, &page_state, index.as_ref());

    let view = match &search_result {
        Ok(result) => Ok(result),
        Err(error) => Err(error.as_str()),
    };

    let markup =
        templates::layout::render_full_page(state, &request, &page_state, &page_urls, view, &entry);
    Html(markup.into_string())
}

fn entry_data_from_index(
    config: &AppConfig,
    page_state: &PageState,
    index: Option<&Result<SearchIndex, String>>,
) -> EntryData {
    let Some(detail) = page_state.detail.as_ref() else {
        return EntryData::Empty;
    };
    let Some(index) = index else {
        return EntryData::Error("search index was not opened".to_owned());
    };
    let index = match index {
        Ok(index) => index,
        Err(error) => return EntryData::Error(error.clone()),
    };

    let lookup_ref = detail
        .ref_id
        .as_deref()
        .or(page_state.source_ref.as_deref())
        .or_else(|| {
            page_state
                .active_ref_set()
                .and_then(|ref_set| config.first_ref_for_ref_set_source(ref_set, &detail.source))
        });
    let ref_id = match resolve_entry_ref(config, &detail.source, lookup_ref) {
        Ok(ref_id) => ref_id,
        Err(error) => return EntryData::Error(format!("{error:#}")),
    };
    let kind = match parse_document_kind(detail.kind.as_deref()) {
        Ok(kind) => kind,
        Err(error) => return EntryData::Error(error),
    };

    match index
        .find_entry(EntryLookup {
            source: detail.source.clone(),
            ref_id,
            name: detail.entry.clone(),
            kind,
        })
        .map_err(|error| format!("{error:#}"))
    {
        Ok(EntryLookupResult::Found(document)) => EntryData::Found(document),
        Ok(EntryLookupResult::NotFound) => EntryData::NotFound,
        Ok(EntryLookupResult::Ambiguous(documents)) => EntryData::Ambiguous(documents),
        Err(error) => EntryData::Error(error),
    }
}

fn page_urls(state: &AppState, headers: &HeaderMap, uri: &Uri) -> PageUrls {
    let path = uri.path();
    let query = uri.query();

    if let Some(public_url) = state.config.server.public_url.as_deref()
        && let Ok(base) = Url::parse(public_url)
    {
        return page_urls_from_base(base, path, query);
    }

    page_urls_from_headers(headers, path, query)
}

fn page_urls_from_base(mut base: Url, path: &str, query: Option<&str>) -> PageUrls {
    let base_path = base.path().trim_end_matches('/').to_owned();
    let path = if path == "/" {
        if base_path.is_empty() {
            "/".to_owned()
        } else {
            format!("{base_path}/")
        }
    } else {
        format!("{base_path}{path}")
    };

    base.set_path(&path);
    base.set_query(query);
    base.set_fragment(None);

    let current_url = base.to_string();

    base.set_path(&format!("{base_path}/apple-touch-icon.png"));
    base.set_query(None);

    PageUrls {
        current_url,
        image_url: base.to_string(),
    }
}

fn page_urls_from_headers(headers: &HeaderMap, path: &str, query: Option<&str>) -> PageUrls {
    let forwarded = forwarded_proto_host(headers);
    let proto = forwarded
        .as_ref()
        .and_then(|(proto, _)| proto.as_deref())
        .or_else(|| first_header_value(headers, "x-forwarded-proto"))
        .unwrap_or("http");
    let host = forwarded
        .as_ref()
        .and_then(|(_, host)| host.as_deref())
        .or_else(|| first_header_value(headers, header::HOST.as_str()))
        .unwrap_or("localhost");
    let origin = format!("{proto}://{host}");
    let path_and_query = match query {
        Some(query) => format!("{path}?{query}"),
        None => path.to_owned(),
    };

    PageUrls {
        current_url: format!("{origin}{path_and_query}"),
        image_url: format!("{origin}/apple-touch-icon.png"),
    }
}

fn forwarded_proto_host(headers: &HeaderMap) -> Option<(Option<String>, Option<String>)> {
    let header = first_header_value(headers, "forwarded")?;
    let first = header.split(',').next().unwrap_or(header);
    let mut proto = None;
    let mut host = None;

    for part in first.split(';') {
        let Some((key, value)) = part.trim().split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        match key.trim().to_ascii_lowercase().as_str() {
            "proto" => proto = Some(value.to_owned()),
            "host" => host = Some(value.to_owned()),
            _ => {}
        }
    }

    (proto.is_some() || host.is_some()).then_some((proto, host))
}

fn first_header_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)?
        .to_str()
        .ok()?
        .split(',')
        .next()
        .map(str::trim)
        .and_then(non_empty)
}

fn run_search(
    state: &AppState,
    request: &PageRequest,
    offset: usize,
    limit: usize,
) -> Result<SearchResult> {
    if normalized_query(&request.query).is_none() {
        return Ok(empty_search_result());
    };

    let index = open_search_index(state)?;
    let page_state = page_state(&state.config, request);

    run_search_with_index(&state.config, &index, request, &page_state, offset, limit)
}

fn open_search_index(state: &AppState) -> Result<SearchIndex> {
    let index_path = state
        .index_path
        .read()
        .expect("index path lock poisoned")
        .clone();

    SearchIndex::open(&index_path)
        .with_context(|| format!("failed to open current search index {}", index_path))
}

fn empty_search_result() -> SearchResult {
    SearchResult {
        hits: Vec::new(),
        total: 0,
    }
}

fn run_search_with_index(
    config: &AppConfig,
    index: &SearchIndex,
    request: &PageRequest,
    page_state: &PageState,
    offset: usize,
    limit: usize,
) -> Result<SearchResult> {
    let Some(q) = normalized_query(&request.query) else {
        return Ok(empty_search_result());
    };

    let scopes = search_scopes_for_state(config, page_state)
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
            limit,
            offset,
            scopes,
        })
        .context("search failed")
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};
    use url::Url;

    use super::{page_urls_from_base, page_urls_from_headers};

    #[test]
    fn page_urls_use_public_url_origin() {
        let urls = page_urls_from_base(
            Url::parse("https://search.example.com/").unwrap(),
            "/nixpkgs/git",
            Some("q=git"),
        );

        assert_eq!(
            urls.current_url,
            "https://search.example.com/nixpkgs/git?q=git"
        );
        assert_eq!(
            urls.image_url,
            "https://search.example.com/apple-touch-icon.png"
        );
    }

    #[test]
    fn page_urls_fall_back_to_forwarded_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "forwarded",
            HeaderValue::from_static("for=127.0.0.1;proto=https;host=nixsearch.example.com"),
        );
        let urls = page_urls_from_headers(&headers, "/", None);

        assert_eq!(urls.current_url, "https://nixsearch.example.com/");
        assert_eq!(
            urls.image_url,
            "https://nixsearch.example.com/apple-touch-icon.png"
        );
    }

    #[test]
    fn page_urls_fall_back_to_host_and_proto_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("localhost:3000"));
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        let urls = page_urls_from_headers(&headers, "/nixpkgs", Some("q=git"));

        assert_eq!(urls.current_url, "https://localhost:3000/nixpkgs?q=git");
        assert_eq!(
            urls.image_url,
            "https://localhost:3000/apple-touch-icon.png"
        );
    }
}
