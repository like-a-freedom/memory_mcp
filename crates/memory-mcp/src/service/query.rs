//! Query preprocessing and utility functions.
//!
//! The functions themselves live in the pure kernel under `shared/`; this
//! module re-exports the ones the service-shaped call sites name through
//! `crate::service::`. Anything with no such caller is not re-exported here —
//! `crates/memory-mcp/tests/public_surface_audit.rs` is the ratchet that keeps
//! the set from regrowing.
pub use crate::shared::search::normalize_text;
pub use crate::shared::temporal::{normalize_dt, now};
