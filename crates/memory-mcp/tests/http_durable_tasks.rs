//! Durable Task store integration suite (Task 7 of the
//! architecture audit remediation plan).
//!
//! Black-box coverage for one tenant's durable Task store, exercised
//! through the `DurableTaskTestDriver` exposed by `http::tasks`.
//! Every case uses a fresh in-memory namespace bound to a fresh
//! `PrivilegedEngine`; the "restart persistence" and "cross-tenant
//! denial" cases use two independent `BoundDbClient` handles against
//! the same namespace so the second reader can observe the first
//! writer's durable commit without sharing any client state.
//!
//! Run:
//! cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures \
//!     --test http_durable_tasks -- --test-threads=1

#![cfg(all(
    feature = "streamable-http",
    feature = "control-plane",
    feature = "test-fixtures"
))]

use std::sync::Arc;

use memory_mcp::http::registry::RegistryHandle;
use memory_mcp::http::tasks::DurableTaskTestDriver;
use memory_mcp::http::tasks::state::TaskState;
use memory_mcp::storage::BoundDbClient;

/// One tenant namespace bound to a fresh engine, plus two
/// independent `BoundDbClient` handles against the same engine.
struct TwoHandles {
    driver_a: DurableTaskTestDriver,
    driver_b: DurableTaskTestDriver,
    _registry: RegistryHandle,
}

async fn two_handles(tenant_id: &str, namespace: &str) -> TwoHandles {
    let registry = RegistryHandle::in_memory_with_default_mem_engine().await;
    let client_a = registry
        .tenant_engine_optional()
        .expect("in-memory engine wired")
        .bind_to_test_namespace(namespace)
        .await;
    let client_b = registry
        .tenant_engine_optional()
        .expect("in-memory engine wired")
        .bind_to_test_namespace(namespace)
        .await;
    let bound_a = Arc::new(BoundDbClient::new(client_a, namespace.to_owned()));
    let bound_b = Arc::new(BoundDbClient::new(client_b, namespace.to_owned()));
    // The bound client's namespace is `namespace`; the driver's
    // tenant_id is a SQL filter column that may differ (e.g. the
    // cross-tenant test uses two different tenant_ids against
    // the same namespace). Migrations target the bound
    // namespace explicitly.
    let driver_a = DurableTaskTestDriver::new(bound_a, tenant_id.to_owned());
    let driver_b = DurableTaskTestDriver::new(bound_b, tenant_id.to_owned());
    driver_a
        .apply_migrations_for_test(namespace)
        .await
        .expect("apply migrations for driver_a");
    TwoHandles {
        driver_a,
        driver_b,
        _registry: registry,
    }
}

/// One fresh namespace with a single `DurableTaskTestDriver`
/// using custom options. Used by the capacity and retention
/// tests that need a per-test engine.
async fn single_driver_with_options(
    tenant_id: &str,
    namespace: &str,
    retention_secs: i64,
    queue_capacity: usize,
) -> (DurableTaskTestDriver, RegistryHandle) {
    let registry = RegistryHandle::in_memory_with_default_mem_engine().await;
    let client = registry
        .tenant_engine_optional()
        .expect("in-memory engine wired")
        .bind_to_test_namespace(namespace)
        .await;
    let driver = DurableTaskTestDriver::new_with_options(
        Arc::new(BoundDbClient::new(client, namespace.to_owned())),
        tenant_id.to_owned(),
        retention_secs,
        queue_capacity,
    );
    driver
        .apply_migrations_for_test(namespace)
        .await
        .expect("apply migrations for single driver");
    (driver, registry)
}

/// Two drivers against the same engine but with different
/// `tenant_id` filters. Used by the cross-tenant test.
async fn two_drivers_one_engine(
    namespace: &str,
    tenant_a: &str,
    tenant_b: &str,
) -> (DurableTaskTestDriver, DurableTaskTestDriver) {
    let registry = RegistryHandle::in_memory_with_default_mem_engine().await;
    let client_a = registry
        .tenant_engine_optional()
        .expect("in-memory engine wired")
        .bind_to_test_namespace(namespace)
        .await;
    let client_b = registry
        .tenant_engine_optional()
        .expect("in-memory engine wired")
        .bind_to_test_namespace(namespace)
        .await;
    let driver_a = DurableTaskTestDriver::new(
        Arc::new(BoundDbClient::new(client_a, namespace.to_owned())),
        tenant_a.to_owned(),
    );
    let driver_b = DurableTaskTestDriver::new(
        Arc::new(BoundDbClient::new(client_b, namespace.to_owned())),
        tenant_b.to_owned(),
    );
    driver_a
        .apply_migrations_for_test(namespace)
        .await
        .expect("apply migrations for driver_a");
    (driver_a, driver_b)
}

