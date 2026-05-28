use std::collections::HashMap;

use anyhow::Result;
use tantivy::schema::Field;
use tantivy::{Index, Score};

use nixsearch_core::{PackageDoc, SearchDocument};

use crate::search::SearchHit;
use crate::tokenize::{
    compact_identifier, dedup_preserving_order, identifier_terms, structured_query_terms,
    tokenized_query_terms,
};

const MAX_RERANK_CANDIDATES: usize = 1_000;

#[derive(Debug)]
pub(crate) struct SearchCandidate {
    pub(crate) score: Score,
    pub(crate) document: SearchDocument,
}

#[derive(Debug)]
struct RankedCandidate {
    base_score: Score,
    final_score: Score,
    source: String,
    cluster_key: Option<String>,
    cluster_explicit: bool,
    document: SearchDocument,
}

#[derive(Debug)]
pub(crate) struct QueryAnalysis {
    normalized: String,
    compact: String,
    terms: Vec<String>,
    structured_terms: Vec<String>,
}

impl QueryAnalysis {
    pub(crate) fn new(index: &Index, field: Field, query: &str) -> Result<Self> {
        let normalized = query.trim().to_lowercase();
        let compact = compact_identifier(query);
        let identifier_terms = identifier_terms(query);
        let structured_terms = structured_query_terms(query);

        let mut terms = tokenized_query_terms(index, field, query)?;
        terms.extend(identifier_terms);

        if !compact.is_empty() {
            terms.push(compact.clone());
        }

        Ok(Self {
            normalized,
            compact,
            terms: dedup_preserving_order(terms),
            structured_terms,
        })
    }

    fn contains_term(&self, term: &str) -> bool {
        self.terms.iter().any(|candidate| candidate == term)
    }

    fn is_structured(&self) -> bool {
        !self.structured_terms.is_empty()
    }
}

pub(crate) fn rerank_candidate_limit(limit: usize, offset: usize) -> usize {
    let target = offset.saturating_add(limit).max(1);
    target.saturating_mul(4).max(200).min(MAX_RERANK_CANDIDATES)
}

pub(crate) fn rerank_candidates(
    candidates: Vec<SearchCandidate>,
    analysis: &QueryAnalysis,
    diversify_sources: bool,
    offset: usize,
    limit: usize,
) -> Vec<SearchHit> {
    if limit == 0 {
        return Vec::new();
    }

    let mut remaining = candidates
        .into_iter()
        .map(|candidate| {
            let source = candidate.document.common().source.clone();
            let cluster_key = duplicate_cluster_key(&candidate.document);
            let cluster_explicit = duplicate_cluster_explicit(&candidate.document, analysis);
            let base_score = domain_relevance_score(candidate.score, &candidate.document, analysis);

            RankedCandidate {
                base_score,
                final_score: base_score,
                source,
                cluster_key,
                cluster_explicit,
                document: candidate.document,
            }
        })
        .collect::<Vec<_>>();

    let needed = offset.saturating_add(limit);
    let mut selected = Vec::with_capacity(needed.min(remaining.len()));
    let mut source_counts: HashMap<String, usize> = HashMap::new();
    let mut cluster_counts: HashMap<String, usize> = HashMap::new();

    while selected.len() < needed && !remaining.is_empty() {
        let mut best_index = 0;
        let mut best_score = f32::NEG_INFINITY;

        for (index, candidate) in remaining.iter().enumerate() {
            let adjusted_score = adjusted_candidate_score(
                candidate,
                diversify_sources,
                &source_counts,
                &cluster_counts,
            );

            if adjusted_score > best_score
                || (adjusted_score == best_score
                    && candidate_sort_key(candidate) < candidate_sort_key(&remaining[best_index]))
            {
                best_index = index;
                best_score = adjusted_score;
            }
        }

        let mut candidate = remaining.swap_remove(best_index);
        candidate.final_score = best_score;

        *source_counts.entry(candidate.source.clone()).or_default() += 1;

        if let Some(cluster_key) = &candidate.cluster_key {
            *cluster_counts.entry(cluster_key.clone()).or_default() += 1;
        }

        selected.push(candidate);
    }

    selected
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|candidate| SearchHit {
            score: candidate.final_score,
            document: candidate.document,
        })
        .collect()
}

fn adjusted_candidate_score(
    candidate: &RankedCandidate,
    diversify_sources: bool,
    source_counts: &HashMap<String, usize>,
    cluster_counts: &HashMap<String, usize>,
) -> Score {
    let source_count = source_counts.get(&candidate.source).copied().unwrap_or(0);
    let source_factor = if diversify_sources {
        1.0 / (1.0 + source_count as Score * 0.18)
    } else {
        1.0
    };

    let cluster_count = candidate
        .cluster_key
        .as_ref()
        .and_then(|key| cluster_counts.get(key))
        .copied()
        .unwrap_or(0);

    candidate.base_score
        * source_factor
        * duplicate_cluster_factor(cluster_count, candidate.cluster_explicit)
}

