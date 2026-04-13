//! Context assembly operations.

use std::collections::HashSet;
use std::time::Instant;

use serde_json::json;

use super::cache::{CacheKey, CacheView};
use super::embedding::embedding_from_value;
use super::error::{MemoryError, error_messages};
use crate::logging::LogLevel;
use crate::models::{AccessContext, AssembleContextRequest, AssembledContextItem};

mod alias_expansion;
mod community;
mod experience;
mod filtering;
mod lexical;
mod logging;
mod ranking;
mod semantic;
mod temporal;
mod views;

use alias_expansion::expand_query_with_aliases;
use community::{CollectCommunityFactsRequest, collect_community_facts};
use experience::{
    RecentExperienceRequest, append_recent_experience_items, collect_recent_experience_facts,
    expand_experience_query_terms,
};
use filtering::filter_facts_by_constraints;
use lexical::{FactQueryParams, select_episode_records_for_query, select_fact_records_for_query};
use logging::{summarize_retrieval_tiers, supplemental_experience_count};
use ranking::{
    RetrievalTier, apply_time_window, build_ranked_context_facts, select_ranked_context_facts,
    sort_ranked_context_facts_for_timeline,
};
use semantic::{CollectSemanticFactsRequest, collect_semantic_facts};
use temporal::{CollectTemporalFactsRequest, collect_temporal_facts, infer_temporal_window};
use views::{
    EpisodeFallbackParams, build_episode_fallback_items, build_facets_view, build_map_view,
    build_wake_up_view,
};

/// Converts a ranked fact into an assembled context item with decayed confidence.
fn ranked_fact_to_item(
    ranked: ranking::RankedContextFact,
    cutoff: chrono::DateTime<chrono::Utc>,
) -> AssembledContextItem {
    let confidence = super::decayed_confidence(&ranked.fact, cutoff);
    AssembledContextItem {
        fact_id: ranked.fact.fact_id,
        content: ranked.fact.content,
        quote: ranked.fact.quote,
        source_episode: ranked.fact.source_episode,
        confidence,
        provenance: ranked.fact.provenance,
        rationale: ranked.rationale,
        retrieval_tier: Some(ranked.retrieval_tier.as_str().to_string()),
    }
}

/// Records fact access for each item, logging errors without failing the operation.
async fn track_fact_accesses(
    service: &crate::service::MemoryService,
    items: &[AssembledContextItem],
    access: &AccessContext,
) {
    for item in items {
        if let Err(err) = service.record_fact_access(&item.fact_id, 1).await {
            service.logger.log(
                super::log_event(
                    "assemble_context.access_track_error",
                    json!({"fact_id": item.fact_id}),
                    json!({"error": err.to_string()}),
                    Some(access),
                    None,
                    None,
                ),
                LogLevel::Warn,
            );
        }
    }
}

async fn collect_episode_fallback_items(
    service: &crate::service::MemoryService,
    params: &DefaultContextParams<'_>,
    query: &str,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let episode_records = select_episode_records_for_query(
        service,
        params.namespace,
        params.scope,
        params.cutoff_iso,
        Some(query),
        params.budget,
        params.project_opt,
    )
    .await?;

    Ok(build_episode_fallback_items(EpisodeFallbackParams {
        episodes: filtering::filter_episodes_by_constraints(
            episode_records,
            params.access,
            params.project_opt,
        ),
        query_opt: Some(query),
        scope: params.scope,
        cutoff: params.cutoff,
        window_start: params.window_start,
        window_end: params.window_end,
        timeline_mode: params.view_mode == Some("timeline"),
        budget: params.budget,
        fallback_rationale_fn: ranking::default_episode_fallback_rationale,
    }))
}

fn should_prefer_episode_content(
    selected_facts: &[ranking::RankedContextFact],
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
        .map(|fact| lexical::lexical_query_overlap_for_fact(&fact.fact, query_terms))
        .max()
        .unwrap_or(0);

    let best_episode_overlap = episode_items
        .iter()
        .map(|item| lexical::lexical_query_overlap_for_text(&item.content, query_terms))
        .max()
        .unwrap_or(0);

    best_episode_overlap > best_fact_overlap
}

fn build_episode_rescue_log_result(
    episode_candidate_count: usize,
    selected_fact_count: usize,
    episode_rescue_used: bool,
) -> serde_json::Value {
    json!({
        "episode_candidate_count": episode_candidate_count,
        "selected_fact_count": selected_fact_count,
        "episode_rescue_used": episode_rescue_used,
    })
}

/// Parameters for the default context assembly pipeline.
struct DefaultContextParams<'a> {
    namespace: &'a str,
    scope: &'a str,
    cutoff_iso: &'a str,
    cutoff: chrono::DateTime<chrono::Utc>,
    query_opt: Option<&'a str>,
    query_terms: &'a [String],
    project_opt: Option<&'a str>,
    fact_types: &'a [String],
    budget: i32,
    window_start: Option<chrono::DateTime<chrono::Utc>>,
    window_end: Option<chrono::DateTime<chrono::Utc>>,
    view_mode: Option<&'a str>,
    access: &'a AccessContext,
}

