use std::collections::HashSet;

use crate::models::AssembledContextItem;
use crate::service::MemoryService;
use crate::service::decayed_confidence;
use crate::service::error::MemoryError;
use crate::service::log_event;

use super::alias_expansion::expand_query_with_aliases;
use super::budget::{collect_episode_fallback_items, should_prefer_episode_content};
use super::community::{CollectCommunityFactsRequest, collect_community_facts};
use super::experience::{
    RecentExperienceRequest, collect_recent_experience_facts, expand_experience_query_terms,
};
use super::filtering::filter_facts_by_constraints;
use super::lexical::{FactQueryParams, select_fact_records_for_query};
use super::params::DefaultContextParams;
use super::ranking::{
    RetrievalTier, apply_time_window, build_ranked_context_facts, select_ranked_context_facts,
    sort_ranked_context_facts_for_timeline,
};
use super::rescue::{
    build_episode_rescue_log_result, maybe_append_first_person_episode_item,
    maybe_append_first_person_ranked_fact_item,
};
use super::scoring::{ranked_fact_to_item, selected_fact_matched_terms};
use super::semantic::{CollectSemanticFactsRequest, collect_semantic_facts};
use super::temporal::{CollectTemporalFactsRequest, collect_temporal_facts, infer_temporal_window};

/// Executes the full multi-tier retrieval pipeline for the default (non-view-mode) path.
///
/// Tiers: lexical BM25 → temporal → alias expansion → experience → community → semantic ANN.
/// Falls back to episode search if no facts match.
pub(super) async fn assemble_default_context(
    service: &MemoryService,
    params: DefaultContextParams<'_>,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let lexical_result = select_fact_records_for_query(
        service,
        FactQueryParams {
            namespace: params.namespace,
            scope: params.scope,
            cutoff_iso: params.cutoff_iso,
            query_opt: params.query_opt,
            limit: params.budget,
            project: params.project_opt,
            fact_types: params.fact_types,
        },
    )
    .await?;

    let direct_retrieval_tier = lexical_result.retrieval_tier;
    let mut direct_facts = filter_facts_by_constraints(
        lexical_result.records,
        params.access,
        params.project_opt,
        params.fact_types,
    );

    let mut expanded_facts = Vec::new();
    let mut ranked_facts = if let Some(query) = params.query_opt {
        let temporal_facts = collect_temporal_facts(
            service,
            CollectTemporalFactsRequest {
                namespace: params.namespace,
                scope: params.scope,
                cutoff_iso: params.cutoff_iso,
                cutoff: params.cutoff,
                query,
                access: params.access,
                project: params.project_opt,
                fact_types: params.fact_types,
                budget: params.budget,
            },
        )
        .await?;

        let expanded_queries = expand_query_with_aliases(service, query, params.namespace).await;
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
                    namespace: params.namespace,
                    scope: params.scope,
                    cutoff_iso: params.cutoff_iso,
                    query_opt: Some(expanded_query),
                    limit: params.budget,
                    project: params.project_opt,
                    fact_types: params.fact_types,
                },
            )
            .await?;
            for fact in filter_facts_by_constraints(
                extra_records.records,
                params.access,
                params.project_opt,
                params.fact_types,
            ) {
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
                namespace: params.namespace,
                scope: params.scope,
                cutoff: params.cutoff,
                project: params.project_opt,
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

        direct_facts.extend(experience_facts);
        direct_facts.sort_by(|left, right| {
            right
                .ft_score
                .total_cmp(&left.ft_score)
                .then_with(|| right.t_valid.cmp(&left.t_valid))
                .then_with(|| left.fact_id.cmp(&right.fact_id))
        });

        let all_direct_ids: HashSet<_> = direct_facts
            .iter()
            .chain(temporal_facts.iter())
            .chain(expanded_facts.iter())
            .map(|fact| fact.fact_id.clone())
            .collect();

        let community_facts = collect_community_facts(
            service,
            CollectCommunityFactsRequest {
                namespace: params.namespace,
                scope: params.scope,
                cutoff_iso: params.cutoff_iso,
                query,
                access: params.access,
                project: params.project_opt,
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
                namespace: params.namespace,
                scope: params.scope,
                cutoff: params.cutoff,
                query,
                access: params.access,
                project: params.project_opt,
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
            lexical_facts,
            community_facts,
            semantic_facts,
            params.raw_query_opt,
            service.embedding_provider.is_enabled(),
            params.scope,
            params.cutoff,
            decayed_confidence,
        )
    } else {
        build_ranked_context_facts(
            direct_facts
                .into_iter()
                .map(|fact| (fact, RetrievalTier::Direct))
                .collect(),
            Vec::new(),
            Vec::new(),
            params.raw_query_opt,
            service.embedding_provider.is_enabled(),
            params.scope,
            params.cutoff,
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

        unreachable!("ranked_facts is empty but no query provided")
    }

    apply_time_window(&mut ranked_facts, params.window_start, params.window_end);
    let ranked_candidates = ranked_facts.clone();
    let selected_ranked = if params.view_mode == Some("timeline") {
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

    let prefer_episode_content = should_prefer_episode_content(
        &selected_ranked,
        &episode_fallback_items,
        params.query_terms,
    );

    if params.query_opt.is_some() {
        use crate::logging::LogLevel;
        use serde_json::json;

        service.logger.log(
            log_event(
                "assemble_context.episode_rescue",
                json!({"scope": params.scope, "query": params.query_opt}),
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
