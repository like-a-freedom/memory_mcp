//! Context assembly operations — thin orchestrator.
//!
//! The heavy lifting lives in [`pipeline`]: parameter preparation,
//! cache operations, and the multi-tier default retrieval pipeline.
//! View-mode-specific builders are in [`views`].

use std::time::Instant;

use serde_json::json;

use crate::logging::LogLevel;
use crate::models::{AccessPayload, AssembleContextRequest, AssembledContextItem};

use crate::memory::retrieval_deps::AssembleContextDeps;

/// The retrieval pipeline's dependency set. Named here so the
/// retrieval helper modules read as depending on a retrieval seam
/// rather than on a shared container.
pub(crate) type RetrievalContext = AssembleContextDeps;

/// Confidence decay for one fact, delegating to [`crate::models::Fact`] as
/// the single source of truth rather than the `service::query` wrapper.
pub(crate) fn fact_decayed_confidence(
    fact: &crate::models::Fact,
    now: chrono::DateTime<chrono::Utc>,
) -> f64 {
    fact.decayed_confidence(now)
}
use crate::error::MemoryError;
use crate::platform::log_event::log_event;
use crate::shared::temporal::normalize_dt;

mod alias_expansion;
mod budget;
mod community;
mod experience;
mod filtering;
pub(crate) mod graph;
pub(crate) mod graph_reads;
pub(crate) mod graph_surprising;
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
mod types;
mod views;

use experience::{RecentExperienceRequest, append_recent_experience_items};
use logging::{summarize_retrieval_tiers, supplemental_experience_count};
use params::DefaultContextParams;
use pipeline::assemble_default_context;
use views::{build_facets_view, build_map_view, build_wake_up_view};

/// Whether an assembled item's id names a real fact record that
/// can carry access heat.
///
/// Several view modes synthesise items under a non-fact id
/// (`episode_fallback:`, `facet:`, `map:`). Those have no fact
/// record to update, and the fact-scoped access writer refuses
/// them, so attempting one is a validation refusal rather than a
/// failure. This predicate is the single place that decision is
/// made, so it is named and tested rather than inlined in the loop.
pub(crate) fn is_fact_access_trackable(fact_id: &str) -> bool {
    fact_id.starts_with("fact:")
}

/// Records fact access for each item, logging errors without failing the operation.
async fn track_fact_accesses(
    ctx: &RetrievalContext,
    items: &[AssembledContextItem],
    access: &AccessPayload,
) {
    for item in items {
        if !is_fact_access_trackable(&item.fact_id) {
            continue;
        }
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

/// Enters the retrieval seam for one context assembly.
pub async fn assemble_context(
    ctx: &AssembleContextDeps,
    request: AssembleContextRequest,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let access = AccessPayload::from_payload(request.access.clone());
    let command = crate::memory::api::RecallCommand {
        query: request.query.clone(),
        budget: request.budget,
        caller_id: access.and_then(|payload| payload.caller_id),
    };
    crate::memory::api::recall_context(
        &RetrievalPort { ctx, request },
        &crate::memory::retrieval_deps::RateLimitDeps {
            rate_limiter: &ctx.rate_limiter,
        },
        &command,
    )
    .await
}

/// Adapts the multi-tier retrieval pipeline to the memory-owned
/// recall port.
///
/// The port is the seam: the entry point above only charges the
/// access policy, and everything below runs against the
/// already-narrowed retrieval dependencies.
struct RetrievalPort<'a> {
    ctx: &'a AssembleContextDeps,
    request: AssembleContextRequest,
}

#[async_trait::async_trait]
impl crate::memory::api::ContextRetrievalPort for RetrievalPort<'_> {
    async fn retrieve(
        &self,
        _command: &crate::memory::api::RecallCommand,
    ) -> Result<Vec<AssembledContextItem>, MemoryError> {
        assemble_context_inner(self.ctx, self.request.clone()).await
    }
}

