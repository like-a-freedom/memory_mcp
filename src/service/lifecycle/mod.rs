//! Background lifecycle jobs for memory hygiene.
//!
//! - Confidence decay refresh: marks stale facts as invalid
//! - Episode archival: archives old episodes without active facts

mod archival;
mod decay;

pub use archival::{run_archival_pass, spawn_archival_worker};
pub use decay::{run_decay_pass, spawn_decay_worker};
