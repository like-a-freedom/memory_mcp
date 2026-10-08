//! Durable Task store integration suite (Task 7 of the
//! architecture audit remediation plan).
//!
//! Black-box coverage for one tenant's durable Task store, exercised
//! through the `DurableTaskTestDriver` exposed by `http::tasks`.
//! Every case uses a fresh in-memory namespace bound to a fresh
//! `PrivilegedEngine`; cross-handle visibility and cross-tenant denial
//! use two independent `BoundDbClient` handles against the same namespace.
//! These tests do not prove process-restart persistence.
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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use memory_mcp::http::registry::RegistryHandle;
use memory_mcp::http::tasks::DurableTaskTestDriver;
use memory_mcp::http::tasks::state::{TASK_KIND_EXTRACT, TaskState};
use memory_mcp::storage::{BoundDbClient, DbClient};

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

struct ArtifactReadMetrics {
    query_count: AtomicUsize,
    max_rows: AtomicUsize,
    page_query_count: AtomicUsize,
    fail_page_once_at: AtomicUsize,
    malformed_page_once: AtomicBool,
    pause_after_upper: AtomicBool,
    upper_bound_captured: tokio::sync::Notify,
    release_upper_bound: tokio::sync::Notify,
}

impl Default for ArtifactReadMetrics {
    fn default() -> Self {
        Self {
            query_count: AtomicUsize::new(0),
            max_rows: AtomicUsize::new(0),
            page_query_count: AtomicUsize::new(0),
            fail_page_once_at: AtomicUsize::new(0),
            malformed_page_once: AtomicBool::new(false),
            pause_after_upper: AtomicBool::new(false),
            upper_bound_captured: tokio::sync::Notify::new(),
            release_upper_bound: tokio::sync::Notify::new(),
        }
    }
}

struct ArtifactReadDb {
    inner: Arc<dyn DbClient>,
    metrics: Arc<ArtifactReadMetrics>,
}

#[async_trait::async_trait]
impl DbClient for ArtifactReadDb {
    async fn select_one(
        &self,
        record_id: &str,
        namespace: &str,
    ) -> Result<Option<serde_json::Value>, memory_mcp::MemoryError> {
        self.inner.select_one(record_id, namespace).await
    }

    async fn select_table(
        &self,
        table: memory_mcp::storage::table_scope::OwnedTable,
        namespace: &str,
    ) -> Result<Vec<serde_json::Value>, memory_mcp::MemoryError> {
        self.inner.select_table(table, namespace).await
    }

    async fn create(
        &self,
        record_id: &str,
        content: serde_json::Value,
        namespace: &str,
        temporal_fields: &[&str],
    ) -> Result<serde_json::Value, memory_mcp::MemoryError> {
        self.inner
            .create(record_id, content, namespace, temporal_fields)
            .await
    }

    async fn update(
        &self,
        record_id: &str,
        content: serde_json::Value,
        namespace: &str,
        temporal_fields: &[&str],
    ) -> Result<serde_json::Value, memory_mcp::MemoryError> {
        self.inner
            .update(record_id, content, namespace, temporal_fields)
            .await
    }

