//! Context assembly operations.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

use serde_json::{Value, json};

use super::cache::{CacheKey, CacheView};
use super::embedding::{cosine_similarity, embedding_from_value};
use super::error::MemoryError;
use crate::logging::LogLevel;
use crate::models::{
    AccessContext, AssembleContextRequest, AssembledContextItem, FACT_TYPE_EXPERIENCE,
};
use crate::storage::GraphDirection;
use crate::storage::{json_f64, json_string};

const RECIPROCAL_RANK_FUSION_K: f64 = 60.0;
const MAX_ITEMS_PER_SOURCE_EPISODE: usize = 2;
const ACCESS_COUNT_NOVELTY_WEIGHT: f64 = 0.08;
const MMR_RELEVANCE_WEIGHT: f64 = 0.80;
const REDUNDANCY_INDEX_KEY_WEIGHT: f64 = 0.70;
const REDUNDANCY_TEMPORAL_WEIGHT: f64 = 0.30;
const TEMPORAL_SIMILARITY_WINDOW_DAYS: f64 = 14.0;
const TEMPORAL_ALIGNMENT_WINDOW_DAYS: f64 = 30.0;
const MIN_TEMPORAL_ALIGNMENT_TO_FILL_BUDGET: f64 = 0.50;
const MIN_RANKED_CONFIDENCE: f64 = 0.01;

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
        return Err(MemoryError::Validation("scope is required".into()));
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
        maybe_record_query_log(service, &request, &cached, true, latency_ms, &access).await;
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
            &namespace,
            &request.scope,
            cutoff,
            project_opt,
            &fact_types,
            request.budget,
            &access,
        )
        .await?
    } else if requested_view_mode == Some("map") {
        build_map_view(service, &namespace, cutoff, request.budget).await?
    } else {
        let lexical_result = select_fact_records_for_query(
            service,
            &namespace,
            &request.scope,
            &cutoff_iso,
            query_opt,
            request.budget,
            project_opt,
            &fact_types,
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
                    &namespace,
                    &request.scope,
                    &cutoff_iso,
                    Some(expanded_query),
                    request.budget,
                    project_opt,
                    &fact_types,
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
            build_episode_fallback_items(
                filter_episodes_by_constraints(episode_records, &access, project_opt),
                query_opt,
                &request.scope,
                cutoff,
                request.window_start,
                request.window_end,
                requested_view_mode == Some("timeline"),
                request.budget,
            )
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
    maybe_record_query_log(service, &request, &results, false, latency_ms, &access).await;

    Ok(results)
}

async fn record_query_log(
    service: &crate::service::MemoryService,
    request: &AssembleContextRequest,
    results: &[AssembledContextItem],
    cache_hit: bool,
    latency_ms: f64,
) -> Result<(), MemoryError> {
    let namespace = service.namespace_for_scope(&request.scope);
    let logged_at = super::now();
    let project = request
        .project
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let view_mode = request
        .view_mode
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let retrieval_tier = results
        .iter()
        .filter_map(|item| item.retrieval_tier.as_deref())
        .map(str::trim)
        .find(|value| !value.is_empty());

    let record_id = format!(
        "query_log:{}",
        super::hash_prefix(&format!(
            "{}|{}|{}|{}|{}|{}|{}|{}",
            super::normalize_text(&request.scope),
            super::normalize_text(&request.query),
            super::normalize_text(project.unwrap_or_default()),
            super::normalize_text(view_mode.unwrap_or_default()),
            super::normalize_text(retrieval_tier.unwrap_or_default()),
            results.len(),
            if cache_hit { "1" } else { "0" },
            super::normalize_dt(logged_at),
        ))
    );

    let mut payload = serde_json::Map::from_iter([
        ("query_log_id".to_string(), json!(record_id.clone())),
        (
            "logged_at".to_string(),
            json!(super::normalize_dt(logged_at)),
        ),
        ("scope".to_string(), json!(request.scope.clone())),
        ("query".to_string(), json!(request.query.clone())),
        ("result_count".to_string(), json!(results.len() as i64)),
        ("latency_ms".to_string(), json!(latency_ms)),
        ("cache_hit".to_string(), json!(cache_hit)),
    ]);

    if let Some(project) = project {
        payload.insert("project".to_string(), json!(project));
    }
    if let Some(view_mode) = view_mode {
        payload.insert("view_mode".to_string(), json!(view_mode));
    }
    if let Some(retrieval_tier) = retrieval_tier {
        payload.insert("retrieval_tier".to_string(), json!(retrieval_tier));
    }

    service
        .db_client
        .create(&record_id, Value::Object(payload), &namespace)
        .await?;

    Ok(())
}

async fn maybe_record_query_log(
    service: &crate::service::MemoryService,
    request: &AssembleContextRequest,
    results: &[AssembledContextItem],
    cache_hit: bool,
    latency_ms: f64,
    access: &AccessContext,
) {
    if !service.is_query_logging_enabled() {
        service.logger.log(
            super::log_event(
                "assemble_context.query_log_skipped",
                json!({
                    "scope": request.scope,
                    "query": request.query,
                    "cache_hit": cache_hit,
                }),
                json!({"reason": "disabled"}),
                Some(access),
            ),
            LogLevel::Trace,
        );
        return;
    }

    match record_query_log(service, request, results, cache_hit, latency_ms).await {
        Ok(()) => {
            service.logger.log(
                super::log_event(
                    "assemble_context.query_log_recorded",
                    json!({
                        "scope": request.scope,
                        "query": request.query,
                        "cache_hit": cache_hit,
                    }),
                    json!({
                        "result_count": results.len(),
                        "latency_ms": latency_ms,
                        "retrieval_tier": primary_retrieval_tier(results),
                    }),
                    Some(access),
                ),
                LogLevel::Debug,
            );

            match prune_expired_query_logs(service, &request.scope).await {
                Ok(pruned_count) if pruned_count > 0 => {
                    service.logger.log(
                        super::log_event(
                            "assemble_context.query_log_pruned",
                            json!({
                                "scope": request.scope,
                                "retention_days": service.query_log_retention_days(),
                            }),
                            json!({"count": pruned_count}),
                            Some(access),
                        ),
                        LogLevel::Trace,
                    );
                }
                Ok(_) => {}
                Err(err) => {
                    service.logger.log(
                        super::log_event(
                            "assemble_context.query_log_prune_error",
                            json!({
                                "scope": request.scope,
                                "retention_days": service.query_log_retention_days(),
                            }),
                            json!({"error": err.to_string()}),
                            Some(access),
                        ),
                        LogLevel::Warn,
                    );
                }
            }
        }
        Err(err) => {
            service.logger.log(
                super::log_event(
                    "assemble_context.query_log_error",
                    json!({
                        "scope": request.scope,
                        "query": request.query,
                        "cache_hit": cache_hit,
                    }),
                    json!({"error": err.to_string()}),
                    Some(access),
                ),
                LogLevel::Warn,
            );
        }
    }
}

async fn prune_expired_query_logs(
    service: &crate::service::MemoryService,
    scope: &str,
) -> Result<usize, MemoryError> {
    let namespace = service.namespace_for_scope(scope);
    let cutoff =
        super::now() - chrono::Duration::days(i64::from(service.query_log_retention_days()));
    let deleted = service
        .db_client
        .query(
            "DELETE query_log WHERE logged_at IS NOT NONE AND type::datetime(logged_at) < type::datetime($cutoff) RETURN BEFORE",
            Some(json!({"cutoff": super::normalize_dt(cutoff)})),
            &namespace,
        )
        .await?;

    Ok(deleted.as_array().map_or(0, std::vec::Vec::len))
}

fn primary_retrieval_tier(results: &[AssembledContextItem]) -> Option<&str> {
    results
        .iter()
        .filter_map(|item| item.retrieval_tier.as_deref())
        .map(str::trim)
        .find(|value| !value.is_empty())
}

fn summarize_retrieval_tiers(results: &[AssembledContextItem]) -> Value {
    let mut counts = serde_json::Map::new();

    for tier in results
        .iter()
        .filter_map(|item| item.retrieval_tier.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let next = counts
            .get(tier)
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .saturating_add(1);
        counts.insert(tier.to_string(), json!(next));
    }

    Value::Object(counts)
}

fn supplemental_experience_count(results: &[AssembledContextItem]) -> usize {
    results
        .iter()
        .filter(|item| item.rationale.starts_with("supplemental experience "))
        .count()
}

struct RecentExperienceRequest<'a> {
    namespace: &'a str,
    scope: &'a str,
    cutoff: chrono::DateTime<chrono::Utc>,
    project: Option<&'a str>,
    access: &'a AccessContext,
    budget: i32,
    fact_types: &'a [String],
}

async fn append_recent_experience_items(
    results: &mut Vec<AssembledContextItem>,
    service: &crate::service::MemoryService,
    request: RecentExperienceRequest<'_>,
) -> Result<usize, MemoryError> {
    let budget = request.budget.max(1) as usize;
    if results.len() >= budget {
        return Ok(0);
    }

    if !request.fact_types.is_empty()
        && !request
            .fact_types
            .iter()
            .any(|fact_type| fact_type == FACT_TYPE_EXPERIENCE)
    {
        return Ok(0);
    }

    let records = service
        .db_client
        .select_active_facts(request.namespace, 500)
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;
    let experience_filter = vec![FACT_TYPE_EXPERIENCE.to_string()];
    let mut facts =
        filter_facts_by_constraints(records, request.access, request.project, &experience_filter)
            .into_iter()
            .filter(|fact| fact.scope == request.scope)
            .filter(|fact| fact_is_active_at(fact, request.cutoff))
            .collect::<Vec<_>>();

    facts.sort_by(|left, right| {
        right
            .t_ingested
            .cmp(&left.t_ingested)
            .then_with(|| compare_facts_by_recency(left, right))
    });

    let mut seen_fact_ids = results
        .iter()
        .map(|item| item.fact_id.clone())
        .collect::<HashSet<_>>();
    let mut appended = 0;

    for fact in facts {
        if results.len() >= budget || !seen_fact_ids.insert(fact.fact_id.clone()) {
            continue;
        }

        let confidence = super::decayed_confidence(&fact, request.cutoff);

        results.push(AssembledContextItem {
            fact_id: fact.fact_id,
            content: fact.content,
            quote: fact.quote,
            source_episode: fact.source_episode,
            confidence,
            provenance: fact.provenance,
            rationale: format!(
                "supplemental experience recent_t_ingested={}",
                super::normalize_dt(fact.t_ingested)
            ),
            retrieval_tier: None,
        });
        appended += 1;
    }

    Ok(appended)
}

