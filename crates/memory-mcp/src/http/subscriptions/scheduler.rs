//! Durable outbox maintenance scheduler.
//!
//! The modern transport has no server-side transport session registry. Each
//! listener polls the tenant-local durable event log directly, so cross-replica
//! visibility comes from the shared SurrealDB rather than an in-process broker.
//! This scheduled pass repairs the sequence-row invariant and probes the event
//! log for ready tenants; lost wakeups are therefore repaired by the next
//! listener poll, without fabricating a second delivery channel.

use std::sync::Arc;

use crate::error::MemoryError;
use crate::http::leases::scheduler::SchedulerJob;
use crate::http::registry::RegistryHandle;
use crate::storage::client::DbClient;

pub fn scheduler_job() -> SchedulerJob {
    Arc::new(|registry: RegistryHandle| {
        Box::pin(async move { poll_and_repair_all(&registry).await })
    })
}

/// Ensure each ready tenant has a stable sequence row and perform a bounded
/// outbox probe. The event log itself is authoritative; no volatile subscriber
/// state is written here.
pub async fn poll_and_repair_all(registry: &RegistryHandle) -> Result<(), MemoryError> {
    let tenants = registry.store_clone().list_ready_tenants(None, 100).await?;
    let Some(engine) = registry.tenant_engine_optional() else {
        return Ok(());
    };
    for tenant in tenants {
        let db = engine.bind(&tenant).await?;
        let sequence = match db
            .query(
                "SELECT VALUE value FROM tenant_change_sequence:default LIMIT 1",
                None,
                &tenant.namespace_binding.namespace,
            )
            .await
        {
            Ok(sequence) => sequence,
            Err(error)
                if (error.to_string().contains("tenant_change_sequence")
                    || error.to_string().contains("tenant_change_event"))
                    && error.to_string().contains("does not exist") =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        let rows: Vec<serde_json::Value> = serde_json::from_value(sequence)
            .map_err(|error| MemoryError::Storage(format!("outbox sequence probe: {error}")))?;
        if rows.is_empty() {
            db.query(
                "CREATE tenant_change_sequence SET value = 0",
                None,
                &tenant.namespace_binding.namespace,
            )
            .await?;
        }
        // A bounded read makes the repair pass exercise the same durable
        // index the listener uses and surfaces a corrupt event log early.
        match db
            .query(
                "SELECT event_seq FROM tenant_change_event ORDER BY event_seq DESC LIMIT 1",
                None,
                &tenant.namespace_binding.namespace,
            )
            .await
        {
            Ok(_) => {}
            Err(error)
                if error.to_string().contains("tenant_change_event")
                    && error.to_string().contains("does not exist") => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
