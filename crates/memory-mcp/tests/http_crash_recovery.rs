//! Deterministic HTTP crash-and-recovery gates (ADR-0053, Task 6).
//!
//! Every test exercises a named [`FaultPoint`] by spinning up the
//! durable scheduler + deletion + outbox code paths with a
//! `FailOnceAt` injector attached. The named transient fires
//! after the durable transition the recovery suite cares
//! about; the next worker advances the partial state forward
//! and the test asserts the expected terminal state via a
//! fresh `SurrealRegistryStore` against the same rocksdb
//! directory.
//!
//! Run:
//!
//! ```bash
//! cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures \
//!     --test http_crash_recovery -- --test-threads=1
//! ```

#![cfg(all(
    feature = "streamable-http",
    feature = "control-plane",
    feature = "test-fixtures"
))]

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::http_server::{HttpServerConfig, HttpServerFixture, TestTenant, mcp_call};
use memory_mcp::config::SurrealTargetConfig;
use memory_mcp::error::MemoryError;
use memory_mcp::http::composition::HttpProductionComposition;
use memory_mcp::http::config::HttpConfig;
use memory_mcp::http::fault_injection::{FailOnceAt, FaultInjector, FaultPoint, NoFaults};
use memory_mcp::http::leases::migration::{
    CURRENT_SCHEMA_VERSION, SurrealTenantMigrations, run_due_provisioning,
};
use memory_mcp::http::principal::api_keys::ApiKeyCredential;
use memory_mcp::http::registry::RegistryHandle;
use memory_mcp::http::registry::models::{
    Account, AccountStatus, ApiKey, ApiKeyStatus, KeyedVerifier, NamespaceBinding, Tenant,
    TenantStatus,
};
#[allow(unused_imports)]
use memory_mcp::http::registry::storage::RegistryStore;
use memory_mcp::http::tasks::scheduler::{ExtractorFn, execute_one_task_for_test};
use memory_mcp::http::tasks::state::TaskStore as _;
use memory_mcp::http::tasks::worker::DurableTaskStore;
use memory_mcp::storage::{BoundDbClient, SurrealDbClient};
use memory_mcp::tools::params::ExtractParams;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const UNIT_KEY_A: &str =
    "mem_sk_ak_aaaa0000-0000-4000-8000-000000000000_crashrecover000000000000000000000000aaaa";
const UNIT_KEY_B: &str =
    "mem_sk_ak_bbbb0000-0000-4000-8000-000000000000_crashrecover000000000000000000000000bbbb";

fn tenant_id_for(name: &str) -> String {
    let digest = Sha256::digest(name.as_bytes());
    let suffix = hex::encode(&digest[..8]);
    format!("ten_test_{suffix}")
}

fn account_id_for(name: &str) -> String {
    let digest = Sha256::digest(name.as_bytes());
    let suffix = hex::encode(&digest[..8]);
    format!("acct_test_{suffix}")
}

async fn open_registry_at(dir: &TempDir) -> RegistryHandle {
    let target = SurrealTargetConfig {
        url: format!("rocksdb://{}", dir.path().display()),
        username: "root".into(),
        password: "root".into(),
        database: "control".into(),
        namespace: "control".into(),
    };
    HttpProductionComposition::connect(&HttpConfig {
        control_db: target.clone(),
        tenant_db: target,
        ..HttpConfig::default_for_test()
    })
    .await
    .expect("durable composition connect")
    .registry
}

async fn seed_reserved_tenant_with_key(registry: RegistryHandle, name: &str, api_key: &str) {
    let cred = ApiKeyCredential::parse(api_key).expect("api key parse");
    let suffix = hex::encode(&Sha256::digest(name.as_bytes())[..8]);
    let account_id = account_id_for(name);
    let tenant_id = tenant_id_for(name);
    let tenant_namespace = format!("tns_test_{suffix}");
    let now = chrono::Utc::now();
    let store = registry.store_clone();
    store
        .write_account(&Account {
            id: account_id.clone(),
            status: AccountStatus::Active,
            tenant_id: tenant_id.clone(),
            created_at: now,
        })
        .await
        .expect("write account");
    store
        .write_tenant(&Tenant {
            id: tenant_id.clone(),
            status: TenantStatus::Reserved,
            namespace_binding: NamespaceBinding {
                namespace: tenant_namespace,
                database: "memory".into(),
            },
            plan_version: 1,
            schema_version: CURRENT_SCHEMA_VERSION.saturating_sub(1),
            retry_stage: None,
            provisioning_lease: None,
            created_at: now,
            version: 0,
        })
        .await
        .expect("write tenant");
    store
        .write_api_key(&ApiKey {
            id: cred.key_id().to_string(),
            account_id: account_id.clone(),
            name: format!("crash-test-{name}"),
            verifier: KeyedVerifier::compute(b"test-pepper-padding-padding-pad", cred.secret()),
            status: ApiKeyStatus::Active,
            created_at: now,
            expires_at: None,
            last_used_at: None,
            version: 0,
        })
        .await
        .expect("write api key");
}