/// Filter facts by access policy and request-level constraints.
fn filter_facts_by_constraints(
    records: Vec<Value>,
    access: &AccessContext,
    project: Option<&str>,
    fact_types: &[String],
) -> Vec<crate::models::Fact> {
    let mut facts = Vec::new();

    for record in records {
        let items: Vec<&Value> = if let Some(arr) = record.get("Array").and_then(|v| v.as_array()) {
            arr.iter().collect()
        } else {
            vec![&record]
        };

        for item in items {
            let fact_item = if let Some(obj) = item.get("Object") {
                obj
            } else {
                item
            };

            if !fact_record_allowed(fact_item, access, project, fact_types) {
                continue;
            }

            if let Some(fact) = super::episode::fact_from_record(fact_item) {
                facts.push(fact);
            }
        }
    }

    facts
}

#[cfg(test)]
fn filter_facts_by_policy(records: Vec<Value>, access: &AccessContext) -> Vec<crate::models::Fact> {
    filter_facts_by_constraints(records, access, None, &[])
}

fn fact_record_allowed(
    record: &Value,
    access: &AccessContext,
    project: Option<&str>,
    fact_types: &[String],
) -> bool {
    fact_record_matches_project(record, project)
        && fact_record_matches_type(record, fact_types)
        && fact_record_allowed_by_policy(record, access)
}

fn fact_record_allowed_by_policy(record: &Value, access: &AccessContext) -> bool {
    let Some(tags) = raw_object(record)
        .and_then(|map| map.get("policy_tags"))
        .and_then(raw_array)
        .map(|values| values.iter().filter_map(json_string).collect::<Vec<_>>())
    else {
        return true;
    };

    if tags.is_empty() {
        return true;
    }

    let Some(allowed_tags) = &access.allowed_tags else {
        return true;
    };

    let allowed = allowed_tags
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    tags.iter().any(|tag| allowed.contains(tag))
}

fn filter_episodes_by_constraints(
    records: Vec<Value>,
    access: &AccessContext,
    project: Option<&str>,
) -> Vec<crate::models::Episode> {
    records
        .into_iter()
        .filter(|record| episode_record_allowed(record, access, project))
        .filter_map(|record| match record {
            Value::Object(map) => super::episode::episode_from_record(&map),
            _ => record
                .get("Object")
                .and_then(Value::as_object)
                .and_then(super::episode::episode_from_record),
        })
        .collect()
}

fn episode_record_allowed(record: &Value, access: &AccessContext, project: Option<&str>) -> bool {
    episode_record_matches_project(record, project)
        && episode_record_allowed_by_policy(record, access)
}

fn episode_record_allowed_by_policy(record: &Value, access: &AccessContext) -> bool {
    let Some(tags) = raw_object(record)
        .and_then(|map| map.get("policy_tags"))
        .and_then(raw_array)
        .map(|values| values.iter().filter_map(json_string).collect::<Vec<_>>())
    else {
        return true;
    };

    if tags.is_empty() {
        return true;
    }

    let Some(allowed_tags) = &access.allowed_tags else {
        return true;
    };

    let allowed = allowed_tags
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    tags.iter().any(|tag| allowed.contains(tag))
}

fn fact_record_matches_project(record: &Value, project: Option<&str>) -> bool {
    let Some(project) = project.filter(|project| !project.trim().is_empty()) else {
        return true;
    };

    raw_object(record)
        .and_then(|map| map.get("project"))
        .and_then(json_string)
        .is_some_and(|value| value == project)
}

fn episode_record_matches_project(record: &Value, project: Option<&str>) -> bool {
    let Some(project) = project.filter(|project| !project.trim().is_empty()) else {
        return true;
    };

    raw_object(record)
        .and_then(|map| map.get("project"))
        .and_then(json_string)
        .is_some_and(|value| value == project)
}

fn fact_record_matches_type(record: &Value, fact_types: &[String]) -> bool {
    if fact_types.is_empty() {
        return true;
    }

    raw_object(record)
        .and_then(|map| map.get("fact_type"))
        .and_then(json_string)
        .is_some_and(|value| fact_types.iter().any(|fact_type| fact_type == value))
}

fn raw_object(record: &Value) -> Option<&serde_json::Map<String, Value>> {
    if let Some(map) = record.as_object() {
        Some(map)
    } else {
        record.get("Object").and_then(Value::as_object)
    }
}

fn raw_array(value: &Value) -> Option<&Vec<Value>> {
    if let Some(array) = value.as_array() {
        Some(array)
    } else {
        value.get("Array").and_then(Value::as_array)
    }
}

