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
    relation_reads: std::sync::atomic::AtomicUsize,
    fail_relation_read_at: Option<usize>,
    fail_edge_write: AtomicBool,
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
        if sql
            == "SELECT * FROM claim_relation WHERE (left_fact_id IN $fact_ids OR right_fact_id IN $fact_ids) AND (t_invalid_ingested IS NONE OR t_invalid_ingested IS NULL)"
        {
            let read = self.relation_reads.fetch_add(1, Ordering::SeqCst);
            if self.fail_relation_read_at == Some(read) {
                return Err(MemoryError::Storage(
                    "controlled claim relation read failure".into(),
                ));
            }
        }
        self.inner.query(sql, vars, ns).await
    }
    async fn transaction_query(
        &self,
        sql: &str,
        vars: Option<Value>,
        ns: &str,
    ) -> Result<(), MemoryError> {
        if sql.contains("RELATE $in -> edge:") && self.fail_edge_write.swap(false, Ordering::SeqCst)
        {
            return Err(MemoryError::Storage("controlled edge write failure".into()));
        }
        self.inner.transaction_query(sql, vars, ns).await
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
        relation_reads: std::sync::atomic::AtomicUsize::new(0),
        fail_relation_read_at: None,
        fail_edge_write: AtomicBool::new(false),
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
    // Claim warning materialization reuses its current fact. The revisioned
    // fact write adds one persisted-row readback, so allow one more than the
    // pre-revision bound while still rejecting a read per relation.
    assert!(
        reads.get(current_id).copied().unwrap_or_default() <= 4,
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
        relation_reads: std::sync::atomic::AtomicUsize::new(0),
        fail_relation_read_at: None,
        fail_edge_write: AtomicBool::new(false),
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

#[tokio::test]
async fn inline_extract_failure_returns_stable_episode_id_for_retry_without_duplicate_episode() {
    let db = Arc::new(
        SurrealDbClient::connect_in_memory("inline_extract_retry", "org", "retry")
            .await
            .expect("connect"),
    );
    db.apply_migrations("org").await.expect("migrate");
    let counter = Arc::new(CountingDb {
        inner: db.clone(),
        reads: Mutex::new(HashMap::new()),
        fail_name_lookup: AtomicBool::new(false),
        relation_reads: std::sync::atomic::AtomicUsize::new(0),
        fail_relation_read_at: None,
        fail_edge_write: AtomicBool::new(false),
    });
    let service = MemoryService::new(counter.clone(), "org".into(), "retry".into(), 50, 100)
        .expect("service");
    let content = "Alice Smith reports ARR is $17M.";
    let params = || memory_mcp::tools::params::ExtractParams {
        episode_id: None,
        content: Some(content.into()),
        text: None,
        source_type: Some("chat".into()),
        source_id: Some("inline-retry-source".into()),
        t_ref: Some("2026-11-22T10:00:00Z".into()),
        zero_shot_labels: None,
        include_fact_evidence: true,
    };

    counter.fail_name_lookup.store(true, Ordering::SeqCst);
    let pending = memory_mcp::tools::extract(&service, params())
        .await
        .expect("a committed episode must be returned as pending when extraction fails");

    assert_eq!(pending.status, "partial");
    assert!(pending.result.episode_id.starts_with("episode:"));
    assert!(pending.result.facts.is_empty());
    let pending_json = serde_json::to_value(&pending.result).expect("serialize partial result");
    assert!(pending_json["facts"].as_array().is_some_and(Vec::is_empty));
    assert!(
        pending
            .guidance
            .as_deref()
            .is_some_and(|text| text.contains("retry"))
    );
    let before_retry_rows = db
        .query(
            "SELECT fact_id FROM fact WHERE source_episode = $episode",
            Some(serde_json::json!({"episode": pending.result.episode_id})),
            "org",
        )
        .await
        .expect("facts before extraction retry");
    assert!(
        serde_json::from_value::<Vec<Value>>(before_retry_rows)
            .expect("pre-retry fact rows")
            .is_empty(),
        "a failed entity lookup must leave no derived facts"
    );

    let retry = memory_mcp::tools::extract(
        &service,
        memory_mcp::tools::params::ExtractParams {
            episode_id: Some(pending.result.episode_id.clone()),
            content: None,
            text: None,
            source_type: None,
            source_id: None,
            t_ref: None,
            zero_shot_labels: None,
            include_fact_evidence: false,
        },
    )
    .await
    .expect("retry by the returned stable episode id");
    assert_eq!(retry.status, "success");
    assert_eq!(retry.result.episode_id, pending.result.episode_id);
    assert!(!retry.result.facts.is_empty());

    let episodes = memory_mcp::knowledge::api::owned_episode_scan(
        &memory_mcp::knowledge::infra::KnowledgeReadAdapter::new(
            service.db_client_for_port(),
            service.namespace_for_port(),
        ),
    )
    .await
    .expect("episode scan");
    assert_eq!(episodes.len(), 1, "retry must not create a second episode");

    let rows = db
        .query(
            "SELECT fact_id FROM fact WHERE source_episode = $episode",
            Some(serde_json::json!({"episode": pending.result.episode_id})),
            "org",
        )
        .await
        .expect("facts for retried episode");
    let facts = serde_json::from_value::<Vec<Value>>(rows).expect("fact rows");
    assert_eq!(facts.len(), retry.result.facts.len());
}

#[tokio::test]
async fn inline_extract_retry_is_idempotent_after_facts_were_written() {
    let db = Arc::new(
        SurrealDbClient::connect_in_memory("inline_extract_partial_write", "org", "retry")
            .await
            .expect("connect"),
    );
    db.apply_migrations("org").await.expect("migrate");
    let counter = Arc::new(CountingDb {
        inner: db.clone(),
        reads: Mutex::new(HashMap::new()),
        fail_name_lookup: AtomicBool::new(false),
        relation_reads: std::sync::atomic::AtomicUsize::new(0),
        fail_relation_read_at: None,
        fail_edge_write: AtomicBool::new(true),
    });
    let service =
        MemoryService::new(counter, "org".into(), "retry".into(), 50, 100).expect("service");

    let pending = memory_mcp::tools::extract(
        &service,
        memory_mcp::tools::params::ExtractParams {
            episode_id: None,
            content: Some("Alice Smith reports ARR is $23M.".into()),
            text: None,
            source_type: Some("chat".into()),
            source_id: Some("inline-partial-write-source".into()),
            t_ref: Some("2026-11-22T10:00:00Z".into()),
            zero_shot_labels: None,
            include_fact_evidence: false,
        },
    )
    .await
    .expect("committed episode with a post-fact failure is pending");

    assert_eq!(pending.status, "partial");
    assert!(pending.result.episode_id.starts_with("episode:"));
    let facts_before_retry = db
        .query(
            "SELECT fact_id FROM fact WHERE source_episode = $episode",
            Some(serde_json::json!({"episode": &pending.result.episode_id})),
            "org",
        )
        .await
        .expect("facts written before the controlled edge failure");
    let facts_before_retry =
        serde_json::from_value::<Vec<Value>>(facts_before_retry).expect("fact rows before retry");
    assert!(
        !facts_before_retry.is_empty(),
        "the injected edge failure must happen after at least one fact write"
    );

    let retry = memory_mcp::tools::extract(
        &service,
        memory_mcp::tools::params::ExtractParams {
            episode_id: Some(pending.result.episode_id.clone()),
            content: None,
            text: None,
            source_type: None,
            source_id: None,
            t_ref: None,
            zero_shot_labels: None,
            include_fact_evidence: false,
        },
    )
    .await
    .expect("retry should finish extraction");
    assert_eq!(retry.status, "success");
    assert_eq!(retry.result.episode_id, pending.result.episode_id);

    let facts_after_retry = db
        .query(
            "SELECT fact_id FROM fact WHERE source_episode = $episode",
            Some(serde_json::json!({"episode": &pending.result.episode_id})),
            "org",
        )
        .await
        .expect("facts after retry");
    let facts_after_retry =
        serde_json::from_value::<Vec<Value>>(facts_after_retry).expect("fact rows after retry");
    assert_eq!(facts_after_retry.len(), facts_before_retry.len());
    assert_eq!(retry.result.facts.len(), facts_after_retry.len());

    let edges_after_retry = db
        .query("SELECT edge_id FROM edge", None, "org")
        .await
        .expect("edges after retry");
    let edges_after_retry =
        serde_json::from_value::<Vec<Value>>(edges_after_retry).expect("edge rows after retry");
    assert!(!edges_after_retry.is_empty());

    memory_mcp::tools::extract(
        &service,
        memory_mcp::tools::params::ExtractParams {
            episode_id: Some(pending.result.episode_id),
            content: None,
            text: None,
            source_type: None,
            source_id: None,
            t_ref: None,
            zero_shot_labels: None,
            include_fact_evidence: false,
        },
    )
    .await
    .expect("repeated extraction remains idempotent");
    let repeated_edges = db
        .query("SELECT edge_id FROM edge", None, "org")
        .await
        .expect("edges after repeated extraction");
    let repeated_edges = serde_json::from_value::<Vec<Value>>(repeated_edges)
        .expect("edge rows after repeated extraction");
    assert_eq!(repeated_edges.len(), edges_after_retry.len());
}

#[tokio::test]
async fn relation_read_failure_is_reported_as_partial_reconciliation() {
    let db = Arc::new(
        SurrealDbClient::connect_in_memory("relation_read_fault", "org", "warn")
            .await
            .expect("connect"),
    );
    db.apply_migrations("org").await.expect("migrate");
    let counter = Arc::new(CountingDb {
        inner: db,
        reads: Mutex::new(HashMap::new()),
        fail_name_lookup: AtomicBool::new(false),
        relation_reads: std::sync::atomic::AtomicUsize::new(0),
        // Extraction first reads relations to build warnings and then reads
        // them again for its reconciliation summary. Fail only the latter.
        fail_relation_read_at: Some(1),
        fail_edge_write: AtomicBool::new(false),
    });
    let service = MemoryService::new(counter, "org".into(), "warn".into(), 50, 100)
        .expect("service")
        .with_claim_rollout_stage("evidence")
        .expect("evidence rollout stage");
    let episode = IngestCapability::ingest_from_service(
        &service,
        IngestRequest {
            source_context: None,
            source_type: "conversation".into(),
            source_id: "relation-read-fault".into(),
            content: "Alice Smith reports ARR is $25M.".into(),
            t_ref: chrono::Utc::now(),
            t_ingested: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .expect("ingest");

    let result = ExtractCapability::extract_from_service(&service, &episode, None, None)
        .await
        .expect("relation metadata read failure does not undo persisted extraction");

    let summary = result.reconciliation.expect("evidence summary is present");
    assert_eq!(
        summary.status,
        memory_mcp::models::ReconciliationStatus::Partial
    );
    assert_eq!(summary.claims_projected, result.facts.len());
    assert_eq!(summary.active_relations, 0);
    assert_eq!(summary.reason_codes, vec!["relation_read_failed"]);
}
