#![cfg(all(
    feature = "streamable-http",
    feature = "control-plane",
    feature = "test-fixtures"
))]

//! Durable task scheduler behaviour.
//!
//! `http::tasks::scheduler` is the process-level job that recovers expired
//! claims, runs at most one due extraction per tenant per tick, and commits
//! the outcome through the fenced store API. Its test seam
//! (`execute_one_task_for_test`) replaces the real GLiNER-backed extract with
//! a stub, so the state machine can be driven without a local checkpoint.
//!
//! Run:
//! cargo test -p memory_mcp --all-features --test task_scheduler

use std::sync::Arc;

use memory_mcp::error::MemoryError;
use memory_mcp::http::registry::RegistryHandle;
use memory_mcp::http::tasks::DurableTaskTestDriver;
use memory_mcp::http::tasks::scheduler::{
    execute_one_task_for_test, run_task_heartbeated_for_test, scheduler_job_with_options,
};
use memory_mcp::http::tasks::state::{TASK_KIND_EXTRACT, TASK_KIND_REEMBED, TaskState, TaskStore};
use memory_mcp::http::tasks::worker::DurableTaskStore;
use memory_mcp::storage::{BoundDbClient, DbClient, SurrealDbClient};

use chrono::Utc;

/// Reembed pass parameters, exactly as the durable row stores them. Written out
/// rather than derived from the Rust struct because the durable contract is the
/// wire form: `max_failures`/`retry_failed` are not extract fields, so a
/// dispatcher that hardcodes `ExtractParams` cannot possibly decode them.
fn reembed_params() -> serde_json::Value {
    serde_json::json!({ "max_failures": null, "retry_failed": false })
}

struct SchedulerHarness {
    store: DurableTaskStore,
    client: Arc<SurrealDbClient>,
    namespace: String,
    _registry: RegistryHandle,
}

/// A fresh in-memory tenant namespace with the `tenant_task` table applied.
async fn harness() -> SchedulerHarness {
    let namespace = format!("task_sched_{}", uuid::Uuid::new_v4().simple());
    let registry = RegistryHandle::in_memory_with_default_mem_engine().await;
    let client = registry
        .tenant_engine_optional()
        .expect("in-memory engine wired")
        .bind_to_test_namespace(&namespace)
        .await;
    // The `tenant_task` table lives in the HTTP tenant migrations rather than
    // the storage migrations, so the test driver applies it for us.
    DurableTaskTestDriver::new_with_options(
        Arc::new(BoundDbClient::new(client.clone(), namespace.clone())),
        "ten_sched".to_string(),
        3600,
        16,
    )
    .apply_migrations_for_test(&namespace)
    .await
    .expect("apply tenant_task migration");
    let store = DurableTaskStore::new_with_options(
        Arc::new(BoundDbClient::new(client.clone(), namespace.clone())),
        "ten_sched".to_string(),
        3600,
        16,
    );
    SchedulerHarness {
        store,
        client,
        namespace,
        _registry: registry,
    }
}

/// Extract parameters naming `episode_id`, shaped exactly as the durable row
/// stores them. Explicitly written rather than derived from `ExtractParams`
/// because the durable contract is the wire form, not the Rust struct.
fn episode_params(episode_id: &str) -> serde_json::Value {
    serde_json::json!({ "episode_id": episode_id })
}

/// An extractor that always succeeds with a fixed artifact.
fn succeeding_extractor() -> memory_mcp::http::tasks::scheduler::ExtractorFn {
    Arc::new(|_params| Box::pin(async { Ok(serde_json::json!({"episode_id": "episode:abc"})) }))
}

/// An extractor that always fails.
fn failing_extractor() -> memory_mcp::http::tasks::scheduler::ExtractorFn {
    Arc::new(|_params| Box::pin(async { Err(MemoryError::Storage("extract failed".into())) }))
}

/// An injector that never fires, so the happy path runs to completion.
fn no_faults() -> Arc<dyn memory_mcp::platform::fault_injection::FaultInjector> {
    Arc::new(memory_mcp::platform::fault_injection::NoFaults)
}

#[tokio::test]
async fn executing_with_no_due_task_is_a_no_op() {
    let h = harness().await;

    let observed = execute_one_task_for_test(
        &h.store,
        h.client.clone(),
        &h.namespace,
        no_faults(),
        succeeding_extractor(),
    )
    .await;

    assert!(observed.is_ok(), "an empty queue must not fail the tick");
}