#[allow(clippy::too_many_arguments)]
fn build_episode_fallback_items(
    mut episodes: Vec<crate::models::Episode>,
    query_opt: Option<&str>,
    scope: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
    window_start: Option<chrono::DateTime<chrono::Utc>>,
    window_end: Option<chrono::DateTime<chrono::Utc>>,
    timeline_mode: bool,
    budget: i32,
) -> Vec<AssembledContextItem> {
    apply_episode_time_window(&mut episodes, window_start, window_end);

    if timeline_mode {
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
        .take(budget.max(1) as usize)
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
            rationale: default_episode_fallback_rationale(query_opt, scope, cutoff),
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

#[allow(clippy::too_many_arguments)]
async fn build_wake_up_view(
    service: &crate::service::MemoryService,
    namespace: &str,
    scope: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
    project: Option<&str>,
    fact_types: &[String],
    budget: i32,
    access: &AccessContext,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let records = service
        .db_client
        .select_table("fact", namespace)
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    let mut facts = filter_facts_by_constraints(records, access, project, fact_types)
        .into_iter()
        .filter(|fact| fact.scope == scope)
        .filter(|fact| fact_is_active_at(fact, cutoff))
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
                    .max(super::decayed_confidence(&fact, cutoff))
            } else {
                super::decayed_confidence(&fact, cutoff)
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
            json!({"scope": scope, "project": project, "fact_type_count": fact_types.len()}),
            json!({"count": items.len(), "persona_count": persona_count}),
            Some(access),
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

fn fact_is_active_at(fact: &crate::models::Fact, cutoff: chrono::DateTime<chrono::Utc>) -> bool {
    if fact.t_valid > cutoff || fact.t_ingested > cutoff {
        return false;
    }

    match (fact.t_invalid, fact.t_invalid_ingested) {
        (None, _) => true,
        (Some(invalidated_at), _) if invalidated_at > cutoff => true,
        (_, Some(invalidated_ingested_at)) if invalidated_ingested_at > cutoff => true,
        _ => false,
    }
}

/// Test-only convenience wrapper around the production comparator below.
///
/// Production code uses `compare_facts_by_recency` directly in composite sorts,
/// while tests keep this helper to assert the standalone ordering contract.
#[cfg(test)]
fn sort_facts_by_recency(facts: &mut [crate::models::Fact]) {
    facts.sort_by(compare_facts_by_recency);
}

fn compare_facts_by_recency(
    left: &crate::models::Fact,
    right: &crate::models::Fact,
) -> std::cmp::Ordering {
    right
        .t_valid
        .cmp(&left.t_valid)
        .then_with(|| left.fact_id.cmp(&right.fact_id))
}

#[derive(Debug)]
struct RankedContextFact {
    fact: crate::models::Fact,
    rationale: String,
    retrieval_tier: RetrievalTier,
    fusion_score: f64,
    source_priority: u8,
    decayed_confidence: f64,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetrievalTier {
    Direct,
    AliasExpanded,
    TemporalExpanded,
    GraphExpanded,
    SemanticExpanded,
    EpisodeFallback,
}

impl RetrievalTier {
    fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::AliasExpanded => "alias",
            Self::TemporalExpanded => "temporal",
            Self::GraphExpanded => "graph",
            Self::SemanticExpanded => "semantic",
            Self::EpisodeFallback => "fallback",
        }
    }

    fn precedence(self) -> u8 {
        match self {
            Self::Direct => 0,
            Self::EpisodeFallback => 1,
            Self::AliasExpanded => 2,
            Self::TemporalExpanded => 3,
            Self::GraphExpanded => 4,
            Self::SemanticExpanded => 5,
        }
    }
}

fn build_ranked_context_facts(
    lexical_facts: Vec<(crate::models::Fact, RetrievalTier)>,
    community_facts: Vec<(crate::models::Fact, String, f64)>,
    semantic_facts: Vec<(crate::models::Fact, String)>,
    query_opt: Option<&str>,
    scope: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
) -> Vec<RankedContextFact> {
    let mut ranked_by_fact_id = std::collections::HashMap::<String, RankedContextFact>::new();

    for (rank, (fact, retrieval_tier)) in lexical_facts.into_iter().enumerate() {
        let fact_id = fact.fact_id.clone();
        let confidence = super::decayed_confidence(&fact, cutoff);
        let lexical_score = lexical_fusion_score(rank, &fact);
        ranked_by_fact_id
            .entry(fact_id)
            .and_modify(|candidate| {
                candidate.fusion_score += lexical_score;
                candidate.source_priority = 0;
                candidate.decayed_confidence = candidate.decayed_confidence.max(confidence);
                if retrieval_tier.precedence() > candidate.retrieval_tier.precedence() {
                    candidate.retrieval_tier = retrieval_tier;
                    candidate.rationale = build_rationale(
                        retrieval_tier,
                        &fact,
                        candidate.decayed_confidence,
                        default_direct_rationale(query_opt, scope, cutoff),
                    );
                }
            })
            .or_insert_with(|| RankedContextFact {
                rationale: build_rationale(
                    retrieval_tier,
                    &fact,
                    confidence,
                    default_direct_rationale(query_opt, scope, cutoff),
                ),
                fact,
                retrieval_tier,
                fusion_score: lexical_score,
                source_priority: 0,
                decayed_confidence: confidence,
            });
    }

    for (rank, (fact, rationale, graph_origin_factor)) in community_facts.into_iter().enumerate() {
        let fact_id = fact.fact_id.clone();
        let confidence = super::decayed_confidence(&fact, cutoff);
        let weighted_rank = reciprocal_rank(rank) * graph_origin_factor.clamp(0.0, 1.0);
        if let Some(candidate) = ranked_by_fact_id.get_mut(&fact_id) {
            candidate.fusion_score += weighted_rank;
            candidate.decayed_confidence = candidate.decayed_confidence.max(confidence);
            continue;
        }

        let rationale = build_rationale(RetrievalTier::GraphExpanded, &fact, confidence, rationale);

        ranked_by_fact_id.insert(
            fact_id,
            RankedContextFact {
                fact,
                rationale,
                retrieval_tier: RetrievalTier::GraphExpanded,
                fusion_score: weighted_rank,
                source_priority: 1,
                decayed_confidence: confidence,
            },
        );
    }

    for (rank, (fact, rationale)) in semantic_facts.into_iter().enumerate() {
        let fact_id = fact.fact_id.clone();
        let confidence = super::decayed_confidence(&fact, cutoff);
        if let Some(candidate) = ranked_by_fact_id.get_mut(&fact_id) {
            candidate.fusion_score += reciprocal_rank(rank);
            candidate.decayed_confidence = candidate.decayed_confidence.max(confidence);
            continue;
        }

        let rationale = build_rationale(
            RetrievalTier::SemanticExpanded,
            &fact,
            confidence,
            rationale,
        );

        ranked_by_fact_id.insert(
            fact_id,
            RankedContextFact {
                fact,
                rationale,
                retrieval_tier: RetrievalTier::SemanticExpanded,
                fusion_score: reciprocal_rank(rank),
                source_priority: 2,
                decayed_confidence: confidence,
            },
        );
    }

    ranked_by_fact_id.into_values().collect()
}

fn lexical_fusion_score(rank: usize, fact: &crate::models::Fact) -> f64 {
    reciprocal_rank(rank) * (1.0 + fact.ft_score.max(0.0))
}

fn reciprocal_rank(rank: usize) -> f64 {
    1.0 / (RECIPROCAL_RANK_FUSION_K + rank as f64 + 1.0)
}

fn default_direct_rationale(
    query_opt: Option<&str>,
    scope: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
) -> String {
    query_opt.map_or_else(
        || {
            format!(
                "matched scope={} and active at {}",
                scope,
                cutoff.date_naive()
            )
        },
        |query| {
            format!(
                "matched lexical query=\"{}\" in scope={} and active at {}",
                query,
                scope,
                cutoff.date_naive()
            )
        },
    )
}

fn default_episode_fallback_rationale(
    query_opt: Option<&str>,
    scope: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
) -> String {
    query_opt.map_or_else(
        || {
            format!(
                "tier=fallback fts=0.00 access_count=0 confidence=1.00 matched episode content in scope={} and active at {}",
                scope,
                cutoff.date_naive()
            )
        },
        |query| {
            format!(
                "tier=fallback fts=0.00 access_count=0 confidence=1.00 matched episode content query=\"{}\" in scope={} and active at {}",
                query,
                scope,
                cutoff.date_naive()
            )
        },
    )
}

fn build_rationale(
    retrieval_tier: RetrievalTier,
    fact: &crate::models::Fact,
    confidence: f64,
    detail: String,
) -> String {
    format!(
        "tier={} fts={:.2} access_count={} confidence={:.2} {}",
        retrieval_tier.as_str(),
        fact.ft_score.max(0.0),
        fact.access_count,
        confidence,
        detail,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TemporalQueryExpansion {
    temporal_groups: Vec<Vec<String>>,
    residual_query: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TemporalWindow {
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
}

fn expand_temporal_synonyms(
    query: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
) -> Option<TemporalQueryExpansion> {
    let tokens = query
        .split_whitespace()
        .map(normalize_temporal_token)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }

    let mut temporal_groups = Vec::new();
    let mut consumed = vec![false; tokens.len()];
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index].as_str();

        if let Some(month) = month_number(token) {
            if let Some(year_token) = tokens
                .get(index + 1)
                .filter(|next| is_four_digit_year(next))
                && let Ok(year) = year_token.parse::<i32>()
            {
                push_temporal_group(
                    &mut temporal_groups,
                    vec![format!("{token} {year}"), format!("{year}-{month:02}")],
                );
                consumed[index] = true;
                consumed[index + 1] = true;
                index += 2;
                continue;
            }

            push_temporal_group(&mut temporal_groups, vec![token.to_string()]);
            consumed[index] = true;
            index += 1;
            continue;
        }

        if is_weekday_name(token) {
            push_temporal_group(&mut temporal_groups, weekday_group(cutoff, token));
            consumed[index] = true;
            index += 1;
            continue;
        }

        if let Some(quarter) = parse_quarter_token(token) {
            push_temporal_group(&mut temporal_groups, quarter_group(cutoff, quarter));
            consumed[index] = true;
            index += 1;
            continue;
        }

        if token == "quarter"
            && let Some(next) = tokens.get(index + 1)
            && let Some(quarter) = parse_quarter_token(next)
        {
            push_temporal_group(&mut temporal_groups, quarter_group(cutoff, quarter));
            consumed[index] = true;
            consumed[index + 1] = true;
            index += 2;
            continue;
        }

        if token == "last" && tokens.get(index + 1).is_some_and(|next| next == "quarter") {
            push_temporal_group(&mut temporal_groups, previous_quarter_group(cutoff));
            consumed[index] = true;
            consumed[index + 1] = true;
            index += 2;
            continue;
        }

        if token == "this" && tokens.get(index + 1).is_some_and(|next| next == "week") {
            push_temporal_group(&mut temporal_groups, current_week_group(cutoff));
            consumed[index] = true;
            consumed[index + 1] = true;
            index += 2;
            continue;
        }

        if let Some(relative_shift_days) = relative_day_shift(token) {
            let target_date = cutoff.date_naive() + chrono::Duration::days(relative_shift_days);
            push_temporal_group(&mut temporal_groups, day_group_queries(target_date));
            consumed[index] = true;
            index += 1;
            continue;
        }

        if let Some(date) = parse_iso_date(token) {
            push_temporal_group(&mut temporal_groups, day_group_queries(date));
            consumed[index] = true;
            index += 1;
            continue;
        }

        index += 1;
    }

    if temporal_groups.is_empty() {
        return None;
    }

    let residual_terms = tokens
        .into_iter()
        .enumerate()
        .filter_map(|(idx, token)| (!consumed[idx]).then_some(token))
        .collect::<Vec<_>>();

    Some(TemporalQueryExpansion {
        temporal_groups,
        residual_query: (!residual_terms.is_empty()).then(|| residual_terms.join(" ")),
    })
}

fn normalize_temporal_token(token: &str) -> String {
    token
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-')
        .to_ascii_lowercase()
}

fn month_number(token: &str) -> Option<u32> {
    match token {
        "january" => Some(1),
        "february" => Some(2),
        "march" => Some(3),
        "april" => Some(4),
        "may" => Some(5),
        "june" => Some(6),
        "july" => Some(7),
        "august" => Some(8),
        "september" => Some(9),
        "october" => Some(10),
        "november" => Some(11),
        "december" => Some(12),
        _ => None,
    }
}

fn is_four_digit_year(token: &str) -> bool {
    token.len() == 4 && token.chars().all(|ch| ch.is_ascii_digit())
}

fn is_weekday_name(token: &str) -> bool {
    matches!(
        token,
        "monday" | "tuesday" | "wednesday" | "thursday" | "friday" | "saturday" | "sunday"
    )
}

fn parse_quarter_token(token: &str) -> Option<u32> {
    match token {
        "q1" | "1" | "first" => Some(1),
        "q2" | "2" | "second" => Some(2),
        "q3" | "3" | "third" => Some(3),
        "q4" | "4" | "fourth" => Some(4),
        _ => None,
    }
}

fn previous_quarter_group(cutoff: chrono::DateTime<chrono::Utc>) -> Vec<String> {
    use chrono::Datelike;

    let current_quarter = ((cutoff.month() - 1) / 3) + 1;
    let (year, quarter) = if current_quarter == 1 {
        (cutoff.year() - 1, 4)
    } else {
        (cutoff.year(), current_quarter - 1)
    };

    quarter_group_for_year(year, quarter)
}

fn quarter_group(cutoff: chrono::DateTime<chrono::Utc>, quarter: u32) -> Vec<String> {
    use chrono::Datelike;

    quarter_group_for_year(cutoff.year(), quarter)
}

fn quarter_group_for_year(year: i32, quarter: u32) -> Vec<String> {
    let mut group = vec![format!("q{quarter}")];
    for month in ((quarter - 1) * 3 + 1)..=((quarter - 1) * 3 + 3) {
        let month_name = month_name(month);
        group.push(format!("{month_name} {year}"));
        group.push(format!("{year}-{month:02}"));
    }
    group
}

fn current_week_group(cutoff: chrono::DateTime<chrono::Utc>) -> Vec<String> {
    let start_of_week = start_of_week(cutoff.date_naive());
    let mut group = Vec::new();
    for offset in 0..7 {
        group.extend(day_group_queries(
            start_of_week + chrono::Duration::days(offset),
        ));
    }
    group
}

fn weekday_group(cutoff: chrono::DateTime<chrono::Utc>, token: &str) -> Vec<String> {
    let Some(target_weekday) = weekday_from_name(token) else {
        return vec![token.to_string()];
    };

    let start_of_week = start_of_week(cutoff.date_naive());
    let target_date =
        start_of_week + chrono::Duration::days(target_weekday.num_days_from_monday() as i64);
    day_group_queries(target_date)
}

fn start_of_week(date: chrono::NaiveDate) -> chrono::NaiveDate {
    use chrono::Datelike;

    date - chrono::Duration::days(date.weekday().num_days_from_monday() as i64)
}

fn weekday_from_name(token: &str) -> Option<chrono::Weekday> {
    match token {
        "monday" => Some(chrono::Weekday::Mon),
        "tuesday" => Some(chrono::Weekday::Tue),
        "wednesday" => Some(chrono::Weekday::Wed),
        "thursday" => Some(chrono::Weekday::Thu),
        "friday" => Some(chrono::Weekday::Fri),
        "saturday" => Some(chrono::Weekday::Sat),
        "sunday" => Some(chrono::Weekday::Sun),
        _ => None,
    }
}

fn relative_day_shift(token: &str) -> Option<i64> {
    match token {
        "yesterday" => Some(-1),
        "today" => Some(0),
        "tomorrow" => Some(1),
        _ => None,
    }
}

fn parse_iso_date(token: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(token, "%Y-%m-%d").ok()
}

fn day_group_queries(date: chrono::NaiveDate) -> Vec<String> {
    use chrono::Datelike;

    vec![
        date.format("%Y-%m-%d").to_string(),
        format!("{} {}", month_name(date.month()), date.year()),
        format!("{}-{:02}", date.year(), date.month()),
        weekday_name(date.weekday()).to_string(),
    ]
}

fn month_name(month: u32) -> &'static str {
    match month {
        1 => "january",
        2 => "february",
        3 => "march",
        4 => "april",
        5 => "may",
        6 => "june",
        7 => "july",
        8 => "august",
        9 => "september",
        10 => "october",
        11 => "november",
        12 => "december",
        _ => "",
    }
}

fn weekday_name(weekday: chrono::Weekday) -> &'static str {
    match weekday {
        chrono::Weekday::Mon => "monday",
        chrono::Weekday::Tue => "tuesday",
        chrono::Weekday::Wed => "wednesday",
        chrono::Weekday::Thu => "thursday",
        chrono::Weekday::Fri => "friday",
        chrono::Weekday::Sat => "saturday",
        chrono::Weekday::Sun => "sunday",
    }
}

