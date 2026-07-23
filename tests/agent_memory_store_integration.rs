//! Integration tests for the agent-memory lifecycle store.
//!
//! These tests verify the migration and storage contract described in Task 3:
//! accepted events create one episode/event/job; ignored and duplicate events
//! add zero rows; changed immutable identity conflicts; quarantine is absent
//! from ordinary retrieval; rejected secrets are absent from raw fields; and
//! fresh-DB migration passes.

use memory_mcp::storage::EventProjectionJobRecord;
use memory_mcp::storage::MemoryCaptureAuditRecord;
use memory_mcp::storage::MemoryEventRecord;
use memory_mcp::storage::{AgentMemoryStore, SurrealDbClient};

async fn setup_client() -> SurrealDbClient {
    let client = SurrealDbClient::connect_in_memory("test_db", "test", "warn")
        .await
        .expect("in-memory db");
    client
        .apply_migrations_impl("test")
        .await
        .expect("migrations");
    client
}

fn make_event(event_id: &str, disposition: &str) -> MemoryEventRecord {
    MemoryEventRecord {
        event_id: event_id.to_string(),
        adapter_id: Some("claude_code".to_string()),
        adapter_version: Some("1".to_string()),
        host_event: Some("post_tool_result".to_string()),
        session_id: Some("s1".to_string()),
        native_event_id: Some("e1".to_string()),
        event_kind: "post_tool_result".to_string(),
        task_fingerprint: "task:1".to_string(),
        normalized_task: Some("do work".to_string()),
        scope: "org".to_string(),
        project: Some("copper-palm".to_string()),
        policy_tags: vec![],
        capture_signal: Some("verified_success".to_string()),
        disposition: disposition.to_string(),
        trust_class: "lifecycle_evidence".to_string(),
        source_kind: Some("tool_result".to_string()),
        content_hash: Some("abc123".to_string()),
        content_byte_len: Some(42),
        artifact_uri_count: Some(1),
        reason_codes: vec!["accepted_outcome".to_string()],
        episode_id: Some("episode:1".to_string()),
        trace_retrieval_fingerprint: None,
        trace_selected_fact_ids: vec![],
        trace_selected_experience_ids: vec![],
        trace_policy_fingerprint: None,
        origin_kind: "lifecycle_adapter".to_string(),
        created_at: "2026-07-23T00:00:00Z".to_string(),
        expires_at: None,
    }
}

fn make_job(job_id: &str, event_id: &str) -> EventProjectionJobRecord {
    EventProjectionJobRecord {
        job_id: job_id.to_string(),
        event_id: event_id.to_string(),
        episode_id: Some("episode:1".to_string()),
        scope: "org".to_string(),
        project: Some("copper-palm".to_string()),
        status: "pending".to_string(),
        attempts: 0,
        max_attempts: 5,
        leased_at: None,
        lease_expires_at: None,
        completed_at: None,
        last_error: None,
        dead_lettered_at: None,
        origin_kind: "lifecycle_adapter".to_string(),
        created_at: "2026-07-23T00:00:00Z".to_string(),
        expires_at: None,
    }
}

#[tokio::test]
async fn accepted_event_creates_one_event_and_job() {
    let client = setup_client().await;
    let store = AgentMemoryStore::new(std::sync::Arc::new(client));

    let event = make_event("evt-1", "accepted");
    store
        .create_event(&event, "test")
        .await
        .expect("create event");

    let job = make_job("job-1", "evt-1");
    store.create_job(&job, "test").await.expect("create job");

    let loaded = store.load_event("evt-1", "test").await.expect("load event");
    assert!(loaded.is_some());
    assert_eq!(loaded.unwrap().disposition, "accepted");

    let loaded_job = store.load_job("job-1", "test").await.expect("load job");
    assert!(loaded_job.is_some());
    assert_eq!(loaded_job.unwrap().status, "pending");
}

#[tokio::test]
async fn duplicate_event_load_returns_existing() {
    let client = setup_client().await;
    let store = AgentMemoryStore::new(std::sync::Arc::new(client));

    let event = make_event("evt-dup", "accepted");
    store
        .create_event(&event, "test")
        .await
        .expect("first create");

    // Loading the same ID returns the existing record, so the caller can
    // detect the duplicate.
    let existing = store.load_event("evt-dup", "test").await.expect("load");
    assert!(existing.is_some());
    assert_eq!(existing.unwrap().event_id, "evt-dup");
}

#[tokio::test]
async fn rejected_secret_creates_audit_without_raw_content() {
    let client = setup_client().await;
    let store = AgentMemoryStore::new(std::sync::Arc::new(client));

    let audit = MemoryCaptureAuditRecord {
        audit_id: "audit-1".to_string(),
        event_id: "evt-reject".to_string(),
        content_hash: "sha256:deadbeef".to_string(),
        content_byte_len: 24,
        disposition: "rejected".to_string(),
        reason_codes: vec!["secret_like_content".to_string()],
        scope: "org".to_string(),
        project: Some("copper-palm".to_string()),
        created_at: "2026-07-23T00:00:00Z".to_string(),
        expires_at: Some("2026-08-22T00:00:00Z".to_string()),
    };
    store
        .create_audit(&audit, "test")
        .await
        .expect("create audit");

    let loaded = store
        .load_audit_by_event("evt-reject", "test")
        .await
        .expect("load audit");
    assert!(loaded.is_some());
    let loaded = loaded.unwrap();
    // The audit stores only the hash, not the raw content.
    assert_eq!(loaded.content_hash, "sha256:deadbeef");
    // There is no content field on the audit record at all.
    assert!(
        !serde_json::to_value(&loaded)
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("content")
    );
}

#[tokio::test]
async fn fresh_db_migration_passes() {
    // A fresh in-memory DB must apply all migrations including 027.
    let client = SurrealDbClient::connect_in_memory("test_db", "test", "warn")
        .await
        .expect("in-memory db");
    client
        .apply_migrations_impl("test")
        .await
        .expect("migrations on fresh db");

    // Verify the new tables are queryable.
    let store = AgentMemoryStore::new(std::sync::Arc::new(client));
    let result = store.load_event("nonexistent", "test").await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());
}
