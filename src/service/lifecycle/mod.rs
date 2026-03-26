//! Background lifecycle jobs for memory hygiene.
//!
//! - Confidence decay refresh: marks stale facts as invalid
//! - Episode archival: archives old episodes without active facts

mod decay;
mod archival;

pub use decay::{run_decay_pass, spawn_decay_worker};
pub use archival::{run_archival_pass, spawn_archival_worker};
