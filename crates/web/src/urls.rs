use nix_search_config::AppConfig;

use crate::request::{LinkOrigin, PageQuery, PageRequest, non_empty};

pub fn source_path(source: &str) -> String {
    format!("/{}", encode_path(source))
}

pub fn search_url_for(source: Option<&str>, query: &PageQuery) -> String {
    let path = source.map(source_path).unwrap_or_else(|| "/".to_owned());

    let qs = query_string([
        ("q", query.q.as_deref()),
        ("ref", query.ref_id.as_deref()),
        ("source", query.source.map(|s| s.as_str())),
    ]);

    if qs.is_empty() {
        path
    } else {
        format!("{path}?{qs}")
    }
}

pub fn entry_url_for(source: &str, entry: &str, kind: Option<&str>, query: &PageQuery) -> String {
    let path = format!("{}/{}", source_path(source), encode_path(entry));

    let qs = query_string([
        ("q", query.q.as_deref()),
        ("ref", query.ref_id.as_deref()),
        ("kind", kind.or(query.kind.as_deref())),
        ("source", query.source.map(|s| s.as_str())),
    ]);

    if qs.is_empty() {
        path
    } else {
        format!("{path}?{qs}")
    }
}

pub fn close_url_for(request: &PageRequest) -> String {
    if request.query.source == Some(LinkOrigin::All) {
        return search_url_for(
            None,
            &PageQuery {
                q: request.query.q.clone(),
                ..PageQuery::default()
            },
        );
    }

    search_url_for(
        request.source.as_deref(),
        &PageQuery {
            q: request.query.q.clone(),
            ref_id: request.query.ref_id.clone(),
            ..PageQuery::default()
        },
    )
}

pub fn ref_id_for_link(config: &AppConfig, source: &str, ref_id: &str) -> Option<String> {
    let default_ref = config
        .sources
        .get(source)
        .and_then(|source| source.default_ref.as_deref());

    if default_ref == Some(ref_id) {
        None
    } else {
        Some(ref_id.to_owned())
    }
}

fn encode_path(value: &str) -> String {
    urlencoding::encode(value).into_owned()
}

fn encode_query(value: &str) -> String {
    urlencoding::encode(value).into_owned()
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
