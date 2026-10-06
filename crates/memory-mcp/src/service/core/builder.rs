use std::sync::Arc;

use lru::LruCache;

use crate::config::SurrealConfig;
use crate::embedding::providers::{DisabledEmbeddingProvider, EmbeddingProvider};
use crate::embedding::runtime::EmbeddingRuntimeState;
use crate::error::MemoryError;
use crate::logging::StdoutLogger;
use crate::memory::context_cache::ContextCache;
use crate::platform::rate_limiter::RateLimiter;
use crate::service::AnnoEntityExtractor;
use crate::service::EntityExtractor;

use crate::shared::triple_extractor::RuleBasedTripleExtractor;
use crate::shared::triple_extractor::TripleExtractor;
use crate::storage::DbClient;

/// Core service for memory operations.
#[derive(Clone)]
pub struct MemoryService {
    /// Database client for storage operations.
    pub(crate) db_client: Arc<dyn DbClient>,
    pub(crate) active_namespace: String,
    pub(crate) logger: StdoutLogger,
    pub(crate) rate_limiter: Arc<RateLimiter>,
    pub(crate) ingestion_service: crate::memory::ingestion::IngestionService,
    pub(crate) entity_service: crate::knowledge::entity_service::EntityService,
    pub(crate) fact_service: crate::knowledge::fact_service::FactService,
    pub(crate) explanation_service: crate::memory::explanation::ExplanationService,
    pub(crate) context_cache: ContextCache,
    pub(crate) entity_extractor: Arc<dyn EntityExtractor>,
    pub(crate) embedding_runtime_state: Arc<std::sync::RwLock<EmbeddingRuntimeState>>,
    pub(crate) embedding_similarity_threshold: f64,
    pub(crate) task_runner: Arc<crate::embedding::providers::task_runner::BackgroundTaskRunner>,
    pub(crate) query_embedding_cache:
        Arc<tokio::sync::Mutex<LruCache<String, crate::service::CachedQueryEmbedding>>>,
    pub(crate) query_logging_enabled: bool,
    pub(crate) query_log_retention_days: u32,
    pub(crate) entity_resolver: crate::knowledge::entity_resolution::EntityResolver,
    pub(crate) triple_extractor: Arc<dyn crate::shared::triple_extractor::TripleExtractor>,
    pub(crate) lifecycle_config: crate::config::LifecycleConfig,
    pub(crate) claim_service: crate::knowledge::claims_policy::projection::ClaimService,
    /// Shared per-session exposure-trace registry for selective recall.
    ///
    /// Holds at most 32 traces per session for 30 minutes. Persists only when a
    /// later significant capture links a trace.
    pub(crate) trace_registry: Arc<crate::memory::agent_memory::recall::SessionTraceRegistry>,
    /// Owned runtime for the lifecycle background workers (decay, archival,
    /// community). `None` when constructed via the test builders that do not
    /// spawn lifecycle workers; populated by the composition root.
    pub(crate) lifecycle_background_workers:
        Option<crate::memory::lifecycle_workers::LifecycleBackgroundWorkerRuntime>,
    /// Owned runtime for remote embedding recovery after a degraded startup.
    pub(crate) embedding_recovery_runtime:
        Option<crate::service::embedding_recovery::EmbeddingRecoveryRuntime>,
    /// Bounded-concurrency semaphore for fire-and-forget triple extraction
    /// tasks. Limits in-flight extraction tasks to prevent unbounded task
    /// spawning under load.
    pub(crate) triple_extraction_semaphore: Arc<tokio::sync::Semaphore>,
    /// Optional Classic GLiNER refresh config retained for the post-readiness
    /// background runtime. Only populated when the configured backend is
    /// Classic GLiNER.
    pub(crate) ner_artifact_refresh_config:
        Option<super::super::model_artifact_refresh::NerArtifactRefreshConfig>,
    /// Native config captured at build time so the post-readiness runtime
    /// can construct an `UnavailableEntityExtractor` if it ever needs to
    /// (e.g. when the refresh task must mirror a configured config). Held
    /// only when the backend is Classic GLiNER.
    pub(crate) ner_artifact_refresh_native: Option<crate::config::NativeGlinerConfig>,
    /// Enables tenant-local transactional outbox hooks for the HTTP runtime.
    #[cfg(feature = "streamable-http")]
    pub(crate) outbox_enabled: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ServiceBuildConfig {
    pub(super) rate_limit_rps: i32,
    pub(super) rate_limit_burst: i32,
    pub(super) cache_size: usize,
    pub(super) embedding_similarity_threshold: f64,
}

/// Crate-private dependency bundle for the `MemoryService`
/// private builder (Task 13, architecture-audit
/// remediation). The four fields are the seam that lets
/// tests inject a custom `TripleExtractor` (and, in the
/// future, alternate `DbClient` / `EntityExtractor` /
/// `EmbeddingProvider` adapters) without changing the
/// public constructor signatures. The bundle does NOT
/// include caches, loggers, rate limiters, stores derived
/// from `DbClient`, semaphores, lifecycle workers, or
/// runtime state — those are constructed inside `build`
/// from the build config and the dependency inputs.
pub(crate) struct MemoryServiceDependencies {
    pub(crate) db_client: Arc<dyn DbClient>,
    pub(crate) entity_extractor: Arc<dyn EntityExtractor>,
    pub(crate) embedding_provider: Arc<dyn EmbeddingProvider>,
    pub(crate) triple_extractor: Arc<dyn TripleExtractor>,
}

impl MemoryServiceDependencies {
    /// Production-default bundle: the
    /// `DisabledEmbeddingProvider`, the built-in
    /// `AnnoEntityExtractor`, and the
    /// `RuleBasedTripleExtractor`. The caller supplies
    /// the `db_client` because it is connection-bound.
    pub(crate) fn with_db_client(db_client: Arc<dyn DbClient>) -> Result<Self, MemoryError> {
        Ok(Self {
            db_client,
            entity_extractor: Arc::new(AnnoEntityExtractor::new()?),
            embedding_provider: Arc::new(DisabledEmbeddingProvider::new(
                crate::config::DEFAULT_EMBEDDING_DIMENSION,
            )),
            triple_extractor: Arc::new(RuleBasedTripleExtractor::new()),
        })
    }