fn push_temporal_group(groups: &mut Vec<Vec<String>>, queries: Vec<String>) {
    let mut seen = HashSet::new();
    let group = queries
        .into_iter()
        .map(|query| query.trim().to_ascii_lowercase())
        .filter(|query| !query.is_empty())
        .filter(|query| seen.insert(query.clone()))
        .collect::<Vec<_>>();

    if !group.is_empty() {
        groups.push(group);
    }
}

fn infer_temporal_window(
    query: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
) -> Option<TemporalWindow> {
    use chrono::Datelike;

    let tokens = query
        .split_whitespace()
        .map(normalize_temporal_token)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }

    let explicit_years = tokens
        .iter()
        .filter(|token| is_four_digit_year(token))
        .filter_map(|token| token.parse::<i32>().ok())
        .collect::<HashSet<_>>();
    let shared_year = (explicit_years.len() == 1)
        .then(|| *explicit_years.iter().next().expect("shared year exists"));

    let mut ranges = Vec::<(chrono::NaiveDate, chrono::NaiveDate)>::new();
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index].as_str();

        if let Some(date) = parse_iso_date(token) {
            ranges.push((date, date));
            index += 1;
            continue;
        }

        if let Some(month) = month_number(token) {
            let next_year = tokens
                .get(index + 1)
                .filter(|next| is_four_digit_year(next))
                .and_then(|next| next.parse::<i32>().ok());
            let year = next_year.or(shared_year).unwrap_or(cutoff.year());
            ranges.push(month_date_range(year, month));
            index += usize::from(next_year.is_some()) + 1;
            continue;
        }

        if let Some(quarter) = parse_quarter_token(token) {
            let next_year = tokens
                .get(index + 1)
                .filter(|next| is_four_digit_year(next))
                .and_then(|next| next.parse::<i32>().ok());
            let year = next_year.or(shared_year).unwrap_or(cutoff.year());
            ranges.push(quarter_date_range(year, quarter));
            index += usize::from(next_year.is_some()) + 1;
            continue;
        }

        if token == "quarter"
            && let Some(next) = tokens.get(index + 1)
            && let Some(quarter) = parse_quarter_token(next)
        {
            let next_year = tokens
                .get(index + 2)
                .filter(|year| is_four_digit_year(year))
                .and_then(|year| year.parse::<i32>().ok());
            let year = next_year.or(shared_year).unwrap_or(cutoff.year());
            ranges.push(quarter_date_range(year, quarter));
            index += if next_year.is_some() { 3 } else { 2 };
            continue;
        }

        if token == "last" && tokens.get(index + 1).is_some_and(|next| next == "quarter") {
            ranges.push(previous_quarter_date_range(cutoff));
            index += 2;
            continue;
        }

        if token == "this" && tokens.get(index + 1).is_some_and(|next| next == "week") {
            let start = start_of_week(cutoff.date_naive());
            let end = start + chrono::Duration::days(6);
            ranges.push((start, end));
            index += 2;
            continue;
        }

        if is_weekday_name(token) {
            let start = start_of_week(cutoff.date_naive());
            if let Some(target_weekday) = weekday_from_name(token) {
                let date =
                    start + chrono::Duration::days(target_weekday.num_days_from_monday() as i64);
                ranges.push((date, date));
                index += 1;
                continue;
            }
        }

        if let Some(relative_shift_days) = relative_day_shift(token) {
            let target_date = cutoff.date_naive() + chrono::Duration::days(relative_shift_days);
            ranges.push((target_date, target_date));
            index += 1;
            continue;
        }

        index += 1;
    }

    if ranges.is_empty() {
        return None;
    }

    let start_date = ranges.iter().map(|(start, _)| *start).min()?;
    let end_date = ranges.iter().map(|(_, end)| *end).max()?;

    Some(TemporalWindow {
        start: start_of_day(start_date),
        end: end_of_day(end_date),
    })
}

fn month_date_range(year: i32, month: u32) -> (chrono::NaiveDate, chrono::NaiveDate) {
    let start = chrono::NaiveDate::from_ymd_opt(year, month, 1).expect("valid month start");
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let next_start =
        chrono::NaiveDate::from_ymd_opt(next_year, next_month, 1).expect("valid next month");
    (start, next_start - chrono::Duration::days(1))
}

fn quarter_date_range(year: i32, quarter: u32) -> (chrono::NaiveDate, chrono::NaiveDate) {
    let start_month = ((quarter - 1) * 3) + 1;
    let end_month = start_month + 2;
    let (start, _) = month_date_range(year, start_month);
    let (_, end) = month_date_range(year, end_month);
    (start, end)
}

fn previous_quarter_date_range(
    cutoff: chrono::DateTime<chrono::Utc>,
) -> (chrono::NaiveDate, chrono::NaiveDate) {
    use chrono::Datelike;

    let current_quarter = ((cutoff.month() - 1) / 3) + 1;
    let (year, quarter) = if current_quarter == 1 {
        (cutoff.year() - 1, 4)
    } else {
        (cutoff.year(), current_quarter - 1)
    };
    quarter_date_range(year, quarter)
}

fn start_of_day(date: chrono::NaiveDate) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_naive_utc_and_offset(
        date.and_hms_opt(0, 0, 0).expect("valid start of day"),
        chrono::Utc,
    )
}

fn end_of_day(date: chrono::NaiveDate) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_naive_utc_and_offset(
        date.and_hms_opt(23, 59, 59).expect("valid end of day"),
        chrono::Utc,
    )
}

fn novelty_factor(access_count: i64) -> f64 {
    let access_count = access_count.max(0) as f64;
    1.0 / (1.0 + access_count.ln_1p() * ACCESS_COUNT_NOVELTY_WEIGHT)
}

fn ranked_relevance_score(fact: &RankedContextFact) -> f64 {
    fact.fusion_score
        * fact.decayed_confidence.max(MIN_RANKED_CONFIDENCE)
        * novelty_factor(fact.fact.access_count)
}

fn temporal_alignment_factor(
    fact_time: chrono::DateTime<chrono::Utc>,
    temporal_focus: &TemporalWindow,
) -> f64 {
    let distance_days = if fact_time < temporal_focus.start {
        (temporal_focus.start - fact_time).num_seconds().abs() as f64 / 86_400.0
    } else if fact_time > temporal_focus.end {
        (fact_time - temporal_focus.end).num_seconds().abs() as f64 / 86_400.0
    } else {
        0.0
    };

    if distance_days <= 0.0 {
        1.0
    } else {
        1.0 / (1.0 + distance_days / TEMPORAL_ALIGNMENT_WINDOW_DAYS)
    }
}

fn candidate_temporal_alignment(
    fact: &RankedContextFact,
    temporal_focus: Option<&TemporalWindow>,
) -> f64 {
    temporal_focus
        .map(|focus| temporal_alignment_factor(fact.fact.t_valid, focus))
        .unwrap_or(1.0)
}

fn focused_ranked_relevance_score(
    fact: &RankedContextFact,
    temporal_focus: Option<&TemporalWindow>,
) -> f64 {
    let temporal_factor = candidate_temporal_alignment(fact, temporal_focus);
    ranked_relevance_score(fact) * temporal_factor
}

