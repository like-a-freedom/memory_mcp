//! Context assembly operations.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use serde_json::{Value, json};

use super::cache::{CacheKey, CacheView};
use super::embedding::{cosine_similarity, embedding_from_value};
use super::error::{MemoryError, error_messages};
use super::value_helpers::json_string;
use crate::logging::LogLevel;
use crate::models::{AccessContext, AssembleContextRequest, AssembledContextItem};

mod alias_expansion;
mod community;
mod experience;
mod filtering;
mod lexical;
mod logging;
mod ranking;
mod temporal;

use alias_expansion::expand_query_with_aliases;
use community::{CollectCommunityFactsRequest, collect_community_facts};
use experience::{RecentExperienceRequest, append_recent_experience_items};
use filtering::{
    compare_facts_by_recency, episode_record_allowed, fact_is_active_at, fact_record_allowed,
    filter_episodes_by_constraints, filter_facts_by_constraints, raw_object,
};
use lexical::{
    FactFilterParams, FactQueryParams, select_episode_records_for_query,
    select_fact_records_for_query,
};
use logging::{summarize_retrieval_tiers, supplemental_experience_count};
use ranking::{
    RetrievalTier, apply_time_window, build_ranked_context_facts,
    default_episode_fallback_rationale, select_ranked_context_facts,
    sort_ranked_context_facts_for_timeline,
};
use temporal::{CollectTemporalFactsRequest, collect_temporal_facts, infer_temporal_window};

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
        for item in &cached {
            if let Err(err) = service.record_fact_access(&item.fact_id, 1).await {
                service.logger.log(
                    super::log_event(
                        "assemble_context.access_track_error",
                        json!({"fact_id": item.fact_id}),
                        json!({"error": err.to_string()}),
                        Some(&access),
                    ),
                    LogLevel::Warn,
                );
            }
        }

        service.logger.log(
            super::log_event(
                "assemble_context.cache_hit",
                json!({"scope": request.scope, "query": request.query}),
                json!({"count": cached.len()}),
                Some(&access),
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
            FactFilterParams {
                namespace: &namespace,
                scope: &request.scope,
                cutoff,
                project: project_opt,
                fact_types: &fact_types,
                access: &access,
            },
            request.budget,
        )
        .await?
    } else if requested_view_mode == Some("map") {
        build_map_view(service, &namespace, cutoff, request.budget).await?
    } else {
        let lexical_result = select_fact_records_for_query(
            service,
            FactQueryParams {
                namespace: &namespace,
                scope: &request.scope,
                cutoff_iso: &cutoff_iso,
                query_opt,
                limit: request.budget,
                project: project_opt,
                fact_types: &fact_types,
            },
        )
        .await?;

        let direct_retrieval_tier = lexical_result.retrieval_tier;
        let direct_facts =
            filter_facts_by_constraints(lexical_result.records, &access, project_opt, &fact_types);

        // Alias expansion: search for additional facts using entity aliases
        let mut expanded_facts = Vec::new();
        let mut ranked_facts = if let Some(query) = query_opt {
            let temporal_facts = collect_temporal_facts(
                service,
                CollectTemporalFactsRequest {
                    namespace: &namespace,
                    scope: &request.scope,
                    cutoff_iso: &cutoff_iso,
                    cutoff,
                    query,
                    access: &access,
                    project: project_opt,
                    fact_types: &fact_types,
                    budget: request.budget,
                },
            )
            .await?;

            let expanded_queries = expand_query_with_aliases(service, query, &namespace).await;
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
                        namespace: &namespace,
                        scope: &request.scope,
                        cutoff_iso: &cutoff_iso,
                        query_opt: Some(expanded_query),
                        limit: request.budget,
                        project: project_opt,
                        fact_types: &fact_types,
                    },
                )
                .await?;
                for fact in filter_facts_by_constraints(
                    extra_records.records,
                    &access,
                    project_opt,
                    &fact_types,
                ) {
                    if !direct_fact_ids.contains(&fact.fact_id) {
                        expanded_facts.push(fact);
                    }
                }
            }
            let all_direct_ids: HashSet<_> = direct_facts
                .iter()
                .chain(temporal_facts.iter())
                .chain(expanded_facts.iter())
                .map(|fact| fact.fact_id.clone())
                .collect();

            let community_facts = collect_community_facts(
                service,
                CollectCommunityFactsRequest {
                    namespace: &namespace,
                    scope: &request.scope,
                    cutoff_iso: &cutoff_iso,
                    query,
                    access: &access,
                    project: project_opt,
                    fact_types: &fact_types,
                    direct_fact_ids: &all_direct_ids,
                    budget: request.budget,
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
                    namespace: &namespace,
                    scope: &request.scope,
                    cutoff,
                    query,
                    access: &access,
                    project: project_opt,
                    fact_types: &fact_types,
                    excluded_fact_ids: &excluded_fact_ids,
                    budget: request.budget,
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
                query_opt,
                &request.scope,
                cutoff,
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
                query_opt,
                &request.scope,
                cutoff,
                super::decayed_confidence,
            )
        };

        if ranked_facts.is_empty() && query_opt.is_some() {
            let episode_records = select_episode_records_for_query(
                service,
                &namespace,
                &request.scope,
                &cutoff_iso,
                query_opt,
                request.budget,
                project_opt,
            )
            .await?;
            build_episode_fallback_items(EpisodeFallbackParams {
                episodes: filter_episodes_by_constraints(episode_records, &access, project_opt),
                query_opt,
                scope: &request.scope,
                cutoff,
                window_start: request.window_start,
                window_end: request.window_end,
                timeline_mode: requested_view_mode == Some("timeline"),
                budget: request.budget,
            })
        } else {
            apply_time_window(&mut ranked_facts, request.window_start, request.window_end);
            if requested_view_mode == Some("timeline") {
                sort_ranked_context_facts_for_timeline(&mut ranked_facts);
                ranked_facts
                    .into_iter()
                    .take(request.budget.max(1) as usize)
                    .map(|ranked| {
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
                    })
                    .collect()
            } else {
                let temporal_focus =
                    query_opt.and_then(|query| infer_temporal_window(query, cutoff));
                let selected_ranked = select_ranked_context_facts(
                    ranked_facts,
                    request.budget.max(1) as usize,
                    temporal_focus,
                    query_opt
                        .map(super::query::search_query_terms)
                        .unwrap_or_default(),
                );
                selected_ranked
                    .into_iter()
                    .map(|ranked| {
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
                    })
                    .collect()
            }
        }
    };

    if requested_view_mode != Some("facets")
        && requested_view_mode != Some("wake_up")
        && requested_view_mode != Some("map")
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
        ),
        LogLevel::Trace,
    );

    for item in &results {
        if let Err(err) = service.record_fact_access(&item.fact_id, 1).await {
            service.logger.log(
                super::log_event(
                    "assemble_context.access_track_error",
                    json!({"fact_id": item.fact_id}),
                    json!({"error": err.to_string()}),
                    Some(&access),
                ),
                LogLevel::Warn,
            );
        }
    }

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
        ),
        LogLevel::Trace,
    );

    let latency_ms = started_at.elapsed().as_secs_f64() * 1000.0;
    logging::maybe_record_query_log(service, &request, &results, false, latency_ms, &access).await;

    Ok(results)
}

