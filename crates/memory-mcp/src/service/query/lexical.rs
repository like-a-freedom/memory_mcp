//! Shared lexical-relevance primitives.
//!
//! Single home for the term-overlap helpers used across context assembly
//! (ranking, budgeting, rescue, scoring) and temporal query parsing.
//! Callers import from here instead of keeping local copies.

use std::collections::HashSet;

use crate::models::Fact;

use super::search_query_terms;

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