#[tokio::test]
async fn a_due_task_completes_after_a_successful_extraction() {
    let h = harness().await;
    let task_id = h
        .store
        .enqueue(TASK_KIND_EXTRACT, "fp-1", episode_params("episode:abc"))
        .await
        .expect("enqueue");

    execute_one_task_for_test(
        &h.store,
        h.client.clone(),
        &h.namespace,
        no_faults(),
        succeeding_extractor(),
    )
    .await
    .expect("tick succeeds");

    let observed = h.store.load(&task_id).await.expect("load");
    assert_eq!(observed.expect("task present").state, TaskState::Completed);
}

#[tokio::test]
async fn a_failing_extraction_marks_the_task_failed() {
    let h = harness().await;
    let task_id = h
        .store
        .enqueue(TASK_KIND_EXTRACT, "fp-2", episode_params("episode:abc"))
        .await
        .expect("enqueue");

    execute_one_task_for_test(
        &h.store,
        h.client.clone(),
        &h.namespace,
        no_faults(),
        failing_extractor(),
    )
    .await
    .expect("the tick itself succeeds");

    let observed = h.store.load(&task_id).await.expect("load");
    assert_eq!(observed.expect("task present").state, TaskState::Failed);
}

#[tokio::test]
async fn a_committed_task_records_its_artifact() {
    let h = harness().await;
    let task_id = h
        .store
        .enqueue(TASK_KIND_EXTRACT, "fp-3", episode_params("episode:abc"))
        .await
        .expect("enqueue");

    execute_one_task_for_test(
        &h.store,
        h.client.clone(),
        &h.namespace,
        no_faults(),
        succeeding_extractor(),
    )
    .await
    .expect("tick succeeds");

    let observed = h.store.load(&task_id).await.expect("load");
    assert!(
        observed.expect("task present").result.is_some(),
        "the artifact is the durable commit boundary"
    );
}

#[tokio::test]
async fn a_task_cancelled_before_the_claim_is_not_extracted() {
    let h = harness().await;
    let task_id = h
        .store
        .enqueue(TASK_KIND_EXTRACT, "fp-4", episode_params("episode:abc"))
        .await
        .expect("enqueue");
    h.store
        .set_cancellation_intent(&task_id)
        .await
        .expect("request cancellation");

    execute_one_task_for_test(
        &h.store,
        h.client.clone(),
        &h.namespace,
        no_faults(),
        succeeding_extractor(),
    )
    .await
    .expect("tick succeeds");

    let observed = h.store.load(&task_id).await.expect("load");
    assert_eq!(observed.expect("task present").state, TaskState::Cancelled);
}

#[tokio::test]
async fn a_task_cancelled_before_the_claim_records_no_artifact() {
    let h = harness().await;
    let task_id = h
        .store
        .enqueue(TASK_KIND_EXTRACT, "fp-5", episode_params("episode:abc"))
        .await
        .expect("enqueue");
    h.store
        .set_cancellation_intent(&task_id)
        .await
        .expect("request cancellation");

    execute_one_task_for_test(
        &h.store,
        h.client.clone(),
        &h.namespace,
        no_faults(),
        succeeding_extractor(),
    )
    .await
    .expect("tick succeeds");

    let observed = h.store.load(&task_id).await.expect("load");
    assert!(observed.expect("task present").result.is_none());
}

#[tokio::test]
async fn an_unparseable_parameter_set_fails_the_tick() {
    let h = harness().await;
    h.store
        .enqueue(
            TASK_KIND_EXTRACT,
            "fp-6",
            serde_json::json!({"unexpected": true}),
        )
        .await
        .expect("enqueue");

    let observed = execute_one_task_for_test(
        &h.store,
        h.client.clone(),
        &h.namespace,
        no_faults(),
        succeeding_extractor(),
    )
    .await;

    assert!(
        matches!(observed, Err(MemoryError::Validation(_))),
        "durable parameters that no longer decode are a server-side fault"
    );
}

#[tokio::test]
async fn a_second_tick_does_not_reprocess_a_completed_task() {
    let h = harness().await;
    let task_id = h
        .store
        .enqueue(TASK_KIND_EXTRACT, "fp-7", episode_params("episode:abc"))
        .await
        .expect("enqueue");
    execute_one_task_for_test(
        &h.store,
        h.client.clone(),
        &h.namespace,
        no_faults(),
        succeeding_extractor(),
    )
    .await
    .expect("first tick");

    // The second tick runs a failing extractor: had it claimed the completed
    // task again, the task would be rewritten to `Failed`.
    execute_one_task_for_test(
        &h.store,
        h.client.clone(),
        &h.namespace,
        no_faults(),
        failing_extractor(),
    )
    .await
    .expect("second tick");

    let observed = h.store.load(&task_id).await.expect("load");
    assert_eq!(observed.expect("task present").state, TaskState::Completed);
}