/// Parameters for building context items from episode fallback records.
struct EpisodeFallbackParams<'a> {
    episodes: Vec<crate::models::Episode>,
    query_opt: Option<&'a str>,
    scope: &'a str,
    cutoff: chrono::DateTime<chrono::Utc>,
    window_start: Option<chrono::DateTime<chrono::Utc>>,
    window_end: Option<chrono::DateTime<chrono::Utc>>,
    timeline_mode: bool,
    budget: i32,
}

fn build_episode_fallback_items(params: EpisodeFallbackParams<'_>) -> Vec<AssembledContextItem> {
    let mut episodes = params.episodes;
    apply_episode_time_window(&mut episodes, params.window_start, params.window_end);

    if params.timeline_mode {
        episodes.sort_by(|left, right| {
            left.t_ref
                .cmp(&right.t_ref)
                .then_with(|| left.episode_id.cmp(&right.episode_id))
        });
    } else {
        episodes.sort_by(|left, right| {
            right
                .t_ref
                .cmp(&left.t_ref)
                .then_with(|| left.episode_id.cmp(&right.episode_id))
        });
    }

    episodes
        .into_iter()
        .take(params.budget.max(1) as usize)
        .map(|episode| AssembledContextItem {
            fact_id: format!("episode_fallback:{}", episode.episode_id),
            content: episode.content.clone(),
            quote: episode.content.clone(),
            source_episode: episode.episode_id.clone(),
            confidence: 1.0,
            provenance: json!({
                "source_episode": episode.episode_id,
                "source_type": episode.source_type,
                "source_id": episode.source_id,
                "episode_fallback": true,
            }),
            rationale: default_episode_fallback_rationale(
                params.query_opt,
                params.scope,
                params.cutoff,
            ),
            retrieval_tier: Some(RetrievalTier::EpisodeFallback.as_str().to_string()),
        })
        .collect()
}