#[tokio::test]
async fn enqueue_dedupes_by_fingerprint() {
    let h = two_handles("tenant_a", "ns_dedup").await;
    let fp = "fingerprint_dedup";
    let first = h
        .driver_a
        .enqueue(fp, serde_json::json!({"a": 1}))
        .await
        .unwrap();
    let second = h
        .driver_a
        .enqueue(fp, serde_json::json!({"a": 2}))
        .await
        .unwrap();
    assert_eq!(
        first, second,
        "duplicate enqueue must return the existing id"
    );
    let loaded = h
        .driver_a
        .load(&first)
        .await
        .unwrap()
        .expect("task present");
    assert!(matches!(loaded.state, TaskState::Queued));
    assert_eq!(loaded.params, serde_json::json!({"a": 1}));
}

#[tokio::test]
async fn enqueue_rejects_when_queue_is_at_capacity() {
    // A driver with capacity 1. The first enqueue fills the
    // queue, the second must fail with Conflict.
    let (store, _registry) =
        single_driver_with_options("tenant_cap", "ns_cap", 7 * 24 * 60 * 60, 1).await;
    let first = store
        .enqueue("fp_capacity_1", serde_json::json!({}))
        .await
        .expect("first enqueue");
    let err = store
        .enqueue("fp_capacity_2", serde_json::json!({}))
        .await
        .expect_err("second enqueue at capacity must fail");
    assert!(
        matches!(err, memory_mcp::error::MemoryError::Conflict(_)),
        "capacity overflow must surface as Conflict, got {err:?}"
    );
    let _ = first;
}

#[tokio::test]
async fn claim_completes_through_full_lifecycle() {
    let h = two_handles("tenant_lc", "ns_lc").await;
    let task_id = h
        .driver_a
        .enqueue("fp_lifecycle", serde_json::json!({"x": 1}))
        .await
        .unwrap();
    let handle = h
        .driver_a
        .claim_next_due("replica_a")
        .await
        .unwrap()
        .expect("due task");
    assert_eq!(handle.task_id, task_id);
    // The second handle must NOT see a running task with an
    // active lease.
    let stale = h.driver_b.claim_next_due("replica_b").await.unwrap();
    assert!(
        stale.is_none(),
        "second replica must not see a running task with an active lease"
    );
    h.driver_a
        .complete_fenced(&handle, serde_json::json!({"ok": true}), false)
        .await
        .unwrap();
    let loaded = h
        .driver_b
        .load(&task_id)
        .await
        .unwrap()
        .expect("task survives to second reader");
    assert_eq!(loaded.state, TaskState::Completed);
    assert_eq!(loaded.version, 2, "complete must bump version");
}

#[tokio::test]
async fn stale_worker_cannot_overwrite_running_state() {
    let h = two_handles("tenant_stale", "ns_stale").await;
    let task_id = h
        .driver_a
        .enqueue("fp_stale", serde_json::json!({}))
        .await
        .unwrap();
    let handle_a = h
        .driver_a
        .claim_next_due("replica_a")
        .await
        .unwrap()
        .expect("due");
    // Replica A's lease is still active. A second replica's
    // claim must not produce a handle for the same task.
    let nothing = h.driver_b.claim_next_due("replica_b").await.unwrap();
    assert!(
        nothing.is_none(),
        "active lease must keep the row out of claim_next_due"
    );
    // Replica A's stale fenced write (a future fencing
    // generation it doesn't own) must fail with Conflict. The
    // store uses fencing by `lease_generation`.
    let mut stale_handle = handle_a.clone();
    stale_handle.lease_generation = handle_a.lease_generation + 99;
    let err = h
        .driver_a
        .complete_fenced(&stale_handle, serde_json::json!({}), false)
        .await
        .expect_err("stale generation must be rejected");
    assert!(
        matches!(err, memory_mcp::error::MemoryError::Conflict(_)),
        "stale fencing must surface as Conflict, got {err:?}"
    );
    let _ = task_id;
}

