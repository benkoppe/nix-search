use maud::{Markup, html};

use nix_search_config::AppConfig;
use nix_search_core::{CommonDoc, SearchDocument, SourceLinkConfig, SourceLinkResolver};
use nix_search_index::SearchHit;

use crate::request::{LinkOrigin, PageQuery, PageRequest, normalized_query};
use crate::urls::{entry_url_for, ref_id_for_link};

pub fn render(request: &PageRequest, hits: &[SearchHit], config: &AppConfig) -> Markup {
    let Some(q) = normalized_query(&request.query) else {
        return render_empty();
    };

    if hits.is_empty() {
        return html! {
            div #results.status {
                "No results for " strong { (q) } "."
            }
        };
    }

    html! {
        div #results.results aria-live="polite" {
            div.status {
                (hits.len()) " result" @if hits.len() != 1 { "s" }
                " for " strong { (q) } "."
            }
            @for hit in hits {
                (render_hit(request, hit, config))
            }
        }
    }
}

pub fn render_empty() -> Markup {
    html! {
        div #results.status { "Enter a search query." }
    }
}

pub fn render_error(error: &str) -> Markup {
    html! {
        div #results.error {
            strong { "Search failed:" } " " (error)
        }
    }
}

fn render_hit(request: &PageRequest, hit: &SearchHit, config: &AppConfig) -> Markup {
    let common = hit.document.common();
    let summary = summary_for_document(&hit.document);
    let source_link = first_source_link(&hit.document, config);

    let from_scope = if request.source.is_none() {
        Some(LinkOrigin::All)
    } else {
        None
    };

    let entry_href = entry_url_for(
        &common.source,
        &common.name,
        None,
        &PageQuery {
            q: request.query.q.clone(),
            ref_id: ref_id_for_link(config, &common.source, &common.ref_id),
            kind: None,
            source: from_scope,
        },
    );

    html! {
        article.result {
            h2 {
                a href=(entry_href) {
                    code { (common.name) }
                }
            }
            div.meta {
                (common.kind.as_str()) " · " (common.source) "/" (common.ref_id)
                " · score " (format!("{:.3}", hit.score))
            }
            @if let Some(summary) = summary {
                p.summary { (summary) }
            }
            @if let Some(source_link) = source_link {
                a href=(source_link) rel="noreferrer" { "Source" }
            }
        }
    }
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

pub fn source_link_config_for_document<'a>(
    config: &'a AppConfig,
    common: &CommonDoc,
) -> Option<&'a SourceLinkConfig> {
    let source = config.sources.get(&common.source)?;
    let ref_config = source.refs.iter().find(|r| r.id == common.ref_id)?;
    ref_config.source_links.as_ref()
}