#[tokio::test]
async fn a_scheduler_job_runs_against_an_empty_registry() {
    let registry = RegistryHandle::in_memory_with_default_mem_engine().await;
    let job =
        scheduler_job_with_options(memory_mcp::http::runtime::storage::RuntimeOptions::default());

    let observed = job(registry).await;

    assert!(observed.is_ok(), "a tick with no tenants must succeed");
}

#[tokio::test]
async fn requeueing_expired_claims_succeeds_on_a_fresh_queue() {
    let h = harness().await;

    let observed = h.store.requeue_expired_running().await;

    assert!(observed.is_ok());
}

#[tokio::test]
async fn reconciling_artifacts_succeeds_with_no_committed_work() {
    let h = harness().await;

    let observed = h.store.reconcile_artifacts().await;

    assert!(observed.is_ok());
}

#[tokio::test]
async fn deleting_expired_tasks_keeps_a_fresh_task() {
    let h = harness().await;
    let task_id = h
        .store
        .enqueue(TASK_KIND_EXTRACT, "fp-8", episode_params("episode:abc"))
        .await
        .expect("enqueue");

    h.store.delete_expired().await.expect("delete expired");

    assert!(
        h.store.load(&task_id).await.expect("load").is_some(),
        "retention must not delete a task inside its window"
    );
}

/// A task row written before the `kind` column existed carries no `kind`. It
/// must still read back as `extract`, or every in-flight extraction breaks on
/// deploy: the legacy row is what an extraction queued before the migration is.
#[tokio::test]
async fn a_task_row_written_without_a_kind_reads_back_as_extract() {
    let h = harness().await;
    let task_id = "legacy-task-without-kind";
    let now = chrono::Utc::now().to_rfc3339();
    let seeded = DbClient::query(
        &*h.client,
        "CREATE type::record('tenant_task', $id) SET tenant_id = $tenant_id, fingerprint = 'fp-legacy', state = 'queued', version = 1, cancellation_intent = false, params = { episode_id: 'episode:legacy' }, created_at = type::datetime($now), updated_at = type::datetime($now), retention_expiry = type::datetime($now)",
        Some(serde_json::json!({
            "id": task_id,
            "tenant_id": "ten_sched",
            "now": now,
        })),
        &h.namespace,
    )
    .await
    .expect("seed a pre-migration tenant_task row");
    let seeded_rows: Vec<serde_json::Value> = serde_json::from_value(seeded).expect("seed rows");
    assert_eq!(seeded_rows.len(), 1, "the legacy row must exist");
    assert!(
        seeded_rows[0].get("kind").is_none(),
        "a row written before 045 has no kind field at all"
    );

    let record = h
        .store
        .load(task_id)
        .await
        .expect("load legacy row")
        .expect("legacy row present");

    assert_eq!(record.kind, TASK_KIND_EXTRACT);
}

/// The discriminator is persisted, not inferred: a reembed row reads back as
/// `reembed` so Task 4's dispatch can branch on it.
#[tokio::test]
async fn an_enqueued_kind_round_trips_through_the_store() {
    let h = harness().await;

    let extract_id = h
        .store
        .enqueue(
            TASK_KIND_EXTRACT,
            "fp-kind-extract",
            episode_params("episode:a"),
        )
        .await
        .expect("enqueue extract");
    let reembed_id = h
        .store
        .enqueue(TASK_KIND_REEMBED, "fp-kind-reembed", serde_json::json!({}))
        .await
        .expect("enqueue reembed");

    assert_eq!(
        h.store
            .load(&extract_id)
            .await
            .expect("load")
            .expect("present")
            .kind,
        TASK_KIND_EXTRACT
    );
    assert_eq!(
        h.store
            .load(&reembed_id)
            .await
            .expect("load")
            .expect("present")
            .kind,
        TASK_KIND_REEMBED
    );
}

