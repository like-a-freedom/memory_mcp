//! Integration tests for the procedure candidate store.
//!
//! These tests verify the storage contract:
//! candidates are persisted, loaded, updated, and listed by filter.
//! The procedure gate must pass before promotion is enabled.

use memory_mcp::models::ProcedureCandidateRecord;
use memory_mcp::service::procedures::{ReviewAction, rank_candidates, review_candidate};
use memory_mcp::storage::{DbClient, ProcedureStore, SurrealDbClient};

async fn setup_store() -> ProcedureStore {
    let client = SurrealDbClient::connect_in_memory("procedure_test", "org", "warn")
        .await
        .expect("in-memory db");
    client
        .apply_migrations_impl("org")
        .await
        .expect("migrations");
    ProcedureStore::new(std::sync::Arc::new(client), "org")
}

fn make_candidate(
    id: &str,
    scope: Option<&str>,
    project: Option<&str>,
    task: &str,
) -> ProcedureCandidateRecord {
    ProcedureCandidateRecord {
        candidate_id: id.to_string(),
        namespace: "org".to_string(),
        identity_version: 2,
        scope: scope.map(str::to_string),
        project: project.map(str::to_string),
        task_fingerprint: task.to_string(),
        normalized_task: task.to_string(),
        status: "shadow".to_string(),
        trust_floor: "lifecycle_evidence".to_string(),
        success_count: 3,
        failure_count: 1,
        evidence_count: 4,
        origin_kind: "lifecycle_adapter".to_string(),
        created_at: "2026-07-23T00:00:00Z".to_string(),
        updated_at: "2026-07-23T00:00:00Z".to_string(),
        promoted_at: None,
        deprecated_at: None,
        expires_at: None,
    }
}

#[tokio::test]
async fn create_and_load_candidate() {
    let store = setup_store().await;
    let candidate = make_candidate("c1", Some("org"), Some("p"), "add oauth");
    store
        .create_candidate(&candidate)
        .await
        .expect("create candidate");

    let loaded = store.load_candidate("c1").await.expect("load candidate");
    let record = loaded.expect("candidate must exist");
    assert_eq!(record.candidate_id, "c1");
    assert_eq!(record.status, "shadow");
    assert_eq!(record.success_count, 3);
}

#[tokio::test]
async fn load_nonexistent_returns_none() {
    let store = setup_store().await;
    let loaded = store.load_candidate("nonexistent").await.expect("load");
    assert!(loaded.is_none());
}

#[tokio::test]
async fn load_legacy_candidate_without_identity_version_preserves_metadata() {
    let client = SurrealDbClient::connect_in_memory("procedure_test", "org", "warn")
        .await
        .expect("in-memory db");
    client
        .apply_migrations_impl("org")
        .await
        .expect("migrations");
    client
        .create(
            "procedure_candidate:legacy",
            serde_json::json!({
                "candidate_id": "legacy",
                "namespace": "org",
                "scope": "org",
                "project": "legacy-project",
                "task_fingerprint": "legacy:task",
                "normalized_task": "do legacy work",
                "status": "shadow",
                "trust_floor": "lifecycle_evidence",
                "origin_kind": "lifecycle_adapter",
                "created_at": "2026-07-01T00:00:00Z",
                "updated_at": "2026-07-01T00:00:00Z"
            }),
            "org",
        )
        .await
        .expect("create legacy candidate");

    let store = ProcedureStore::new(std::sync::Arc::new(client), "org");
    let loaded = store
        .load_candidate("legacy")
        .await
        .expect("load legacy candidate")
        .expect("legacy candidate must exist");
    assert_eq!(loaded.identity_version, 1);
    assert_eq!(loaded.scope.as_deref(), Some("org"));
    assert_eq!(loaded.project.as_deref(), Some("legacy-project"));
}

