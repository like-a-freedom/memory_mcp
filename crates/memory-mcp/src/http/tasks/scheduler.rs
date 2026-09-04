//! Durable extraction task worker and retention scheduler.
//!
//! The process-level job walks ready tenants, performs bounded maintenance,
//! claims at most one extraction task per tenant per tick. A claimed task
//! executes through the same `MemoryService` tool path as a request, then
//! commits its terminal outcome through the fenced `TaskStore` API.

use std::sync::Arc;

use crate::error::MemoryError;
use crate::http::fault_injection::{FaultInjector, FaultPoint};
use crate::http::leases::scheduler::SchedulerJob;
use crate::http::registry::RegistryHandle;

use crate::http::tasks::state::TaskStore;
use crate::http::tasks::worker::DurableTaskStore;
use crate::storage::client::{BoundDbClient, SurrealDbClient};

/// Test-only seam (ADR-0053, Task 6). The HTTP crash-recovery tests use
/// this to drive the same `execute_one_task` body with a stub extractor
/// instead of [`crate::tools::extract`], so the fault-point coverage
/// does not depend on a local GLiNER checkpoint. Production callers must
/// always use [`execute_one_task`] through the scheduler.
#[cfg(any(test, feature = "test-fixtures"))]
pub type ExtractorFuture = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<serde_json::Value, MemoryError>> + Send>,
>;

/// Test-only extractor seam. The closure receives the durable task's
/// stored `ExtractParams` and returns the JSON value the worker would
/// otherwise receive from a real `extract` call. See [`execute_one_task_for_test`].
#[cfg(any(test, feature = "test-fixtures"))]
pub type ExtractorFn =
    Arc<dyn Fn(crate::tools::params::ExtractParams) -> ExtractorFuture + Send + Sync>;

/// The retention/retry/execution job. Registers itself with the process-level
/// scheduler; it never creates an untracked per-tenant loop.
///
/// This entry point is retained for compatibility but
/// accepts no options. Use `scheduler_job_with_options`
/// when a non-default task retention, queue capacity, or
/// fault injector override is needed.
#[deprecated(note = "use scheduler_job_with_options")]
pub fn scheduler_job() -> SchedulerJob {
    scheduler_job_with_options(crate::http::runtime::storage::RuntimeOptions::default())
}

pub fn scheduler_job_with_options(
    options: crate::http::runtime::storage::RuntimeOptions,
) -> SchedulerJob {
    let options_for_job = options.clone();
    Arc::new(move |registry| {
        let options = options_for_job.clone();
        let injector = options.fault_injector.clone();
        Box::pin(async move {
            retry_reconcile_and_retain_with_options(&registry, options, injector).await
        })
    })
}

/// Walk a bounded ready-tenant batch, recover expired tasks, execute one due
/// extraction per tenant, reconcile durable artifacts, and delete only terminal
/// rows past retention.
pub async fn retry_reconcile_and_retain(registry: &RegistryHandle) -> Result<(), MemoryError> {
    retry_reconcile_and_retain_with_options(
        registry,
        crate::http::runtime::storage::RuntimeOptions::default(),
        Arc::new(crate::http::fault_injection::NoFaults),
    )
    .await
}

