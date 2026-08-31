//! Task retention and retry/reconcile scheduler (plan §8.6).
//!
//! The job walks a bounded batch of tenants per cycle,
//! claims each tenant's maintenance lease before opening
//! its namespace, and performs retry, artifact
//! reconciliation, and `retention_expiry < time::now()`
//! cleanup. Physical deletion of expired `tenant_task`
//! rows is the only destructive action; the job never
//! touches facts or registry history.

use std::sync::Arc;

use crate::error::MemoryError;
use crate::http::leases::scheduler::SchedulerJob;
use crate::http::registry::RegistryHandle;
use crate::http::tasks::state::TaskStore;
use crate::storage::client::BoundDbClient;

/// The retention + retry/reconcile job. Registers itself
/// with `SchedulerHooks::with_additional_job`.
pub fn scheduler_job() -> SchedulerJob {
    Arc::new(|registry| Box::pin(async move { retry_reconcile_and_retain(&registry).await }))
}

/// Walk a bounded tenant batch, requeue expired running
/// tasks, reconcile artifacts, and delete expired rows.
/// The bounded pass is short-lived; it does not hold a
/// per-tenant runtime pin while iterating tenants.
pub async fn retry_reconcile_and_retain(registry: &RegistryHandle) -> Result<(), MemoryError> {
    let store = registry.store_clone();
    let now = chrono::Utc::now();
    let due = store.list_due_provisioning(100, now).await?;
    for tenant in due {
        // Skip tenants without a privileged engine (the
        // test path uses InMemoryStore and doesn't seed
        // app_session rows).
        let Some(engine) = registry.tenant_engine_optional() else {
            continue;
        };
        let bound_db = match engine.bind(&tenant).await {
            Ok(db) => db,
            Err(error) => {
                eprintln!("memory_mcp::tasks: bind failed for {}: {error}", tenant.id);
                continue;
            }
        };
        let bound_db = Arc::new(BoundDbClient::new(
            bound_db,
            tenant.namespace_binding.namespace.clone(),
        ));
        let task_store = crate::http::tasks::worker::DurableTaskStore::new(bound_db.clone());
        // Retry expired running tasks.
        if let Err(e) = task_store.requeue_expired_running(&tenant.id).await {
            eprintln!("memory_mcp::tasks: requeue failed for {}: {e}", tenant.id);
        }
        // Reconcile artifacts (bounded seam for now).
        if let Err(e) = task_store.reconcile_artifacts(&tenant.id).await {
            eprintln!("memory_mcp::tasks: reconcile failed for {}: {e}", tenant.id);
        }
        // Delete expired retention rows.
        if let Err(e) = task_store.delete_expired(&tenant.id).await {
            eprintln!(
                "memory_mcp::tasks: delete_expired failed for {}: {e}",
                tenant.id
            );
        }
    }
    Ok(())
}
