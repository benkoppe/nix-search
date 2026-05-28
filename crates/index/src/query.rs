use anyhow::Result;
use tantivy::query::{BooleanQuery, BoostQuery, FuzzyTermQuery, Occur, Query};
use tantivy::schema::{Field, IndexRecordOption};
use tantivy::{Index, Score, Term};

use crate::schema::IndexFields;
use crate::search::{SearchIndex, SearchScope};
use crate::tokenize::{compact_identifier, dedup_preserving_order, tokenized_query_terms};

impl SearchIndex {
    pub(crate) fn build_query(
        &self,
        query_text: &str,
        scopes: &[SearchScope],
    ) -> Result<Box<dyn Query>> {
        let text_query = self.build_text_query(query_text)?;

        let mut clauses: Vec<(Occur, Box<dyn Query>)> = vec![(Occur::Must, text_query)];

        if !scopes.is_empty() {
            let mut scope_clauses = Vec::with_capacity(scopes.len());

            for scope in scopes {
                let source_query: Box<dyn Query> = Box::new(tantivy::query::TermQuery::new(
                    Term::from_field_text(self.fields.source, &scope.source),
                    tantivy::schema::IndexRecordOption::Basic,
                ));

                let ref_query: Box<dyn Query> = Box::new(tantivy::query::TermQuery::new(
                    Term::from_field_text(self.fields.ref_id, &scope.ref_id),
                    tantivy::schema::IndexRecordOption::Basic,
                ));

                let pair_query: Box<dyn Query> = Box::new(BooleanQuery::new(vec![
                    (Occur::Must, source_query),
                    (Occur::Must, ref_query),
                ]));

                scope_clauses.push((Occur::Should, pair_query));
            }

            clauses.push((Occur::Must, Box::new(BooleanQuery::new(scope_clauses))));
        }

        Ok(Box::new(BooleanQuery::new(clauses)))
    }

    fn build_text_query(&self, query_text: &str) -> Result<Box<dyn Query>> {
        let mut text_clauses = self.build_exact_text_clauses(query_text)?;

        if let Some(fuzzy_query) = build_fuzzy_query(&self.index, query_text, &self.fields)? {
            text_clauses.push((Occur::Should, fuzzy_query));
        }

        Ok(Box::new(BooleanQuery::new(text_clauses)))
    }

    fn build_exact_text_clauses(&self, query_text: &str) -> Result<Vec<(Occur, Box<dyn Query>)>> {
        let mut clauses = Vec::new();
        let raw = query_text.trim();

        if !raw.is_empty() {
            for value in dedup_preserving_order(vec![raw.to_owned(), raw.to_lowercase()]) {
                self.add_string_field_clauses(&mut clauses, &value);
            }
        }

        let mut terms = tokenized_query_terms(&self.index, self.fields.name_text, query_text)?;
        let compact = compact_identifier(query_text);

        if !compact.is_empty() {
            terms.push(compact);
        }

        for term in dedup_preserving_order(terms) {
            self.add_token_field_clauses(&mut clauses, &term);
        }

        Ok(clauses)
    }

    fn add_string_field_clauses(&self, clauses: &mut Vec<(Occur, Box<dyn Query>)>, value: &str) {
        add_boosted_term_clause(
            clauses,
            self.fields.name_exact,
            value,
            20.0,
            IndexRecordOption::Basic,
        );
        add_boosted_term_clause(
            clauses,
            self.fields.name_groups,
            value,
            15.0,
            IndexRecordOption::Basic,
        );
        add_boosted_term_clause(
            clauses,
            self.fields.name_root,
            value,
            8.0,
            IndexRecordOption::Basic,
        );
        add_boosted_term_clause(
            clauses,
            self.fields.name_leaf,
            value,
            6.0,
            IndexRecordOption::Basic,
        );
        add_boosted_term_clause(
            clauses,
            self.fields.attribute_exact,
            value,
            25.0,
            IndexRecordOption::Basic,
        );
        add_boosted_term_clause(
            clauses,
            self.fields.main_program,
            value,
            20.0,
            IndexRecordOption::Basic,
        );
        add_boosted_term_clause(
            clauses,
            self.fields.option_set,
            value,
            3.0,
            IndexRecordOption::Basic,
        );
        add_boosted_term_clause(
            clauses,
            self.fields.parents,
            value,
            2.0,
            IndexRecordOption::Basic,
        );
        add_boosted_term_clause(
            clauses,
            self.fields.package_set,
            value,
            2.0,
            IndexRecordOption::Basic,
        );
        add_boosted_term_clause(
            clauses,
            self.fields.platforms,
            value,
            1.0,
            IndexRecordOption::Basic,
        );
    }

