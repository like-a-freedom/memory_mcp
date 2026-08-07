use std::sync::Arc;

use lru::LruCache;

use crate::config::SurrealConfig;
use crate::logging::StdoutLogger;
use crate::models::AssembledContextItem;
use crate::service::AnnoEntityExtractor;
use crate::service::EntityExtractor;
use crate::service::cache::CacheKey;
use crate::service::embedding::{
    DisabledEmbeddingProvider, EmbeddingProvider, create_embedding_provider_with_dimension,
};
use crate::service::entity_extraction::create_entity_extractor;
use crate::service::error::MemoryError;
use crate::service::startup::{
    EmbeddingActivationMode, EmbeddingStartupDecision, apply_startup_migrations,
    build_startup_versions_event, resolve_embedding_startup, write_bootstrap_ready_states,
};
use crate::service::util::RateLimiter;
use crate::storage::{DbClient, SurrealDbClient};

/// Core service for memory operations.
#[derive(Clone)]
pub struct MemoryService {
    /// Database client for storage operations.
    pub(crate) db_client: Arc<dyn DbClient>,
    pub(crate) namespaces: Vec<String>,
    pub(crate) default_namespace: String,
    pub(crate) logger: StdoutLogger,
    pub(crate) rate_limiter: Arc<RateLimiter>,
    pub(crate) ingestion_service: super::super::ingestion::IngestionService,
    pub(crate) entity_service: super::super::entity::EntityService,
    pub(crate) fact_service: super::super::fact::FactService,
    pub(crate) explanation_service: super::super::explanation::ExplanationService,
    pub(crate) context_cache:
        Arc<tokio::sync::RwLock<LruCache<CacheKey, Vec<AssembledContextItem>>>>,
    pub(crate) entity_extractor: Arc<dyn EntityExtractor>,
    pub(crate) embedding_provider: Arc<dyn EmbeddingProvider>,
    pub(crate) embedding_similarity_threshold: f64,
    pub(crate) current_embedding_signature: Option<String>,
    pub(crate) current_embedding_model: Option<String>,
    pub(crate) current_embedding_dimension: Option<usize>,
    pub(crate) task_runner: Arc<super::super::embedding::task_runner::BackgroundTaskRunner>,
    pub(crate) query_embedding_cache:
        Arc<tokio::sync::Mutex<LruCache<String, crate::service::CachedQueryEmbedding>>>,
    pub(crate) query_logging_enabled: bool,
    pub(crate) query_log_retention_days: u32,
    pub(crate) entity_resolver: super::super::entity_resolution::EntityResolver,
    pub(crate) triple_extractor: Arc<dyn super::super::triple_extractor::TripleExtractor>,
    pub(crate) lifecycle_config: crate::config::LifecycleConfig,
    #[allow(dead_code)]
    pub(crate) claim_service: super::super::claims::project::ClaimService,
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
        let config = SurrealConfig::from_env()?;
        let default_namespace = config
            .default_namespace()
            .ok_or_else(|| MemoryError::ConfigInvalid("namespaces cannot be empty".to_string()))?;

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
            "namespaces".to_string(),
            serde_json::json!(config.namespaces.clone()),
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

