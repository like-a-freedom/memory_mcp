//! MemoryService implementation - core service orchestration.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::logging::LogLevel;

use crate::error::MemoryError;

pub(crate) mod builder;
use crate::platform::log_event::log_event;
pub use builder::MemoryService;

impl MemoryService {
    /// The database client, for callers that own their own query.
    ///
    /// The diff command lives in knowledge but is reached from the
    /// transport, which holds the service; this is the narrow handoff
    /// that lets knowledge own the query without the transport
    /// reaching into the container's fields.
    pub fn db_client_for_port(&self) -> Arc<dyn crate::storage::DbClient> {
        self.db_client.clone()
    }

    /// The bound Active Namespace, for the same handoff.
    pub fn namespace_for_port(&self) -> String {
        self.active_namespace.clone()
    }

    /// The knowledge-owned graph store: entities, communities and
    /// edges.
    pub(crate) fn knowledge_graph_store(
        &self,
    ) -> crate::knowledge::graph_store::KnowledgeGraphStore {
        crate::knowledge::graph_store::KnowledgeGraphStore::new(
            self.db_client.clone(),
            self.active_namespace.clone(),
        )
    }

    #[cfg(feature = "streamable-http")]
    pub(crate) async fn retained_cache_bytes(&self) -> (usize, usize) {
        let context_bytes = {
            let cache = self.context_cache.read().await;
            cache.accounted_bytes()
        };
        let query_bytes = {
            let cache = self.query_embedding_cache.lock().await;
            cache.accounted_bytes()
        };
        (context_bytes, query_bytes)
    }

