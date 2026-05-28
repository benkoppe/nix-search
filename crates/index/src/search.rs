use std::collections::HashSet;
use std::fs;

use anyhow::{Context, Result};
use camino::Utf8Path;
use tantivy::collector::{Count, TopDocs};
use tantivy::schema::{TantivyDocument, Value as _};
use tantivy::{DocAddress, Index, IndexReader, Searcher};

use nixsearch_core::{DocumentKind, SearchDocument};

use crate::ranking::{QueryAnalysis, SearchCandidate, rerank_candidate_limit, rerank_candidates};
use crate::schema::{IndexFields, build_schema};
use crate::writer::SearchIndexWriter;

pub struct SearchIndex {
    pub(crate) index: Index,
    reader: IndexReader,
    pub(crate) fields: IndexFields,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub score: f32,
    pub document: SearchDocument,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub hits: Vec<SearchHit>,
    pub total: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchScope {
    pub source: String,
    pub ref_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    pub query: String,
    pub limit: usize,
    pub offset: usize,
    pub scopes: Vec<SearchScope>,
}

#[derive(Debug, Clone)]
pub struct EntryLookup {
    pub source: String,
    pub ref_id: String,
    pub name: String,
    pub kind: Option<DocumentKind>,
}

#[derive(Debug, Clone)]
pub enum EntryLookupResult {
    Found(Box<SearchDocument>),
    NotFound,
    Ambiguous(Vec<SearchDocument>),
}

impl SearchIndex {
    pub fn create_or_replace(path: impl AsRef<Utf8Path>) -> Result<Self> {
        let path = path.as_ref();

        if path.exists() {
            fs::remove_dir_all(path)
                .with_context(|| format!("failed to remove existing index {path}"))?;
        }

        fs::create_dir_all(path)
            .with_context(|| format!("failed to create index directory {path}"))?;

        let schema = build_schema();
        let fields = IndexFields::from_schema(&schema)?;
        let index = Index::create_in_dir(path, schema)
            .with_context(|| format!("failed to create Tantivy index at {path}"))?;
        let reader = index.reader().context("failed to create index reader")?;

        Ok(Self {
            index,
            reader,
            fields,
        })
    }

    pub fn open(path: impl AsRef<Utf8Path>) -> Result<Self> {
        let path = path.as_ref();
        let index = Index::open_in_dir(path)
            .with_context(|| format!("failed to open Tantivy index at {path}"))?;
        let schema = index.schema();
        let fields = IndexFields::from_schema(&schema)?;
        let reader = index.reader().context("failed to create index reader")?;

        Ok(Self {
            index,
            reader,
            fields,
        })
    }

    pub fn writer(&self) -> Result<SearchIndexWriter> {
        let writer = self
            .index
            .writer(crate::WRITER_MEMORY_BYTES)
            .context("failed to create index writer")?;

        Ok(SearchIndexWriter {
            writer,
            fields: self.fields.clone(),
        })
    }

    pub fn search(&self, options: SearchOptions) -> Result<SearchResult> {
        let searcher = self.reader.searcher();
        let candidate_limit = rerank_candidate_limit(options.limit, options.offset);
        let diversify_sources = options.scopes.len() > 1;
        let analysis = QueryAnalysis::new(&self.index, self.fields.name_text, &options.query)?;

        let (candidates, total) = if diversify_sources {
            let mut candidates = Vec::new();
            let mut total = 0;

            for scope in &options.scopes {
                let (mut scope_candidates, scope_total) = self.search_candidates(
                    &searcher,
                    &options.query,
                    std::slice::from_ref(scope),
                    candidate_limit,
                )?;
                total += scope_total;
                candidates.append(&mut scope_candidates);
            }

            (candidates, total)
        } else {
            self.search_candidates(&searcher, &options.query, &options.scopes, candidate_limit)?
        };
        let candidate_window_len = candidates.len();
        let candidate_ids = candidates
            .iter()
            .map(|candidate| candidate.document.id().to_owned())
            .collect::<HashSet<_>>();

        let mut hits = rerank_candidates(
            candidates,
            &analysis,
            diversify_sources,
            options.offset,
            options.limit,
        );

        if hits.len() < options.limit && total > options.offset.saturating_add(hits.len()) {
            let remaining = options.limit - hits.len();
            let mut fallback_hits = if diversify_sources {
                self.search_native_filtered_hits(
                    &searcher,
                    &options.query,
                    &options.scopes,
                    &candidate_ids,
                    options.offset.saturating_sub(candidate_window_len),
                    remaining,
                    total,
                )?
            } else {
                self.search_native_hits(
                    &searcher,
                    &options.query,
                    &options.scopes,
                    options.offset.max(candidate_window_len),
                    remaining,
                )?
            };

            hits.append(&mut fallback_hits);
        }

        Ok(SearchResult { hits, total })
    }

    fn search_candidates(
        &self,
        searcher: &Searcher,
        query_text: &str,
        scopes: &[SearchScope],
        limit: usize,
    ) -> Result<(Vec<SearchCandidate>, usize)> {
        let query = self.build_query(query_text, scopes)?;

        let (total, top_docs) = searcher
            .search(
                &*query,
                &(Count, TopDocs::with_limit(limit).order_by_score()),
            )
            .context("search failed")?;

        let mut candidates = Vec::with_capacity(top_docs.len());

        for (score, address) in top_docs {
            candidates.push(SearchCandidate {
                score,
                document: self.document_at_with_searcher(searcher, address)?,
            });
        }

        Ok((candidates, total))
    }