async fn seed_reserved_tenant(registry: RegistryHandle, name: &str) {
    seed_reserved_tenant_with_key(registry, name, UNIT_KEY_A).await;
}

/// Test-only seam: write a tenant directly in `Ready` state. Used
/// by the deletion-recovery test to populate a sibling tenant
/// without re-running the provisioning scheduler. The sibling
/// is never opened via the data plane; the test only verifies
/// the registry row's status and `schema_version` remain
/// unchanged after the targeted deletion.
async fn seed_ready_sibling(registry: RegistryHandle, name: &str, api_key: &str) {
    let cred = ApiKeyCredential::parse(api_key).expect("api key parse");
    let suffix = hex::encode(&Sha256::digest(name.as_bytes())[..8]);
    let account_id = account_id_for(name);
    let tenant_id = tenant_id_for(name);
    let tenant_namespace = format!("tns_test_{suffix}");
    let now = chrono::Utc::now();
    let store = registry.store_clone();
    store
        .write_account(&Account {
            id: account_id.clone(),
            status: AccountStatus::Active,
            tenant_id: tenant_id.clone(),
            created_at: now,
        })
        .await
        .expect("write sibling account");
    store
        .write_tenant(&Tenant {
            id: tenant_id.clone(),
            status: TenantStatus::Ready,
            namespace_binding: NamespaceBinding {
                namespace: tenant_namespace,
                database: "memory".into(),
            },
            plan_version: 1,
            schema_version: CURRENT_SCHEMA_VERSION,
            retry_stage: None,
            provisioning_lease: None,
            created_at: now,
            version: 0,
        })
        .await
        .expect("write sibling tenant");
    store
        .write_api_key(&ApiKey {
            id: cred.key_id().to_string(),
            account_id: account_id.clone(),
            name: format!("crash-test-{name}"),
            verifier: KeyedVerifier::compute(b"test-pepper-padding-padding-pad", cred.secret()),
            status: ApiKeyStatus::Active,
            created_at: now,
            expires_at: None,
            last_used_at: None,
            version: 0,
        })
        .await
        .expect("write sibling api key");
}

async fn tick_provisioning_with_injector(
    registry: RegistryHandle,
    injector: Arc<dyn FaultInjector>,
) {
    let migrations = Arc::new(SurrealTenantMigrations::new(
        registry.tenant_engine().expect("tenant engine"),
    ));
    let _ = run_due_provisioning(registry, migrations, injector).await;
}

async fn tick_provisioning_with_injector_and_ttl(
    registry: RegistryHandle,
    injector: Arc<dyn FaultInjector>,
    lease_ttl_secs: i64,
) {
    use memory_mcp::http::leases::migration::run_due_provisioning_for;
    let store = registry.store_clone();
    let migrations = Arc::new(SurrealTenantMigrations::new(
        registry.tenant_engine().expect("tenant engine"),
    ));
    let _ = run_due_provisioning_for(
        registry,
        store,
        migrations,
        injector,
        100,
        chrono::Utc::now(),
        lease_ttl_secs,
    )
    .await;
}

// ---------------------------------------------------------------------------
// Provisioning recovery — drive the durable scheduler in-process
// ---------------------------------------------------------------------------

