//! Provisioning seam (ADR-0052, plan §4.7).
//!
//! Task 5.1 extends this file with the state machine; the Task
//! 6.2 scheduler consumes the events. For now: a single
//! `enqueue_provisioning` durable seam that the Phase 4 control
//! API calls after writing a reserved Tenant.

use std::sync::Arc;

use crate::error::MemoryError;
use crate::http::registry::models::Tenant;
use crate::http::registry::RegistryStore;

/// Durable enqueue: append a provisioning event for the reserved
/// tenant. Idempotency is enforced by the store (duplicate
/// `(tenant_id, stage)` events are ignored by the scheduler, Task
/// 6.2).
pub async fn enqueue_provisioning(
    store: &Arc<dyn RegistryStore>,
    tenant: &Tenant,
) -> Result<(), MemoryError> {
    store.append_provisioning_event(&tenant.id, "reserved").await
}