    fn search_native_hits(
        &self,
        searcher: &Searcher,
        query_text: &str,
        scopes: &[SearchScope],
        offset: usize,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let query = self.build_query(query_text, scopes)?;
        let top_docs = searcher
            .search(
                &*query,
                &TopDocs::with_limit(limit)
                    .and_offset(offset)
                    .order_by_score(),
            )
            .context("native fallback search failed")?;

        let mut hits = Vec::with_capacity(top_docs.len());

        for (score, address) in top_docs {
            hits.push(SearchHit {
                score,
                document: self.document_at_with_searcher(searcher, address)?,
            });
        }

        Ok(hits)
    }

    fn search_native_filtered_hits(
        &self,
        searcher: &Searcher,
        query_text: &str,
        scopes: &[SearchScope],
        excluded_ids: &HashSet<String>,
        skip: usize,
        limit: usize,
        total: usize,
    ) -> Result<Vec<SearchHit>> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let query = self.build_query(query_text, scopes)?;
        let mut hits = Vec::with_capacity(limit);
        let mut skipped = 0;
        let mut native_offset = 0;

        while hits.len() < limit && native_offset < total {
            let batch_limit = skip
                .saturating_sub(skipped)
                .saturating_add(limit - hits.len())
                .saturating_add(excluded_ids.len())
                .clamp(200, 5_000)
                .min(total - native_offset);

            if batch_limit == 0 {
                break;
            }

            let top_docs = searcher
                .search(
                    &*query,
                    &TopDocs::with_limit(batch_limit)
                        .and_offset(native_offset)
                        .order_by_score(),
                )
                .context("filtered native fallback search failed")?;

            if top_docs.is_empty() {
                break;
            }

            native_offset += top_docs.len();

            for (score, address) in top_docs {
                let document = self.document_at_with_searcher(searcher, address)?;

                if excluded_ids.contains(document.id()) {
                    continue;
                }

                if skipped < skip {
                    skipped += 1;
                    continue;
                }

                hits.push(SearchHit { score, document });

                if hits.len() == limit {
                    break;
                }
            }
        }

        Ok(hits)
    }

    pub fn get_by_id(&self, id: &str) -> Result<Option<SearchDocument>> {
        let searcher = self.reader.searcher();

        let query = tantivy::query::TermQuery::new(
            tantivy::Term::from_field_text(self.fields.id, id),
            tantivy::schema::IndexRecordOption::Basic,
        );

        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(1).order_by_score())
            .context("entry lookup by id failed")?;

        let Some((_, address)) = top_docs.into_iter().next() else {
            return Ok(None);
        };

        self.document_at(address).map(Some)
    }

    pub fn find_entry(&self, lookup: EntryLookup) -> Result<EntryLookupResult> {
        let searcher = self.reader.searcher();

        let mut clauses: Vec<(tantivy::query::Occur, Box<dyn tantivy::query::Query>)> = vec![
            (
                tantivy::query::Occur::Must,
                Box::new(tantivy::query::TermQuery::new(
                    tantivy::Term::from_field_text(self.fields.source, &lookup.source),
                    tantivy::schema::IndexRecordOption::Basic,
                )),
            ),
            (
                tantivy::query::Occur::Must,
                Box::new(tantivy::query::TermQuery::new(
                    tantivy::Term::from_field_text(self.fields.ref_id, &lookup.ref_id),
                    tantivy::schema::IndexRecordOption::Basic,
                )),
            ),
            (
                tantivy::query::Occur::Must,
                Box::new(tantivy::query::TermQuery::new(
                    tantivy::Term::from_field_text(self.fields.name_exact, &lookup.name),
                    tantivy::schema::IndexRecordOption::Basic,
                )),
            ),
        ];

        if let Some(kind) = lookup.kind {
            clauses.push((
                tantivy::query::Occur::Must,
                Box::new(tantivy::query::TermQuery::new(
                    tantivy::Term::from_field_text(self.fields.kind, kind.as_str()),
                    tantivy::schema::IndexRecordOption::Basic,
                )),
            ));
        }

        let query = tantivy::query::BooleanQuery::new(clauses);

        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(10).order_by_score())
            .context("entry lookup failed")?;

        let mut documents = Vec::with_capacity(top_docs.len());

        for (_, address) in top_docs {
            documents.push(self.document_at(address)?);
        }

        match documents.len() {
            0 => Ok(EntryLookupResult::NotFound),
            1 => Ok(EntryLookupResult::Found(Box::new(documents.remove(0)))),
            _ => Ok(EntryLookupResult::Ambiguous(documents)),
        }
    }

    fn document_at(&self, address: DocAddress) -> Result<SearchDocument> {
        let searcher = self.reader.searcher();
        self.document_at_with_searcher(&searcher, address)
    }

    fn document_at_with_searcher(
        &self,
        searcher: &Searcher,
        address: DocAddress,
    ) -> Result<SearchDocument> {
        let retrieved: TantivyDocument = searcher
            .doc(address)
            .context("failed to retrieve entry document")?;

        let stored_json = retrieved
            .get_first(self.fields.stored_json)
            .and_then(|value| value.as_str())
            .context("entry document did not contain stored_json")?;

        serde_json::from_str(stored_json).context("failed to deserialize entry document")
    }
}