async fn retry_reconcile_and_retain_with_options(
    registry: &RegistryHandle,
    options: crate::http::runtime::storage::RuntimeOptions,
    fault_injector: Arc<dyn FaultInjector>,
) -> Result<(), MemoryError> {
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
        let task_store = DurableTaskStore::new_with_options(
            bound_db,
            tenant.id.clone(),
            options.task_retention_secs,
            options.task_queue_capacity,
        );
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
        match execute_one_task(
            &task_store,
            db,
            &tenant.namespace_binding.namespace,
            &fault_injector,
        )
        .await
        {
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
    db: Arc<SurrealDbClient>,
    namespace: &str,
    fault_injector: &Arc<dyn FaultInjector>,
) -> Result<(), MemoryError> {
    let replica_id = crate::http::leases::scheduler::replica_id();
    let Some(handle) = task_store.claim_next_due(&replica_id).await? else {
        return Ok(());
    };
    // Hit after the claim is durable. The next worker sees a
    // `Running` row with an expired lease and reclaims it.
    fault_injector.hit(FaultPoint::TaskClaimed)?;
    let record = task_store.load(&handle.task_id).await?.ok_or_else(|| {
        MemoryError::NotFound(format!("task {} disappeared after claim", handle.task_id))
    })?;
    if record.cancellation_intent {
        return task_store.cancel_before_commit_fenced(&handle).await;
    }
    let params: crate::tools::params::ExtractParams = serde_json::from_value(record.params.clone())
        .map_err(|error| {
            MemoryError::Validation(format!("invalid durable extract parameters: {error}"))
        })?;
    let service =
        crate::service::MemoryService::new(db, namespace.to_owned(), "info".into(), 100, 100)?
            .with_http_outbox();
    let extraction = crate::tools::extract(&service.build_context(), params).await;
    match extraction {
        Ok(result) => {
            let value = serde_json::to_value(result).map_err(|error| {
                MemoryError::Storage(format!("serialize extract result: {error}"))
            })?;
            // The artifact is the durable commit boundary. Once it exists, a
            // cancellation request is reported as completed_before_cancel.
            task_store.record_artifact_fenced(&handle, &value).await?;
            // Hit after the artifact row is committed. The next
            // worker sees the artifact via `reconcile_artifacts`
            // and projects the completed terminal state.
            fault_injector.hit(FaultPoint::TaskArtifactCommitted)?;
            let cancelled_after_commit = task_store
                .load(&handle.task_id)
                .await?
                .is_some_and(|task| task.cancellation_intent);
            task_store
                .complete_fenced(&handle, value, cancelled_after_commit)
                .await?;
            // Hit after the terminal state is committed.
            fault_injector.hit(FaultPoint::TaskCompleted)?;
            Ok(())
        }
        Err(error) => {
            task_store
                .fail_fenced(&handle, serde_json::json!({"message": error.to_string()}))
                .await
        }
    }
}

/// Test-only mirror of [`execute_one_task`]. Drives the same durable
/// state machine but replaces the real `extract` call with the
/// provided [`ExtractorFn`], so the recovery tests can exercise the
/// `TaskClaimed` / `TaskArtifactCommitted` / `TaskCompleted` fault
/// points without a local GLiNER checkpoint.
///
/// The closure is invoked with the deserialized `ExtractParams` from
/// the durable task row and must return the JSON value
/// `record_artifact_fenced` would otherwise receive. The exact same
/// hit points fire in the same order as the production path.
#[cfg(any(test, feature = "test-fixtures"))]
pub async fn execute_one_task_for_test(
    task_store: &DurableTaskStore,
    db: Arc<SurrealDbClient>,
    namespace: &str,
    fault_injector: Arc<dyn FaultInjector>,
    extractor: ExtractorFn,
) -> Result<(), MemoryError> {
    let replica_id = crate::http::leases::scheduler::replica_id();
    let Some(handle) = task_store.claim_next_due(&replica_id).await? else {
        return Ok(());
    };
    // Hit after the claim is durable. The next worker sees a
    // `Running` row with an expired lease and reclaims it.
    fault_injector.hit(FaultPoint::TaskClaimed)?;
    let record = task_store.load(&handle.task_id).await?.ok_or_else(|| {
        MemoryError::NotFound(format!("task {} disappeared after claim", handle.task_id))
    })?;
    if record.cancellation_intent {
        return task_store.cancel_before_commit_fenced(&handle).await;
    }
    let params: crate::tools::params::ExtractParams = serde_json::from_value(record.params.clone())
        .map_err(|error| {
            MemoryError::Validation(format!("invalid durable extract parameters: {error}"))
        })?;
    // The stub extractor is only used in test-fixtures builds; the
    // real `extract` call is the production seam and lives in
    // `execute_one_task` above.
    let extraction = extractor(params).await;
    match extraction {
        Ok(value) => {
            task_store.record_artifact_fenced(&handle, &value).await?;
            // Hit after the artifact row is committed. The next
            // worker sees the artifact via `reconcile_artifacts`
            // and projects the completed terminal state.
            fault_injector.hit(FaultPoint::TaskArtifactCommitted)?;
            let cancelled_after_commit = task_store
                .load(&handle.task_id)
                .await?
                .is_some_and(|task| task.cancellation_intent);
            task_store
                .complete_fenced(&handle, value, cancelled_after_commit)
                .await?;
            // Hit after the terminal state is committed.
            fault_injector.hit(FaultPoint::TaskCompleted)?;
            // `db` is unused on this test seam path because the stub
            // does not run real extract; the local-binding analysis
            // would otherwise flag it as dead. Document the
            // intentional reservation for future stub extractors that
            // need the underlying client.
            let _ = (db, namespace);
            Ok(())
        }
        Err(error) => {
            task_store
                .fail_fenced(&handle, serde_json::json!({"message": error.to_string()}))
                .await
        }
    }
}