fn compare_ranked_context_facts_with_focus(
    a: &RankedContextFact,
    b: &RankedContextFact,
    temporal_focus: Option<&TemporalWindow>,
) -> Ordering {
    let score_a = focused_ranked_relevance_score(a, temporal_focus);
    let score_b = focused_ranked_relevance_score(b, temporal_focus);
    score_b
        .total_cmp(&score_a)
        .then_with(|| a.source_priority.cmp(&b.source_priority))
        .then_with(|| b.fact.ft_score.total_cmp(&a.fact.ft_score))
        .then_with(|| b.fact.t_valid.cmp(&a.fact.t_valid))
        .then_with(|| a.fact.fact_id.cmp(&b.fact.fact_id))
}

#[cfg_attr(not(test), allow(dead_code))]
fn compare_ranked_context_facts(a: &RankedContextFact, b: &RankedContextFact) -> Ordering {
    compare_ranked_context_facts_with_focus(a, b, None)
}

fn source_episode_selection_cap(budget: usize) -> usize {
    MAX_ITEMS_PER_SOURCE_EPISODE.min(budget.max(1))
}

fn temporal_similarity(
    left: chrono::DateTime<chrono::Utc>,
    right: chrono::DateTime<chrono::Utc>,
) -> f64 {
    let diff_days = (left - right).num_seconds().abs() as f64 / 86_400.0;
    1.0 / (1.0 + diff_days / TEMPORAL_SIMILARITY_WINDOW_DAYS)
}

fn index_key_jaccard_similarity(left: &[String], right: &[String]) -> f64 {
    let left = left
        .iter()
        .map(|key| super::normalize_text(key))
        .filter(|key| !key.is_empty())
        .collect::<HashSet<_>>();
    let right = right
        .iter()
        .map(|key| super::normalize_text(key))
        .filter(|key| !key.is_empty())
        .collect::<HashSet<_>>();

    if left.is_empty() || right.is_empty() {
        return 0.0;
    }

    let intersection = left.intersection(&right).count() as f64;
    let union = left.union(&right).count() as f64;
    if union <= 0.0 {
        0.0
    } else {
        intersection / union
    }
}

fn matched_query_terms_for_fact(
    fact: &RankedContextFact,
    query_terms: &[String],
) -> HashSet<String> {
    if query_terms.is_empty() {
        return HashSet::new();
    }

    let mut fact_terms = super::query::search_query_terms(&fact.fact.content)
        .into_iter()
        .collect::<HashSet<_>>();
    for index_key in &fact.fact.index_keys {
        fact_terms.extend(super::query::search_query_terms(index_key));
    }

    query_terms
        .iter()
        .filter(|term| fact_terms.contains(term.as_str()))
        .cloned()
        .collect()
}

fn query_term_set_similarity(left: &HashSet<String>, right: &HashSet<String>) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }

    let intersection = left.intersection(right).count() as f64;
    let union = left.union(right).count() as f64;
    if union <= 0.0 {
        0.0
    } else {
        intersection / union
    }
}

fn prune_redundant_selected_facts(
    mut selected: Vec<RankedContextFact>,
    query_terms: &[String],
    temporal_focus: Option<&TemporalWindow>,
) -> Vec<RankedContextFact> {
    const REDUNDANT_SUPPORT_SIMILARITY: f64 = 0.40;
    if selected.len() <= 1 || query_terms.len() < 4 {
        return selected;
    }

    loop {
        let matched_terms = selected
            .iter()
            .map(|fact| matched_query_terms_for_fact(fact, query_terms))
            .collect::<Vec<_>>();
        let mut term_frequency = HashMap::<String, usize>::new();
        for terms in &matched_terms {
            for term in terms {
                *term_frequency.entry(term.clone()).or_default() += 1;
            }
        }
        let informative_terms = matched_terms
            .iter()
            .map(|terms| {
                terms
                    .iter()
                    .filter(|term| {
                        term_frequency.get(term.as_str()).copied().unwrap_or(0) < selected.len()
                    })
                    .cloned()
                    .collect::<HashSet<_>>()
            })
            .collect::<Vec<_>>();

        let mut removal_idx = None;
        let mut removal_support_count = 0usize;
        let mut removal_score = f64::INFINITY;

        for idx in 0..selected.len() {
            if informative_terms[idx].len() < 4 {
                continue;
            }

            let mut support_count = 0usize;
            let mut similarity_count = 0usize;

            for other_idx in 0..selected.len() {
                if idx == other_idx {
                    continue;
                }

                let similarity = query_term_set_similarity(
                    &informative_terms[idx],
                    &informative_terms[other_idx],
                );
                if similarity >= REDUNDANT_SUPPORT_SIMILARITY {
                    support_count += 1;
                }
                similarity_count += 1;
            }

            if support_count < 2 || similarity_count == 0 {
                continue;
            }

            let score = focused_ranked_relevance_score(&selected[idx], temporal_focus);
            let should_remove = removal_idx.is_none()
                || support_count > removal_support_count
                || (support_count == removal_support_count && score < removal_score);
            if should_remove {
                removal_idx = Some(idx);
                removal_support_count = support_count;
                removal_score = score;
            }
        }

        let Some(removal_idx) = removal_idx else {
            break;
        };
        selected.remove(removal_idx);
    }

    selected
}

fn candidate_redundancy(candidate: &RankedContextFact, selected: &RankedContextFact) -> f64 {
    let index_key_similarity =
        index_key_jaccard_similarity(&candidate.fact.index_keys, &selected.fact.index_keys);
    let temporal_overlap = temporal_similarity(candidate.fact.t_valid, selected.fact.t_valid);
    ((REDUNDANCY_INDEX_KEY_WEIGHT * index_key_similarity)
        + (REDUNDANCY_TEMPORAL_WEIGHT * temporal_overlap))
        .clamp(0.0, 1.0)
}

fn mmr_selection_score(
    candidate: &RankedContextFact,
    selected: &[RankedContextFact],
    max_relevance: f64,
    temporal_focus: Option<&TemporalWindow>,
) -> f64 {
    let relevance = (focused_ranked_relevance_score(candidate, temporal_focus)
        / max_relevance.max(MIN_RANKED_CONFIDENCE))
    .clamp(0.0, 1.0);
    if selected.is_empty() {
        return relevance;
    }

    let redundancy = selected
        .iter()
        .map(|picked| candidate_redundancy(candidate, picked))
        .fold(0.0, f64::max);

    (MMR_RELEVANCE_WEIGHT * relevance) - ((1.0 - MMR_RELEVANCE_WEIGHT) * redundancy)
}

fn select_ranked_context_facts(
    mut facts: Vec<RankedContextFact>,
    budget: usize,
    temporal_focus: Option<TemporalWindow>,
    query_terms: Vec<String>,
) -> Vec<RankedContextFact> {
    if facts.is_empty() || budget == 0 {
        return Vec::new();
    }

    let temporal_focus_ref = temporal_focus.as_ref();
    facts.sort_by(|left, right| {
        compare_ranked_context_facts_with_focus(left, right, temporal_focus_ref)
    });

    let max_relevance = facts
        .first()
        .map(|fact| focused_ranked_relevance_score(fact, temporal_focus_ref))
        .unwrap_or(1.0)
        .max(MIN_RANKED_CONFIDENCE);
    let per_source_episode_cap = source_episode_selection_cap(budget);
    let mut source_counts = HashMap::<String, usize>::new();
    let mut selected = Vec::with_capacity(budget.min(facts.len()));

    while selected.len() < budget && !facts.is_empty() {
        let enforce_temporal_alignment = temporal_focus_ref.is_some()
            && facts.iter().any(|candidate| {
                candidate_temporal_alignment(candidate, temporal_focus_ref)
                    >= MIN_TEMPORAL_ALIGNMENT_TO_FILL_BUDGET
            });
        let enforce_cap = facts.iter().any(|candidate| {
            source_counts
                .get(candidate.fact.source_episode.as_str())
                .copied()
                .unwrap_or(0)
                < per_source_episode_cap
        });

        let mut best_idx = None;
        let mut best_score = f64::NEG_INFINITY;
        let mut best_alignment = 1.0;
        for (idx, candidate) in facts.iter().enumerate() {
            let source_count = source_counts
                .get(candidate.fact.source_episode.as_str())
                .copied()
                .unwrap_or(0);
            if enforce_cap && source_count >= per_source_episode_cap {
                continue;
            }

            let temporal_alignment = candidate_temporal_alignment(candidate, temporal_focus_ref);
            if enforce_temporal_alignment
                && temporal_alignment < MIN_TEMPORAL_ALIGNMENT_TO_FILL_BUDGET
            {
                continue;
            }

            let score =
                mmr_selection_score(candidate, &selected, max_relevance, temporal_focus_ref);
            let is_better = match best_idx {
                None => true,
                Some(_) if score > best_score + 1e-9 => true,
                Some(current_best_idx)
                    if (score - best_score).abs() <= 1e-9
                        && compare_ranked_context_facts_with_focus(
                            candidate,
                            &facts[current_best_idx],
                            temporal_focus_ref,
                        ) == Ordering::Less =>
                {
                    true
                }
                _ => false,
            };

            if is_better {
                best_idx = Some(idx);
                best_score = score;
                best_alignment = temporal_alignment;
            }
        }

        let Some(best_idx) = best_idx else {
            break;
        };
        if !selected.is_empty() && best_alignment < MIN_TEMPORAL_ALIGNMENT_TO_FILL_BUDGET {
            break;
        }
        let chosen = facts.remove(best_idx);
        *source_counts
            .entry(chosen.fact.source_episode.clone())
            .or_default() += 1;
        selected.push(chosen);
    }

    prune_redundant_selected_facts(selected, &query_terms, temporal_focus_ref)
}

