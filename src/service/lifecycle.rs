//! Background lifecycle jobs for memory hygiene.
//!
//! - Confidence decay refresh: marks stale facts as invalid
//! - Episode archival: archives old episodes without active facts
//! - Community recomputation: rebuilds community components from active edges
//!
//! Community recomputation pages active-edge scans in 10K batches per namespace,
//! which avoids truncating larger graphs while still bounding per-query memory.

use crate::config::LifecycleConfig;
use crate::service::{LifecyclePolicy, MemoryService};

mod archival;
mod communities;
mod decay;

pub use archival::{run_archival_pass, spawn_archival_worker};
pub use communities::{run_community_rebuild_pass, spawn_community_worker};
pub use decay::{run_decay_pass, spawn_decay_worker};

/// Spawns all lifecycle workers based on configuration.
pub fn spawn_workers_from_config(service: &MemoryService, config: &LifecycleConfig) {
    if !config.enabled {
        return;
    }
    let policy = LifecyclePolicy::from(config);

    let decay_service = service.clone();
    let decay_config = config.clone();
    let _decay_handle = spawn_decay_worker(
        decay_service,
        decay_config.decay_interval_secs,
        policy.decay_confidence_threshold,
        policy.decay_half_life_days,
    );

    let archival_service = service.clone();
    let archival_config = config.clone();
    let _archival_handle = spawn_archival_worker(
        archival_service,
        archival_config.archival_interval_secs,
        policy.archival_age_days,
    );

    let community_service = service.clone();
    let community_config = config.clone();
    let _community_handle =
        spawn_community_worker(community_service, community_config.archival_interval_secs);

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
}
