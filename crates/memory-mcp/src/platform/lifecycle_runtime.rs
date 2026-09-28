//! Container-facing entry points for the memory-owned lifecycle jobs.
//!
//! The jobs themselves live in `memory::lifecycle_workers` and take a
//! narrow port ([`LifecycleHandles`]) rather than the container. This
//! module is the one place the container is converted for them, which is
//! why the conversion lives on the adapter side of the dependency
//! direction rather than inside the context.

use std::sync::Arc;

use crate::config::LifecycleConfig;
use crate::error::MemoryError;
use crate::memory::lifecycle_workers::{self, LifecycleHandles, LifecyclePolicy};
use crate::service::MemoryService;

pub use crate::memory::lifecycle_workers::LifecycleBackgroundWorkerRuntime;

/// Builds the narrow port the lifecycle jobs take from the container.
pub fn handles_from<'a>(service: &'a MemoryService) -> LifecycleHandles<'a> {
    LifecycleHandles {
        db_client: service.db_client.clone(),
        active_namespace: &service.active_namespace,
        logger: &service.logger,
        policy: service.lifecycle_policy(),
        claim_service: &service.claim_service,
    }
}

/// Runs one archival pass over a service.
pub async fn archival_pass(service: &MemoryService, age_days: u32) -> Result<usize, MemoryError> {
    lifecycle_workers::archival::run_archival_pass(&handles_from(service), age_days).await
}

/// Runs one decay pass over a service.
pub async fn decay_pass(
    service: &MemoryService,
    threshold: f64,
    half_life_days: f64,
) -> Result<usize, MemoryError> {
    lifecycle_workers::decay::run_decay_pass(&handles_from(service), threshold, half_life_days)
        .await
}

/// Rebuilds the community table from all currently active edges.
pub async fn run_community_rebuild_pass(service: &MemoryService) -> Result<usize, MemoryError> {
    lifecycle_workers::run_community_rebuild_pass(&handles_from(service)).await
}

/// Spawns all three lifecycle workers against `service` with the given
/// intervals. The configuration-driven path is `spawn_workers_from_config`;
/// this one takes the intervals directly for tests that assert spawn and
/// shutdown behaviour without a config file.
pub fn spawn_all_for_test(
    service: &MemoryService,
    interval_secs: u64,
    threshold: f64,
    half_life_days: f64,
    age_days: u32,
) -> LifecycleBackgroundWorkerRuntime {
    let runtime = LifecycleBackgroundWorkerRuntime::new();
    let policy = service.lifecycle_policy();
    let db_client: Arc<dyn crate::storage::DbClient> = service.db_client.clone();
    let namespace = service.active_namespace.clone();
    let logger = service.logger.clone();
    let claim_service = service.claim_service.clone();
    runtime.spawn_decay(
        db_client.clone(),
        namespace.clone(),
        logger.clone(),
        policy,
        claim_service.clone(),
        interval_secs,
        threshold,
        half_life_days,
    );
    runtime.spawn_archival(
        db_client.clone(),
        namespace.clone(),
        logger.clone(),
        policy,
        claim_service.clone(),
        interval_secs,
        age_days,
    );
    runtime.spawn_community(
        db_client,
        namespace,
        logger,
        policy,
        claim_service,
        interval_secs,
    );
    runtime
}

/// Spawns all lifecycle workers based on configuration, returning a runtime
/// that owns the worker handles for clean shutdown.
///
/// When `config.enabled` is false, returns an empty runtime (no workers).
pub fn spawn_workers_from_config(
    service: &MemoryService,
    config: &LifecycleConfig,
) -> LifecycleBackgroundWorkerRuntime {
    let runtime = LifecycleBackgroundWorkerRuntime::new();
    if !config.enabled {
        return runtime;
    }
    let policy = LifecyclePolicy::from(config);
    let db_client: Arc<dyn crate::storage::DbClient> = service.db_client.clone();
    let namespace = service.active_namespace.clone();
    let logger = service.logger.clone();
    let claim_service = service.claim_service.clone();

    let port = || {
        (
            db_client.clone(),
            namespace.clone(),
            logger.clone(),
            policy,
            claim_service.clone(),
        )
    };

    let (db, ns, log, pol, claims) = port();
    runtime.spawn_decay(
        db,
        ns,
        log,
        pol,
        claims,
        config.decay_interval_secs,
        policy.decay_confidence_threshold,
        policy.decay_half_life_days,
    );

    let (db, ns, log, pol, claims) = port();
    runtime.spawn_archival(
        db,
        ns,
        log,
        pol,
        claims,
        config.archival_interval_secs,
        policy.archival_age_days,
    );

    let (db, ns, log, pol, claims) = port();
    runtime.spawn_community(db, ns, log, pol, claims, config.archival_interval_secs);

    let mut event = std::collections::HashMap::new();
    event.insert(
        "op".to_string(),
        serde_json::json!("lifecycle.workers.started"),
    );
    for (key, value) in [
        ("decay_interval_secs", config.decay_interval_secs),
        ("archival_interval_secs", config.archival_interval_secs),
    ] {
        event.insert(
            key.to_string(),
            serde_json::Value::Number(serde_json::Number::from(value)),
        );
    }
    service.logger.log(event, crate::logging::LogLevel::Info);
    runtime
}