fn apply_episode_time_window(
    episodes: &mut Vec<crate::models::Episode>,
    window_start: Option<chrono::DateTime<chrono::Utc>>,
    window_end: Option<chrono::DateTime<chrono::Utc>>,
) {
    if window_start.is_none() && window_end.is_none() {
        return;
    }

    episodes.retain(|episode| {
        let after_start = window_start.is_none_or(|start| episode.t_ref >= start);
        let before_end = window_end.is_none_or(|end| episode.t_ref <= end);
        after_start && before_end
    });
}

async fn build_facets_view(
    service: &crate::service::MemoryService,
    namespace: &str,
    scope: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
    project: Option<&str>,
    budget: i32,
    access: &AccessContext,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let records = service
        .db_client
        .select_table("episode", namespace)
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    let mut buckets = HashMap::<String, (usize, chrono::DateTime<chrono::Utc>)>::new();

    for record in records {
        let Some(map) = raw_object(&record) else {
            continue;
        };
        let Some(episode) = super::episode::episode_from_record(map) else {
            continue;
        };
        if episode.scope != scope
            || episode.t_ref > cutoff
            || episode.t_ingested > cutoff
            || !episode_record_allowed(&record, access, project)
        {
            continue;
        }

        let label = map
            .get("project")
            .and_then(json_string)
            .filter(|value| !value.trim().is_empty())
            .map(ToString::to_string)
            .or_else(|| episode.policy_tags.first().cloned())
            .unwrap_or_else(|| scope.to_string());

        buckets
            .entry(label)
            .and_modify(|(count, latest)| {
                *count += 1;
                *latest = (*latest).max(episode.t_ingested);
            })
            .or_insert((1, episode.t_ingested));
    }

    let mut buckets = buckets.into_iter().collect::<Vec<_>>();
    buckets.sort_by(
        |(left_label, (_, left_latest)), (right_label, (_, right_latest))| {
            right_latest
                .cmp(left_latest)
                .then_with(|| left_label.cmp(right_label))
        },
    );

    let items = buckets
        .into_iter()
        .take(budget.max(1) as usize)
        .map(|(label, (count, latest))| AssembledContextItem {
            fact_id: format!("facet:{label}"),
            content: label.clone(),
            quote: format!("{count} episodes"),
            source_episode: format!("facet:{label}"),
            confidence: 1.0,
            provenance: json!({
                "facet": label,
                "count": count,
                "max_t_ingested": super::normalize_dt(latest),
            }),
            rationale: "view_mode=facets grouped episodes by project/policy/scope".to_string(),
            retrieval_tier: None,
        })
        .collect::<Vec<_>>();

    service.logger.log(
        super::log_event(
            "assemble_context.facets_view",
            json!({"scope": scope, "project": project}),
            json!({"count": items.len()}),
            Some(access),
        ),
        LogLevel::Debug,
    );

    Ok(items)
}

