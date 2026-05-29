use maud::{Markup, html};

use nixsearch_config::app::AppConfig;
use nixsearch_core::document::SearchDocument;

use crate::request::PageState;
use crate::urls::{close_url_for_state, entry_url_for_document, source_url_from_state};

use super::{detail, source_tag};

#[derive(Debug, Clone, PartialEq)]
pub enum EntryData {
    Empty,
    Found(Box<SearchDocument>),
    NotFound,
    Ambiguous(Vec<SearchDocument>),
    Error(String),
}

impl EntryData {
    pub fn document(&self) -> Option<&SearchDocument> {
        match self {
            Self::Found(document) => Some(document),
            Self::Empty | Self::NotFound | Self::Ambiguous(_) | Self::Error(_) => None,
        }
    }
}

pub fn render(config: &AppConfig, page_state: &PageState, entry: &EntryData) -> Markup {
    match entry {
        EntryData::Empty => render_empty(),
        EntryData::Found(document) => render_entry(page_state, document, config),
        EntryData::NotFound => render_error(config, page_state, "Entry not found."),
        EntryData::Ambiguous(documents) => render_ambiguous(page_state, documents, config),
        EntryData::Error(error) => render_error(config, page_state, error),
    }
}

fn render_empty() -> Markup {
    html! {
        div #entry-modal-container {}
    }
}

fn render_entry(state: &PageState, document: &SearchDocument, config: &AppConfig) -> Markup {
    let common = document.common();
    let close_href = close_url_for_state(config, state);
    let source_color = source_tag::color_for_source(config, &common.source);
    let source_href = source_url_from_state(config, state, &common.source);

    html! {
        div #entry-modal-container {
            a.modal-backdrop href=(close_href) aria-label="Close modal" {}
            dialog #entry-modal data-close-url=(close_href) {
                article.entry {
                    header {
                        div {
                            div.entry-title-row {
                                h2 { (common.name) }
                                button.entry-copy
                                    type="button"
                                    data-copy-entry=(common.name)
                                    aria-label="Copy entry name"
                                    title="Copy"
                                    style=(format!("--source-color: {source_color};")) {
                                    svg.copy-icon xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" aria-hidden="true" {
                                        path d="M8 7a3 3 0 0 1 3-3h6a3 3 0 0 1 3 3v6a3 3 0 0 1-3 3h-1v1a3 3 0 0 1-3 3H7a3 3 0 0 1-3-3v-6a3 3 0 0 1 3-3h1V7Zm2 1h3a3 3 0 0 1 3 3v3h1a1 1 0 0 0 1-1V7a1 1 0 0 0-1-1h-6a1 1 0 0 0-1 1v1Zm-3 2a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h6a1 1 0 0 0 1-1v-6a1 1 0 0 0-1-1H7Z" {}
                                    }
                                    svg.check-icon xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" aria-hidden="true" {
                                        path d="M9.55 17.6 4.7 12.75l1.4-1.4 3.45 3.45 8.35-8.35 1.4 1.4-9.75 9.75Z" {}
                                    }
                                }
                            }
                            div.meta {
                                (source_tag::render_link(config, &common.source, &source_href))
                                (common.kind.as_str()) " · " (common.ref_id)
                                @if let Some(revision) = &common.revision {
                                    " · " code { (&revision[..revision.len().min(8)]) }
                                }
                            }
                        }
                        a.entry-close href=(close_href) data-role="entry-close" autofocus { "✕ Close" }
                    }
                    (detail::render(document, config))
                }
            }
        }
    }
}

fn render_error(config: &AppConfig, state: &PageState, message: &str) -> Markup {
    let close_href = close_url_for_state(config, state);

    html! {
        div #entry-modal-container {
            a.modal-backdrop href=(close_href) aria-label="Close modal" {}
            dialog #entry-modal data-close-url=(close_href) {
                article.entry {
                    header {
                        h2 { "Error" }
                        a.entry-close href=(close_href) data-role="entry-close" autofocus { "✕ Close" }
                    }
                    div.results-error { (message) }
                }
            }
        }
    }
}

fn render_ambiguous(state: &PageState, documents: &[SearchDocument], config: &AppConfig) -> Markup {
    let close_href = close_url_for_state(config, state);

    html! {
        div #entry-modal-container {
            a.modal-backdrop href=(close_href) aria-label="Close modal" {}
            dialog #entry-modal data-close-url=(close_href) {
                article.entry {
                    header {
                        h2 { "Multiple entries found" }
                        a.entry-close href=(close_href) data-role="entry-close" autofocus { "✕ Close" }
                    }
                    p { "This name exists in multiple forms. Choose one:" }
                    ul {
                        @for document in documents {
                            @let common = document.common();
                            @let href = entry_url_for_document(config, state, document, Some(common.kind.as_str()), state.page);
                            li {
                                a href=(href) {
                                    (common.kind.as_str()) " · " (common.source) "/" (common.ref_id)
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
