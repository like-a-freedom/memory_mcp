use crate::models::AssembledContextItem;

use super::filtering;
use super::lexical;
use super::params::DefaultContextParams;
use super::ranking::{RankedContextFact, RetrievalTier, default_episode_fallback_rationale};
use super::scoring::selected_fact_query_term_coverage;
use crate::service::MemoryService;
use crate::service::error::MemoryError;

fn matched_query_terms_for_text(text: &str, query_terms: &[String]) -> Vec<String> {
    if query_terms.is_empty() {
        return Vec::new();
    }

    let content_terms = crate::service::query::search_query_terms(text)
        .into_iter()
        .collect::<std::collections::HashSet<_>>();

    query_terms
        .iter()
        .filter(|term| content_terms.contains(term.as_str()))
        .cloned()
        .collect()
}

pub(super) fn should_prefer_episode_content(
    selected_facts: &[RankedContextFact],
    episode_items: &[AssembledContextItem],
    query_terms: &[String],
) -> bool {
    if episode_items.is_empty() {
        return false;
    }

    if selected_facts
        .iter()
        .any(|fact| fact.retrieval_tier == RetrievalTier::GraphExpanded)
    {
        return false;
    }

    let best_fact_overlap = selected_facts
        .iter()
        .map(|fact| lexical::lexical_query_score_for_fact(&fact.fact, query_terms))
        .max()
        .unwrap_or(0);

    let Some(best_episode_item) = episode_items
        .iter()
        .max_by_key(|item| lexical::lexical_query_score_for_text(&item.content, query_terms))
    else {
        return false;
    };

    let best_episode_overlap =
        lexical::lexical_query_score_for_text(&best_episode_item.content, query_terms);

    if best_episode_overlap <= best_fact_overlap {
        return false;
    }

    let best_episode_term_coverage =
        matched_query_terms_for_text(&best_episode_item.content, query_terms).len();
    let selected_fact_term_coverage =
        selected_fact_query_term_coverage(selected_facts, query_terms);

    best_episode_term_coverage > selected_fact_term_coverage
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Fact;
    use crate::service::context::ranking::{RankedContextFact, RetrievalTier};

    fn make_fact(content: &str) -> Fact {
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
            index_keys: vec![],
            access_count: 0,
            last_accessed: None,
            entity_links: vec![],
            scope: "org".into(),
            policy_tags: vec![],
            provenance: serde_json::Value::Null,
            ft_score: 0.0,
        }
    }

    fn make_ranked(fact: Fact, tier: RetrievalTier) -> RankedContextFact {
        RankedContextFact {
            fact,
            rationale: "test".into(),
            retrieval_tier: tier,
            fusion_score: 0.5,
            source_priority: 0,
            decayed_confidence: 0.8,
            query_alignment_factor: 1.0,
            grounding_score: 0.5,
            semantic_available: false,
            matched_query_terms: vec![],
            graph_trace: None,
        }
    }

    fn make_episode_item(content: &str) -> AssembledContextItem {
        AssembledContextItem {
            fact_id: "ep:1".into(),
            content: content.into(),
            quote: String::new(),
            source_episode: "ep:1".into(),
            confidence: 0.9,
            relevance: None,
            grounding: None,
            semantic_available: None,
            provenance: serde_json::Value::Null,
            rationale: String::new(),
            retrieval_tier: None,
        }
    }

    // -- should_prefer_episode_content -------------------------------------

    #[test]
    fn prefers_false_when_no_episode_items() {
        assert!(!should_prefer_episode_content(
            &[],
            &[],
            &["coffee".to_string()]
        ));
    }

    #[test]
    fn prefers_false_when_graph_expanded_facts_present() {
        let fact = make_fact("coffee brewing guide");
        let ranked = make_ranked(fact, RetrievalTier::GraphExpanded);
        let episodes = vec![make_episode_item("coffee brewing techniques")];
        assert!(!should_prefer_episode_content(
            &[ranked],
            &episodes,
            &["coffee".to_string()]
        ));
    }

    #[test]
    fn prefers_false_when_episode_overlap_not_better() {
        let fact = make_fact("coffee brewing guide for beginners");
        let ranked = make_ranked(fact, RetrievalTier::Direct);
        let episodes = vec![make_episode_item("unrelated topic")];
        assert!(!should_prefer_episode_content(
            &[ranked],
            &episodes,
            &["coffee".to_string()]
        ));
    }

    #[test]
    fn prefers_true_when_episode_coverage_exceeds_fact() {
        let fact = make_fact("coffee");
        let ranked = make_ranked(fact, RetrievalTier::Direct);
        let episodes = vec![make_episode_item("coffee brewing techniques from ethiopia")];
        assert!(should_prefer_episode_content(
            &[ranked],
            &episodes,
            &[
                "coffee".to_string(),
                "brewing".to_string(),
                "ethiopia".to_string()
            ]
        ),);
    }

    #[test]
    fn prefers_false_when_fact_has_better_coverage() {
        let fact = make_fact("coffee brewing techniques from ethiopia");
        let ranked = make_ranked(fact, RetrievalTier::Direct);
        let episodes = vec![make_episode_item("coffee")];
        assert!(!should_prefer_episode_content(
            &[ranked],
            &episodes,
            &[
                "coffee".to_string(),
                "brewing".to_string(),
                "ethiopia".to_string()
            ]
        ),);
    }

    #[test]
    fn prefers_false_with_empty_query_terms() {
        let fact = make_fact("coffee");
        let ranked = make_ranked(fact, RetrievalTier::Direct);
        let episodes = vec![make_episode_item("coffee")];
        assert!(!should_prefer_episode_content(&[ranked], &episodes, &[]));
    }

    // -- matched_query_terms_for_text --------------------------------------

    #[test]
    fn matched_terms_finds_overlap() {
        let terms = matched_query_terms_for_text(
            "coffee brewing guide",
            &[
                "coffee".to_string(),
                "brewing".to_string(),
                "missing".to_string(),
            ],
        );
        assert_eq!(terms.len(), 2);
        assert!(terms.contains(&"coffee".to_string()));
        assert!(terms.contains(&"brewing".to_string()));
    }

    #[test]
    fn matched_terms_empty_when_no_overlap() {
        let terms = matched_query_terms_for_text("hello world", &["coffee".to_string()]);
        assert!(terms.is_empty());
    }

    #[test]
    fn matched_terms_empty_query() {
        let terms = matched_query_terms_for_text("hello world", &[]);
        assert!(terms.is_empty());
    }
}