fn duplicate_cluster_factor(count: usize, explicit: bool) -> Score {
    if count == 0 {
        return 1.0;
    }

    if explicit {
        return 1.0 / (1.0 + count as Score * 0.15);
    }

    match count {
        1 => 0.45,
        2 => 0.25,
        _ => 0.15,
    }
}

fn candidate_sort_key(candidate: &RankedCandidate) -> (&str, &str) {
    (candidate.source.as_str(), candidate.document.name())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StructuredMatchTier {
    Unstructured,
    PathPrefix,
    AllTermsInPath,
    PartialPath,
    NoPathMatch,
}

impl StructuredMatchTier {
    fn score_bonus(self) -> Score {
        match self {
            Self::Unstructured => 0.0,
            Self::PathPrefix => 500.0,
            Self::AllTermsInPath => 260.0,
            Self::PartialPath => -120.0,
            Self::NoPathMatch => -180.0,
        }
    }
}

fn domain_relevance_score(
    original_score: Score,
    document: &SearchDocument,
    analysis: &QueryAnalysis,
) -> Score {
    let mut score = original_score.max(0.0).ln_1p() * 4.0;
    let common = document.common();
    let name = common.name.to_lowercase();
    let name_compact = compact_identifier(&common.name);
    let name_terms = identifier_terms(&common.name);
    let path_terms = document_path_terms(document);
    let structured_tier = structured_match_tier(analysis, &path_terms);

    score += structured_tier.score_bonus();

    if structured_tier == StructuredMatchTier::PathPrefix {
        score -= structured_path_depth_penalty(document, analysis);
    }

    if name == analysis.normalized {
        score += 90.0;
    }

    if !analysis.compact.is_empty() && name_compact == analysis.compact {
        score += 80.0;
    }

    if !analysis.normalized.is_empty() && name.starts_with(&analysis.normalized) {
        score += 35.0;
    }

    if all_query_terms_match(&analysis.structured_terms, &name_terms) {
        score += 18.0;
    }

    match document {
        SearchDocument::Option(option) => {
            let segments = if option.loc.is_empty() {
                common
                    .name_parts
                    .segments
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
            } else {
                option.loc.iter().map(String::as_str).collect::<Vec<_>>()
            };

            score += option_name_score(analysis, &segments);
        }

        SearchDocument::Package(package) => {
            score += package_name_score(analysis, package);
        }
    }

    score.max(0.0)
}

fn structured_match_tier(analysis: &QueryAnalysis, path_terms: &[String]) -> StructuredMatchTier {
    if !analysis.is_structured() {
        return StructuredMatchTier::Unstructured;
    }

    if starts_with_query_terms(path_terms, &analysis.structured_terms) {
        return StructuredMatchTier::PathPrefix;
    }

    if all_query_terms_match_path(&analysis.structured_terms, path_terms) {
        return StructuredMatchTier::AllTermsInPath;
    }

    if analysis.structured_terms.iter().any(|term| {
        path_terms
            .iter()
            .any(|path_term| path_term_matches_query_term(path_term, term))
    }) {
        return StructuredMatchTier::PartialPath;
    }

    StructuredMatchTier::NoPathMatch
}

fn starts_with_query_terms(values: &[String], prefix: &[String]) -> bool {
    values.len() >= prefix.len()
        && values
            .iter()
            .zip(prefix)
            .all(|(value, prefix)| path_term_matches_query_term(value, prefix))
}

fn all_query_terms_match_path(query_terms: &[String], path_terms: &[String]) -> bool {
    !query_terms.is_empty()
        && query_terms.iter().all(|term| {
            path_terms
                .iter()
                .any(|path_term| path_term_matches_query_term(path_term, term))
        })
}

fn path_term_matches_query_term(path_term: &str, query_term: &str) -> bool {
    path_term == query_term
        || (query_term.chars().count() >= 3 && path_term.starts_with(query_term))
}

fn structured_path_depth_penalty(document: &SearchDocument, analysis: &QueryAnalysis) -> Score {
    let depth = document_path_depth(document);
    let query_depth = analysis.structured_terms.len();
    depth.saturating_sub(query_depth) as Score * 3.0
}

fn document_path_terms(document: &SearchDocument) -> Vec<String> {
    document_path_segments(document)
        .into_iter()
        .flat_map(|segment| identifier_terms(&segment))
        .collect()
}

fn document_path_depth(document: &SearchDocument) -> usize {
    document_path_segments(document).len()
}

fn document_path_segments(document: &SearchDocument) -> Vec<String> {
    match document {
        SearchDocument::Option(option) if !option.loc.is_empty() => option.loc.clone(),
        SearchDocument::Option(option) => option.common.name_parts.segments.clone(),
        SearchDocument::Package(package) => package
            .attribute
            .split('.')
            .filter(|segment| !segment.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
    }
}

fn option_name_score(analysis: &QueryAnalysis, segments: &[&str]) -> Score {
    let normalized_segments = segments
        .iter()
        .map(|segment| segment.to_lowercase())
        .collect::<Vec<_>>();
    let expanded_terms = segments
        .iter()
        .flat_map(|segment| identifier_terms(segment))
        .collect::<Vec<_>>();

    let mut score = 0.0;

    for term in &analysis.terms {
        if normalized_segments.iter().any(|segment| segment == term) {
            score += 52.0;
        } else if expanded_terms.iter().any(|segment| segment == term) {
            score += 34.0;
        }
    }

    if all_query_terms_match(&analysis.structured_terms, &expanded_terms) {
        score += 20.0;
    }

    score
}

fn package_name_score(analysis: &QueryAnalysis, package: &PackageDoc) -> Score {
    let mut score = 0.0;
    let attribute = package.attribute.to_lowercase();
    let attribute_compact = compact_identifier(&package.attribute);
    let leaf = package
        .attribute
        .rsplit('.')
        .next()
        .unwrap_or(&package.attribute);
    let leaf_normalized = leaf.to_lowercase();
    let leaf_terms = identifier_terms(leaf);
    let nested = package.package_set.is_some();
    let package_set_explicit = package
        .package_set
        .as_deref()
        .is_some_and(|package_set| query_mentions_package_set(analysis, package_set));

    if attribute == analysis.normalized {
        score += 110.0;
    }

    if !analysis.compact.is_empty() && attribute_compact == analysis.compact {
        score += 90.0;
    }

    if leaf_normalized == analysis.normalized {
        score += if nested && !package_set_explicit {
            34.0
        } else {
            70.0
        };
    }

    for term in &analysis.terms {
        if leaf_terms.iter().any(|leaf_term| leaf_term == term) {
            score += if nested && !package_set_explicit {
                18.0
            } else {
                32.0
            };
        }
    }

    if let Some(main_program) = &package.main_program {
        if main_program.to_lowercase() == analysis.normalized {
            score += 85.0;
        }
    }

    if package_set_explicit {
        score += 35.0;
    } else if nested {
        score -= 6.0;
    }

    score
}

fn all_query_terms_match(query_terms: &[String], document_terms: &[String]) -> bool {
    let meaningful_terms = query_terms
        .iter()
        .filter(|term| term.chars().count() > 1)
        .collect::<Vec<_>>();

    !meaningful_terms.is_empty()
        && meaningful_terms.iter().all(|term| {
            document_terms
                .iter()
                .any(|document_term| document_term == *term)
        })
}

fn duplicate_cluster_key(document: &SearchDocument) -> Option<String> {
    let SearchDocument::Package(package) = document else {
        return None;
    };

    let package_set = package.package_set.as_deref()?;
    let leaf = package.attribute.rsplit('.').next()?;

    Some(format!(
        "{}:{}:{}",
        package.common.source.as_str(),
        package_set_family(package_set),
        compact_identifier(leaf)
    ))
}

fn duplicate_cluster_explicit(document: &SearchDocument, analysis: &QueryAnalysis) -> bool {
    let SearchDocument::Package(package) = document else {
        return false;
    };

    let Some(package_set) = package.package_set.as_deref() else {
        return false;
    };

    query_mentions_package_set(analysis, package_set)
}

fn query_mentions_package_set(analysis: &QueryAnalysis, package_set: &str) -> bool {
    let package_set_compact = compact_identifier(package_set);
    let family = package_set_family(package_set);
    let family_compact = compact_identifier(&family);

    if !analysis.compact.is_empty()
        && (analysis.compact.contains(&package_set_compact)
            || analysis.compact.contains(&family_compact))
    {
        return true;
    }

    let family_terms = identifier_terms(&family)
        .into_iter()
        .filter(|term| term.chars().any(|ch| ch.is_alphabetic()))
        .collect::<Vec<_>>();

    !family_terms.is_empty() && family_terms.iter().all(|term| analysis.contains_term(term))
}

fn package_set_family(package_set: &str) -> String {
    let mut family = package_set;

    while let Some((prefix, suffix)) = family.rsplit_once('_') {
        if suffix.chars().all(|ch| ch.is_ascii_digit()) {
            family = prefix;
        } else {
            break;
        }
    }

    family.to_owned()
}
