use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde_json::json;

use crate::logging::LogLevel;
use crate::models::{AccessPayload, AssembleContextRequest, AssembledContextItem};
use crate::service::decayed_confidence;
use crate::service::error::MemoryError;
use crate::service::log_event;
use crate::service::service_context::RetrievalContext;

use super::alias_expansion::expand_query_with_aliases;
use super::budget::{collect_episode_fallback_items, should_prefer_episode_content};
use super::community::{CollectCommunityFactsRequest, collect_community_facts};
use super::experience::{
    RecentExperienceRequest, collect_recent_experience_facts, expand_experience_query_terms,
};
use super::filtering::filter_facts_by_constraints;
use super::graph::{CollectGraphFactsRequest, collect_graph_facts};
use super::lexical::{FactQueryParams, select_fact_records_for_query};
use super::params::DefaultContextParams;
use super::query_mode;
use super::ranking::{
    BuildRankedContextFactsRequest, RetrievalTier, apply_time_window, build_ranked_context_facts,
    select_ranked_context_facts, sort_ranked_context_facts_for_timeline,
};
use super::rescue::{
    build_episode_rescue_log_result, maybe_append_first_person_episode_item,
    maybe_append_first_person_ranked_fact_item,
};
use super::scoring::{ranked_fact_to_item, selected_fact_matched_terms};
use super::semantic::{CollectSemanticFactsRequest, collect_semantic_facts};
use super::temporal::{CollectTemporalFactsRequest, collect_temporal_facts, infer_temporal_window};
use super::triple::collect_triple_facts;
use super::types::RankedContextFact;
use crate::service::cache::{CacheKey, CacheView};

// ─── Parameter preparation and cache operations ──────────────────────────

/// Prepared parameters for context assembly, extracted from the request
/// and access payload. Avoids re-parsing in the main orchestrator.
pub(super) struct PreparedContextParams {
    pub access: AccessPayload,
    pub namespace: String,
    pub cutoff: DateTime<Utc>,
    pub cutoff_iso: String,
    pub cleaned_query: String,
    pub fact_types: Vec<String>,
    pub query_terms: Vec<String>,
    pub resolved_view_mode_opt: Option<String>,
    pub query_flags: super::query_mode::QueryFlags,
    pub cache_key: CacheKey,
}