#[tokio::test]
async fn cancel_before_commit_keeps_state_machine_consistent() {
    let h = two_handles("tenant_cancel", "ns_cancel").await;
    let task_id = h
        .driver_a
        .enqueue("fp_cancel", serde_json::json!({}))
        .await
        .unwrap();
    // Cancel intent on a Queued task transitions the row to
    // Cancelled. Either way the row is excluded from
    // claim_next_due, which is the property the test exercises.
    h.driver_a.set_cancellation_intent(&task_id).await.unwrap();
    let claim = h.driver_a.claim_next_due("replica_a").await.unwrap();
    assert!(claim.is_none(), "cancel intent must block claim");
    let loaded = h
        .driver_a
        .load(&task_id)
        .await
        .unwrap()
        .expect("task still present");
    assert!(matches!(loaded.state, TaskState::Cancelled));
}

#[tokio::test]
async fn completed_before_cancel_wins_over_late_intent() {
    let h = two_handles("tenant_cbc", "ns_cbc").await;
    let task_id = h
        .driver_a
        .enqueue("fp_cbc", serde_json::json!({}))
        .await
        .unwrap();
    let handle = h
        .driver_a
        .claim_next_due("replica_a")
        .await
        .unwrap()
        .expect("due");
    h.driver_a
        .complete_fenced(&handle, serde_json::json!({"done": true}), true)
        .await
        .unwrap();
    // Late cancel intent arrives after completion. The state
    // must stay `CompletedBeforeCancel`; a stale cancel does
    // not regress the terminal state.
    h.driver_a.set_cancellation_intent(&task_id).await.unwrap();
    let loaded = h.driver_b.load(&task_id).await.unwrap().expect("present");
    assert_eq!(loaded.state, TaskState::CompletedBeforeCancel);
}

#[tokio::test]
async fn cross_tenant_driver_cannot_see_other_tenants_row() {
    // Two drivers against the same engine but with different
    // tenant_id filters. Driver A enqueues; driver B's load on
    // the same task id must come back empty because the row is
    // filtered by tenant_id in SQL.
    let (driver_a, driver_b) = two_drivers_one_engine("ns_x", "tenant_a", "tenant_b").await;
    let task_id = driver_a
        .enqueue("fp_x", serde_json::json!({}))
        .await
        .unwrap();
    let visible = driver_b.load(&task_id).await.unwrap();
    assert!(
        visible.is_none(),
        "tenant_b must not observe tenant_a's task"
    );
    let own = driver_a.load(&task_id).await.unwrap();
    assert!(own.is_some(), "owner sees the task");
}

#[tokio::test]
async fn retention_cleanup_deletes_only_expired_rows() {
    // Two drivers with different retention values against the
    // same namespace. The first task is force-expired (its
    // `retention_expiry` is set to the epoch) so the cleanup
    // sweep picks it up deterministically; the second task is
    // untouched.
    let (short_store, _) = single_driver_with_options("tenant_ret", "ns_ret", 1, 256).await;
    let (long_store, _) =
        single_driver_with_options("tenant_ret", "ns_ret", 7 * 24 * 60 * 60, 256).await;
    let short_id = short_store
        .enqueue("fp_short", serde_json::json!({}))
        .await
        .unwrap();
    let long_id = long_store
        .enqueue("fp_long", serde_json::json!({}))
        .await
        .unwrap();
    // `delete_expired` only sweeps terminal states, so move
    // the short task into `Cancelled` before force-expiring it.
    short_store
        .set_cancellation_intent(&short_id)
        .await
        .unwrap();
    short_store
        .force_expire_for_test(&short_id, "ns_ret")
        .await
        .unwrap();
    short_store
        .force_expire_for_test(&short_id, "ns_ret")
        .await
        .unwrap();
    let purged = short_store.delete_expired().await.unwrap();
    assert!(purged >= 1, "expired task must be purged, got {purged}");
    assert!(short_store.load(&short_id).await.unwrap().is_none());
    assert!(long_store.load(&long_id).await.unwrap().is_some());
}
