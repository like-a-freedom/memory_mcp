//! Claim extraction, normalization, and reconciliation.
//!
//! Knowledge owns the claim domain: contradiction, correction,
//! supersession and retraction. The store is in `knowledge/claims.rs`
//! and these use cases sit with it.

pub(crate) mod backfill;
pub(crate) mod extract;
pub(crate) mod projection;
pub(crate) mod reconcile;
pub(crate) mod schema;
pub(crate) mod structural;
#[cfg(any(test, feature = "prometheus"))]
pub mod telemetry;
#[cfg(not(any(test, feature = "prometheus")))]
pub(crate) mod telemetry;
pub(crate) mod worker;
