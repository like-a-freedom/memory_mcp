#![cfg(all(
    feature = "streamable-http",
    feature = "control-plane",
    feature = "test-fixtures"
))]

//! Durable registry store behaviour against a real in-memory SurrealDB.
//!
//! `SurrealRegistryStore` is the only place the registry's uniqueness and
//! conditional-transition guarantees are enforced, and both are enforced in
//! SurrealQL rather than in Rust — a Rust-level unit test cannot observe them.
//! These tests therefore drive the real store against `mem://`, which keeps
//! them fast and hermetic while exercising the actual statements.

use memory_mcp::http::registry::SurrealRegistryStore;
use memory_mcp::http::registry::models::{
    Account, AccountStatus, NamespaceBinding, Tenant, TenantStatus,
};
use memory_mcp::http::registry::storage::{AccountStore, TenantStore};

/// A migrated in-memory registry under a namespace unique to this test, so
/// tests never share rows and can run in parallel.
async fn store() -> SurrealRegistryStore {
    let namespace = format!("registry_store_{}", uuid::Uuid::new_v4().simple());
    SurrealRegistryStore::connect_in_memory(&namespace, "registry")
        .await
        .expect("migrated in-memory registry")
}

/// A tenant bound to `index`, in the ready state.
fn ready_tenant(index: usize) -> Tenant {
    Tenant {
        id: format!("ten_{index}"),
        status: TenantStatus::Ready,
        namespace_binding: NamespaceBinding {
            namespace: format!("tns_{index}"),
            database: "memory".into(),
        },
        plan_version: 1,
        schema_version: 0,
        retry_stage: None,
        provisioning_lease: None,
        created_at: chrono::Utc::now(),
        version: 0,
    }
}

/// An active account pointing at `tenant_id`.
fn active_account(id: &str, tenant_id: &str) -> Account {
    Account {
        id: id.to_string(),
        status: AccountStatus::Active,
        tenant_id: tenant_id.to_string(),
        created_at: chrono::Utc::now(),
    }
}

#[tokio::test]
async fn writes_an_account_and_reads_it_back() {
    let store = store().await;
    let account = active_account("acc_1", "ten_1");
    store.write_account(&account).await.expect("write account");

    let observed = store
        .find_account_by_id("acc_1")
        .await
        .expect("lookup account");

    assert_eq!(observed.expect("account present").id, "acc_1");
}

#[tokio::test]
async fn reading_an_unknown_account_returns_none() {
    let store = store().await;

    let observed = store
        .find_account_by_id("acc_missing")
        .await
        .expect("lookup of a missing account is not an error");

    assert!(observed.is_none());
}

#[tokio::test]
async fn an_account_keeps_its_tenant_reference() {
    let store = store().await;
    store
        .write_account(&active_account("acc_2", "ten_2"))
        .await
        .expect("write account");

    let observed = store
        .find_account_by_id("acc_2")
        .await
        .expect("lookup account");

    assert_eq!(observed.expect("account present").tenant_id, "ten_2");
}

#[tokio::test]
async fn a_written_account_reads_back_as_active() {
    let store = store().await;
    store
        .write_account(&active_account("acc_3", "ten_3"))
        .await
        .expect("write account");

    let observed = store
        .find_account_by_id("acc_3")
        .await
        .expect("lookup account");

    assert_eq!(
        observed.expect("account present").status,
        AccountStatus::Active
    );
}

#[tokio::test]
async fn transitions_an_account_from_active_to_suspended() {
    let store = store().await;
    store
        .write_account(&active_account("acc_4", "ten_4"))
        .await
        .expect("write account");

    store
        .transition_account_state("acc_4", AccountStatus::Active, AccountStatus::Suspended)
        .await
        .expect("transition to suspended");

    let observed = store
        .find_account_by_id("acc_4")
        .await
        .expect("lookup account");

    assert_eq!(
        observed.expect("account present").status,
        AccountStatus::Suspended
    );
}

