use maud::{DOCTYPE, Markup, PreEscaped, html};

use nixsearch_config::app::AppConfig;
use nixsearch_index::search::SearchResult;

use crate::AppState;
use crate::RECONCILE_EVENTS_URL;
use crate::request::{PageRequest, SourceFilter, normalized_query};
use crate::scripts::navigation_script;
use crate::urls::source_path;

use super::footer;
use super::home;
use super::modal;
use super::results;
use super::search;
use super::source_tag;

static CSS: &str = include_str!("../../style.css");

pub fn render_full_page(
    state: &AppState,
    request: &PageRequest,
    search_result: Result<&SearchResult, &str>,
) -> Markup {
    let q = request.query.q.as_deref().unwrap_or("");
    let source_filter = SourceFilter::from_request(request);

    let results_markup = match search_result {
        Ok(result) if normalized_query(&request.query).is_some() => {
            results::render(request, &result.hits, result.total, &state.config)
        }
        Ok(_) => home::render(state, request),
        Err(error) => results::render_error(error),
    };

    let modal_markup = modal::render(state, request);
    let source_metadata = source_metadata_json(&state.config);
    let page_title = title_for(&state.config, request, &source_filter);

    let form_action = match &source_filter {
        SourceFilter::All => "/".to_owned(),
        SourceFilter::Named(source) => source_path(source),
    };
    let logo_style = match &source_filter {
        SourceFilter::All => None,
        SourceFilter::Named(source) => Some(format!(
            "--logo-accent: {};",
            source_tag::color_for_source(&state.config, source)
        )),
    };

    let reconcile_attr = format!(
        "@get('{RECONCILE_EVENTS_URL}?url=' + encodeURIComponent(location.pathname + location.search) + '&previous_url=' + encodeURIComponent(window.nixsearchPreviousUrl || ''))"
    );

    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (page_title) }
                link rel="icon" type="image/x-icon" href="/favicon.ico";
                script type="module"
                    src="https://cdn.jsdelivr.net/gh/starfederation/datastar@main/bundles/datastar.js" {}
                style { (PreEscaped(CSS)) }
                noscript {
                    style { ".js-ref-radios { display: none; } dialog#entry-modal { display: block; z-index: 201; } .modal-backdrop { display: block; position: fixed; inset: 0; z-index: 200; background: rgb(0 0 0 / 0.6); }" }
                }
            }
            body data-on:nixsearch-reconcile__window=(reconcile_attr) {
                header.header {
                    div.header-inner {
                        a.site-title href="/" aria-label="nixsearch" style=[logo_style] {
                            span.site-title-nix { "nix" }
                            span.site-title-search { "search" }
                        }
                        (search::render_form(
                            &state.config,
                            &source_filter,
                            &form_action,
                            q,
                            request.query.ref_id.as_deref().unwrap_or(""),
                            request.query.ref_set.as_deref().unwrap_or(""),
                        ))
                    }
                }
                main.main {
                    (results_markup)
                    (modal_markup)
                }
                (footer::render_footer(state))

                script #source-metadata type="application/json" {
                    (PreEscaped(&source_metadata))
                }
                script { (PreEscaped(navigation_script())) }
            }
        }
    }
}

fn title_for(config: &AppConfig, request: &PageRequest, source_filter: &SourceFilter) -> String {
    let mut parts = Vec::new();

    if let Some(q) = normalized_query(&request.query) {
        parts.push(q.to_owned());
    }

    if let SourceFilter::Named(source_id) = source_filter {
        parts.push(source_display_name(config, source_id).to_owned());
    }

    parts.push("nixsearch".to_owned());
    parts.join(" · ")
}

fn source_display_name<'a>(config: &'a AppConfig, source_id: &'a str) -> &'a str {
    config
        .sources
        .get(source_id)
        .and_then(|source| source.name.as_deref())
        .unwrap_or(source_id)
}

fn source_metadata_json(config: &AppConfig) -> String {
    let sources: Vec<String> = config
           .sources
           .iter()
           .map(|(id, source)| {
               let name = source.name.as_deref().unwrap_or(id);
               let color = source_tag::color_for_source(config, id);
               let refs: Vec<&str> = source.refs.iter().map(|r| r.id.as_str()).collect();
               let refs_json = refs
                   .iter()
                   .map(|r| format!("\"{}\"", r.replace('"', "\\\"")))
                   .collect::<Vec<_>>()
                   .join(",");
               let default_ref = source.default_ref.as_deref().unwrap_or("");

               format!(
                   r#"{{"id":"{id}","name":"{name}","color":"{color}","refs":[{refs_json}],"defaultRef":"{default_ref}"}}"#,
                   id = id.replace('"', "\\\""),
                   name = name.replace('"', "\\\""),
                   color = color.replace('"', "\\\""),
                   default_ref = default_ref.replace('"', "\\\""),
               )
            })
            .collect();

    let ref_sets = config
        .ref_sets
        .keys()
        .map(|ref_set| format!("\"{}\"", ref_set.replace('"', "\\\"")))
        .collect::<Vec<_>>()
        .join(",");
    let default_ref_set = config.default_ref_set().unwrap_or("");

    format!(
        r#"{{"sources":[{}],"refSets":[{}],"defaultRefSet":"{}"}}"#,
        sources.join(","),
        ref_sets,
        default_ref_set.replace('"', "\\\""),
    )
}

#[cfg(test)]
mod tests {
    use nixsearch_test_support::{SOURCE_FIXTURES, app_config, utf8_path_buf};
    use tempfile::tempdir;

    use crate::request::{PageQuery, PageRequest, SourceFilter};

    use super::title_for;

    fn config() -> nixsearch_config::app::AppConfig {
        let tempdir = tempdir().unwrap();
        app_config(utf8_path_buf(tempdir.path().join("indexes")))
    }

    #[test]
    fn title_includes_query_and_named_source() {
        let config = config();
        let request = PageRequest {
            source: Some(SOURCE_FIXTURES.to_owned()),
            entry: None,
            query: PageQuery {
                q: Some("git".to_owned()),
                ..PageQuery::default()
            },
        };

        assert_eq!(
            title_for(
                &config,
                &request,
                &SourceFilter::Named(SOURCE_FIXTURES.to_owned())
            ),
            "git · Fixtures · nixsearch"
        );
    }

    #[test]
    fn title_omits_all_source_filter() {
        let config = config();
        let request = PageRequest {
            source: None,
            entry: None,
            query: PageQuery {
                q: Some("git".to_owned()),
                ..PageQuery::default()
            },
        };

        assert_eq!(
            title_for(&config, &request, &SourceFilter::All),
            "git · nixsearch"
        );
    }

    #[test]
    fn title_includes_named_source_without_query() {
        let config = config();
        let request = PageRequest {
            source: Some(SOURCE_FIXTURES.to_owned()),
            entry: None,
            query: PageQuery::default(),
        };

        assert_eq!(
            title_for(
                &config,
                &request,
                &SourceFilter::Named(SOURCE_FIXTURES.to_owned())
            ),
            "Fixtures · nixsearch"
        );
    }
}