/// Parses the request and prepares all parameters needed for context assembly.
pub(super) async fn prepare_context_params(
    service: &RetrievalContext,
    request: &AssembleContextRequest,
    access_opt: Option<AccessPayload>,
) -> Result<PreparedContextParams, MemoryError> {
    let cutoff = request.as_of.unwrap_or_else(super::super::query::now);
    let access = access_opt.unwrap_or(AccessPayload {
        allowed_tags: None,
        caller_id: None,
        session_vars: None,
        transport: None,
        content_type: None,
    });

    let namespace = service.active_namespace.clone();
    let cutoff_iso = super::super::normalize_dt(cutoff);
    let cleaned_query = super::super::preprocess_search_query(&request.query);

    let requested_view_mode = request
        .view_mode
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let fact_types = request
        .fact_types
        .iter()
        .filter_map(|fact_type| {
            let trimmed = fact_type.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
        .collect::<Vec<_>>();
    let query_terms = if cleaned_query.is_empty() {
        Vec::new()
    } else {
        super::super::query::search_query_terms(&cleaned_query)
    };
    let raw_query_opt = if request.query.trim().is_empty() {
        None
    } else {
        Some(request.query.as_str())
    };
    let (resolved_view_mode, query_flags) = query_mode::resolve_view_mode(
        requested_view_mode.as_deref(),
        raw_query_opt,
        &query_terms,
        cutoff,
    );
    let resolved_view_mode_opt = resolved_view_mode.as_option_str().map(str::to_string);
    let cache_key = CacheKey::new(
        &request.query,
        cutoff,
        request.budget,
        &request.fact_types,
        CacheView::new(
            resolved_view_mode.as_option_str(),
            request.window_start,
            request.window_end,
        ),
        access.allowed_tags.clone(),
    );

    Ok(PreparedContextParams {
        access,
        namespace,
        cutoff,
        cutoff_iso,
        cleaned_query,
        fact_types,
        query_terms,
        resolved_view_mode_opt,
        query_flags,
        cache_key,
    })
}

/// Checks the context cache for a hit. Returns cached items if found.
pub(super) async fn check_cache(
    service: &RetrievalContext,
    cache_key: &CacheKey,
) -> Option<Vec<AssembledContextItem>> {
    let mut cache = service.context_cache.write().await;
    cache.get(cache_key).cloned()
}

/// Stores results in the context cache.
pub(super) async fn store_cache(
    service: &RetrievalContext,
    cache_key: CacheKey,
    results: &[AssembledContextItem],
) {
    let mut cache = service.context_cache.write().await;
    cache.put(cache_key, results.to_vec());
}

/// Logs the start of context assembly.
pub(super) fn log_context_start(
    service: &RetrievalContext,
    request: &AssembleContextRequest,
    access: Option<&AccessPayload>,
) {
    service.logger.log(
        log_event(
            "assemble_context.start",
            json!({"query": request.query, "budget": request.budget}),
            json!({}),
            access,
            None,
            None,
        ),
        LogLevel::Info,
    );
}

/// Executes the full multi-tier retrieval pipeline for the default (non-view-mode) path.
///
/// Tiers: lexical BM25 → temporal → alias expansion → experience → community → semantic ANN.
/// Falls back to episode search if no facts match.
/// The tier-fallback outcome for the default context pipeline.
///
/// When the episode-content rescue tier and the ranked-fact tiers compete for
/// the response, this records which side wins. `assemble_default_context`
/// returns episode items on [`FallbackDecision::UseEpisodes`] and otherwise
/// builds the response from the ranked facts (with episodes appended only when
/// they earn a slot via the first-person heuristics).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FallbackDecision {
    /// Episode content replaces the ranked-fact tiers.
    UseEpisodes,
    /// Ranked facts win; episode content may still be appended opportunistically.
    UseRanked,
}

/// Strategy object for the episode-rescue tier fallback.
///
/// Encapsulates the decision `assemble_default_context` makes when the
/// episode-content tier should replace the ranked-fact tiers: when episode
/// overlap out-scores every ranked candidate (see
/// [`super::budget::should_prefer_episode_content`]). The companion decision —
/// ranking produced nothing for a query — is guarded in the pipeline before
/// selection; both branches of the fallback are covered by this strategy's
/// unit tests.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct EpisodeFallbackStrategy;

impl EpisodeFallbackStrategy {
    /// Decides whether episode content should replace the ranked facts.
    pub(super) fn decide(
        &self,
        selected_ranked: &[RankedContextFact],
        episode_fallback_items: &[AssembledContextItem],
        query_terms: &[String],
    ) -> FallbackDecision {
        if should_prefer_episode_content(selected_ranked, episode_fallback_items, query_terms) {
            FallbackDecision::UseEpisodes
        } else {
            FallbackDecision::UseRanked
        }
    }
}

pub(super) async fn assemble_default_context(
    service: &RetrievalContext,
    params: DefaultContextParams<'_>,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let lexical_result = select_fact_records_for_query(
        service,
        FactQueryParams {
            cutoff_iso: params.cutoff_iso,
            query_opt: params.query_opt,
            limit: params.budget,
            fact_types: params.fact_types,
        },
    )
    .await?;

    let direct_retrieval_tier = lexical_result.retrieval_tier;
    let mut direct_facts =
        filter_facts_by_constraints(lexical_result.records, params.access, params.fact_types);

    let mut expanded_facts = Vec::new();
    let mut ranked_facts = if let Some(query) = params.query_opt {
        let temporal_facts = collect_temporal_facts(
            service,
            CollectTemporalFactsRequest {
                cutoff_iso: params.cutoff_iso,
                cutoff: params.cutoff,
                query,
                access: params.access,
                fact_types: params.fact_types,
                budget: params.budget,
            },
        )
        .await?;

        let expanded_queries = expand_query_with_aliases(service, query).await;
        let direct_fact_ids: HashSet<_> = direct_facts
            .iter()
            .chain(temporal_facts.iter())
            .map(|fact| fact.fact_id.clone())
            .collect();

        for expanded_query in &expanded_queries {
            if expanded_query == query {
                continue;
            }
            let extra_records = select_fact_records_for_query(
                service,
                FactQueryParams {
                    cutoff_iso: params.cutoff_iso,
                    query_opt: Some(expanded_query),
                    limit: params.budget,
                    fact_types: params.fact_types,
                },
            )
            .await?;
            for fact in
                filter_facts_by_constraints(extra_records.records, params.access, params.fact_types)
            {
                if !direct_fact_ids.contains(&fact.fact_id) {
                    expanded_facts.push(fact);
                }
            }
        }
        let base_direct_ids: HashSet<_> = direct_facts
            .iter()
            .chain(temporal_facts.iter())
            .chain(expanded_facts.iter())
            .map(|fact| fact.fact_id.clone())
            .collect();

        let experience_query_terms =
            expand_experience_query_terms(params.query_terms, &direct_facts);
        let experience_topic_terms = experience_query_terms
            .iter()
            .filter(|term| !params.query_terms.contains(term))
            .cloned()
            .collect::<Vec<_>>();
        let mut experience_facts = collect_recent_experience_facts(
            service,
            RecentExperienceRequest {
                cutoff: params.cutoff,
                access: params.access,
                budget: params.budget,
                fact_types: params.fact_types,
            },
            &experience_query_terms,
            &experience_topic_terms,
            &base_direct_ids,
        )
        .await?;

        if !experience_topic_terms.is_empty() {
            let topical_floor = direct_facts
                .first()
                .map(|fact| fact.ft_score)
                .unwrap_or(0.0);
            for fact in &mut experience_facts {
                fact.ft_score = fact.ft_score.max(topical_floor + 1.0);
            }
        }

        // Triple-expanded facts: facts linked via matching triples.
        let triple_facts =
            collect_triple_facts(service, params.cutoff_iso, query, params.budget).await?;
        for fact in triple_facts {
            if !base_direct_ids.contains(&fact.fact_id) {
                expanded_facts.push(fact);
            }
        }

        direct_facts.extend(experience_facts);
        direct_facts.sort_by(|left, right| {
            right
                .ft_score
                .total_cmp(&left.ft_score)
                .then_with(|| right.t_valid.cmp(&left.t_valid))
                .then_with(|| left.fact_id.cmp(&right.fact_id))
        });

        let direct_fact_ids: HashSet<_> = direct_facts
            .iter()
            .chain(temporal_facts.iter())
            .chain(expanded_facts.iter())
            .map(|fact| fact.fact_id.clone())
            .collect();

        let graph_facts = collect_graph_facts(
            service,
            CollectGraphFactsRequest {
                cutoff_iso: params.cutoff_iso,
                raw_query: query,
                access: params.access,
                fact_types: params.fact_types,
                direct_fact_ids: &direct_fact_ids,
                lexical_facts: &direct_facts,
                max_hops: params.query_flags.max_graph_hops(),
                budget: params.budget,
            },
        )
        .await?;

        let all_direct_ids: HashSet<_> = direct_facts
            .iter()
            .chain(temporal_facts.iter())
            .chain(expanded_facts.iter())
            .chain(graph_facts.iter().map(|candidate| &candidate.fact))
            .map(|fact| fact.fact_id.clone())
            .collect();

        let community_facts = collect_community_facts(
            service,
            CollectCommunityFactsRequest {
                cutoff_iso: params.cutoff_iso,
                query,
                access: params.access,
                fact_types: params.fact_types,
                direct_fact_ids: &all_direct_ids,
                budget: params.budget,
            },
        )
        .await?;

        let excluded_fact_ids = all_direct_ids
            .iter()
            .cloned()
            .chain(
                community_facts
                    .iter()
                    .map(|(fact, _, _)| fact.fact_id.clone()),
            )
            .collect::<HashSet<_>>();

        let semantic_facts = collect_semantic_facts(
            service,
            CollectSemanticFactsRequest {
                cutoff: params.cutoff,
                query,
                access: params.access,
                fact_types: params.fact_types,
                excluded_fact_ids: &excluded_fact_ids,
                budget: params.budget,
            },
        )
        .await?;

        let mut lexical_facts = direct_facts
            .into_iter()
            .map(|fact| (fact, direct_retrieval_tier))
            .collect::<Vec<_>>();
        lexical_facts.extend(
            temporal_facts
                .into_iter()
                .map(|fact| (fact, RetrievalTier::TemporalExpanded)),
        );
        lexical_facts.extend(
            expanded_facts
                .into_iter()
                .map(|fact| (fact, RetrievalTier::AliasExpanded)),
        );

        build_ranked_context_facts(
            BuildRankedContextFactsRequest {
                lexical_facts,
                graph_facts,
                community_facts,
                semantic_facts,
                query_opt: params.raw_query_opt,
                semantic_available: service.embedding_service.embedding_provider().is_enabled(),
                cutoff: params.cutoff,
            },
            decayed_confidence,
        )
    } else {
        build_ranked_context_facts(
            BuildRankedContextFactsRequest {
                lexical_facts: direct_facts
                    .into_iter()
                    .map(|fact| (fact, RetrievalTier::Direct))
                    .collect(),
                graph_facts: Vec::new(),
                community_facts: Vec::new(),
                semantic_facts: Vec::new(),
                query_opt: params.raw_query_opt,
                semantic_available: service.embedding_service.embedding_provider().is_enabled(),
                cutoff: params.cutoff,
            },
            decayed_confidence,
        )
    };

    let episode_fallback_items = if let Some(query) = params.query_opt {
        collect_episode_fallback_items(service, &params, query).await?
    } else {
        Vec::new()
    };

    if ranked_facts.is_empty() {
        if params.query_opt.is_some() {
            return Ok(episode_fallback_items);
        }

        return Ok(Vec::new());
    }

    apply_time_window(&mut ranked_facts, params.window_start, params.window_end);
    let ranked_candidates = ranked_facts.clone();
    let timeline_mode = params.resolved_view_mode == Some("timeline")
        || (params.resolved_view_mode.is_none() && params.query_flags.wants_timeline);
    let selected_ranked = if timeline_mode {
        sort_ranked_context_facts_for_timeline(&mut ranked_facts);
        ranked_facts
            .into_iter()
            .take(params.budget.max(1) as usize)
            .collect::<Vec<_>>()
    } else {
        let temporal_focus = params
            .query_opt
            .and_then(|query| infer_temporal_window(query, params.cutoff));
        select_ranked_context_facts(
            ranked_facts,
            params.budget.max(1) as usize,
            temporal_focus,
            params.query_terms.to_vec(),
        )
    };

    let strategy = EpisodeFallbackStrategy;
    let prefer_episode_content = strategy.decide(
        &selected_ranked,
        &episode_fallback_items,
        params.query_terms,
    ) == FallbackDecision::UseEpisodes;

    if params.query_opt.is_some() {
        use crate::logging::LogLevel;
        use serde_json::json;

        service.logger.log(
            log_event(
                "assemble_context.episode_rescue",
                json!({"namespace": params.namespace, "query": params.query_opt}),
                build_episode_rescue_log_result(
                    episode_fallback_items.len(),
                    selected_ranked.len(),
                    prefer_episode_content,
                ),
                Some(params.access),
                None,
                None,
            ),
            LogLevel::Debug,
        );
    }

    if prefer_episode_content {
        return Ok(episode_fallback_items);
    }

    let selected_terms = selected_fact_matched_terms(&selected_ranked, params.query_terms);
    let mut results = selected_ranked
        .into_iter()
        .map(|ranked| ranked_fact_to_item(ranked, params.cutoff, decayed_confidence))
        .collect::<Vec<_>>();

    maybe_append_first_person_episode_item(
        &mut results,
        &episode_fallback_items,
        &selected_terms,
        params.raw_query_opt,
        params.query_terms,
        params.budget.max(1) as usize,
    );

    maybe_append_first_person_ranked_fact_item(
        &mut results,
        &ranked_candidates,
        params.raw_query_opt,
        params.query_terms,
        params.budget.max(1) as usize,
        params.cutoff,
    );

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Fact;

    fn create_test_fact(
        fact_id: &str,
        content: &str,
        t_valid: chrono::DateTime<chrono::Utc>,
    ) -> Fact {
        Fact {
            fact_id: fact_id.to_string(),
            fact_type: "note".to_string(),
            content: content.to_string(),
            quote: content.to_string(),
            source_episode: "episode:test".to_string(),
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
        content: &str,
        tier: RetrievalTier,
        t_valid: chrono::DateTime<chrono::Utc>,
    ) -> RankedContextFact {
        RankedContextFact {
            fact: create_test_fact(fact_id, content, t_valid),
            rationale: "test rationale".to_string(),
            retrieval_tier: tier,
            fusion_score: 1.0,
            source_priority: 0,
            decayed_confidence: 1.0,
            query_alignment_factor: 1.0,
            grounding_score: 1.0,
            semantic_available: false,
            matched_query_terms: Vec::new(),
            graph_trace: None,
        }
    }

    fn episode_item(content: &str) -> AssembledContextItem {
        AssembledContextItem {
            fact_id: "episode_fallback:episode:july".to_string(),
            content: content.to_string(),
            quote: content.to_string(),
            source_episode: "episode:july".to_string(),
            confidence: 1.0,
            provenance: serde_json::json!({"episode_fallback": true}),
            rationale: "fallback".to_string(),
            retrieval_tier: Some("fallback".to_string()),
            ..Default::default()
        }
    }

    fn query_terms() -> Vec<String> {
        crate::service::query::search_query_terms("platform planning notes july 2025")
    }

    #[test]
    fn decide_uses_episodes_when_episode_overlap_is_stronger() {
        let fact_time = chrono::DateTime::parse_from_rfc3339("2025-07-13T10:00:00Z")
            .expect("fact timestamp")
            .with_timezone(&chrono::Utc);
        // The ranked fact has no lexical overlap with the query terms.
        let selected = vec![create_ranked_test_fact(
            "fact:noise",
            "Acme Corp quarterly renewal workflow.",
            RetrievalTier::Direct,
            fact_time,
        )];
        let episodes = vec![episode_item(
            "Platform planning notes July 2025: release scope, integrations, and response workflow updates.",
        )];

        let decision = EpisodeFallbackStrategy.decide(&selected, &episodes, &query_terms());
        assert_eq!(decision, FallbackDecision::UseEpisodes);
    }

    #[test]
    fn decide_uses_ranked_when_fact_overlap_is_equal_or_better() {
        let fact_time = chrono::DateTime::parse_from_rfc3339("2025-07-13T10:00:00Z")
            .expect("fact timestamp")
            .with_timezone(&chrono::Utc);
        // The ranked fact directly matches the query terms.
        let selected = vec![create_ranked_test_fact(
            "fact:strong",
            "Platform planning notes July 2025 for release scope and integrations.",
            RetrievalTier::Direct,
            fact_time,
        )];
        let episodes = vec![episode_item(
            "Platform notes July 2025 with rollout reminders.",
        )];

        let decision = EpisodeFallbackStrategy.decide(&selected, &episodes, &query_terms());
        assert_eq!(decision, FallbackDecision::UseRanked);
    }

    #[test]
    fn decide_uses_ranked_when_no_episode_items_exist() {
        let fact_time = chrono::DateTime::parse_from_rfc3339("2025-07-13T10:00:00Z")
            .expect("fact timestamp")
            .with_timezone(&chrono::Utc);
        let selected = vec![create_ranked_test_fact(
            "fact:weak",
            "Acme Corp quarterly renewal workflow.",
            RetrievalTier::Direct,
            fact_time,
        )];

        let decision = EpisodeFallbackStrategy.decide(&selected, &[], &query_terms());
        assert_eq!(decision, FallbackDecision::UseRanked);
    }

    #[test]
    fn decide_uses_ranked_when_graph_tier_is_present() {
        let fact_time = chrono::DateTime::parse_from_rfc3339("2025-07-13T10:00:00Z")
            .expect("fact timestamp")
            .with_timezone(&chrono::Utc);
        // Graph-expanded facts block the episode rescue regardless of overlap.
        let selected = vec![create_ranked_test_fact(
            "fact:graph",
            "Acme Corp quarterly renewal workflow.",
            RetrievalTier::GraphExpanded,
            fact_time,
        )];
        let episodes = vec![episode_item(
            "Platform planning notes July 2025: release scope, integrations, and response workflow updates.",
        )];

        let decision = EpisodeFallbackStrategy.decide(&selected, &episodes, &query_terms());
        assert_eq!(decision, FallbackDecision::UseRanked);
    }
}
