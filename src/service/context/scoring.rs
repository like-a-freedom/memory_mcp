use std::collections::HashSet;

use serde_json::json;

use crate::models::{AssembledContextItem, Fact};

use super::ranking;

/// Converts a ranked fact into an assembled context item with decayed confidence.
pub(super) fn ranked_fact_to_item(
    ranked: ranking::RankedContextFact,
    cutoff: chrono::DateTime<chrono::Utc>,
    decay_fn: impl FnOnce(&Fact, chrono::DateTime<chrono::Utc>) -> f64,
) -> AssembledContextItem {
    let relevance = ranking::normalized_relevance_score(&ranked);
    let ranking::RankedContextFact {
        fact,
        rationale,
        retrieval_tier,
        grounding_score: grounding,
        semantic_available,
        matched_query_terms,
        graph_trace,
        ..
    } = ranked;
    let confidence = decay_fn(&fact, cutoff);
    let mut provenance = fact.provenance;

    if !provenance.is_object() {
        provenance = json!({});
    }
    if let Some(map) = provenance.as_object_mut() {
        if !matched_query_terms.is_empty() {
            map.insert(
                "matched_query_terms".to_string(),
                json!(matched_query_terms),
            );
        }
        if let Some(trace) = &graph_trace {
            map.insert(
                "graph_trace".to_string(),
                json!({
                    "anchor_entity_id": trace.anchor_entity_id,
                    "anchor_canonical_name": trace.anchor_canonical_name,
                    "hop_count": trace.hop_count,
                    "path": trace.path,
                }),
            );
        }
    }

    AssembledContextItem {
        fact_id: fact.fact_id,
        content: fact.content,
        quote: fact.quote,
        source_episode: fact.source_episode,
        confidence,
        relevance: Some(relevance),
        grounding: Some(grounding),
        semantic_available: Some(semantic_available),
        provenance,
        rationale,
        retrieval_tier: Some(retrieval_tier.as_str().to_string()),
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
