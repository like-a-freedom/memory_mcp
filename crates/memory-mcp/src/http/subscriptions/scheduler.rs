//! Outbox polling/reconciliation scheduler job (spec §11).
//!
//! Reads a bounded sequence window for each active
//! subscription, emits only invalidation events, and
//! advances the durable cursor after delivery.

use std::sync::Arc;

use crate::error::MemoryError;
use crate::http::leases::scheduler::SchedulerJob;
use crate::http::registry::RegistryHandle;

/// The outbox polling job. Registers itself with
/// `SchedulerHooks::with_additional_job`.
pub fn scheduler_job() -> SchedulerJob {
    Arc::new(|_registry: RegistryHandle| Box::pin(async move { poll_and_repair_all().await }))
}

/// Bounded poll pass. For now this is a no-op seam;
/// the concrete polling and repair logic lands with the
/// cross-replica wake wiring (Task 9.3).
pub async fn poll_and_repair_all() -> Result<(), MemoryError> {
    Ok(())
}
