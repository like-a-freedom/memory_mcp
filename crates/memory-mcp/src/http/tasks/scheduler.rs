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
    scheduler_job_with_policy(options, None)
}

/// The task job with the deployment's embedding policy attached, so a claimed
/// `reembed` row can force-enable the deployment provider for a tenant whose
/// own runtime is degraded.
///
/// A sibling rather than a parameter on [`scheduler_job_with_options`], because
/// the policy is an input only the reembed executor reads: widening the
/// existing entry point would make every current caller — and the deprecated
/// [`scheduler_job`] that delegates to it — name an argument it does not use.
///
/// [`SchedulerJob`] receives only the [`RegistryHandle`], so the policy is
/// captured by the closure, exactly as `RuntimeOptions` is. `None` means the
/// deployment has no embedding policy at all, which is what a lexical-only
/// deployment passes; a reembed row then fails loudly rather than running with
/// a provider nobody resolved.
pub fn scheduler_job_with_policy(
    options: crate::http::runtime::storage::RuntimeOptions,
    policy: Option<crate::http::runtime::bootstrap::DeploymentPolicy>,
) -> SchedulerJob {
    let options_for_job = options.clone();
    let policy_for_job = policy.clone();
    Arc::new(move |registry| {
        let options = options_for_job.clone();
        let embedding = policy_for_job
            .as_ref()
            .and_then(|policy| policy.embedding.as_ref())
            .cloned();
        let injector = options.fault_injector.clone();
        Box::pin(async move {
            retry_reconcile_and_retain_with_policy(&registry, options, injector, embedding).await
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

/// [`retry_reconcile_and_retain_with_policy`] with no embedding policy: the
/// pre-Task-6 entry point, where a `reembed` row could not be executed at all
/// because no provider was reachable from here.
async fn retry_reconcile_and_retain_with_options(
    registry: &RegistryHandle,
    options: crate::http::runtime::storage::RuntimeOptions,
    fault_injector: Arc<dyn FaultInjector>,
) -> Result<(), MemoryError> {
    retry_reconcile_and_retain_with_policy(registry, options, fault_injector, None).await
}

/// The pass itself, with the deployment's embedding policy, which only the
/// reembed executor reads.
async fn retry_reconcile_and_retain_with_policy(
    registry: &RegistryHandle,
    options: crate::http::runtime::storage::RuntimeOptions,
    fault_injector: Arc<dyn FaultInjector>,
    embedding: Option<crate::http::runtime::storage::EmbeddingPolicy>,
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
            embedding.as_ref(),
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
    embedding: Option<&crate::http::runtime::storage::EmbeddingPolicy>,
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
            // A reembed needs the deployment's provider. A tick with no policy
            // has none to force-enable, so the row fails loudly rather than
            // being decoded as an extraction or silently rewritten with a
            // degraded provider.
            let Some(policy) = embedding else {
                return task_store
                    .fail_fenced(
                        &handle,
                        serde_json::json!({
                            "message": "no deployment embedding policy is configured, so a reembed \
                                        cannot force-enable a provider for this tenant",
                    }),
                    )
                    .await;
            };
            execute_reembed_task(task_store, db, namespace, &handle, &record, policy).await
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

/// The reembed executor: one whole-namespace rewrite, committed through the
/// durable state machine.
///
/// The service it builds is **force-enabled**. `prepare_reembed_pass` refuses a
/// service carrying a disabled provider or a `None` signature, which is correct
/// for serving and fatal for rewriting — and the namespaces that need a reembed
/// most are precisely those whose runtime provider the activation path degraded,
/// because their stored vectors disagree with the deployment's signature. If
/// this executor reused the tenant's runtime provider, the one tenant that most
/// needs the rewrite would be the one that cannot get it. So it builds its own
/// service from the deployment policy and forces that identity onto it, through
/// the same helper the stdio CLI uses.
///
/// The mapping onto the durable state machine:
///
/// | [`ReembedOutcome`]            | durable state | why                                    |
/// |-------------------------------|---------------|----------------------------------------|
/// | `Completed` / `NothingToDo`  | `completed`   | every vector agrees with the target   |
/// | `CompletedWithErrors`         | `completed`   | within quota; the failure count is in the result |
/// | `Failed` / `Interrupted`      | `failed`      | cut short; resumable, because `embedding_job:fact_reembed` holds the cursor |
///
/// A `Failed`/`Interrupted` row is re-requestable rather than a dead end: the
/// job row's `last_completed_fact_id` means the next pass resumes at the
/// cursor instead of restarting, and the operator route's per-tenant
/// fingerprint lets a failed row be re-requested (the dedupe only names a
/// *live* task).
///
/// `CompletedWithErrors` completes rather than fails because the pass did
/// finish: the vectors it did rewrite are at the target, and an operator who
/// re-runs after a within-quota failure must get the failures retried rather
/// than have them silently treated as fatal.
async fn execute_reembed_task(
    task_store: &DurableTaskStore,
    db: Arc<SurrealDbClient>,
    namespace: &str,
    handle: &crate::http::tasks::state::TaskHandle,
    record: &crate::http::tasks::state::TenantTaskRecord,
    policy: &crate::http::runtime::storage::EmbeddingPolicy,
) -> Result<(), MemoryError> {
    // One token for the pass and the heartbeat. A shutdown cancels both, and a
    // lost fence cancels it too — from the pass's point of view those are the
    // same signal: stop, someone else owns this task now.
    let cancel = tokio_util::sync::CancellationToken::new();
    let service = build_force_enabled_reembed_service(db, namespace, policy)?;
    let options = ReembedTaskParams::from_record(record)?.into();
    // `LogProgressReporter::new` takes the logger by value and
    // `MemoryService::logger` is a `Clone`, so the reporter borrows a clone
    // rather than fighting the service for its own field.
    let reporter =
        crate::service::reembed_progress::LogProgressReporter::new(service.logger.clone());
    let cancel_for_pass = cancel.clone();
    let pass = run_task_heartbeated(
        Arc::new(task_store.clone()),
        handle.clone(),
        crate::http::tasks::worker::TASK_LEASE_TTL_SECS,
        cancel,
        async move {
            service
                .reembed_all_facts(&options, &reporter, &cancel_for_pass)
                .await
        },
    )
    .await;
    // The terminal write is deliberately *outside* the heartbeat: the loop is
    // stopped before the outcome is committed, so the last renewal cannot race
    // the state transition it exists to protect.
    match pass {
        Ok((summary, outcome)) => {
            let failed = matches!(
                outcome,
                crate::service::reembed_options::ReembedOutcome::Failed
                    | crate::service::reembed_options::ReembedOutcome::Interrupted
            );
            let result = reembed_result(&outcome, &summary);
            if failed {
                task_store.fail_fenced(handle, result).await
            } else {
                task_store.complete_fenced(handle, result, false).await
            }
        }
        Err(error) => {
            // The pass returned an error rather than an outcome (the provider
            // refused, the namespace was unreadable, the fence was lost).
            // Nothing partial is reported as done: the operator's row says the
            // rewrite did not complete, and the next pass resumes from the
            // durable cursor rather than restarting.
            task_store
                .fail_fenced(handle, serde_json::json!({"message": error.to_string()}))
                .await
        }
    }
}

/// Build the service a reembed pass runs against, with the deployment's
/// embedding identity **forced** onto it.
///
/// The forcing is the point, and it goes through
/// [`crate::bootstrap::stdio::forced_embedding_runtime_state`] rather than a
/// private copy of the same line: stdio's `ForceEnabledForReembed` arm and this
/// executor share one definition of "ignore the namespace decision, use the
/// deployment's provider".
///
/// `new_with_embedding_provider` starts the service with a `None` signature
/// (it cannot know one), and `prepare_reembed_pass` refuses exactly that — so
/// without the forced state the pass would fail with "reembed requires an
/// enabled embedding signature" on every namespace, degraded or not.
///
/// The entity extractor is the built-in rule extractor rather than the
/// deployment's: a reembed rewrites vectors, not entities, so loading a
/// deployment's GLiNER checkpoint here would cost a model load per pass and
/// change nothing about the vectors written.
fn build_force_enabled_reembed_service(
    db: Arc<SurrealDbClient>,
    namespace: &str,
    policy: &crate::http::runtime::storage::EmbeddingPolicy,
) -> Result<crate::service::MemoryService, MemoryError> {
    let extractor =
        std::sync::Arc::new(crate::knowledge::entity_extraction::AnnoEntityExtractor::new()?)
            as std::sync::Arc<dyn crate::knowledge::entity_extraction::EntityExtractor>;
    // `Arc<SurrealDbClient>` coerces to `Arc<dyn DbClient>`; the client itself
    // is neither `Clone` nor able to lend its engine, so the `Arc` handle the
    // caller already owns is the only conversion available (ADR-0042).
    let service = crate::service::MemoryService::new_with_embedding_provider(
        db as Arc<dyn crate::storage::client::DbClient>,
        namespace.to_owned(),
        "info".into(),
        // Rate limits are request-path policy; a maintenance pass is not
        // request traffic and must not be throttled by one.
        100,
        100,
        policy.provider.clone(),
        crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
        extractor,
    )?;
    service.replace_embedding_runtime_state(
        crate::bootstrap::stdio::forced_embedding_runtime_state(
            policy.provider.clone(),
            Some(policy.signature.clone()),
            policy.model.clone(),
            Some(policy.dimension),
        ),
    );
    Ok(service)
}

/// The durable row's reembed parameters, decoded from the stored `params`.
///
/// `ReembedOptions` is not `Deserialize` (it is a CLI-shaped struct whose
/// `max_failures: Option<usize>` carries documented `None`/`Some(0)` semantics),
/// so the wire form is read here and converted.
///
/// The read goes through [`durable_scalar`] because `params` is a SurrealDB
/// `option<object> FLEXIBLE` column, and the values in it come back in the
/// server's **tagged** wire form rather than as the JSON that went in:
///
/// ```text
/// enqueued: {"max_failures": null, "retry_failed": false}
/// stored:   {"max_failures": "Null",   "retry_failed": {"Bool": false}}
/// ```
///
/// A plain `serde` decode therefore fails on every row the operator route has
/// ever written — with `invalid type: string "Null", expected usize` for the
/// first field and `invalid type: map, expected a boolean` for the second. This
/// is a property of the durable form rather than of any one writer, so it is
/// decoded once, here, instead of patched at the route.
struct ReembedTaskParams {
    max_failures: Option<usize>,
    retry_failed: bool,
}

impl ReembedTaskParams {
    fn from_record(
        record: &crate::http::tasks::state::TenantTaskRecord,
    ) -> Result<Self, MemoryError> {
        let params = &record.params;
        Ok(Self {
            max_failures: durable_scalar::<usize>(params.get("max_failures"))?,
            // An absent, null or `"Null"` value means "not set", and not set
            // means the wire default the operator route writes: `false`.
            retry_failed: durable_scalar::<bool>(params.get("retry_failed"))?.unwrap_or(false),
        })
    }
}

impl From<ReembedTaskParams> for crate::service::reembed_options::ReembedOptions {
    fn from(params: ReembedTaskParams) -> Self {
        Self {
            max_failures: params.max_failures,
            retry_failed: params.retry_failed,
        }
    }
}

/// Read one field out of a durable `params` payload, accepting both the JSON
/// that was enqueued and the tagged wire form SurrealDB stores it in.
///
/// `None` covers all three ways a field can be unset: the key is absent, the
/// value is JSON `null`, or the value is SurrealDB's `"Null"` marker. Anything
/// that is present but unreadable as `T` is an error rather than a silent
/// default — a row whose `max_failures` says `"twelve"` must fail the pass, not
/// quietly reembed with the default quota.
fn durable_scalar<T>(value: Option<&serde_json::Value>) -> Result<Option<T>, MemoryError>
where
    T: serde::de::DeserializeOwned,
{
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() || value.as_str() == Some("Null") {
        return Ok(None);
    }
    // Unwrap a single-key tag (`{"Bool": false}`, `{"Number": 3}`) back to the
    // value it wraps, so both spellings decode through one path.
    let untagged = match value.as_object() {
        Some(object) if object.len() == 1 => object.values().next().unwrap_or(value).clone(),
        _ => value.clone(),
    };
    serde_json::from_value(untagged).map(Some).map_err(|error| {
        MemoryError::Validation(format!(
            "invalid durable reembed parameter {value}: {error}"
        ))
    })
}

/// The durable result payload for a pass that returned an outcome.
///
/// `CompletedWithErrors` completes with its failure count here rather than
/// failing the row: the pass finished, the vectors it did write are at the
/// target, and an operator reading `failed_facts` is told the truth about the
/// ones that were not.
fn reembed_result(
    outcome: &crate::service::reembed_options::ReembedOutcome,
    summary: &crate::service::ReembedSummary,
) -> serde_json::Value {
    use crate::service::reembed_options::ReembedOutcome;
    let outcome = match outcome {
        ReembedOutcome::Completed => "completed",
        ReembedOutcome::CompletedWithErrors => "completed_with_errors",
        ReembedOutcome::Failed => "failed",
        ReembedOutcome::Interrupted => "interrupted",
        ReembedOutcome::NothingToDo => "nothing_to_do",
    };
    serde_json::json!({
        "outcome": outcome,
        "total_facts": summary.total_facts,
        "processed_facts": summary.processed_facts,
        "succeeded_facts": summary.succeeded_facts,
        "failed_facts": summary.failed_facts,
    })
}

/// Test-only seam over the reembed executor: claim a real due `reembed` row and
/// delegate to [`execute_reembed_task`] with the deployment policy.
///
/// A sibling rather than a widened [`execute_one_task_for_test`], because the
/// policy is an input only the reembed executor reads, and the stub extractor
/// is an input only the extract executor reads. Two task kinds, two shapes of
/// seam — neither shared function grows an argument its other caller ignores.
///
/// Claims nothing and commits nothing of its own: the claim, the heartbeat and
/// the terminal write all happen inside the production executor, so a test
/// observes exactly what the scheduler would observe.
#[cfg(any(test, feature = "test-fixtures"))]
pub async fn execute_reembed_task_for_test(
    task_store: &DurableTaskStore,
    db: Arc<SurrealDbClient>,
    namespace: &str,
    policy: &crate::http::runtime::storage::EmbeddingPolicy,
) -> Result<(), MemoryError> {
    let replica_id = crate::http::leases::scheduler::replica_id();
    let Some(handle) = task_store.claim_next_due(&replica_id).await? else {
        return Ok(());
    };
    let record = task_store.load(&handle.task_id).await?.ok_or_else(|| {
        MemoryError::NotFound(format!("task {} disappeared after claim", handle.task_id))
    })?;
    if record.cancellation_intent {
        return task_store.cancel_before_commit_fenced(&handle).await;
    }
    if record.kind.as_str() != TASK_KIND_REEMBED {
        // The caller asked for a reembed tick. Anything else is a test bug, and
        // running it anyway would mean the row is rewritten by an executor the
        // test did not mean to exercise.
        return task_store
            .fail_fenced(
                &handle,
                serde_json::json!({
                    "message": format!(
                        "execute_reembed_task_for_test claimed a `{}` task, not a reembed",
                        record.kind
                    ),
                }),
            )
            .await;
    }
    execute_reembed_task(task_store, db, namespace, &handle, &record, policy).await
}

/// Heartbeat the task lease while a long pass runs, then return the pass's own
/// result.
///
/// A claim lasts [`TASK_LEASE_TTL_SECS`]. A reembed over a large namespace can
/// outlive that, and once the lease lapses the row is claimable again: a second
/// replica claims it, bumps the generation, and starts a second pass while the
/// first is still rewriting vectors. Both passes then fail their fenced
/// completion, and the task is re-run forever. Renewing on a `ttl / 3` cadence
/// keeps the fence held for as long as the pass runs.
///
/// The loop's arithmetic is deliberately *not* shared with
/// [`crate::http::leases::migration::run_heartbeated`], which cannot be reused
/// here (it renews a provisioning lease in a different table through a
/// different store). Duplicating ~10 lines of cadence arithmetic is the cheap
/// half of that trade: the two leases are unrelated, and a shared helper for
/// exactly two callers whose lease types share nothing is an abstraction with
/// no reason behind it. When a *third* lease type needs the same cadence,
/// extract it then — this is the one place in the plan where YAGNI and DRY
/// genuinely disagreed, and YAGNI won.
///
/// Two boundaries, both deliberate:
/// - `shutdown` stops the loop and, because the caller hands the same token to
///   the pass, the pass too. One shutdown signal, no ordering to get wrong.
/// - A `renew_lease` that returns `Conflict` means the fence is gone, so the
///   shared token is cancelled: the pass is told to stop rather than running on
///   against a task another worker now owns. The pass's own result stays
///   authoritative — the wrapper does not invent an error on its behalf.
async fn run_task_heartbeated<F, T>(
    task_store: Arc<DurableTaskStore>,
    handle: crate::http::tasks::state::TaskHandle,
    ttl_secs: i64,
    shutdown: tokio_util::sync::CancellationToken,
    body: F,
) -> Result<T, MemoryError>
where
    F: std::future::Future<Output = Result<T, MemoryError>> + Send,
    T: Send + 'static,
{
    let base_interval =
        std::time::Duration::from_secs(u64::try_from((ttl_secs / 3).max(1)).unwrap_or(u64::MAX));
    // Apply ±20% jitter from the process clock without adding a random-number
    // dependency to this path: two replicas that started together must not
    // renew on the same instant forever.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.subsec_nanos() as u64)
        .unwrap_or(0);
    let span_ms = (base_interval.as_millis() / 5) as u64;
    let jitter_ms = if span_ms == 0 {
        0
    } else {
        nanos % (span_ms * 2)
    };
    let offset = jitter_ms.saturating_sub(span_ms);
    let mut interval =
        tokio::time::interval(base_interval + std::time::Duration::from_millis(offset));
    // Skip, not Burst: a pass that blocked the runtime past several periods has
    // no use for a burst of renewals for periods it already skipped.
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Do not renew synchronously before the pass starts: the lease was just
    // claimed, and the first regular tick is well inside the window.
    interval.tick().await;

    let heartbeat_cancel = tokio_util::sync::CancellationToken::new();
    let heartbeat = tokio::spawn({
        let heartbeat_cancel = heartbeat_cancel.clone();
        let shutdown = shutdown.clone();
        async move {
            loop {
                tokio::select! {
                    _ = heartbeat_cancel.cancelled() => break,
                    _ = shutdown.cancelled() => break,
                    _ = interval.tick() => {
                        let extend_to =
                            chrono::Utc::now() + chrono::Duration::seconds(ttl_secs);
                        if task_store.renew_lease(&handle, extend_to).await.is_err() {
                            // The fence is gone. Stop the pass too: the caller
                            // shares this token with it.
                            shutdown.cancel();
                            break;
                        }
                    }
                }
            }
        }
    });

    let result = body.await;
    heartbeat_cancel.cancel();
    let _ = heartbeat.await;
    result
}

/// Test-only seam over [`run_task_heartbeated`]. Claims nothing and commits
/// nothing: the caller owns the `TaskHandle` and writes the terminal outcome
/// itself, which is what lets the test drive a pass that outlives its lease and
/// then observe that the completion it writes still matches the row.
///
/// It exists under the same gate as [`execute_one_task_for_test`] because a
/// test outside the crate cannot reach a private function, and the alternative
/// — widening the production entry point's visibility for one test — is the
/// worse trade.
#[cfg(any(test, feature = "test-fixtures"))]
pub async fn run_task_heartbeated_for_test<F, T>(
    task_store: Arc<DurableTaskStore>,
    handle: crate::http::tasks::state::TaskHandle,
    ttl_secs: i64,
    body: F,
) -> Result<T, MemoryError>
where
    F: std::future::Future<Output = Result<T, MemoryError>> + Send,
    T: Send + 'static,
{
    run_task_heartbeated(
        task_store,
        handle,
        ttl_secs,
        tokio_util::sync::CancellationToken::new(),
        body,
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
    execute_one_task_with_policy(task_store, db, namespace, &fault_injector, extractor, None).await
}

/// Test-only mirror of [`execute_one_task`] that also carries the deployment's
/// embedding policy, so a `reembed` row dispatched through the seam reaches the
/// real reembed executor with a provider it can force-enable.
///
/// A sibling rather than a fifth parameter on [`execute_one_task_for_test`]:
/// the provider is an argument only one of the two task kinds reads, and every
/// existing caller of that seam would carry a parameter it ignores.
#[cfg(any(test, feature = "test-fixtures"))]
pub async fn execute_one_task_with_policy(
    task_store: &DurableTaskStore,
    db: Arc<SurrealDbClient>,
    namespace: &str,
    fault_injector: &Arc<dyn FaultInjector>,
    extractor: ExtractorFn,
    embedding_policy: Option<&crate::http::runtime::storage::EmbeddingPolicy>,
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
            execute_extract_task_for_test(task_store, &handle, &record, fault_injector, &extractor)
                .await
        }
        TASK_KIND_REEMBED => {
            // A reembed goes to the real executor, because a stubbed routing
            // decision is precisely what this seam must not fake — and, since
            // Task 6, that executor needs a provider to force-enable. It gets
            // the same shape of failure production does: a row dispatched here
            // without a policy fails loudly instead of running with a degraded
            // provider.
            let Some(policy) = embedding_policy else {
                return task_store
                    .fail_fenced(
                        &handle,
                        serde_json::json!({
                            "message": "no deployment embedding policy is configured, so a reembed \
                                        cannot force-enable a provider for this tenant",
                    }),
                    )
                    .await;
            };
            execute_reembed_task(task_store, db, namespace, &handle, &record, policy).await
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
