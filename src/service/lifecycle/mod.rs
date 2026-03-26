//! Background lifecycle jobs for memory hygiene.
//!
//! - Confidence decay refresh: marks stale facts as invalid
//! - Episode archival: archives old episodes without active facts
//!
//! ## Known limitations
//!
//! Community records are created by `update_communities` during episode
//! extraction when 2+ entities are connected. There is no periodic community
//! recomputation pass — communities are only updated incrementally as new
//! episodes are ingested. Stale communities from removed edges are not
//! automatically cleaned up outside of episode extraction.

mod archival;
mod decay;

pub use archival::{run_archival_pass, spawn_archival_worker};
pub use decay::{run_decay_pass, spawn_decay_worker};
