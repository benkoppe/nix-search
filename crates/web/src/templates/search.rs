use maud::{Markup, html};

use nix_search_config::AppConfig;

use crate::request::SourceFilter;

pub fn render_form(
    config: &AppConfig,
    source_filter: &SourceFilter,
    form_action: &str,
    q: &str,
) -> Markup {
    html! {
        form.search action=(form_action) method="get" {
            label {
                "Query"
                input type="search" name="q" value=(q)
                    placeholder="git, programs.git.enable, services.nginx..."
                    autocomplete="off" autofocus
                    data-nix-search-input="q";
            }

            div.filters {
                (render_source_select(config, source_filter))
                (render_ref_select(config, source_filter, ""))
            }

            button type="submit" { "Search" }
        }
    }
}

fn render_source_select(config: &AppConfig, selected: &SourceFilter) -> Markup {
    html! {
        label {
            "Source"
            select data-nix-search-input="source-path" {
                option value="" selected[*selected == SourceFilter::All] { "All" }
                @for (id, source) in &config.sources {
                    @let name = source.name.as_deref().unwrap_or(id);
                    @let is_selected = matches!(selected, SourceFilter::Named(s) if s == id);
                    option value=(id) selected[is_selected] { (name) }
                }
            }
        }
    }
}

fn render_ref_select(
    config: &AppConfig,
    selected_source: &SourceFilter,
    current_ref: &str,
) -> Markup {
    let (refs, default_ref): (Vec<&str>, Option<&str>) = match selected_source {
        SourceFilter::All => (Vec::new(), None),
        SourceFilter::Named(source_id) => match config.sources.get(source_id.as_str()) {
            Some(source) => (
                source.refs.iter().map(|r| r.id.as_str()).collect(),
                source.default_ref.as_deref(),
            ),
            None => (Vec::new(), None),
        },
    };

    let hidden = refs.is_empty();

    html! {
        label hidden[hidden] {
            "Ref"
            select name="ref" data-nix-search-input="ref" {
                @for ref_id in &refs {
                    @let is_selected = if current_ref.is_empty() {
                        default_ref == Some(*ref_id)
                    } else {
                        *ref_id == current_ref
                    };
                    option value=(ref_id) selected[is_selected] { (ref_id) }
                }
            }
        }
    }
}
