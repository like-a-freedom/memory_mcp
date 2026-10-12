use crate::models::AssembledContextItem;
use std::collections::HashSet;

use super::admission::{build_query_evidence, episode_candidate_is_admitted_at_reference_time};
use super::filtering;
use super::params::DefaultContextParams;
use super::ranking::default_episode_fallback_rationale;
use super::scoring::selected_fact_query_term_coverage;
use super::types::{RankedContextFact, RetrievalTier};
use crate::error::MemoryError;
use crate::memory::retrieval::RetrievalContext;
use crate::shared::search_lexical::matched_query_terms_for_text;

pub(super) fn should_prefer_episode_content(
    selected_facts: &[RankedContextFact],
    episode_items: &[AssembledContextItem],
    query_terms: &[String],
) -> bool {
    if episode_items.is_empty() || query_terms.is_empty() {
        return false;
    }

    if episode_items.iter().any(|episode| {
        selected_facts
            .iter()
            .any(|fact| fact.fact.source_episode == episode.source_episode)
    }) {
        return false;
    }

    if selected_facts.iter().any(|fact| {
        matches!(
            fact.retrieval_tier,
            RetrievalTier::SourceExpanded | RetrievalTier::GraphExpanded
        )
    }) {
        return false;
    }

    let selected_fact_terms = selected_facts
        .iter()
        .flat_map(|fact| matched_query_terms_for_text(&fact.fact.content, query_terms))
        .collect::<HashSet<_>>();
    let episode_terms = episode_items
        .iter()
        .flat_map(|item| matched_query_terms_for_text(&item.content, query_terms))
        .collect::<HashSet<_>>();
    if !selected_fact_terms.is_subset(&episode_terms) {
        return false;
    }

    let best_fact_reranker = max_consistent_reranker_score(
        selected_facts
            .iter()
            .map(|fact| fact.evidence.reranking.as_ref().and_then(reranker_score)),
    );
    let best_episode_reranker = max_consistent_reranker_score(
        episode_items
            .iter()
            .map(|item| item.provenance.get("reranking").and_then(reranker_score)),
    );
    if let (Some((fact_signature, fact_score)), Some((episode_signature, episode_score))) =
        (best_fact_reranker, best_episode_reranker)
        && fact_signature == episode_signature
    {
        let episode_floor = episode_items
            .iter()
            .filter(|item| {
                item.provenance
                    .get("reranking")
                    .and_then(reranker_score)
                    .is_some_and(|(signature, score)| {
                        signature == episode_signature && score == episode_score
                    })
            })
            .filter_map(|item| {
                item.provenance
                    .get("reranking")
                    .and_then(|evidence| evidence.get("admission_floor"))
                    .and_then(serde_json::Value::as_f64)
            })
            .max_by(f64::total_cmp);
        if episode_floor.is_some_and(|floor| episode_score < floor) {
            return false;
        }
        if episode_score > fact_score {
            return true;
        }
    }

    let best_fact_overlap = selected_facts
        .iter()
        .map(|fact| super::lexical::lexical_query_score_for_fact(&fact.fact, query_terms))
        .max()
        .unwrap_or(0);

    let Some(best_episode_item) = episode_items.iter().max_by_key(|item| {
        super::lexical::lexical_query_score_for_text(&item.content, query_terms)
    }) else {
        return false;
    };

    let best_episode_overlap =
        super::lexical::lexical_query_score_for_text(&best_episode_item.content, query_terms);

    if best_episode_overlap <= best_fact_overlap {
        return false;
    }

    let best_episode_term_coverage =
        matched_query_terms_for_text(&best_episode_item.content, query_terms).len();
    let selected_fact_term_coverage =
        selected_fact_query_term_coverage(selected_facts, query_terms);

    best_episode_term_coverage > selected_fact_term_coverage
}

fn reranker_score(evidence: &serde_json::Value) -> Option<(&str, f64)> {
    let signature = evidence.get("model_signature")?.as_str()?;
    let score = evidence.get("relevance_logit")?.as_f64()?;
    (!signature.is_empty() && score.is_finite()).then_some((signature, score))
}

fn max_consistent_reranker_score<'a>(
    evidence: impl IntoIterator<Item = Option<(&'a str, f64)>>,
) -> Option<(&'a str, f64)> {
    let mut signature = None;
    let mut max_score: Option<f64> = None;
    for (candidate_signature, score) in evidence.into_iter().collect::<Option<Vec<_>>>()? {
        if signature.is_some_and(|signature| signature != candidate_signature) {
            return None;
        }
        signature = Some(candidate_signature);
        max_score = Some(max_score.map_or(score, |current| current.max(score)));
    }
    Some((signature?, max_score?))
}

