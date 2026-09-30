//! Shared lexical-relevance primitives.
//!
//! Single home for the term-overlap helpers used across context assembly
//! (ranking, budgeting, rescue, scoring) and temporal query parsing.
//! Callers import from here instead of keeping local copies.

use std::collections::HashSet;

use crate::models::Fact;

use super::search::search_query_terms;

/// Subset of `query_terms` that appear in `text` (both normalized via
/// [`search_query_terms`]). Empty `query_terms` yields an empty set.
pub fn matched_query_terms_for_text(text: &str, query_terms: &[String]) -> HashSet<String> {
    if query_terms.is_empty() {
        return HashSet::new();
    }

    let content_terms = search_query_terms(text).into_iter().collect::<HashSet<_>>();

    query_terms
        .iter()
        .filter(|term| content_terms.contains(term.as_str()))
        .cloned()
        .collect()
}

/// Normalized term set of a fact: its content plus all index keys.
pub fn fact_term_set(fact: &Fact) -> HashSet<String> {
    let mut fact_terms = search_query_terms(&fact.content)
        .into_iter()
        .collect::<HashSet<_>>();
    for index_key in &fact.index_keys {
        fact_terms.extend(search_query_terms(index_key));
    }
    fact_terms
}

/// Subset of `query_terms` matched by a fact's content and index keys.
/// Empty `query_terms` yields an empty set.
pub fn matched_query_terms_for_fact(fact: &Fact, query_terms: &[String]) -> HashSet<String> {
    if query_terms.is_empty() {
        return HashSet::new();
    }

    let fact_terms = fact_term_set(fact);

    query_terms
        .iter()
        .filter(|term| fact_terms.contains(term.as_str()))
        .cloned()
        .collect()
}

/// Normalized term set of a record's `content` plus its `index_keys`.
///
/// This is `fact_term_set` for a raw SurrealDB row, and it exists separately
/// because the row cannot always be decoded into a [`Fact`]:
/// `knowledge::fact_parsing::fact_from_record` requires `fact_id`,
/// `fact_type`, `content`, `quote`, `source_episode` and `t_valid`, and a
/// partial record — which a lexical scan does encounter — would be dropped.
/// Dropping a record from a lexical scan is worse than reading two fields
/// from it, so the row-shaped path reads the fields it needs and nothing
/// else.
pub fn record_term_set(record: &serde_json::Value) -> HashSet<String> {
    let mut terms = HashSet::new();
    if let Some(content) = string_field(record, "content") {
        terms.extend(search_query_terms(&content));
    }
    for index_key in string_array_field(record, "index_keys") {
        terms.extend(search_query_terms(&index_key));
    }
    terms
}

/// How many of `query_terms` a raw record contains.
///
/// The `Value`-typed counterpart of the `Fact`-typed overlap, for the same
/// reason `record_term_set` exists: the row may not decode.
pub fn record_query_overlap(record: &serde_json::Value, query_terms: &[String]) -> usize {
    if query_terms.is_empty() {
        return 0;
    }
    let record_terms = record_term_set(record);
    query_terms
        .iter()
        .filter(|term| record_terms.contains(term.as_str()))
        .count()
}