#[cfg_attr(not(test), allow(dead_code))]
fn sort_ranked_context_facts(facts: &mut [RankedContextFact]) {
    facts.sort_by(compare_ranked_context_facts);
}

fn sort_ranked_context_facts_for_timeline(facts: &mut [RankedContextFact]) {
    facts.sort_by(|a, b| {
        a.fact
            .t_valid
            .cmp(&b.fact.t_valid)
            .then_with(|| a.fact.fact_id.cmp(&b.fact.fact_id))
    });
}

fn apply_time_window(
    facts: &mut Vec<RankedContextFact>,
    window_start: Option<chrono::DateTime<chrono::Utc>>,
    window_end: Option<chrono::DateTime<chrono::Utc>>,
) {
    if window_start.is_none() && window_end.is_none() {
        return;
    }

    facts.retain(|ranked| {
        let after_start = window_start.is_none_or(|start| ranked.fact.t_valid >= start);
        let before_end = window_end.is_none_or(|end| ranked.fact.t_valid <= end);
        after_start && before_end
    });
}

/// Expands a search query with entity aliases for broader recall.
///
/// Looks up entities whose canonical names appear in the query,
/// and returns additional query terms derived from their aliases.
async fn expand_query_with_aliases(
    service: &crate::service::MemoryService,
    query: &str,
    namespace: &str,
) -> Vec<String> {
    let terms: Vec<&str> = query.split_whitespace().collect();
    if terms.is_empty() {
        return Vec::new();
    }

    // Collect all n-gram phrases and their positions
    let mut phrase_entries: Vec<(String, usize, usize)> = Vec::new();
    for span_len in (1..=terms.len()).rev() {
        for start in 0..=terms.len().saturating_sub(span_len) {
            let end = start + span_len;
            let phrase = terms[start..end].join(" ");
            if phrase.len() >= 2 {
                phrase_entries.push((phrase, start, end));
            }
        }
    }

    // Deduplicate normalized names for batch lookup
    let normalized_names: Vec<String> = phrase_entries
        .iter()
        .map(|(phrase, _, _)| super::normalize_text(phrase))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    // Single batch query instead of O(N²) individual lookups
    let entities = service
        .db_client
        .select_entities_batch(namespace, &normalized_names)
        .await
        .unwrap_or_default();

    // Build lookup map: normalized_name → aliases
    let mut entity_aliases: HashMap<String, Vec<String>> = HashMap::new();
    for entity in &entities {
        let obj = match entity.as_object() {
            Some(obj) => obj,
            None => continue,
        };
        // Use canonical_name_normalized as primary key, fall back to normalizing canonical_name
        let canonical_norm = obj
            .get("canonical_name_normalized")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                obj.get("canonical_name")
                    .and_then(|v| v.as_str())
                    .map(super::normalize_text)
            })
            .unwrap_or_default();
        let aliases: Vec<String> = obj
            .get("aliases")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if !canonical_norm.is_empty() && !aliases.is_empty() {
            entity_aliases.entry(canonical_norm).or_insert(aliases);
        }
    }

    // Expand queries using matched entities
    let mut expanded = HashSet::new();
    for (phrase, start, end) in &phrase_entries {
        let normalized = super::normalize_text(phrase);
        if let Some(aliases) = entity_aliases.get(&normalized) {
            for alias_str in aliases {
                let mut parts: Vec<String> = terms[..*start]
                    .iter()
                    .map(|term| (*term).to_string())
                    .collect();
                parts.push(alias_str.clone());
                parts.extend(terms[*end..].iter().map(|term| (*term).to_string()));
                let alias_expanded = parts.join(" ");

                if alias_expanded != query {
                    expanded.insert(alias_expanded);
                }
            }
        }
    }

    expanded.into_iter().collect()
}

#[cfg(test)]
#[allow(dead_code)]
async fn expand_query_with_aliases_for_test(
    service: &crate::service::MemoryService,
    query: &str,
    namespace: &str,
) -> Vec<String> {
    expand_query_with_aliases(service, query, namespace).await
}

struct LexicalQueryResult {
    records: Vec<Value>,
    retrieval_tier: RetrievalTier,
}

#[allow(clippy::too_many_arguments)]
async fn select_fact_records_for_query(
    service: &crate::service::MemoryService,
    namespace: &str,
    scope: &str,
    cutoff_iso: &str,
    query_opt: Option<&str>,
    limit: i32,
    project: Option<&str>,
    fact_types: &[String],
) -> Result<LexicalQueryResult, MemoryError> {
    let query_terms = query_opt
        .map(super::query::search_query_terms)
        .unwrap_or_default();
    let candidate_limit = lexical_candidate_limit(limit);

    let initial = service
        .db_client
        .select_facts_filtered_advanced(
            namespace,
            scope,
            cutoff_iso,
            query_opt,
            candidate_limit,
            project,
            fact_types,
        )
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;
    let initial = rank_lexical_records(initial, &query_terms);

    let Some(_query) = query_opt else {
        return Ok(LexicalQueryResult {
            records: initial,
            retrieval_tier: RetrievalTier::Direct,
        });
    };

    if query_terms.len() < 3 {
        return Ok(LexicalQueryResult {
            records: initial,
            retrieval_tier: RetrievalTier::Direct,
        });
    }

    let fallback_terms = build_lexical_fallback_queries(&query_terms);

    let mut fallback_records = Vec::new();
    for term in fallback_terms {
        let term_records = service
            .db_client
            .select_facts_filtered_advanced(
                namespace,
                scope,
                cutoff_iso,
                Some(term.as_str()),
                candidate_limit,
                project,
                fact_types,
            )
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;
        fallback_records.extend(term_records);
    }

    let mut seen_fact_ids = std::collections::HashSet::new();
    fallback_records.retain(|record| {
        // Use unwrap_record_string to handle both plain strings and wrapped forms
        // like {"String": "fact:xyz"} that SurrealDB may return.
        let Some(fact_id) = record
            .get("fact_id")
            .and_then(super::episode::unwrap_record_string)
        else {
            return true;
        };
        seen_fact_ids.insert(fact_id)
    });

    let fallback_records = rank_lexical_records(fallback_records, &query_terms);

    let initial_score = top_query_score(&initial, &query_terms);
    let fallback_score = top_query_score(&fallback_records, &query_terms);
    let best_score = initial_score.max(fallback_score);
    let best_phrase_overlap = top_phrase_overlap(&initial, &query_terms)
        .max(top_phrase_overlap(&fallback_records, &query_terms));

    if best_score < query_terms.len().min(4) || (query_terms.len() >= 3 && best_phrase_overlap == 0)
    {
        let scanned_records = scan_fact_records_by_query_terms(
            service,
            namespace,
            scope,
            cutoff_iso,
            project,
            fact_types,
            &query_terms,
            candidate_limit,
        )
        .await?;
        let scanned_score = top_query_score(&scanned_records, &query_terms);
        if (query_terms.len() >= 3 && best_phrase_overlap == 0 && !scanned_records.is_empty())
            || scanned_score > best_score
        {
            return Ok(LexicalQueryResult {
                records: scanned_records,
                retrieval_tier: RetrievalTier::EpisodeFallback,
            });
        }
    }

    if fallback_score > initial_score {
        return Ok(LexicalQueryResult {
            records: fallback_records,
            retrieval_tier: RetrievalTier::EpisodeFallback,
        });
    }

    if !initial.is_empty() {
        return Ok(LexicalQueryResult {
            records: initial,
            retrieval_tier: RetrievalTier::Direct,
        });
    }

    let retrieval_tier = if fallback_records.is_empty() {
        RetrievalTier::Direct
    } else {
        RetrievalTier::EpisodeFallback
    };

    Ok(LexicalQueryResult {
        records: fallback_records,
        retrieval_tier,
    })
}

fn lexical_candidate_limit(limit: i32) -> i32 {
    let base = limit.max(1);
    let cap = base.max(50);
    (base.saturating_mul(5)).clamp(base, cap)
}

fn build_lexical_fallback_queries(query_terms: &[String]) -> Vec<String> {
    let mut queries = Vec::new();

    for width in (2..=3).rev() {
        if query_terms.len() < width {
            continue;
        }
        for window in query_terms.windows(width) {
            let query = window.join(" ");
            if !queries.contains(&query) {
                queries.push(query);
            }
        }
    }

    for term in query_terms {
        if !queries.contains(term) {
            queries.push(term.clone());
        }
    }

    queries
}

#[allow(clippy::too_many_arguments)]
async fn scan_fact_records_by_query_terms(
    service: &crate::service::MemoryService,
    namespace: &str,
    scope: &str,
    cutoff_iso: &str,
    project: Option<&str>,
    fact_types: &[String],
    query_terms: &[String],
    limit: i32,
) -> Result<Vec<Value>, MemoryError> {
    let records = service
        .db_client
        .select_table("fact", namespace)
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    let mut filtered = records
        .into_iter()
        .filter(|record| {
            raw_object(record)
                .and_then(|map| map.get("scope"))
                .and_then(json_string)
                .is_some_and(|value| value == scope)
        })
        .filter(|record| {
            raw_object(record)
                .and_then(|map| map.get("t_valid"))
                .and_then(json_string)
                .is_some_and(|value| value <= cutoff_iso)
        })
        .filter(|record| {
            raw_object(record)
                .and_then(|map| map.get("t_invalid"))
                .and_then(json_string)
                .is_none_or(|value| value > cutoff_iso)
        })
        .filter(|record| fact_record_matches_project(record, project))
        .filter(|record| fact_record_matches_type(record, fact_types))
        .filter(|record| lexical_query_overlap(record, query_terms) > 0)
        .map(|mut record| {
            let score = lexical_query_score(&record, query_terms) as f64;
            if let Some(object) = record.as_object_mut() {
                object.insert("ft_score".to_string(), json!(score));
            } else if let Some(object) = record.get_mut("Object").and_then(Value::as_object_mut) {
                object.insert("ft_score".to_string(), json!(score));
            }
            record
        })
        .collect::<Vec<_>>();

    filtered = rank_lexical_records(filtered, query_terms);
    filtered.truncate(limit.max(1) as usize);
    Ok(filtered)
}

