use std::convert::Infallible;

use anyhow::{Context, Result};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Sse, sse::Event};
use datastar::prelude::{ExecuteScript, PatchElements};
use futures_util::stream;
use serde::Deserialize;

use nix_search_index::{SearchIndex, SearchOptions, SearchResult, SearchScope};

use crate::AppState;
use crate::DEFAULT_LIMIT;
use crate::request::{
    LinkOrigin, PageQuery, PageRequest, decode_path_value, non_empty, normalized_query,
    page_request_from_public_url,
};
use crate::scripts::dialog_reconcile_script;
use crate::templates;

#[derive(Debug, Clone, Deserialize)]
pub struct StateQuery {
    url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MoreQuery {
    url: String,
    offset: usize,
}

pub async fn health() -> &'static str {
    "ok"
}

pub async fn root_page(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> impl IntoResponse {
    render_full_page_response(
        &state,
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
    Query(query): Query<PageQuery>,
) -> impl IntoResponse {
    render_full_page_response(
        &state,
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
    Query(query): Query<PageQuery>,
) -> impl IntoResponse {
    let entry = decode_path_value(&entry).unwrap_or(entry);

    render_full_page_response(
        &state,
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

    let search_result = run_search(&state, &request, 0);

    let results_html = match &search_result {
        Ok(result) => {
            templates::results::render(&request, &result.hits, result.total, &state.config)
                .into_string()
        }
        Err(error) => templates::results::render_error(&format!("{error:#}")).into_string(),
    };

    let modal_html = templates::modal::render(&state, &request).into_string();

    let events: Vec<std::result::Result<Event, Infallible>> = vec![
        Ok(PatchElements::new(results_html).write_as_axum_sse_event()),
        Ok(PatchElements::new(modal_html).write_as_axum_sse_event()),
        Ok(ExecuteScript::new(dialog_reconcile_script()).write_as_axum_sse_event()),
    ];

    Sse::new(stream::iter(events))
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

    let search_result = run_search(&state, &request, query.offset);

    match search_result {
        Ok(result) => {
            let rows_html =
                templates::results::render_rows_only(&request, &result.hits, &state.config);
            let sentinel_html = templates::results::render_sentinel_update(
                &result.hits,
                query.offset,
                result.total,
            );

            Json(serde_json::json!({
                "rows": rows_html,
                "sentinel": sentinel_html
            }))
        }
        Err(error) => Json(serde_json::json!({
            "error": format!("{error:#}")
        })),
    }
}

fn render_full_page_response(state: &AppState, request: PageRequest) -> Html<String> {
    let search_result = run_search(state, &request, 0);
    let error_message = search_result.as_ref().err().map(|e| format!("{e:#}"));

    let view = match (&search_result, &error_message) {
        (Ok(result), _) => Ok(result),
        (Err(_), Some(error)) => Err(error.as_str()),
        (Err(_), None) => unreachable!(),
    };

    let markup = templates::layout::render_full_page(state, &request, view);
    Html(markup.into_string())
}

fn run_search(state: &AppState, request: &PageRequest, offset: usize) -> Result<SearchResult> {
    let Some(q) = normalized_query(&request.query) else {
        return Ok(SearchResult {
            hits: Vec::new(),
            total: 0,
        });
    };

    let index = SearchIndex::open(&*state.index_path).with_context(|| {
        format!(
            "failed to open current search index {}",
            state.index_path.display()
        )
    })?;

    // source=all overrides path-based source filter
    let effective_source = match request.query.source {
        Some(LinkOrigin::All) => None,
        _ => request.source.as_deref().and_then(non_empty),
    };

    let scopes = state
        .config
        .resolve_search_scopes(
            effective_source,
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
            offset,
            scopes,
        })
        .context("search failed")
}