pub(super) async fn collect_episode_fallback_items(
    service: &RetrievalContext,
    params: &DefaultContextParams<'_>,
    query: &str,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let response_budget = params.budget.max(1) as usize;
    let candidate_budget = super::reranking::candidate_pool_limit(
        response_budget,
        service.embedding_provider.reranking_signature().is_some(),
    );
    let candidate_limit = candidate_budget.min(i32::MAX as usize) as i32;
    let episode_records = super::lexical::select_episode_records_for_query(
        service,
        params.cutoff_iso,
        Some(query),
        candidate_limit,
    )
    .await?;

    let query_terms = crate::shared::search::search_query_terms(query);
    let query_evidence = build_query_evidence(Some(query), Vec::new(), &[], params.query_flags);
    let reranker_available = service.embedding_provider.reranking_signature().is_some();
    let mut episodes = filtering::filter_episodes_by_constraints(episode_records, params.access);
    episodes.retain(|episode| {
        if reranker_available {
            super::admission::episode_candidate_is_rerankable_at_reference_time(
                &query_evidence,
                &episode.content,
                Some(episode.t_ref),
            )
        } else {
            episode_candidate_is_admitted_at_reference_time(
                &query_evidence,
                &episode.content,
                Some(episode.t_ref),
            )
        }
    });

    episodes.sort_by(|left, right| {
        super::lexical::lexical_query_score_for_text(&right.content, &query_terms)
            .cmp(&super::lexical::lexical_query_score_for_text(
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
        cutoff: params.cutoff,
        window_start: params.window_start,
        window_end: params.window_end,
        timeline_mode: params.resolved_view_mode == Some("timeline"),
        budget: candidate_limit,
        fallback_rationale_fn: default_episode_fallback_rationale,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::retrieval::types::{RankedContextFact, RetrievalTier};
    use crate::models::Fact;

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
            evidence: crate::memory::retrieval::types::CandidateEvidence::default(),
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
            retrieval_evidence: None,
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
    fn prefers_false_when_source_expanded_facts_present() {
        let fact = make_fact("coffee brewing guide");
        let ranked = make_ranked(fact, RetrievalTier::SourceExpanded);
        let episodes = vec![make_episode_item("coffee brewing techniques")];
        assert!(!should_prefer_episode_content(
            &[ranked],
            &episodes,
            &["coffee".to_string()]
        ));
    }

    #[test]
    fn prefers_a_relevant_episode_that_covers_more_query_terms_than_its_best_fact() {
        let mut ranked = make_ranked(
            make_fact("HTTP body diagnostics record buffered bytes"),
            RetrievalTier::Direct,
        );
        ranked.evidence.reranking = Some(serde_json::json!({
            "model_signature": "reranksig:test",
            "relevance_logit": 3.0,
            "admission_floor": 0.0,
        }));
        let mut episode = make_episode_item(
            "HTTP body measurements and diagnostics include buffered bytes and retained resources",
        );
        episode.source_episode = "episode:other".into();
        episode.provenance["reranking"] = serde_json::json!({
            "model_signature": "reranksig:test",
            "relevance_logit": 0.5,
            "admission_floor": 0.0,
        });

        assert!(should_prefer_episode_content(
            &[ranked],
            &[episode],
            &[
                "http".to_string(),
                "body".to_string(),
                "measurements".to_string(),
                "diagnostics".to_string(),
                "retained".to_string(),
                "resources".to_string(),
            ]
        ));
    }

    #[test]
    fn prefers_ranked_facts_over_raw_fallback_from_the_same_source_episode() {
        let mut ranked = make_ranked(
            make_fact("HTTP body diagnostics record buffered bytes"),
            RetrievalTier::Direct,
        );
        ranked.evidence.reranking = Some(serde_json::json!({
            "model_signature": "reranksig:test",
            "relevance_logit": 0.2,
            "admission_floor": 0.0,
        }));
        let mut episode =
            make_episode_item("HTTP body measurements and diagnostics include buffered bytes");
        episode.provenance["reranking"] = serde_json::json!({
            "model_signature": "reranksig:test",
            "relevance_logit": 1.0,
            "admission_floor": 0.0,
        });

        assert!(!should_prefer_episode_content(
            &[ranked],
            &[episode],
            &[
                "http".to_string(),
                "body".to_string(),
                "diagnostics".to_string()
            ]
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
        let mut episode = make_episode_item("coffee brewing techniques from ethiopia");
        episode.source_episode = "episode:other".into();
        let episodes = vec![episode];
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
    fn does_not_prefer_episode_when_it_would_drop_selected_fact_query_terms() {
        let query_terms = vec![
            "jon".to_string(),
            "gina".to_string(),
            "destress".to_string(),
        ];
        let mut fact = make_ranked(
            make_fact("Gina encourages Jon to take breaks and dance it out to destress."),
            RetrievalTier::Direct,
        );
        fact.evidence.reranking = Some(serde_json::json!({
            "model_signature": "reranker-v1",
            "relevance_logit": 2.2,
        }));
        let episodes = vec![AssembledContextItem {
            content: "Jon says dancing is his passion and escape; he wants to teach others.".into(),
            provenance: serde_json::json!({
                "reranking": {
                    "model_signature": "reranker-v1",
                    "relevance_logit": 8.7,
                }
            }),
            ..make_episode_item(
                "Jon says dancing is his passion and escape; he wants to teach others.",
            )
        }];

        assert!(!should_prefer_episode_content(
            &[fact],
            &episodes,
            &query_terms
        ));
    }

    #[test]
    fn same_model_episode_score_can_win_when_it_preserves_selected_fact_query_terms() {
        let query_terms = vec![
            "jon".to_string(),
            "dancing".to_string(),
            "passion".to_string(),
        ];
        let mut fact = make_ranked(
            make_fact("Jon's passion includes dancing every weekend with friends."),
            RetrievalTier::Direct,
        );
        fact.evidence.reranking = Some(serde_json::json!({
            "model_signature": "reranker-v1",
            "relevance_logit": 2.2,
        }));
        let mut episode = AssembledContextItem {
            content: "Jon describes dancing as a passion.".to_string(),
            provenance: serde_json::json!({
                "reranking": {
                    "model_signature": "reranker-v1",
                    "relevance_logit": 8.7,
                }
            }),
            ..make_episode_item("Jon describes dancing as a passion.")
        };
        episode.source_episode = "episode:other".into();

        assert!(should_prefer_episode_content(
            &[fact],
            &[episode],
            &query_terms
        ));
    }

    #[test]
    fn partial_episode_cannot_replace_collective_fact_evidence_even_with_higher_reranker_score() {
        let query_terms = vec!["jon".to_string(), "gina".to_string(), "status".to_string()];
        let mut jon_fact = make_ranked(
            make_fact("Jon owns the deployment status."),
            RetrievalTier::Direct,
        );
        jon_fact.evidence.reranking = Some(serde_json::json!({
            "model_signature": "reranker-v1",
            "relevance_logit": 8.0,
        }));
        let mut gina_fact = make_ranked(
            make_fact("Gina owns the billing status."),
            RetrievalTier::Direct,
        );
        gina_fact.evidence.reranking = Some(serde_json::json!({
            "model_signature": "reranker-v1",
            "relevance_logit": 7.0,
        }));
        let episode = AssembledContextItem {
            content: "Jon owns the deployment status.".to_string(),
            provenance: serde_json::json!({
                "reranking": {
                    "model_signature": "reranker-v1",
                    "relevance_logit": 9.0,
                }
            }),
            ..make_episode_item("Jon owns the deployment status.")
        };

        assert!(!should_prefer_episode_content(
            &[jon_fact, gina_fact],
            &[episode],
            &query_terms,
        ));
    }

    #[test]
    fn incomparable_reranker_scores_do_not_override_lexical_fallback() {
        let query_terms = vec!["coffee".to_string(), "brewing".to_string()];
        let mut fact = make_ranked(
            make_fact("coffee brewing guide for beginners"),
            RetrievalTier::Direct,
        );
        fact.evidence.reranking = Some(serde_json::json!({
            "model_signature": "reranker-v1",
            "relevance_logit": 2.2,
        }));
        let episodes = vec![AssembledContextItem {
            provenance: serde_json::json!({
                "reranking": {
                    "model_signature": "reranker-v2",
                    "relevance_logit": 8.7,
                }
            }),
            ..make_episode_item("coffee")
        }];

        assert!(!should_prefer_episode_content(
            &[fact],
            &episodes,
            &query_terms
        ));
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
            evidence: crate::memory::retrieval::types::CandidateEvidence::default(),
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
            crate::shared::search::search_query_terms("platform planning notes july 2025");
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
            crate::shared::search::search_query_terms("platform planning notes july 2025");
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
        let query_terms = crate::shared::search::search_query_terms("bob jones");
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
            crate::shared::search::search_query_terms("help kickoff naming localization alignment");
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
        let query_terms = crate::shared::search::search_query_terms(
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