pub(super) async fn collect_episode_fallback_items(
    service: &MemoryService,
    params: &DefaultContextParams<'_>,
    query: &str,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let episode_records = lexical::select_episode_records_for_query(
        service,
        params.namespace,
        params.scope,
        params.cutoff_iso,
        Some(query),
        params.budget,
        params.project_opt,
    )
    .await?;

    let query_terms = crate::service::query::search_query_terms(query);
    let mut episodes = filtering::filter_episodes_by_constraints(
        episode_records,
        params.access,
        params.project_opt,
    );

    episodes.sort_by(|left, right| {
        lexical::lexical_query_score_for_text(&right.content, &query_terms)
            .cmp(&lexical::lexical_query_score_for_text(
                &left.content,
                &query_terms,
            ))
            .then_with(|| right.t_ref.cmp(&left.t_ref))
            .then_with(|| left.episode_id.cmp(&right.episode_id))
    });

    use super::views::{EpisodeFallbackParams, build_episode_fallback_items};

    Ok(build_episode_fallback_items(EpisodeFallbackParams {
        episodes,
        query_opt: Some(query),
        semantic_available: service.embedding_provider.is_enabled(),
        scope: params.scope,
        cutoff: params.cutoff,
        window_start: params.window_start,
        window_end: params.window_end,
        timeline_mode: params.resolved_view_mode == Some("timeline"),
        budget: params.budget,
        fallback_rationale_fn: default_episode_fallback_rationale,
    }))
}
