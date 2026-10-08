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

/// The handles the lifecycle background passes need.
///
/// Decay, archival and community rebuild are all background hygiene
/// jobs: they read the database through the bound namespace, log what
/// they do, and otherwise talk to whichever context owns the table
/// they are working on. Its core is four things, not twenty-nine; the
/// decay pass additionally requires knowledge's atomic-retraction port.
pub struct LifecycleHandles<'a> {
    pub(crate) db_client: Arc<dyn crate::storage::DbClient>,
    pub(crate) active_namespace: &'a str,
    pub(crate) logger: &'a crate::logging::StdoutLogger,
    pub(crate) policy: LifecyclePolicy,
    /// Present only for decay; archival and community passes do not retain it.
    pub(crate) claim_store: Option<Arc<dyn crate::knowledge::claims::ClaimStore>>,
}

impl<'a> LifecycleHandles<'a> {
    /// The memory-owned episode store, for the archival pass.
    pub(crate) fn episode_store(&self) -> crate::memory::episode_store::EpisodeStoreClient {
        crate::memory::episode_store::EpisodeStoreClient::new(
            self.db_client.clone(),
            self.active_namespace.to_string(),
        )
    }

    /// The memory-owned fact access log, for decay heat.
    pub(crate) fn fact_access_store(&self) -> crate::memory::fact_access_store::FactAccessStore {
        crate::memory::fact_access_store::FactAccessStore::new(
            self.db_client.clone(),
            self.active_namespace.to_string(),
        )
    }

    /// The knowledge-owned graph store.
    ///
    /// Backs the community rebuild pass and the fact lookups the
    /// archival pass needs. The `edge`, `fact` and `community` tables
    /// it reads are knowledge's, so the reads leave through the
    /// knowledge context rather than through the episode store.
    pub(crate) fn knowledge_graph_store(
        &self,
    ) -> crate::knowledge::graph_store::KnowledgeGraphStore {
        crate::knowledge::graph_store::KnowledgeGraphStore::new(
            self.db_client.clone(),
            self.active_namespace.to_string(),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LifecyclePolicy {
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

pub mod archival;
mod communities;
pub mod decay;

pub(crate) use archival::spawn_archival_worker;
pub(crate) use communities::{run_community_rebuild_pass, spawn_community_worker};
pub(crate) use decay::spawn_decay_worker;

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
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_decay(
        &self,
        db_client: Arc<dyn crate::storage::DbClient>,
        active_namespace: String,
        logger: crate::logging::StdoutLogger,
        policy: LifecyclePolicy,
        claim_store: Arc<dyn crate::knowledge::claims::ClaimStore>,
        interval_secs: u64,
        threshold: f64,
        half_life_days: f64,
    ) {
        let handle = spawn_decay_worker(
            db_client,
            active_namespace,
            logger,
            policy,
            claim_store,
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
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_archival(
        &self,
        db_client: Arc<dyn crate::storage::DbClient>,
        active_namespace: String,
        logger: crate::logging::StdoutLogger,
        policy: LifecyclePolicy,
        interval_secs: u64,
        age_days: u32,
    ) {
        let handle = spawn_archival_worker(
            db_client,
            active_namespace,
            logger,
            policy,
            interval_secs,
            age_days,
            self.shutdown.clone(),
        );
        if let Ok(mut handles) = self.handles.try_lock() {
            handles.push(handle);
        } else {
            handle.abort();
        }
    }

    /// Spawn the community recomputation worker, tracking its handle for shutdown.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_community(
        &self,
        db_client: Arc<dyn crate::storage::DbClient>,
        active_namespace: String,
        logger: crate::logging::StdoutLogger,
        policy: LifecyclePolicy,
        interval_secs: u64,
    ) {
        let handle = spawn_community_worker(
            db_client,
            active_namespace,
            logger,
            policy,
            interval_secs,
            self.shutdown.clone(),
        );
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