fn rank_lexical_records(mut records: Vec<Value>, query_terms: &[String]) -> Vec<Value> {
    if query_terms.is_empty() {
        return records;
    }

    for record in &mut records {
        let combined_score =
            lexical_ft_score(record) + lexical_query_score(record, query_terms) as f64;
        if let Some(object) = record.as_object_mut() {
            object.insert("ft_score".to_string(), json!(combined_score));
        } else if let Some(object) = record.get_mut("Object").and_then(Value::as_object_mut) {
            object.insert("ft_score".to_string(), json!(combined_score));
        }
    }

    records.sort_by(|left, right| {
        lexical_query_score(right, query_terms)
            .cmp(&lexical_query_score(left, query_terms))
            .then_with(|| lexical_ft_score(right).total_cmp(&lexical_ft_score(left)))
            .then_with(|| lexical_t_valid(right).cmp(&lexical_t_valid(left)))
            .then_with(|| lexical_fact_id(left).cmp(&lexical_fact_id(right)))
    });

    records
}

fn top_query_score(records: &[Value], query_terms: &[String]) -> usize {
    records
        .iter()
        .map(|record| lexical_query_score(record, query_terms))
        .max()
        .unwrap_or(0)
}

fn top_phrase_overlap(records: &[Value], query_terms: &[String]) -> usize {
    records
        .iter()
        .map(|record| lexical_phrase_overlap(record, query_terms))
        .max()
        .unwrap_or(0)
}

fn lexical_query_overlap(record: &Value, query_terms: &[String]) -> usize {
    if query_terms.is_empty() {
        return 0;
    }

    let mut record_terms = std::collections::HashSet::<String>::new();
    if let Some(content) = raw_object(record)
        .and_then(|map| map.get("content"))
        .and_then(json_string)
    {
        record_terms.extend(super::query::search_query_terms(content));
    }
    if let Some(index_keys) = raw_object(record)
        .and_then(|map| map.get("index_keys"))
        .and_then(raw_array)
    {
        for value in index_keys {
            if let Some(index_key) = json_string(value) {
                record_terms.extend(super::query::search_query_terms(index_key));
            }
        }
    }

    query_terms
        .iter()
        .filter(|term| record_terms.contains(term.as_str()))
        .count()
}

fn lexical_query_score(record: &Value, query_terms: &[String]) -> usize {
    let unigram_overlap = lexical_query_overlap(record, query_terms);
    let phrase_overlap = lexical_phrase_overlap(record, query_terms);
    let trigram_overlap = lexical_ngram_overlap(record, query_terms, 3);

    unigram_overlap + (phrase_overlap * 2) + trigram_overlap
}

fn lexical_query_overlap_for_fact(fact: &crate::models::Fact, query_terms: &[String]) -> usize {
    if query_terms.is_empty() {
        return 0;
    }

    let mut fact_terms = super::query::search_query_terms(&fact.content)
        .into_iter()
        .collect::<HashSet<_>>();
    for index_key in &fact.index_keys {
        fact_terms.extend(super::query::search_query_terms(index_key));
    }

    query_terms
        .iter()
        .filter(|term| fact_terms.contains(term.as_str()))
        .count()
}

fn lexical_query_score_for_fact(fact: &crate::models::Fact, query_terms: &[String]) -> usize {
    let content_terms = super::query::search_query_terms(&fact.content);
    let unigram_overlap = lexical_query_overlap_for_fact(fact, query_terms);
    let phrase_overlap = lexical_ngram_overlap_for_terms(&content_terms, query_terms, 2)
        + lexical_ngram_overlap_for_terms(&content_terms, query_terms, 3);
    let trigram_overlap = lexical_ngram_overlap_for_terms(&content_terms, query_terms, 3);

    unigram_overlap + (phrase_overlap * 2) + trigram_overlap
}

fn lexical_phrase_overlap(record: &Value, query_terms: &[String]) -> usize {
    lexical_ngram_overlap(record, query_terms, 2) + lexical_ngram_overlap(record, query_terms, 3)
}

fn lexical_ngram_overlap(record: &Value, query_terms: &[String], width: usize) -> usize {
    if query_terms.len() < width {
        return 0;
    }

    let content_terms = lexical_record_terms(record);
    lexical_ngram_overlap_for_terms(&content_terms, query_terms, width)
}

fn lexical_ngram_overlap_for_terms(
    content_terms: &[String],
    query_terms: &[String],
    width: usize,
) -> usize {
    if content_terms.len() < width {
        return 0;
    }

    let record_ngrams = content_terms
        .windows(width)
        .map(|window| window.join(" "))
        .collect::<HashSet<_>>();

    query_terms
        .windows(width)
        .filter(|window| record_ngrams.contains(&window.join(" ")))
        .count()
}

fn lexical_record_terms(record: &Value) -> Vec<String> {
    raw_object(record)
        .and_then(|map| map.get("content"))
        .and_then(json_string)
        .map(super::query::search_query_terms)
        .unwrap_or_default()
}

fn lexical_ft_score(record: &Value) -> f64 {
    raw_object(record)
        .and_then(|map| map.get("ft_score"))
        .and_then(json_f64)
        .unwrap_or(0.0)
}

fn lexical_t_valid(record: &Value) -> String {
    raw_object(record)
        .and_then(|map| map.get("t_valid"))
        .and_then(json_string)
        .unwrap_or_default()
        .to_string()
}

fn lexical_fact_id(record: &Value) -> String {
    raw_object(record)
        .and_then(|map| map.get("fact_id"))
        .and_then(super::episode::unwrap_record_string)
        .unwrap_or_default()
        .to_string()
}

async fn select_episode_records_for_query(
    service: &crate::service::MemoryService,
    namespace: &str,
    scope: &str,
    cutoff_iso: &str,
    query_opt: Option<&str>,
    limit: i32,
    project: Option<&str>,
) -> Result<Vec<Value>, MemoryError> {
    let initial = service
        .db_client
        .select_episodes_by_content_advanced(
            namespace, scope, cutoff_iso, query_opt, limit, project,
        )
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    if !initial.is_empty() || query_opt.is_none() {
        return Ok(initial);
    }

    let Some(query) = query_opt else {
        return Ok(initial);
    };

    let fallback_terms = query
        .split_whitespace()
        .filter(|term| !term.trim().is_empty())
        .collect::<Vec<_>>();
    if fallback_terms.len() < 2 {
        return Ok(initial);
    }

    let mut fallback_records = Vec::new();
    for term in fallback_terms {
        let term_records = service
            .db_client
            .select_episodes_by_content_advanced(
                namespace,
                scope,
                cutoff_iso,
                Some(term),
                limit,
                project,
            )
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;
        fallback_records.extend(term_records);
    }

    let mut seen_episode_ids = HashSet::new();
    fallback_records.retain(|record| {
        let Some(episode_id) = record
            .get("episode_id")
            .and_then(json_string)
            .or_else(|| record.get("id").and_then(json_string))
        else {
            return true;
        };
        seen_episode_ids.insert(episode_id.to_string())
    });

    Ok(fallback_records)
}

struct CollectTemporalFactsRequest<'a> {
    namespace: &'a str,
    scope: &'a str,
    cutoff_iso: &'a str,
    cutoff: chrono::DateTime<chrono::Utc>,
    query: &'a str,
    access: &'a AccessContext,
    project: Option<&'a str>,
    fact_types: &'a [String],
    budget: i32,
}

async fn collect_temporal_facts(
    service: &crate::service::MemoryService,
    request: CollectTemporalFactsRequest<'_>,
) -> Result<Vec<crate::models::Fact>, MemoryError> {
    let Some(expansion) = expand_temporal_synonyms(request.query, request.cutoff) else {
        return Ok(Vec::new());
    };
    let residual_query_terms = expansion
        .residual_query
        .as_deref()
        .map(super::query::search_query_terms)
        .unwrap_or_default();

    if let Some(temporal_window) = infer_temporal_window(request.query, request.cutoff) {
        let records = service
            .db_client
            .select_table("fact", request.namespace)
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

        let mut facts = filter_facts_by_constraints(
            records,
            request.access,
            request.project,
            request.fact_types,
        )
        .into_iter()
        .filter(|fact| fact.scope == request.scope)
        .filter(|fact| fact_is_active_at(fact, request.cutoff))
        .filter(|fact| fact.t_valid >= temporal_window.start && fact.t_valid <= temporal_window.end)
        .collect::<Vec<_>>();

        rank_temporal_candidate_facts(&mut facts, &residual_query_terms);
        facts.truncate(request.budget.max(1) as usize);
        return Ok(facts);
    }

    let search_limit = request.budget.max(1) * 4;
    let mut matched_facts_by_id = HashMap::<String, crate::models::Fact>::new();
    let mut eligible_fact_ids: Option<HashSet<String>> = None;

    for temporal_group in expansion.temporal_groups {
        let mut group_fact_ids = HashSet::new();

        for temporal_query in temporal_group {
            let records = service
                .db_client
                .select_facts_filtered_advanced(
                    request.namespace,
                    request.scope,
                    request.cutoff_iso,
                    Some(&temporal_query),
                    search_limit,
                    request.project,
                    request.fact_types,
                )
                .await
                .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

            for fact in filter_facts_by_constraints(
                records,
                request.access,
                request.project,
                request.fact_types,
            ) {
                group_fact_ids.insert(fact.fact_id.clone());
                matched_facts_by_id
                    .entry(fact.fact_id.clone())
                    .or_insert(fact);
            }
        }

        if group_fact_ids.is_empty() {
            return Ok(Vec::new());
        }

        eligible_fact_ids = Some(match eligible_fact_ids {
            None => group_fact_ids,
            Some(mut existing) => {
                existing.retain(|fact_id| group_fact_ids.contains(fact_id));
                existing
            }
        });

        if eligible_fact_ids.as_ref().is_some_and(HashSet::is_empty) {
            return Ok(Vec::new());
        }
    }

    let mut facts = eligible_fact_ids
        .unwrap_or_default()
        .into_iter()
        .filter_map(|fact_id| matched_facts_by_id.remove(&fact_id))
        .collect::<Vec<_>>();
    rank_temporal_candidate_facts(&mut facts, &residual_query_terms);
    facts.truncate(request.budget.max(1) as usize);
    Ok(facts)
}

