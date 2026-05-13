//! Background lifecycle jobs for memory hygiene.
//!
//! - Confidence decay refresh: marks stale facts as invalid
//! - Episode archival: archives old episodes without active facts
//! - Community recomputation: rebuilds community components from active edges
//!
//! Community recomputation pages active-edge scans in 10K batches per namespace,
//! which avoids truncating larger graphs while still bounding per-query memory.

mod archival;
mod communities;
mod decay;

pub use archival::{run_archival_pass, spawn_archival_worker};
pub use communities::{run_community_rebuild_pass, spawn_community_worker};
pub use decay::{run_decay_pass, spawn_decay_worker};
