#![cfg(feature = "streamable-http")]

//! Tenant runtime binding-identity contract.
//!
//! Request and maintenance resolution use the owning registry adapter in
//! `bootstrap::integration::tenancy_resolution`; activation and residency belong
//! to the Pool under ADR-0072 and are exercised through its public acquisition
//! interface.

use memory_mcp::tenancy::api::{TenantLifecycleStatus, TenantRuntimeSpec};

fn spec(tenant_id: &str, namespace: &str) -> TenantRuntimeSpec {
    TenantRuntimeSpec {
        tenant_id: tenant_id.into(),
        namespace: namespace.into(),
        database: "memory".into(),
        plan_version: 1,
        schema_version: 7,
        status: TenantLifecycleStatus::Ready,
    }
}

/// Plan, schema and status describe the runtime revision and the caller's
/// permission, not which storage the runtime is bound to. Treating any of them
/// as identity would refuse a legitimate change instead of replacing a runtime
/// the change made stale, and would cache an authorization decision.
#[test]
fn identity_ignores_plan_schema_and_status() {
    let mut changed = spec("ten_a", "tns_a");
    changed.plan_version = 9;
    changed.schema_version = 12;
    changed.status = TenantLifecycleStatus::Deleting;

    assert_eq!(changed.identity(), spec("ten_a", "tns_a").identity());
}

#[test]
fn identity_changes_when_the_tenant_changes() {
    let base = spec("ten_a", "tns_a").identity();
    let changed = spec("ten_b", "tns_a").identity();

    assert_ne!(base, changed);
}

#[test]
fn identity_changes_when_the_namespace_changes() {
    let base = spec("ten_a", "tns_a").identity();
    let changed = spec("ten_a", "tns_b").identity();

    assert_ne!(base, changed);
}

#[test]
fn identity_changes_when_the_database_changes() {
    let base = spec("ten_a", "tns_a").identity();
    let mut other_database = spec("ten_a", "tns_a");
    other_database.database = "other".into();
    assert_ne!(base, other_database.identity());
}

#[cfg(feature = "test-fixtures")]
#[tokio::test]
async fn idle_pool_eviction_releases_runtime_before_same_tenant_reactivation() {
    use std::sync::Arc;
    use std::time::Duration;

    use memory_mcp::http::config::HttpConfig;
    use memory_mcp::http::test_state::HttpStateTestBuilder;

    let mut config = HttpConfig::default_for_test();
    config.runtime_idle_ttl = Duration::ZERO;
    let state = HttpStateTestBuilder::new()
        .await
        .with_config(config)
        .build()
        .await
        .expect("test HTTP state builds");
    let runtime_spec = spec("ten_eviction", "tns_eviction");

    let old_guard = state
        .pool
        .acquire_spec_with_limit(&runtime_spec, 4)
        .await
        .expect("first tenant runtime activates");
    let old_generation = Arc::downgrade(old_guard.runtime());
    assert_eq!(old_guard.runtime().tenant_id, runtime_spec.tenant_id);
    drop(old_guard);

    assert_eq!(state.pool.evict_idle().await, 1);
    assert!(
        old_generation.upgrade().is_none(),
        "idle eviction releases the prior runtime generation"
    );

    let new_guard = state
        .pool
        .acquire_spec_with_limit(&runtime_spec, 4)
        .await
        .expect("same tenant can activate after eviction");
    assert_eq!(new_guard.runtime().tenant_id, runtime_spec.tenant_id);
    assert!(
        !old_generation.ptr_eq(&Arc::downgrade(new_guard.runtime())),
        "reactivation creates a distinct tenant runtime generation"
    );
}