    async fn query(
        &self,
        sql: &str,
        vars: Option<serde_json::Value>,
        namespace: &str,
    ) -> Result<serde_json::Value, memory_mcp::MemoryError> {
        let mut result = self.inner.query(sql, vars, namespace).await?;
        if sql.trim_start().starts_with("SELECT")
            && sql.contains("FROM task_artifact")
            && !sql.contains("count()")
        {
            let rows: Vec<serde_json::Value> = serde_json::from_value(result.clone())
                .map_err(|error| memory_mcp::MemoryError::Storage(error.to_string()))?;
            self.metrics.query_count.fetch_add(1, Ordering::Relaxed);
            self.metrics
                .max_rows
                .fetch_max(rows.len(), Ordering::Relaxed);
            if sql.contains("LIMIT $limit") {
                let page = self
                    .metrics
                    .page_query_count
                    .fetch_add(1, Ordering::Relaxed)
                    + 1;
                if self
                    .metrics
                    .malformed_page_once
                    .swap(false, Ordering::SeqCst)
                {
                    let mut malformed_rows = rows;
                    if let Some(row) = malformed_rows
                        .first_mut()
                        .and_then(serde_json::Value::as_object_mut)
                    {
                        row.remove("id");
                    }
                    result = serde_json::Value::Array(malformed_rows);
                }
                if self
                    .metrics
                    .fail_page_once_at
                    .compare_exchange(page, 0, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
                {
                    return Err(memory_mcp::MemoryError::Transient(
                        "injected artifact page failure".to_string(),
                    ));
                }
            }
            if sql.contains("ORDER BY id DESC LIMIT 1")
                && self.metrics.pause_after_upper.swap(false, Ordering::SeqCst)
            {
                self.metrics.upper_bound_captured.notify_one();
                self.metrics.release_upper_bound.notified().await;
            }
        }
        Ok(result)
    }

    async fn apply_migrations(&self, namespace: &str) -> Result<(), memory_mcp::MemoryError> {
        self.inner.apply_migrations(namespace).await
    }
}

struct ArtifactHarness {
    driver_a: DurableTaskTestDriver,
    driver_b: DurableTaskTestDriver,
    db_a: Arc<dyn DbClient>,
    db_b: Arc<dyn DbClient>,
    read_metrics: Arc<ArtifactReadMetrics>,
    _registry: RegistryHandle,
}

async fn artifact_harness(namespace: &str) -> ArtifactHarness {
    let registry = RegistryHandle::in_memory_with_default_mem_engine().await;
    let db_a = registry
        .tenant_engine_optional()
        .expect("in-memory engine wired")
        .bind_to_test_namespace(namespace)
        .await;
    let db_b = registry
        .tenant_engine_optional()
        .expect("in-memory engine wired")
        .bind_to_test_namespace(namespace)
        .await;
    let read_metrics = Arc::new(ArtifactReadMetrics::default());
    let observed_db_a: Arc<dyn DbClient> = Arc::new(ArtifactReadDb {
        inner: db_a.clone(),
        metrics: read_metrics.clone(),
    });
    let driver_a = DurableTaskTestDriver::new(
        Arc::new(BoundDbClient::new(observed_db_a, namespace.to_owned())),
        "tenant-artifact-a".to_string(),
    );
    let driver_b = DurableTaskTestDriver::new(
        Arc::new(BoundDbClient::new(db_b.clone(), namespace.to_owned())),
        "tenant-artifact-b".to_string(),
    );
    driver_a
        .apply_migrations_for_test(namespace)
        .await
        .expect("apply tenant task migrations");
    db_a.query(
        include_str!("../migrations/044_task_artifacts.surql"),
        None,
        namespace,
    )
    .await
    .expect("apply task artifact migration");

    ArtifactHarness {
        driver_a,
        driver_b,
        db_a,
        db_b,
        read_metrics,
        _registry: registry,
    }
}

async fn seed_committed_artifact(
    db: &dyn DbClient,
    namespace: &str,
    tenant_id: &str,
    artifact_key: &str,
    task_id: &str,
) -> Result<(), memory_mcp::MemoryError> {
    db.query(
        "CREATE type::record('task_artifact', $artifact_key) SET \
         task_id = $task_id, tenant_id = $tenant_id, fingerprint = $fingerprint, \
         episode_id = $episode_id, fact_ids = $fact_ids, state = 'committed', \
         created_at = time::now(), completed_at = time::now()",
        Some(serde_json::json!({
            "artifact_key": artifact_key,
            "task_id": task_id,
            "tenant_id": tenant_id,
            "fingerprint": format!("fixture:{artifact_key}"),
            "episode_id": format!("episode:{artifact_key}"),
            "fact_ids": [format!("fact:{artifact_key}")],
        })),
        namespace,
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn reconcile_pending_artifacts_across_pages_without_cross_tenant_changes()
-> Result<(), Box<dyn std::error::Error>> {
    let harness = artifact_harness("ns_artifact_pages").await;
    let namespace = "ns_artifact_pages";

    seed_committed_artifact(
        harness.db_a.as_ref(),
        namespace,
        "tenant-artifact-a",
        "artifact-000",
        "missing-orphan-000",
    )
    .await?;
    seed_committed_artifact(
        harness.db_a.as_ref(),
        namespace,
        "tenant-artifact-a",
        "artifact-001",
        "missing-orphan-001",
    )
    .await?;

    let mut terminal_ids = Vec::new();
    for index in 0..2 {
        let task_id = harness
            .driver_a
            .enqueue(
                TASK_KIND_EXTRACT,
                &format!("terminal-fingerprint-{index}"),
                serde_json::json!({}),
            )
            .await?;
        let handle = harness
            .driver_a
            .claim_next_due("artifact-reconcile-test")
            .await?
            .expect("terminal fixture task is due");
        harness
            .driver_a
            .complete_fenced(&handle, serde_json::json!({"done": true}), false)
            .await?;
        let version = harness
            .driver_a
            .load(&task_id)
            .await?
            .expect("completed fixture task exists")
            .version;
        let artifact_key = format!("artifact-{:03}", index + 2);
        seed_committed_artifact(
            harness.db_a.as_ref(),
            namespace,
            "tenant-artifact-a",
            &artifact_key,
            &task_id,
        )
        .await?;
        terminal_ids.push((task_id, version));
    }

    let mut pending_ids = Vec::new();
    for index in 0..125 {
        let task_id = harness
            .driver_a
            .enqueue(
                TASK_KIND_EXTRACT,
                &format!("pending-fingerprint-{index}"),
                serde_json::json!({}),
            )
            .await?;
        let artifact_key = format!("artifact-{:03}", index + 4);
        seed_committed_artifact(
            harness.db_a.as_ref(),
            namespace,
            "tenant-artifact-a",
            &artifact_key,
            &task_id,
        )
        .await?;
        pending_ids.push(task_id);
    }

    let foreign_task_id = harness
        .driver_b
        .enqueue(
            TASK_KIND_EXTRACT,
            "foreign-pending-fingerprint",
            serde_json::json!({}),
        )
        .await?;
    seed_committed_artifact(
        harness.db_b.as_ref(),
        namespace,
        "tenant-artifact-b",
        "artifact-foreign",
        &foreign_task_id,
    )
    .await?;

    assert_eq!(harness.driver_a.count_committed_artifacts().await?, 129);
    assert_eq!(harness.driver_b.count_committed_artifacts().await?, 1);
    let reconciled = harness.driver_a.reconcile_artifacts().await?;
    assert_eq!(reconciled, 125);
    assert!(harness.read_metrics.query_count.load(Ordering::Relaxed) > 1);
    assert!(harness.read_metrics.max_rows.load(Ordering::Relaxed) <= 64);

    for task_id in pending_ids {
        let task = harness
            .driver_a
            .load(&task_id)
            .await?
            .expect("pending task exists after reconciliation");
        assert_eq!(task.state, TaskState::Completed);
        assert_eq!(task.version, 2);
    }
    for (task_id, original_version) in terminal_ids {
        let task = harness
            .driver_a
            .load(&task_id)
            .await?
            .expect("terminal task remains present");
        assert_eq!(task.state, TaskState::Completed);
        assert_eq!(task.version, original_version);
    }
    let foreign_task = harness
        .driver_b
        .load(&foreign_task_id)
        .await?
        .expect("other tenant task remains present");
    assert_eq!(foreign_task.state, TaskState::Queued);
    assert_eq!(foreign_task.version, 1);
    assert_eq!(harness.driver_a.count_committed_artifacts().await?, 129);
    assert_eq!(harness.driver_b.count_committed_artifacts().await?, 1);
    Ok(())
}

#[tokio::test]
async fn reconcile_exact_page_boundary_once() -> Result<(), Box<dyn std::error::Error>> {
    let harness = artifact_harness("ns_artifact_boundary").await;
    let namespace = "ns_artifact_boundary";
    let mut task_ids = Vec::new();
    for index in 0..64 {
        let task_id = harness
            .driver_a
            .enqueue(
                TASK_KIND_EXTRACT,
                &format!("boundary-fingerprint-{index}"),
                serde_json::json!({}),
            )
            .await?;
        seed_committed_artifact(
            harness.db_a.as_ref(),
            namespace,
            "tenant-artifact-a",
            &format!("boundary-{index:03}"),
            &task_id,
        )
        .await?;
        task_ids.push(task_id);
    }

    assert_eq!(harness.driver_a.reconcile_artifacts().await?, 64);
    for task_id in task_ids {
        assert_eq!(
            harness
                .driver_a
                .load(&task_id)
                .await?
                .expect("task exists")
                .state,
            TaskState::Completed
        );
    }
    assert_eq!(harness.driver_a.reconcile_artifacts().await?, 0);
    Ok(())
}

#[tokio::test]
async fn repeated_pass_does_not_increment_task_versions() -> Result<(), Box<dyn std::error::Error>>
{
    let harness = artifact_harness("ns_artifact_repeat").await;
    let namespace = "ns_artifact_repeat";
    let task_id = harness
        .driver_a
        .enqueue(
            TASK_KIND_EXTRACT,
            "repeat-fingerprint",
            serde_json::json!({}),
        )
        .await?;
    seed_committed_artifact(
        harness.db_a.as_ref(),
        namespace,
        "tenant-artifact-a",
        "repeat-artifact",
        &task_id,
    )
    .await?;

    assert_eq!(harness.driver_a.reconcile_artifacts().await?, 1);
    let completed = harness
        .driver_a
        .load(&task_id)
        .await?
        .expect("task completed after first pass");
    assert_eq!(completed.state, TaskState::Completed);
    assert_eq!(completed.version, 2);

    assert_eq!(harness.driver_a.reconcile_artifacts().await?, 0);
    let after_repeat = harness
        .driver_a
        .load(&task_id)
        .await?
        .expect("task remains present after repeated pass");
    assert_eq!(after_repeat.state, TaskState::Completed);
    assert_eq!(after_repeat.version, completed.version);
    assert_eq!(harness.driver_a.count_committed_artifacts().await?, 1);
    Ok(())
}

#[tokio::test]
async fn expired_task_artifacts_remain_audit_records() -> Result<(), Box<dyn std::error::Error>> {
    let harness = artifact_harness("ns_artifact_expired").await;
    let namespace = "ns_artifact_expired";
    let task_id = harness
        .driver_a
        .enqueue(
            TASK_KIND_EXTRACT,
            "expired-artifact-fingerprint",
            serde_json::json!({}),
        )
        .await?;
    let handle = harness
        .driver_a
        .claim_next_due("artifact-expired-test")
        .await?
        .expect("task is due");
    harness
        .driver_a
        .complete_fenced(&handle, serde_json::json!({"done": true}), false)
        .await?;
    seed_committed_artifact(
        harness.db_a.as_ref(),
        namespace,
        "tenant-artifact-a",
        "expired-artifact",
        &task_id,
    )
    .await?;
    harness
        .driver_a
        .force_expire_for_test(&task_id, namespace)
        .await?;

    assert_eq!(harness.driver_a.delete_expired().await?, 1);
    assert!(harness.driver_a.load(&task_id).await?.is_none());
    assert_eq!(harness.driver_a.count_committed_artifacts().await?, 1);
    assert_eq!(harness.driver_a.reconcile_artifacts().await?, 0);
    assert_eq!(harness.driver_a.count_committed_artifacts().await?, 1);
    Ok(())
}

#[tokio::test]
async fn later_pass_after_page_failure_reconciles_all_pending_rows()
-> Result<(), Box<dyn std::error::Error>> {
    let harness = artifact_harness("ns_artifact_page_failure").await;
    let namespace = "ns_artifact_page_failure";
    let mut task_ids = Vec::new();
    for index in 0..65 {
        let task_id = harness
            .driver_a
            .enqueue(
                TASK_KIND_EXTRACT,
                &format!("page-failure-fingerprint-{index}"),
                serde_json::json!({}),
            )
            .await?;
        seed_committed_artifact(
            harness.db_a.as_ref(),
            namespace,
            "tenant-artifact-a",
            &format!("page-failure-{index:03}"),
            &task_id,
        )
        .await?;
        task_ids.push(task_id);
    }
    harness
        .read_metrics
        .fail_page_once_at
        .store(2, Ordering::Relaxed);

    let first_pass = harness.driver_a.reconcile_artifacts().await;
    assert!(matches!(
        first_pass,
        Err(memory_mcp::MemoryError::Transient(_))
    ));
    for task_id in task_ids.iter().take(64) {
        assert_eq!(
            harness
                .driver_a
                .load(task_id)
                .await?
                .expect("first page task exists")
                .state,
            TaskState::Completed
        );
    }
    assert_eq!(
        harness
            .driver_a
            .load(task_ids.last().expect("65 tasks were seeded"))
            .await?
            .expect("last-page task exists")
            .state,
        TaskState::Queued
    );

    assert_eq!(harness.driver_a.reconcile_artifacts().await?, 1);
    for task_id in task_ids {
        assert_eq!(
            harness
                .driver_a
                .load(&task_id)
                .await?
                .expect("task exists after retry")
                .state,
            TaskState::Completed
        );
    }
    assert_eq!(harness.driver_a.count_committed_artifacts().await?, 65);
    Ok(())
}

#[tokio::test]
async fn malformed_or_nonadvancing_cursor_returns_error() -> Result<(), Box<dyn std::error::Error>>
{
    let harness = artifact_harness("ns_artifact_malformed_cursor").await;
    let namespace = "ns_artifact_malformed_cursor";
    let task_id = harness
        .driver_a
        .enqueue(
            TASK_KIND_EXTRACT,
            "malformed-cursor-fingerprint",
            serde_json::json!({}),
        )
        .await?;
    seed_committed_artifact(
        harness.db_a.as_ref(),
        namespace,
        "tenant-artifact-a",
        "malformed-cursor-artifact",
        &task_id,
    )
    .await?;
    harness
        .read_metrics
        .malformed_page_once
        .store(true, Ordering::SeqCst);

    let error = harness
        .driver_a
        .reconcile_artifacts()
        .await
        .expect_err("a page without a record cursor cannot be advanced safely");

    assert!(matches!(
        error,
        memory_mcp::MemoryError::Storage(message)
            if message.contains("task artifact page has no record id")
    ));
    let task = harness
        .driver_a
        .load(&task_id)
        .await?
        .expect("pending task remains present after malformed page");
    assert_eq!(task.state, TaskState::Queued);
    assert_eq!(harness.driver_a.count_committed_artifacts().await?, 1);
    Ok(())
}

#[tokio::test]
async fn insert_after_upper_key_is_seen_on_next_pass() -> Result<(), Box<dyn std::error::Error>> {
    let harness = artifact_harness("ns_artifact_upper").await;
    let namespace = "ns_artifact_upper";
    let driver = Arc::new(harness.driver_a);
    let original_task_id = driver
        .enqueue(
            TASK_KIND_EXTRACT,
            "upper-bound-original",
            serde_json::json!({}),
        )
        .await?;
    seed_committed_artifact(
        harness.db_a.as_ref(),
        namespace,
        "tenant-artifact-a",
        "artifact-old",
        &original_task_id,
    )
    .await?;

    harness
        .read_metrics
        .pause_after_upper
        .store(true, Ordering::SeqCst);
    let reconcile_driver = driver.clone();
    let reconciliation = tokio::spawn(async move { reconcile_driver.reconcile_artifacts().await });
    let captured_upper = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        harness.read_metrics.upper_bound_captured.notified(),
    )
    .await;
    if captured_upper.is_err() {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), reconciliation).await;
        panic!("reconciliation must capture a stable upper record id before paging");
    }

