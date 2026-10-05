//! Durable extraction task worker and retention scheduler.
//!
//! The process-level job walks ready tenants, performs bounded maintenance,
//! claims at most one extraction task per tenant per tick. A claimed task
//! executes through the same `MemoryService` tool path as a request, then
//! commits its terminal outcome through the fenced `TaskStore` API.

use std::sync::Arc;

use crate::error::MemoryError;
use crate::http::leases::scheduler::SchedulerJob;
use crate::http::registry::RegistryHandle;
use crate::platform::fault_injection::{FaultInjector, FaultPoint};

use crate::http::tasks::state::{TASK_KIND_EXTRACT, TASK_KIND_REEMBED, TaskStore};
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
        Arc::new(crate::platform::fault_injection::NoFaults),
    )
    .await
}

async fn retry_reconcile_and_retain_with_options(
    registry: &RegistryHandle,
    options: crate::http::runtime::storage::RuntimeOptions,
    fault_injector: Arc<dyn FaultInjector>,
) -> Result<(), MemoryError> {
    let tenants = registry.tenants().list_ready_tenants(None, 100).await?;
    let Some(engine) = registry.tenant_engine_optional() else {
        return Ok(());
    };

    // Measured around the whole pass rather than each step inside it: a tenant
    // loop that grows slow shows up here as one rising series, and the per-step
    // refusals below already say which step refused. A timer per step would add
    // five histograms to say the same thing with less legibility.
    let started = std::time::Instant::now();
    let mut degraded = false;

    for tenant in tenants {
        let db = match engine.bind(&tenant).await {
            Ok(db) => db,
            Err(error) => {
                degraded = true;
                crate::http::logging::log_warn(
                    "http.task.bind_failed",
                    &format!("tenant {}: {error}", tenant.id),
                );
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
            degraded = true;
            crate::http::logging::log_warn(
                "http.task.requeue_failed",
                &format!("tenant {}: {error}", tenant.id),
            );
        }
        if let Err(error) = task_store.reconcile_artifacts().await {
            degraded = true;
            crate::http::logging::log_warn(
                "http.task.reconcile_failed",
                &format!("tenant {}: {error}", tenant.id),
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
                degraded = true;
                crate::http::logging::log_warn(
                    "http.task.execution_failed",
                    &format!("tenant {}: {error}", tenant.id),
                );
            }
        }
        if let Err(error) = task_store.delete_expired().await {
            degraded = true;
            crate::http::logging::log_warn(
                "http.task.delete_expired_failed",
                &format!("tenant {}: {error}", tenant.id),
            );
        }
    }
    crate::observability::record_job_metric(
        "task",
        if degraded { "degraded" } else { "ok" },
        started.elapsed().as_secs_f64(),
    );
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
    match record.kind.as_str() {
        TASK_KIND_EXTRACT => {
            execute_extract_task(task_store, db, namespace, &handle, &record, fault_injector).await
        }
        TASK_KIND_REEMBED => {
            execute_reembed_task(task_store, db, namespace, &handle, &record).await
        }
        // Fail closed. An unrecognised kind is never decoded as whichever
        // payload happens to fit: the only two kinds that exist are the two
        // above, and guessing would run a maintenance task as an extraction (or
        // an extraction as a destructive whole-namespace rewrite). Naming the
        // kind is what makes the row diagnosable instead of mysterious.
        other => {
            task_store
                .fail_fenced(
                    &handle,
                    serde_json::json!({
                        "message": format!("unknown task kind `{other}`"),
                    }),
                )
                .await
        }
    }
}

/// The extract executor. Unchanged from the pre-`kind` body of
/// [`execute_one_task`]: a row whose kind is `extract` — including every row
/// written before migration 045, which reads back as `extract` — decodes
/// [`crate::tools::params::ExtractParams`] and runs the real `extract` tool.
async fn execute_extract_task(
    task_store: &DurableTaskStore,
    db: Arc<SurrealDbClient>,
    namespace: &str,
    handle: &crate::http::tasks::state::TaskHandle,
    record: &crate::http::tasks::state::TenantTaskRecord,
    fault_injector: &Arc<dyn FaultInjector>,
) -> Result<(), MemoryError> {
    let params: crate::tools::params::ExtractParams = serde_json::from_value(record.params.clone())
        .map_err(|error| {
            MemoryError::Validation(format!("invalid durable extract parameters: {error}"))
        })?;
    let service =
        crate::service::MemoryService::new(db, namespace.to_owned(), "info".into(), 100, 100)?
            .with_http_outbox();
    let extraction = crate::tools::extract(&service, params).await;
    match extraction {
        Ok(result) => {
            let value = serde_json::to_value(result).map_err(|error| {
                MemoryError::Storage(format!("serialize extract result: {error}"))
            })?;
            // The artifact is the durable commit boundary. Once it exists, a
            // cancellation request is reported as completed_before_cancel.
            task_store.record_artifact_fenced(handle, &value).await?;
            // Hit after the artifact row is committed. The next
            // worker sees the artifact via `reconcile_artifacts`
            // and projects the completed terminal state.
            fault_injector.hit(FaultPoint::TaskArtifactCommitted)?;
            let cancelled_after_commit = task_store
                .load(&handle.task_id)
                .await?
                .is_some_and(|task| task.cancellation_intent);
            task_store
                .complete_fenced(handle, value, cancelled_after_commit)
                .await?;
            // Hit after the terminal state is committed.
            fault_injector.hit(FaultPoint::TaskCompleted)?;
            Ok(())
        }
        Err(error) => {
            task_store
                .fail_fenced(handle, serde_json::json!({"message": error.to_string()}))
                .await
        }
    }
}

/// The reembed executor. **STUB — Task 6 replaces this body.**
///
/// Dispatch exists (Task 4) so the operator route can enqueue a `reembed` task
/// and the scheduler can route it here; the pass itself — building a
/// force-enabled `MemoryService`, parsing `ReembedOptions`, running
/// `reembed_all_facts`, heartbeating the lease (Task 5) and mapping
/// `ReembedOutcome` onto the durable state machine — lands in Task 6.
///
/// It fails the task rather than completing it, and says so in the stored
/// error. A silent success here would be the worst possible stub: the task
/// would read `completed` with no vectors rewritten, and Task 6's end-to-end
/// test would pass without Task 6 having run. A loud durable failure is a
/// truthful intermediate state an operator can see.
async fn execute_reembed_task(
    task_store: &DurableTaskStore,
    _db: Arc<SurrealDbClient>,
    _namespace: &str,
    handle: &crate::http::tasks::state::TaskHandle,
    _record: &crate::http::tasks::state::TenantTaskRecord,
) -> Result<(), MemoryError> {
    // STUB (Task 6). See the doc comment above: fails loudly, never succeeds.
    task_store
        .fail_fenced(
            handle,
            serde_json::json!({
                "message": "execute_reembed_task is not implemented: the reembed pass \
                            lands in Task 6, so this task failed without rewriting any vector",
            }),
        )
        .await
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
    // Dispatch mirrors [`execute_one_task`] exactly, so a test cannot observe a
    // kind routing the production scheduler would not have applied. Only the
    // `extract` executor is stubbed; `reembed` goes to the real one, because a
    // stubbed routing decision is precisely what this seam must not fake.
    match record.kind.as_str() {
        TASK_KIND_EXTRACT => {
            execute_extract_task_for_test(task_store, &handle, &record, &fault_injector, &extractor)
                .await
        }
        TASK_KIND_REEMBED => {
            execute_reembed_task(task_store, db, namespace, &handle, &record).await
        }
        other => {
            task_store
                .fail_fenced(
                    &handle,
                    serde_json::json!({
                        "message": format!("unknown task kind `{other}`"),
                    }),
                )
                .await
        }
    }
}

/// The stubbed `extract` executor behind [`execute_one_task_for_test`]. Runs
/// the same durable state machine with the same fault points as
/// [`execute_extract_task`], substituting the supplied [`ExtractorFn`] for the
/// real `extract` tool.
#[cfg(any(test, feature = "test-fixtures"))]
async fn execute_extract_task_for_test(
    task_store: &DurableTaskStore,
    handle: &crate::http::tasks::state::TaskHandle,
    record: &crate::http::tasks::state::TenantTaskRecord,
    fault_injector: &Arc<dyn FaultInjector>,
    extractor: &ExtractorFn,
) -> Result<(), MemoryError> {
    let params: crate::tools::params::ExtractParams = serde_json::from_value(record.params.clone())
        .map_err(|error| {
            MemoryError::Validation(format!("invalid durable extract parameters: {error}"))
        })?;
    // The stub extractor is only used in test-fixtures builds; the
    // real `extract` call is the production seam and lives in
    // `execute_extract_task`.
    let extraction = extractor(params).await;
    match extraction {
        Ok(value) => {
            task_store.record_artifact_fenced(handle, &value).await?;
            // Hit after the artifact row is committed. The next
            // worker sees the artifact via `reconcile_artifacts`
            // and projects the completed terminal state.
            fault_injector.hit(FaultPoint::TaskArtifactCommitted)?;
            let cancelled_after_commit = task_store
                .load(&handle.task_id)
                .await?
                .is_some_and(|task| task.cancellation_intent);
            task_store
                .complete_fenced(handle, value, cancelled_after_commit)
                .await?;
            // Hit after the terminal state is committed.
            fault_injector.hit(FaultPoint::TaskCompleted)?;
            Ok(())
        }
        Err(error) => {
            task_store
                .fail_fenced(handle, serde_json::json!({"message": error.to_string()}))
                .await
        }
    }
}