    fn add_token_field_clauses(&self, clauses: &mut Vec<(Occur, Box<dyn Query>)>, term: &str) {
        add_boosted_term_clause(
            clauses,
            self.fields.name_text,
            term,
            10.0,
            IndexRecordOption::WithFreqs,
        );
        add_boosted_term_clause(
            clauses,
            self.fields.attribute_text,
            term,
            12.0,
            IndexRecordOption::WithFreqs,
        );
        add_boosted_term_clause(
            clauses,
            self.fields.description,
            term,
            1.0,
            IndexRecordOption::WithFreqs,
        );
        add_boosted_term_clause(
            clauses,
            self.fields.option_type,
            term,
            1.5,
            IndexRecordOption::WithFreqs,
        );
        add_boosted_term_clause(
            clauses,
            self.fields.name_root,
            term,
            8.0,
            IndexRecordOption::Basic,
        );
        add_boosted_term_clause(
            clauses,
            self.fields.name_leaf,
            term,
            6.0,
            IndexRecordOption::Basic,
        );
        add_boosted_term_clause(
            clauses,
            self.fields.main_program,
            term,
            20.0,
            IndexRecordOption::Basic,
        );
        add_boosted_term_clause(
            clauses,
            self.fields.option_set,
            term,
            3.0,
            IndexRecordOption::Basic,
        );
        add_boosted_term_clause(
            clauses,
            self.fields.package_set,
            term,
            2.0,
            IndexRecordOption::Basic,
        );
    }
}

fn add_boosted_term_clause(
    clauses: &mut Vec<(Occur, Box<dyn Query>)>,
    field: Field,
    value: &str,
    boost: Score,
    index_record_option: IndexRecordOption,
) {
    if value.is_empty() {
        return;
    }

    let query: Box<dyn Query> = Box::new(tantivy::query::TermQuery::new(
        Term::from_field_text(field, value),
        index_record_option,
    ));

    clauses.push((Occur::Should, Box::new(BoostQuery::new(query, boost))));
}

fn build_fuzzy_query(
    index: &Index,
    query: &str,
    fields: &IndexFields,
) -> Result<Option<Box<dyn Query>>> {
    let terms = fuzzy_query_terms(index, fields.name_text, query)?;

    let mut term_clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();

    for term in terms {
        let Some(distance) = fuzzy_distance(&term) else {
            continue;
        };

        let mut field_clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();

        add_fuzzy_field_clause(&mut field_clauses, fields.name_text, &term, distance, 10.0);
        add_fuzzy_field_clause(
            &mut field_clauses,
            fields.attribute_text,
            &term,
            distance,
            12.0,
        );

        term_clauses.push((Occur::Should, Box::new(BooleanQuery::new(field_clauses))));
    }

    if term_clauses.is_empty() {
        return Ok(None);
    }

    Ok(Some(Box::new(BooleanQuery::new(term_clauses))))
}

fn fuzzy_query_terms(index: &Index, field: Field, query: &str) -> Result<Vec<String>> {
    let mut terms = tokenized_query_terms(index, field, query)?;
    let compact = compact_identifier(query);

    if !compact.is_empty() {
        terms.push(compact);
    }

    Ok(dedup_preserving_order(terms))
}

fn add_fuzzy_field_clause(
    clauses: &mut Vec<(Occur, Box<dyn Query>)>,
    field: Field,
    term: &str,
    distance: u8,
    field_boost: f32,
) {
    let fuzzy_query: Box<dyn Query> = Box::new(FuzzyTermQuery::new_prefix(
        Term::from_field_text(field, term),
        distance,
        true,
    ));

    clauses.push((
        Occur::Should,
        Box::new(BoostQuery::new(
            fuzzy_query,
            fuzzy_boost(distance, field_boost),
        )),
    ));
}

fn fuzzy_distance(term: &str) -> Option<u8> {
    match term.chars().count() {
        0..=3 => None,
        4..=7 => Some(1),
        _ => Some(2),
    }
}

fn fuzzy_boost(distance: u8, field_boost: f32) -> f32 {
    let distance_boost = match distance {
        0 => 1.0,
        1 => 0.25,
        _ => 0.10,
    };

    field_boost * distance_boost
}