/// The scheduler dispatches on `kind`. A `reembed` row whose payload is a
/// reembed pass (not an `ExtractParams`) must reach the reembed executor rather
/// than be decoded as extract parameters — which is the failure that made the
/// kind column inert: dispatch hardcoded `ExtractParams`, so the only kind
/// reachable was `extract`.
#[tokio::test]
async fn a_reembed_task_is_dispatched_to_the_reembed_executor() {
    let h = harness().await;
    let task_id = h
        .store
        .enqueue(TASK_KIND_REEMBED, "reembed:ten_sched", reembed_params())
        .await
        .expect("enqueue reembed");

    let observed = execute_one_task_for_test(
        &h.store,
        h.client.clone(),
        &h.namespace,
        no_faults(),
        succeeding_extractor(),
    )
    .await;

    // The tick itself must succeed: dispatch named the reembed executor, which
    // committed its own durable outcome rather than leaving the payload to be
    // decoded as extract parameters.
    assert!(
        observed.is_ok(),
        "a reembed task must not be decoded as extract parameters: {observed:?}"
    );
    let record = h
        .store
        .load(&task_id)
        .await
        .expect("load")
        .expect("reembed row present");
    assert_eq!(record.state, TaskState::Failed);
    assert!(
        names_reembed_stub(&record),
        "the row must carry the reembed executor's own outcome, so dispatch is \
         distinguishable from extract decoding: {record:?}"
    );
}

/// A kind the scheduler does not recognise fails closed, naming the kind. It is
/// never guessed at: decoding an unknown payload as whichever kind happens to
/// fit is how a maintenance task would be run as an extraction.
#[tokio::test]
async fn an_unrecognised_task_kind_fails_closed_naming_the_kind() {
    let h = harness().await;
    let task_id = h
        .store
        .enqueue(
            "not-a-kind",
            "fp-unknown-kind",
            serde_json::json!({ "whatever": true }),
        )
        .await
        .expect("enqueue unknown kind");

    let observed = execute_one_task_for_test(
        &h.store,
        h.client.clone(),
        &h.namespace,
        no_faults(),
        succeeding_extractor(),
    )
    .await;

    assert!(
        observed.is_ok(),
        "an unknown kind is a durable failure, not a tick error: {observed:?}"
    );
    let record = h
        .store
        .load(&task_id)
        .await
        .expect("load")
        .expect("unknown-kind row present");
    assert_eq!(record.state, TaskState::Failed);
    let message = record
        .error
        .expect("an unknown kind records an error")
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    assert!(
        message.contains("not-a-kind"),
        "the failure must name the unknown kind so the row is diagnosable: {message}"
    );
}

/// The lease a claim takes. `claim_next_due` writes `now + 60s`; the test
/// names that constant rather than restating the arithmetic, so a change to the
/// claim window is visible here instead of silently weakening the assertion.
const CLAIM_LEASE_SECS: i64 = 60;

