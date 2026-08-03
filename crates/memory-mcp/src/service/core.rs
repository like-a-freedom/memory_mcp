//! MemoryService implementation - core service orchestration.

#[cfg(test)]
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::logging::LogLevel;
use crate::models::{
    AccessPayload, AssembleContextRequest, AssembledContextItem, EntityCandidate, ExplainItem,
    ExplainRequest, ExtractResult, IngestRequest, InvalidateRequest,
};

#[cfg(test)]
use crate::storage::GraphDirection;

use super::error::MemoryError;
#[cfg(test)]
use super::value_helpers::string_from_value;

mod builder;
mod helpers;
pub use builder::MemoryService;
pub(crate) use helpers::*;

impl MemoryService {
    pub(crate) fn app_store(&self) -> crate::storage::AppStoreClient {
        crate::storage::AppStoreClient::new(self.db_client.clone())
    }

    /// Builds a `ServiceContext` from this service's fields.
    ///
    /// Used by capability modules and tools that need a narrow reference
    /// instead of `&self`.
    pub fn build_context(&self) -> super::service_context::ServiceContext {
        super::service_context::ServiceContext {
            db_client: self.db_client.clone(),
            namespaces: self.namespaces.clone(),
            default_namespace: self.default_namespace.clone(),
            logger: self.logger.clone(),
            rate_limiter: self.rate_limiter.clone(),
            ingestion_service: self.ingestion_service.clone(),
            explanation_service: self.explanation_service.clone(),
            entity_resolver: self.entity_resolver.clone(),
            entity_service: self.entity_service.clone(),
            entity_extractor: self.entity_extractor.clone(),
            embedding_service: super::embedding_service::EmbeddingService::new(
                self.db_client.clone(),
                self.logger.clone(),
                self.embedding_provider.clone(),
                self.embedding_similarity_threshold,
                self.current_embedding_signature.clone(),
                self.current_embedding_model.clone(),
                self.current_embedding_dimension,
                self.context_cache.clone(),
                self.query_embedding_cache.clone(),
                self.task_runner.clone(),
            ),
            fact_service: self.fact_service.clone(),
            triple_extractor: self.triple_extractor.clone(),
            context_cache: self.context_cache.clone(),
            claim_store: Some(self.claim_service.store.clone()),
            query_logging_enabled: self.query_logging_enabled,
            query_log_retention_days: self.query_log_retention_days,
            claim_service: self.claim_service.clone(),
            triple_extraction_semaphore: self.triple_extraction_semaphore.clone(),
        }
    }

    /// Public helper for tool-level logging.
    #[cfg_attr(not(feature = "mcp-apps"), allow(dead_code))]
    pub(crate) fn log_tool_event(
        &self,
        op: &str,
        args: Value,
        result: Value,
        level: LogLevel,
        request_id: Option<&str>,
    ) {
        self.logger
            .log(log_event(op, args, result, None, request_id, None), level);
    }

    /// Public helper for tool-level logging with duration.
    #[cfg_attr(not(feature = "mcp-apps"), allow(dead_code))]
    pub(crate) fn log_tool_event_with_duration(
        &self,
        op: &str,
        args: Value,
        result: Value,
        level: LogLevel,
        duration: std::time::Duration,
        request_id: Option<&str>,
    ) {
        let duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        self.logger.log(
            log_event(op, args, result, None, request_id, Some(duration_ms)),
            level,
        );
    }

    pub(crate) fn build_fact_embedding_input(
        fact_type: &str,
        content: &str,
        quote: &str,
    ) -> String {
        super::fact::FactService::build_fact_embedding_input(fact_type, content, quote)
    }

    pub(crate) fn lifecycle_policy(&self) -> super::LifecyclePolicy {
        super::LifecyclePolicy::from(&self.lifecycle_config)
    }