/// Assembles context after the outer service adapter has entered the
/// retrieval seam.
///
/// Orchestrates: parameter preparation → cache check → view-mode dispatch
/// (facets / wake_up / map / default multi-tier) → experience append →
/// cache store → query log. All logic is delegated to `pipeline` and `views`.
async fn assemble_context_inner(
    ctx: &RetrievalContext,
    request: AssembleContextRequest,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let started_at = Instant::now();
    let access = AccessPayload::from_payload(request.access.clone());

    pipeline::log_context_start(ctx, &request, access.as_ref());
    let params = pipeline::prepare_context_params(ctx, &request, access).await?;

    let query_log_diagnostics = logging::QueryLogDiagnostics {
        resolved_view_mode: params.resolved_view_mode_opt.as_deref(),
        query_flags: &params.query_flags.as_labels(),
    };

    // --- Cache check ---
    let cache_generation = match pipeline::check_cache(ctx, &params.cache_key).await {
        crate::platform::context_cache::ContextCacheLookup::Hit(cached) => {
            track_fact_accesses(ctx, &cached, &params.access).await;

            ctx.logger.log(
                log_event(
                    "assemble_context.cache_hit",
                    json!({"namespace": params.namespace, "query": request.query}),
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
        crate::platform::context_cache::ContextCacheLookup::Miss(generation) => generation,
    };

    ctx.logger.log(
        log_event(
            "assemble_context.cache_miss",
            json!({"namespace": params.namespace, "query": request.query, "budget": request.budget}),
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
                "namespace": params.namespace,
                "query": request.query,
                "budget": request.budget,
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
            build_facets_view(ctx, params.cutoff, request.budget, &params.access).await?
        }
        Some("wake_up") => {
            build_wake_up_view(
                ctx,
                views::FactFilterParams {
                    cutoff: params.cutoff,
                    fact_types: &params.fact_types,
                    access: &params.access,
                },
                request.budget,
                crate::memory::retrieval::fact_decayed_confidence,
                normalize_dt,
            )
            .await?
        }
        Some("map") => build_map_view(ctx, params.cutoff, request.budget, normalize_dt).await?,
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
                    cutoff_iso: &params.cutoff_iso,
                    cutoff: params.cutoff,
                    raw_query_opt,
                    query_opt,
                    query_terms: &params.query_terms,
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
                cutoff: params.cutoff,
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
                    json!({"namespace": params.namespace, "query": request.query}),
                    json!({"count": appended}),
                    Some(&params.access),
                    None,
                    None,
                ),
                LogLevel::Trace,
            );
        }
    }

    attach_reconciliation(ctx, &mut results).await?;
    // Post-assembly, before the cache: a cached list must carry the reorder
    // already, because a cache hit returns before any post-processing.
    results = ranking::demote_superseded(ranking::DemoteSupersededRequest { items: &results });

    // --- Results logging, access tracking, cache store ---
    ctx.logger.log(
        log_event(
            "assemble_context.results",
            json!({
                "namespace": params.namespace,
                "query": request.query,
                "view_mode": params.resolved_view_mode_opt,
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
    let cache_insert =
        pipeline::store_cache(ctx, cache_generation, params.cache_key.clone(), &results).await;
    let cache_status = match cache_insert {
        crate::platform::context_cache::CacheInsertOutcome::Stored => "stored",
        crate::platform::context_cache::CacheInsertOutcome::Oversized => "oversized",
        crate::platform::context_cache::CacheInsertOutcome::StaleGeneration => "stale_generation",
    };

    ctx.logger.log(
        log_event(
            "assemble_context.cache_set",
            json!({"namespace": params.namespace, "query": request.query, "budget": request.budget}),
            json!({"count": results.len(), "status": cache_status}),
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

/// Attach the claim relations of each assembled fact to its item.
///
/// One read for the whole pack — a per-item call would be N queries on the hot
/// path — and it runs *before* `pipeline::store_cache`, because the cache
/// stores `Vec<AssembledContextItem>`: items cached without relations would be
/// served with `reconciliation: None` forever.
///
/// Facts with no relations are left untouched rather than set to `Some(empty)`,
/// so `None` keeps meaning "this fact participates in nothing".
///
/// **A failed read fails the assembly.** This is deliberately asymmetric with
/// the steps beside it: `track_fact_accesses` and `store_cache` are best-effort
/// because losing them costs telemetry or a cache entry. Losing this read would
/// silently downgrade a disclosed pack to an undisclosed one, and at the
/// `evidence` stage the caller believes disclosure was served. Failing loudly
/// is the smaller error.
async fn attach_reconciliation(
    ctx: &RetrievalContext,
    items: &mut [AssembledContextItem],
) -> Result<(), MemoryError> {
    // Dedup on the borrow, not on a fresh `FactId` per item: this runs on
    // every cache miss, and allocating one String per element to discover an
    // id is already in the set is pure waste. One allocation per *distinct*
    // fact is the floor.
    let mut seen = std::collections::HashSet::new();
    let fact_ids: Vec<crate::models::FactId> = items
        .iter()
        .map(|item| item.fact_id.as_str())
        .filter(|id| seen.insert(*id))
        .map(crate::models::FactId::from)
        .collect();

    if fact_ids.is_empty() {
        return Ok(());
    }

    let read = crate::knowledge::api::relations_for_facts(&*ctx.relation_read, &fact_ids).await?;
    if read.relations.is_empty() {
        return Ok(());
    }

    let by_fact = crate::knowledge::api::reconciliation_metadata_by_fact(&read);
    if by_fact.is_empty() {
        return Ok(());
    }

    for item in items.iter_mut() {
        if let Some(metadata) = by_fact.get(&item.fact_id) {
            item.reconciliation = Some(metadata.clone());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_EMBEDDING_DIMENSION;
    use crate::service::EmbeddingProvider;
    use crate::storage::DbClient;
    use async_trait::async_trait;
    use chrono::Utc;
    use serde_json::{Value, json};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn only_real_fact_ids_are_access_trackable() {
        // A real fact record: heat is recorded.
        assert!(is_fact_access_trackable("fact:abc123"));

        // Synthesised view ids. Each of these reaches
        // `track_fact_accesses` in production (map, facets and
        // episode-fallback views), and each is refused by the
        // fact-scoped access writer, so none may be recorded.
        for synthetic in [
            "map:hub:entity:58c3bf0176e35089f2dcce6d",
            "map:community:community:atlas-team",
            "facet:uncategorized",
            "facet:persona",
            "episode_fallback:episode:02f58543a58ded9a15e12912",
        ] {
            assert!(
                !is_fact_access_trackable(synthetic),
                "{synthetic} is a synthesised id and must not be access-tracked"
            );
        }

        // A record kind that is not a fact, reached through the
        // generic id space, is equally untrackable.
        for other in ["edge:abc", "episode:abc", "entity:abc", ""] {
            assert!(
                !is_fact_access_trackable(other),
                "{other} is not a fact record id and must not be access-tracked"
            );
        }
    }

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
                    "t_valid": crate::shared::temporal::normalize_dt(
                        chrono::DateTime::parse_from_rfc3339(t_valid)
                            .expect("t_valid")
                            .with_timezone(&Utc)
                    ),
                    "t_ingested": crate::shared::temporal::normalize_dt(
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
                crate::knowledge::queries::FACT_TEMPORAL_FIELDS,
            )
            .await
            .expect("seed context fact");
    }

    #[tokio::test]
    async fn assemble_context_marks_term_fallback_results_with_fallback_tier() {
        let service = crate::service::MemoryService::new(
            Arc::new(
                crate::service::mock_db::MockDbClient::new().expect_query_with(
                    // The context store runs the full-text fact retrieval
                    // through the core `query` op, and dispatches per term, so
                    // the responder keys on `vars["query"]` — which is exactly
                    // what the old fake did, in its `query` body.
                    |sql| sql.contains("search::score"),
                    |_sql, vars| {
                        let query = vars
                            .and_then(|vars| vars.get("query").cloned())
                            .and_then(|v| v.as_str().map(str::to_string));
                        Ok(Value::Array(match query.as_deref() {
                            Some("atlas launch checklist") => vec![],
                            Some("atlas") | Some("launch") | Some("checklist") => vec![json!({
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
                        }))
                    },
                ),
            ),
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let items = assemble_context(
            &crate::memory::retrieval_deps::AssembleContextDeps::from(&service),
            AssembleContextRequest {
                query: "atlas launch checklist".to_string(),
                as_of: None,
                budget: 5,
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
        let service = crate::service::MemoryService::new(
            Arc::new(
                crate::service::mock_db::MockDbClient::new().expect_query_with(
                    |sql| sql.contains("FROM episode"),
                    // The context store runs the episode-content fallback
                    // through the core `query` op, so the assertion the old
                    // fake made inline — that the query carries the user's
                    // words — moves here rather than disappearing with it.
                    |_sql, vars| {
                        assert_eq!(
                            vars.and_then(|v| v.get("query")).cloned(),
                            Some(json!("hello world")),
                            "the episode fallback query must carry the user's words"
                        );
                        Ok(json!([{
                            "episode_id": "episode:doc",
                            "source_type": "document",
                            "source_id": "fixture:pdf",
                            "content": "Hello World from episode fallback.",
                            "t_ref": "2026-04-07T10:00:00Z",
                            "t_ingested": "2026-04-07T10:00:00Z",
                            "scope": "org",
                            "visibility_scope": "org",
                            "policy_tags": [],
                        }]))
                    },
                ),
            ),
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let items = assemble_context(
            &crate::memory::retrieval_deps::AssembleContextDeps::from(&service),
            AssembleContextRequest {
                query: "hello world".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
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
        /// Stays a hand-written `DbClient`, deliberately.
        ///
        /// This is the one retrieval fake the consolidation does not absorb,
        /// and the reason is the assertion rather than the responses: the test
        /// counts how many times each query shape is issued and asserts both
        /// counts, which is how it proves the summary path goes through the
        /// indexed community lookup and not through a per-fact fallback.
        /// `MockDbClient`'s responders are stateless — they see a SQL and
        /// answer — and a stateless responder has nowhere to record that it was
        /// called.
        ///
        /// Making it expressible would mean giving a responder a shared
        /// counter, which is a capability with no second user today. When one
        /// arrives, migrate this fake then; do not build the counter earlier.
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
                table: crate::storage::table_scope::OwnedTable,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                assert_eq!(table.as_str(), "fact");
                Ok(vec![])
            }

            #[allow(clippy::too_many_arguments)]
            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
                _temporal_fields: &[&str],
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
                _temporal_fields: &[&str],
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                sql: &str,
                vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                // The context store now runs the indexed community lookup and the
                // entity-link fact expansion through the core `query` op; serve
                // both here so the test can assert the DB-side paths are used.
                if sql.contains("FROM community") {
                    self.community_lookup_calls.fetch_add(1, Ordering::SeqCst);
                    if let Some(vars) = vars {
                        assert_eq!(vars["query"], json!("alice atlas"));
                    }
                    return Ok(json!([{
                        "community_id": "community:atlas",
                        "summary": "Alice and the Atlas project team",
                        "member_entities": ["entity:alice"]
                    }]));
                }
                if sql.contains("search::score") {
                    return Ok(json!([]));
                }
                if sql.contains("FROM fact") && sql.contains("CONTAINSANY") {
                    self.entity_link_fact_calls.fetch_add(1, Ordering::SeqCst);
                    if let Some(vars) = vars {
                        assert_eq!(vars["entity_links"], json!(["entity:alice"]));
                    }
                    return Ok(json!([
                        {
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
                        },
                        {
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
                        }
                    ]));
                }
                if sql.contains("FROM fact") && sql.contains("scope = $scope") {
                    panic!(
                        "community fact expansion should not use unfiltered select_facts_filtered fallback"
                    );
                }
                Ok(Value::Null)
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
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &crate::memory::retrieval_deps::AssembleContextDeps::from(&service),
            crate::models::AssembleContextRequest {
                query: "alice atlas".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
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
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &crate::memory::retrieval_deps::AssembleContextDeps::from(&service),
            crate::models::AssembleContextRequest {
                query: "alice platform".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
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
        let service = crate::service::MemoryService::new(
            Arc::new(
                crate::service::mock_db::MockDbClient::new()
                    // The entity-link fact expansion runs through the core
                    // `query` op; serve the community-linked fact here. The
                    // `entity_links` assertion moves into the responder, which
                    // is the part of the old `query` body that had teeth.
                    .expect_query_with(
                        |sql| sql.contains("FROM fact") && sql.contains("CONTAINSANY"),
                        |_sql, vars| {
                            assert_eq!(
                                vars.and_then(|v| v.get("entity_links")).cloned(),
                                Some(json!(["entity:atlas"])),
                                "the entity-link expansion must be bound to the anchor entity"
                            );
                            Ok(json!([{
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
                            }]))
                        },
                    )
                    .expect_query_with(
                        |sql| sql.contains("search::score"),
                        |_sql, vars| {
                            let query = vars
                                .and_then(|vars| vars.get("query").cloned())
                                .and_then(|v| v.as_str().map(str::to_string));
                            Ok(Value::Array(match query.as_deref() {
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
                            }))
                        },
                    ),
            ),
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &crate::memory::retrieval_deps::AssembleContextDeps::from(&service),
            crate::models::AssembleContextRequest {
                query: "atlas launch".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
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
            "secondary expansion should remain the community-linked fact, even if graph expansion surfaces it first; got tier={:?} rationale={}",
            results[1].retrieval_tier,
            results[1].rationale
        );
    }

    #[tokio::test]
    async fn assemble_context_orders_community_facts_by_matching_summary_relevance() {
        let service = crate::service::MemoryService::new(
            Arc::new(
                crate::service::mock_db::MockDbClient::new()
                    // The indexed community lookup runs through the core
                    // `query` op; serve ranked communities so the ranking
                    // pipeline can order their member facts by summary
                    // relevance. The `entity_links` assertion the old fake
                    // made inline is now inside the responder, so it still
                    // fails the test rather than disappearing with the body.
                    .expect_query_with(
                        |sql| sql.contains("FROM fact") && sql.contains("CONTAINSANY"),
                        |_sql, vars| {
                            assert_eq!(
                                vars.and_then(|v| v.get("entity_links")).cloned(),
                                Some(json!(["entity:alpha", "entity:beta"])),
                                "the member-fact expansion must be bound to both anchors"
                            );
                            Ok(json!([
                                {
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
                                },
                                {
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
                                }
                            ]))
                        },
                    )
                    .expect_query_with(
                        |sql| sql.contains("FROM community"),
                        |_sql, _| {
                            Ok(json!([
                                {
                                    "community_id": "community:alpha",
                                    "summary": "Alpha launch workstream",
                                    "member_entities": ["entity:alpha"],
                                    "ft_score": 20.0
                                },
                                {
                                    "community_id": "community:beta",
                                    "summary": "Beta launch workstream",
                                    "member_entities": ["entity:beta"],
                                    "ft_score": 10.0
                                }
                            ]))
                        },
                    ),
            ),
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &crate::memory::retrieval_deps::AssembleContextDeps::from(&service),
            crate::models::AssembleContextRequest {
                query: "launch workstream".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
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
        let service = crate::service::MemoryService::new(
            Arc::new(
                crate::service::mock_db::MockDbClient::new()
                    .expect_query_with(
                        |sql| sql.contains("FROM fact") && sql.contains("CONTAINSANY"),
                        |_sql, vars| {
                            assert_eq!(
                                vars.and_then(|v| v.get("entity_links")).cloned(),
                                Some(json!(["entity:alpha", "entity:beta"])),
                                "the member-fact expansion must be bound to both anchors"
                            );
                            Ok(json!([
                                {
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
                                },
                                {
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
                                }
                            ]))
                        },
                    )
                    // The inferred edge is served for `entity:beta` and the
                    // extracted one for `entity:alpha`, keyed on
                    // `vars["node_id"]` — the variable the traversal binds, not
                    // the table. This is the one fake that needed the
                    // responder to see the bound variables, which is why
                    // `expect_query_with` takes a predicate and a responder
                    // rather than one closure with an internal match.
                    .expect_query_with(
                        |sql| sql.contains("FROM edge"),
                        |_sql, vars| {
                            let node_id = vars
                                .and_then(|v| v.get("node_id").cloned())
                                .and_then(|v| v.as_str().map(str::to_string))
                                .unwrap_or_default();
                            Ok(Value::Array(match node_id.as_str() {
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
                            }))
                        },
                    )
                    .expect_query_with(
                        |sql| sql.contains("FROM community"),
                        |_sql, _| {
                            Ok(json!([
                                {
                                    "community_id": "community:beta",
                                    "summary": "Beta launch workstream",
                                    "member_entities": ["entity:beta"],
                                    "ft_score": 20.0
                                },
                                {
                                    "community_id": "community:alpha",
                                    "summary": "Alpha launch workstream",
                                    "member_entities": ["entity:alpha"],
                                    "ft_score": 10.0
                                }
                            ]))
                        },
                    ),
            ),
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &crate::memory::retrieval_deps::AssembleContextDeps::from(&service),
            crate::models::AssembleContextRequest {
                query: "launch workstream".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
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
        // The ANN retrieval runs through the core `query` op; serve the
        // semantically-similar fact so the provider-backed path is exercised
        // end to end. `expect_select_table_panic` carries the second half of
        // the old fake's contract: semantic retrieval must not fall back to
        // scanning the whole `fact` table.
        let semantic_db = crate::service::mock_db::MockDbClient::new()
            .expect_select_table_panic("fact")
            .expect_query_with(
                |sql| sql.contains("vector::similarity"),
                |_sql, _| {
                    let mut embedding = vec![0.0; DEFAULT_EMBEDDING_DIMENSION];
                    embedding[0] = 1.0;
                    Ok(json!([{
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
                    }]))
                },
            );

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
            Arc::new(semantic_db),
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
            Arc::new(SemanticEmbeddingProvider),
            crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
            Arc::new(crate::service::AnnoEntityExtractor::new().expect("anno extractor")),
        )
        .expect("service");

        let results = assemble_context(
            &crate::memory::retrieval_deps::AssembleContextDeps::from(&service),
            crate::models::AssembleContextRequest {
                query: "salary raise".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
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
        // Every method of the old hand-written fake returned the default a
        // bare `MockDbClient` returns — `None`, `vec![]`, `Value::Null`, `Ok(())`
        // — so it carried no behaviour at all. `MockDbClient` is now that
        // zero-configuration default, and the struct is gone.
        let service = crate::service::MemoryService::new(
            Arc::new(crate::service::mock_db::MockDbClient::new()),
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &crate::memory::retrieval_deps::AssembleContextDeps::from(&service),
            crate::models::AssembleContextRequest {
                query: "orphan community query".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
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
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &crate::memory::retrieval_deps::AssembleContextDeps::from(&service),
            crate::models::AssembleContextRequest {
                query: "Which hotel is better if I want somewhere quieter away from nightlife?"
                    .to_string(),
                as_of: Some(
                    chrono::DateTime::parse_from_rfc3339("2026-02-14T10:00:00Z")
                        .expect("timestamp")
                        .with_timezone(&Utc),
                ),
                budget: 1,
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
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &crate::memory::retrieval_deps::AssembleContextDeps::from(&service),
            crate::models::AssembleContextRequest {
                query: "Which venue would work best for my conference?".to_string(),
                as_of: Some(
                    chrono::DateTime::parse_from_rfc3339("2026-02-14T10:00:00Z")
                        .expect("timestamp")
                        .with_timezone(&Utc),
                ),
                budget: 1,
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
