use crate::models::AssembledContextItem;

use super::filtering;
use super::lexical;
use super::params::DefaultContextParams;
use super::ranking::default_episode_fallback_rationale;
use super::scoring::selected_fact_query_term_coverage;
use super::types::{RankedContextFact, RetrievalTier};
use crate::service::error::MemoryError;
use crate::service::query::matched_query_terms_for_text;
use crate::service::service_context::RetrievalContext;

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

pub(super) async fn collect_episode_fallback_items(
    service: &RetrievalContext,
    params: &DefaultContextParams<'_>,
    query: &str,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let episode_records = lexical::select_episode_records_for_query(
        service,
        params.cutoff_iso,
        Some(query),
        params.budget,
    )
    .await?;

    let query_terms = crate::service::query::search_query_terms(query);
    let mut episodes = filtering::filter_episodes_by_constraints(episode_records, params.access);

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
        semantic_available: service.embedding_service.embedding_provider().is_enabled(),
        cutoff: params.cutoff,
        window_start: params.window_start,
        window_end: params.window_end,
        timeline_mode: params.resolved_view_mode == Some("timeline"),
        budget: params.budget,
        fallback_rationale_fn: default_episode_fallback_rationale,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Fact;
    use crate::service::context::types::{RankedContextFact, RetrievalTier};

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
            provenance: crate::models::Provenance::manual(),
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
            provenance: serde_json::json!({}),
            rationale: String::new(),
            retrieval_tier: None,
            reconciliation: None,
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
        assert!(terms.contains("coffee"));
        assert!(terms.contains("brewing"));
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

    // -----------------------------------------------------------------------
    // Tests relocated from context.rs — full scenario coverage for
    // should_prefer_episode_content using realistic query/fact/episode inputs.
    // -----------------------------------------------------------------------

    fn create_test_fact(fact_id: &str, t_valid: chrono::DateTime<chrono::Utc>) -> Fact {
        Fact {
            fact_id: fact_id.to_string(),
            fact_type: "note".to_string(),
            content: "Test content".to_string(),
            quote: "Test quote".to_string(),
            source_episode: "episode:123".to_string(),
            t_valid,
            t_ingested: t_valid,
            t_invalid: None,
            t_invalid_ingested: None,
            confidence: 1.0,
            index_keys: vec![],
            access_count: 0,
            last_accessed: None,
            entity_links: vec![],
            scope: "org".to_string(),
            policy_tags: vec![],
            provenance: crate::models::Provenance::manual(),
            ft_score: 0.0,
        }
    }

    fn create_ranked_test_fact(
        fact_id: &str,
        source_episode: &str,
        t_valid: chrono::DateTime<chrono::Utc>,
        fusion_score: f64,
        ft_score: f64,
        access_count: i64,
        index_keys: &[&str],
    ) -> RankedContextFact {
        let mut fact = create_test_fact(fact_id, t_valid);
        fact.source_episode = source_episode.to_string();
        fact.ft_score = ft_score;
        fact.access_count = access_count;
        fact.index_keys = index_keys.iter().map(|key| (*key).to_string()).collect();

        RankedContextFact {
            fact,
            rationale: "test rationale".to_string(),
            retrieval_tier: RetrievalTier::Direct,
            fusion_score,
            source_priority: 0,
            decayed_confidence: 1.0,
            query_alignment_factor: 1.0,
            grounding_score: 1.0,
            semantic_available: false,
            matched_query_terms: Vec::new(),
            graph_trace: None,
        }
    }

    #[test]
    fn should_prefer_episode_content_when_episode_overlap_is_stronger() {
        let query_terms =
            crate::service::query::search_query_terms("platform planning notes july 2025");
        let fact_time = chrono::DateTime::parse_from_rfc3339("2025-07-13T10:00:00Z")
            .expect("fact timestamp")
            .with_timezone(&chrono::Utc);

        let selected_facts = vec![RankedContextFact {
            fact: Fact {
                content: "July 2025 platform licensing notes for renewal workflow.".to_string(),
                ..create_ranked_test_fact(
                    "fact:noise",
                    "episode:noise",
                    fact_time,
                    1.0,
                    4.0,
                    0,
                    &[],
                )
                .fact
            },
            ..create_ranked_test_fact("fact:noise", "episode:noise", fact_time, 1.0, 4.0, 0, &[])
        }];

        let episode_items = vec![AssembledContextItem {
            fact_id: "episode_fallback:episode:july".to_string(),
            content: "Platform planning notes July 2025: release scope, integrations, and response workflow updates.".to_string(),
            quote: "Platform planning notes July 2025: release scope, integrations, and response workflow updates.".to_string(),
            source_episode: "episode:july".to_string(),
            confidence: 1.0,
            provenance: serde_json::json!({"episode_fallback": true}),
            rationale: "fallback".to_string(),
            retrieval_tier: Some("fallback".to_string()),
            ..Default::default()
        }];

        assert!(should_prefer_episode_content(
            &selected_facts,
            &episode_items,
            &query_terms,
        ));
    }

    #[test]
    fn should_not_prefer_episode_content_when_fact_overlap_is_equal_or_better() {
        let query_terms =
            crate::service::query::search_query_terms("platform planning notes july 2025");
        let fact_time = chrono::DateTime::parse_from_rfc3339("2025-07-13T10:00:00Z")
            .expect("fact timestamp")
            .with_timezone(&chrono::Utc);

        let selected_facts = vec![RankedContextFact {
            fact: Fact {
                content: "Platform planning notes July 2025 for release scope and integrations."
                    .to_string(),
                ..create_ranked_test_fact(
                    "fact:strong",
                    "episode:strong",
                    fact_time,
                    1.0,
                    5.0,
                    0,
                    &[],
                )
                .fact
            },
            ..create_ranked_test_fact("fact:strong", "episode:strong", fact_time, 1.0, 5.0, 0, &[])
        }];

        let episode_items = vec![AssembledContextItem {
            fact_id: "episode_fallback:episode:july".to_string(),
            content: "Platform notes July 2025 with rollout reminders.".to_string(),
            quote: "Platform notes July 2025 with rollout reminders.".to_string(),
            source_episode: "episode:july".to_string(),
            confidence: 1.0,
            provenance: serde_json::json!({"episode_fallback": true}),
            rationale: "fallback".to_string(),
            retrieval_tier: Some("fallback".to_string()),
            ..Default::default()
        }];

        assert!(!should_prefer_episode_content(
            &selected_facts,
            &episode_items,
            &query_terms,
        ));
    }

    #[test]
    fn should_not_prefer_episode_content_over_graph_expanded_matches() {
        let query_terms = crate::service::query::search_query_terms("bob jones");
        let fact_time = chrono::DateTime::parse_from_rfc3339("2025-07-13T10:00:00Z")
            .expect("fact timestamp")
            .with_timezone(&chrono::Utc);

        let selected_facts = vec![RankedContextFact {
            fact: Fact {
                content: "Prototype milestone is blocked.".to_string(),
                ..create_ranked_test_fact(
                    "fact:graph",
                    "episode:graph",
                    fact_time,
                    1.0,
                    0.0,
                    0,
                    &[],
                )
                .fact
            },
            retrieval_tier: RetrievalTier::GraphExpanded,
            ..create_ranked_test_fact("fact:graph", "episode:graph", fact_time, 1.0, 0.0, 0, &[])
        }];

        let episode_items = vec![AssembledContextItem {
            fact_id: "episode_fallback:episode:july".to_string(),
            content: "Alice Smith met Bob Jones to plan next steps.".to_string(),
            quote: "Alice Smith met Bob Jones to plan next steps.".to_string(),
            source_episode: "episode:july".to_string(),
            confidence: 1.0,
            provenance: serde_json::json!({"episode_fallback": true}),
            rationale: "fallback".to_string(),
            retrieval_tier: Some("fallback".to_string()),
            ..Default::default()
        }];

        assert!(!should_prefer_episode_content(
            &selected_facts,
            &episode_items,
            &query_terms,
        ));
    }

    #[test]
    fn should_not_prefer_episode_content_when_fact_captures_best_matching_summary_line() {
        let query_terms =
            crate::service::query::search_query_terms("help kickoff naming localization alignment");
        let fact_time = chrono::DateTime::parse_from_rfc3339("2026-04-13T09:00:00Z")
            .expect("fact timestamp")
            .with_timezone(&chrono::Utc);

        let selected_facts = vec![RankedContextFact {
            fact: Fact {
                content: "Help kickoff is open; naming and localization details need alignment across products.".to_string(),
                ..create_ranked_test_fact(
                    "fact:docs",
                    "episode:docs",
                    fact_time,
                    1.0,
                    6.0,
                    0,
                    &[],
                )
                .fact
            },
            ..create_ranked_test_fact("fact:docs", "episode:docs", fact_time, 1.0, 6.0, 0, &[])
        }];

        let episode_items = vec![AssembledContextItem {
            fact_id: "episode_fallback:episode:docs".to_string(),
            content: "Documentation and localization facts for product materials:\n\n- Fact: Help kickoff is open; naming and localization details need alignment.\n- Fact: Docs team is asking for final terminology in both languages.".to_string(),
            quote: "Documentation and localization facts for product materials:\n\n- Fact: Help kickoff is open; naming and localization details need alignment.\n- Fact: Docs team is asking for final terminology in both languages.".to_string(),
            source_episode: "episode:docs".to_string(),
            confidence: 1.0,
            provenance: serde_json::json!({"episode_fallback": true}),
            rationale: "fallback".to_string(),
            retrieval_tier: Some("fallback".to_string()),
            ..Default::default()
        }];

        assert!(!should_prefer_episode_content(
            &selected_facts,
            &episode_items,
            &query_terms,
        ));
    }

    #[test]
    fn should_not_prefer_episode_content_when_selected_facts_collectively_cover_query() {
        let query_terms = crate::service::query::search_query_terms(
            "suite alpha beta gamma shared platform q3 2026 roadmap rollout controls versioning graphical rules",
        );
        let fact_time = chrono::DateTime::parse_from_rfc3339("2026-04-13T09:00:00Z")
            .expect("fact timestamp")
            .with_timezone(&chrono::Utc);

        let selected_facts = vec![
            RankedContextFact {
                fact: Fact {
                    content: "Suite Alpha, Suite Beta, and Suite Gamma launch on the shared platform in Q3 2026.".to_string(),
                    ..create_ranked_test_fact(
                        "fact:launch",
                        "episode:launch-summary",
                        fact_time,
                        1.0,
                        6.0,
                        0,
                        &[],
                    )
                    .fact
                },
                ..create_ranked_test_fact(
                    "fact:launch",
                    "episode:launch-summary",
                    fact_time,
                    1.0,
                    6.0,
                    0,
                    &[],
                )
            },
            RankedContextFact {
                fact: Fact {
                    content: "Roadmap adds staged rollout controls and export automation in Q4 2026.".to_string(),
                    ..create_ranked_test_fact(
                        "fact:roadmap",
                        "episode:launch-summary",
                        fact_time,
                        0.9,
                        5.0,
                        0,
                        &[],
                    )
                    .fact
                },
                ..create_ranked_test_fact(
                    "fact:roadmap",
                    "episode:launch-summary",
                    fact_time,
                    0.9,
                    5.0,
                    0,
                    &[],
                )
            },
            RankedContextFact {
                fact: Fact {
                    content: "Following wave adds workflow versioning and graphical rules.".to_string(),
                    ..create_ranked_test_fact(
                        "fact:followup",
                        "episode:launch-summary",
                        fact_time,
                        0.8,
                        4.0,
                        0,
                        &[],
                    )
                    .fact
                },
                ..create_ranked_test_fact(
                    "fact:followup",
                    "episode:launch-summary",
                    fact_time,
                    0.8,
                    4.0,
                    0,
                    &[],
                )
            },
        ];

        let episode_items = vec![AssembledContextItem {
            fact_id: "episode_fallback:episode:launch-summary".to_string(),
            content: "Quarterly launch brief:\n- Suite Alpha, Suite Beta, and Suite Gamma launch on the shared platform in Q3 2026.\n- Technical preview is September 30, 2026, with general availability in late October 2026.\n- Roadmap adds staged rollout controls and export automation in Q4 2026.\n- Following wave adds workflow versioning and graphical rules.".to_string(),
            quote: "Quarterly launch brief:\n- Suite Alpha, Suite Beta, and Suite Gamma launch on the shared platform in Q3 2026.\n- Technical preview is September 30, 2026, with general availability in late October 2026.\n- Roadmap adds staged rollout controls and export automation in Q4 2026.\n- Following wave adds workflow versioning and graphical rules.".to_string(),
            source_episode: "episode:launch-summary".to_string(),
            confidence: 1.0,
            provenance: serde_json::json!({"episode_fallback": true}),
            rationale: "fallback".to_string(),
            retrieval_tier: Some("fallback".to_string()),
            ..Default::default()
        }];

        assert!(!should_prefer_episode_content(
            &selected_facts,
            &episode_items,
            &query_terms,
        ));
    }
}