    /// Build a bundle from an already-constructed
    /// `embedding_provider` and `entity_extractor`,
    /// keeping the production
    /// `RuleBasedTripleExtractor` as the triple
    /// extractor. Used by the public
    /// `new_with_embedding_provider` constructor.
    pub(crate) fn with_db_and_providers(
        db_client: Arc<dyn DbClient>,
        embedding_provider: Arc<dyn EmbeddingProvider>,
        entity_extractor: Arc<dyn EntityExtractor>,
    ) -> Self {
        Self {
            db_client,
            entity_extractor,
            embedding_provider,
            triple_extractor: Arc::new(RuleBasedTripleExtractor::new()),
        }
    }
}

pub(crate) fn startup_config_events(
    config: &SurrealConfig,
) -> Vec<std::collections::HashMap<String, serde_json::Value>> {
    let mut events = config
        .defaulted_variables
        .iter()
        .map(|variable| {
            std::collections::HashMap::from([
                (
                    "op".to_string(),
                    serde_json::json!("config.default_applied"),
                ),
                ("variable".to_string(), serde_json::json!(variable)),
            ])
        })
        .collect::<Vec<_>>();

    if let Some(path) = &config.legacy_data_dir {
        events.push(std::collections::HashMap::from([
            (
                "op".to_string(),
                serde_json::json!("config.legacy_data_dir_detected"),
            ),
            ("path".to_string(), serde_json::json!(path)),
        ]));
    }

    events
}

impl MemoryService {
    /// Creates a new service instance.
    pub fn new(
        db_client: Arc<dyn DbClient>,
        active_namespace: String,
        log_level: String,
        rate_limit_rps: i32,
        rate_limit_burst: i32,
    ) -> Result<Self, MemoryError> {
        Self::build(
            MemoryServiceDependencies::with_db_client(db_client)?,
            active_namespace,
            log_level,
            ServiceBuildConfig {
                rate_limit_rps,
                rate_limit_burst,
                cache_size: crate::service::CONTEXT_CACHE_SIZE,
                embedding_similarity_threshold:
                    crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_with_embedding_provider(
        db_client: Arc<dyn DbClient>,
        active_namespace: String,
        log_level: String,
        rate_limit_rps: i32,
        rate_limit_burst: i32,
        embedding_provider: Arc<dyn EmbeddingProvider>,
        embedding_similarity_threshold: f64,
        entity_extractor: Arc<dyn EntityExtractor>,
    ) -> Result<Self, MemoryError> {
        Self::build(
            MemoryServiceDependencies::with_db_and_providers(
                db_client,
                embedding_provider,
                entity_extractor,
            ),
            active_namespace,
            log_level,
            ServiceBuildConfig {
                rate_limit_rps,
                rate_limit_burst,
                cache_size: crate::service::CONTEXT_CACHE_SIZE,
                embedding_similarity_threshold,
            },
        )
    }

    fn build(
        dependencies: MemoryServiceDependencies,
        active_namespace: String,
        log_level: String,
        build_config: ServiceBuildConfig,
    ) -> Result<Self, MemoryError> {
        if active_namespace.trim().is_empty() {
            return Err(MemoryError::ConfigInvalid(
                "one active namespace is required".to_string(),
            ));
        }
        let active_namespace = active_namespace.trim().to_string();
        let cache_size = std::num::NonZeroUsize::new(build_config.cache_size).ok_or_else(|| {
            MemoryError::ConfigInvalid("context cache size must be > 0".to_string())
        })?;
        let query_embedding_cache_size =
            std::num::NonZeroUsize::new(crate::service::DEFAULT_QUERY_EMBEDDING_CACHE_SIZE)
                .ok_or_else(|| {
                    MemoryError::ConfigInvalid("query embedding cache size must be > 0".to_string())
                })?;
        let logger = StdoutLogger::new(&log_level);
        let rate_limiter = Arc::new(RateLimiter::new(
            build_config.rate_limit_rps,
            build_config.rate_limit_burst,
        ));
        let db_client = dependencies.db_client.clone();
        let ingestion_service = crate::memory::ingestion::IngestionService::new(
            db_client.clone(),
            active_namespace.clone(),
            logger.clone(),
            rate_limiter.clone(),
        );
        let entity_service = crate::knowledge::entity_service::EntityService::new(
            db_client.clone(),
            active_namespace.clone(),
        );
        let fact_service = crate::knowledge::fact_service::FactService::new(
            crate::knowledge::FactStoreClient::new(db_client.clone(), active_namespace.clone()),
        );
        let explanation_service = crate::memory::explanation::ExplanationService::new(
            db_client.clone(),
            logger.clone(),
            active_namespace.clone(),
        );
        let claim_store = Arc::new(crate::knowledge::claims::SurrealClaimStore::new(
            db_client.clone(),
            active_namespace.clone(),
        ));
        let fuzzy_threshold = crate::config::ner::entity_fuzzy_threshold()?;
        Ok(Self {
            db_client: dependencies.db_client,
            active_namespace,
            logger,
            rate_limiter,
            ingestion_service,
            entity_service,
            fact_service,
            explanation_service,
            context_cache: Arc::new(tokio::sync::RwLock::new(LruCache::new(cache_size))),
            entity_extractor: dependencies.entity_extractor,
            embedding_runtime_state: Arc::new(std::sync::RwLock::new(EmbeddingRuntimeState::new(
                dependencies.embedding_provider,
                None,
                None,
                None,
            ))),
            embedding_similarity_threshold: build_config.embedding_similarity_threshold,
            task_runner: Arc::new(
                crate::embedding::providers::task_runner::BackgroundTaskRunner::new(),
            ),
            query_embedding_cache: Arc::new(tokio::sync::Mutex::new(LruCache::new(
                query_embedding_cache_size,
            ))),
            query_logging_enabled: false,
            query_log_retention_days: crate::config::DEFAULT_QUERY_LOG_RETENTION_DAYS,
            entity_resolver: crate::knowledge::entity_resolution::EntityResolver::new(
                fuzzy_threshold,
            ),
            triple_extractor: dependencies.triple_extractor,
            lifecycle_config: crate::config::LifecycleConfig::default(),
            claim_service: crate::knowledge::claims_policy::projection::ClaimService::new(
                claim_store,
            ),
            trace_registry: Arc::new(
                crate::memory::agent_memory::recall::SessionTraceRegistry::new(),
            ),
            lifecycle_background_workers: None,
            embedding_recovery_runtime: None,
            triple_extraction_semaphore: Arc::new(tokio::sync::Semaphore::new(
                crate::service::TRIPLE_EXTRACTION_MAX_CONCURRENCY,
            )),
            ner_artifact_refresh_config: None,
            ner_artifact_refresh_native: None,
            #[cfg(feature = "streamable-http")]
            outbox_enabled: false,
        })
    }

    /// Enable the tenant-local transactional outbox for the HTTP runtime.
    /// Stdio never calls this method, preserving its direct storage behavior.
    #[cfg(feature = "streamable-http")]
    pub(crate) fn with_http_outbox(mut self) -> Self {
        self.ingestion_service = self.ingestion_service.with_outbox();
        self.entity_service = self.entity_service.with_outbox();
        self.fact_service = self.fact_service.with_outbox();
        self.outbox_enabled = true;
        self
    }

    /// Returns a copy of the service with persisted query analytics enabled or disabled.
    #[must_use]
    pub fn with_query_logging_enabled(mut self, enabled: bool) -> Self {
        self.query_logging_enabled = enabled;
        self
    }

    /// Returns a copy of the service with a custom query-log retention window.
    #[must_use]
    pub fn with_query_log_retention_days(mut self, days: u32) -> Self {
        self.query_log_retention_days = days;
        self
    }

    /// Returns a copy of the service with a different claim rollout stage.
    ///
    /// This is the configuration seam for the stage, complementing the
    /// `MEMORY_CLAIM_ROLLOUT_STAGE` environment variable that
    /// `bootstrap::stdio` applies. It exists because `ClaimConfig` and
    /// `ClaimRolloutStage` are crate-private while the stage changes what
    /// `assemble_context` may disclose: `shadow` and `relations` project and
    /// persist claims but serve no relations to the read path, and `evidence`
    /// does. Accepting the same vocabulary as the environment variable keeps
    /// one stringly-typed boundary instead of widening the public API with a
    /// crate-private type.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::ConfigInvalid`] for a stage name outside
    /// `disabled`, `shadow`, `relations`, `evidence`.
    pub fn with_claim_rollout_stage(
        mut self,
        stage: &str,
    ) -> Result<Self, crate::error::MemoryError> {
        use std::str::FromStr;
        let rollout_stage = crate::config::claims::ClaimRolloutStage::from_str(stage)?;
        let config = crate::config::claims::ClaimConfig {
            rollout_stage,
            ..Default::default()
        };
        self.claim_service = self.claim_service.clone().with_config(config);
        Ok(self)
    }

    /// Returns a copy of the service with lifecycle integration enabled or
    /// disabled. This controls whether `lifecycle_capture` returns `Some` and
    /// whether the projection worker is started.
    ///
    /// `MemoryService::new` takes the lifecycle configuration from its
    /// caller rather than from the environment, so a caller that builds a
    /// container directly — the eval harness, a test — has no other way to
    /// turn it on. The composition root sets the same field from
    /// `LifecycleConfig::from_env`.
    #[must_use]
    pub fn with_lifecycle_enabled(mut self, enabled: bool) -> Self {
        self.lifecycle_config.enabled = enabled;
        self
    }

    /// Returns whether persisted query analytics are enabled.
    #[must_use]
    pub fn is_query_logging_enabled(&self) -> bool {
        self.query_logging_enabled
    }

    /// Returns the query-log retention window in days.
    #[must_use]
    pub fn query_log_retention_days(&self) -> u32 {
        self.query_log_retention_days
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> SurrealConfig {
        crate::config::SurrealConfigBuilder::new()
            .db_name("memory")
            .namespace("org")
            .credentials("root", "root")
            .embedded(true)
            .build()
            .expect("valid config")
    }

    #[test]
    fn startup_config_events_report_defaulted_variables_without_values() {
        let mut config = config();
        config.defaulted_variables = vec![
            "SURREALDB_DB_NAME",
            "SURREALDB_NAMESPACE",
            "SURREALDB_EMBEDDED",
            "SURREALDB_USERNAME",
            "SURREALDB_PASSWORD",
            "SURREALDB_DATA_DIR",
        ];

        let events = startup_config_events(&config);

        assert_eq!(events.len(), 6);
        for event in &events {
            assert_eq!(event.keys().count(), 2);
            assert_eq!(
                event.get("op"),
                Some(&serde_json::json!("config.default_applied"))
            );
            assert!(event.contains_key("variable"));
            assert!(
                !event
                    .values()
                    .any(|value| value == &serde_json::json!("root"))
            );
        }
    }

    #[test]
    fn startup_config_events_are_empty_for_explicit_configuration() {
        let config = config();

        assert!(startup_config_events(&config).is_empty());
    }

    #[test]
    fn startup_config_events_report_selected_legacy_path() {
        let mut config = config();
        config.legacy_data_dir = Some("/tmp/legacy/surrealdb".to_string());

        let events = startup_config_events(&config);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].keys().count(), 2);
        assert_eq!(
            events[0].get("op"),
            Some(&serde_json::json!("config.legacy_data_dir_detected"))
        );
        assert_eq!(
            events[0].get("path"),
            Some(&serde_json::json!("/tmp/legacy/surrealdb"))
        );
    }

    /// The `MemoryServiceDependencies` seam (Task 13) lets a
    /// caller inject a custom `TripleExtractor` without
    /// changing the public constructor signatures. The
    /// test builds a service through the public
    /// `new_with_embedding_provider` constructor and
    /// verifies the seam by exercising the stored
    /// `triple_extractor` field directly. A future caller
    /// that needs to swap the extractor can route through
    /// the new `MemoryServiceDependencies::with_db_client`
    /// builder, which the public constructors use
    /// internally; this test pins the wiring so the seam
    /// is observable in CI.
    #[tokio::test]
    async fn triple_extractor_is_retained_by_the_service() {
        use crate::shared::triple_extractor::TripleExtractor;
        use std::sync::Arc;

        // Build a real `DbClient` against an in-memory
        // Surreal engine. The service holds the
        // `Arc<dyn DbClient>`; no background workers are
        // spawned because the service is built without
        // env-driven startup, so the in-memory client
        // never tries to open a real socket.
        let db: surrealdb::Surreal<surrealdb::engine::local::Db> =
            surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
                .await
                .expect("mem engine init");
        db.use_ns("test_namespace")
            .use_db("memory")
            .await
            .expect("bind");
        let db_client: Arc<dyn crate::storage::DbClient> = Arc::new(
            crate::storage::SurrealDbClient::from_prebound_mem(db, "test_namespace", "error"),
        );

        // Build a service through the public constructor.
        // The seam is exercised by the constructor: the
        // `with_db_and_providers` helper injects the
        // production `RuleBasedTripleExtractor`, and the
        // service retains it.
        let service = MemoryService::new_with_embedding_provider(
            db_client,
            "test_namespace".to_string(),
            "error".to_string(),
            100,
            10,
            Arc::new(crate::embedding::providers::DisabledEmbeddingProvider::new(
                crate::config::DEFAULT_EMBEDDING_DIMENSION,
            )),
            crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
            Arc::new(crate::service::AnnoEntityExtractor::new().expect("default entity extractor")),
        )
        .expect("service builds");

        // Exercise the stored extractor through the trait
        // object. A regression that swapped the extractor
        // out would surface as a panic or a type error.
        let extractor: Arc<dyn TripleExtractor> = service.triple_extractor.clone();
        let triples = extractor
            .extract("X works at Y", "src-1")
            .await
            .expect("production extractor succeeds");
        // The rule-based extractor recognizes "X works at Y".
        assert!(
            !triples.is_empty(),
            "rule-based extractor should produce at least one triple"
        );
        assert!(triples.iter().any(|t| t.predicate == "works_at"));
    }
}