fn rank_temporal_candidate_facts(
    facts: &mut Vec<crate::models::Fact>,
    residual_query_terms: &[String],
) {
    if residual_query_terms.is_empty() {
        facts.sort_by(compare_facts_by_recency);
        return;
    }

    facts.retain(|fact| lexical_query_overlap_for_fact(fact, residual_query_terms) > 0);
    facts.sort_by(|left, right| {
        lexical_query_score_for_fact(right, residual_query_terms)
            .cmp(&lexical_query_score_for_fact(left, residual_query_terms))
            .then_with(|| compare_facts_by_recency(left, right))
    });
}

struct CollectCommunityFactsRequest<'a> {
    namespace: &'a str,
    scope: &'a str,
    cutoff_iso: &'a str,
    query: &'a str,
    access: &'a AccessContext,
    project: Option<&'a str>,
    fact_types: &'a [String],
    direct_fact_ids: &'a std::collections::HashSet<String>,
    budget: i32,
}

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

#[derive(Debug, Clone)]
struct CommunityMatch {
    rank: usize,
    community_id: String,
    summary: String,
}

async fn collect_community_facts(
    service: &crate::service::MemoryService,
    request: CollectCommunityFactsRequest<'_>,
) -> Result<Vec<(crate::models::Fact, String, f64)>, MemoryError> {
    let matched_communities =
        find_matching_communities(service, request.namespace, request.query).await?;
    if matched_communities.is_empty() {
        return Ok(Vec::new());
    }

    let member_ids = matched_communities
        .iter()
        .flat_map(|community| community.member_entities.iter().cloned())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let fallback_records = service
        .db_client
        .select_facts_by_entity_links(
            request.namespace,
            request.scope,
            request.cutoff_iso,
            &member_ids,
            request.budget.max(1),
        )
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    let community_summary_by_member = matched_communities
        .iter()
        .enumerate()
        .flat_map(|(rank, community)| {
            community
                .member_entities
                .iter()
                .cloned()
                .map(move |entity_id| {
                    (
                        entity_id,
                        CommunityMatch {
                            rank,
                            community_id: community.community_id.clone(),
                            summary: community.summary.clone(),
                        },
                    )
                })
        })
        .collect::<std::collections::HashMap<_, _>>();

    let mut facts = filter_facts_by_constraints(
        fallback_records,
        request.access,
        request.project,
        request.fact_types,
    )
    .into_iter()
    .filter(|fact| !request.direct_fact_ids.contains(&fact.fact_id))
    .filter(|fact| {
        fact.entity_links
            .iter()
            .any(|entity_id| member_ids.iter().any(|member_id| member_id == entity_id))
    })
    .collect::<Vec<_>>();
    facts.sort_by(|left, right| {
        let left_rank = best_community_match(left, &community_summary_by_member)
            .map(|matched| matched.rank)
            .unwrap_or(usize::MAX);
        let right_rank = best_community_match(right, &community_summary_by_member)
            .map(|matched| matched.rank)
            .unwrap_or(usize::MAX);

        left_rank
            .cmp(&right_rank)
            .then_with(|| compare_facts_by_recency(left, right))
    });

    let mut entity_origin_factor_cache = HashMap::<String, f64>::new();

    let mut ranked_facts = Vec::new();
    for fact in facts.into_iter().take(request.budget.max(1) as usize) {
        let rationale = best_community_match(&fact, &community_summary_by_member).map_or_else(
            || format!("matched community summary for query=\"{}\"", request.query),
            |matched| {
                format!(
                    "matched community summary for query=\"{}\" via {}: {}",
                    request.query, matched.community_id, matched.summary
                )
            },
        );
        let origin_factor = community_origin_factor_for_fact(
            service,
            request.namespace,
            request.cutoff_iso,
            &fact,
            &community_summary_by_member,
            &mut entity_origin_factor_cache,
        )
        .await?;
        ranked_facts.push((fact, rationale, origin_factor));
    }

    Ok(ranked_facts)
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
            .and_then(|map| map.get("sem_score"))
            .and_then(|v| v.as_f64())
            .unwrap_or_else(|| {
                let embedding = record
                    .as_object()
                    .and_then(|map| map.get("embedding"))
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

#[derive(Debug)]
struct StoredCommunitySummary {
    community_id: String,
    summary: String,
    member_entities: Vec<String>,
    ft_score: f64,
}

async fn find_matching_communities(
    service: &crate::service::MemoryService,
    namespace: &str,
    query: &str,
) -> Result<Vec<StoredCommunitySummary>, MemoryError> {
    let communities = service
        .db_client
        .select_communities_matching_summary(namespace, query)
        .await?;

    let mut matched = communities
        .iter()
        .filter_map(stored_community_summary_from_value)
        .collect::<Vec<_>>();
    matched.sort_by(|left, right| {
        right
            .ft_score
            .total_cmp(&left.ft_score)
            .then_with(|| left.community_id.cmp(&right.community_id))
    });

    Ok(matched)
}

fn stored_community_summary_from_value(value: &Value) -> Option<StoredCommunitySummary> {
    let map = value.as_object()?;
    let community_id = map
        .get("community_id")
        .and_then(json_string)
        .or_else(|| map.get("id").and_then(json_string))?
        .to_string();
    let summary = map
        .get("summary")
        .and_then(json_string)
        .unwrap_or_default()
        .to_string();
    let member_entities = map
        .get("member_entities")
        .and_then(unwrap_context_array)
        .map(|values| {
            values
                .iter()
                .filter_map(json_string)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let ft_score = map.get("ft_score").and_then(json_f64).unwrap_or(0.0);

    if summary.is_empty() || member_entities.is_empty() {
        return None;
    }

    Some(StoredCommunitySummary {
        community_id,
        summary,
        member_entities,
        ft_score,
    })
}

fn unwrap_context_array(value: &Value) -> Option<&Vec<Value>> {
    if let Some(array) = value.as_array() {
        Some(array)
    } else if let Some(object) = value.as_object() {
        object.get("Array").and_then(Value::as_array)
    } else {
        None
    }
}

fn best_community_match<'a>(
    fact: &crate::models::Fact,
    matches_by_entity: &'a std::collections::HashMap<String, CommunityMatch>,
) -> Option<&'a CommunityMatch> {
    fact.entity_links
        .iter()
        .filter_map(|entity_id| matches_by_entity.get(entity_id))
        .min_by(|left, right| left.rank.cmp(&right.rank))
}

async fn community_origin_factor_for_fact(
    service: &crate::service::MemoryService,
    namespace: &str,
    cutoff_iso: &str,
    fact: &crate::models::Fact,
    matches_by_entity: &std::collections::HashMap<String, CommunityMatch>,
    entity_origin_factor_cache: &mut HashMap<String, f64>,
) -> Result<f64, MemoryError> {
    let mut best_factor: Option<f64> = None;

    for entity_id in fact
        .entity_links
        .iter()
        .filter(|entity_id| matches_by_entity.contains_key(*entity_id))
    {
        let factor = entity_origin_factor(
            service,
            namespace,
            cutoff_iso,
            entity_id,
            entity_origin_factor_cache,
        )
        .await?;
        best_factor = Some(if let Some(current) = best_factor {
            current.max(factor)
        } else {
            factor
        });
    }

    Ok(best_factor.unwrap_or(1.0))
}

async fn entity_origin_factor(
    service: &crate::service::MemoryService,
    namespace: &str,
    cutoff_iso: &str,
    entity_id: &str,
    entity_origin_factor_cache: &mut HashMap<String, f64>,
) -> Result<f64, MemoryError> {
    if let Some(cached) = entity_origin_factor_cache.get(entity_id) {
        return Ok(*cached);
    }

    let mut best_factor: Option<f64> = None;
    for direction in [GraphDirection::Incoming, GraphDirection::Outgoing] {
        for edge in service
            .db_client
            .select_edge_neighbors(namespace, entity_id, cutoff_iso, direction)
            .await?
        {
            let factor = edge_origin_factor(&edge);
            best_factor = Some(if let Some(current) = best_factor {
                current.max(factor)
            } else {
                factor
            });
        }
    }

    let factor = best_factor.unwrap_or(1.0);
    entity_origin_factor_cache.insert(entity_id.to_string(), factor);
    Ok(factor)
}

fn edge_origin_factor(edge: &Value) -> f64 {
    let Some(map) = edge.as_object() else {
        return 1.0;
    };

    let confidence = map
        .get("confidence")
        .and_then(json_f64)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);

    match map.get("origin").and_then(json_string) {
        Some("extracted") => 1.0,
        Some("inferred") => confidence,
        Some("ambiguous") => 0.5,
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests {
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
            "org",
            "org",
            "2026-01-15T10:30:00Z",
            Some("atlas launch checklist"),
            10,
            None,
            &[],
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
            "org",
            "org",
            "2026-01-15T10:30:00Z",
            Some("caroline lgbtq support group"),
            5,
            None,
            &[],
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
                assert_eq!(query_contains, Some("atlas launch"));
                Ok(vec![json!({
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
                })])
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
