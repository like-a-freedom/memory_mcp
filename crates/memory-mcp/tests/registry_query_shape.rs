#![cfg(all(
    feature = "streamable-http",
    feature = "control-plane",
    feature = "test-fixtures"
))]

//! Durable registry query-shape regression coverage.
//!
//! `Response::take::<Option<T>>` in SurrealDB means "at most one row" and
//! refuses a multi-element array with "Tried to take only a single result
//! from a query that contains multiple". Routing every durable read
//! through that shape silently broke any `SELECT` returning two or more
//! records — most visibly the provisioning scheduler's tenant listing, so
//! tenants never left `reserved`. These tests pin the fixed behaviour for
//! both the multi-row and the single-result case.

use memory_mcp::http::registry::SurrealRegistryStore;
use memory_mcp::http::registry::models::{NamespaceBinding, Tenant, TenantStatus};
use memory_mcp::http::registry::storage::RegistryStore;

async fn store() -> SurrealRegistryStore {
    let namespace = format!("registry_query_shape_{}", uuid::Uuid::new_v4().simple());
    SurrealRegistryStore::connect_in_memory(&namespace, "registry")
        .await
        .expect("migrated in-memory registry")
}

async fn write_ready_tenant(store: &SurrealRegistryStore, index: usize) {
    store
        .write_tenant(&Tenant {
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
        })
        .await
        .expect("write tenant");
}

#[tokio::test]
async fn multi_row_select_returns_every_row() {
    let store = store().await;
    for index in 0..3 {
        write_ready_tenant(&store, index).await;
    }

    let rows = store
        .list_ready_tenants(None, 10)
        .await
        .expect("multi-row reads must succeed");
    assert_eq!(rows.len(), 3, "all three tenants must be returned");
    assert_eq!(rows[0].id, "ten_0");
    assert_eq!(rows[2].id, "ten_2");
}

#[tokio::test]
async fn empty_select_is_an_empty_list_not_an_error() {
    let store = store().await;
    let rows = store
        .list_ready_tenants(None, 10)
        .await
        .expect("an empty result is not an error");
    assert!(rows.is_empty());
}

#[tokio::test]
async fn single_row_select_still_decodes() {
    let store = store().await;
    write_ready_tenant(&store, 7).await;
    let rows = store
        .list_ready_tenants(None, 10)
        .await
        .expect("single-row reads must succeed");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "ten_7");

    // A scalar-returning statement keeps working too.
    let found = store
        .find_tenant_by_id("ten_7")
        .await
        .expect("single lookup");
    assert_eq!(found.expect("tenant present").id, "ten_7");
}

#[tokio::test]
async fn pagination_reads_through_the_cursor() {
    let store = store().await;
    for index in 0..5 {
        write_ready_tenant(&store, index).await;
    }
    let first = store.list_ready_tenants(None, 2).await.expect("first page");
    assert_eq!(first.len(), 2);
    let second = store
        .list_ready_tenants(Some(&first[1].id), 2)
        .await
        .expect("second page");
    assert_eq!(second.len(), 2);
    assert_eq!(second[0].id, "ten_2");
}
