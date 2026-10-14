#![cfg(feature = "test-fixtures")]

use memory_mcp::http::{
    leases::scheduler::{MaintenanceJob, MaintenanceSchedule},
    registry::{
        models::{Account, AccountStatus, NamespaceBinding, Tenant, TenantStatus},
        storage::{AccountStore, InMemoryStore, TenantStore},
    },
};
use std::time::{Duration, Instant};

fn ready_tenant(index: usize) -> Tenant {
    Tenant {
        id: format!("ten_{index:04}"),
        status: TenantStatus::Ready,
        namespace_binding: NamespaceBinding {
            namespace: format!("tns_{index:04}"),
            database: "memory".to_string(),
        },
        plan_version: 1,
        schema_version: 0,
        retry_stage: None,
        provisioning_lease: None,
        created_at: chrono::Utc::now(),
        version: 0,
    }
}

async fn store_with_ready_tenants(count: usize) -> InMemoryStore {
    let store = InMemoryStore::default();
    for index in 1..=count {
        let tenant = ready_tenant(index);
        store
            .write_account(&Account {
                id: format!("acct_{index:04}"),
                status: AccountStatus::Active,
                tenant_id: tenant.id.clone(),
                created_at: chrono::Utc::now(),
                display_name: None,
            })
            .await
            .expect("write fixture account");
        store
            .write_tenant(&tenant)
            .await
            .expect("write fixture tenant");
    }
    store
}

#[tokio::test]
async fn skipped_jobs_do_not_count_as_executed() {
    let schedule = MaintenanceSchedule::default();
    let started = Instant::now();

    assert!(schedule.due(MaintenanceJob::PhysicalCleanup, started));
    schedule.record_completed(MaintenanceJob::PhysicalCleanup, started);
    assert!(!schedule.due(
        MaintenanceJob::PhysicalCleanup,
        started + Duration::from_secs(59)
    ));
    assert!(!schedule.due(
        MaintenanceJob::PhysicalCleanup,
        started + Duration::from_secs(59) + Duration::from_millis(500)
    ));
    assert!(schedule.due(
        MaintenanceJob::PhysicalCleanup,
        started + Duration::from_secs(60)
    ));
}

#[test]
fn task_recovery_stays_fast_while_physical_cleanup_stays_slow() {
    assert_eq!(
        MaintenanceJob::TaskDispatch.cadence(),
        Duration::from_secs(1)
    );
    assert_eq!(
        MaintenanceJob::DeletionRecovery.cadence(),
        Duration::from_secs(1)
    );
    assert_eq!(
        MaintenanceJob::PhysicalCleanup.cadence(),
        Duration::from_secs(60)
    );
}

#[test]
fn maintenance_is_due_when_the_monotonic_clock_moves_backwards() {
    let schedule = MaintenanceSchedule::default();
    let now = Instant::now();
    schedule.record_completed(MaintenanceJob::TaskDispatch, now + Duration::from_secs(30));

    assert!(schedule.due(MaintenanceJob::TaskDispatch, now));
}

#[test]
fn a_new_schedule_runs_work_after_process_restart() {
    let prior_process = MaintenanceSchedule::default();
    prior_process.record_completed(MaintenanceJob::PhysicalCleanup, Instant::now());
    let restarted_process = MaintenanceSchedule::default();

    assert!(restarted_process.due(MaintenanceJob::PhysicalCleanup, Instant::now()));
}

#[tokio::test]
async fn tenant_101_receives_maintenance() {
    let store = store_with_ready_tenants(257).await;
    let schedule = MaintenanceSchedule::default();

    let first = schedule
        .next_ready_tenants(MaintenanceJob::TaskDispatch, &store, 100)
        .await
        .expect("first tenant page");
    let second = schedule
        .next_ready_tenants(MaintenanceJob::TaskDispatch, &store, 100)
        .await
        .expect("second tenant page");

    assert_eq!(first.len(), 100);
    assert_eq!(first[0].id, "ten_0001");
    assert_eq!(second[0].id, "ten_0101");
}

#[tokio::test]
async fn tenant_257_is_not_starved() {
    let store = store_with_ready_tenants(257).await;
    let schedule = MaintenanceSchedule::default();

    let _ = schedule
        .next_ready_tenants(MaintenanceJob::TaskDispatch, &store, 100)
        .await
        .expect("first tenant page");
    let _ = schedule
        .next_ready_tenants(MaintenanceJob::TaskDispatch, &store, 100)
        .await
        .expect("second tenant page");
    let third = schedule
        .next_ready_tenants(MaintenanceJob::TaskDispatch, &store, 100)
        .await
        .expect("third tenant page");
    let wrapped = schedule
        .next_ready_tenants(MaintenanceJob::TaskDispatch, &store, 100)
        .await
        .expect("cursor wraps after the final tenant page");

    assert_eq!(third.len(), 57);
    assert_eq!(
        third.last().map(|tenant| tenant.id.as_str()),
        Some("ten_0257")
    );
    assert_eq!(wrapped.len(), 100);
    assert_eq!(wrapped[0].id, "ten_0001");
}