/// Executes the full multi-tier retrieval pipeline for the default (non-view-mode) path.
///
/// Tiers: lexical BM25 → temporal → alias expansion → experience → community → semantic ANN.
/// Falls back to episode search if no facts match.
async fn assemble_default_context(
    service: &crate::service::MemoryService,
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
            params.query_opt,
            params.scope,
            params.cutoff,
            super::decayed_confidence,
        )
    } else {
        build_ranked_context_facts(
            direct_facts
                .into_iter()
                .map(|fact| (fact, RetrievalTier::Direct))
                .collect(),
            Vec::new(),
            Vec::new(),
            params.query_opt,
            params.scope,
            params.cutoff,
            super::decayed_confidence,
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
        service.logger.log(
            super::log_event(
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

    Ok(selected_ranked
        .into_iter()
        .map(|ranked| ranked_fact_to_item(ranked, params.cutoff))
        .collect())
}

/// Assemble context for a query.
pub async fn assemble_context(
    service: &crate::service::MemoryService,
    request: AssembleContextRequest,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let started_at = Instant::now();
    let access = AccessContext::from_payload(request.access.clone());

    service.logger.log(
        super::log_event(
            "assemble_context.start",
            json!({"scope": request.scope, "query": request.query, "budget": request.budget}),
            json!({}),
            access.as_ref(),
            None,
            None,
        ),
        LogLevel::Info,
    );

    service.enforce_rate_limit(access.as_ref())?;

    if request.scope.trim().is_empty() {
        return Err(MemoryError::Validation(
            error_messages::SCOPE_REQUIRED.into(),
        ));
    }

    let cutoff = request.as_of.unwrap_or_else(super::query::now);
    let access = access.unwrap_or_else(|| AccessContext {
        allowed_scopes: Some(vec![request.scope.clone()]),
        allowed_tags: None,
        caller_id: None,
        session_vars: None,
        transport: None,
        content_type: None,
        cross_scope_allow: None,
    });

    if !service.is_scope_allowed(&request.scope, &access) {
        return Ok(vec![]);
    }

    let cache_key = CacheKey::new(
        &request.query,
        &request.scope,
        cutoff,
        request.budget,
        request.project.as_deref(),
        &request.fact_types,
        CacheView::new(
            request.view_mode.as_deref(),
            request.window_start,
            request.window_end,
        ),
        access.allowed_tags.clone(),
    );

    let cached = {
        let mut cache = service.context_cache.write().await;
        cache.get(&cache_key).cloned()
    };

    if let Some(cached) = cached {
        track_fact_accesses(service, &cached, &access).await;

        service.logger.log(
            super::log_event(
                "assemble_context.cache_hit",
                json!({"scope": request.scope, "query": request.query}),
                json!({"count": cached.len()}),
                Some(&access),
                None,
                None,
            ),
            LogLevel::Info,
        );

        let latency_ms = started_at.elapsed().as_secs_f64() * 1000.0;
        logging::maybe_record_query_log(service, &request, &cached, true, latency_ms, &access)
            .await;
        return Ok(cached);
    }

    service.logger.log(
        super::log_event(
            "assemble_context.cache_miss",
            json!({"scope": request.scope, "query": request.query, "budget": request.budget}),
            json!({"status": "computing"}),
            Some(&access),
            None,
            None,
        ),
        LogLevel::Trace,
    );

    let namespace = service.namespace_for_scope(&request.scope);
    let cutoff_iso = super::normalize_dt(cutoff);
    let cleaned_query = super::preprocess_search_query(&request.query);
    let query_opt = if cleaned_query.is_empty() {
        None
    } else {
        Some(cleaned_query.as_str())
    };
    let project_opt = request
        .project
        .as_deref()
        .filter(|project| !project.trim().is_empty());
    let requested_view_mode = request
        .view_mode
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let fact_types = request
        .fact_types
        .iter()
        .filter_map(|fact_type| {
            let trimmed = fact_type.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
        .collect::<Vec<_>>();
    let query_terms = query_opt
        .map(super::query::search_query_terms)
        .unwrap_or_default();

    service.logger.log(
        super::log_event(
            "assemble_context.features",
            json!({
                "scope": request.scope,
                "query": request.query,
                "budget": request.budget,
                "project": project_opt,
                "view_mode": requested_view_mode,
                "fact_type_count": fact_types.len(),
                "window_start": request.window_start.map(super::normalize_dt),
                "window_end": request.window_end.map(super::normalize_dt),
                "query_logging_enabled": service.is_query_logging_enabled(),
            }),
            json!({}),
            Some(&access),
            None,
            None,
        ),
        LogLevel::Debug,
    );

    if let Some(view_mode) = requested_view_mode
        && !matches!(view_mode, "facets" | "wake_up" | "map" | "timeline")
    {
        service.logger.log(
            super::log_event(
                "assemble_context.view_mode_unknown",
                json!({"scope": request.scope, "query": request.query, "view_mode": view_mode}),
                json!({"fallback": "default_ranked_retrieval"}),
                Some(&access),
                None,
                None,
            ),
            LogLevel::Warn,
        );
    }

    let mut results: Vec<AssembledContextItem> = if requested_view_mode == Some("facets") {
        build_facets_view(
            service,
            &namespace,
            &request.scope,
            cutoff,
            project_opt,
            request.budget,
            &access,
        )
        .await?
    } else if requested_view_mode == Some("wake_up") {
        build_wake_up_view(
            service,
            views::FactFilterParams {
                namespace: &namespace,
                scope: &request.scope,
                cutoff,
                project: project_opt,
                fact_types: &fact_types,
                access: &access,
            },
            request.budget,
            super::decayed_confidence,
            super::normalize_dt,
        )
        .await?
    } else if requested_view_mode == Some("map") {
        build_map_view(
            service,
            &namespace,
            cutoff,
            request.budget,
            super::normalize_dt,
        )
        .await?
    } else {
        assemble_default_context(
            service,
            DefaultContextParams {
                namespace: &namespace,
                scope: &request.scope,
                cutoff_iso: &cutoff_iso,
                cutoff,
                query_opt,
                query_terms: &query_terms,
                project_opt,
                fact_types: &fact_types,
                budget: request.budget,
                window_start: request.window_start,
                window_end: request.window_end,
                view_mode: requested_view_mode,
                access: &access,
            },
        )
        .await?
    };

    // Append recent experience facts as supplemental context only for browse-like
    // calls without a search query. Query-driven retrieval should not mix in
    // unrelated recent memories purely because they are fresh.
    if requested_view_mode != Some("facets")
        && requested_view_mode != Some("wake_up")
        && requested_view_mode != Some("map")
        && query_opt.is_none()
    {
        let appended_experience = append_recent_experience_items(
            &mut results,
            service,
            RecentExperienceRequest {
                namespace: &namespace,
                scope: &request.scope,
                cutoff,
                project: project_opt,
                access: &access,
                budget: request.budget,
                fact_types: &fact_types,
            },
        )
        .await?;

        if appended_experience > 0 {
            service.logger.log(
                super::log_event(
                    "assemble_context.experience_appended",
                    json!({"scope": request.scope, "query": request.query}),
                    json!({"count": appended_experience}),
                    Some(&access),
                    None,
                    None,
                ),
                LogLevel::Trace,
            );
        }
    }

    service.logger.log(
        super::log_event(
            "assemble_context.results",
            json!({
                "scope": request.scope,
                "query": request.query,
                "view_mode": requested_view_mode,
                "project": project_opt,
            }),
            json!({
                "count": results.len(),
                "retrieval_tiers": summarize_retrieval_tiers(&results),
                "supplemental_experience": supplemental_experience_count(&results),
            }),
            Some(&access),
            None,
            None,
        ),
        LogLevel::Trace,
    );

    track_fact_accesses(service, &results, &access).await;

    {
        let mut cache = service.context_cache.write().await;
        cache.put(cache_key, results.clone());
    }

    service.logger.log(
        super::log_event(
            "assemble_context.cache_set",
            json!({"scope": request.scope, "query": request.query, "budget": request.budget}),
            json!({"count": results.len()}),
            Some(&access),
            None,
            None,
        ),
        LogLevel::Trace,
    );

    let latency_ms = started_at.elapsed().as_secs_f64() * 1000.0;
    logging::maybe_record_query_log(service, &request, &results, false, latency_ms, &access).await;

    Ok(results)
}

#[cfg(test)]
#[allow(unused_imports, dead_code)]
mod tests {
    use super::alias_expansion::expand_query_with_aliases_for_test;
    use super::community::stored_community_summary_from_value;
    use super::filtering::{compare_facts_by_recency, filter_facts_by_policy};
    use super::lexical::{lexical_candidate_limit, rank_lexical_records};
    use super::ranking::{
        RankedContextFact, prune_redundant_selected_facts, ranked_relevance_score,
        sort_ranked_context_facts,
    };
    use super::temporal::expand_temporal_synonyms;
    use super::*;
    use crate::config::DEFAULT_EMBEDDING_DIMENSION;
    use crate::service::EmbeddingProvider;
    use crate::storage::{DbClient, GraphDirection};
    use async_trait::async_trait;
    use chrono::Utc;
    use serde_json::{Value, json};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn sort_facts_by_recency(facts: &mut [crate::models::Fact]) {
        use super::filtering::compare_facts_by_recency;
        facts.sort_by(compare_facts_by_recency);
    }

    fn create_test_fact(fact_id: &str, t_valid: chrono::DateTime<Utc>) -> crate::models::Fact {
        crate::models::Fact {
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
            provenance: json!({}),
            ft_score: 0.0,
        }
    }

    fn create_ranked_test_fact(
        fact_id: &str,
        source_episode: &str,
        t_valid: chrono::DateTime<Utc>,
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
        }
    }

    fn fixed_temporal_cutoff() -> chrono::DateTime<Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-04-08T12:00:00Z")
            .expect("cutoff")
            .with_timezone(&Utc)
    }

    #[test]
    fn infer_temporal_window_reuses_shared_year_for_adjacent_months() {
        let window = infer_temporal_window(
            "march april 2026 alpha suite decisions",
            fixed_temporal_cutoff(),
        )
        .expect("temporal window");

        assert_eq!(
            window.start.date_naive(),
            chrono::NaiveDate::from_ymd_opt(2026, 3, 1).expect("march start")
        );
        assert_eq!(
            window.end.date_naive(),
            chrono::NaiveDate::from_ymd_opt(2026, 4, 30).expect("april end")
        );
    }

    #[test]
    fn sort_facts_by_recency_orders_by_date_desc() {
        let t1 = Utc::now();
        let t2 = t1 - chrono::Duration::hours(1);
        let t3 = t1 - chrono::Duration::hours(2);

        let mut facts = vec![
            create_test_fact("fact:3", t3),
            create_test_fact("fact:1", t1),
            create_test_fact("fact:2", t2),
        ];

        sort_facts_by_recency(&mut facts);

        assert_eq!(facts[0].fact_id, "fact:1");
        assert_eq!(facts[1].fact_id, "fact:2");
        assert_eq!(facts[2].fact_id, "fact:3");
    }

    #[test]
    fn sort_facts_by_recency_breaks_ties_with_id() {
        let t = Utc::now();

        let mut facts = vec![
            create_test_fact("fact:b", t),
            create_test_fact("fact:a", t),
            create_test_fact("fact:c", t),
        ];

        sort_facts_by_recency(&mut facts);

        assert_eq!(facts[0].fact_id, "fact:a");
        assert_eq!(facts[1].fact_id, "fact:b");
        assert_eq!(facts[2].fact_id, "fact:c");
    }

    #[test]
    fn rank_lexical_records_promotes_more_specific_query_overlap() {
        let query_terms = vec![
            "caroline".to_string(),
            "lgbtq".to_string(),
            "support".to_string(),
            "group".to_string(),
        ];

        let ranked = rank_lexical_records(
            vec![
                json!({
                    "fact_id": "fact:generic",
                    "content": "Caroline passed the adoption agency interviews last Friday.",
                    "t_valid": "2026-01-10T10:30:00Z",
                    "ft_score": 20.0
                }),
                json!({
                    "fact_id": "fact:support",
                    "content": "Caroline attended the LGBTQ support group recently.",
                    "t_valid": "2026-01-09T10:30:00Z",
                    "ft_score": 5.0
                }),
            ],
            &query_terms,
        );

        let fact_ids = ranked
            .iter()
            .filter_map(|record| record.get("fact_id").and_then(Value::as_str))
            .collect::<Vec<_>>();

        assert_eq!(fact_ids, vec!["fact:support", "fact:generic"]);
    }

    #[test]
    fn rank_lexical_records_prefers_sentence_cohesion_over_cross_sentence_term_soup() {
        let query_terms = crate::service::query::search_query_terms(
            "I recently attended an event where there was a unique blend of modern beats with Pacific sounds.",
        );

        let ranked = rank_lexical_records(
            vec![
                json!({
                    "fact_id": "fact:term-soup",
                    "content": "I recently updated my studio notes after an event planning session. The next experiment used modern beats in a new mix. A Pacific sound library added a unique texture to the blend.",
                    "t_valid": "2026-01-10T10:30:00Z",
                    "ft_score": 18.0
                }),
                json!({
                    "fact_id": "fact:exact-sentence",
                    "content": "I was so thrilled to see that fusion in action! The blend of traditional Pacific sounds with modern beats created a captivating experience that resonated deeply with the audience.",
                    "t_valid": "2026-01-09T10:30:00Z",
                    "ft_score": 8.0
                }),
            ],
            &query_terms,
        );

        let fact_ids = ranked
            .iter()
            .filter_map(|record| record.get("fact_id").and_then(Value::as_str))
            .collect::<Vec<_>>();

        assert_eq!(
            fact_ids,
            vec!["fact:exact-sentence", "fact:term-soup"],
            "exact sentence matches should outrank cross-sentence term soup even when the soup has a stronger raw ft_score"
        );
    }

    #[test]
    fn select_ranked_context_facts_filters_out_of_window_candidates_without_temporal_support() {
        let temporal_focus =
            infer_temporal_window("july 2025", fixed_temporal_cutoff()).expect("temporal focus");
        let query_terms =
            crate::service::query::search_query_terms("platform planning notes july 2025");

        let july_candidate_time = chrono::DateTime::parse_from_rfc3339("2025-07-10T10:00:00Z")
            .expect("july candidate timestamp")
            .with_timezone(&Utc);
        let october_candidate_time = chrono::DateTime::parse_from_rfc3339("2025-10-13T10:00:00Z")
            .expect("october candidate timestamp")
            .with_timezone(&Utc);

        let july_candidate = RankedContextFact {
            fact: crate::models::Fact {
                content: "Platform planning notes were finalized in July 2025.".to_string(),
                ..create_ranked_test_fact(
                    "fact:july",
                    "episode:july",
                    july_candidate_time,
                    2.0,
                    6.0,
                    0,
                    &[],
                )
                .fact
            },
            retrieval_tier: RetrievalTier::Direct,
            ..create_ranked_test_fact(
                "fact:july",
                "episode:july",
                july_candidate_time,
                2.0,
                6.0,
                0,
                &[],
            )
        };

        let october_semantic_candidate = RankedContextFact {
            fact: crate::models::Fact {
                content: "October 2025 summary: Platform 2.3 patch release updates.".to_string(),
                ..create_ranked_test_fact(
                    "fact:october",
                    "episode:october",
                    october_candidate_time,
                    1.8,
                    5.0,
                    0,
                    &[],
                )
                .fact
            },
            retrieval_tier: RetrievalTier::SemanticExpanded,
            ..create_ranked_test_fact(
                "fact:october",
                "episode:october",
                october_candidate_time,
                1.8,
                5.0,
                0,
                &[],
            )
        };

        let selected = select_ranked_context_facts(
            vec![october_semantic_candidate, july_candidate],
            5,
            Some(temporal_focus),
            query_terms,
        );

        let fact_ids = selected
            .iter()
            .map(|fact| fact.fact.fact_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(fact_ids, vec!["fact:july"]);
    }

    #[test]
    fn should_prefer_episode_content_when_episode_overlap_is_stronger() {
        let query_terms =
            crate::service::query::search_query_terms("platform planning notes july 2025");
        let fact_time = chrono::DateTime::parse_from_rfc3339("2025-07-13T10:00:00Z")
            .expect("fact timestamp")
            .with_timezone(&Utc);

        let selected_facts = vec![RankedContextFact {
            fact: crate::models::Fact {
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
            provenance: json!({"episode_fallback": true}),
            rationale: "fallback".to_string(),
            retrieval_tier: Some("fallback".to_string()),
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
            .with_timezone(&Utc);

        let selected_facts = vec![RankedContextFact {
            fact: crate::models::Fact {
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
            provenance: json!({"episode_fallback": true}),
            rationale: "fallback".to_string(),
            retrieval_tier: Some("fallback".to_string()),
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
            .with_timezone(&Utc);

        let selected_facts = vec![RankedContextFact {
            fact: crate::models::Fact {
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
            provenance: json!({"episode_fallback": true}),
            rationale: "fallback".to_string(),
            retrieval_tier: Some("fallback".to_string()),
        }];

        assert!(!should_prefer_episode_content(
            &selected_facts,
            &episode_items,
            &query_terms,
        ));
    }

    #[test]
    fn build_episode_rescue_log_result_reports_candidate_and_decision_counts() {
        let result = build_episode_rescue_log_result(3, 2, true);

        assert_eq!(
            result
                .get("episode_candidate_count")
                .and_then(Value::as_u64),
            Some(3)
        );
        assert_eq!(
            result.get("selected_fact_count").and_then(Value::as_u64),
            Some(2)
        );
        assert_eq!(
            result.get("episode_rescue_used").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[tokio::test]
    async fn select_fact_records_for_query_deduplicates_term_fallback_records() {
        struct DedupFallbackDbClient;

        #[async_trait::async_trait]
        impl DbClient for DedupFallbackDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(match query_contains {
                    Some("atlas launch checklist") => vec![],
                    Some("atlas") => vec![
                        json!({
                            "fact_id": "fact:shared",
                            "fact_type": "note",
                            "content": "Atlas launch is scheduled.",
                            "quote": "Atlas launch is scheduled.",
                            "source_episode": "episode:1",
                            "t_valid": "2026-01-10T10:30:00Z",
                            "t_ingested": "2026-01-10T10:30:00Z",
                            "scope": "org"
                        }),
                        json!({
                            "fact_id": "fact:atlas-only",
                            "fact_type": "note",
                            "content": "Atlas has a risk review.",
                            "quote": "Atlas has a risk review.",
                            "source_episode": "episode:2",
                            "t_valid": "2026-01-09T10:30:00Z",
                            "t_ingested": "2026-01-09T10:30:00Z",
                            "scope": "org"
                        }),
                    ],
                    Some("launch") => vec![
                        json!({
                            "fact_id": "fact:shared",
                            "fact_type": "note",
                            "content": "Atlas launch is scheduled.",
                            "quote": "Atlas launch is scheduled.",
                            "source_episode": "episode:1",
                            "t_valid": "2026-01-10T10:30:00Z",
                            "t_ingested": "2026-01-10T10:30:00Z",
                            "scope": "org"
                        }),
                        json!({
                            "fact_id": "fact:launch-only",
                            "fact_type": "note",
                            "content": "Launch checklist is ready.",
                            "quote": "Launch checklist is ready.",
                            "source_episode": "episode:3",
                            "t_valid": "2026-01-08T10:30:00Z",
                            "t_ingested": "2026-01-08T10:30:00Z",
                            "scope": "org"
                        }),
                    ],
                    Some("checklist") => vec![json!({
                        "fact_id": "fact:launch-only",
                        "fact_type": "note",
                        "content": "Launch checklist is ready.",
                        "quote": "Launch checklist is ready.",
                        "source_episode": "episode:3",
                        "t_valid": "2026-01-08T10:30:00Z",
                        "t_ingested": "2026-01-08T10:30:00Z",
                        "scope": "org"
                    })],
                    _ => vec![],
                })
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }
            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let service = crate::service::MemoryService::new(
            Arc::new(DedupFallbackDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let lexical_result = select_fact_records_for_query(
            &service,
            FactQueryParams {
                namespace: "org",
                scope: "org",
                cutoff_iso: "2026-01-15T10:30:00Z",
                query_opt: Some("atlas launch checklist"),
                limit: 10,
                project: None,
                fact_types: &[],
            },
        )
        .await
        .expect("fallback records");

        let fact_ids = lexical_result
            .records
            .iter()
            .filter_map(|record| record.get("fact_id").and_then(Value::as_str))
            .collect::<Vec<_>>();

        assert_eq!(
            lexical_result.retrieval_tier,
            RetrievalTier::EpisodeFallback
        );
        assert_eq!(
            fact_ids,
            vec!["fact:shared", "fact:launch-only", "fact:atlas-only"]
        );
    }

    #[tokio::test]
    async fn select_fact_records_for_query_prefers_term_fallback_with_better_overlap() {
        struct FallbackPreferenceDbClient;

        #[async_trait]
        impl DbClient for FallbackPreferenceDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(match query_contains {
                    Some("caroline lgbtq support group") => vec![
                        json!({
                            "fact_id": "fact:generic-1",
                            "fact_type": "note",
                            "content": "Caroline passed the adoption agency interviews last Friday.",
                            "quote": "Caroline passed the adoption agency interviews last Friday.",
                            "source_episode": "episode:1",
                            "t_valid": "2026-01-10T10:30:00Z",
                            "t_ingested": "2026-01-10T10:30:00Z",
                            "scope": "org",
                            "ft_score": 30.0
                        }),
                        json!({
                            "fact_id": "fact:generic-2",
                            "fact_type": "note",
                            "content": "Caroline is excited about building her own family through adoption.",
                            "quote": "Caroline is excited about building her own family through adoption.",
                            "source_episode": "episode:2",
                            "t_valid": "2026-01-09T10:30:00Z",
                            "t_ingested": "2026-01-09T10:30:00Z",
                            "scope": "org",
                            "ft_score": 25.0
                        }),
                    ],
                    Some("caroline") => vec![json!({
                        "fact_id": "fact:generic-1",
                        "fact_type": "note",
                        "content": "Caroline passed the adoption agency interviews last Friday.",
                        "quote": "Caroline passed the adoption agency interviews last Friday.",
                        "source_episode": "episode:1",
                        "t_valid": "2026-01-10T10:30:00Z",
                        "t_ingested": "2026-01-10T10:30:00Z",
                        "scope": "org",
                        "ft_score": 30.0
                    })],
                    Some("lgbtq") | Some("support") | Some("group") => vec![json!({
                        "fact_id": "fact:support-group",
                        "fact_type": "note",
                        "content": "Caroline attended the LGBTQ support group recently.",
                        "quote": "Caroline attended the LGBTQ support group recently.",
                        "source_episode": "episode:3",
                        "t_valid": "2026-01-08T10:30:00Z",
                        "t_ingested": "2026-01-08T10:30:00Z",
                        "scope": "org",
                        "ft_score": 5.0
                    })],
                    _ => vec![],
                })
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let service = crate::service::MemoryService::new(
            Arc::new(FallbackPreferenceDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let lexical_result = select_fact_records_for_query(
            &service,
            FactQueryParams {
                namespace: "org",
                scope: "org",
                cutoff_iso: "2026-01-15T10:30:00Z",
                query_opt: Some("caroline lgbtq support group"),
                limit: 5,
                project: None,
                fact_types: &[],
            },
        )
        .await
        .expect("fallback records");

        let fact_ids = lexical_result
            .records
            .iter()
            .filter_map(|record| record.get("fact_id").and_then(Value::as_str))
            .collect::<Vec<_>>();

        assert_eq!(
            lexical_result.retrieval_tier,
            RetrievalTier::EpisodeFallback
        );
        assert_eq!(fact_ids.first().copied(), Some("fact:support-group"));
    }

    #[tokio::test]
    async fn select_fact_records_for_short_query_uses_term_fallback() {
        struct ShortQueryFallbackDbClient;

        #[async_trait]
        impl DbClient for ShortQueryFallbackDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(match query_contains {
                    Some("What degree did I graduate with?") => vec![],
                    Some("degree graduate") => vec![json!({
                        "fact_id": "fact:answer",
                        "fact_type": "note",
                        "content": "I will graduate with a degree in Business Administration.",
                        "quote": "I will graduate with a degree in Business Administration.",
                        "source_episode": "episode:1",
                        "t_valid": "2026-01-10T10:30:00Z",
                        "t_ingested": "2026-01-10T10:30:00Z",
                        "scope": "org",
                        "ft_score": 4.0
                    })],
                    Some("degree") => vec![
                        json!({
                            "fact_id": "fact:generic",
                            "fact_type": "note",
                            "content": "The degree committee met to review course requirements.",
                            "quote": "The degree committee met to review course requirements.",
                            "source_episode": "episode:2",
                            "t_valid": "2026-01-09T10:30:00Z",
                            "t_ingested": "2026-01-09T10:30:00Z",
                            "scope": "org",
                            "ft_score": 8.0
                        }),
                        json!({
                            "fact_id": "fact:answer",
                            "fact_type": "note",
                            "content": "I will graduate with a degree in Business Administration.",
                            "quote": "I will graduate with a degree in Business Administration.",
                            "source_episode": "episode:1",
                            "t_valid": "2026-01-10T10:30:00Z",
                            "t_ingested": "2026-01-10T10:30:00Z",
                            "scope": "org",
                            "ft_score": 4.0
                        }),
                    ],
                    Some("graduate") => vec![json!({
                        "fact_id": "fact:answer",
                        "fact_type": "note",
                        "content": "I will graduate with a degree in Business Administration.",
                        "quote": "I will graduate with a degree in Business Administration.",
                        "source_episode": "episode:1",
                        "t_valid": "2026-01-10T10:30:00Z",
                        "t_ingested": "2026-01-10T10:30:00Z",
                        "scope": "org",
                        "ft_score": 4.0
                    })],
                    _ => vec![],
                })
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let service = crate::service::MemoryService::new(
            Arc::new(ShortQueryFallbackDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let lexical_result = select_fact_records_for_query(
            &service,
            FactQueryParams {
                namespace: "org",
                scope: "org",
                cutoff_iso: "2026-01-15T10:30:00Z",
                query_opt: Some("What degree did I graduate with?"),
                limit: 5,
                project: None,
                fact_types: &[],
            },
        )
        .await
        .expect("short-query fallback records");

        let fact_ids = lexical_result
            .records
            .iter()
            .filter_map(|record| record.get("fact_id").and_then(Value::as_str))
            .collect::<Vec<_>>();

        assert_eq!(
            lexical_result.retrieval_tier,
            RetrievalTier::EpisodeFallback
        );
        assert_eq!(fact_ids.first().copied(), Some("fact:answer"));
    }

    #[tokio::test]
    async fn assemble_context_marks_term_fallback_results_with_fallback_tier() {
        struct FallbackTierDbClient;

        #[async_trait::async_trait]
        impl DbClient for FallbackTierDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(match query_contains {
                    Some("atlas launch checklist") => vec![],
                    Some("atlas") => vec![json!({
                        "fact_id": "fact:fallback",
                        "fact_type": "note",
                        "content": "Atlas launch is scheduled.",
                        "quote": "Atlas launch is scheduled.",
                        "source_episode": "episode:1",
                        "t_valid": "2026-01-10T10:30:00Z",
                        "t_ingested": "2026-01-10T10:30:00Z",
                        "scope": "org"
                    })],
                    Some("launch") => vec![json!({
                        "fact_id": "fact:fallback",
                        "fact_type": "note",
                        "content": "Atlas launch is scheduled.",
                        "quote": "Atlas launch is scheduled.",
                        "source_episode": "episode:1",
                        "t_valid": "2026-01-10T10:30:00Z",
                        "t_ingested": "2026-01-10T10:30:00Z",
                        "scope": "org"
                    })],
                    Some("checklist") => vec![json!({
                        "fact_id": "fact:fallback",
                        "fact_type": "note",
                        "content": "Atlas launch is scheduled.",
                        "quote": "Atlas launch is scheduled.",
                        "source_episode": "episode:1",
                        "t_valid": "2026-01-10T10:30:00Z",
                        "t_ingested": "2026-01-10T10:30:00Z",
                        "scope": "org"
                    })],
                    _ => vec![],
                })
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let service = crate::service::MemoryService::new(
            Arc::new(FallbackTierDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let items = assemble_context(
            &service,
            AssembleContextRequest {
                query: "atlas launch checklist".to_string(),
                scope: "org".to_string(),
                as_of: None,
                budget: 5,
                project: None,
                fact_types: vec![],
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            },
        )
        .await
        .expect("assemble context with fallback result");

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].retrieval_tier.as_deref(), Some("fallback"));
        assert!(items[0].rationale.contains("tier=fallback"));
    }

    #[tokio::test]
    async fn assemble_context_falls_back_to_episode_content_when_no_facts_match() {
        struct EpisodeContentFallbackDbClient;

        #[async_trait::async_trait]
        impl DbClient for EpisodeContentFallbackDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_by_content(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(match query_contains {
                    Some("hello world") => vec![json!({
                        "episode_id": "episode:doc",
                        "source_type": "document",
                        "source_id": "fixture:pdf",
                        "content": "Hello World from episode fallback.",
                        "t_ref": "2026-04-07T10:00:00Z",
                        "t_ingested": "2026-04-07T10:00:00Z",
                        "scope": "org",
                        "visibility_scope": "org",
                        "policy_tags": [],
                    })],
                    _ => vec![],
                })
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let service = crate::service::MemoryService::new(
            Arc::new(EpisodeContentFallbackDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let items = assemble_context(
            &service,
            AssembleContextRequest {
                query: "hello world".to_string(),
                scope: "org".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
                project: None,
                fact_types: vec![],
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            },
        )
        .await
        .expect("episode fallback context");

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].source_episode, "episode:doc");
        assert_eq!(items[0].retrieval_tier.as_deref(), Some("fallback"));
        assert!(items[0].content.contains("Hello World"));
    }

    #[test]
    fn expand_temporal_synonyms_extracts_month_year_and_residual_terms() {
        let cutoff = chrono::DateTime::parse_from_rfc3339("2026-04-07T12:00:00Z")
            .expect("cutoff")
            .with_timezone(&Utc);

        let expansion = expand_temporal_synonyms("march 2026 launch review", cutoff)
            .expect("temporal expansion");

        assert_eq!(
            expansion.temporal_groups,
            vec![vec!["march 2026".to_string(), "2026-03".to_string()]]
        );
        assert_eq!(expansion.residual_query.as_deref(), Some("launch review"));
    }

    #[test]
    fn expand_temporal_synonyms_expands_weekday_to_date_relative_to_as_of() {
        let expansion = expand_temporal_synonyms("monday planning", fixed_temporal_cutoff())
            .expect("temporal expansion");

        assert_eq!(expansion.temporal_groups.len(), 1);
        let group = &expansion.temporal_groups[0];
        assert!(group.contains(&"2026-04-06".to_string()));
        assert!(group.contains(&"april 2026".to_string()));
        assert!(group.contains(&"2026-04".to_string()));
        assert!(group.contains(&"monday".to_string()));
        assert_eq!(expansion.residual_query.as_deref(), Some("planning"));
    }

    #[test]
    fn expand_temporal_synonyms_expands_this_week_to_current_week_dates() {
        let expansion = expand_temporal_synonyms("this week launch", fixed_temporal_cutoff())
            .expect("temporal expansion");

        assert_eq!(expansion.temporal_groups.len(), 1);
        let group = &expansion.temporal_groups[0];
        for date in [
            "2026-04-06",
            "2026-04-07",
            "2026-04-08",
            "2026-04-09",
            "2026-04-10",
            "2026-04-11",
            "2026-04-12",
        ] {
            assert!(
                group.contains(&date.to_string()),
                "expected current-week group to include {date}, got {group:?}"
            );
        }
        assert_eq!(expansion.residual_query.as_deref(), Some("launch"));
    }

    #[test]
    fn expand_temporal_synonyms_expands_quarter_to_current_year_month_range() {
        let expansion = expand_temporal_synonyms("q1 closeout", fixed_temporal_cutoff())
            .expect("temporal expansion");

        assert_eq!(expansion.temporal_groups.len(), 1);
        let group = &expansion.temporal_groups[0];
        for term in [
            "q1",
            "january 2026",
            "2026-01",
            "february 2026",
            "2026-02",
            "march 2026",
            "2026-03",
        ] {
            assert!(
                group.contains(&term.to_string()),
                "expected quarter group to include {term}, got {group:?}"
            );
        }
        assert_eq!(expansion.residual_query.as_deref(), Some("closeout"));
    }

    #[test]
    fn build_ranked_context_facts_promotes_temporal_tier_over_direct() {
        let cutoff = Utc::now();
        let fact = create_test_fact("fact:temporal", cutoff - chrono::Duration::days(1));

        let ranked = build_ranked_context_facts(
            vec![
                (fact.clone(), RetrievalTier::Direct),
                (fact, RetrievalTier::TemporalExpanded),
            ],
            Vec::new(),
            Vec::new(),
            Some("march 2026 launch review"),
            "org",
            cutoff,
            crate::service::decayed_confidence,
        );

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].retrieval_tier, RetrievalTier::TemporalExpanded);
        assert!(ranked[0].rationale.contains("tier=temporal"));
    }

    #[test]
    fn build_ranked_context_facts_weights_graph_results_by_origin_factor() {
        let cutoff = Utc::now();

        let mut inferred = create_test_fact("fact:inferred", cutoff - chrono::Duration::days(1));
        inferred.content = "Inferred fact content from beta community".to_string();
        let mut extracted = create_test_fact("fact:extracted", cutoff - chrono::Duration::days(1));
        extracted.content = "Extracted fact content from alpha community".to_string();

        let mut ranked = build_ranked_context_facts(
            Vec::new(),
            vec![
                (
                    inferred,
                    "matched community summary via community:beta".to_string(),
                    0.2,
                ),
                (
                    extracted,
                    "matched community summary via community:alpha".to_string(),
                    1.0,
                ),
            ],
            Vec::new(),
            Some("launch workstream"),
            "org",
            cutoff,
            crate::service::decayed_confidence,
        );
        sort_ranked_context_facts(&mut ranked);

        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].fact.fact_id, "fact:extracted");
        assert_eq!(ranked[1].fact.fact_id, "fact:inferred");
    }

    #[test]
    fn ranked_relevance_score_softly_penalizes_frequently_accessed_facts() {
        let cutoff = Utc::now();
        let cold = create_ranked_test_fact(
            "fact:cold",
            "episode:cold",
            cutoff,
            10.0,
            5.0,
            0,
            &["alpha"],
        );
        let hot =
            create_ranked_test_fact("fact:hot", "episode:hot", cutoff, 10.0, 5.0, 50, &["alpha"]);

        assert!(ranked_relevance_score(&cold) > ranked_relevance_score(&hot));
    }

    #[test]
    fn ranked_relevance_score_prefers_experience_facts_when_other_signals_tie() {
        let cutoff = Utc::now();
        let mut note = create_ranked_test_fact(
            "fact:note",
            "episode:shared",
            cutoff,
            10.0,
            5.0,
            0,
            &["hotel", "quiet"],
        );
        note.fact.fact_type = "note".to_string();

        let mut experience = create_ranked_test_fact(
            "fact:experience",
            "episode:shared",
            cutoff,
            10.0,
            5.0,
            0,
            &["hotel", "quiet"],
        );
        experience.fact.fact_type = "experience".to_string();

        assert!(
            ranked_relevance_score(&experience) > ranked_relevance_score(&note),
            "experience memories should beat otherwise identical generic notes"
        );
    }

    #[test]
    fn build_ranked_context_facts_prefers_user_memories_for_first_person_queries() {
        let cutoff = Utc::now();

        let mut user_fact = create_test_fact("fact:user", cutoff);
        user_fact.content =
            "User: I was thrilled to hear modern beats blended with Pacific sounds live."
                .to_string();
        user_fact.quote = user_fact.content.clone();
        user_fact.ft_score = 4.0;

        let mut assistant_fact = create_test_fact("fact:assistant", cutoff);
        assistant_fact.content =
            "Assistant: It sounds like live music gives you a strong sense of cultural connection."
                .to_string();
        assistant_fact.quote = assistant_fact.content.clone();
        assistant_fact.ft_score = 4.0;

        let mut ranked = build_ranked_context_facts(
            vec![
                (assistant_fact, RetrievalTier::Direct),
                (user_fact, RetrievalTier::Direct),
            ],
            Vec::new(),
            Vec::new(),
            Some(
                "I recently attended an event where there was a unique blend of modern beats with Pacific sounds.",
            ),
            "org",
            cutoff,
            crate::service::decayed_confidence,
        );
        sort_ranked_context_facts(&mut ranked);

        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].fact.fact_id, "fact:user");
        assert_eq!(ranked[1].fact.fact_id, "fact:assistant");
    }

    #[test]
    fn select_ranked_context_facts_caps_source_episode_before_budget_fill() {
        let cutoff = Utc::now();
        let selected = select_ranked_context_facts(
            vec![
                create_ranked_test_fact(
                    "fact:a1",
                    "episode:alpha",
                    cutoff,
                    12.0,
                    10.0,
                    0,
                    &["alpha", "shared"],
                ),
                create_ranked_test_fact(
                    "fact:a2",
                    "episode:alpha",
                    cutoff - chrono::Duration::days(1),
                    11.0,
                    9.0,
                    0,
                    &["alpha", "shared"],
                ),
                create_ranked_test_fact(
                    "fact:a3",
                    "episode:alpha",
                    cutoff - chrono::Duration::days(2),
                    10.5,
                    8.0,
                    0,
                    &["alpha", "shared"],
                ),
                create_ranked_test_fact(
                    "fact:b1",
                    "episode:beta",
                    cutoff - chrono::Duration::days(3),
                    9.5,
                    8.0,
                    0,
                    &["beta"],
                ),
                create_ranked_test_fact(
                    "fact:c1",
                    "episode:gamma",
                    cutoff - chrono::Duration::days(4),
                    9.0,
                    8.0,
                    0,
                    &["gamma"],
                ),
            ],
            4,
            None,
            vec![],
        );

        assert_eq!(selected.len(), 4);
        assert_eq!(
            selected
                .iter()
                .filter(|item| item.fact.source_episode == "episode:alpha")
                .count(),
            2
        );
        assert!(
            selected
                .iter()
                .any(|item| item.fact.source_episode == "episode:beta")
        );
        assert!(
            selected
                .iter()
                .any(|item| item.fact.source_episode == "episode:gamma")
        );
    }

    #[test]
    fn select_ranked_context_facts_prefers_novel_index_keys_when_scores_are_close() {
        let cutoff = Utc::now();
        let selected = select_ranked_context_facts(
            vec![
                create_ranked_test_fact(
                    "fact:anchor",
                    "episode:anchor",
                    cutoff,
                    10.0,
                    10.0,
                    0,
                    &["alpha", "beta"],
                ),
                create_ranked_test_fact(
                    "fact:redundant",
                    "episode:redundant",
                    cutoff - chrono::Duration::days(1),
                    9.9,
                    9.0,
                    0,
                    &["alpha", "beta"],
                ),
                create_ranked_test_fact(
                    "fact:diverse",
                    "episode:diverse",
                    cutoff - chrono::Duration::days(1),
                    9.7,
                    9.0,
                    0,
                    &["gamma", "delta"],
                ),
            ],
            2,
            None,
            vec![],
        );

        let fact_ids = selected
            .iter()
            .map(|item| item.fact.fact_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(fact_ids, vec!["fact:anchor", "fact:diverse"]);
    }

    #[test]
    fn select_ranked_context_facts_prefers_temporal_spread_for_tied_candidates() {
        let anchor_time = chrono::DateTime::parse_from_rfc3339("2026-01-01T12:00:00Z")
            .expect("anchor time")
            .with_timezone(&Utc);
        let selected = select_ranked_context_facts(
            vec![
                create_ranked_test_fact(
                    "fact:anchor",
                    "episode:anchor",
                    anchor_time,
                    10.0,
                    10.0,
                    0,
                    &[],
                ),
                create_ranked_test_fact(
                    "fact:nearby",
                    "episode:nearby",
                    anchor_time + chrono::Duration::days(1),
                    9.5,
                    9.0,
                    0,
                    &[],
                ),
                create_ranked_test_fact(
                    "fact:distant",
                    "episode:distant",
                    anchor_time + chrono::Duration::days(60),
                    9.5,
                    9.0,
                    0,
                    &[],
                ),
            ],
            2,
            None,
            vec![],
        );

        let fact_ids = selected
            .iter()
            .map(|item| item.fact.fact_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(fact_ids, vec!["fact:anchor", "fact:distant"]);
    }

    #[test]
    fn select_ranked_context_facts_prefers_in_window_items_over_stale_out_of_window_digests() {
        let anchor_time = chrono::DateTime::parse_from_rfc3339("2026-03-10T12:00:00Z")
            .expect("anchor time")
            .with_timezone(&Utc);
        let temporal_focus = infer_temporal_window(
            "march april 2026 alpha suite decisions",
            fixed_temporal_cutoff(),
        );

        let selected = select_ranked_context_facts(
            vec![
                create_ranked_test_fact(
                    "fact:stale-digest",
                    "episode:stale-digest",
                    chrono::DateTime::parse_from_rfc3339("2025-10-14T09:00:00Z")
                        .expect("stale time")
                        .with_timezone(&Utc),
                    12.0,
                    11.0,
                    0,
                    &["alpha", "suite", "decisions"],
                ),
                create_ranked_test_fact(
                    "fact:in-window",
                    "episode:in-window",
                    anchor_time,
                    10.5,
                    9.0,
                    0,
                    &["alpha", "suite", "decisions"],
                ),
            ],
            1,
            temporal_focus,
            vec![
                "march".to_string(),
                "april".to_string(),
                "2026".to_string(),
                "alpha".to_string(),
                "suite".to_string(),
                "decision".to_string(),
            ],
        );

        let fact_ids = selected
            .iter()
            .map(|item| item.fact.fact_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(fact_ids, vec!["fact:in-window"]);
    }

    #[test]
    fn select_ranked_context_facts_stops_before_budget_for_far_out_of_window_tail() {
        let temporal_focus = infer_temporal_window(
            "march april 2026 alpha suite delta control signal monitor orbit portal decisions",
            fixed_temporal_cutoff(),
        );
        let selected = select_ranked_context_facts(
            vec![
                create_ranked_test_fact(
                    "fact:alpha",
                    "episode:alpha",
                    chrono::DateTime::parse_from_rfc3339("2026-03-10T09:00:00Z")
                        .expect("alpha time")
                        .with_timezone(&Utc),
                    11.0,
                    10.0,
                    0,
                    &["alpha", "suite", "decisions"],
                ),
                create_ranked_test_fact(
                    "fact:delta",
                    "episode:delta",
                    chrono::DateTime::parse_from_rfc3339("2026-03-11T09:00:00Z")
                        .expect("delta time")
                        .with_timezone(&Utc),
                    10.5,
                    9.5,
                    0,
                    &["delta", "control", "decisions"],
                ),
                create_ranked_test_fact(
                    "fact:signal",
                    "episode:signal",
                    chrono::DateTime::parse_from_rfc3339("2026-04-02T09:00:00Z")
                        .expect("signal time")
                        .with_timezone(&Utc),
                    10.0,
                    9.0,
                    0,
                    &["signal", "monitor", "decisions"],
                ),
                create_ranked_test_fact(
                    "fact:orbit",
                    "episode:orbit",
                    chrono::DateTime::parse_from_rfc3339("2026-04-03T09:00:00Z")
                        .expect("orbit time")
                        .with_timezone(&Utc),
                    9.8,
                    9.0,
                    0,
                    &["orbit", "portal", "decisions"],
                ),
                create_ranked_test_fact(
                    "fact:stale-1",
                    "episode:stale-1",
                    chrono::DateTime::parse_from_rfc3339("2025-10-14T09:00:00Z")
                        .expect("stale 1 time")
                        .with_timezone(&Utc),
                    12.0,
                    11.0,
                    0,
                    &["alpha", "suite", "decisions"],
                ),
                create_ranked_test_fact(
                    "fact:stale-2",
                    "episode:stale-2",
                    chrono::DateTime::parse_from_rfc3339("2025-10-13T09:00:00Z")
                        .expect("stale 2 time")
                        .with_timezone(&Utc),
                    11.5,
                    10.5,
                    0,
                    &["orbit", "portal", "decisions"],
                ),
            ],
            6,
            temporal_focus,
            vec![
                "march".to_string(),
                "april".to_string(),
                "2026".to_string(),
                "alpha".to_string(),
                "suite".to_string(),
                "delta".to_string(),
                "control".to_string(),
                "signal".to_string(),
                "monitor".to_string(),
                "orbit".to_string(),
                "portal".to_string(),
                "decision".to_string(),
            ],
        );

        let fact_ids = selected
            .iter()
            .map(|item| item.fact.fact_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            fact_ids,
            vec!["fact:alpha", "fact:delta", "fact:signal", "fact:orbit"]
        );
    }

    #[test]
    fn prune_redundant_selected_facts_removes_broad_umbrella_summaries() {
        let selected = prune_redundant_selected_facts(
            vec![
                // Specific facts first — highest relevance, they fill the protected set.
                {
                    let mut fact = create_ranked_test_fact(
                        "fact:atlas",
                        "episode:atlas",
                        chrono::DateTime::parse_from_rfc3339("2026-03-15T09:00:00Z")
                            .expect("atlas time")
                            .with_timezone(&Utc),
                        10.0,
                        9.0,
                        0,
                        &[],
                    );
                    fact.fact.content = "March 2026 Atlas blocker: legal signoff is still missing for the reseller appendix.".to_string();
                    fact.fact.quote = fact.fact.content.clone();
                    fact
                },
                {
                    let mut fact = create_ranked_test_fact(
                        "fact:beacon",
                        "episode:beacon",
                        chrono::DateTime::parse_from_rfc3339("2026-03-16T09:00:00Z")
                            .expect("beacon time")
                            .with_timezone(&Utc),
                        9.9,
                        9.0,
                        0,
                        &[],
                    );
                    fact.fact.content =
                        "March 2026 Beacon blocker and decision: finance approved the revised launch budget after the blocker was resolved."
                            .to_string();
                    fact.fact.quote = fact.fact.content.clone();
                    fact
                },
                {
                    let mut fact = create_ranked_test_fact(
                        "fact:atlas-april",
                        "episode:atlas-april",
                        chrono::DateTime::parse_from_rfc3339("2026-04-05T09:00:00Z")
                            .expect("atlas april time")
                            .with_timezone(&Utc),
                        9.8,
                        9.0,
                        0,
                        &[],
                    );
                    fact.fact.content = "April 2026 Atlas decision: partner onboarding moved to the managed rollout path.".to_string();
                    fact.fact.quote = fact.fact.content.clone();
                    fact
                },
                {
                    let mut fact = create_ranked_test_fact(
                        "fact:beacon-april",
                        "episode:beacon-april",
                        chrono::DateTime::parse_from_rfc3339("2026-04-06T09:00:00Z")
                            .expect("beacon april time")
                            .with_timezone(&Utc),
                        9.7,
                        9.0,
                        0,
                        &[],
                    );
                    fact.fact.content = "April 2026 Beacon blocker: the migration depends on the final tax mapping table.".to_string();
                    fact.fact.quote = fact.fact.content.clone();
                    fact
                },
                // Broad umbrella summaries — lower scores, NOT protected.
                {
                    let mut fact = create_ranked_test_fact(
                        "fact:digest-a",
                        "episode:digest-a",
                        chrono::DateTime::parse_from_rfc3339("2026-04-07T09:00:00Z")
                            .expect("digest a time")
                            .with_timezone(&Utc),
                        8.5,
                        7.5,
                        0,
                        &[],
                    );
                    fact.fact.content = "Quarterly digest for Atlas and Beacon repeated blockers and decisions keywords across March and April 2026 without resolving any specific item.".to_string();
                    fact.fact.quote = fact.fact.content.clone();
                    fact
                },
                {
                    let mut fact = create_ranked_test_fact(
                        "fact:digest-b",
                        "episode:digest-b",
                        chrono::DateTime::parse_from_rfc3339("2026-04-07T10:00:00Z")
                            .expect("digest b time")
                            .with_timezone(&Utc),
                        8.3,
                        7.3,
                        0,
                        &[],
                    );
                    fact.fact.content = "Combined Atlas and Beacon digest covering March and April 2026: blocker updates, decision summaries, and launch progress across both workstreams.".to_string();
                    fact.fact.quote = fact.fact.content.clone();
                    fact
                },
            ],
            &[
                "march".to_string(),
                "april".to_string(),
                "2026".to_string(),
                "atlas".to_string(),
                "beacon".to_string(),
                "blocker".to_string(),
                "decision".to_string(),
            ],
            None,
        );

        let fact_ids = selected
            .iter()
            .map(|item| item.fact.fact_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            fact_ids,
            vec![
                "fact:atlas",
                "fact:beacon",
                "fact:atlas-april",
                "fact:beacon-april"
            ]
        );
    }

    #[test]
    fn sort_facts_by_recency_handles_empty() {
        let mut facts: Vec<crate::models::Fact> = vec![];
        sort_facts_by_recency(&mut facts);
        assert!(facts.is_empty());
    }

    #[test]
    fn sort_facts_by_recency_handles_single() {
        let mut facts = vec![create_test_fact("fact:1", Utc::now())];
        sort_facts_by_recency(&mut facts);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].fact_id, "fact:1");
    }

    #[test]
    fn filter_facts_by_policy_returns_empty_for_empty_input() {
        let access = AccessContext::default();
        let result = filter_facts_by_policy(vec![], &access);
        assert!(result.is_empty());
    }

    #[test]
    fn filter_facts_by_policy_skips_invalid_records() {
        let access = AccessContext::default();
        let records = vec![json!({"invalid": "data"})];
        let result = filter_facts_by_policy(records, &access);
        assert!(result.is_empty());
    }

    #[test]
    fn filter_facts_by_policy_filters_by_allowed_tags() {
        let mut fact1 = create_test_fact("fact:1", Utc::now());
        fact1.policy_tags = vec!["allowed".to_string(), "other".to_string()];

        let mut fact2 = create_test_fact("fact:2", Utc::now());
        fact2.policy_tags = vec!["blocked".to_string()];

        let access = AccessContext {
            allowed_scopes: None,
            allowed_tags: Some(vec!["allowed".to_string()]),
            caller_id: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: None,
        };

        let records = vec![
            json!({
                "fact_id": "fact:1",
                "fact_type": "note",
                "content": "Test",
                "quote": "Quote",
                "source_episode": "episode:1",
                "t_valid": "2024-01-15T10:30:00Z",
                "scope": "org",
                "policy_tags": ["allowed", "other"]
            }),
            json!({
                "fact_id": "fact:2",
                "fact_type": "note",
                "content": "Test",
                "quote": "Quote",
                "source_episode": "episode:1",
                "t_valid": "2024-01-15T10:30:00Z",
                "scope": "org",
                "policy_tags": ["blocked"]
            }),
        ];

        let result = filter_facts_by_policy(records, &access);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].fact_id, "fact:1");
    }

    #[test]
    fn filter_facts_by_policy_allows_all_when_no_tags_specified() {
        let access = AccessContext {
            allowed_scopes: None,
            allowed_tags: None,
            caller_id: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: None,
        };

        let records = vec![
            json!({
                "fact_id": "fact:1",
                "fact_type": "note",
                "content": "Test",
                "quote": "Quote",
                "source_episode": "episode:1",
                "t_valid": "2024-01-15T10:30:00Z",
                "scope": "org",
                "policy_tags": ["tag1"]
            }),
            json!({
                "fact_id": "fact:2",
                "fact_type": "note",
                "content": "Test",
                "quote": "Quote",
                "source_episode": "episode:1",
                "t_valid": "2024-01-15T10:30:00Z",
                "scope": "org"
            }),
        ];

        let result = filter_facts_by_policy(records, &access);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn filter_facts_by_policy_handles_wrapped_objects() {
        let access = AccessContext::default();

        let records = vec![json!({
            "Object": {
                "fact_id": "fact:1",
                "fact_type": "note",
                "content": "Test",
                "quote": "Quote",
                "source_episode": "episode:1",
                "t_valid": "2024-01-15T10:30:00Z",
                "scope": "org"
            }
        })];

        let result = filter_facts_by_policy(records, &access);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].fact_id, "fact:1");
    }

    #[test]
    fn filter_facts_by_policy_handles_array_wrapped_objects() {
        let access = AccessContext::default();

        let records = vec![json!({
            "Array": [
                {
                    "Object": {
                        "fact_id": "fact:1",
                        "fact_type": "note",
                        "content": "Test",
                        "quote": "Quote",
                        "source_episode": "episode:1",
                        "t_valid": "2024-01-15T10:30:00Z",
                        "scope": "org"
                    }
                },
                {
                    "Object": {
                        "fact_id": "fact:2",
                        "fact_type": "note",
                        "content": "Test2",
                        "quote": "Quote2",
                        "source_episode": "episode:2",
                        "t_valid": "2024-01-15T10:30:00Z",
                        "scope": "org"
                    }
                }
            ]
        })];

        let result = filter_facts_by_policy(records, &access);
        assert_eq!(result.len(), 2);
    }

    #[tokio::test]
    async fn assemble_context_uses_db_side_community_lookup_for_summary_matches() {
        struct CommunityLookupDbClient {
            community_lookup_calls: AtomicUsize,
            entity_link_fact_calls: AtomicUsize,
        }

        #[async_trait::async_trait]
        impl DbClient for CommunityLookupDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                assert_eq!(table, "fact");
                Ok(vec![])
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                if query_contains.is_some() {
                    Ok(vec![])
                } else {
                    panic!(
                        "community fact expansion should not use unfiltered select_facts_filtered fallback"
                    )
                }
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                self.entity_link_fact_calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(entity_links, &["entity:alice".to_string()]);

                Ok(vec![
                    json!({
                        "fact_id": "fact:community",
                        "fact_type": "note",
                        "content": "Alice works on project Atlas",
                        "quote": "Alice works on project Atlas",
                        "source_episode": "episode:1",
                        "t_valid": "2026-01-15T10:30:00Z",
                        "t_ingested": "2026-01-15T10:30:00Z",
                        "scope": "org",
                        "entity_links": ["entity:alice"],
                        "policy_tags": [],
                        "provenance": {"source_episode": "episode:1"}
                    }),
                    json!({
                        "fact_id": "fact:other",
                        "fact_type": "note",
                        "content": "Mallory works elsewhere",
                        "quote": "Mallory works elsewhere",
                        "source_episode": "episode:2",
                        "t_valid": "2026-01-15T10:30:00Z",
                        "t_ingested": "2026-01-15T10:30:00Z",
                        "scope": "org",
                        "entity_links": ["entity:mallory"],
                        "policy_tags": [],
                        "provenance": {"source_episode": "episode:2"}
                    }),
                ])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                self.community_lookup_calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(query, "alice atlas");

                Ok(vec![json!({
                    "community_id": "community:atlas",
                    "summary": "Alice and the Atlas project team",
                    "member_entities": ["entity:alice"]
                })])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }
            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let db_client = Arc::new(CommunityLookupDbClient {
            community_lookup_calls: AtomicUsize::new(0),
            entity_link_fact_calls: AtomicUsize::new(0),
        });
        let service = crate::service::MemoryService::new(
            db_client.clone(),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &service,
            crate::models::AssembleContextRequest {
                query: "alice atlas".to_string(),
                scope: "org".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
                project: None,
                fact_types: vec![],
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            },
        )
        .await
        .expect("assemble context");

        assert_eq!(db_client.community_lookup_calls.load(Ordering::SeqCst), 1);
        assert_eq!(db_client.entity_link_fact_calls.load(Ordering::SeqCst), 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].fact_id, "fact:community");
        assert!(results[0].rationale.contains("community:atlas"));
    }

    #[tokio::test]
    async fn assemble_context_without_lexical_or_graph_matches_returns_empty() {
        struct EmptyDbClient;

        #[async_trait::async_trait]
        impl DbClient for EmptyDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }
            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let db_client = Arc::new(EmptyDbClient);
        let service = crate::service::MemoryService::new(
            db_client.clone(),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &service,
            crate::models::AssembleContextRequest {
                query: "alice platform".to_string(),
                scope: "org".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
                project: None,
                fact_types: vec![],
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            },
        )
        .await
        .expect("assemble context");

        assert!(
            results.is_empty(),
            "without lexical or graph matches, assemble_context should return no results"
        );
    }

    #[tokio::test]
    async fn assemble_context_prefers_direct_lexical_matches_over_newer_community_expansion() {
        struct FusionDbClient;

        #[async_trait::async_trait]
        impl DbClient for FusionDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(match query_contains {
                    Some("atlas launch") => vec![json!({
                        "fact_id": "fact:direct",
                        "fact_type": "note",
                        "content": "Atlas launch checklist is blocked on DNS cutover.",
                        "quote": "Atlas launch checklist is blocked on DNS cutover.",
                        "source_episode": "episode:direct",
                        "t_valid": "2026-01-10T10:30:00Z",
                        "t_ingested": "2026-01-10T10:30:00Z",
                        "scope": "org",
                        "entity_links": ["entity:atlas"],
                        "policy_tags": [],
                        "provenance": {"source_episode": "episode:direct"},
                        "ft_score": 100.0
                    })],
                    Some("atlas") | Some("launch") => vec![],
                    other => panic!("unexpected fallback query: {other:?}"),
                })
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                assert_eq!(entity_links, &["entity:atlas".to_string()]);
                Ok(vec![json!({
                    "fact_id": "fact:community",
                    "fact_type": "note",
                    "content": "Atlas team sync moved to Friday.",
                    "quote": "Atlas team sync moved to Friday.",
                    "source_episode": "episode:community",
                    "t_valid": "2026-01-15T10:30:00Z",
                    "t_ingested": "2026-01-15T10:30:00Z",
                    "scope": "org",
                    "entity_links": ["entity:atlas"],
                    "policy_tags": [],
                    "provenance": {"source_episode": "episode:community"}
                })])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                assert_eq!(query, "atlas launch");
                Ok(vec![json!({
                    "community_id": "community:atlas",
                    "summary": "Atlas launch workstream",
                    "member_entities": ["entity:atlas"]
                })])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }
            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let service = crate::service::MemoryService::new(
            Arc::new(FusionDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &service,
            crate::models::AssembleContextRequest {
                query: "atlas launch".to_string(),
                scope: "org".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
                project: None,
                fact_types: vec![],
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            },
        )
        .await
        .expect("assemble context");

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].fact_id, "fact:direct");
        assert!(
            results[0].rationale.contains("lexical"),
            "direct lexical result should explain itself as a lexical match, got: {}",
            results[0].rationale
        );
        assert_eq!(results[1].fact_id, "fact:community");
        assert!(results[1].rationale.contains("community:atlas"));
    }

    #[tokio::test]
    async fn assemble_context_orders_community_facts_by_matching_summary_relevance() {
        struct CommunityRankingDbClient;

        #[async_trait::async_trait]
        impl DbClient for CommunityRankingDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                assert_eq!(
                    entity_links,
                    &["entity:alpha".to_string(), "entity:beta".to_string()]
                );

                Ok(vec![
                    json!({
                        "fact_id": "fact:beta",
                        "fact_type": "note",
                        "content": "Beta launch note.",
                        "quote": "Beta launch note.",
                        "source_episode": "episode:beta",
                        "t_valid": "2026-01-20T10:30:00Z",
                        "t_ingested": "2026-01-20T10:30:00Z",
                        "scope": "org",
                        "entity_links": ["entity:beta"],
                        "policy_tags": [],
                        "provenance": {"source_episode": "episode:beta"}
                    }),
                    json!({
                        "fact_id": "fact:alpha",
                        "fact_type": "note",
                        "content": "Alpha launch note.",
                        "quote": "Alpha launch note.",
                        "source_episode": "episode:alpha",
                        "t_valid": "2026-01-10T10:30:00Z",
                        "t_ingested": "2026-01-10T10:30:00Z",
                        "scope": "org",
                        "entity_links": ["entity:alpha"],
                        "policy_tags": [],
                        "provenance": {"source_episode": "episode:alpha"}
                    }),
                ])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                assert_eq!(query, "launch workstream");
                Ok(vec![
                    json!({
                        "community_id": "community:alpha",
                        "summary": "Alpha launch workstream",
                        "member_entities": ["entity:alpha"],
                        "ft_score": 20.0
                    }),
                    json!({
                        "community_id": "community:beta",
                        "summary": "Beta launch workstream",
                        "member_entities": ["entity:beta"],
                        "ft_score": 10.0
                    }),
                ])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }
            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let service = crate::service::MemoryService::new(
            Arc::new(CommunityRankingDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &service,
            crate::models::AssembleContextRequest {
                query: "launch workstream".to_string(),
                scope: "org".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
                project: None,
                fact_types: vec![],
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            },
        )
        .await
        .expect("assemble context");

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].fact_id, "fact:alpha");
        assert!(results[0].rationale.contains("community:alpha"));
        assert_eq!(results[1].fact_id, "fact:beta");
    }

    #[tokio::test]
    async fn assemble_context_prefers_extracted_community_paths_over_higher_ranked_inferred_ones() {
        struct CommunityOriginWeightDbClient;

        #[async_trait::async_trait]
        impl DbClient for CommunityOriginWeightDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                assert_eq!(
                    entity_links,
                    &["entity:alpha".to_string(), "entity:beta".to_string()]
                );

                Ok(vec![
                    json!({
                        "fact_id": "fact:beta",
                        "fact_type": "note",
                        "content": "Beta launch note.",
                        "quote": "Beta launch note.",
                        "source_episode": "episode:beta",
                        "t_valid": "2026-01-15T10:30:00Z",
                        "t_ingested": "2026-01-15T10:30:00Z",
                        "scope": "org",
                        "entity_links": ["entity:beta"],
                        "policy_tags": [],
                        "confidence": 1.0,
                        "provenance": {"source_episode": "episode:beta"}
                    }),
                    json!({
                        "fact_id": "fact:alpha",
                        "fact_type": "note",
                        "content": "Alpha launch note.",
                        "quote": "Alpha launch note.",
                        "source_episode": "episode:alpha",
                        "t_valid": "2026-01-15T10:30:00Z",
                        "t_ingested": "2026-01-15T10:30:00Z",
                        "scope": "org",
                        "entity_links": ["entity:alpha"],
                        "policy_tags": [],
                        "confidence": 1.0,
                        "provenance": {"source_episode": "episode:alpha"}
                    }),
                ])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                node_id: &str,
                _cutoff: &str,
                _direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(match node_id {
                    "entity:alpha" => vec![json!({
                        "edge_id": "edge:alpha-extracted",
                        "in": "entity:alpha",
                        "relation": "knows",
                        "out": "entity:anchor_alpha",
                        "origin": "extracted",
                        "confidence": 0.9,
                        "t_valid": "2026-01-10T10:30:00Z",
                        "t_ingested": "2026-01-10T10:30:00Z"
                    })],
                    "entity:beta" => vec![json!({
                        "edge_id": "edge:beta-inferred",
                        "in": "entity:beta",
                        "relation": "knows",
                        "out": "entity:anchor_beta",
                        "origin": "inferred",
                        "confidence": 0.2,
                        "t_valid": "2026-01-10T10:30:00Z",
                        "t_ingested": "2026-01-10T10:30:00Z"
                    })],
                    _ => vec![],
                })
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                assert_eq!(query, "launch workstream");
                Ok(vec![
                    json!({
                        "community_id": "community:beta",
                        "summary": "Beta launch workstream",
                        "member_entities": ["entity:beta"],
                        "ft_score": 20.0
                    }),
                    json!({
                        "community_id": "community:alpha",
                        "summary": "Alpha launch workstream",
                        "member_entities": ["entity:alpha"],
                        "ft_score": 10.0
                    }),
                ])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let service = crate::service::MemoryService::new(
            Arc::new(CommunityOriginWeightDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &service,
            crate::models::AssembleContextRequest {
                query: "launch workstream".to_string(),
                scope: "org".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
                project: None,
                fact_types: vec![],
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            },
        )
        .await
        .expect("assemble context");

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].fact_id, "fact:alpha");
        assert_eq!(results[1].fact_id, "fact:beta");
    }

    #[tokio::test]
    async fn assemble_context_uses_provider_backed_semantic_similarity() {
        struct SemanticDbClient;

        #[async_trait::async_trait]
        impl DbClient for SemanticDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                panic!("semantic retrieval should not scan the full fact table")
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                let mut embedding = vec![0.0; DEFAULT_EMBEDDING_DIMENSION];
                embedding[0] = 1.0;
                Ok(vec![json!({
                    "fact_id": "fact:semantic",
                    "fact_type": "note",
                    "content": "Compensation increase approved for the engineering team",
                    "quote": "Compensation increase approved",
                    "source_episode": "episode:semantic",
                    "t_valid": "2026-01-15T10:30:00Z",
                    "t_ingested": "2026-01-15T10:30:00Z",
                    "scope": "org",
                    "entity_links": [],
                    "policy_tags": [],
                    "confidence": 0.9,
                    "provenance": {},
                    "embedding": embedding,
                    "sem_score": 0.99,
                })])
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        struct SemanticEmbeddingProvider;

        #[async_trait]
        impl EmbeddingProvider for SemanticEmbeddingProvider {
            fn is_enabled(&self) -> bool {
                true
            }

            fn provider_name(&self) -> &'static str {
                "test"
            }

            fn dimension(&self) -> usize {
                DEFAULT_EMBEDDING_DIMENSION
            }

            async fn embed(&self, _input: &str) -> Result<Vec<f64>, MemoryError> {
                let mut embedding = vec![0.0; DEFAULT_EMBEDDING_DIMENSION];
                embedding[0] = 1.0;
                Ok(embedding)
            }
        }

        let service = crate::service::MemoryService::new_with_embedding_provider(
            Arc::new(SemanticDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
            Arc::new(SemanticEmbeddingProvider),
            crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
        )
        .expect("service");

        let results = assemble_context(
            &service,
            crate::models::AssembleContextRequest {
                query: "salary raise".to_string(),
                scope: "org".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
                project: None,
                fact_types: vec![],
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            },
        )
        .await
        .expect("assemble context");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].fact_id, "fact:semantic");
        assert!(results[0].rationale.contains("semantic similarity"));
    }

    #[test]
    fn stored_community_summary_from_value_handles_wrapped_ft_score_number() {
        let summary = stored_community_summary_from_value(&json!({
            "community_id": "community:atlas",
            "summary": "Atlas workstream",
            "member_entities": ["entity:atlas"],
            "ft_score": {"Number": 42.5}
        }))
        .expect("community summary");

        assert_eq!(summary.ft_score, 42.5);
    }

    #[tokio::test]
    async fn expand_query_with_aliases_supports_multi_word_entities() {
        struct MultiWordAliasDbClient;

        #[async_trait]
        impl DbClient for MultiWordAliasDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                if normalized_name == "alice smith" {
                    return Ok(Some(json!({
                        "entity_id": "entity:alice_smith",
                        "aliases": ["alice s."]
                    })));
                }

                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                let mut results = Vec::new();
                for name in names {
                    if name == "alice smith" {
                        results.push(json!({
                            "entity_id": "entity:alice_smith",
                            "canonical_name_normalized": "alice smith",
                            "aliases": ["alice s."]
                        }));
                    }
                }
                Ok(results)
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let service = crate::service::MemoryService::new(
            Arc::new(MultiWordAliasDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let expanded =
            expand_query_with_aliases_for_test(&service, "alice smith atlas", "org").await;

        assert!(
            expanded.iter().any(|query| query == "alice s. atlas"),
            "multi-word entity alias should expand the full phrase, got: {expanded:?}"
        );
    }

    #[tokio::test]
    async fn community_expansion_returns_empty_when_no_entity_links_match() {
        struct EmptyCommunityFactDbClient;

        #[async_trait::async_trait]
        impl DbClient for EmptyCommunityFactDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                vec![json!({
                    "community_id": "community:orphan",
                    "summary": "Orphan community with no facts",
                    "member_entities": ["entity:nobody"],
                    "ft_score": 1.0
                })]
                .into_iter()
                .map(Ok)
                .collect::<Result<Vec<_>, _>>()
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let service = crate::service::MemoryService::new(
            Arc::new(EmptyCommunityFactDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &service,
            crate::models::AssembleContextRequest {
                query: "orphan community query".to_string(),
                scope: "org".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
                project: None,
                fact_types: vec![],
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            },
        )
        .await
        .expect("assemble context should not panic on empty community expansion");

        assert!(
            results.is_empty(),
            "community expansion with no matching entity_links should produce no results, got {}",
            results.len()
        );
    }

    #[tokio::test]
    async fn assemble_context_promotes_relevant_experience_candidates_into_primary_ranking() {
        struct ExperiencePrimaryRankingDbClient;

        #[async_trait]
        impl DbClient for ExperiencePrimaryRankingDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(match query_contains {
                    Some("hotel quieter nightlife") | Some("hotel") => vec![json!({
                        "fact_id": "fact:generic-note",
                        "fact_type": "note",
                        "content": "I need a hotel that can host our annual conference during the trip.",
                        "quote": "I need a hotel that can host our annual conference during the trip.",
                        "source_episode": "episode:generic-note",
                        "t_valid": "2026-02-12T10:00:00Z",
                        "t_ingested": "2026-02-12T10:00:00Z",
                        "scope": "org",
                        "confidence": 0.8,
                        "ft_score": 12.0,
                        "index_keys": [],
                        "entity_links": [],
                        "policy_tags": [],
                        "provenance": {"source_episode": "episode:generic-note"}
                    })],
                    _ => vec![],
                })
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![json!({
                    "fact_id": "fact:experience",
                    "fact_type": "experience",
                    "content": "I usually prefer quieter hotels away from the city center, because I avoid nightlife-heavy properties.",
                    "quote": "I usually prefer quieter hotels away from the city center, because I avoid nightlife-heavy properties.",
                    "source_episode": "episode:experience",
                    "t_valid": "2026-02-13T10:00:00Z",
                    "t_ingested": "2026-02-13T10:00:00Z",
                    "scope": "org",
                    "confidence": 0.9,
                    "ft_score": 0.0,
                    "index_keys": ["hotel", "quiet", "nightlife"],
                    "entity_links": [],
                    "policy_tags": [],
                    "provenance": {"source_episode": "episode:experience"}
                })])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let service = crate::service::MemoryService::new(
            Arc::new(ExperiencePrimaryRankingDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &service,
            crate::models::AssembleContextRequest {
                query: "Which hotel is better if I want somewhere quieter away from nightlife?"
                    .to_string(),
                scope: "org".to_string(),
                as_of: Some(
                    chrono::DateTime::parse_from_rfc3339("2026-02-14T10:00:00Z")
                        .expect("timestamp")
                        .with_timezone(&Utc),
                ),
                budget: 1,
                project: None,
                fact_types: vec![],
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            },
        )
        .await
        .expect("assemble context should succeed");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].fact_id, "fact:experience");
    }

    #[tokio::test]
    async fn assemble_context_uses_repeated_direct_topics_to_surface_implicit_preferences() {
        struct ImplicitExperienceTopicDbClient;

        #[async_trait]
        impl DbClient for ImplicitExperienceTopicDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(match query_contains {
                    Some("venue work best my conference") => vec![
                        json!({
                            "fact_id": "fact:conference-hotel",
                            "fact_type": "note",
                            "content": "I'm heading to a conference next month and need to book a hotel.",
                            "quote": "I'm heading to a conference next month and need to book a hotel.",
                            "source_episode": "episode:conference-hotel",
                            "t_valid": "2026-02-12T10:00:00Z",
                            "t_ingested": "2026-02-12T10:00:00Z",
                            "scope": "org",
                            "confidence": 0.8,
                            "ft_score": 8.0,
                            "index_keys": [],
                            "entity_links": [],
                            "policy_tags": [],
                            "provenance": {"source_episode": "episode:conference-hotel"}
                        }),
                        json!({
                            "fact_id": "fact:conference-hotel-shape",
                            "fact_type": "note",
                            "content": "For the conference, I want a hotel that is not too tall.",
                            "quote": "For the conference, I want a hotel that is not too tall.",
                            "source_episode": "episode:conference-hotel-shape",
                            "t_valid": "2026-02-11T10:00:00Z",
                            "t_ingested": "2026-02-11T10:00:00Z",
                            "scope": "org",
                            "confidence": 0.8,
                            "ft_score": 7.0,
                            "index_keys": [],
                            "entity_links": [],
                            "policy_tags": [],
                            "provenance": {"source_episode": "episode:conference-hotel-shape"}
                        }),
                    ],
                    Some("conference") => vec![
                        json!({
                            "fact_id": "fact:conference-hotel",
                            "fact_type": "note",
                            "content": "I'm heading to a conference next month and need to book a hotel.",
                            "quote": "I'm heading to a conference next month and need to book a hotel.",
                            "source_episode": "episode:conference-hotel",
                            "t_valid": "2026-02-12T10:00:00Z",
                            "t_ingested": "2026-02-12T10:00:00Z",
                            "scope": "org",
                            "confidence": 0.8,
                            "ft_score": 8.0,
                            "index_keys": [],
                            "entity_links": [],
                            "policy_tags": [],
                            "provenance": {"source_episode": "episode:conference-hotel"}
                        }),
                        json!({
                            "fact_id": "fact:conference-hotel-shape",
                            "fact_type": "note",
                            "content": "For the conference, I want a hotel that is not too tall.",
                            "quote": "For the conference, I want a hotel that is not too tall.",
                            "source_episode": "episode:conference-hotel-shape",
                            "t_valid": "2026-02-11T10:00:00Z",
                            "t_ingested": "2026-02-11T10:00:00Z",
                            "scope": "org",
                            "confidence": 0.8,
                            "ft_score": 7.0,
                            "index_keys": [],
                            "entity_links": [],
                            "policy_tags": [],
                            "provenance": {"source_episode": "episode:conference-hotel-shape"}
                        }),
                    ],
                    _ => vec![],
                })
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![json!({
                    "fact_id": "fact:experience",
                    "fact_type": "experience",
                    "content": "I usually prefer quieter hotels away from the city center, because I avoid nightlife-heavy properties.",
                    "quote": "I usually prefer quieter hotels away from the city center, because I avoid nightlife-heavy properties.",
                    "source_episode": "episode:experience",
                    "t_valid": "2026-02-13T10:00:00Z",
                    "t_ingested": "2026-02-13T10:00:00Z",
                    "scope": "org",
                    "confidence": 0.9,
                    "ft_score": 0.0,
                    "index_keys": ["hotel", "quiet", "nightlife"],
                    "entity_links": [],
                    "policy_tags": [],
                    "provenance": {"source_episode": "episode:experience"}
                })])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let service = crate::service::MemoryService::new(
            Arc::new(ImplicitExperienceTopicDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &service,
            crate::models::AssembleContextRequest {
                query: "Which venue would work best for my conference?".to_string(),
                scope: "org".to_string(),
                as_of: Some(
                    chrono::DateTime::parse_from_rfc3339("2026-02-14T10:00:00Z")
                        .expect("timestamp")
                        .with_timezone(&Utc),
                ),
                budget: 1,
                project: None,
                fact_types: vec![],
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            },
        )
        .await
        .expect("assemble context should succeed");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].fact_id, "fact:experience");
    }

    #[test]
    fn lexical_candidate_limit_preserves_preexpanded_limits() {
        assert_eq!(lexical_candidate_limit(5), 25);
        assert_eq!(lexical_candidate_limit(50), 50);
        assert_eq!(lexical_candidate_limit(200), 200);
    }
}
