//! Context assembly operations — thin orchestrator.
//!
//! The heavy lifting lives in [`pipeline`]: parameter preparation,
//! cache operations, and the multi-tier default retrieval pipeline.
//! View-mode-specific builders are in [`views`].

use std::time::Instant;

use serde_json::json;

use crate::logging::LogLevel;
use crate::models::{AccessPayload, AssembleContextRequest, AssembledContextItem};

use super::error::MemoryError;
use super::{log_event, normalize_dt};

mod alias_expansion;
mod budget;
mod community;
mod experience;
mod filtering;
mod graph;
mod lexical;
mod logging;
mod params;
mod pipeline;
mod query_mode;
mod ranking;
mod rescue;
mod scoring;
mod semantic;
mod temporal;
mod triple;
mod views;

use experience::{RecentExperienceRequest, append_recent_experience_items};
use logging::{summarize_retrieval_tiers, supplemental_experience_count};
use params::DefaultContextParams;
use pipeline::assemble_default_context;
use views::{build_facets_view, build_map_view, build_wake_up_view};

/// Records fact access for each item, logging errors without failing the operation.
async fn track_fact_accesses(
    ctx: &crate::service::service_context::ServiceContext,
    items: &[AssembledContextItem],
    access: &AccessPayload,
) {
    for item in items {
        if let Err(err) = ctx.record_fact_access(&item.fact_id, 1).await {
            ctx.logger.log(
                log_event(
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

/// Assembles context for a query.
///
/// Orchestrates: parameter preparation → cache check → view-mode dispatch
/// (facets / wake_up / map / default multi-tier) → experience append →
/// cache store → query log. All logic is delegated to `pipeline` and `views`.
pub async fn assemble_context(
    ctx: &crate::service::service_context::ServiceContext,
    request: AssembleContextRequest,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let started_at = Instant::now();
    let access = AccessPayload::from_payload(request.access.clone());

    pipeline::log_context_start(ctx, &request, access.as_ref());
    ctx.enforce_rate_limit(access.as_ref())?;

    let params = pipeline::prepare_context_params(ctx, &request, access).await?;

    if !params.access.is_scope_allowed(&request.scope) {
        return Ok(vec![]);
    }

    let query_log_diagnostics = logging::QueryLogDiagnostics {
        resolved_view_mode: params.resolved_view_mode_opt.as_deref(),
        query_flags: &params.query_flags.as_labels(),
    };

    // --- Cache check ---
    if let Some(cached) = pipeline::check_cache(ctx, &params.cache_key).await {
        track_fact_accesses(ctx, &cached, &params.access).await;

        ctx.logger.log(
            log_event(
                "assemble_context.cache_hit",
                json!({"scope": request.scope, "query": request.query}),
                json!({"count": cached.len()}),
                Some(&params.access),
                None,
                None,
            ),
            LogLevel::Info,
        );

        let latency_ms = started_at.elapsed().as_secs_f64() * 1000.0;
        logging::maybe_record_query_log(
            ctx,
            &request,
            &cached,
            true,
            latency_ms,
            &params.access,
            &query_log_diagnostics,
        )
        .await;
        return Ok(cached);
    }

    ctx.logger.log(
        log_event(
            "assemble_context.cache_miss",
            json!({"scope": request.scope, "query": request.query, "budget": request.budget}),
            json!({"status": "computing"}),
            Some(&params.access),
            None,
            None,
        ),
        LogLevel::Trace,
    );

    ctx.logger.log(
        log_event(
            "assemble_context.features",
            json!({
                "scope": request.scope,
                "query": request.query,
                "budget": request.budget,
                "project": params.project_opt,
                "resolved_view_mode": params.resolved_view_mode_opt,
                "fact_type_count": params.fact_types.len(),
                "window_start": request.window_start.map(normalize_dt),
                "window_end": request.window_end.map(normalize_dt),
                "query_logging_enabled": ctx.is_query_logging_enabled(),
            }),
            json!({}),
            Some(&params.access),
            None,
            None,
        ),
        LogLevel::Debug,
    );

    // --- View-mode dispatch ---
    let mut results: Vec<AssembledContextItem> = match params.resolved_view_mode_opt.as_deref() {
        Some("facets") => {
            build_facets_view(
                ctx,
                &params.namespace,
                &request.scope,
                params.cutoff,
                params.project_opt.as_deref(),
                request.budget,
                &params.access,
            )
            .await?
        }
        Some("wake_up") => {
            build_wake_up_view(
                ctx,
                views::FactFilterParams {
                    namespace: &params.namespace,
                    scope: &request.scope,
                    cutoff: params.cutoff,
                    project: params.project_opt.as_deref(),
                    fact_types: &params.fact_types,
                    access: &params.access,
                },
                request.budget,
                super::decayed_confidence,
                normalize_dt,
            )
            .await?
        }
        Some("map") => {
            build_map_view(
                ctx,
                &params.namespace,
                params.cutoff,
                request.budget,
                normalize_dt,
            )
            .await?
        }
        _ => {
            let query_opt = if params.cleaned_query.is_empty() {
                None
            } else {
                Some(params.cleaned_query.as_str())
            };
            let raw_query_opt = if request.query.trim().is_empty() {
                None
            } else {
                Some(request.query.as_str())
            };
            assemble_default_context(
                ctx,
                DefaultContextParams {
                    namespace: &params.namespace,
                    scope: &request.scope,
                    cutoff_iso: &params.cutoff_iso,
                    cutoff: params.cutoff,
                    raw_query_opt,
                    query_opt,
                    query_terms: &params.query_terms,
                    project_opt: params.project_opt.as_deref(),
                    fact_types: &params.fact_types,
                    budget: request.budget,
                    window_start: request.window_start,
                    window_end: request.window_end,
                    resolved_view_mode: params.resolved_view_mode_opt.as_deref(),
                    query_flags: &params.query_flags,
                    access: &params.access,
                },
            )
            .await?
        }
    };

    // --- Append recent experience for browse-like queries ---
    if params.resolved_view_mode_opt.as_deref() != Some("facets")
        && params.resolved_view_mode_opt.as_deref() != Some("wake_up")
        && params.resolved_view_mode_opt.as_deref() != Some("map")
        && params.cleaned_query.is_empty()
    {
        let appended = append_recent_experience_items(
            &mut results,
            ctx,
            RecentExperienceRequest {
                namespace: &params.namespace,
                scope: &request.scope,
                cutoff: params.cutoff,
                project: params.project_opt.as_deref(),
                access: &params.access,
                budget: request.budget,
                fact_types: &params.fact_types,
            },
        )
        .await?;

        if appended > 0 {
            ctx.logger.log(
                log_event(
                    "assemble_context.experience_appended",
                    json!({"scope": request.scope, "query": request.query}),
                    json!({"count": appended}),
                    Some(&params.access),
                    None,
                    None,
                ),
                LogLevel::Trace,
            );
        }
    }

    // --- Results logging, access tracking, cache store ---
    ctx.logger.log(
        log_event(
            "assemble_context.results",
            json!({
                "scope": request.scope,
                "query": request.query,
                "view_mode": params.resolved_view_mode_opt,
                "project": params.project_opt,
            }),
            json!({
                "count": results.len(),
                "retrieval_tiers": summarize_retrieval_tiers(&results),
                "supplemental_experience": supplemental_experience_count(&results),
            }),
            Some(&params.access),
            None,
            None,
        ),
        LogLevel::Trace,
    );

    track_fact_accesses(ctx, &results, &params.access).await;
    pipeline::store_cache(ctx, params.cache_key.clone(), &results).await;

    ctx.logger.log(
        log_event(
            "assemble_context.cache_set",
            json!({"scope": request.scope, "query": request.query, "budget": request.budget}),
            json!({"count": results.len()}),
            Some(&params.access),
            None,
            None,
        ),
        LogLevel::Trace,
    );

    let latency_ms = started_at.elapsed().as_secs_f64() * 1000.0;
    logging::maybe_record_query_log(
        ctx,
        &request,
        &results,
        false,
        latency_ms,
        &params.access,
        &query_log_diagnostics,
    )
    .await;

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_EMBEDDING_DIMENSION;
    use crate::service::EmbeddingProvider;
    use crate::storage::{DbClient, GraphDirection};
    use async_trait::async_trait;
    use chrono::Utc;
    use serde_json::{Value, json};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Seeds a `fact` record the way a real ingestion would, so the real
    /// retrieval SQL (full-text `search::score`, bi-temporal visibility)
    /// finds it identically to the canned mock records it replaces.
    async fn seed_context_fact(
        db_client: &Arc<crate::storage::SurrealDbClient>,
        fact_id: &str,
        fact_type: &str,
        content: &str,
        t_valid: &str,
        source_episode: &str,
        index_keys: &[&str],
    ) {
        let now = normalize_dt(Utc::now());
        let embedding = vec![0.0f64; DEFAULT_EMBEDDING_DIMENSION];
        db_client
            .create(
                fact_id,
                json!({
                    "fact_id": fact_id,
                    "fact_type": fact_type,
                    "content": content,
                    "quote": content,
                    "source_episode": format!("episode:{source_episode}"),
                    "t_valid": crate::service::normalize_dt(
                        chrono::DateTime::parse_from_rfc3339(t_valid)
                            .expect("t_valid")
                            .with_timezone(&Utc)
                    ),
                    "t_ingested": crate::service::normalize_dt(
                        chrono::DateTime::parse_from_rfc3339(t_valid)
                            .expect("t_ingested")
                            .with_timezone(&Utc)
                    ),
                    "confidence": 0.8,
                    "index_keys": index_keys,
                    "access_count": 0,
                    "entity_links": [],
                    "scope": "org",
                    "policy_tags": [],
                    "provenance": {"source_episode": format!("episode:{source_episode}")},
                    "embedding": embedding,
                    "embedding_provider": "legacy-test",
                    "embedding_model": "legacy-model",
                    "embedding_dimension": DEFAULT_EMBEDDING_DIMENSION,
                    "embedding_signature": Some("embsig:test"),
                    "embedding_updated_at": now,
                }),
                "org",
            )
            .await
            .expect("seed context fact");
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

            #[allow(clippy::too_many_arguments)]
            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
                _fact_types: &[String],
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

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
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

            async fn select_episodes_by_content(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
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
            &service.build_context(),
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
                compact: crate::tools::parsers::default_compact(),
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

            #[allow(clippy::too_many_arguments)]
            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
                _fact_types: &[String],
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

            async fn select_episodes_by_content(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
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
            &service.build_context(),
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
                compact: crate::tools::parsers::default_compact(),
            },
        )
        .await
        .expect("episode fallback context");

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].source_episode, "episode:doc");
        assert_eq!(items[0].retrieval_tier.as_deref(), Some("fallback"));
        assert!(items[0].content.contains("Hello World"));
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

            #[allow(clippy::too_many_arguments)]
            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
                _fact_types: &[String],
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

            async fn select_episodes_by_content(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
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
            &service.build_context(),
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
                compact: crate::tools::parsers::default_compact(),
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
        let db_client = Arc::new(crate::service::mock_db::MockDbClient::new());
        let service = crate::service::MemoryService::new(
            db_client.clone(),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &service.build_context(),
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
                compact: crate::tools::parsers::default_compact(),
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

            #[allow(clippy::too_many_arguments)]
            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
                _fact_types: &[String],
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

            async fn select_episodes_by_content(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
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
            &service.build_context(),
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
                compact: crate::tools::parsers::default_compact(),
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
        assert!(
            results[1].retrieval_tier.as_deref() == Some("graph")
                || results[1].rationale.contains("community:atlas"),
            "secondary expansion should remain the community-linked fact, even if Task 2 now surfaces it via graph expansion first; got tier={:?} rationale={}",
            results[1].retrieval_tier,
            results[1].rationale
        );
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

            #[allow(clippy::too_many_arguments)]
            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
                _fact_types: &[String],
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

            async fn select_episodes_by_content(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
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
            &service.build_context(),
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
                compact: crate::tools::parsers::default_compact(),
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

            #[allow(clippy::too_many_arguments)]
            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
                _fact_types: &[String],
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

            async fn select_episodes_by_content(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
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
            &service.build_context(),
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
                compact: crate::tools::parsers::default_compact(),
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

            #[allow(clippy::too_many_arguments)]
            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
                _fact_types: &[String],
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

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
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

            async fn select_episodes_by_content(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
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
            &service.build_context(),
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
                compact: crate::tools::parsers::default_compact(),
            },
        )
        .await
        .expect("assemble context");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].fact_id, "fact:semantic");
        assert!(results[0].rationale.contains("semantic similarity"));
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

            #[allow(clippy::too_many_arguments)]
            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
                _fact_types: &[String],
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

            async fn select_episodes_by_content(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
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
            &service.build_context(),
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
                compact: crate::tools::parsers::default_compact(),
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
        let db_client = Arc::new(
            crate::storage::SurrealDbClient::connect_in_memory_with_namespaces(
                &format!(
                    "experience_ranking_test_{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ),
                &["org".to_string()],
                "warn",
            )
            .await
            .expect("connect in memory db"),
        );
        db_client
            .apply_migrations("org")
            .await
            .expect("apply migrations");

        seed_context_fact(
            &db_client,
            "fact:generic-note",
            "note",
            "I need a hotel that can host our annual conference during the trip.",
            "2026-02-12T10:00:00Z",
            "generic-note",
            &[],
        )
        .await;
        seed_context_fact(
            &db_client,
            "fact:experience",
            "experience",
            "I usually prefer quieter hotels away from the city center, because I avoid nightlife-heavy properties.",
            "2026-02-13T10:00:00Z",
            "experience",
            &["hotel", "quiet", "nightlife"],
        )
        .await;

        let service = crate::service::MemoryService::new(
            db_client,
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &service.build_context(),
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
                compact: crate::tools::parsers::default_compact(),
            },
        )
        .await
        .expect("assemble context should succeed");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].fact_id, "fact:experience");
    }

    #[tokio::test]
    async fn assemble_context_uses_repeated_direct_topics_to_surface_implicit_preferences() {
        let db_client = Arc::new(
            crate::storage::SurrealDbClient::connect_in_memory_with_namespaces(
                &format!(
                    "implicit_experience_topic_test_{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ),
                &["org".to_string()],
                "warn",
            )
            .await
            .expect("connect in memory db"),
        );
        db_client
            .apply_migrations("org")
            .await
            .expect("apply migrations");

        seed_context_fact(
            &db_client,
            "fact:conference-hotel",
            "note",
            "I'm heading to a conference next month and need to book a hotel.",
            "2026-02-12T10:00:00Z",
            "conference-hotel",
            &[],
        )
        .await;
        seed_context_fact(
            &db_client,
            "fact:conference-hotel-shape",
            "note",
            "For the conference, I want a hotel that is not too tall.",
            "2026-02-11T10:00:00Z",
            "conference-hotel-shape",
            &[],
        )
        .await;
        seed_context_fact(
            &db_client,
            "fact:experience",
            "experience",
            "I usually prefer quieter hotels away from the city center, because I avoid nightlife-heavy properties.",
            "2026-02-13T10:00:00Z",
            "experience",
            &["hotel", "conference", "quiet", "nightlife"],
        )
        .await;

        let service = crate::service::MemoryService::new(
            db_client,
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &service.build_context(),
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
                compact: crate::tools::parsers::default_compact(),
            },
        )
        .await
        .expect("assemble context should succeed");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].fact_id, "fact:experience");
    }
}
