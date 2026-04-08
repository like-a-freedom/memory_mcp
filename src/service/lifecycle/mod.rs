//! Background lifecycle jobs for memory hygiene.
//!
//! - Confidence decay refresh: marks stale facts as invalid
//! - Episode archival: archives old episodes without active facts
//! - Community recomputation: rebuilds community components from active edges
//!
//! ## Known limitations
//!
//! Community recomputation scans up to 10K active edges per namespace via
//! `select_edges_filtered`. Larger graphs will be rebuilt from a truncated view,
//! and the storage layer logs a warning when the limit is hit.

mod archival;
mod communities;
mod decay;

pub use archival::{run_archival_pass, spawn_archival_worker};
pub use communities::{run_community_rebuild_pass, spawn_community_worker};
pub use decay::{run_decay_pass, spawn_decay_worker};