        let db_client = SurrealDbClient::connect(&config, default_namespace).await?;
        let server_version = match db_client.server_version(default_namespace).await {
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
        apply_startup_migrations(&db_client, &config.namespaces).await?;

        let (decision, target) = resolve_embedding_startup(
            &config.embedding,
            &db_client,
            &config.namespaces,
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
            | (_, EmbeddingStartupDecision::BootstrapReadyNamespaces { .. }) => {
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

        let entity_extractor =
            create_entity_extractor(&config.ner, &effective_data_dir, &startup_logger).await?;

        let mut service = Self::new_with_embedding_provider(
            db_client.clone(),
            config.namespaces,
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
        service.current_embedding_signature = target.as_ref().map(|value| value.signature.clone());
        service.current_embedding_model = target.as_ref().and_then(|value| value.model.clone());
        service.current_embedding_dimension = target.as_ref().map(|value| value.dimension);

        // Wire environment-driven claim configuration
        if let Ok(claim_config) = crate::config::claims::ClaimConfig::from_env() {
            service.claim_service = service.claim_service.clone().with_config(claim_config);
        }

        if let (
            EmbeddingStartupDecision::BootstrapReadyNamespaces {
                namespaces,
                active_signature,
            },
            Some(target),
        ) = (&decision, target.as_ref())
        {
            write_bootstrap_ready_states(
                &service.db_client,
                namespaces,
                active_signature,
                config.embedding.provider_label(),
                config.embedding.model.as_deref(),
                target.dimension,
            )
            .await?;

            let mut event = std::collections::HashMap::new();
            event.insert(
                "op".to_string(),
                serde_json::json!("embedding.bootstrap_ready_written"),
            );
            event.insert(
                "namespaces".to_string(),
                serde_json::json!(namespaces.clone()),
            );
            event.insert(
                "target_signature".to_string(),
                serde_json::json!(active_signature.clone()),
            );
            startup_logger.log(event, crate::logging::LogLevel::Info);
        }

        service.check_surrealdb_connection().await?;

        // Schedule one backfill job per namespace
        for ns in &service.namespaces {
            if let Err(e) = crate::service::claims::backfill::schedule_namespace_backfill(
                &service.claim_service,
                ns,
            )
            .await
            {
                startup_logger.log(
                    std::collections::HashMap::from([
                        (
                            "op".to_string(),
                            serde_json::json!("claim.backfill_schedule_failed"),
                        ),
                        ("namespace".to_string(), serde_json::json!(ns)),
                        ("error".to_string(), serde_json::json!(e.to_string())),
                    ]),
                    crate::logging::LogLevel::Warn,
                );
            }
        }

        // Spawn lifecycle workers if enabled
        let lifecycle_background_workers =
            super::super::lifecycle::spawn_workers_from_config(&service, &config.lifecycle);
        service.lifecycle_background_workers = Some(lifecycle_background_workers);

        Ok(service)
    }

    /// Creates a new service instance.
    pub fn new(
        db_client: Arc<dyn DbClient>,
        namespaces: Vec<String>,
        log_level: String,
        rate_limit_rps: i32,
        rate_limit_burst: i32,
    ) -> Result<Self, MemoryError> {
        Self::build(
            db_client,
            namespaces,
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

    pub(crate) fn new_with_embedding_provider(
        db_client: Arc<dyn DbClient>,
        namespaces: Vec<String>,
        log_level: String,
        rate_limit_rps: i32,
        rate_limit_burst: i32,
        embedding_provider: Arc<dyn EmbeddingProvider>,
        embedding_similarity_threshold: f64,
        entity_extractor: Arc<dyn EntityExtractor>,
    ) -> Result<Self, MemoryError> {
        Self::build(
            db_client,
            namespaces,
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

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn new_with_cache_size(
        db_client: Arc<dyn DbClient>,
        namespaces: Vec<String>,
        log_level: String,
        rate_limit_rps: i32,
        rate_limit_burst: i32,
        cache_size: usize,
    ) -> Result<Self, MemoryError> {
        Self::build(
            db_client,
            namespaces,
            log_level,
            ServiceBuildConfig {
                rate_limit_rps,
                rate_limit_burst,
                cache_size,
                embedding_similarity_threshold:
                    crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
            },
            Arc::new(DisabledEmbeddingProvider::new(
                crate::config::DEFAULT_EMBEDDING_DIMENSION,
            )),
            Arc::new(AnnoEntityExtractor::new()?),
        )
    }

    fn build(
        db_client: Arc<dyn DbClient>,
        namespaces: Vec<String>,
        log_level: String,
        build_config: ServiceBuildConfig,
        embedding_provider: Arc<dyn EmbeddingProvider>,
        entity_extractor: Arc<dyn EntityExtractor>,
    ) -> Result<Self, MemoryError> {
        if namespaces.is_empty() {
            return Err(MemoryError::ConfigInvalid(
                "namespaces cannot be empty".to_string(),
            ));
        }
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
            namespaces.clone(),
            logger.clone(),
            rate_limiter.clone(),
        );
        let entity_service = super::super::entity::EntityService::new(db_client.clone());
        let fact_service = super::super::fact::FactService::new(
            crate::storage::FactStoreClient::new(db_client.clone()),
        );
        let explanation_service = super::super::explanation::ExplanationService::new(
            db_client.clone(),
            logger.clone(),
            namespaces.clone(),
        );
        let claim_store = Arc::new(crate::storage::claims::SurrealClaimStore::new(
            db_client.clone(),
        ));
        let fuzzy_threshold = std::env::var("ENTITY_FUZZY_THRESHOLD")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(super::super::entity_resolution::DEFAULT_FUZZY_THRESHOLD);
        Ok(Self {
            db_client,
            namespaces: namespaces.clone(),
            default_namespace: namespaces[0].clone(),
            logger,
            rate_limiter,
            ingestion_service,
            entity_service,
            fact_service,
            explanation_service,
            context_cache: Arc::new(tokio::sync::RwLock::new(LruCache::new(cache_size))),
            entity_extractor,
            embedding_provider,
            embedding_similarity_threshold: build_config.embedding_similarity_threshold,
            current_embedding_signature: None,
            current_embedding_model: None,
            current_embedding_dimension: None,
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
            claim_service: super::super::claims::project::ClaimService::new(claim_store),
            trace_registry: Arc::new(
                super::super::agent_memory::recall::SessionTraceRegistry::new(),
            ),
            lifecycle_background_workers: None,
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
            "SURREALDB_NAMESPACES",
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