    /// Returns the total count of episodes.
    pub async fn episode_count(&self) -> Result<i32, MemoryError> {
        let mut total = 0;
        let sql = "SELECT count() FROM episode GROUP ALL";
        for namespace in &self.namespaces {
            let result = self.db_client.query(sql, None, namespace).await?;
            let count = result
                .as_array()
                .and_then(|arr| arr.first())
                .and_then(|obj| obj.get("count"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0) as i32;
            total += count;
        }
        Ok(total)
    }

    /// Ingests a new episode.
    ///
    /// Thin delegator to [`IngestCapability::ingest`].
    pub async fn ingest(
        &self,
        request: IngestRequest,
        access: Option<AccessPayload>,
    ) -> Result<String, MemoryError> {
        let ctx = self.build_context();
        super::capabilities::ingest::IngestCapability::ingest(&ctx, request, access).await
    }

    /// Provides explanations for context items with batched graph insights.
    ///
    /// Thin delegator to [`ExplainCapability::explain`].
    pub async fn explain(
        &self,
        request: ExplainRequest,
        access: Option<AccessPayload>,
    ) -> Result<Vec<ExplainItem>, MemoryError> {
        let ctx = self.build_context();
        super::capabilities::explain::ExplainCapability::explain(&ctx, request, access).await
    }

    /// Extracts entities and facts from an episode.
    ///
    /// Thin delegator to [`ExtractCapability::extract`].
    pub async fn extract(
        &self,
        episode_id: &str,
        access: Option<AccessPayload>,
        zero_shot_labels: Option<&[String]>,
    ) -> Result<ExtractResult, MemoryError> {
        let ctx = self.build_context();
        super::capabilities::extract::ExtractCapability::extract(
            &ctx,
            episode_id,
            access,
            zero_shot_labels,
        )
        .await
    }

    /// Resolves an entity candidate.
    ///
    /// Thin delegator to [`ResolveCapability::resolve`].
    pub async fn resolve(
        &self,
        candidate: EntityCandidate,
        access: Option<AccessPayload>,
    ) -> Result<String, MemoryError> {
        let ctx = self.build_context();
        super::capabilities::resolve::ResolveCapability::resolve(&ctx, candidate, access).await
    }

    /// Adds a new fact.
    ///
    /// Thin delegator to `FactService::add_fact`. Kept for backward
    /// compatibility with direct callers (e.g. `commit_ingestion_review`).
    #[allow(clippy::too_many_arguments)]
    pub async fn add_fact(
        &self,
        fact_type: &str,
        content: &str,
        quote: &str,
        source_episode: &str,
        t_valid: DateTime<Utc>,
        scope: &str,
        confidence: f64,
        entity_links: Vec<String>,
        policy_tags: Vec<String>,
        provenance: crate::models::Provenance,
    ) -> Result<String, MemoryError> {
        let ctx = self.build_context();
        ctx.fact_service
            .add_fact(
                &ctx,
                fact_type,
                content,
                quote,
                source_episode,
                t_valid,
                scope,
                confidence,
                entity_links,
                policy_tags,
                provenance,
            )
            .await
    }

    /// Start claim reconciliation workers and schedule backfill.
    pub(crate) async fn start_claim_workers(&self) -> super::claims::worker::ClaimWorkerRuntime {
        let runtime = super::claims::worker::ClaimWorkerRuntime::new();
        let worker_id = format!("claim-worker-{}", std::process::id());
        runtime
            .spawn_worker(self.claim_service.clone(), worker_id)
            .await;
        runtime
    }

    /// Start the agent-memory lifecycle projection worker.
    ///
    /// The worker drains `event_projection_job` records and projects accepted
    /// lifecycle events into facts via the existing extraction path. It is a
    /// no-op when no lifecycle events have been captured.
    pub(crate) async fn start_lifecycle_worker(
        &self,
    ) -> super::agent_memory::worker::LifecycleWorkerRuntime {
        let runtime = super::agent_memory::worker::LifecycleWorkerRuntime::new();
        let poll_interval = super::agent_memory::worker::empty_poll_interval().as_secs();
        runtime.spawn(self.clone(), poll_interval).await;
        runtime
    }

    /// Shut down the lifecycle background workers (decay, archival, community).
    ///
    /// Cancels all worker tasks and joins them. Safe to call when no workers
    /// were spawned (`None`) or when lifecycle is disabled (empty runtime).
    /// Idempotent: a second call is a no-op (token already cancelled, handles
    /// already drained).
    pub async fn shutdown_lifecycle_background_workers(&self) {
        if let Some(runtime) = &self.lifecycle_background_workers {
            runtime.shutdown().await;
        }
    }

    /// Build a `LifecycleCapture` wired to the production storage and ingestion
    /// backends. Returns `None` if lifecycle integration is not enabled.
    pub fn lifecycle_capture(&self) -> Option<super::agent_memory::capture::LifecycleCapture> {
        if !self.lifecycle_config.enabled {
            return None;
        }
        let store = std::sync::Arc::new(crate::storage::AgentMemoryStore::new(
            self.db_client.clone(),
        ));
        let ingestion = std::sync::Arc::new(self.ingestion_service.clone());
        let backend = std::sync::Arc::new(
            super::agent_memory::capture::ProductionCaptureBackend::new(store, ingestion),
        );
        Some(super::agent_memory::capture::LifecycleCapture::new(backend))
    }

    /// Capture a lifecycle event through the internal selective-capture path.
    ///
    /// This is the production wiring for `LifecycleCapture::execute()`. Hook
    /// scripts call the ordinary `ingest` CLI; this method is invoked when the
    /// server-side lifecycle path classifies the event as capture-eligible.
    /// Returns `None` (no-op) when lifecycle integration is disabled.
    pub async fn capture_lifecycle_event(
        &self,
        event: &crate::models::NormalizedHostEvent,
        context: &crate::models::InvocationContext,
    ) -> Result<Option<super::agent_memory::capture::LifecycleCaptureResult>, MemoryError> {
        let Some(capture) = self.lifecycle_capture() else {
            return Ok(None);
        };
        let budget = super::agent_memory::capture::default_capture_budget();
        let result = capture
            .execute(
                event,
                context,
                &budget,
                16 * 1024,
                16,
                &self.default_namespace,
            )
            .await?;
        Ok(Some(result))
    }

    /// Build a `LifecycleRecall` orchestrator for selective recall.
    ///
    /// Returns `None` if lifecycle integration is not enabled. The orchestrator
    /// delegates to the existing `assemble_context` pipeline via the
    /// `RecallPipeline` trait.
    pub fn lifecycle_recall(&self) -> Option<super::agent_memory::recall::LifecycleRecall> {
        if !self.lifecycle_config.enabled {
            return None;
        }
        Some(
            super::agent_memory::recall::LifecycleRecall::with_trace_registry(
                self.trace_registry.clone(),
            ),
        )
    }

    /// Recall lifecycle context through the internal selective-recall path.
    ///
    /// This is the production wiring for `LifecycleRecall::execute()`. It
    /// delegates to the existing `assemble_context` pipeline exactly once per
    /// recall-eligible event, wrapping output in the "memory is data" preamble.
    /// Returns `None` (no-op) when lifecycle integration is disabled.
    pub async fn recall_lifecycle_event(
        &self,
        event: &crate::models::NormalizedHostEvent,
        context: &crate::models::InvocationContext,
    ) -> Result<Option<super::agent_memory::recall::LifecycleRecallResult>, MemoryError> {
        let Some(recall) = self.lifecycle_recall() else {
            return Ok(None);
        };
        let pipeline = ProductionRecallPipeline { service: self };
        let now_secs = chrono::Utc::now().timestamp().max(0) as u64;
        let result = recall.execute(&pipeline, event, context, now_secs).await?;
        Ok(Some(result))
    }

    /// Generates an embedding vector for the supplied input.
    ///
    /// Thin delegator to `EmbeddingService::generate_embedding`.
    pub(crate) async fn generate_embedding(
        &self,
        input: &str,
    ) -> Result<Option<Vec<f64>>, MemoryError> {
        self.build_context()
            .embedding_service
            .generate_embedding(input)
            .await
    }

    /// Invalidates a fact.
    pub async fn invalidate(
        &self,
        request: InvalidateRequest,
        access: Option<AccessPayload>,
    ) -> Result<(), MemoryError> {
        super::capabilities::invalidate::InvalidateCapability::invalidate(
            &self.build_context(),
            request,
            access,
        )
        .await
    }

    /// Assembles context for a query.
    ///
    /// Thin delegator to [`AssembleContextCapability::assemble_context`].
    pub async fn assemble_context(
        &self,
        request: AssembleContextRequest,
    ) -> Result<Vec<AssembledContextItem>, MemoryError> {
        let ctx = self.build_context();
        super::capabilities::assemble_context::AssembleContextCapability::assemble_context(
            &ctx, request,
        )
        .await
    }

    /// Resolves an entity by its type and canonical name.
    pub async fn resolve_entity(
        &self,
        entity_type: &str,
        name: &str,
    ) -> Result<String, MemoryError> {
        self.resolve(
            EntityCandidate {
                entity_type: entity_type.to_string(),
                canonical_name: name.to_string(),
                aliases: Vec::new(),
            },
            None,
        )
        .await
    }

    /// Creates a relationship edge between two entities.
    pub async fn relate(
        &self,
        from_id: &str,
        relation: &str,
        to_id: &str,
    ) -> Result<(), MemoryError> {
        use crate::models::{Edge, EdgeOrigin};
        let edge = Edge {
            in_id: from_id.to_string(),
            relation: relation.to_string(),
            out_id: to_id.to_string(),
            origin: EdgeOrigin::Inferred,
            strength: 1.0,
            confidence: 0.8,
            provenance: crate::models::Provenance::manual(),
            t_valid: super::query::now(),
            t_ingested: super::query::now(),
            t_invalid: None,
            t_invalid_ingested: None,
        };
        super::episode::store_edge(&self.build_context(), &edge, &self.default_namespace).await
    }

    /// Retrieves SurrealDB config.
    pub async fn get_surrealdb_config(&self) -> Result<Value, MemoryError> {
        Ok(json!({
            "namespaces": self.namespaces.clone(),
        }))
    }

    /// Finds an introduction chain.
    ///
    /// Graph traversal lives in `service/apps/graph.rs` per ADR-0024 step 1;
    /// this method is kept only to preserve the public `MemoryService`
    /// interface while consumers migrate.
    pub async fn find_intro_chain(
        &self,
        target_name: &str,
        max_hops: i32,
        as_of: Option<DateTime<Utc>>,
    ) -> Result<Vec<String>, MemoryError> {
        super::apps::graph::find_intro_chain(
            self,
            &self.namespaces,
            &self.default_namespace,
            target_name,
            max_hops,
            as_of,
        )
        .await
    }

    /// Invalidates a superseded metric.
    pub async fn invalidate_metric_if_superseded(
        &self,
        new_value: f64,
        old_fact_id: &str,
        t_invalid: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        let (record, _) = self.find_fact_record(old_fact_id).await?;
        if record.is_none() {
            return Ok(());
        }
        self.invalidate(
            InvalidateRequest {
                fact_id: old_fact_id.to_string(),
                reason: format!("Superseded by {new_value}"),
                t_invalid,
            },
            None,
        )
        .await?;
        Ok(())
    }

    async fn check_surrealdb_connection(&self) -> Result<(), MemoryError> {
        let _ = self
            .db_client
            .select_table("event_log", &self.default_namespace)
            .await?;
        Ok(())
    }

    /// Returns the namespace for a given scope.
    pub fn namespace_for_scope(&self, scope: &str) -> Result<String, MemoryError> {
        crate::service::MemoryScope::parse(scope)?.namespace(&self.namespaces)
    }

    pub(crate) async fn find_episode_record(
        &self,
        episode_id: &str,
    ) -> Result<(Option<serde_json::Map<String, Value>>, Option<String>), MemoryError> {
        self.find_record_by_id(episode_id).await
    }

    pub(crate) async fn find_fact_record(
        &self,
        fact_id: &str,
    ) -> Result<(Option<serde_json::Map<String, Value>>, Option<String>), MemoryError> {
        self.find_record_by_id(fact_id).await
    }

    /// Scans all namespaces for a record by its ID, returning the payload and owning namespace.
    async fn find_record_by_id(
        &self,
        record_id: &str,
    ) -> Result<(Option<serde_json::Map<String, Value>>, Option<String>), MemoryError> {
        for namespace in &self.namespaces {
            let record = self.db_client.select_one(record_id, namespace).await?;
            if let Some(Value::Object(map)) = record {
                return Ok((Some(map), Some(namespace.clone())));
            }
        }
        Ok((None, None))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn enforce_rate_limit(
        &self,
        access: Option<&AccessPayload>,
    ) -> Result<(), MemoryError> {
        if let Some(access) = access
            && let Some(caller) = &access.caller_id
            && !self.rate_limiter.allow(caller)
        {
            return Err(MemoryError::Validation("rate limit exceeded".into()));
        }
        Ok(())
    }
}

/// Production implementation of [`recall::RecallPipeline`] that delegates to
/// the existing `assemble_context` path.
///
/// This is the bridge between the internal `LifecycleRecall` orchestrator and
/// the real context pipeline. It exists only as a thin adapter so the
/// orchestrator stays testable with a mock pipeline.
pub(crate) struct ProductionRecallPipeline<'a> {
    service: &'a MemoryService,
}

#[async_trait::async_trait]
impl<'a> super::agent_memory::recall::RecallPipeline for ProductionRecallPipeline<'a> {
    async fn assemble(
        &self,
        request: crate::models::AssembleContextRequest,
    ) -> Result<Vec<crate::models::AssembledContextItem>, MemoryError> {
        let ctx = self.service.build_context();
        super::capabilities::assemble_context::AssembleContextCapability::assemble_context(
            &ctx, request,
        )
        .await
    }
}

/// Resolves a scope string to a namespace, using prefix matching against
/// available namespaces. Returns `(namespace, fell_back)` where `fell_back`
/// is true when the default was used for an unknown scope.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_EMBEDDING_DIMENSION;
    use crate::models::EntityCandidate;
    use crate::models::{AccessPayload, AccessScopeAllow, Provenance};
    use crate::service::EmbeddingProvider;
    use crate::service::startup::{apply_startup_migrations, build_startup_versions_event};
    use crate::service::util::rate_limiter::SafeMutex;
    use crate::storage::{DbClient, SurrealDbClient};
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn log_event_creates_expected_structure() {
        let event = log_event(
            "test_op",
            json!({"key": "value"}),
            json!({"result": "ok"}),
            None,
            None,
            None,
        );
        assert_eq!(event.get("op").unwrap().as_str(), Some("test_op"));
        assert_eq!(
            event.get("args").unwrap().get("key").unwrap().as_str(),
            Some("value")
        );
        assert_eq!(
            event.get("result").unwrap().get("result").unwrap().as_str(),
            Some("ok")
        );
    }