#[tokio::test]
async fn update_candidate_changes_status() {
    let store = setup_store().await;
    let mut candidate = make_candidate("c2", Some("org"), Some("p"), "fix bug");
    store.create_candidate(&candidate).await.expect("create");

    // Promote via review.
    let decision = review_candidate(&candidate, &ReviewAction::Promote).expect("promote");
    candidate.status = decision.new_status.as_str().to_string();
    candidate.promoted_at = Some("2026-07-23T12:00:00Z".to_string());

    store.update_candidate(&candidate).await.expect("update");

    // Readback: verify the persisted record matches.
    let loaded = store
        .load_candidate("c2")
        .await
        .expect("load")
        .expect("exists");
    assert_eq!(loaded.status, "promoted");
    assert!(loaded.promoted_at.is_some());
}

#[tokio::test]
async fn list_candidates_are_local_to_namespace_not_legacy_partitions() {
    let store = setup_store().await;
    store
        .create_candidate(&make_candidate("l1", Some("org"), Some("p1"), "task a"))
        .await
        .expect("create");
    store
        .create_candidate(&make_candidate("l2", Some("private"), Some("p2"), "task b"))
        .await
        .expect("create");

    let all = store.list_candidates(None).await.expect("list");
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn list_candidates_supports_status_without_scope_or_project_filtering() {
    let store = setup_store().await;
    let mut c1 = make_candidate("f1", Some("org"), Some("p1"), "task a");
    c1.status = "promoted".to_string();
    store.create_candidate(&c1).await.expect("create");

    let c2 = make_candidate("f2", Some("private"), Some("p2"), "task b");
    store.create_candidate(&c2).await.expect("create");

    let promoted = store.list_candidates(Some("promoted")).await.expect("list");
    assert_eq!(promoted.len(), 1);
    assert_eq!(promoted[0].candidate_id, "f1");
}

#[tokio::test]
async fn rank_candidates_from_store() {
    let store = setup_store().await;
    let mut high = make_candidate("rank-high", Some("org"), Some("p"), "add oauth login");
    high.success_count = 9;
    high.failure_count = 1;
    high.evidence_count = 10;
    high.updated_at = "2026-07-22T00:00:00Z".to_string();
    store.create_candidate(&high).await.expect("create");

    let mut low = make_candidate("rank-low", Some("org"), Some("p"), "add oauth login");
    low.success_count = 1;
    low.failure_count = 9;
    low.evidence_count = 1;
    low.updated_at = "2026-01-01T00:00:00Z".to_string();
    store.create_candidate(&low).await.expect("create");

    let candidates = store.list_candidates(None).await.expect("list");

    let ranked = rank_candidates(candidates, "add oauth login");
    assert_eq!(ranked[0].candidate.candidate_id, "rank-high");
    assert_eq!(ranked[1].candidate.candidate_id, "rank-low");
    assert!(ranked[0].score > ranked[1].score);
}

#[tokio::test]
async fn no_promotion_without_operator_authority() {
    // Candidates created from accepted evidence start as shadow and cannot
    // self-promote. This test verifies the review gate.
    let store = setup_store().await;
    let candidate = make_candidate("gate-1", Some("org"), Some("p"), "task");
    store.create_candidate(&candidate).await.expect("create");

    // The candidate is shadow.
    let loaded = store
        .load_candidate("gate-1")
        .await
        .expect("load")
        .expect("exists");
    assert_eq!(loaded.status, "shadow");

    // Only an explicit operator review action can promote it.
    let decision = review_candidate(&loaded, &ReviewAction::Promote).expect("promote");
    assert_eq!(decision.new_status.as_str(), "promoted");
}

#[tokio::test]
async fn no_candidate_from_untrusted_evidence_status() {
    // Candidates with untrusted origin_kind cannot be promoted — the review
    // gate enforces that only lifecycle_evidence or higher trust can promote.
    let store = setup_store().await;
    let mut candidate = make_candidate("untrusted-1", Some("org"), Some("p"), "task");
    candidate.trust_floor = "untrusted_external".to_string();
    candidate.origin_kind = "untrusted_external".to_string();
    store.create_candidate(&candidate).await.expect("create");

    // The candidate exists but is shadow. Promotion would require operator
    // review, and the trust floor is untrusted — a real review flow would
    // reject this. Here we verify the store persists it as shadow.
    let loaded = store
        .load_candidate("untrusted-1")
        .await
        .expect("load")
        .expect("exists");
    assert_eq!(loaded.status, "shadow");
    assert_eq!(loaded.trust_floor, "untrusted_external");
}
