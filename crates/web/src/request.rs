use anyhow::{Context, Result};
use serde::Deserialize;

use nix_search_config::AppConfig;
use nix_search_core::DocumentKind;

#[derive(Debug, Clone, Default)]
pub struct PageRequest {
    pub source: Option<String>,
    pub entry: Option<String>,
    pub query: PageQuery,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PageQuery {
    pub q: Option<String>,

    #[serde(rename = "ref")]
    pub ref_id: Option<String>,

    pub kind: Option<String>,

    pub source: Option<LinkOrigin>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkOrigin {
    All,
}

impl LinkOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
        }
    }

    pub fn from_query_param(s: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(s.to_owned())).ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceFilter {
    All,
    Named(String),
}

impl SourceFilter {
    pub fn from_request(request: &PageRequest) -> Self {
        match &request.source {
            None => Self::All,
            Some(source) => Self::Named(source.clone()),
        }
    }
}

pub fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty() { None } else { Some(value) }
}

pub fn normalized_query(query: &PageQuery) -> Option<&str> {
    query.q.as_deref().and_then(non_empty)
}

pub fn decode_path_value(value: &str) -> Option<String> {
    urlencoding::decode(value)
        .ok()
        .map(|value| value.into_owned())
}

pub fn page_request_from_public_url(raw_url: &str) -> std::result::Result<PageRequest, String> {
    let (raw_path, raw_query) = raw_url
        .split_once('?')
        .map_or((raw_url, ""), |(path, query)| (path, query));

    let path_parts = raw_path
        .trim_start_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();

    let source = path_parts
        .first()
        .map(|value| decode_path_value(value).unwrap_or_else(|| (*value).to_owned()));

    let entry = if path_parts.len() >= 2 {
        let raw_entry = path_parts[1..].join("/");
        Some(decode_path_value(&raw_entry).unwrap_or(raw_entry))
    } else {
        None
    };

    let mut q = None;
    let mut ref_id = None;
    let mut kind = None;
    let mut source_param = None;

    for (key, value) in url::form_urlencoded::parse(raw_query.as_bytes()) {
        match key.as_ref() {
            "q" => q = Some(value.into_owned()),
            "ref" => ref_id = Some(value.into_owned()),
            "kind" => kind = Some(value.into_owned()),
            "source" => source_param = LinkOrigin::from_query_param(&value),
            _ => {}
        }
    }

    Ok(PageRequest {
        source,
        entry,
        query: PageQuery {
            q,
            ref_id,
            kind,
            source: source_param,
        },
    })
}

pub fn resolve_entry_ref(
    config: &AppConfig,
    source_id: &str,
    ref_id: Option<&str>,
) -> Result<String> {
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

pub fn parse_document_kind(
    value: Option<&str>,
) -> std::result::Result<Option<DocumentKind>, String> {
    match value.and_then(non_empty) {
        None => Ok(None),
        Some("option") => Ok(Some(DocumentKind::Option)),
        Some("package") => Ok(Some(DocumentKind::Package)),
        Some("app") => Ok(Some(DocumentKind::App)),
        Some("service") => Ok(Some(DocumentKind::Service)),
        Some(other) => Err(format!("unknown entry kind {other:?}")),
    }
}
