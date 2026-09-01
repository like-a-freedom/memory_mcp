//! Durable extraction task worker and retention scheduler.
//!
//! The process-level job walks ready tenants, performs bounded maintenance,
//! and claims at most one extraction task per tenant per tick. A claimed task
//! executes through the same `MemoryService` tool path as a request, then
//! commits its terminal outcome through the fenced `TaskStore` API.

use std::sync::Arc;

use crate::error::MemoryError;
use crate::http::leases::scheduler::SchedulerJob;
use crate::http::registry::RegistryHandle;

use crate::http::tasks::state::TaskStore;
use crate::http::tasks::worker::DurableTaskStore;
use crate::storage::client::BoundDbClient;

/// The retention/retry/execution job. Registers itself with the process-level
/// scheduler; it never creates an untracked per-tenant loop.
pub fn scheduler_job() -> SchedulerJob {
    Arc::new(|registry| Box::pin(async move { retry_reconcile_and_retain(&registry).await }))
}

/// Walk a bounded ready-tenant batch, recover expired tasks, execute one due
/// extraction per tenant, reconcile durable artifacts, and delete only terminal
/// rows past retention.
pub async fn retry_reconcile_and_retain(registry: &RegistryHandle) -> Result<(), MemoryError> {
    let store = registry.store_clone();
    let tenants = store.list_ready_tenants(None, 100).await?;
    let Some(engine) = registry.tenant_engine_optional() else {
        return Ok(());
    };

    for tenant in tenants {
        let db = match engine.bind(&tenant).await {
            Ok(db) => db,
            Err(error) => {
                eprintln!("memory_mcp::tasks: bind failed for {}: {error}", tenant.id);
                continue;
            }
        };
        let bound_db = Arc::new(BoundDbClient::new(
            db.clone(),
            tenant.namespace_binding.namespace.clone(),
        ));
        let task_store = DurableTaskStore::new(bound_db, tenant.id.clone());
        if let Err(error) = task_store.requeue_expired_running().await {
            if error.to_string().contains("tenant_task")
                && error.to_string().contains("does not exist")
            {
                continue;
            }
            eprintln!(
                "memory_mcp::tasks: requeue failed for {}: {error}",
                tenant.id
            );
        }
        if let Err(error) = task_store.reconcile_artifacts().await {
            eprintln!(
                "memory_mcp::tasks: reconcile failed for {}: {error}",
                tenant.id
            );
        }
        match execute_one_task(&task_store, db, &tenant.namespace_binding.namespace).await {
            Ok(()) => {}
            Err(error)
                if error.to_string().contains("tenant_task")
                    && error.to_string().contains("does not exist") => {}
            Err(error) => {
                eprintln!(
                    "memory_mcp::tasks: execution failed for {}: {error}",
                    tenant.id
                );
            }
        }
        if let Err(error) = task_store.delete_expired().await {
            eprintln!(
                "memory_mcp::tasks: delete_expired failed for {}: {error}",
                tenant.id
            );
        }
    }
    Ok(())
}

async fn execute_one_task(
    task_store: &DurableTaskStore,
    db: Arc<crate::storage::client::SurrealDbClient>,
    namespace: &str,
) -> Result<(), MemoryError> {
    let Some(handle) = task_store.claim_next_due("scheduler").await? else {
        return Ok(());
    };
    let record = task_store.load(&handle.task_id).await?.ok_or_else(|| {
        MemoryError::NotFound(format!("task {} disappeared after claim", handle.task_id))
    })?;
    let params: crate::tools::params::ExtractParams = serde_json::from_value(record.params.clone())
        .map_err(|error| {
            MemoryError::Validation(format!("invalid durable extract parameters: {error}"))
        })?;
    let service =
        crate::service::MemoryService::new(db, namespace.to_owned(), "info".into(), 100, 100)?
            .with_http_outbox();
    let extraction = crate::tools::extract(&service.build_context(), params).await;
    let current = task_store.load(&handle.task_id).await?;
    let cancelled_after_commit = current
        .as_ref()
        .is_some_and(|task| task.cancellation_intent);
    match extraction {
        Ok(result) => {
            let value = serde_json::to_value(result).map_err(|error| {
                MemoryError::Storage(format!("serialize extract result: {error}"))
            })?;
            task_store.record_artifact_fenced(&handle, &value).await?;
            task_store
                .complete_fenced(&handle, value, cancelled_after_commit)
                .await
        }
        Err(error) => {
            task_store
                .fail_fenced(&handle, serde_json::json!({"message": error.to_string()}))
                .await
        }
    }
}
