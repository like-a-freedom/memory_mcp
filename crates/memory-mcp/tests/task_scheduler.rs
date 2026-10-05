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
use memory_mcp::http::tasks::scheduler::{execute_one_task_for_test, scheduler_job_with_options};
use memory_mcp::http::tasks::state::{TASK_KIND_EXTRACT, TASK_KIND_REEMBED, TaskState, TaskStore};
use memory_mcp::http::tasks::worker::DurableTaskStore;
use memory_mcp::storage::{BoundDbClient, DbClient, SurrealDbClient};

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
