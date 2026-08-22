use std::sync::Arc;

use lru::LruCache;

use crate::config::SurrealConfig;
use crate::error::MemoryError;
use crate::logging::StdoutLogger;
use crate::models::AssembledContextItem;
use crate::service::AnnoEntityExtractor;
use crate::service::EntityExtractor;
use crate::service::cache::CacheKey;
use crate::service::embedding::{
    DisabledEmbeddingProvider, EmbeddingProvider, create_embedding_provider_with_dimension,
};
use crate::service::embedding_recovery::{
    EmbeddingRecoveryRuntime, should_spawn_embedding_recovery,
};
use crate::service::embedding_runtime::EmbeddingRuntimeState;
use crate::service::entity_extraction::create_entity_extractor_with_progress;
use crate::service::startup::{
    EmbeddingActivationMode, EmbeddingStartupDecision, apply_startup_migrations,
    build_startup_versions_event, resolve_embedding_startup, write_bootstrap_ready_state,
};
use crate::service::util::RateLimiter;
use crate::storage::{DbClient, SurrealDbClient};

/// Core service for memory operations.
#[derive(Clone)]
pub struct MemoryService {
    /// Database client for storage operations.
    pub(crate) db_client: Arc<dyn DbClient>,
    pub(crate) active_namespace: String,
    pub(crate) logger: StdoutLogger,
    pub(crate) rate_limiter: Arc<RateLimiter>,
    pub(crate) ingestion_service: super::super::ingestion::IngestionService,
    pub(crate) entity_service: super::super::entity::EntityService,
    pub(crate) fact_service: super::super::fact::FactService,
    pub(crate) explanation_service: super::super::explanation::ExplanationService,
    pub(crate) context_cache:
        Arc<tokio::sync::RwLock<LruCache<CacheKey, Vec<AssembledContextItem>>>>,
    pub(crate) entity_extractor: Arc<dyn EntityExtractor>,
    pub(crate) embedding_runtime_state: Arc<std::sync::RwLock<EmbeddingRuntimeState>>,
    pub(crate) embedding_similarity_threshold: f64,
    pub(crate) task_runner: Arc<super::super::embedding::task_runner::BackgroundTaskRunner>,
    pub(crate) query_embedding_cache:
        Arc<tokio::sync::Mutex<LruCache<String, crate::service::CachedQueryEmbedding>>>,
    pub(crate) query_logging_enabled: bool,
    pub(crate) query_log_retention_days: u32,
    pub(crate) entity_resolver: super::super::entity_resolution::EntityResolver,
    pub(crate) triple_extractor: Arc<dyn super::super::triple_extractor::TripleExtractor>,
    pub(crate) lifecycle_config: crate::config::LifecycleConfig,
    pub(crate) claim_service: super::super::claims::projection::ClaimService,
    /// Shared per-session exposure-trace registry for selective recall.
    ///
    /// Holds at most 32 traces per session for 30 minutes. Persists only when a
    /// later significant capture links a trace (ADR-0016 AD-7).
    pub(crate) trace_registry: Arc<super::super::agent_memory::recall::SessionTraceRegistry>,
    /// Owned runtime for the lifecycle background workers (decay, archival,
    /// community). `None` when constructed via the test builders that do not
    /// spawn lifecycle workers; populated by `new_from_env_with_mode`.
    pub(crate) lifecycle_background_workers:
        Option<super::super::lifecycle::LifecycleBackgroundWorkerRuntime>,
    /// Owned runtime for remote embedding recovery after a degraded startup.
    pub(crate) embedding_recovery_runtime: Option<EmbeddingRecoveryRuntime>,
    /// Bounded-concurrency semaphore for fire-and-forget triple extraction
    /// tasks. Limits in-flight extraction tasks to prevent unbounded task
    /// spawning under load.
    pub(crate) triple_extraction_semaphore: Arc<tokio::sync::Semaphore>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ServiceBuildConfig {
    pub(super) rate_limit_rps: i32,
    pub(super) rate_limit_burst: i32,
    pub(super) cache_size: usize,
    pub(super) embedding_similarity_threshold: f64,
}

fn startup_config_events(
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
    /// Creates a new `MemoryService` from environment variables.
    pub async fn new_from_env() -> Result<Self, MemoryError> {
        Self::new_from_env_with_mode(EmbeddingActivationMode::Standard).await
    }

    pub(crate) async fn new_from_env_with_mode(
        mode: EmbeddingActivationMode,
    ) -> Result<Self, MemoryError> {
        Self::new_from_env_with_mode_and_progress(
            mode,
            std::sync::Arc::new(crate::service::model_artifacts::CliProgressSink::new()),
        )
        .await
    }

    /// Creates a service with an explicit model-progress sink. MCP stdio
    /// processes pass [`crate::service::model_artifacts::JsonLineProgressSink`]
    /// so stdout stays JSON-RPC-only; CLI paths use the default human sink.
    pub(crate) async fn new_from_env_with_mode_and_progress(
        mode: EmbeddingActivationMode,
        ner_progress: std::sync::Arc<dyn crate::service::model_artifacts::ModelProgressSink>,
    ) -> Result<Self, MemoryError> {
        let config = SurrealConfig::from_env()?;
        let active_namespace = config.active_namespace().as_str().to_string();

        let effective_data_dir = config.data_dir_or_default();
        let startup_logger = crate::logging::StdoutLogger::new(&config.log_level);
        for event in startup_config_events(&config) {
            startup_logger.log(event, crate::logging::LogLevel::Info);
        }
        let mut startup_event = std::collections::HashMap::new();
        startup_event.insert("op".to_string(), serde_json::json!("startup"));
        startup_event.insert(
            "db_mode".to_string(),
            serde_json::json!(if config.embedded {
                "embedded"
            } else {
                "remote"
            }),
        );
        startup_event.insert(
            "namespace".to_string(),
            serde_json::json!(config.active_namespace().as_str()),
        );
        startup_event.insert(
            "query_logging_enabled".to_string(),
            serde_json::json!(config.query_logging_enabled),
        );
        startup_event.insert(
            "query_log_retention_days".to_string(),
            serde_json::json!(config.query_log_retention_days),
        );
        if config.embedded {
            startup_event.insert(
                "data_dir".to_string(),
                serde_json::json!(effective_data_dir),
            );
        } else if let Some(url) = &config.url {
            startup_event.insert("url".to_string(), serde_json::json!(url));
        }
        startup_logger.log(startup_event, crate::logging::LogLevel::Info);

        let db_client = SurrealDbClient::connect(&config).await?;
        let server_version = match db_client.server_version(&active_namespace).await {
            Ok(version) => version,
            Err(err) => {
                let mut event = std::collections::HashMap::new();
                event.insert(
                    "op".to_string(),
                    serde_json::json!("startup.version_probe_failed"),
                );
                event.insert("error".to_string(), serde_json::json!(err.to_string()));
                startup_logger.log(event, crate::logging::LogLevel::Warn);
                None
            }
        };

        let client_version = option_env!("CARGO_PKG_VERSION").unwrap_or("unknown");
        let versions_event =
            build_startup_versions_event(client_version, server_version.as_deref());
        startup_logger.log(versions_event, crate::logging::LogLevel::Info);

        let db_client = Arc::new(db_client) as Arc<dyn DbClient>;
        apply_startup_migrations(&db_client, &active_namespace).await?;

        let (decision, target) = resolve_embedding_startup(
            &config.embedding,
            &db_client,
            &active_namespace,
            &effective_data_dir,
            &startup_logger,
        )
        .await?;

        let embedding_provider: Arc<dyn EmbeddingProvider> = match (&mode, &decision) {
            (EmbeddingActivationMode::ForceEnabledForReembed, _) => {
                let target = target.as_ref().ok_or_else(|| {
                    MemoryError::ConfigInvalid(
                        "reembed mode requires a resolved embedding target".to_string(),
                    )
                })?;
                create_embedding_provider_with_dimension(
                    &config.embedding,
                    &effective_data_dir,
                    target.dimension,
                )
                .await?
            }
            (_, EmbeddingStartupDecision::UseConfiguredProvider)
            | (_, EmbeddingStartupDecision::ResumePendingBackfill { .. })
            | (_, EmbeddingStartupDecision::BootstrapReadyNamespace { .. }) => {
                create_embedding_provider_with_dimension(
                    &config.embedding,
                    &effective_data_dir,
                    target
                        .as_ref()
                        .map(|value| value.dimension)
                        .unwrap_or_else(|| config.embedding.fallback_dimension()),
                )
                .await?
            }
            (_, EmbeddingStartupDecision::RecoverMissingEmbeddings { target_signature }) => {
                let mut event = std::collections::HashMap::new();
                event.insert(
                    "op".to_string(),
                    serde_json::json!("embedding.rebuild_required"),
                );
                event.insert(
                    "reason".to_string(),
                    serde_json::json!(
                        "configured embedding signature differs; missing embeddings will be recovered without rewriting existing vectors"
                    ),
                );
                event.insert(
                    "target_signature".to_string(),
                    serde_json::json!(target_signature),
                );
                startup_logger.log(event, crate::logging::LogLevel::Warn);
                Arc::new(DisabledEmbeddingProvider::new(
                    target
                        .as_ref()
                        .map(|value| value.dimension)
                        .unwrap_or_else(|| config.embedding.fallback_dimension()),
                ))
            }
            (_, EmbeddingStartupDecision::DisableSemantic { reason }) => {
                let mut event = std::collections::HashMap::new();
                event.insert(
                    "op".to_string(),
                    serde_json::json!("embedding.rebuild_required"),
                );
                event.insert("reason".to_string(), serde_json::json!(reason));
                event.insert(
                    "target_signature".to_string(),
                    serde_json::json!(target.as_ref().map(|value| value.signature.clone())),
                );
                startup_logger.log(event, crate::logging::LogLevel::Warn);
                Arc::new(DisabledEmbeddingProvider::new(
                    target
                        .as_ref()
                        .map(|value| value.dimension)
                        .unwrap_or_else(|| config.embedding.fallback_dimension()),
                ))
            }
        };

        let entity_extractor = create_entity_extractor_with_progress(
            &config.ner,
            &effective_data_dir,
            &startup_logger,
            ner_progress,
        )
        .await?;

        let runtime_provider = embedding_provider.clone();
        let mut service = Self::new_with_embedding_provider(
            db_client.clone(),
            config.active_namespace().as_str().to_string(),
            config.log_level,
            50,
            100,
            embedding_provider,
            config.embedding.similarity_threshold,
            entity_extractor,
        )?
        .with_query_logging_enabled(config.query_logging_enabled)
        .with_query_log_retention_days(config.query_log_retention_days);
        service.lifecycle_config = config.lifecycle.clone();
        service.replace_embedding_runtime_state(EmbeddingRuntimeState::new(
            runtime_provider,
            target.as_ref().map(|value| value.signature.clone()),
            target.as_ref().and_then(|value| value.model.clone()),
            target.as_ref().map(|value| value.dimension),
        ));

        // Wire environment-driven claim configuration
        if let Ok(claim_config) = crate::config::claims::ClaimConfig::from_env() {
            service.claim_service = service.claim_service.clone().with_config(claim_config);
        }

        if let (
            EmbeddingStartupDecision::BootstrapReadyNamespace { active_signature },
            Some(target),
        ) = (&decision, target.as_ref())
        {
            let bound_db = crate::storage::BoundDbClient::new(
                service.db_client.clone(),
                service.active_namespace.clone(),
            );
            write_bootstrap_ready_state(
                &bound_db,
                active_signature,
                config.embedding.provider_label(),
                config.embedding.model.as_deref(),
                target.dimension,
                false,
            )
            .await?;

            let mut event = std::collections::HashMap::new();
            event.insert(
                "op".to_string(),
                serde_json::json!("embedding.bootstrap_ready_written"),
            );
            event.insert(
                "namespace".to_string(),
                serde_json::json!(service.active_namespace.clone()),
            );
            event.insert(
                "target_signature".to_string(),
                serde_json::json!(active_signature.clone()),
            );
            startup_logger.log(event, crate::logging::LogLevel::Info);
        }

        service.check_surrealdb_connection().await?;

        // The initial durable backfill schedule is part of readiness. A worker
        // must never start with a best-effort, in-memory-only promise to process
        // legacy facts later.
        crate::service::claims::backfill::schedule_namespace_backfill(
            &service.claim_service,
            &service.active_namespace,
        )
        .await?;

        // Spawn lifecycle workers if enabled
        let lifecycle_background_workers =
            super::super::lifecycle::spawn_workers_from_config(&service, &config.lifecycle);
        service.lifecycle_background_workers = Some(lifecycle_background_workers);

        if should_spawn_embedding_recovery(mode, &decision, &config.embedding) {
            service.embedding_recovery_runtime = Some(
                service
                    .start_embedding_recovery_worker(
                        config.embedding.clone(),
                        effective_data_dir.clone(),
                    )
                    .await,
            );
        }

        Ok(service)
    }

    /// Creates a new service instance.
    pub fn new(
        db_client: Arc<dyn DbClient>,
        active_namespace: String,
        log_level: String,
        rate_limit_rps: i32,
        rate_limit_burst: i32,
    ) -> Result<Self, MemoryError> {
        Self::build(
            db_client,
            active_namespace,
            log_level,
            ServiceBuildConfig {
                rate_limit_rps,
                rate_limit_burst,
                cache_size: crate::service::CONTEXT_CACHE_SIZE,
                embedding_similarity_threshold:
                    crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
            },
            Arc::new(DisabledEmbeddingProvider::new(
                crate::config::DEFAULT_EMBEDDING_DIMENSION,
            )),
            Arc::new(AnnoEntityExtractor::new()?),
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
            db_client,
            active_namespace,
            log_level,
            ServiceBuildConfig {
                rate_limit_rps,
                rate_limit_burst,
                cache_size: crate::service::CONTEXT_CACHE_SIZE,
                embedding_similarity_threshold,
            },
            embedding_provider,
            entity_extractor,
        )
    }

    fn build(
        db_client: Arc<dyn DbClient>,
        active_namespace: String,
        log_level: String,
        build_config: ServiceBuildConfig,
        embedding_provider: Arc<dyn EmbeddingProvider>,
        entity_extractor: Arc<dyn EntityExtractor>,
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
        let ingestion_service = super::super::ingestion::IngestionService::new(
            db_client.clone(),
            active_namespace.clone(),
            logger.clone(),
            rate_limiter.clone(),
        );
        let entity_service =
            super::super::entity::EntityService::new(db_client.clone(), active_namespace.clone());
        let fact_service = super::super::fact::FactService::new(
            crate::storage::FactStoreClient::new(db_client.clone(), active_namespace.clone()),
        );
        let explanation_service = super::super::explanation::ExplanationService::new(
            db_client.clone(),
            logger.clone(),
            active_namespace.clone(),
        );
        let claim_store = Arc::new(crate::storage::claims::SurrealClaimStore::new(
            db_client.clone(),
            active_namespace.clone(),
        ));
        let fuzzy_threshold = crate::config::ner::entity_fuzzy_threshold()?;
        Ok(Self {
            db_client,
            active_namespace,
            logger,
            rate_limiter,
            ingestion_service,
            entity_service,
            fact_service,
            explanation_service,
            context_cache: Arc::new(tokio::sync::RwLock::new(LruCache::new(cache_size))),
            entity_extractor,
            embedding_runtime_state: Arc::new(std::sync::RwLock::new(EmbeddingRuntimeState::new(
                embedding_provider,
                None,
                None,
                None,
            ))),
            embedding_similarity_threshold: build_config.embedding_similarity_threshold,
            task_runner: Arc::new(
                super::super::embedding::task_runner::BackgroundTaskRunner::new(),
            ),
            query_embedding_cache: Arc::new(tokio::sync::Mutex::new(LruCache::new(
                query_embedding_cache_size,
            ))),
            query_logging_enabled: false,
            query_log_retention_days: crate::config::DEFAULT_QUERY_LOG_RETENTION_DAYS,
            entity_resolver: super::super::entity_resolution::EntityResolver::new(fuzzy_threshold),
            triple_extractor: Arc::new(
                super::super::triple_extractor::RuleBasedTripleExtractor::new(),
            ),
            lifecycle_config: crate::config::LifecycleConfig::default(),
            claim_service: super::super::claims::projection::ClaimService::new(claim_store),
            trace_registry: Arc::new(
                super::super::agent_memory::recall::SessionTraceRegistry::new(),
            ),
            lifecycle_background_workers: None,
            embedding_recovery_runtime: None,
            triple_extraction_semaphore: Arc::new(tokio::sync::Semaphore::new(
                crate::service::TRIPLE_EXTRACTION_MAX_CONCURRENCY,
            )),
        })
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

    /// Returns a copy of the service with lifecycle integration enabled or
    /// disabled. This controls whether `lifecycle_capture` returns `Some` and
    /// whether the projection worker is started.
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
}
