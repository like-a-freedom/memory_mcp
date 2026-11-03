//! Real-store extraction with a counting storage adapter.

mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use memory_mcp::MemoryError;
use memory_mcp::models::IngestRequest;
use memory_mcp::service::MemoryService;
use memory_mcp::service::memory_container_shims::memory_capabilities_extract::ExtractCapability;
use memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability;
use memory_mcp::storage::{DbClient, OwnedTable, SurrealDbClient};
use serde_json::Value;

struct CountingDb {
    inner: Arc<SurrealDbClient>,
    reads: Mutex<HashMap<String, usize>>,
    fail_name_lookup: AtomicBool,
}

#[async_trait]
impl DbClient for CountingDb {
    async fn select_one(&self, id: &str, ns: &str) -> Result<Option<Value>, MemoryError> {
        *self
            .reads
            .lock()
            .expect("read counter")
            .entry(id.into())
            .or_default() += 1;
        self.inner.select_one(id, ns).await
    }
    async fn select_table(&self, table: OwnedTable, ns: &str) -> Result<Vec<Value>, MemoryError> {
        self.inner.select_table(table, ns).await
    }
    async fn create(
        &self,
        id: &str,
        value: Value,
        ns: &str,
        fields: &[&str],
    ) -> Result<Value, MemoryError> {
        self.inner.create(id, value, ns, fields).await
    }
    async fn update(
        &self,
        id: &str,
        value: Value,
        ns: &str,
        fields: &[&str],
    ) -> Result<Value, MemoryError> {
        self.inner.update(id, value, ns, fields).await
    }
    async fn query(&self, sql: &str, vars: Option<Value>, ns: &str) -> Result<Value, MemoryError> {
        if sql == "SELECT entity_id, canonical_name, aliases FROM entity WHERE entity_id IN $ids"
            && self.fail_name_lookup.swap(false, Ordering::SeqCst)
        {
            return Err(MemoryError::Storage(
                "controlled entity name lookup failure".into(),
            ));
        }
        self.inner.query(sql, vars, ns).await
    }
    async fn apply_migrations(&self, ns: &str) -> Result<(), MemoryError> {
        self.inner.apply_migrations(ns).await
    }
}

#[tokio::test]
async fn warning_materialization_reuses_the_current_fact_for_all_relations() {
    let db = Arc::new(
        SurrealDbClient::connect_in_memory("warning_reads", "org", "warn")
            .await
            .expect("connect"),
    );
    db.apply_migrations("org").await.expect("migrate");
    let counter = Arc::new(CountingDb {
        inner: db,
        reads: Mutex::new(HashMap::new()),
        fail_name_lookup: AtomicBool::new(false),
    });
    let service =
        MemoryService::new(counter.clone(), "org".into(), "warn".into(), 50, 100).expect("service");
    let subject = common::resolve_entity(&service, "person", "Alice Smith")
        .await
        .expect("resolve");
    for value in 0..20 {
        common::seed_fact_with_links(
            &service,
            "org",
            &format!("Alice Smith reports ARR is ${value}M."),
            "2026-10-01T00:00:00Z".parse().expect("time"),
            vec![subject.clone()],
        )
        .await;
    }
    let episode = IngestCapability::ingest_from_service(
        &service,
        IngestRequest {
            source_context: None,
            source_type: "conversation".into(),
            source_id: "warning-reads".into(),
            content: "Alice Smith reports ARR is $25M.".into(),
            t_ref: chrono::Utc::now(),
            t_ingested: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .expect("ingest");
    counter.reads.lock().expect("read counter").clear();
    let result = ExtractCapability::extract_from_service(&service, &episode, None, None)
        .await
        .expect("extract");
    assert_eq!(result.warnings.len(), 20);
    let current_id = &result.facts[0].fact_id;
    let reads = counter.reads.lock().expect("read counter");
    // One read to build warnings plus two reads elsewhere in extraction.
    assert!(
        reads.get(current_id).copied().unwrap_or_default() <= 3,
        "fact reads: {reads:?}"
    );
}

#[tokio::test]
async fn name_lookup_failure_before_fact_creation_can_be_retried() {
    let db = Arc::new(
        SurrealDbClient::connect_in_memory("name_lookup_fault", "org", "warn")
            .await
            .expect("connect"),
    );
    db.apply_migrations("org").await.expect("migrate");
    let counter = Arc::new(CountingDb {
        inner: db.clone(),
        reads: Mutex::new(HashMap::new()),
        fail_name_lookup: AtomicBool::new(false),
    });
    let service =
        MemoryService::new(counter.clone(), "org".into(), "warn".into(), 50, 100).expect("service");
    let episode = IngestCapability::ingest_from_service(
        &service,
        IngestRequest {
            source_context: None,
            source_type: "chat".into(),
            source_id: "name-lookup-fault".into(),
            content: "Alice Smith reports ARR is $5M.".into(),
            t_ref: chrono::Utc::now(),
            t_ingested: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .expect("ingest");
    counter.fail_name_lookup.store(true, Ordering::SeqCst);
    let failed = ExtractCapability::extract_from_service(&service, &episode, None, None).await;
    assert!(failed.is_err());
    let rows = db
        .query(
            "SELECT fact_id FROM fact WHERE source_episode = $episode",
            Some(serde_json::json!({"episode": episode})),
            "org",
        )
        .await
        .expect("facts");
    assert!(
        serde_json::from_value::<Vec<Value>>(rows)
            .expect("rows")
            .is_empty()
    );
    let result = ExtractCapability::extract_from_service(&service, &episode, None, None)
        .await
        .expect("retry");
    assert!(!result.facts.is_empty());
    let rows = db
        .query(
            "SELECT source_fact_id FROM claim WHERE source_episode_id = $episode",
            Some(serde_json::json!({"episode": episode})),
            "org",
        )
        .await
        .expect("claims");
    assert!(
        !serde_json::from_value::<Vec<Value>>(rows)
            .expect("rows")
            .is_empty()
    );
}
