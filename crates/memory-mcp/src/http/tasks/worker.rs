//! Fenced Task worker (spec §10.2).
//!
//! The worker claims due tasks with a monotonic lease
//! generation, commits state transitions with a
//! `lease_generation = current` CAS, and observes
//! cancellation as intent (never deletes, never rolls
//! back committed facts). The `DurableTaskStore`
//! implementation lands in Task 8.3.

use crate::error::MemoryError;

/// Claim the next due task for a tenant, or `None` when
/// no task is due. The signature is the Task 8.3 seam;
/// the implementation over the `tenant_task` table
/// arrives with `DurableTaskStore`.
pub async fn claim_next_due(
    _store: &dyn super::state::TaskStore,
    _tenant_id: &str,
    _replica_id: &str,
) -> Result<Option<super::state::TaskHandle>, MemoryError> {
    Err(MemoryError::Unavailable(
        "claim_next_due is wired in Task 8.3".into(),
    ))
}