async fn run_provisioning_recovery_in_process(label: &str, fault: FaultPoint) {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = open_registry_at(&dir).await;
    seed_reserved_tenant(registry.clone(), label).await;
    let tenant_id = tenant_id_for(label);
    let injector = Arc::new(FailOnceAt::new(fault));
    let injector_dyn: Arc<dyn FaultInjector> = injector.clone();
    let store = registry.store_clone();
    let now = chrono::Utc::now();
    // `TenantReadyCommitted` fires AFTER the Ready transition
    // is committed, so the tenant reaches Ready on the very
    // first scheduler tick; the others leave the tenant in
    // Migrating (lease released) so the next tick advances
    // it.
    let mut fault_observed = false;
    for _ in 0..400 {
        tick_provisioning_with_injector(registry.clone(), injector_dyn.clone()).await;
        if injector.consumed() == 1 && !fault_observed {
            fault_observed = true;
        }
        let tenant = store
            .find_tenant_by_id(&tenant_id)
            .await
            .expect("tenant lookup")
            .expect("tenant present");
        if tenant.status == TenantStatus::Ready {
            assert_eq!(tenant.schema_version, CURRENT_SCHEMA_VERSION);
            assert!(
                fault_observed,
                "{label}: injector must have fired the simulated transient"
            );
            assert_eq!(
                injector.consumed(),
                1,
                "{label}: FailOnceAt must have fired exactly once"
            );
            return;
        }
        if chrono::Utc::now() - now > chrono::Duration::seconds(20) {
            panic!(
                "{label}: recovery did not converge; tenant status={:?}",
                tenant.status
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{label}: too many scheduler ticks");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn provisioning_recovers_after_lease_claim_fault() {
    // The ProvisioningLeaseClaimed fault fires after the
    // lease is durable; the next worker cannot re-claim
    // until the 60s lease expires. Shorten the lease so
    // the test converges in finite time.
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = open_registry_at(&dir).await;
    seed_reserved_tenant(registry.clone(), "recovery_lease").await;
    let tenant_id = tenant_id_for("recovery_lease");
    let injector = Arc::new(FailOnceAt::new(FaultPoint::ProvisioningLeaseClaimed));
    let injector_dyn: Arc<dyn FaultInjector> = injector.clone();
    let store = registry.store_clone();
    let started = Instant::now();
    let mut fault_seen = false;
    // Use a 2-second TTL so the recovery tick re-claims
    // quickly. The fault fires on the first tick; the
    // scheduler then skips the tenant until the lease
    // expires; the next tick re-claims and advances to
    // Ready.
    for _ in 0..120 {
        tick_provisioning_with_injector_and_ttl(registry.clone(), injector_dyn.clone(), 2).await;
        let tenant = store
            .find_tenant_by_id(&tenant_id)
            .await
            .expect("tenant lookup")
            .expect("tenant present");
        if tenant.status == TenantStatus::Ready {
            assert_eq!(tenant.schema_version, CURRENT_SCHEMA_VERSION);
            assert!(
                fault_seen,
                "recovery_lease: must observe at least one faulted tick"
            );
            assert_eq!(injector.consumed(), 1);
            return;
        }
        fault_seen = true;
        if started.elapsed() > Duration::from_secs(20) {
            panic!(
                "recovery_lease: did not converge; tenant status={:?}",
                tenant.status
            );
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    panic!("recovery_lease: too many scheduler ticks");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn provisioning_recovers_after_namespace_created_fault() {
    run_provisioning_recovery_in_process("recovery_namespace", FaultPoint::NamespaceCreated).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn provisioning_recovers_after_migrations_applied_fault() {
    run_provisioning_recovery_in_process(
        "recovery_migrations",
        FaultPoint::TenantMigrationsApplied,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn provisioning_recovers_after_ready_committed_fault() {
    run_provisioning_recovery_in_process("recovery_ready", FaultPoint::TenantReadyCommitted).await;
}

// ---------------------------------------------------------------------------
// Outbox recovery
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbox_recovery_after_first_mutation_transient() {
    use memory_mcp::http::subscriptions::outbox::{
        TenantChangeEvent, TenantMutation, commit_tenant_mutation_with_event,
    };
    use memory_mcp::storage::DbClient;
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = open_registry_at(&dir).await;
    seed_reserved_tenant(registry.clone(), "outbox_recovery").await;
    let tenant_id = tenant_id_for("outbox_recovery");
    let injector_noop: Arc<dyn FaultInjector> = Arc::new(NoFaults);
    let store = registry.store_clone();
    let now = chrono::Utc::now();
    let mut converged = false;
    for _ in 0..400 {
        tick_provisioning_with_injector(registry.clone(), injector_noop.clone()).await;
        let t = store
            .find_tenant_by_id(&tenant_id)
            .await
            .expect("tenant lookup")
            .expect("tenant present");
        if t.status == TenantStatus::Ready {
            converged = true;
            break;
        }
        if chrono::Utc::now() - now > chrono::Duration::seconds(20) {
            panic!("outbox_recovery: provisioning did not converge");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(converged, "tenant must reach Ready before outbox commit");
    let tenant = store
        .find_tenant_by_id(&tenant_id)
        .await
        .expect("tenant lookup")
        .expect("tenant present");
    let engine = registry.tenant_engine().expect("tenant engine");
    let db = engine.bind(&tenant).await.expect("tenant bind");
    let bound = memory_mcp::storage::BoundDbClient::new(
        db.clone(),
        tenant.namespace_binding.namespace.clone(),
    );
    let fault_injector = Arc::new(FailOnceAt::new(FaultPoint::OutboxMutationCommitted));
    let fault_injector_dyn: Arc<dyn FaultInjector> = fault_injector.clone();
    let event = TenantChangeEvent {
        sequence: 0,
        resource_id: "ui://memory/apps/inspector".into(),
        revision: 1,
        change_kind: "fact_created".into(),
        created_at: chrono::Utc::now(),
    };
    // Inline DEFINE for a transient table that the outbox commit
    // path can use without colliding with the rich fact schema.
    let _ = db
        .query(
            "DEFINE TABLE outbox_recovery_test SCHEMALESS",
            None,
            &tenant.namespace_binding.namespace,
        )
        .await;
    let mutation = TenantMutation::new(
        "CREATE outbox_recovery_test:recovery SET body = 'hello'",
        serde_json::json!({}),
    )
    .expect("mutation");
    let first =
        commit_tenant_mutation_with_event(&bound, mutation, event, &fault_injector_dyn).await;
    assert!(
        matches!(first, Err(MemoryError::Transient(_))),
        "first commit must return a transient, got: {first:?}"
    );
    let event2 = TenantChangeEvent {
        sequence: 0,
        resource_id: "ui://memory/apps/inspector".into(),
        revision: 2,
        change_kind: "fact_created".into(),
        created_at: chrono::Utc::now(),
    };
    let mutation2 = TenantMutation::new(
        "CREATE outbox_recovery_test:recovery_2 SET body = 'world'",
        serde_json::json!({}),
    )
    .expect("mutation");
    commit_tenant_mutation_with_event(&bound, mutation2, event2, &fault_injector_dyn)
        .await
        .expect("second commit");
    let db2 = engine.bind(&tenant).await.expect("tenant bind");
    let seq_result = db2
        .query(
            "SELECT VALUE value FROM tenant_change_sequence:default LIMIT 1",
            None,
            &tenant.namespace_binding.namespace,
        )
        .await
        .expect("seq query");
    let seq: Vec<serde_json::Value> = serde_json::from_value(seq_result).expect("parse seq");
    let delivered = seq.first().and_then(|v| v.as_u64()).unwrap_or(0);
    let events_result = db2
        .query(
            "SELECT event_seq FROM tenant_change_event ORDER BY event_seq",
            None,
            &tenant.namespace_binding.namespace,
        )
        .await
        .expect("events query");
    let events: Vec<serde_json::Value> =
        serde_json::from_value(events_result).expect("parse events");
    let committed = events.len() as u64;
    assert_eq!(
        delivered, committed,
        "outbox sequence must match committed event count after recovery"
    );
    assert_eq!(
        fault_injector.consumed(),
        1,
        "FailOnceAt should have fired exactly once"
    );
}

// ---------------------------------------------------------------------------
// Deletion recovery
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deletion_recovers_after_finalize_transient() {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = open_registry_at(&dir).await;
    seed_reserved_tenant(registry.clone(), "deletion_recovery").await;
    let tenant_id = tenant_id_for("deletion_recovery");
    let injector_noop: Arc<dyn FaultInjector> = Arc::new(NoFaults);
    let store = registry.store_clone();
    let now = chrono::Utc::now();
    let mut converged = false;
    for _ in 0..400 {
        tick_provisioning_with_injector(registry.clone(), injector_noop.clone()).await;
        let t = store
            .find_tenant_by_id(&tenant_id)
            .await
            .expect("tenant lookup")
            .expect("tenant present");
        if t.status == TenantStatus::Ready {
            converged = true;
            break;
        }
        if chrono::Utc::now() - now > chrono::Duration::seconds(20) {
            panic!("deletion_recovery: provisioning did not converge");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(converged);
    // Seed a sibling tenant directly as `Ready` so the
    // deletion worker is exercised against a populated control
    // registry. The plan's recovery contract asserts the
    // sibling's state is unchanged after the transient.
    // Seeding the sibling post-provisioning avoids re-running
    // the scheduler with two Reserved tenants in the same
    // database, which surfaces a latent `query_json`
    // multi-row limitation that is out of scope for Task 6.
    seed_ready_sibling(registry.clone(), "deletion_recovery_sibling", UNIT_KEY_B).await;
    let sibling_id = tenant_id_for("deletion_recovery_sibling");
    let sibling_before = store
        .find_tenant_by_id(&sibling_id)
        .await
        .expect("sibling lookup")
        .expect("sibling present");
    assert_eq!(sibling_before.status, TenantStatus::Ready);
    let sibling_schema_before = sibling_before.schema_version;
    store
        .begin_operator_deletion(&tenant_id, "test", chrono::Utc::now())
        .await
        .expect("begin operator deletion");
    let fault_injector = Arc::new(FailOnceAt::new(FaultPoint::AccountDeletionFinalized));
    memory_mcp::control::deletion::run_deletion_worker(registry.clone(), fault_injector.clone())
        .await
        .expect("a durably committed deletion is reported as success");
    let after_first = store
        .find_tenant_by_id(&tenant_id)
        .await
        .expect("tenant lookup")
        .expect("tenant present");
    assert_eq!(after_first.status, TenantStatus::Purged);
    memory_mcp::control::deletion::run_deletion_worker(registry.clone(), Arc::new(NoFaults))
        .await
        .expect("deletion worker retries");
    let after_second = store
        .find_tenant_by_id(&tenant_id)
        .await
        .expect("tenant lookup")
        .expect("tenant present");
    assert_eq!(after_second.status, TenantStatus::Purged);
    let sibling_after = store
        .find_tenant_by_id(&sibling_id)
        .await
        .expect("sibling lookup")
        .expect("sibling present");
    // The sibling tenant must remain in `Ready` with the same
    // schema version; the deletion worker is scoped to the
    // targeted tenant only.
    assert_eq!(sibling_after.status, TenantStatus::Ready);
    assert_eq!(sibling_after.schema_version, sibling_schema_before);
    assert_eq!(fault_injector.consumed(), 1);
}

// ---------------------------------------------------------------------------
// Task recovery — drive the durable task worker in-process with a
// stub extractor. The hit points (TaskClaimed,
// TaskArtifactCommitted, TaskCompleted) live in
// `tasks::scheduler::execute_one_task`; the recovery contract is
// that after a Transient at any of those three points, the next
// worker pass advances the durable state to exactly one completed
// task with exactly one committed artifact.
// ---------------------------------------------------------------------------

/// Test-only stub extractor: returns a stable, deterministic
/// `ToolResponse<ExtractResult>` JSON value so the recovery tests
/// can exercise the durable state machine without a local GLiNER
/// checkpoint. The shape matches what `record_artifact_fenced`
/// expects (`{ "result": { "episode_id": ..., "facts": [...] } }`).
fn stub_extractor() -> ExtractorFn {
    Arc::new(|_params: ExtractParams| {
        Box::pin(async move {
            Ok(serde_json::json!({
                "result": {
                    "episode_id": "episode:stub",
                    "facts": [],
                    "entities": [],
                    "links": [],
                    "warnings": []
                },
                "guidance": null
            }))
        })
    })
}

/// Run the provisioning scheduler with `NoFaults` until the tenant
/// reaches `Ready`. Returns the registry handle, the ready tenant,
/// and the `SurrealDbClient` bound to the tenant namespace.
async fn bring_tenant_to_ready(
    registry: RegistryHandle,
    name: &str,
) -> (
    Tenant,
    std::sync::Arc<SurrealDbClient>,
    Arc<BoundDbClient>,
    DurableTaskStore,
) {
    seed_reserved_tenant(registry.clone(), name).await;
    let tenant_id = tenant_id_for(name);
    let injector_noop: Arc<dyn FaultInjector> = Arc::new(NoFaults);
    let store = registry.store_clone();
    let now = chrono::Utc::now();
    let mut converged = false;
    for _ in 0..400 {
        tick_provisioning_with_injector(registry.clone(), injector_noop.clone()).await;
        let t = store
            .find_tenant_by_id(&tenant_id)
            .await
            .expect("tenant lookup")
            .expect("tenant present");
        if t.status == TenantStatus::Ready {
            converged = true;
            break;
        }
        if chrono::Utc::now() - now > chrono::Duration::seconds(20) {
            panic!("{name}: provisioning did not converge");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(converged, "{name}: tenant must reach Ready");
    let tenant = store
        .find_tenant_by_id(&tenant_id)
        .await
        .expect("tenant lookup")
        .expect("tenant present");
    let engine = registry.tenant_engine().expect("tenant engine");
    let db = engine.bind(&tenant).await.expect("tenant bind");
    let bound = Arc::new(BoundDbClient::new(
        db.clone(),
        tenant.namespace_binding.namespace.clone(),
    ));
    let task_store = DurableTaskStore::new(bound.clone(), tenant.id.clone());
    (tenant, db, bound, task_store)
}

/// Shared body for the three task fault-point tests. Drives
/// `execute_one_task_for_test` against the same store and
/// `FailOnceAt` twice: the first call must return the simulated
/// Transient, the second call must complete the durable state
/// machine end-to-end.
async fn run_task_recovery(registry: RegistryHandle, name: &str, fault: FaultPoint) {
    let (tenant, db, _bound, task_store) = bring_tenant_to_ready(registry.clone(), name).await;
    // Enqueue a single extract task against the stubbed
    // `episode_id`. The worker deserialises `ExtractParams` from
    // the durable row and passes them to the stub extractor.
    // Only fields the production extract path requires are
    // persisted; the rest fall through to their struct defaults.
    let task_id = task_store
        .enqueue(
            &format!("fp_{name}"),
            serde_json::to_value(ExtractParams {
                episode_id: Some("episode:stub".into()),
                content: None,
                text: None,
                source_type: Some("crash_recovery".into()),
                source_id: Some(format!("src_{name}")),
                t_ref: Some("2026-09-02T00:00:00Z".into()),
                zero_shot_labels: Some(Vec::new()),
            })
            .expect("serialize ExtractParams"),
        )
        .await
        .expect("enqueue task");
    let injector = Arc::new(FailOnceAt::new(fault));
    let injector_dyn: Arc<dyn FaultInjector> = injector.clone();
    let extractor = stub_extractor();
    // First pass: must surface the simulated Transient.
    let first = execute_one_task_for_test(
        &task_store,
        db.clone(),
        &tenant.namespace_binding.namespace,
        injector_dyn.clone(),
        extractor.clone(),
    )
    .await;
    assert!(
        matches!(first, Err(MemoryError::Transient(_))),
        "{name}: first call must return Transient, got {first:?}"
    );
    // For the `TaskClaimed` fault the task is left in
    // `Running` with an active lease. The recovery contract
    // requires the next worker to re-claim and advance; in
    // production this happens after the 60s lease TTL, but the
    // test forces a requeue so the assertion is bounded.
    // For the `TaskArtifactCommitted` fault the task is also
    // in `Running` because `complete_fenced` is reached only
    // after the hit. For the `TaskCompleted` fault the task
    // is already `Completed`; `requeue_expired_running` is a
    // no-op.
    let _ = task_store.force_requeue_all_for_test().await;
    // Second pass: the fault is consumed, the durable state
    // machine runs end-to-end.
    let second = execute_one_task_for_test(
        &task_store,
        db.clone(),
        &tenant.namespace_binding.namespace,
        injector_dyn.clone(),
        extractor,
    )
    .await;
    assert!(
        second.is_ok(),
        "{name}: second call must succeed, got {second:?}"
    );
    // Counts.
    let completed = task_store
        .count_completed_tasks()
        .await
        .expect("count completed");
    let artifacts = task_store
        .count_committed_artifacts()
        .await
        .expect("count artifacts");
    assert_eq!(
        completed, 1,
        "{name}: exactly one completed task after recovery; got {completed}"
    );
    assert_eq!(
        artifacts, 1,
        "{name}: exactly one committed artifact after recovery; got {artifacts}"
    );
    assert_eq!(
        injector.consumed(),
        1,
        "{name}: FailOnceAt should have fired exactly once"
    );
    // `_bound` is retained to prove the test owns the binding
    // path; suppress the unused warning.
    let _ = _bound;
    let _ = task_id;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_recovers_after_claim_fault() {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = open_registry_at(&dir).await;
    run_task_recovery(registry, "task_recovery_claim", FaultPoint::TaskClaimed).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_recovers_after_artifact_committed_fault() {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = open_registry_at(&dir).await;
    run_task_recovery(
        registry,
        "task_recovery_artifact",
        FaultPoint::TaskArtifactCommitted,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_recovers_after_completed_fault() {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = open_registry_at(&dir).await;
    run_task_recovery(
        registry,
        "task_recovery_completed",
        FaultPoint::TaskCompleted,
    )
    .await;
}

// ---------------------------------------------------------------------------
// HTTP fixture smoke — verifies the same recovery invariant
// from the binary's perspective.
// ---------------------------------------------------------------------------

fn fixture_config(
    dir: &TempDir,
    tenant_name: &str,
    api_key: &str,
    fault: Option<FaultPoint>,
) -> HttpServerConfig {
    let mut extra_env = vec![(
        "MEMORY_MCP_HTTP_TEST_SEED_RESERVED".to_owned(),
        format!("{tenant_name}={api_key}"),
    )];
    if let Some(point) = fault {
        extra_env.push((
            "MEMORY_MCP_HTTP_TEST_FAULT_POINT".to_owned(),
            format!("{point:?}"),
        ));
    }
    HttpServerConfig {
        tenants: vec![TestTenant::new(tenant_name, api_key)],
        extra_env,
        storage_url: format!("rocksdb://{}", dir.path().display()),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_fixture_recovers_after_lease_claim_fault() {
    let dir = tempfile::tempdir().expect("tempdir");
    let name = "http_recovery_lease";
    let mut faulty = HttpServerFixture::spawn(fixture_config(
        &dir,
        name,
        UNIT_KEY_A,
        Some(FaultPoint::ProvisioningLeaseClaimed),
    ))
    .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    faulty.kill();
    // Restart the fixture against the same rocksdb path
    // so the new subprocess's scheduler advances the
    // partial tenant. Opening a fresh connection in the
    // test process would race with the new subprocess on
    // the rocksdb LOCK file; the HTTP layer is the
    // verifiable seam, so the test asserts the tenant is
    // usable via MCP `ingest` (which only succeeds against a
    // Ready tenant).
    let recovered = HttpServerFixture::spawn(fixture_config(&dir, name, UNIT_KEY_A, None)).await;
    recovered.wait_ready().await;
    let mut ingest = mcp_call(
        recovered.client(),
        &recovered.base_url,
        UNIT_KEY_A,
        "tools/call",
        serde_json::json!({
            "name": "ingest",
            "arguments": {
                "content": format!("{name}_marker"),
                "source_type": "crash_recovery",
                "source_id": format!("{name}_marker_1"),
                "t_ref": "2026-09-02T00:00:00Z",
                "t_ingested": null,
                "policy_tags": []
            }
        }),
    )
    .await;
    // The scheduler needs up to a few ticks to converge
    // the partial tenant; an ingest against a non-Ready
    // tenant returns 503. Poll briefly before asserting.
    let mut attempts = 0;
    while ingest["http_status"].as_u64() == Some(503) && attempts < 20 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        ingest = mcp_call(
            recovered.client(),
            &recovered.base_url,
            UNIT_KEY_A,
            "tools/call",
            serde_json::json!({
                "name": "ingest",
                "arguments": {
                    "content": format!("{name}_marker"),
                    "source_type": "crash_recovery",
                    "source_id": format!("{name}_marker_attempt_{attempts}"),
                    "t_ref": "2026-09-02T00:00:00Z",
                    "t_ingested": null,
                    "policy_tags": []
                }
            }),
        )
        .await;
        attempts += 1;
    }
    assert_eq!(
        ingest["http_status"], 200,
        "ingest must succeed after crash recovery; got {ingest}"
    );
}
