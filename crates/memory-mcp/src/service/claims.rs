//! Claim extraction, normalization, and reconciliation.

pub(crate) mod backfill;
pub(crate) mod extract;
pub(crate) mod normalize;
pub(crate) mod project;
pub(crate) mod reconcile;
pub(crate) mod schema;
pub(crate) mod structural;
#[cfg(any(test, feature = "prometheus"))]
pub mod telemetry;
#[cfg(not(any(test, feature = "prometheus")))]
pub(crate) mod telemetry;
pub(crate) mod worker;
