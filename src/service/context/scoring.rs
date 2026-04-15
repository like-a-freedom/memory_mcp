use std::collections::HashSet;

use crate::models::{AssembledContextItem, Fact};

use super::ranking;

/// Converts a ranked fact into an assembled context item with decayed confidence.
pub(super) fn ranked_fact_to_item(
    ranked: ranking::RankedContextFact,
    cutoff: chrono::DateTime<chrono::Utc>,
    decay_fn: impl FnOnce(&Fact, chrono::DateTime<chrono::Utc>) -> f64,
) -> AssembledContextItem {
    let relevance = ranking::normalized_relevance_score(&ranked);
    let grounding = ranked.grounding_score;
    let semantic_available = ranked.semantic_available;
    let confidence = decay_fn(&ranked.fact, cutoff);
    AssembledContextItem {
        fact_id: ranked.fact.fact_id,
        content: ranked.fact.content,
        quote: ranked.fact.quote,
        source_episode: ranked.fact.source_episode,
        confidence,
        relevance: Some(relevance),
        grounding: Some(grounding),
        semantic_available: Some(semantic_available),
        provenance: ranked.fact.provenance,
        rationale: ranked.rationale,
        retrieval_tier: Some(ranked.retrieval_tier.as_str().to_string()),
    }
}

fn matched_query_terms_for_fact(fact: &Fact, query_terms: &[String]) -> HashSet<String> {
    if query_terms.is_empty() {
        return HashSet::new();
    }

    let mut fact_terms = crate::service::query::search_query_terms(&fact.content)
        .into_iter()
        .collect::<HashSet<_>>();
    for index_key in &fact.index_keys {
        fact_terms.extend(crate::service::query::search_query_terms(index_key));
    }

    query_terms
        .iter()
        .filter(|term| fact_terms.contains(term.as_str()))
        .cloned()
        .collect()
}

pub(super) fn selected_fact_matched_terms(
    selected_facts: &[ranking::RankedContextFact],
    query_terms: &[String],
) -> HashSet<String> {
    let mut matched_terms = HashSet::new();

    for ranked in selected_facts {
        matched_terms.extend(matched_query_terms_for_fact(&ranked.fact, query_terms));
    }

    matched_terms
}

pub(super) fn selected_fact_query_term_coverage(
    selected_facts: &[ranking::RankedContextFact],
    query_terms: &[String],
) -> usize {
    selected_fact_matched_terms(selected_facts, query_terms).len()
}
