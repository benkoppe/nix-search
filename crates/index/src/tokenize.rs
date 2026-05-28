use anyhow::{Context, Result};
use tantivy::Index;
use tantivy::schema::Field;
use tantivy::tokenizer::TokenStream as _;

pub(crate) fn tokenized_query_terms(
    index: &Index,
    field: Field,
    query: &str,
) -> Result<Vec<String>> {
    let mut analyzer = index
        .tokenizer_for_field(field)
        .context("failed to get tokenizer for query field")?;
    let mut token_stream = analyzer.token_stream(query);
    let mut terms = Vec::new();

    token_stream.process(&mut |token| {
        terms.push(token.text.clone());
    });

    Ok(terms)
}

pub(crate) fn structured_query_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .filter_map(|part| {
            let segments = part
                .split('.')
                .filter(|segment| !segment.is_empty())
                .collect::<Vec<_>>();

            if segments.len() >= 2
                && segments
                    .iter()
                    .any(|segment| segment.chars().any(|ch| ch.is_alphabetic()))
            {
                Some(identifier_terms(part))
            } else {
                None
            }
        })
        .max_by_key(Vec::len)
        .unwrap_or_default()
}

pub(crate) fn identifier_terms(value: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    let mut previous_was_lowercase = false;

    for ch in value.chars() {
        if !ch.is_alphanumeric() {
            push_identifier_term(&mut terms, &mut current);
            previous_was_lowercase = false;
            continue;
        }

        if ch.is_uppercase() && previous_was_lowercase {
            push_identifier_term(&mut terms, &mut current);
        }

        for lowercase in ch.to_lowercase() {
            current.push(lowercase);
        }
        previous_was_lowercase = ch.is_lowercase();
    }

    push_identifier_term(&mut terms, &mut current);
    dedup_preserving_order(terms)
}

fn push_identifier_term(terms: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        terms.push(std::mem::take(current));
    }
}

pub(crate) fn compact_identifier(value: &str) -> String {
    let mut compact = String::new();

    for ch in value.chars().filter(|ch| ch.is_alphanumeric()) {
        compact.extend(ch.to_lowercase());
    }

    compact
}

pub(crate) fn dedup_preserving_order(terms: Vec<String>) -> Vec<String> {
    let mut deduped = Vec::new();

    for term in terms {
        if !deduped.contains(&term) {
            deduped.push(term);
        }
    }

    deduped
}

#[cfg(test)]
mod tests {
    #[test]
    fn structured_query_terms_uses_only_path_like_token() {
        assert_eq!(
            super::structured_query_terms("foo services.nginx.enable bar.baz"),
            ["services", "nginx", "enable"]
        );
    }
}
