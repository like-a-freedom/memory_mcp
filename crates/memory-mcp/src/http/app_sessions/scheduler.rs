//! App session cleanup scheduler.
//!
//! The job is a process-level bounded pass, not a
//! per-tenant loop. It walks up to 100 ready tenants
//! per cycle, binds each namespace through the
//! privileged maintenance factory, and asks
//! [`AppSessionStore`] to delete rows whose `idle_expiry`
//! or `absolute_expiry` has passed. The DELETE lives in the
//! store that owns the table, so the account-deletion
//! recovery path runs the identical statement and the two
//! cannot drift. The job never touches facts or registry
//! history.

use std::sync::Arc;

use crate::error::MemoryError;
use crate::http::app_sessions::store::AppSessionStore;
use crate::http::leases::scheduler::SchedulerJob;
use crate::http::registry::RegistryHandle;
use crate::storage::client::BoundDbClient;

/// The cleanup job. Registers itself with
/// `SchedulerHooks::with_additional_job`.
pub fn scheduler_job() -> SchedulerJob {
    Arc::new(|registry| Box::pin(async move { cleanup_expired_for_all(&registry).await }))
}

/// Walk at most 100 ready tenants per cycle, binding
/// each namespace through the privileged maintenance
/// factory, and delete expired `app_session` rows.
/// The job is short-lived; it does not hold a per-tenant
/// runtime pin while iterating.
pub async fn cleanup_expired_for_all(registry: &RegistryHandle) -> Result<(), MemoryError> {
    // Tenant listing is tenancy's; the sweeper never needed the rest of the registry.
    let due = registry.tenants().list_ready_tenants(None, 100).await?;
    for tenant in due {
        // Production path: bind the tenant namespace
        // through the privileged engine and issue the
        // DELETE. The InMemoryStore path used by the
        // conformance test does not have a per-tenant
        // engine; we skip the delete for tenants whose
        // engine is None (the test path does not seed
        // app_session rows).
        let Some(engine) = registry.tenant_engine_optional() else {
            continue;
        };
        let db = match engine.bind(&tenant).await {
            Ok(db) => db,
            Err(error) => {
                eprintln!(
                    "memory_mcp::app_sessions: bind failed for {}: {error}",
                    tenant.id
                );
                continue;
            }
        };
        // `BoundDbClient` pins the namespace as an invariant rather than a
        // per-call argument, so this store physically cannot reach another
        // tenant's rows. `delete_expired` owns the statement and the
        // missing-table handling.
        let sessions = AppSessionStore::new(Arc::new(BoundDbClient::new(
            db,
            tenant.namespace_binding.namespace.clone(),
        )));
        sessions.delete_expired().await.map_err(|error| {
            MemoryError::Storage(format!("expired app-session cleanup failed: {error}"))
        })?;
    }
    Ok(())
}
