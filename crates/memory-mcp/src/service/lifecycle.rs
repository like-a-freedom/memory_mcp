//! Background lifecycle jobs for memory hygiene.
//!
//! - Confidence decay refresh: marks stale facts as invalid
//! - Episode archival: archives old episodes without active facts
//! - Community recomputation: rebuilds community components from active edges
//!
//! Community recomputation pages active-edge scans in 10K batches per namespace,
//! which avoids truncating larger graphs while still bounding per-query memory.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::config::LifecycleConfig;
use crate::service::MemoryService;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LifecyclePolicy {
    pub(crate) archival_age_days: u32,
    pub(crate) decay_confidence_threshold: f64,
    pub(crate) decay_half_life_days: f64,
}

impl Default for LifecyclePolicy {
    fn default() -> Self {
        Self {
            archival_age_days: 90,
            decay_confidence_threshold: 0.3,
            decay_half_life_days: 365.0,
        }
    }
}

impl From<&LifecycleConfig> for LifecyclePolicy {
    fn from(config: &LifecycleConfig) -> Self {
        Self {
            archival_age_days: config.archival_age_days,
            decay_confidence_threshold: config.decay_confidence_threshold,
            decay_half_life_days: config.decay_half_life_days,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_policy_matches_config_defaults() {
        let policy = LifecyclePolicy::default();
        assert_eq!(policy.archival_age_days, 90);
        assert_eq!(policy.decay_confidence_threshold, 0.3);
        assert_eq!(policy.decay_half_life_days, 365.0);
    }
}

mod archival;
mod communities;
mod decay;

pub use archival::{run_archival_pass, spawn_archival_worker};
pub use communities::{run_community_rebuild_pass, spawn_community_worker};
pub use decay::{run_decay_pass, spawn_decay_worker};

/// Bounded runtime for the lifecycle background workers (decay, archival,
/// community).
///
/// Mirrors `ClaimWorkerRuntime`: each worker task observes a shared
/// [`CancellationToken`] and its `JoinHandle` is tracked here so that
/// [`shutdown`](Self::shutdown) can cancel the token and join all workers.
///
/// `Clone` so it can live on `MemoryService` (which derives `Clone`); the
/// shared `Arc<Mutex<...>>` handle list and the `CancellationToken` (internally
/// ref-counted) keep all clones consistent.
#[derive(Clone)]
pub struct LifecycleBackgroundWorkerRuntime {
    shutdown: CancellationToken,
    handles: Arc<tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl LifecycleBackgroundWorkerRuntime {
    pub fn new() -> Self {
        Self {
            shutdown: CancellationToken::new(),
            handles: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        }
    }

    /// Spawn the decay worker, tracking its handle for shutdown.
    pub fn spawn_decay(
        &self,
        service: MemoryService,
        interval_secs: u64,
        threshold: f64,
        half_life_days: f64,
    ) {
        let handle = spawn_decay_worker(
            service,
            interval_secs,
            threshold,
            half_life_days,
            self.shutdown.clone(),
        );
        // No async lock contention here: spawn_workers_from_config runs
        // synchronously, but try_lock avoids requiring an async context.
        if let Ok(mut handles) = self.handles.try_lock() {
            handles.push(handle);
        } else {
            // Fallback: leak the handle into the runtime so it is at least
            // cancellation-aware. This branch should be unreachable given the
            // synchronous call site.
            handle.abort();
        }
    }

    /// Spawn the archival worker, tracking its handle for shutdown.
    pub fn spawn_archival(&self, service: MemoryService, interval_secs: u64, age_days: u32) {
        let handle = spawn_archival_worker(service, interval_secs, age_days, self.shutdown.clone());
        if let Ok(mut handles) = self.handles.try_lock() {
            handles.push(handle);
        } else {
            handle.abort();
        }
    }

    /// Spawn the community recomputation worker, tracking its handle for shutdown.
    pub fn spawn_community(&self, service: MemoryService, interval_secs: u64) {
        let handle = spawn_community_worker(service, interval_secs, self.shutdown.clone());
        if let Ok(mut handles) = self.handles.try_lock() {
            handles.push(handle);
        } else {
            handle.abort();
        }
    }

    /// Cancel all workers and join their tasks.
    ///
    /// Safe to call even when no workers were spawned (returns immediately).
    pub async fn shutdown(&self) {
        self.shutdown.cancel();
        let handles = std::mem::take(&mut *self.handles.lock().await);
        for handle in handles {
            let _ = handle.await;
        }
    }
}

impl Default for LifecycleBackgroundWorkerRuntime {
    fn default() -> Self {
        Self::new()
    }
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

    let decay_service = service.clone();
    let decay_config = config.clone();
    runtime.spawn_decay(
        decay_service,
        decay_config.decay_interval_secs,
        policy.decay_confidence_threshold,
        policy.decay_half_life_days,
    );

    let archival_service = service.clone();
    let archival_config = config.clone();
    runtime.spawn_archival(
        archival_service,
        archival_config.archival_interval_secs,
        policy.archival_age_days,
    );

    let community_service = service.clone();
    let community_config = config.clone();
    runtime.spawn_community(community_service, community_config.archival_interval_secs);

    let mut event = std::collections::HashMap::new();
    event.insert(
        "op".to_string(),
        serde_json::json!("lifecycle.workers.started"),
    );
    event.insert(
        "decay_interval".to_string(),
        serde_json::json!(config.decay_interval_secs),
    );
    event.insert(
        "archival_interval".to_string(),
        serde_json::json!(config.archival_interval_secs),
    );
    event.insert(
        "community_interval".to_string(),
        serde_json::json!(config.archival_interval_secs),
    );
    service.logger.log(event, crate::logging::LogLevel::Info);

    runtime
}
