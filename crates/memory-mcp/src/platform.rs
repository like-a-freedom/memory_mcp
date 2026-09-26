//! Technical mechanisms shared across the modules.
//!
//! The spec's rule: platform holds technical dependencies and the
//! pure kernel, and carries no business orchestration. So a database
//! client, its retry mechanics and its error classification belong
//! here; a fact, a session or a deletion policy does not.
//!
//! Only infra and bootstrap may use this module. A bounded context
//! reaches storage through its own owner-scoped store, never through
//! platform directly — otherwise the database would be one shared
//! tool with no stated owner, which is the thing this separation
//! exists to prevent.

pub mod persistence;
