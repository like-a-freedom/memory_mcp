//! Outbox polling/reconciliation scheduler job.
//!
//! The primary cross-replica wake mechanism is SurrealDB's
//! LIVE SELECT on `tenant_change_event`. This scheduler
//! job is the fallback: when a LIVE SELECT wake is lost
//! (replica restart, network partition), the polling job
//! re-reads the durable outbox and re-delivers missed
//! events to active subscribers.
//!
//! In the 2026-07-28 stateless transport, subscriptions are
//! per-request (no persistent subscription state). The
//! outbox polling ensures that events committed by one
//! replica are visible to subscribers on other replicas
//! via the shared SurrealDB. The concrete per-subscription
//! cursor tracking and re-delivery logic requires the
//! full subscription infrastructure (persistent subscriber
//! registry) which is deferred to a later phase.

use std::sync::Arc;

use crate::error::MemoryError;
use crate::http::leases::scheduler::SchedulerJob;
use crate::http::registry::RegistryHandle;

/// The outbox polling job. Registers itself with
/// `SchedulerHooks::with_additional_job`.
pub fn scheduler_job() -> SchedulerJob {
    Arc::new(|_registry: RegistryHandle| Box::pin(async move { poll_and_repair_all().await }))
}

/// Bounded poll pass. Reads the outbox sequence for each
/// active tenant and ensures event visibility across
/// replicas. Currently a no-op seam pending the full
/// subscription infrastructure.
pub async fn poll_and_repair_all() -> Result<(), MemoryError> {
    Ok(())
}
