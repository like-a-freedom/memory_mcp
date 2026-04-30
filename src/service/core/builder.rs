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
    resolve_embedding_target_identity,
};
use crate::service::entity_extraction::create_entity_extractor;
use crate::service::error::MemoryError;
use crate::service::lifecycle::{
    spawn_archival_worker, spawn_community_worker, spawn_decay_worker,
};
use crate::service::startup::{
    EmbeddingActivationMode, EmbeddingStartupDecision, LEGACY_EMBEDDING_SAMPLE_SIZE,
    apply_startup_migrations, build_startup_versions_event, count_facts_per_namespace,
    decide_embedding_startup, load_embedding_states, sample_stored_embedding_dimensions,
    write_bootstrap_ready_states,
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
    pub(crate) context_cache:
        Arc<tokio::sync::RwLock<LruCache<CacheKey, Vec<AssembledContextItem>>>>,
    pub(crate) entity_extractor: Arc<dyn EntityExtractor>,
    pub(crate) embedding_provider: Arc<dyn EmbeddingProvider>,
    pub(crate) embedding_similarity_threshold: f64,
    pub(crate) current_embedding_signature: Option<String>,
    pub(crate) current_embedding_model: Option<String>,
    pub(crate) current_embedding_dimension: Option<usize>,
    pub(crate) query_logging_enabled: bool,
    pub(crate) query_log_retention_days: u32,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ServiceBuildConfig {
    pub(super) rate_limit_rps: i32,
    pub(super) rate_limit_burst: i32,
    pub(super) cache_size: usize,
    pub(super) embedding_similarity_threshold: f64,
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

        let target = if config.embedding.is_enabled() {
            let mut event = std::collections::HashMap::new();
            event.insert(
                "op".to_string(),
                serde_json::json!("embedding.preflight_started"),
            );
            event.insert(
                "provider".to_string(),
                serde_json::json!(config.embedding.provider_label()),
            );
            event.insert(
                "model".to_string(),
                serde_json::json!(config.embedding.model.clone()),
            );
            startup_logger.log(event, crate::logging::LogLevel::Debug);

            match resolve_embedding_target_identity(&config.embedding, &effective_data_dir).await {
                Ok(target) => {
                    let mut event = std::collections::HashMap::new();
                    event.insert(
                        "op".to_string(),
                        serde_json::json!("embedding.preflight_succeeded"),
                    );
                    event.insert(
                        "provider".to_string(),
                        serde_json::json!(target.provider_label),
                    );
                    event.insert("model".to_string(), serde_json::json!(target.model.clone()));
                    event.insert("dimension".to_string(), serde_json::json!(target.dimension));
                    event.insert(
                        "target_signature".to_string(),
                        serde_json::json!(target.signature.clone()),
                    );
                    startup_logger.log(event, crate::logging::LogLevel::Info);
                    Some(target)
                }
                Err(err) if mode == EmbeddingActivationMode::Standard => {
                    let mut event = std::collections::HashMap::new();
                    event.insert(
                        "op".to_string(),
                        serde_json::json!("embedding.preflight_failed"),
                    );
                    event.insert("error".to_string(), serde_json::json!(err.to_string()));
                    startup_logger.log(event, crate::logging::LogLevel::Warn);
                    None
                }
                Err(err) => return Err(err),
            }
        } else {
            None
        };

        let decision = if let Some(target) = target.as_ref() {
            let namespace_states = load_embedding_states(&db_client, &config.namespaces).await?;
            let fact_counts = count_facts_per_namespace(&db_client, &config.namespaces).await?;
            let sample_dimensions = sample_stored_embedding_dimensions(
                &db_client,
                &config.namespaces,
                LEGACY_EMBEDDING_SAMPLE_SIZE,
            )
            .await?;

            let mut event = std::collections::HashMap::new();
            event.insert(
                "op".to_string(),
                serde_json::json!("embedding.startup_state_loaded"),
            );
            event.insert(
                "namespaces".to_string(),
                serde_json::json!(config.namespaces.clone()),
            );
            event.insert(
                "state_count".to_string(),
                serde_json::json!(namespace_states.len()),
            );
            event.insert(
                "fact_counts".to_string(),
                serde_json::json!(fact_counts.clone()),
            );
            startup_logger.log(event, crate::logging::LogLevel::Debug);

            decide_embedding_startup(
                &config.namespaces,
                &namespace_states,
                &target.signature,
                &sample_dimensions,
                &fact_counts,
                target.dimension,
            )
        } else if config.embedding.is_enabled() {
            EmbeddingStartupDecision::DisableSemantic {
                reason: "embedding target preflight failed".to_string(),
            }
        } else {
            EmbeddingStartupDecision::UseConfiguredProvider
        };

        let mut decision_event = std::collections::HashMap::new();
        decision_event.insert(
            "op".to_string(),
            serde_json::json!("embedding.startup_decision"),
        );
        decision_event.insert(
            "decision".to_string(),
            serde_json::json!(format!("{:?}", decision)),
        );
        decision_event.insert(
            "namespaces".to_string(),
            serde_json::json!(config.namespaces.clone()),
        );
        decision_event.insert(
            "target_signature".to_string(),
            serde_json::json!(target.as_ref().map(|value| value.signature.clone())),
        );
        startup_logger.log(decision_event, crate::logging::LogLevel::Info);

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
        )?
        .with_query_logging_enabled(config.query_logging_enabled)
        .with_query_log_retention_days(config.query_log_retention_days);
        service.current_embedding_signature = target.as_ref().map(|value| value.signature.clone());
        service.current_embedding_model = target.as_ref().and_then(|value| value.model.clone());
        service.current_embedding_dimension = target.as_ref().map(|value| value.dimension);
        service.entity_extractor = entity_extractor;

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

        // Spawn lifecycle workers if enabled
        if config.lifecycle.enabled {
            let decay_service = service.clone();
            let decay_config = config.lifecycle.clone();

            let _decay_handle = spawn_decay_worker(
                decay_service,
                decay_config.decay_interval_secs,
                decay_config.decay_confidence_threshold,
                decay_config.decay_half_life_days,
            );

            let archival_service = service.clone();
            let archival_config = config.lifecycle.clone();

            let _archival_handle = spawn_archival_worker(
                archival_service,
                archival_config.archival_interval_secs,
                archival_config.archival_age_days,
            );

            let community_service = service.clone();
            let community_config = config.lifecycle.clone();

            let _community_handle =
                spawn_community_worker(community_service, community_config.archival_interval_secs);

            let mut event = std::collections::HashMap::new();
            event.insert(
                "op".to_string(),
                serde_json::json!("lifecycle.workers.started"),
            );
            event.insert(
                "decay_interval".to_string(),
                serde_json::json!(config.lifecycle.decay_interval_secs),
            );
            event.insert(
                "archival_interval".to_string(),
                serde_json::json!(config.lifecycle.archival_interval_secs),
            );
            event.insert(
                "community_interval".to_string(),
                serde_json::json!(config.lifecycle.archival_interval_secs),
            );
            service.logger.log(event, crate::logging::LogLevel::Info);
        }

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
        )
    }

    fn build(
        db_client: Arc<dyn DbClient>,
        namespaces: Vec<String>,
        log_level: String,
        build_config: ServiceBuildConfig,
        embedding_provider: Arc<dyn EmbeddingProvider>,
    ) -> Result<Self, MemoryError> {
        if namespaces.is_empty() {
            return Err(MemoryError::ConfigInvalid(
                "namespaces cannot be empty".to_string(),
            ));
        }
        let cache_size = std::num::NonZeroUsize::new(build_config.cache_size).ok_or_else(|| {
            MemoryError::ConfigInvalid("context cache size must be > 0".to_string())
        })?;
        let logger = StdoutLogger::new(&log_level);
        Ok(Self {
            db_client,
            namespaces: namespaces.clone(),
            default_namespace: namespaces[0].clone(),
            logger,
            rate_limiter: Arc::new(RateLimiter::new(
                build_config.rate_limit_rps,
                build_config.rate_limit_burst,
            )),
            context_cache: Arc::new(tokio::sync::RwLock::new(LruCache::new(cache_size))),
            entity_extractor: Arc::new(AnnoEntityExtractor::new()?),
            embedding_provider,
            embedding_similarity_threshold: build_config.embedding_similarity_threshold,
            current_embedding_signature: None,
            current_embedding_model: None,
            current_embedding_dimension: None,
            query_logging_enabled: false,
            query_log_retention_days: crate::config::DEFAULT_QUERY_LOG_RETENTION_DAYS,
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
