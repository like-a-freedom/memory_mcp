//! The stdio composition root: read the environment, start the service.
//!
//! This used to be `MemoryService::new_from_env_with_mode_and_progress` in
//! `service/core/builder.rs`, which put 279 lines of startup policy in the
//! file that constructs the container. ADR-0067 moves it: the container
//! constructs and holds; the composition root starts.
use std::sync::Arc;

use crate::config::SurrealConfig;
use crate::embedding::providers::{
    DisabledEmbeddingProvider, EmbeddingProvider, create_embedding_provider_with_dimension,
};
use crate::embedding::runtime::EmbeddingRuntimeState;
use crate::error::MemoryError;
use crate::knowledge::entity_extraction::create_entity_extractor_with_progress;
use crate::service::MemoryService;
use crate::service::core::builder::startup_config_events;
use crate::service::embedding_recovery::should_spawn_embedding_recovery;
use crate::service::startup::{
    EmbeddingStartupDecision, apply_startup_migrations, build_startup_versions_event,
    resolve_embedding_startup, write_bootstrap_ready_state,
};
use crate::storage::{DbClient, SurrealDbClient};

pub use crate::service::startup::EmbeddingActivationMode;

/// A progress sink that reports nothing.
///
/// The stdio CLI has a sink that prints; a second caller — a test, or an
/// embedding into another host — may have nowhere to print, and a startup
/// sequence that requires a sink in order to be testable is not.
pub struct NoopProgressSink;
impl crate::embedding::model_artifacts::ModelProgressSink for NoopProgressSink {
    fn emit(&self, _event: &crate::embedding::model_artifacts::ModelProgressEvent) {}
}
pub async fn build_memory_service_from_env(
    mode: EmbeddingActivationMode,
    ner_progress: std::sync::Arc<dyn crate::embedding::model_artifacts::ModelProgressSink>,
) -> Result<MemoryService, MemoryError> {
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
    let versions_event = build_startup_versions_event(client_version, server_version.as_deref());
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
    // Capture the Classic GLiNER refresh config only when the configured
    // backend is Classic GLiNER. The runtime starts after MCP readiness
    // (see `run_stdio_server`); it is intentionally not spawned here.
    let (ner_artifact_refresh_config, ner_artifact_refresh_native) =
        if let crate::config::NerExtractorConfig::ClassicGliner(native) = &config.ner.extractor {
            let default_root = std::path::PathBuf::from(&effective_data_dir)
                .join("models")
                .join("ner");
            let store_root = native.model.cache_dir.clone().unwrap_or(default_root);
            let progress_for_refresh: std::sync::Arc<
                dyn crate::embedding::model_artifacts::ModelProgressSink,
            > = std::sync::Arc::new(crate::embedding::model_artifacts::JsonLineProgressSink::new());
            (
                Some(
                    crate::service::model_artifact_refresh::NerArtifactRefreshConfig {
                        store_root,
                        progress: progress_for_refresh,
                    },
                ),
                Some(native.clone()),
            )
        } else {
            (None, None)
        };
    let runtime_provider = embedding_provider.clone();
    let cache_limits = crate::config::CacheLimits::from_env(
        crate::config::memory::DEFAULT_LOCAL_CONTEXT_CACHE_BYTES,
    )?;
    let mut service = MemoryService::new_with_embedding_provider_and_cache_limits(
        db_client.clone(),
        config.active_namespace().as_str().to_string(),
        config.log_level,
        50,
        100,
        embedding_provider,
        config.embedding.similarity_threshold,
        entity_extractor,
        cache_limits,
    )?
    .with_query_logging_enabled(config.query_logging_enabled)
    .with_query_log_retention_days(config.query_log_retention_days);
    service.lifecycle_config = config.lifecycle.clone();
    service.ner_artifact_refresh_config = ner_artifact_refresh_config;
    service.ner_artifact_refresh_native = ner_artifact_refresh_native;
    service.replace_embedding_runtime_state(forced_embedding_runtime_state(
        runtime_provider,
        target.as_ref().map(|value| value.signature.clone()),
        target.as_ref().and_then(|value| value.model.clone()),
        target.as_ref().map(|value| value.dimension),
    ));
    // Wire environment-driven claim configuration. `?`, not `if let Ok`: an
    // invalid `MEMORY_CLAIM_*` value is a startup error, which is what README
    // promises — swallowing it left a typo silently running the default stage.
    let claim_config = crate::config::claims::ClaimConfig::from_env()?;
    service.claim_service = service.claim_service.clone().with_config(claim_config);
    if let (EmbeddingStartupDecision::BootstrapReadyNamespace { active_signature }, Some(target)) =
        (&decision, target.as_ref())
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
    crate::knowledge::claims_policy::backfill::schedule_namespace_backfill(
        &service.claim_service,
        &service.active_namespace,
    )
    .await?;
    // Spawn lifecycle workers if enabled
    let lifecycle_background_workers =
        crate::platform::lifecycle_runtime::spawn_workers_from_config(&service, &config.lifecycle);
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

/// Force a deployment's embedding identity onto a service, ignoring whatever
/// per-namespace decision was reached.
///
/// This is the step every *rewrite* shares, and it is deliberately one function
/// rather than two bodies. `prepare_reembed_pass` refuses a service carrying a
/// disabled provider or a `None` signature — correct for serving, fatal for
/// rewriting — so the reembed path has to override the decision before the pass
/// starts. The stdio CLI does that under
/// [`EmbeddingActivationMode::ForceEnabledForReembed`]; the HTTP durable-task
/// executor does it for the tenant whose runtime provider the activation path
/// downgraded, which is exactly the tenant that needs a reembed most. A third
/// copy of this line would eventually diverge from the other two, and a
/// divergence here is a namespace left half-rewritten.
pub(crate) fn forced_embedding_runtime_state(
    provider: Arc<dyn EmbeddingProvider>,
    signature: Option<String>,
    model: Option<String>,
    dimension: Option<usize>,
) -> EmbeddingRuntimeState {
    EmbeddingRuntimeState::new(provider, signature, model, dimension)
}