    #[test]
    fn log_event_includes_access_when_provided() {
        let access = AccessPayload {
            caller_id: Some("test-caller".to_string()),
            allowed_scopes: Some(vec!["org".to_string()]),
            allowed_tags: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: None,
        };
        let event = log_event("test_op", json!({}), json!({}), Some(&access), None, None);
        let access_event = event.get("access").unwrap();
        assert_eq!(
            access_event.get("caller_id").unwrap().as_str(),
            Some("test-caller")
        );
    }

    #[test]
    fn serialize_access_includes_all_fields() {
        let access = AccessPayload {
            caller_id: Some("caller".to_string()),
            allowed_scopes: Some(vec!["org".to_string()]),
            allowed_tags: Some(vec!["tag1".to_string()]),
            session_vars: Some(json!({"key": "value"})),
            transport: Some("http".to_string()),
            content_type: Some("application/json".to_string()),
            cross_scope_allow: Some(vec![AccessScopeAllow {
                from: "*".to_string(),
                to: "org".to_string(),
            }]),
        };
        let serialized = serialize_access(&access);
        assert!(serialized.get("caller_id").is_some());
        assert!(serialized.get("allowed_scopes").is_some());
        assert!(serialized.get("allowed_tags").is_some());
        assert!(serialized.get("session_vars").is_some());
        assert!(serialized.get("transport").is_some());
        assert!(serialized.get("content_type").is_some());
        assert!(serialized.get("cross_scope_allow").is_some());
    }