#[tokio::test]
async fn a_transition_from_a_stale_status_is_a_conflict() {
    let store = store().await;
    store
        .write_account(&active_account("acc_5", "ten_5"))
        .await
        .expect("write account");
    store
        .transition_account_state("acc_5", AccountStatus::Active, AccountStatus::Suspended)
        .await
        .expect("first transition");

    // The account is already suspended, so expecting `Active` is a stale read.
    let observed = store
        .transition_account_state("acc_5", AccountStatus::Active, AccountStatus::Deleting)
        .await;

    assert!(
        matches!(observed, Err(memory_mcp::error::MemoryError::Conflict(_))),
        "a conditional transition must not overwrite a newer state"
    );
}

#[tokio::test]
async fn a_transition_leaves_the_account_suspended_after_a_conflict() {
    let store = store().await;
    store
        .write_account(&active_account("acc_6", "ten_6"))
        .await
        .expect("write account");
    store
        .transition_account_state("acc_6", AccountStatus::Active, AccountStatus::Suspended)
        .await
        .expect("first transition");
    let _ = store
        .transition_account_state("acc_6", AccountStatus::Active, AccountStatus::Deleting)
        .await;

    let observed = store
        .find_account_by_id("acc_6")
        .await
        .expect("lookup account");

    assert_eq!(
        observed.expect("account present").status,
        AccountStatus::Suspended
    );
}

#[tokio::test]
async fn transitions_an_account_through_to_deleting() {
    let store = store().await;
    store
        .write_account(&active_account("acc_7", "ten_7"))
        .await
        .expect("write account");
    store
        .transition_account_state("acc_7", AccountStatus::Active, AccountStatus::Deleting)
        .await
        .expect("transition to deleting");

    let observed = store
        .find_account_by_id("acc_7")
        .await
        .expect("lookup account");

    assert_eq!(
        observed.expect("account present").status,
        AccountStatus::Deleting
    );
}

#[tokio::test]
async fn writes_a_tenant_and_reads_it_back() {
    let store = store().await;
    store
        .write_tenant(&ready_tenant(11))
        .await
        .expect("write tenant");

    let observed = store
        .find_tenant_by_id("ten_11")
        .await
        .expect("lookup tenant");

    assert_eq!(observed.expect("tenant present").id, "ten_11");
}

#[tokio::test]
async fn reading_an_unknown_tenant_returns_none() {
    let store = store().await;

    let observed = store
        .find_tenant_by_id("ten_missing")
        .await
        .expect("lookup of a missing tenant is not an error");

    assert!(observed.is_none());
}

#[tokio::test]
async fn a_tenant_reads_back_with_its_namespace_binding() {
    let store = store().await;
    store
        .write_tenant(&ready_tenant(12))
        .await
        .expect("write tenant");

    let observed = store
        .find_tenant_by_id("ten_12")
        .await
        .expect("lookup tenant");

    assert_eq!(
        observed
            .expect("tenant present")
            .namespace_binding
            .namespace,
        "tns_12"
    );
}

#[tokio::test]
async fn list_ready_tenants_omits_a_tenant_that_is_not_ready() {
    let store = store().await;
    let mut pending = ready_tenant(13);
    pending.status = TenantStatus::Reserved;
    store.write_tenant(&pending).await.expect("write tenant");

    let observed = store
        .list_ready_tenants(None, 10)
        .await
        .expect("list ready tenants");

    assert!(observed.is_empty(), "a reserved tenant is not ready");
}

#[tokio::test]
async fn list_ready_tenants_honours_its_limit() {
    let store = store().await;
    store
        .write_tenant(&ready_tenant(14))
        .await
        .expect("write tenant");
    store
        .write_tenant(&ready_tenant(15))
        .await
        .expect("write tenant");

    let observed = store
        .list_ready_tenants(None, 1)
        .await
        .expect("list ready tenants");

    assert_eq!(observed.len(), 1, "the limit must bound the page");
}

#[tokio::test]
async fn rewriting_a_tenant_updates_the_stored_row() {
    // A second write of the same id is an update, not a duplicate row.
    let store = store().await;
    store
        .write_tenant(&ready_tenant(16))
        .await
        .expect("first write");
    let mut updated = ready_tenant(16);
    updated.status = TenantStatus::Suspended;
    updated.version = 1;
    store.write_tenant(&updated).await.expect("second write");

    let observed = store
        .find_tenant_by_id("ten_16")
        .await
        .expect("lookup tenant");

    assert_eq!(
        observed.expect("tenant present").status,
        TenantStatus::Suspended
    );
}