    let inserted_task_id = driver
        .enqueue(
            TASK_KIND_EXTRACT,
            "upper-bound-inserted-after-snapshot",
            serde_json::json!({}),
        )
        .await?;
    seed_committed_artifact(
        harness.db_a.as_ref(),
        namespace,
        "tenant-artifact-a",
        "artifact-zz-new",
        &inserted_task_id,
    )
    .await?;
    harness.read_metrics.release_upper_bound.notify_one();

    let reconciled = tokio::time::timeout(std::time::Duration::from_secs(5), reconciliation)
        .await
        .expect("first reconciliation pass should finish")
        .expect("first reconciliation task should not panic")?;
    assert_eq!(reconciled, 1);
    assert_eq!(
        driver
            .load(&inserted_task_id)
            .await?
            .expect("inserted task remains present")
            .state,
        TaskState::Queued
    );

    assert_eq!(driver.reconcile_artifacts().await?, 1);
    assert_eq!(
        driver
            .load(&inserted_task_id)
            .await?
            .expect("inserted task remains present")
            .state,
        TaskState::Completed
    );
    Ok(())
}

#[tokio::test]
async fn enqueue_dedupes_by_fingerprint() {
    let h = two_handles("tenant_a", "ns_dedup").await;
    let fp = "fingerprint_dedup";
    let first = h
        .driver_a
        .enqueue(TASK_KIND_EXTRACT, fp, serde_json::json!({"a": 1}))
        .await
        .unwrap();
    let second = h
        .driver_a
        .enqueue(TASK_KIND_EXTRACT, fp, serde_json::json!({"a": 2}))
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
        .enqueue(TASK_KIND_EXTRACT, "fp_capacity_1", serde_json::json!({}))
        .await
        .expect("first enqueue");
    let err = store
        .enqueue(TASK_KIND_EXTRACT, "fp_capacity_2", serde_json::json!({}))
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
        .enqueue(
            TASK_KIND_EXTRACT,
            "fp_lifecycle",
            serde_json::json!({"x": 1}),
        )
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
        .enqueue(TASK_KIND_EXTRACT, "fp_stale", serde_json::json!({}))
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
        .enqueue(TASK_KIND_EXTRACT, "fp_cancel", serde_json::json!({}))
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
        .enqueue(TASK_KIND_EXTRACT, "fp_cbc", serde_json::json!({}))
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
        .enqueue(TASK_KIND_EXTRACT, "fp_x", serde_json::json!({}))
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
        .enqueue(TASK_KIND_EXTRACT, "fp_short", serde_json::json!({}))
        .await
        .unwrap();
    let long_id = long_store
        .enqueue(TASK_KIND_EXTRACT, "fp_long", serde_json::json!({}))
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