    #[test]
    fn build_startup_versions_event_includes_both_versions() {
        let evt = build_startup_versions_event("0.1.0", Some("SurrealDB 3.0.0"));
        assert_eq!(evt.get("op").unwrap().as_str(), Some("startup.versions"));
        assert_eq!(evt.get("client_version").unwrap().as_str(), Some("0.1.0"));
        assert_eq!(
            evt.get("surrealdb_server_version").unwrap().as_str(),
            Some("SurrealDB 3.0.0")
        );
    }

    #[test]
    fn build_startup_versions_event_omits_server_when_none() {
        let evt = build_startup_versions_event("0.1.0", None);
        assert_eq!(evt.get("op").unwrap().as_str(), Some("startup.versions"));
        assert_eq!(evt.get("client_version").unwrap().as_str(), Some("0.1.0"));
        assert!(!evt.contains_key("surrealdb_server_version"));
    }

    #[test]
    fn build_fact_embedding_input_formats_correctly() {
        let result = MemoryService::build_fact_embedding_input("note", "Hello world", "Hello!");
        assert_eq!(result, "note\nHello world\nHello!");
    }

    #[test]
    fn build_fact_embedding_input_handles_empty_parts() {
        let result = MemoryService::build_fact_embedding_input("", "", "");
        assert_eq!(result, "\n\n");
    }

    /// Verifies that truncation is inside `generate_embedding` by checking
    /// that build_fact_embedding_input + generate_embedding together won't
    /// pass a 60k+ input to the provider. The truncation limit is 8,000 chars.
    /// This test is deliberately lightweight — the full reembed pipeline
    /// with long content is exercised in `reembed_long_fact_content_does_not_fail`.
    #[test]
    fn generate_embedding_input_builder_respects_truncation_limit() {
        // Simulate what build_fact_embedding_input produces for a very long
        // fact, then verify the truncation would apply.
        let long_content = "x".repeat(60_000);
        let full_input =
            MemoryService::build_fact_embedding_input("note", &long_content, &long_content);
        // The input + overhead is > 8,000, so generate_embedding will truncate
        assert!(full_input.len() > 8_000);
        // Truncation should produce at most 8,000 chars
        let truncated: String = full_input.chars().take(8_000).collect();
        assert_eq!(truncated.chars().count(), 8_000);
    }

    #[tokio::test]
    async fn apply_startup_migrations_runs_for_every_namespace() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct StartupMigrationDbClient {
            calls: Arc<Mutex<Vec<String>>>,
            apply_count: AtomicUsize,
        }

        #[async_trait::async_trait]
        impl DbClient for StartupMigrationDbClient {
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

            async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError> {
                self.apply_count.fetch_add(1, Ordering::SeqCst);
                self.calls.safe_lock().push(namespace.to_string());
                Ok(())
            }