/// Backdate the lease on a running row so a second replica can claim it, the
/// same shape the fenced-staleness tests use. Direct SQL on purpose: the
/// store must not expose "make my lease expire", and the operation under test
/// is the claim that follows.
async fn expire_lease_externally(h: &SchedulerHarness, task_id: &str) {
    let past = (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
    DbClient::query(
        &*h.client,
        "UPDATE tenant_task SET lease_expiry = type::datetime($past) WHERE id = type::record('tenant_task', $id)",
        Some(serde_json::json!({ "id": task_id, "past": past })),
        &h.namespace,
    )
    .await
    .expect("backdate the lease");
}

/// Renewing a lease twice keeps the fence and pushes the expiry forward.
///
/// This is the mechanism a reembed longer than 60 seconds depends on: the
/// generation is what another worker's claim would bump, so a renewal that
/// moved it would invalidate the very handle doing the renewal. It must not,
/// and the expiry it writes must actually move.
#[tokio::test]
async fn renewing_a_lease_extends_the_expiry_without_moving_the_fence() {
    let h = harness().await;
    h.store
        .enqueue(TASK_KIND_REEMBED, "reembed:ten_renew", reembed_params())
        .await
        .expect("enqueue reembed");
    let handle = h
        .store
        .claim_next_due("replica_a")
        .await
        .expect("claim")
        .expect("a queued task is due");
    let claimed_expiry = h
        .store
        .load(&handle.task_id)
        .await
        .expect("load")
        .expect("present")
        .lease_expiry
        .expect("a claimed row carries a lease expiry");

    h.store
        .renew_lease(
            &handle,
            chrono::Utc::now() + chrono::Duration::seconds(CLAIM_LEASE_SECS * 2),
        )
        .await
        .expect("first renewal succeeds on a live lease");
    let after_first = h
        .store
        .load(&handle.task_id)
        .await
        .expect("load")
        .expect("present");

    h.store
        .renew_lease(
            &handle,
            chrono::Utc::now() + chrono::Duration::seconds(CLAIM_LEASE_SECS * 3),
        )
        .await
        .expect("second renewal succeeds on a live lease");
    let after_second = h
        .store
        .load(&handle.task_id)
        .await
        .expect("load")
        .expect("present");

    let first = after_first
        .lease_expiry
        .expect("expiry after first renewal");
    let second = after_second
        .lease_expiry
        .expect("expiry after second renewal");
    assert!(
        second > first && first > claimed_expiry,
        "each renewal must push the expiry forward: claimed={claimed_expiry:?} \
         first={first:?} second={second:?}"
    );
    assert_eq!(
        after_second.lease_generation,
        Some(handle.lease_generation),
        "a renewal extends the lease; it never re-fences it, or the renewing \
         handle would invalidate itself"
    );
    assert_eq!(
        after_second.lease_owner.as_deref(),
        Some(handle.lease_owner.as_str()),
        "a renewal must not change the owner it was claimed under"
    );
    assert_eq!(
        after_second.state,
        TaskState::Running,
        "a renewal is not a state transition"
    );
}

/// A lease that expired and was stolen cannot be renewed by the worker that
/// lost it. The renewal is fenced on `lease_owner` AND `lease_generation`, so
/// the superseded worker gets `Conflict` and the row is left exactly as the
/// new owner wrote it — the alternative is a resurrected lease and a task that
/// runs twice forever.
#[tokio::test]
async fn renewing_a_superseded_lease_conflicts_and_leaves_the_row_untouched() {
    let h = harness().await;
    let task_id = h
        .store
        .enqueue(TASK_KIND_REEMBED, "reembed:ten_steal", reembed_params())
        .await
        .expect("enqueue reembed");
    let stale = h
        .store
        .claim_next_due("replica_a")
        .await
        .expect("first claim")
        .expect("a queued task is due");
    expire_lease_externally(&h, &task_id).await;
    let stolen = h
        .store
        .claim_next_due("replica_b")
        .await
        .expect("second claim")
        .expect("a running task with an expired lease is claimable");
    assert_eq!(
        stolen.lease_generation,
        stale.lease_generation + 1,
        "the steal is what supersedes the first worker"
    );
    let before = h
        .store
        .load(&task_id)
        .await
        .expect("load")
        .expect("present");

    let observed = h
        .store
        .renew_lease(
            &stale,
            chrono::Utc::now() + chrono::Duration::seconds(CLAIM_LEASE_SECS * 2),
        )
        .await;

    assert!(
        matches!(observed, Err(MemoryError::Conflict(_))),
        "a superseded lease must not be able to renew itself: {observed:?}"
    );
    let after = h
        .store
        .load(&task_id)
        .await
        .expect("load")
        .expect("present");
    assert_eq!(after.lease_expiry, before.lease_expiry);
    assert_eq!(after.lease_generation, before.lease_generation);
    assert_eq!(after.lease_owner, before.lease_owner);
    assert_eq!(after.state, TaskState::Running);
}

/// A pass that outlives its lease keeps its fence: the heartbeat extends the
/// lease while the body is still blocked, so the completion the body eventually
/// returns is committed under the original handle rather than rejected as a
/// lost fence.
///
/// The blocked body is the whole point of the test. Without one the tick
/// finishes long before the first heartbeat tick, and a heartbeat that never
/// fired would look exactly like one that did. The clock is real time — no
/// sleep anywhere — so the test costs one heartbeat period, which is why the
/// lease is a few seconds here rather than the production 60.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_blocked_pass_keeps_its_fence_through_the_heartbeat() {
    const HEARTBEAT_TTL_SECS: i64 = 3;
    let h = harness().await;
    let task_id = h
        .store
        .enqueue(TASK_KIND_REEMBED, "reembed:ten_long", reembed_params())
        .await
        .expect("enqueue reembed");
    // Claim it here rather than letting the helper claim, so the test owns the
    // handle: the wrapper must renew under exactly the fence it was given, and
    // it can only do that if the caller supplies the handle.
    let handle = h
        .store
        .claim_next_due("replica_a")
        .await
        .expect("claim")
        .expect("a queued reembed task is due");
    // Backdate the claim's own lease, without touching its owner or generation.
    // What follows is therefore a pass that begins after its lease has lapsed:
    // the row is claimable again, so a second replica would claim it and run a
    // second pass while the first is still rewriting vectors. That is the
    // failure Task 5 exists to prevent.
    expire_lease_externally(&h, &task_id).await;
    let lapsed_expiry = h
        .store
        .load(&task_id)
        .await
        .expect("load")
        .expect("present")
        .lease_expiry
        .expect("a claimed row carries a lease expiry");
    assert!(
        lapsed_expiry < Utc::now(),
        "the pass must start behind a lapsed lease, or it proves nothing"
    );

    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    // The pass itself: report that it started, then block until the test
    // releases it. Polling the row before that point would only prove the pass
    // is slow, not that the heartbeat fired. The receiver sits behind a mutex
    // so it can be shared with a future the helper may need to own more than
    // once; a single pass awaits it exactly once.
    let release = Arc::new(tokio::sync::Mutex::new(Some(release_rx)));
    let pass = {
        let release = release.clone();
        async move {
            let _ = entered_tx.send(());
            let mut blocked = release.lock().await;
            if let Some(receiver) = blocked.take() {
                let _ = receiver.await;
            }
            Ok(())
        }
    };

    let body_task = tokio::spawn(run_task_heartbeated_for_test(
        Arc::new(h.store.clone()),
        handle.clone(),
        HEARTBEAT_TTL_SECS,
        pass,
    ));

    // Explicit wait for the pass to start. A timeout here is a real failure,
    // not a hang: the heartbeat loop is already running.
    tokio::time::timeout(std::time::Duration::from_secs(30), entered_rx)
        .await
        .expect("the reembed pass must start")
        .expect("the pass must report that it started");

    // Explicit polling on the row — no sleep anywhere. Read until the heartbeat
    // has written a lease that is both still in the future and past the lapsed
    // one the pass started with. Only the heartbeat can move that field, so the
    // wait is on the write rather than on a duration.
    let settled = async {
        loop {
            let current = h
                .store
                .load(&task_id)
                .await
                .expect("load")
                .expect("present");
            if current
                .lease_expiry
                .is_some_and(|expiry| expiry > Utc::now() && expiry > lapsed_expiry)
            {
                return current;
            }
            tokio::task::yield_now().await;
        }
    };
    let renewed = tokio::time::timeout(std::time::Duration::from_secs(60), settled)
        .await
        .expect("the heartbeat must renew the lease while the pass is blocked");
    assert_eq!(
        renewed.state,
        TaskState::Running,
        "the pass is mid-flight, so the row must still read running: {renewed:?}"
    );
    assert_eq!(
        renewed.lease_generation,
        Some(handle.lease_generation),
        "the heartbeat renews the fence it was given; it never re-fences"
    );
    assert_eq!(
        renewed.lease_owner.as_deref(),
        Some("replica_a"),
        "renewal happens under the owner that holds the claim"
    );

    // The pass resolves under the fence it still holds, and commits its own
    // terminal outcome. Without the heartbeat this write would have matched no
    // rows: the lapped lease left the row claimable, and any other replica's
    // claim would have moved the generation out from under this handle.
    release_tx
        .send(())
        .expect("the blocked pass must still be waiting for its release");
    body_task
        .await
        .expect("the heartbeat wrapper must not panic")
        .expect("the pass returns while it still holds the fence");
    h.store
        .complete_fenced(
            &handle,
            serde_json::json!({ "message": "reembed pass finished" }),
            false,
        )
        .await
        .expect("the pass commits its own outcome while it still holds the fence");

    let after = h
        .store
        .load(&task_id)
        .await
        .expect("load")
        .expect("present");
    assert_eq!(
        after.state,
        TaskState::Completed,
        "a pass that heartbeated through its own expiry commits, rather than \
         being requeued and re-run: {after:?}"
    );
}

/// Whether the reembed row carries the Task 4 stub's loud marker. The stub
/// fails the task rather than completing it, precisely so this assertion can
/// tell "dispatched to reembed" apart from "silently succeeded" — a silent
/// success would make Task 6's end-to-end test pass without Task 6.
fn names_reembed_stub(record: &memory_mcp::http::tasks::state::TenantTaskRecord) -> bool {
    record
        .error
        .as_ref()
        .and_then(|error| error.get("message"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|message| message.contains("execute_reembed_task is not implemented"))
}