    pub(crate) fn embedding_runtime_snapshot(
        &self,
    ) -> crate::embedding::runtime::EmbeddingRuntimeState {
        self.embedding_runtime_state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn replace_embedding_runtime_state(
        &self,
        state: crate::embedding::runtime::EmbeddingRuntimeState,
    ) {
        *self
            .embedding_runtime_state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = state;
    }

    /// The embedding service bound to the current runtime state.
    ///
    /// The provider, signature and dimension are read from the live
    /// runtime snapshot, so a runtime that swapped its target is
    /// reflected without rebuilding the service.
    pub(crate) fn embedding_service(&self) -> crate::embedding::service::EmbeddingService {
        let state = self.embedding_runtime_snapshot();
        crate::embedding::service::EmbeddingService::new(
            self.db_client.clone(),
            self.active_namespace.clone(),
            self.logger.clone(),
            state.provider,
            self.embedding_similarity_threshold,
            state.signature,
            state.model,
            state.dimension,
            self.context_cache.clone(),
            self.query_embedding_cache.clone(),
            self.task_runner.clone(),
        )
    }

    /// The claim store behind the claim service. Present whenever the
    /// claim pipeline is wired, which is the condition the
    /// invalidation use case checks before closing derived claims.
    pub(crate) fn claim_store(&self) -> Option<Arc<dyn crate::knowledge::claims::ClaimStore>> {
        Some(self.claim_service.store.clone())
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

    pub(crate) fn lifecycle_policy(&self) -> super::LifecyclePolicy {
        super::LifecyclePolicy::from(&self.lifecycle_config)
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
        confidence: f64,
        entity_links: Vec<String>,
        policy_tags: Vec<String>,
        provenance: crate::models::Provenance,
    ) -> Result<String, MemoryError> {
        let deps = crate::memory::capabilities::deps::ExtractDeps::from(self);
        self.fact_service
            .add_fact(
                &deps,
                fact_type,
                content,
                quote,
                source_episode,
                t_valid,
                confidence,
                entity_links,
                policy_tags,
                provenance,
            )
            .await
    }

    /// Start claim reconciliation workers and schedule backfill.
    pub(crate) async fn start_claim_workers(
        &self,
    ) -> crate::knowledge::claims_policy::worker::ClaimWorkerRuntime {
        let runtime = crate::knowledge::claims_policy::worker::ClaimWorkerRuntime::new();
        let worker_id = format!("claim-worker-{}", std::process::id());
        runtime
            .spawn_worker(self.claim_service.clone(), worker_id)
            .await;
        runtime
    }

    /// Start the filesystem-ingestion runtime inside `serve`.
    #[cfg(feature = "fs-watch")]
    pub async fn start_fs_watch(
        &self,
        config: crate::config::fs_watch::FsWatchConfig,
    ) -> Result<super::fs_watch::runtime::FsWatchRuntime, MemoryError> {
        super::fs_watch::runtime::FsWatchRuntime::start(self.clone(), config).await
    }

    /// Start the agent-memory lifecycle projection worker.
    ///
    /// The worker drains `event_projection_job` records and projects accepted
    /// lifecycle events into facts via the existing extraction path. It is a
    /// no-op when no lifecycle events have been captured.
    pub(crate) async fn start_lifecycle_worker(
        &self,
    ) -> crate::service::agent_memory::worker::LifecycleWorkerRuntime {
        let runtime = crate::service::agent_memory::worker::LifecycleWorkerRuntime::new();
        let poll_interval = crate::service::agent_memory::worker::empty_poll_interval().as_secs();
        runtime.spawn(self.clone(), poll_interval).await;
        runtime
    }

    pub(crate) async fn start_embedding_recovery_worker(
        &self,
        config: crate::config::EmbeddingConfig,
        data_dir: String,
    ) -> super::embedding_recovery::EmbeddingRecoveryRuntime {
        let runtime = super::embedding_recovery::EmbeddingRecoveryRuntime::new();
        let backend = std::sync::Arc::new(
            super::embedding_recovery::ConfiguredEmbeddingRecoveryBackend::new(
                config.clone(),
                data_dir,
            ),
        );
        runtime
            .spawn(
                self.clone(),
                config.clone(),
                backend,
                super::embedding_recovery::RecoveryWorkerSettings::production(
                    config.recovery_interval_secs,
                ),
            )
            .await;
        runtime
    }

    /// Starts the one-shot Classic GLiNER artifact refresh task when the
    /// configured backend is Classic GLiNER. Returns `None` for any other
    /// backend. The caller is responsible for awaiting the returned
    /// runtime's `shutdown()` to ensure cancellation and join.
    pub(crate) fn start_ner_artifact_refresh(
        &self,
    ) -> Option<super::model_artifact_refresh::NerArtifactRefreshRuntime> {
        let config = self.ner_artifact_refresh_config.clone()?;
        let native = self.ner_artifact_refresh_native.clone()?;
        let spec = crate::knowledge::entity_extraction::gliner::CLASSIC_GLINER_SPEC.clone();
        Some(
            super::model_artifact_refresh::NerArtifactRefreshRuntime::start(
                config,
                spec,
                native,
                self.logger.clone(),
            ),
        )
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
        if let Some(runtime) = &self.embedding_recovery_runtime {
            runtime.shutdown().await;
        }
    }

    /// Build a `LifecycleCapture` wired to the production storage and ingestion
    /// backends. Returns `None` if lifecycle integration is not enabled.
    pub fn lifecycle_capture(
        &self,
    ) -> Option<crate::memory::agent_memory::capture::LifecycleCapture> {
        if !self.lifecycle_config.enabled {
            return None;
        }
        let store = std::sync::Arc::new(crate::storage::AgentMemoryStore::new(
            self.db_client.clone(),
            self.active_namespace.clone(),
        ));
        let ingestion = std::sync::Arc::new(self.ingestion_service.clone());
        let backend = std::sync::Arc::new(
            crate::memory::agent_memory::capture::ProductionCaptureBackend::new(store, ingestion),
        );
        Some(crate::memory::agent_memory::capture::LifecycleCapture::new(
            backend,
        ))
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
    ) -> Result<Option<crate::memory::agent_memory::capture::LifecycleCaptureResult>, MemoryError>
    {
        let Some(capture) = self.lifecycle_capture() else {
            return Ok(None);
        };
        let budget = crate::memory::agent_memory::capture::default_capture_budget();
        let result = capture
            .execute(event, context, &budget, 16 * 1024, 16)
            .await?;
        Ok(Some(result))
    }

    /// Build a `LifecycleRecall` orchestrator for selective recall.
    ///
    /// Returns `None` if lifecycle integration is not enabled. The orchestrator
    /// delegates to the existing `assemble_context` pipeline via the
    /// `RecallPipeline` trait.
    pub fn lifecycle_recall(&self) -> Option<crate::memory::agent_memory::recall::LifecycleRecall> {
        if !self.lifecycle_config.enabled {
            return None;
        }
        Some(
            crate::memory::agent_memory::recall::LifecycleRecall::with_trace_registry(
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
    ) -> Result<Option<crate::memory::agent_memory::recall::LifecycleRecallResult>, MemoryError>
    {
        let Some(recall) = self.lifecycle_recall() else {
            return Ok(None);
        };
        let pipeline = ProductionRecallPipeline { service: self };
        let now_secs = chrono::Utc::now().timestamp().max(0) as u64;
        let result = recall.execute(&pipeline, event, context, now_secs).await?;
        Ok(Some(result))
    }

    pub(crate) async fn check_surrealdb_connection(&self) -> Result<(), MemoryError> {
        let store = crate::storage::EventLogStoreClient::new(
            self.db_client.clone(),
            self.active_namespace.clone(),
        );
        let _ = store.select_event_log().await?;
        Ok(())
    }

    /// Read one episode record, refusing an id that names another
    /// kind.
    pub(crate) async fn find_episode_record(
        &self,
        episode_id: &str,
    ) -> Result<crate::storage::RecordLookup, MemoryError> {
        crate::storage::owner_scoped_read(
            crate::memory::episode_store::EpisodeStoreClient::new(
                self.db_client.clone(),
                self.active_namespace.clone(),
            )
            .select_episode(episode_id)
            .await,
        )
    }

    /// Read one fact record, refusing an id that names another kind.
    pub(crate) async fn find_fact_record(
        &self,
        fact_id: &str,
    ) -> Result<crate::storage::RecordLookup, MemoryError> {
        crate::storage::owner_scoped_read(
            crate::knowledge::FactStoreClient::new(
                self.db_client.clone(),
                self.active_namespace.clone(),
            )
            .select_fact(fact_id)
            .await,
        )
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
impl<'a> crate::memory::agent_memory::recall::RecallPipeline for ProductionRecallPipeline<'a> {
    async fn assemble(
        &self,
        request: crate::models::AssembleContextRequest,
    ) -> Result<Vec<crate::models::AssembledContextItem>, MemoryError> {
        crate::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability::assemble_context_from_service(
            self.service,
            request,
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
    use crate::embedding::runtime::EmbeddingRuntimeState;
    use crate::models::{AccessPayload, Provenance};
    use crate::service::EmbeddingProvider;
    use crate::storage::{DbClient, SurrealDbClient};
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// The container's store constructors bind the Active Namespace, not a
    /// default. This is a characterisation test: it pins the binding so
    /// moving the constructors to their call sites cannot silently change
    /// which namespace a store reads.
    #[tokio::test]
    async fn the_containers_store_constructors_bind_the_active_namespace() {
        let db = Arc::new(crate::service::mock_db::MockDbClient::new());
        let svc = MemoryService::new(
            db.clone(),
            "tenant-a".to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("container");

        // Two different stores, so the assertion covers both constructors
        // rather than one twice.
        let reembed = crate::embedding::reembed_store::ReembedStoreClient::new(
            db.clone(),
            svc.active_namespace.clone(),
        );
        reembed.remove_embedding_index().await.ok();
        let _ = svc.check_surrealdb_connection().await;

        let seen = db.seen_namespaces();
        assert!(
            !seen.is_empty(),
            "expected the stores to have issued at least one read"
        );
        assert!(
            seen.iter().all(|ns| ns == "tenant-a"),
            "every store read must target the Active Namespace, saw {seen:?}"
        );
    }

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
            allowed_tags: None,
            session_vars: None,
            transport: None,
            content_type: None,
        };
        let event = log_event("test_op", json!({}), json!({}), Some(&access), None, None);
        let access_event = event.get("access").unwrap();
        assert_eq!(
            access_event.get("caller_id").unwrap().as_str(),
            Some("test-caller")
        );
    }

    #[test]
    fn serialize_access_preserves_supported_fields_and_omits_scope_fields() {
        let access = AccessPayload {
            caller_id: Some("caller".to_string()),
            allowed_tags: Some(vec!["tag1".to_string()]),
            session_vars: Some(json!({"key": "value"})),
            transport: Some("http".to_string()),
            content_type: Some("application/json".to_string()),
        };
        let serialized = crate::platform::log_event::serialize_access(&access);
        assert!(serialized.get("caller_id").is_some());
        assert!(serialized.get("allowed_tags").is_some());
        assert!(serialized.get("session_vars").is_some());
        assert!(serialized.get("transport").is_some());
        assert!(serialized.get("content_type").is_some());
        assert!(serialized.get("allowed_scopes").is_none());
        assert!(serialized.get("cross_scope_allow").is_none());
    }

    /// Verifies that the fact-owned input builder produces a long input for
    /// the embedding service's 8,000-character truncation limit.
    /// This test is deliberately lightweight — the full reembed pipeline
    /// with long content is exercised in `reembed_long_fact_content_does_not_fail`.
    #[test]
    fn generate_embedding_input_builder_respects_truncation_limit() {
        // Simulate what build_fact_embedding_input produces for a very long
        // fact, then verify the truncation would apply.
        let long_content = "x".repeat(60_000);
        let full_input = crate::knowledge::fact_service::FactService::build_fact_embedding_input(
            "note",
            &long_content,
            &long_content,
        );
        // The input + overhead is > 8,000, so generate_embedding will truncate
        assert!(full_input.len() > 8_000);
        // Truncation should produce at most 8,000 chars
        let truncated: String = full_input.chars().take(8_000).collect();
        assert_eq!(truncated.chars().count(), 8_000);
    }

    fn create_test_service(active_namespace: &str) -> MemoryService {
        use std::sync::Arc;

        MemoryService::new(
            Arc::new(crate::service::mock_db::MockDbClient::new()),
            active_namespace.to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn build_context_uses_the_latest_embedding_runtime_state() {
        let service = create_test_service("org");
        let replacement = Arc::new(StaticTestEmbeddingProvider::new());
        service.replace_embedding_runtime_state(EmbeddingRuntimeState::new(
            replacement,
            Some("embsig:test".to_string()),
            Some("test-model".to_string()),
            Some(DEFAULT_EMBEDDING_DIMENSION),
        ));

        let embedding = service.embedding_service();
        assert_eq!(embedding.embedding_provider().provider_name(), "test");
        assert_eq!(embedding.current_embedding_signature(), Some("embsig:test"));
        assert_eq!(
            embedding.current_embedding_dimension(),
            Some(DEFAULT_EMBEDDING_DIMENSION)
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
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
            Arc::new(StaticTestEmbeddingProvider::new()),
            crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
            Arc::new(crate::service::AnnoEntityExtractor::new().expect("anno extractor")),
        )
        .expect("service");

        let fact_id = service
            .add_fact(
                "note",
                "Compensation increase approved for engineering",
                "Compensation increase approved",
                "episode:test",
                Utc::now(),
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

        let service = MemoryService::new_with_embedding_provider(
            db_client.clone(),
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
            Arc::new(FlakyRemoteTestEmbeddingProvider::new(1)),
            crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
            Arc::new(crate::service::AnnoEntityExtractor::new().expect("anno extractor")),
        )
        .expect("service");
        let provider = service.embedding_runtime_snapshot().provider;
        service.replace_embedding_runtime_state(EmbeddingRuntimeState::new(
            provider,
            Some("embsig:background-test".to_string()),
            Some("test-model".to_string()),
            Some(DEFAULT_EMBEDDING_DIMENSION),
        ));

        let fact_id = service
            .add_fact(
                "note",
                "Provider outage should not block fact creation",
                "Provider outage should not block fact creation",
                "episode:test",
                Utc::now(),
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

        let service = MemoryService::new_with_embedding_provider(
            db_client,
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
            Arc::new(FlakyRemoteTestEmbeddingProvider::new(1)),
            crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
            Arc::new(crate::service::AnnoEntityExtractor::new().expect("anno extractor")),
        )
        .expect("service");
        let provider = service.embedding_runtime_snapshot().provider;
        service.replace_embedding_runtime_state(EmbeddingRuntimeState::new(
            provider,
            Some("embsig:background-query-test".to_string()),
            Some("test-model".to_string()),
            Some(DEFAULT_EMBEDDING_DIMENSION),
        ));

        let first = service
            .embedding_service()
            .generate_query_embedding_with_background("salary raise")
            .await
            .expect("transient failure should degrade to background mode");
        assert!(first.is_none());

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if service
                    .embedding_service()
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

        let second = service
            .embedding_service()
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
            allowed_tags: Some(vec!["tag1".to_string()]),
            session_vars: Some(json!({"session": "value"})),
            transport: Some("grpc".to_string()),
            content_type: Some("application/grpc".to_string()),
        };
        let event = log_event("test_op", json!({}), json!({}), Some(&access), None, None);
        let access_val = event.get("access").unwrap();
        assert_eq!(
            access_val.get("caller_id").unwrap().as_str(),
            Some("test-user")
        );
        assert_eq!(
            access_val
                .get("allowed_tags")
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            access_val
                .get("session_vars")
                .and_then(|value| value.get("session"))
                .and_then(Value::as_str),
            Some("value")
        );
        assert!(access_val.get("allowed_scopes").is_none());
        assert!(access_val.get("cross_scope_allow").is_none());
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
        let args = crate::platform::log_event::log_args_with_duration(
            json!({"scope": "org"}),
            std::time::Duration::from_millis(42),
        );

        assert_eq!(args.get("scope").and_then(Value::as_str), Some("org"));
        assert_eq!(args.get("duration_ms").and_then(Value::as_u64), Some(42));
    }

    #[test]
    fn build_embedding_log_result_reports_generated_count_and_dimension() {
        let result = crate::platform::log_event::build_embedding_log_result(1, Some(384));

        assert_eq!(
            result.get("generated_embeddings").and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(result.get("dimension").and_then(Value::as_u64), Some(384));
    }

    #[test]
    fn serialize_access_with_all_none_fields_omits_scope_fields() {
        let access = AccessPayload {
            caller_id: None,
            allowed_tags: None,
            session_vars: None,
            transport: None,
            content_type: None,
        };
        let serialized = crate::platform::log_event::serialize_access(&access);
        assert!(serialized.get("caller_id").is_some());
        assert!(serialized.get("allowed_tags").is_some());
        assert!(serialized.get("session_vars").is_some());
        assert!(serialized.get("transport").is_some());
        assert!(serialized.get("content_type").is_some());
        assert!(serialized.get("allowed_scopes").is_none());
        assert!(serialized.get("cross_scope_allow").is_none());
    }

    // -----------------------------------------------------------------------
    // record-id validation wired into the owner-scoped accessors
    // (tests — exercised via the MemoryService entry points).
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn find_episode_record_rejects_bare_hex() {
        let service = create_test_service("org");
        let result = service
            .find_episode_record("474b2d8b81b3feabf832ef08")
            .await;
        match result {
            Err(crate::error::MemoryError::Validation(msg)) => {
                // The refusal must name the exact id to pass back, not a
                // `<table>:<id>` placeholder the caller still has to fill in.
                assert!(msg.contains("474b2d8b81b3feabf832ef08"), "{msg}");
                assert!(msg.contains("episode:474b2d8b81b3feabf832ef08"), "{msg}");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn find_episode_record_rejects_empty_id_part() {
        let service = create_test_service("org");
        let result = service.find_episode_record("episode:").await;
        assert!(matches!(
            result,
            Err(crate::error::MemoryError::Validation(_))
        ));
    }

    #[tokio::test]
    async fn find_fact_record_rejects_bare_hex() {
        let service = create_test_service("org");
        let result = service.find_fact_record("072d682d0d467aa94aad684d").await;
        assert!(matches!(
            result,
            Err(crate::error::MemoryError::Validation(_))
        ));
    }

    #[tokio::test]
    async fn find_fact_record_rejects_empty_id_part() {
        let service = create_test_service("org");
        let result = service.find_fact_record("fact:").await;
        assert!(matches!(
            result,
            Err(crate::error::MemoryError::Validation(_))
        ));
    }

    #[tokio::test]
    async fn find_episode_record_accepts_wellformed_episode_id() {
        // Sanity: well-formed ids pass validation and reach the DB (mock returns None).
        let service = create_test_service("org");
        let result = service.find_episode_record("episode:doesnotexist").await;
        assert!(
            result.is_ok(),
            "well-formed id must pass validation: {result:?}"
        );
    }
}
