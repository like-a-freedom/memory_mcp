use std::sync::Arc;

use lru::LruCache;

use crate::config::SurrealConfig;
use crate::logging::StdoutLogger;
use crate::models::AssembledContextItem;
use crate::service::AnnoEntityExtractor;
use crate::service::EntityExtractor;
use crate::service::cache::CacheKey;
use crate::service::embedding::{
    DisabledEmbeddingProvider, EmbeddingProvider, create_embedding_provider,
};
use crate::service::entity_extraction::create_entity_extractor;
use crate::service::error::MemoryError;
use crate::service::lifecycle::{
    spawn_archival_worker, spawn_community_worker, spawn_decay_worker,
};
use crate::service::rate_limit::RateLimiter;
use crate::service::startup::{apply_startup_migrations, build_startup_versions_event};
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

        let embedding_provider =
            create_embedding_provider(&config.embedding, &effective_data_dir).await?;
        let entity_extractor =
            create_entity_extractor(&config.ner, &effective_data_dir, &startup_logger).await?;

        let mut service = Self::new_with_embedding_provider(
            Arc::new(db_client),
            config.namespaces,
            config.log_level,
            50,
            100,
            embedding_provider,
            config.embedding.similarity_threshold,
        )?
        .with_query_logging_enabled(config.query_logging_enabled)
        .with_query_log_retention_days(config.query_log_retention_days);
        service.entity_extractor = entity_extractor;
        apply_startup_migrations(&service.db_client, &service.namespaces).await?;
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
