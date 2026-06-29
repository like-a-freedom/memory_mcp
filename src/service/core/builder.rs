use std::collections::HashSet;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use lru::LruCache;

use crate::config::SurrealConfig;
use crate::logging::StdoutLogger;
use crate::models::{
    AccessPayload, AssembleContextRequest, AssembledContextItem, IngestRequest, InvalidateRequest,
};
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
    pub(crate) ingestion_service: super::super::ingestion::IngestionService,
    pub(crate) entity_service: super::super::entity::EntityService,
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
                Err(err)
                    if mode == EmbeddingActivationMode::ForceEnabledForReembed
                        && config.embedding.dimension_override.is_some() =>
                {
                    // Probe failed but operator provided an explicit dimension
                    // override — fall back to it with a strong warning.
                    let dimension = config.embedding.dimension_override.unwrap();
                    let signature = crate::config::build_embedding_signature(
                        config.embedding.provider_label(),
                        config.embedding.model.as_deref(),
                        config.embedding.base_url.as_deref(),
                        dimension,
                    );
                    let mut event = std::collections::HashMap::new();
                    event.insert(
                        "op".to_string(),
                        serde_json::json!("embedding.preflight_fallback"),
                    );
                    event.insert("error".to_string(), serde_json::json!(err.to_string()));
                    event.insert(
                        "provider".to_string(),
                        serde_json::json!(config.embedding.provider_label()),
                    );
                    event.insert("dimension".to_string(), serde_json::json!(dimension));
                    startup_logger.log(event, crate::logging::LogLevel::Warn);
                    Some(crate::service::embedding::ResolvedEmbeddingTarget {
                        provider_label: config.embedding.provider_label(),
                        model: config.embedding.model.clone(),
                        dimension,
                        signature,
                    })
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
        super::super::lifecycle::spawn_workers_from_config(&service, &config.lifecycle);

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
        let entity_service = super::super::entity::EntityService::new(
            db_client.clone(),
            namespaces[0].clone(),
            rate_limiter.clone(),
        );
        Ok(Self {
            db_client,
            namespaces: namespaces.clone(),
            default_namespace: namespaces[0].clone(),
            logger,
            rate_limiter,
            ingestion_service,
            entity_service,
            context_cache: Arc::new(tokio::sync::RwLock::new(LruCache::new(cache_size))),
            entity_extractor: Arc::new(AnnoEntityExtractor::new()?),
            embedding_provider,
            embedding_similarity_threshold: build_config.embedding_similarity_threshold,
            current_embedding_signature: None,
            current_embedding_model: None,
            current_embedding_dimension: None,
            task_runner: Arc::new(super::super::embedding::task_runner::BackgroundTaskRunner::new()),
            query_embedding_cache: Arc::new(tokio::sync::Mutex::new(LruCache::new(
                query_embedding_cache_size,
            ))),
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

impl IngestRequest {
    /// Creates a new builder for IngestRequest.
    pub fn builder() -> IngestRequestBuilder {
        IngestRequestBuilder::default()
    }
}

/// Builder for IngestRequest.
#[derive(Default)]
pub struct IngestRequestBuilder {
    source_type: Option<String>,
    source_id: Option<String>,
    content: Option<String>,
    t_ref: Option<DateTime<Utc>>,
    scope: Option<String>,
    project: Option<String>,
    t_ingested: Option<DateTime<Utc>>,
    visibility_scope: Option<String>,
    policy_tags: Vec<String>,
}

impl IngestRequestBuilder {
    /// Sets the source type.
    pub fn source_type(mut self, value: impl Into<String>) -> Self {
        self.source_type = Some(value.into());
        self
    }

    /// Sets the source ID.
    pub fn source_id(mut self, value: impl Into<String>) -> Self {
        self.source_id = Some(value.into());
        self
    }

    /// Sets the content.
    pub fn content(mut self, value: impl Into<String>) -> Self {
        self.content = Some(value.into());
        self
    }

    /// Sets the reference timestamp.
    pub fn t_ref(mut self, value: DateTime<Utc>) -> Self {
        self.t_ref = Some(value);
        self
    }

    /// Sets the scope.
    pub fn scope(mut self, value: impl Into<String>) -> Self {
        self.scope = Some(value.into());
        self
    }

    /// Sets the project.
    pub fn project(mut self, value: impl Into<String>) -> Self {
        self.project = Some(value.into());
        self
    }

    /// Sets the ingestion timestamp.
    pub fn t_ingested(mut self, value: DateTime<Utc>) -> Self {
        self.t_ingested = Some(value);
        self
    }

    /// Sets the visibility scope.
    pub fn visibility_scope(mut self, value: impl Into<String>) -> Self {
        self.visibility_scope = Some(value.into());
        self
    }

    /// Sets the policy tags.
    pub fn policy_tags(mut self, value: Vec<String>) -> Self {
        self.policy_tags = value;
        self
    }

    /// Builds the IngestRequest.
    pub fn build(self) -> Result<IngestRequest, String> {
        Ok(IngestRequest {
            source_type: self.source_type.ok_or("source_type is required")?,
            source_id: self.source_id.ok_or("source_id is required")?,
            content: self.content.ok_or("content is required")?,
            t_ref: self.t_ref.ok_or("t_ref is required")?,
            scope: self.scope.ok_or("scope is required")?,
            project: self.project,
            t_ingested: self.t_ingested,
            visibility_scope: self.visibility_scope,
            policy_tags: self.policy_tags,
        })
    }
}

impl InvalidateRequest {
    /// Creates a new builder for InvalidateRequest.
    pub fn builder() -> InvalidateRequestBuilder {
        InvalidateRequestBuilder::default()
    }
}

/// Builder for InvalidateRequest.
#[derive(Default)]
pub struct InvalidateRequestBuilder {
    fact_id: Option<String>,
    reason: Option<String>,
    t_invalid: Option<DateTime<Utc>>,
}

impl InvalidateRequestBuilder {
    /// Sets the fact ID.
    pub fn fact_id(mut self, value: impl Into<String>) -> Self {
        self.fact_id = Some(value.into());
        self
    }

    /// Sets the reason.
    pub fn reason(mut self, value: impl Into<String>) -> Self {
        self.reason = Some(value.into());
        self
    }

    /// Sets the invalidation timestamp.
    pub fn t_invalid(mut self, value: DateTime<Utc>) -> Self {
        self.t_invalid = Some(value);
        self
    }

    /// Builds the InvalidateRequest.
    pub fn build(self) -> Result<InvalidateRequest, String> {
        Ok(InvalidateRequest {
            fact_id: self.fact_id.ok_or("fact_id is required")?,
            reason: self.reason.ok_or("reason is required")?,
            t_invalid: self.t_invalid.ok_or("t_invalid is required")?,
        })
    }
}

impl AssembleContextRequest {
    /// Creates a new builder for AssembleContextRequest.
    pub fn builder() -> AssembleContextRequestBuilder {
        AssembleContextRequestBuilder::default()
    }
}

/// Builder for AssembleContextRequest.
#[derive(Default)]
pub struct AssembleContextRequestBuilder {
    query: Option<String>,
    scope: Option<String>,
    as_of: Option<DateTime<Utc>>,
    budget: Option<i32>,
    project: Option<String>,
    fact_types: Vec<String>,
    view_mode: Option<String>,
    window_start: Option<DateTime<Utc>>,
    window_end: Option<DateTime<Utc>>,
    access: Option<AccessPayload>,
}

impl AssembleContextRequestBuilder {
    /// Sets the query.
    pub fn query(mut self, value: impl Into<String>) -> Self {
        self.query = Some(value.into());
        self
    }

    /// Sets the scope.
    pub fn scope(mut self, value: impl Into<String>) -> Self {
        self.scope = Some(value.into());
        self
    }

    /// Sets the as-of timestamp.
    pub fn as_of(mut self, value: DateTime<Utc>) -> Self {
        self.as_of = Some(value);
        self
    }

    /// Sets the budget.
    pub fn budget(mut self, value: i32) -> Self {
        self.budget = Some(value);
        self
    }

    /// Sets the project.
    pub fn project(mut self, value: impl Into<String>) -> Self {
        self.project = Some(value.into());
        self
    }

    /// Sets the fact types.
    pub fn fact_types(mut self, value: Vec<String>) -> Self {
        self.fact_types = value;
        self
    }

    /// Sets the view mode.
    pub fn view_mode(mut self, value: impl Into<String>) -> Self {
        self.view_mode = Some(value.into());
        self
    }

    /// Sets the window start.
    pub fn window_start(mut self, value: DateTime<Utc>) -> Self {
        self.window_start = Some(value);
        self
    }

    /// Sets the window end.
    pub fn window_end(mut self, value: DateTime<Utc>) -> Self {
        self.window_end = Some(value);
        self
    }

    /// Sets the access payload.
    pub fn access(mut self, value: AccessPayload) -> Self {
        self.access = Some(value);
        self
    }

    /// Builds the AssembleContextRequest.
    pub fn build(self) -> Result<AssembleContextRequest, String> {
        Ok(AssembleContextRequest {
            query: self.query.ok_or("query is required")?,
            scope: self.scope.ok_or("scope is required")?,
            as_of: self.as_of,
            budget: self.budget.unwrap_or(5),
            project: self.project,
            fact_types: self.fact_types,
            view_mode: self.view_mode,
            window_start: self.window_start,
            window_end: self.window_end,
            access: self.access,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ingest_request_builder() {
        let request = IngestRequest::builder()
            .source_type("email")
            .source_id("MSG-201")
            .content("test content")
            .t_ref(Utc::now())
            .scope("org")
            .build()
            .unwrap();

        assert_eq!(request.source_type, "email");
        assert_eq!(request.source_id, "MSG-201");
        assert_eq!(request.content, "test content");
        assert_eq!(request.scope, "org");
    }

    #[test]
    fn test_ingest_request_builder_missing_required() {
        let result = IngestRequest::builder()
            .source_type("email")
            .build();

        assert!(result.is_err());
    }

    #[test]
    fn test_invalidate_request_builder() {
        let request = InvalidateRequest::builder()
            .fact_id("fact:123")
            .reason("outdated")
            .t_invalid(Utc::now())
            .build()
            .unwrap();

        assert_eq!(request.fact_id, "fact:123");
        assert_eq!(request.reason, "outdated");
    }

    #[test]
    fn test_assemble_context_request_builder() {
        let request = AssembleContextRequest::builder()
            .query("test query")
            .scope("org")
            .budget(5)
            .build()
            .unwrap();

        assert_eq!(request.query, "test query");
        assert_eq!(request.scope, "org");
        assert_eq!(request.budget, 5);
    }
}