fn string_field(record: &serde_json::Value, field: &str) -> Option<String> {
    let value = record.get(field)?;
    if value.is_string() {
        return value.as_str().map(str::to_string);
    }
    // SurrealDB serialises a `Strand` as `{"Strand": {"String": "…"}}`, and a
    // lexical scan reads rows straight out of the driver.
    value
        .get("String")
        .or_else(|| value.get("Strand").and_then(|s| s.get("String")))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn string_array_field(record: &serde_json::Value, field: &str) -> Vec<String> {
    let Some(values) = record.get(field).and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    values
        .iter()
        .filter_map(|value| match value {
            serde_json::Value::String(text) => Some(text.clone()),
            other => string_field(other, "String"),
        })
        .collect()
}

/// True if `term` is exactly four ASCII digits (a year token like `2026`).
pub fn is_four_digit_year(term: &str) -> bool {
    term.len() == 4 && term.chars().all(|character| character.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_fact(content: &str, index_keys: Vec<String>) -> Fact {
        Fact {
            fact_id: "f:1".into(),
            fact_type: "note".into(),
            content: content.into(),
            quote: String::new(),
            source_episode: "ep:1".into(),
            t_valid: chrono::Utc::now(),
            t_ingested: chrono::Utc::now(),
            t_invalid: None,
            t_invalid_ingested: None,
            confidence: 0.9,
            index_keys,
            access_count: 0,
            last_accessed: None,
            entity_links: vec![],
            scope: "org".into(),
            policy_tags: vec![],
            provenance: crate::models::Provenance::manual(),
            ft_score: 0.0,
        }
    }

    fn terms(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    // -- matched_query_terms_for_text ---------------------------------------

    #[test]
    fn text_match_finds_overlap() {
        let matched = matched_query_terms_for_text(
            "coffee brewing guide",
            &terms(&["coffee", "brewing", "missing"]),
        );
        assert_eq!(matched.len(), 2);
        assert!(matched.contains("coffee"));
        assert!(matched.contains("brewing"));
    }

    #[test]
    fn text_match_empty_when_no_overlap() {
        let matched = matched_query_terms_for_text("hello world", &terms(&["coffee"]));
        assert!(matched.is_empty());
    }

    #[test]
    fn text_match_empty_query_terms() {
        let matched = matched_query_terms_for_text("hello world", &[]);
        assert!(matched.is_empty());
    }

    #[test]
    fn text_match_counts_duplicate_query_terms_once() {
        let matched = matched_query_terms_for_text("coffee brewing", &terms(&["coffee", "coffee"]));
        assert_eq!(matched.len(), 1);
        assert!(matched.contains("coffee"));
    }

    // -- fact_term_set -------------------------------------------------------

    #[test]
    fn fact_term_set_includes_content_and_index_keys() {
        let fact = make_fact("coffee brewing", terms(&["ethiopia yirgacheffe"]));
        let set = fact_term_set(&fact);
        assert!(set.contains("coffee"));
        assert!(set.contains("brewing"));
        assert!(set.contains("ethiopia"));
        assert!(set.contains("yirgacheffe"));
    }

    // -- matched_query_terms_for_fact ----------------------------------------

    #[test]
    fn fact_match_finds_content_overlap() {
        let fact = make_fact("coffee brewing guide", vec![]);
        let matched = matched_query_terms_for_fact(&fact, &terms(&["coffee", "missing"]));
        assert_eq!(matched.len(), 1);
        assert!(matched.contains("coffee"));
    }

    #[test]
    fn fact_match_finds_index_key_overlap() {
        let fact = make_fact("unrelated content", terms(&["ethiopia yirgacheffe"]));
        let matched = matched_query_terms_for_fact(&fact, &terms(&["ethiopia"]));
        assert_eq!(matched.len(), 1);
        assert!(matched.contains("ethiopia"));
    }

    #[test]
    fn fact_match_empty_query_terms() {
        let fact = make_fact("coffee brewing", vec![]);
        let matched = matched_query_terms_for_fact(&fact, &[]);
        assert!(matched.is_empty());
    }

    #[test]
    fn fact_match_empty_when_no_overlap() {
        let fact = make_fact("coffee brewing", terms(&["ethiopia"]));
        let matched = matched_query_terms_for_fact(&fact, &terms(&["tea"]));
        assert!(matched.is_empty());
    }

    // -- is_four_digit_year ---------------------------------------------------

    #[test]
    fn four_digit_year_accepts_years() {
        assert!(is_four_digit_year("2026"));
        assert!(is_four_digit_year("1999"));
    }

    #[test]
    fn four_digit_year_rejects_non_years() {
        assert!(!is_four_digit_year("26"));
        assert!(!is_four_digit_year("abc"));
        assert!(!is_four_digit_year("20261"));
        assert!(!is_four_digit_year("2o26"));
        assert!(!is_four_digit_year(""));
    }
}