async fn build_wake_up_view(
    service: &crate::service::MemoryService,
    params: FactFilterParams<'_>,
    budget: i32,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let records = service
        .db_client
        .select_table("fact", params.namespace)
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    let mut facts =
        filter_facts_by_constraints(records, params.access, params.project, params.fact_types)
            .into_iter()
            .filter(|fact| fact.scope == params.scope)
            .filter(|fact| fact_is_active_at(fact, params.cutoff))
            .collect::<Vec<_>>();

    facts.sort_by(|left, right| {
        let left_persona = left.policy_tags.iter().any(|tag| tag == "persona");
        let right_persona = right.policy_tags.iter().any(|tag| tag == "persona");
        right_persona
            .cmp(&left_persona)
            .then_with(|| right.t_ingested.cmp(&left.t_ingested))
            .then_with(|| right.t_valid.cmp(&left.t_valid))
            .then_with(|| left.fact_id.cmp(&right.fact_id))
    });

    let persona_count = facts
        .iter()
        .filter(|fact| fact.policy_tags.iter().any(|tag| tag == "persona"))
        .count();

    let items = facts
        .into_iter()
        .take(budget.max(1) as usize)
        .map(|fact| {
            let persona = fact.policy_tags.iter().any(|tag| tag == "persona");
            let confidence = if persona {
                fact.confidence
                    .max(super::decayed_confidence(&fact, params.cutoff))
            } else {
                super::decayed_confidence(&fact, params.cutoff)
            };
            AssembledContextItem {
                fact_id: fact.fact_id,
                content: fact.content,
                quote: fact.quote,
                source_episode: fact.source_episode,
                confidence,
                provenance: fact.provenance,
                rationale: format!(
                    "view_mode=wake_up persona={} recent_t_ingested={}",
                    persona,
                    super::normalize_dt(fact.t_ingested)
                ),
                retrieval_tier: None,
            }
        })
        .collect::<Vec<_>>();

    service.logger.log(
        super::log_event(
            "assemble_context.wake_up_view",
            json!({"scope": params.scope, "project": params.project, "fact_type_count": params.fact_types.len()}),
            json!({"count": items.len(), "persona_count": persona_count}),
            Some(params.access),
        ),
        LogLevel::Debug,
    );

    Ok(items)
}

async fn build_map_view(
    service: &crate::service::MemoryService,
    namespace: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
    budget: i32,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let hub_entities =
        super::apps::graph::find_hub_entities(service, namespace, cutoff, budget).await?;
    let communities =
        super::apps::graph::list_communities(service, namespace, cutoff, budget).await?;

    service.logger.log(
        super::log_event(
            "assemble_context.map_view",
            json!({"namespace": namespace, "budget": budget}),
            json!({"hub_entities": hub_entities.len(), "communities": communities.len()}),
            None,
        ),
        LogLevel::Debug,
    );

    let mut items = Vec::with_capacity(hub_entities.len() + communities.len());

    for hub in hub_entities {
        items.push(AssembledContextItem {
            fact_id: format!("map:hub:{}", hub.entity_id),
            content: hub.canonical_name.clone(),
            quote: format!("{} connections", hub.degree),
            source_episode: hub.entity_id.clone(),
            confidence: 1.0,
            provenance: json!({
                "kind": "hub_entity",
                "entity_id": hub.entity_id,
                "canonical_name": hub.canonical_name,
                "degree": hub.degree,
            }),
            rationale: "view_mode=map ranked hub entities by active graph degree".to_string(),
            retrieval_tier: None,
        });
    }

    for community in communities {
        let member_count = community.member_entities.len();
        items.push(AssembledContextItem {
            fact_id: format!("map:community:{}", community.community_id),
            content: community.summary.clone(),
            quote: format!("{member_count} members"),
            source_episode: community.community_id.clone(),
            confidence: 1.0,
            provenance: json!({
                "kind": "community",
                "community_id": community.community_id,
                "member_entities": community.member_entities,
                "member_count": member_count,
                "updated_at": community.updated_at.map(super::normalize_dt),
            }),
            rationale: "view_mode=map listed active communities from the graph index".to_string(),
            retrieval_tier: None,
        });
    }

    items.truncate(budget.max(1) as usize);
    Ok(items)
}

/// Test-only convenience wrapper around the production comparator below.
///
/// Production code uses `compare_facts_by_recency` directly in composite sorts,
/// while tests keep this helper to assert the standalone ordering contract.
#[cfg(test)]
fn sort_facts_by_recency(facts: &mut [crate::models::Fact]) {
    facts.sort_by(filtering::compare_facts_by_recency);
}

#[derive(Debug)]

struct CollectSemanticFactsRequest<'a> {
    namespace: &'a str,
    scope: &'a str,
    cutoff: chrono::DateTime<chrono::Utc>,
    query: &'a str,
    access: &'a AccessContext,
    project: Option<&'a str>,
    fact_types: &'a [String],
    excluded_fact_ids: &'a HashSet<String>,
    budget: i32,
}

