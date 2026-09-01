//! App session cleanup scheduler.
//!
//! The job is a process-level bounded pass, not a
//! per-tenant loop. It walks up to 100 ready tenants
//! per cycle, binds each namespace through the
//! privileged maintenance factory, and issues a
//! parameterized DELETE on the `app_session` table for
//! rows whose `idle_expiry` or `absolute_expiry` has
//! passed. The DELETE is the only physical delete the
//! job is allowed to issue; it never touches facts or
//! registry history.

use std::sync::Arc;

use crate::error::MemoryError;
use crate::http::leases::scheduler::SchedulerJob;
use crate::http::registry::RegistryHandle;
#[allow(unused_imports)]
use crate::http::registry::RegistryStore;
use crate::storage::client::DbClient;

/// The cleanup job. Registers itself with
/// `SchedulerHooks::with_additional_job`.
pub fn scheduler_job() -> SchedulerJob {
    Arc::new(|registry| Box::pin(async move { cleanup_expired_for_all(&registry).await }))
}

/// Walk at most 100 ready tenants per cycle, binding
/// each namespace through the privileged maintenance
/// factory, and issue a parameterized DELETE on
/// `app_session` for expired rows. The job is
/// short-lived; it does not hold a per-tenant runtime
/// pin while iterating.
pub async fn cleanup_expired_for_all(registry: &RegistryHandle) -> Result<(), MemoryError> {
    let store = registry.store_clone();
    let due = store.list_ready_tenants(None, 100).await?;
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
        if let Err(error) = db
            .query(
                "DELETE FROM app_session WHERE idle_expiry <= time::now() OR absolute_expiry <= time::now()",
                None,
                &tenant.namespace_binding.namespace,
            )
            .await
        {
            if error.to_string().contains("app_session")
                && error.to_string().contains("does not exist")
            {
                continue;
            }
            return Err(MemoryError::Storage(format!(
                "expired app-session cleanup failed: {error}"
            )));
        }
    }
    Ok(())
}
