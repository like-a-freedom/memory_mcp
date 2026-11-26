//! Access heat writes retain metadata without transporting the immutable source again.
mod common;
use async_trait::async_trait;
use memory_mcp::{
    MemoryError,
    memory::fact_access_store::FactAccessStore,
    storage::{DbClient, SurrealDbClient, table_scope::OwnedTable},
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct MeasuredDb {
    inner: Arc<SurrealDbClient>,
    returned_bytes: AtomicUsize,
}
#[async_trait]
impl DbClient for MeasuredDb {
    async fn query(&self, sql: &str, vars: Option<Value>, ns: &str) -> Result<Value, MemoryError> {
        let result = self.inner.query(sql, vars, ns).await?;
        self.returned_bytes
            .fetch_add(serde_json::to_vec(&result).unwrap().len(), Ordering::SeqCst);
        Ok(result)
    }
    async fn select_one(&self, id: &str, ns: &str) -> Result<Option<Value>, MemoryError> {
        self.inner.select_one(id, ns).await
    }
    async fn select_table(&self, table: OwnedTable, ns: &str) -> Result<Vec<Value>, MemoryError> {
        self.inner.select_table(table, ns).await
    }
    async fn create(
        &self,
        id: &str,
        data: Value,
        ns: &str,
        times: &[&str],
    ) -> Result<Value, MemoryError> {
        self.inner.create(id, data, ns, times).await
    }
    async fn update(
        &self,
        id: &str,
        data: Value,
        ns: &str,
        times: &[&str],
    ) -> Result<Value, MemoryError> {
        self.inner.update(id, data, ns, times).await
    }
    async fn apply_migrations(&self, ns: &str) -> Result<(), MemoryError> {
        self.inner.apply_migrations(ns).await
    }
}

#[tokio::test]
async fn access_write_does_not_return_the_large_source_afterimage() {
    let (_, db) = common::make_service_with_client().await;
    let content = "Falcon source details. ".repeat(10000);
    db.create("fact:access_payload", json!({"fact_id":"fact:access_payload", "fact_type":"note", "content":content, "quote":"Falcon source details.", "source_episode":"episode:access_payload", "t_valid":"2026-01-01T00:00:00Z", "t_ingested":"2026-01-01T00:00:00Z", "confidence":0.9,"index_keys":[],"entity_links":[],"scope":"org","policy_tags":[],"access_count":4,"provenance":{"source":"manual"}}), "org", &["t_valid", "t_ingested"]).await.unwrap();
    let measured = Arc::new(MeasuredDb {
        inner: db.clone(),
        returned_bytes: AtomicUsize::new(0),
    });
    FactAccessStore::new(measured.clone(), "org")
        .record_fact_access("fact:access_payload", 1)
        .await
        .unwrap();
    let stored = db
        .select_one("fact:access_payload", "org")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored["access_count"], 5);
    assert!(stored["last_accessed"].is_string());
    assert_eq!(stored["content"], content);
    let bytes = measured.returned_bytes.load(Ordering::SeqCst);
    println!("ACCESS_WRITE_RETURNED_BYTES {bytes}");
    assert!(
        bytes < 128,
        "heat-only write must not return the source body; observed {bytes} bytes"
    );
}