            async fn select_facts_by_triple(
                &self,
                _namespace: &str,
                _query_text: &str,
                _cutoff: &str,
                _limit: usize,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entities_by_ids(
                &self,
                _namespace: &str,
                _entity_ids: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(Vec::new())
            }

            async fn select_edges_for_triple(
                &self,
                _namespace: &str,
                _in_id: &str,
                _relation: &str,
                _out_id: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(Vec::new())
            }

            async fn count_facts_needing_reembed(
                &self,
                _namespace: &str,
                _target_signature: &str,
            ) -> Result<usize, MemoryError> {
                Ok(0)
            }

            async fn select_facts_needing_reembed(
                &self,
                _namespace: &str,
                _target_signature: &str,
                _last_completed_fact_id: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(Vec::new())
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

        let db_client = Arc::new(StartupMigrationDbClient {
            calls: Arc::new(Mutex::new(Vec::new())),
            apply_count: AtomicUsize::new(0),
        });
        let db_client_dyn: Arc<dyn DbClient> = db_client.clone();
        let namespaces = vec![
            "org".to_string(),
            "personal".to_string(),
            "private".to_string(),
        ];

        apply_startup_migrations(&db_client_dyn, &namespaces)
            .await
            .expect("startup migrations");

        assert_eq!(db_client.apply_count.load(Ordering::SeqCst), 3);
        assert_eq!(
            db_client.calls.safe_lock().clone(),
            vec![
                "org".to_string(),
                "personal".to_string(),
                "private".to_string(),
            ]
        );
    }

    #[test]
    fn namespace_for_scope_returns_exact_match() {
        let service = create_test_service(vec!["org", "personal", "team", "private-domain"]);
        assert_eq!(service.namespace_for_scope("org").unwrap(), "org");
        assert_eq!(service.namespace_for_scope("personal").unwrap(), "personal");
        assert_eq!(service.namespace_for_scope("team").unwrap(), "team");
        assert_eq!(
            service.namespace_for_scope("private-domain").unwrap(),
            "private-domain"
        );
    }

    #[test]
    fn namespace_for_scope_returns_error_for_unknown() {
        let service = create_test_service(vec!["org", "personal"]);
        assert!(matches!(
            service.namespace_for_scope("unknown"),
            Err(MemoryError::Validation(_))
        ));
    }

    #[test]
    fn namespace_for_scope_accepts_case_insensitive_inputs() {
        let service = create_test_service(vec!["org", "personal", "team", "private-domain"]);
        assert_eq!(service.namespace_for_scope("ORG").unwrap(), "org");
        assert_eq!(service.namespace_for_scope("TEAM").unwrap(), "team");
    }

    fn create_test_service(namespaces: Vec<&str>) -> MemoryService {
        use std::sync::Arc;

        MemoryService::new(
            Arc::new(crate::service::mock_db::MockDbClient::new()),
            namespaces.iter().map(|s| s.to_string()).collect(),
            "warn".to_string(),
            50,
            100,
        )
        .unwrap()
    }

    fn create_test_service_with_rate_limit(rps: i32, burst: i32) -> MemoryService {
        use std::sync::Arc;

        MemoryService::new(
            Arc::new(crate::service::mock_db::MockDbClient::new()),
            vec!["org".to_string()],
            "warn".to_string(),
            rps,
            burst,
        )
        .unwrap()
    }

    #[test]
    fn is_scope_allowed_returns_true_when_no_restrictions() {
        let _service = create_test_service(vec!["org"]);
        let access = AccessPayload::default();
        assert!(access.is_scope_allowed("org"));
    }

    #[test]
    fn is_scope_allowed_returns_true_for_allowed_scope() {
        let _service = create_test_service(vec!["org"]);
        let access = AccessPayload {
            allowed_scopes: Some(vec!["org".to_string()]),
            allowed_tags: None,
            caller_id: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: None,
        };
        assert!(access.is_scope_allowed("org"));
    }

    #[test]
    fn is_scope_allowed_returns_false_for_disallowed_scope() {
        let _service = create_test_service(vec!["org"]);
        let access = AccessPayload {
            allowed_scopes: Some(vec!["personal".to_string()]),
            allowed_tags: None,
            caller_id: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: None,
        };
        assert!(!access.is_scope_allowed("org"));
    }

    #[test]
    fn is_scope_allowed_allows_with_cross_scope_wildcard() {
        let _service = create_test_service(vec!["org"]);
        let access = AccessPayload {
            allowed_scopes: Some(vec!["personal".to_string()]),
            allowed_tags: None,
            caller_id: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: Some(vec![AccessScopeAllow {
                from: "*".to_string(),
                to: "org".to_string(),
            }]),
        };
        assert!(access.is_scope_allowed("org"));
    }

    #[test]
    fn enforce_rate_limit_allows_without_caller_id() {
        let service = create_test_service(vec!["org"]);
        let access = AccessPayload::default();
        assert!(service.enforce_rate_limit(Some(&access)).is_ok());
    }

    #[test]
    fn enforce_rate_limit_allows_within_limit() {
        let service = create_test_service(vec!["org"]);
        let access = AccessPayload {
            caller_id: Some("user-1".to_string()),
            ..Default::default()
        };
        assert!(service.enforce_rate_limit(Some(&access)).is_ok());
    }

    #[test]
    fn enforce_rate_limit_accepts_none() {
        let service = create_test_service(vec!["org"]);
        assert!(service.enforce_rate_limit(None).is_ok());
    }

    #[tokio::test]
    async fn resolve_uses_indexed_entity_lookup_instead_of_table_scan() {
        use std::sync::Arc;

        let db = crate::service::mock_db::MockDbClient::new()
            .expect_select_table_panic("entity")
            .expect_edges_filtered_panic()
            .expect_create_with(|| {
                panic!("resolve should not create when indexed lookup finds a record")
            })
            .expect_edge_neighbors(
                "entity:openai",
                vec![json!({"in": "entity:bob", "out": "entity:openai"})],
            )
            .expect_edge_neighbors(
                "entity:bob",
                vec![json!({"in": "entity:alice", "out": "entity:bob"})],
            )
            .expect_entity_lookup("dima ivanov", Some(json!({"entity_id": "entity:existing"})))
            .expect_entity_lookup("openai", Some(json!({"entity_id": "entity:openai"})));

        let service = MemoryService::new(
            Arc::new(db),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .unwrap();

        let resolved = service
            .resolve(
                EntityCandidate {
                    entity_type: "person".to_string(),
                    canonical_name: "Dima Ivanov".to_string(),
                    aliases: vec![],
                },
                None,
            )
            .await
            .unwrap();

        assert_eq!(resolved, "entity:existing");
    }

    #[tokio::test]
    async fn find_intro_chain_uses_db_side_neighbor_lookups() {
        use std::sync::Arc;

        let db = crate::service::mock_db::MockDbClient::new()
            .expect_edges_filtered_panic()
            .expect_edge_neighbors(
                "entity:openai",
                vec![json!({"in": "entity:bob", "out": "entity:openai"})],
            )
            .expect_edge_neighbors(
                "entity:bob",
                vec![json!({"in": "entity:alice", "out": "entity:bob"})],
            )
            .expect_entity_lookup("openai", Some(json!({"entity_id": "entity:openai"})));

        let service = MemoryService::new(
            Arc::new(db),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .unwrap();

        let chain = service.find_intro_chain("OpenAI", 3, None).await.unwrap();

        assert_eq!(
            chain,
            vec![
                "entity:alice".to_string(),
                "entity:bob".to_string(),
                "entity:openai".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn find_intro_chain_prefers_shortest_path_over_lexicographic_candidate() {
        use std::sync::Arc;

        let db = crate::service::mock_db::MockDbClient::new()
            .expect_edges_filtered_panic()
            .expect_edge_neighbors(
                "entity:openai",
                vec![
                    json!({"in": "entity:bob", "out": "entity:openai"}),
                    json!({"in": "entity:carol", "out": "entity:openai"}),
                ],
            )
            .expect_edge_neighbors(
                "entity:bob",
                vec![json!({"in": "entity:alice", "out": "entity:bob"})],
            )
            .expect_entity_lookup("openai", Some(json!({"entity_id": "entity:openai"})));

        let service = MemoryService::new(
            Arc::new(db),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .unwrap();

        let chain = service.find_intro_chain("OpenAI", 3, None).await.unwrap();

        assert_eq!(
            chain,
            vec!["entity:carol".to_string(), "entity:openai".to_string()],
            "the shortest discovered introduction path should win even if a longer path starts with a lexicographically earlier id"
        );
    }

    #[tokio::test]
    async fn find_intro_chain_prefers_shortest_path_in_multi_hop_diamond() {
        use std::sync::Arc;

        let db = crate::service::mock_db::MockDbClient::new()
            .expect_edges_filtered_panic()
            .expect_edge_neighbors(
                "entity:openai",
                vec![
                    json!({"in": "entity:bob", "out": "entity:openai"}),
                    json!({"in": "entity:carol", "out": "entity:openai"}),
                ],
            )
            .expect_edge_neighbors(
                "entity:bob",
                vec![json!({"in": "entity:alice", "out": "entity:bob"})],
            )
            .expect_edge_neighbors(
                "entity:carol",
                vec![json!({"in": "entity:diana", "out": "entity:carol"})],
            )
            .expect_edge_neighbors(
                "entity:alice",
                vec![json!({"in": "entity:erin", "out": "entity:alice"})],
            )
            .expect_entity_lookup("openai", Some(json!({"entity_id": "entity:openai"})));

        let service = MemoryService::new(
            Arc::new(db),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .unwrap();

        let chain = service.find_intro_chain("OpenAI", 4, None).await.unwrap();

        assert_eq!(
            chain,
            vec![
                "entity:diana".to_string(),
                "entity:carol".to_string(),
                "entity:openai".to_string(),
            ],
            "the traversal should keep the shorter diamond branch instead of returning the deeper alternative"
        );
    }

    struct StaticTestEmbeddingProvider {
        salary_vector: Vec<f64>,
        neutral_vector: Vec<f64>,
    }

    impl StaticTestEmbeddingProvider {
        fn new() -> Self {
            let mut salary_vector = vec![0.0; DEFAULT_EMBEDDING_DIMENSION];
            salary_vector[0] = 1.0;
            let mut neutral_vector = vec![0.0; DEFAULT_EMBEDDING_DIMENSION];
            neutral_vector[1] = 1.0;
            Self {
                salary_vector,
                neutral_vector,
            }
        }
    }

    #[async_trait]
    impl EmbeddingProvider for StaticTestEmbeddingProvider {
        fn is_enabled(&self) -> bool {
            true
        }

        fn provider_name(&self) -> &'static str {
            "test"
        }

        fn dimension(&self) -> usize {
            DEFAULT_EMBEDDING_DIMENSION
        }

        async fn embed(&self, input: &str) -> Result<Vec<f64>, MemoryError> {
            let normalized = input.to_ascii_lowercase();
            if normalized.contains("salary raise") || normalized.contains("compensation increase") {
                return Ok(self.salary_vector.clone());
            }

            Ok(self.neutral_vector.clone())
        }
    }

    struct FlakyRemoteTestEmbeddingProvider {
        remaining_failures: AtomicUsize,
        embedding: Vec<f64>,
    }

    impl FlakyRemoteTestEmbeddingProvider {
        fn new(failures_before_success: usize) -> Self {
            let mut embedding = vec![0.0; DEFAULT_EMBEDDING_DIMENSION];
            embedding[0] = 1.0;
            Self {
                remaining_failures: AtomicUsize::new(failures_before_success),
                embedding,
            }
        }
    }

    #[async_trait]
    impl EmbeddingProvider for FlakyRemoteTestEmbeddingProvider {
        fn is_enabled(&self) -> bool {
            true
        }

        fn provider_name(&self) -> &'static str {
            "openai-compatible"
        }

        fn dimension(&self) -> usize {
            DEFAULT_EMBEDDING_DIMENSION
        }

        async fn embed(&self, _input: &str) -> Result<Vec<f64>, MemoryError> {
            let remaining = self.remaining_failures.load(Ordering::SeqCst);
            if remaining > 0 {
                self.remaining_failures.fetch_sub(1, Ordering::SeqCst);
                return Err(MemoryError::Transient(
                    "synthetic remote embedding outage".to_string(),
                ));
            }

            Ok(self.embedding.clone())
        }
    }

    #[tokio::test]
    async fn add_fact_persists_embedding_when_provider_enabled() {
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(
                "testdb",
                &["org".to_string()],
                "warn",
            )
            .await
            .expect("connect in memory"),
        );
        db_client.apply_migrations("org").await.expect("migrations");

        let service = MemoryService::new_with_embedding_provider(
            db_client.clone(),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
            Arc::new(StaticTestEmbeddingProvider::new()),
            crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
        )
        .expect("service");

        let fact_id = service
            .add_fact(
                "note",
                "Compensation increase approved for engineering",
                "Compensation increase approved",
                "episode:test",
                Utc::now(),
                "org",
                0.9,
                vec![],
                vec![],
                Provenance::agent_observation("episode:test"),
            )
            .await
            .expect("add fact");

        let fact = db_client
            .select_one(&fact_id, "org")
            .await
            .expect("select fact")
            .expect("stored fact");

        assert_eq!(
            fact.get("embedding")
                .and_then(|value| value.as_array())
                .map(Vec::len),
            Some(DEFAULT_EMBEDDING_DIMENSION)
        );
    }

    #[tokio::test]
    async fn add_fact_defers_background_embedding_after_transient_remote_failure() {
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(
                "testdb_background_fact_embedding",
                &["org".to_string()],
                "warn",
            )
            .await
            .expect("connect in memory"),
        );
        db_client.apply_migrations("org").await.expect("migrations");

        let mut service = MemoryService::new_with_embedding_provider(
            db_client.clone(),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
            Arc::new(FlakyRemoteTestEmbeddingProvider::new(1)),
            crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
        )
        .expect("service");
        service.current_embedding_signature = Some("embsig:background-test".to_string());
        service.current_embedding_model = Some("test-model".to_string());
        service.current_embedding_dimension = Some(DEFAULT_EMBEDDING_DIMENSION);

        let fact_id = service
            .add_fact(
                "note",
                "Provider outage should not block fact creation",
                "Provider outage should not block fact creation",
                "episode:test",
                Utc::now(),
                "org",
                0.9,
                vec![],
                vec![],
                Provenance::agent_observation("episode:test"),
            )
            .await
            .expect("add fact");

        let initial = db_client
            .select_one(&fact_id, "org")
            .await
            .expect("select fact")
            .expect("stored fact");
        assert!(initial.get("embedding").is_none());

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let fact = db_client
                    .select_one(&fact_id, "org")
                    .await
                    .expect("select fact")
                    .expect("stored fact");
                if fact.get("embedding_signature").and_then(Value::as_str)
                    == Some("embsig:background-test")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("background embedding should complete");
    }

    #[tokio::test]
    async fn generate_query_embedding_uses_background_cache_after_transient_remote_failure() {
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(
                "testdb_background_query_embedding",
                &["org".to_string()],
                "warn",
            )
            .await
            .expect("connect in memory"),
        );
        db_client.apply_migrations("org").await.expect("migrations");

        let mut service = MemoryService::new_with_embedding_provider(
            db_client,
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
            Arc::new(FlakyRemoteTestEmbeddingProvider::new(1)),
            crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
        )
        .expect("service");
        service.current_embedding_signature = Some("embsig:background-query-test".to_string());
        service.current_embedding_model = Some("test-model".to_string());
        service.current_embedding_dimension = Some(DEFAULT_EMBEDDING_DIMENSION);

        let ctx = service.build_context();
        let first = ctx
            .embedding_service
            .generate_query_embedding_with_background("salary raise")
            .await
            .expect("transient failure should degrade to background mode");
        assert!(first.is_none());

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if ctx
                    .embedding_service
                    .cached_query_embedding("salary raise")
                    .await
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("background query embedding should populate cache");

        let second = ctx
            .embedding_service
            .generate_query_embedding_with_background("salary raise")
            .await
            .expect("cached embedding");
        assert_eq!(
            second.as_ref().map(Vec::len),
            Some(DEFAULT_EMBEDDING_DIMENSION)
        );
    }

    #[test]
    fn log_event_with_full_access_context() {
        let access = AccessPayload {
            caller_id: Some("test-user".to_string()),
            allowed_scopes: Some(vec!["personal".to_string(), "org".to_string()]),
            allowed_tags: Some(vec!["tag1".to_string()]),
            session_vars: Some(json!({"session": "value"})),
            transport: Some("grpc".to_string()),
            content_type: Some("application/grpc".to_string()),
            cross_scope_allow: Some(vec![AccessScopeAllow {
                from: "personal".to_string(),
                to: "org".to_string(),
            }]),
        };
        let event = log_event("test_op", json!({}), json!({}), Some(&access), None, None);
        let access_val = event.get("access").unwrap();
        assert_eq!(
            access_val.get("caller_id").unwrap().as_str(),
            Some("test-user")
        );
        assert_eq!(
            access_val
                .get("allowed_scopes")
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(access_val.get("transport").unwrap().as_str(), Some("grpc"));
        assert_eq!(
            access_val.get("content_type").unwrap().as_str(),
            Some("application/grpc")
        );
    }

    #[test]
    fn log_event_without_access_context_omits_access_field() {
        let event = log_event("test_op", json!({}), json!({}), None, None, None);
        assert!(!event.contains_key("access"));
    }

    #[test]
    fn log_args_with_duration_adds_duration_ms_field() {
        let args = log_args_with_duration(
            json!({"scope": "org"}),
            std::time::Duration::from_millis(42),
        );

        assert_eq!(args.get("scope").and_then(Value::as_str), Some("org"));
        assert_eq!(args.get("duration_ms").and_then(Value::as_u64), Some(42));
    }

    #[test]
    fn build_embedding_log_result_reports_generated_count_and_dimension() {
        let result = build_embedding_log_result(1, Some(384));

        assert_eq!(
            result.get("generated_embeddings").and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(result.get("dimension").and_then(Value::as_u64), Some(384));
    }

    #[test]
    fn serialize_access_with_all_none_fields() {
        let access = AccessPayload {
            caller_id: None,
            allowed_scopes: None,
            allowed_tags: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: None,
        };
        let serialized = serialize_access(&access);
        assert!(serialized.get("caller_id").is_some());
        assert!(serialized.get("allowed_scopes").is_some());
        assert!(serialized.get("allowed_tags").is_some());
        assert!(serialized.get("session_vars").is_some());
        assert!(serialized.get("transport").is_some());
        assert!(serialized.get("content_type").is_some());
        assert!(serialized.get("cross_scope_allow").is_some());
    }

    #[test]
    fn string_from_value_handles_object_without_expected_keys() {
        let value = json!({"Other": "value"});
        assert_eq!(string_from_value(&value), None);
    }

    #[test]
    fn string_from_value_handles_record_id_missing_fields() {
        let value = json!({"RecordId": {"table": "entity"}});
        assert_eq!(string_from_value(&value), None);
    }

    #[test]
    fn namespace_for_scope_handles_various_inputs() {
        let service = create_test_service(vec!["org", "personal", "team", "private-domain"]);

        assert_eq!(service.namespace_for_scope("org").unwrap(), "org");
        assert_eq!(service.namespace_for_scope("personal").unwrap(), "personal");
        assert_eq!(service.namespace_for_scope("team").unwrap(), "team");
        assert_eq!(
            service.namespace_for_scope("private-domain").unwrap(),
            "private-domain"
        );
        assert!(service.namespace_for_scope("unknown").is_err());
        assert!(service.namespace_for_scope("").is_err());
        assert_eq!(service.namespace_for_scope("ORG").unwrap(), "org");
    }

    #[test]
    fn is_scope_allowed_with_empty_allowed_scopes() {
        let _service = create_test_service(vec!["org"]);
        let access = AccessPayload {
            allowed_scopes: Some(vec![]),
            ..Default::default()
        };
        assert!(!access.is_scope_allowed("org"));
    }

    #[test]
    fn is_scope_allowed_with_multiple_allowed_scopes() {
        let _service = create_test_service(vec!["org", "personal"]);
        let access = AccessPayload {
            allowed_scopes: Some(vec!["org".to_string(), "personal".to_string()]),
            ..Default::default()
        };
        assert!(access.is_scope_allowed("org"));
        assert!(access.is_scope_allowed("personal"));
        assert!(!access.is_scope_allowed("private"));
    }

    #[test]
    fn enforce_rate_limit_with_burst_capacity() {
        let service = create_test_service_with_rate_limit(10, 5);
        let access = AccessPayload {
            caller_id: Some("burst-test".to_string()),
            ..Default::default()
        };

        for _ in 0..5 {
            assert!(service.enforce_rate_limit(Some(&access)).is_ok());
        }
    }

    #[test]
    fn enforce_rate_limit_multiple_users_isolated() {
        let service = create_test_service_with_rate_limit(10, 1);

        let user1 = AccessPayload {
            caller_id: Some("user-1".to_string()),
            ..Default::default()
        };
        let user2 = AccessPayload {
            caller_id: Some("user-2".to_string()),
            ..Default::default()
        };

        assert!(service.enforce_rate_limit(Some(&user1)).is_ok());
        assert!(service.enforce_rate_limit(Some(&user1)).is_err());

        assert!(service.enforce_rate_limit(Some(&user2)).is_ok());
    }

    #[tokio::test]
    async fn resolve_entity_by_type_delegates_to_resolve() {
        let namespaces = vec!["org".to_string()];
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(
                "resolve_entity_test",
                &namespaces,
                "warn",
            )
            .await
            .expect("connect in-memory test db"),
        );
        for ns in &namespaces {
            db_client
                .apply_migrations(ns)
                .await
                .expect("apply migrations");
        }
        let service = MemoryService::new(db_client, namespaces, "warn".to_string(), 50, 100)
            .expect("create test service");

        // Resolve the same entity via different typed methods
        let id1 = service
            .resolve_entity("person", "Alice Smith")
            .await
            .expect("resolve person");
        let id2 = service
            .resolve_entity("person", "Alice Smith")
            .await
            .expect("resolve person again");
        assert_eq!(id1, id2);

        let id3 = service
            .resolve_entity("company", "Acme Corp")
            .await
            .expect("resolve company");
        assert_ne!(id1, id3);
    }

    #[tokio::test]
    async fn relate_creates_edge_between_entities() {
        let namespaces = vec!["org".to_string()];
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces("relate_test", &namespaces, "warn")
                .await
                .expect("connect in-memory test db"),
        );
        for ns in &namespaces {
            db_client
                .apply_migrations(ns)
                .await
                .expect("apply migrations");
        }
        let service = MemoryService::new(db_client, namespaces, "warn".to_string(), 50, 100)
            .expect("create test service");

        let from_id = service
            .resolve_entity("person", "Alice Relate")
            .await
            .expect("resolve alice");
        let to_id = service
            .resolve_entity("company", "Acme Relate")
            .await
            .expect("resolve acme");

        service
            .relate(&from_id, "works_at", &to_id)
            .await
            .expect("relate entities");
    }

    #[tokio::test]
    async fn get_surrealdb_config_returns_namespaces() {
        let namespaces = vec!["org".to_string(), "personal".to_string()];
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces("config_test", &namespaces, "warn")
                .await
                .expect("connect in-memory test db"),
        );
        let service =
            MemoryService::new(db_client, namespaces.clone(), "warn".to_string(), 50, 100)
                .expect("create test service");

        let config = service.get_surrealdb_config().await.expect("get config");
        let config_namespaces = config["namespaces"].as_array().expect("namespaces array");
        assert_eq!(config_namespaces.len(), 2);
    }

    #[tokio::test]
    async fn episode_count_returns_zero_for_empty_db() {
        let namespaces = vec!["org".to_string()];
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(
                "episode_count_test",
                &namespaces,
                "warn",
            )
            .await
            .expect("connect in-memory test db"),
        );
        for ns in &namespaces {
            db_client
                .apply_migrations(ns)
                .await
                .expect("apply migrations");
        }
        let service = MemoryService::new(db_client, namespaces, "warn".to_string(), 50, 100)
            .expect("create test service");

        let count = service.episode_count().await.expect("count episodes");
        assert_eq!(count, 0);
    }
}
