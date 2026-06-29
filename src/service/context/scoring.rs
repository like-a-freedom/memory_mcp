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

    // Build provenance output: start with structured fields, then add
    // query-time enrichment (matched_query_terms, graph_trace).
    let mut provenance = fact.provenance.to_json_value();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Fact;
    use crate::service::context::graph::GraphTrace;
    use crate::service::context::ranking;

    fn make_fact(content: &str, index_keys: Vec<String>) -> Fact {
        Fact {
            fact_id: "test:1".into(),
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

    fn make_ranked(
        fact: Fact,
        matched_query_terms: Vec<String>,
        tier: ranking::RetrievalTier,
    ) -> ranking::RankedContextFact {
        ranking::RankedContextFact {
            fact,
            rationale: "test".into(),
            retrieval_tier: tier,
            fusion_score: 0.5,
            source_priority: 0,
            decayed_confidence: 0.8,
            query_alignment_factor: 1.0,
            grounding_score: 0.5,
            semantic_available: false,
            matched_query_terms,
            graph_trace: None,
        }
    }

    // -- selected_fact_matched_terms ---------------------------------------

    #[test]
    fn selected_fact_matched_terms_finds_match_in_fact_content() {
        let fact = make_fact("finding relevant content for query terms", vec![]);
        let ranked = make_ranked(
            fact,
            vec!["relevant".to_string(), "content".to_string()],
            ranking::RetrievalTier::Direct,
        );
        let terms = selected_fact_matched_terms(
            &[ranked],
            &[
                "relevant".to_string(),
                "content".to_string(),
                "missing".to_string(),
            ],
        );
        let mut expected = HashSet::new();
        expected.insert("relevant".to_string());
        expected.insert("content".to_string());
        assert_eq!(terms, expected);
    }

    #[test]
    fn selected_fact_matched_terms_matches_from_index_keys() {
        let fact = make_fact("unrelated content", vec!["python".to_string()]);
        let ranked = make_ranked(
            fact,
            vec!["python".to_string()],
            ranking::RetrievalTier::Direct,
        );
        let terms = selected_fact_matched_terms(&[ranked], &["python".to_string()]);
        let mut expected = HashSet::new();
        expected.insert("python".to_string());
        assert_eq!(terms, expected);
    }

    #[test]
    fn selected_fact_matched_terms_empty_when_no_match() {
        let fact = make_fact("hello world", vec![]);
        let ranked = make_ranked(fact, vec![], ranking::RetrievalTier::Direct);
        let terms = selected_fact_matched_terms(&[ranked], &["coffee".to_string()]);
        assert!(terms.is_empty());
    }

    #[test]
    fn selected_fact_matched_terms_empty_query_terms() {
        let fact = make_fact("hello world", vec![]);
        let ranked = make_ranked(fact, vec![], ranking::RetrievalTier::Direct);
        let terms = selected_fact_matched_terms(&[ranked], &[]);
        assert!(terms.is_empty());
    }

    #[test]
    fn selected_fact_matched_terms_empty_selected_facts() {
        let terms = selected_fact_matched_terms(&[], &["hello".to_string()]);
        assert!(terms.is_empty());
    }

    #[test]
    fn selected_fact_matched_terms_deduplicates_across_facts() {
        let f1 = make_fact("coffee brewing techniques require practice", vec![]);
        let f2 = make_fact("coffee beans from ethiopia are excellent", vec![]);
        let facts = vec![
            make_ranked(
                f1,
                vec!["coffee".to_string()],
                ranking::RetrievalTier::Direct,
            ),
            make_ranked(
                f2,
                vec!["coffee".to_string(), "ethiopia".to_string()],
                ranking::RetrievalTier::TemporalExpanded,
            ),
        ];
        let terms = selected_fact_matched_terms(
            &facts,
            &[
                "coffee".to_string(),
                "brewing".to_string(),
                "ethiopia".to_string(),
            ],
        );
        let mut expected = HashSet::new();
        expected.insert("coffee".to_string());
        expected.insert("brewing".to_string());
        expected.insert("ethiopia".to_string());
        assert_eq!(terms, expected);
    }

    // -- selected_fact_query_term_coverage ---------------------------------

    #[test]
    fn coverage_counts_distinct_matched_terms() {
        let f1 = make_fact("coffee brewing guide", vec![]);
        let f2 = make_fact("python coding guide", vec![]);
        let facts = vec![
            make_ranked(
                f1,
                vec!["coffee".to_string()],
                ranking::RetrievalTier::Direct,
            ),
            make_ranked(
                f2,
                vec!["guide".to_string()],
                ranking::RetrievalTier::GraphExpanded,
            ),
        ];
        assert_eq!(
            selected_fact_query_term_coverage(
                &facts,
                &[
                    "coffee".to_string(),
                    "guide".to_string(),
                    "missing".to_string()
                ]
            ),
            2
        );
    }

    #[test]
    fn coverage_zero_for_no_matches() {
        let fact = make_fact("hello world", vec![]);
        let ranked = make_ranked(fact, vec![], ranking::RetrievalTier::SemanticExpanded);
        assert_eq!(
            selected_fact_query_term_coverage(&[ranked], &["rareword".to_string()]),
            0
        );
    }

    #[test]
    fn coverage_zero_for_empty_selected_facts() {
        assert_eq!(
            selected_fact_query_term_coverage(&[], &["hello".to_string()]),
            0
        );
    }

    // -- ranked_fact_to_item ------------------------------------------------

    #[test]
    fn ranked_fact_to_item_preserves_fields() {
        let fact = make_fact("test content", vec![]);
        let ranked = make_ranked(
            fact,
            vec!["hello".to_string()],
            ranking::RetrievalTier::Direct,
        );
        let cutoff = chrono::Utc::now();
        let item = ranked_fact_to_item(ranked, cutoff, |_, _| 0.75);
        assert_eq!(item.content, "test content");
        assert_eq!(item.fact_id, "test:1");
        assert_eq!(item.source_episode, "ep:1");
        assert_eq!(item.retrieval_tier.as_deref(), Some("direct"));
    }

    #[test]
    fn ranked_fact_to_item_applies_decay_fn() {
        let fact = make_fact("test content", vec![]);
        let ranked = make_ranked(fact, vec![], ranking::RetrievalTier::Direct);
        let cutoff = chrono::Utc::now();
        let item = ranked_fact_to_item(ranked, cutoff, |_, _| 0.42);
        assert!((item.confidence - 0.42).abs() < f64::EPSILON);
    }

    #[test]
    fn ranked_fact_to_item_sets_relevance_and_grounding() {
        let fact = make_fact("test content", vec![]);
        let ranked = make_ranked(fact, vec![], ranking::RetrievalTier::Direct);
        let cutoff = chrono::Utc::now();
        let item = ranked_fact_to_item(ranked, cutoff, |_, _| 0.5);
        assert!(item.relevance.is_some());
        assert!(item.grounding.is_some());
        assert!(item.semantic_available.is_some());
    }

    #[test]
    fn ranked_fact_to_item_injects_matched_query_terms_into_provenance() {
        let fact = make_fact("test content", vec![]);
        let ranked = make_ranked(
            fact,
            vec!["hello".to_string(), "world".to_string()],
            ranking::RetrievalTier::TemporalExpanded,
        );
        let cutoff = chrono::Utc::now();
        let item = ranked_fact_to_item(ranked, cutoff, |_, _| 0.5);
        let provenance = item
            .provenance
            .as_object()
            .expect("provenance should be object");
        assert_eq!(
            provenance["matched_query_terms"]
                .as_array()
                .map(|a| a.len()),
            Some(2),
        );
    }

    #[test]
    fn ranked_fact_to_item_handles_non_object_provenance() {
        let fact = Fact {
            provenance: crate::models::Provenance::from_json_value(&serde_json::json!(
                "I am a string, not an object"
            )),
            ..make_fact("test", vec![])
        };
        let ranked = make_ranked(
            fact,
            vec!["hello".to_string()],
            ranking::RetrievalTier::Direct,
        );
        let cutoff = chrono::Utc::now();
        let item = ranked_fact_to_item(ranked, cutoff, |_, _| 0.5);
        assert_eq!(item.provenance["matched_query_terms"][0], "hello");
    }

    #[test]
    fn ranked_fact_to_item_handles_graph_trace() {
        let trace = GraphTrace {
            anchor_entity_id: "e:42".to_string(),
            anchor_canonical_name: "Acme".to_string(),
            hop_count: 1,
            path: vec!["e:42".to_string(), "e:99".to_string()],
        };
        let fact = make_fact("test", vec![]);
        let ranked = ranking::RankedContextFact {
            graph_trace: Some(trace),
            ..make_ranked(fact, vec![], ranking::RetrievalTier::GraphExpanded)
        };
        let cutoff = chrono::Utc::now();
        let item = ranked_fact_to_item(ranked, cutoff, |_, _| 0.5);
        let trace_obj = &item.provenance["graph_trace"];
        assert_eq!(trace_obj["anchor_entity_id"], "e:42");
        assert_eq!(trace_obj["hop_count"], 1);
    }

    #[test]
    fn ranked_fact_to_item_retrieval_tier_matches_input() {
        let fact = make_fact("test", vec![]);
        let ranked = make_ranked(fact, vec![], ranking::RetrievalTier::SemanticExpanded);
        let cutoff = chrono::Utc::now();
        let item = ranked_fact_to_item(ranked, cutoff, |_, _| 0.5);
        assert_eq!(item.retrieval_tier.as_deref(), Some("semantic"));
    }
}