async fn collect_semantic_facts(
    service: &crate::service::MemoryService,
    request: CollectSemanticFactsRequest<'_>,
) -> Result<Vec<(crate::models::Fact, String)>, MemoryError> {
    let query_embedding = match service.generate_embedding(request.query).await {
        Ok(Some(embedding)) => embedding,
        Ok(None) => return Ok(Vec::new()),
        Err(err) => {
            service.logger.log(
                std::collections::HashMap::from([
                    ("op".to_string(), json!("embedding.query_skipped")),
                    (
                        "provider".to_string(),
                        json!(service.embedding_provider.provider_name()),
                    ),
                    ("error".to_string(), json!(err.to_string())),
                ]),
                LogLevel::Warn,
            );
            return Ok(Vec::new());
        }
    };

    if query_embedding.is_empty() {
        return Ok(Vec::new());
    }

    // Request more candidates than budget since HNSW results may be filtered
    // by temporal/scope constraints post-search
    let search_limit = request.budget.max(1) * 4;

    let fact_records = service
        .db_client
        .select_facts_ann(
            request.namespace,
            request.scope,
            &super::normalize_dt(request.cutoff),
            &query_embedding,
            search_limit,
        )
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    let mut ranked_facts = Vec::new();
    for record in fact_records {
        if !fact_record_allowed(&record, request.access, request.project, request.fact_types) {
            continue;
        }

        let Some(fact) = super::episode::fact_from_record(&record) else {
            continue;
        };

        if fact.scope != request.scope
            || request.excluded_fact_ids.contains(&fact.fact_id)
            || !fact_is_active_at(&fact, request.cutoff)
        {
            continue;
        }

        // Use DB-computed sem_score if available, otherwise compute in Rust
        let similarity = record
            .as_object()
            .and_then(|map: &serde_json::Map<String, Value>| map.get("sem_score"))
            .and_then(|v| v.as_f64())
            .unwrap_or_else(|| {
                let embedding = record
                    .as_object()
                    .and_then(|map: &serde_json::Map<String, Value>| map.get("embedding"))
                    .and_then(embedding_from_value);
                match embedding {
                    Some(ref emb) if emb.len() == query_embedding.len() => {
                        cosine_similarity(&query_embedding, emb)
                    }
                    _ => 0.0,
                }
            });

        if similarity < service.embedding_similarity_threshold {
            continue;
        }

        ranked_facts.push((similarity, fact));
    }

    ranked_facts.sort_by(
        |(left_similarity, left_fact), (right_similarity, right_fact)| {
            right_similarity
                .total_cmp(left_similarity)
                .then_with(|| compare_facts_by_recency(left_fact, right_fact))
        },
    );

    Ok(ranked_facts
        .into_iter()
        .take(request.budget.max(1) as usize)
        .map(|(similarity, fact)| {
            (
                fact,
                format!(
                    "matched semantic similarity={similarity:.3} for query=\"{}\"",
                    request.query
                ),
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::alias_expansion::expand_query_with_aliases_for_test;
    use super::community::stored_community_summary_from_value;
    use super::filtering::filter_facts_by_policy;
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
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

        let inferred = create_test_fact("fact:inferred", cutoff - chrono::Duration::days(1));
        let extracted = create_test_fact("fact:extracted", cutoff - chrono::Duration::days(1));

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
                {
                    let mut fact = create_ranked_test_fact(
                        "fact:digest-a",
                        "episode:digest-a",
                        chrono::DateTime::parse_from_rfc3339("2026-04-07T09:00:00Z")
                            .expect("digest a time")
                            .with_timezone(&Utc),
                        12.0,
                        11.0,
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
                        11.8,
                        10.8,
                        0,
                        &[],
                    );
                    fact.fact.content = "Another quarterly digest for Atlas and Beacon repeated blockers and decisions keywords across March and April 2026 without resolving any specific item.".to_string();
                    fact.fact.quote = fact.fact.content.clone();
                    fact
                },
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
                        "March 2026 Beacon decision: finance approved the revised launch budget."
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

    #[test]
    fn lexical_candidate_limit_preserves_preexpanded_limits() {
        assert_eq!(lexical_candidate_limit(5), 25);
        assert_eq!(lexical_candidate_limit(50), 50);
        assert_eq!(lexical_candidate_limit(200), 200);
    }
}
