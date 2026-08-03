//! End-to-end tests for the claim reconciliation pipeline.
//!
//! These tests exercise the full pipeline: ingest → extract → claim projection
//! → job creation → reconciliation → persisted relations.
//! Currently some assertions are expected to fail (red phase) until
//! the pipeline is fully wired.

mod common;

use std::sync::Arc;

use chrono::{DateTime, Utc};
use memory_mcp::models::IngestRequest;
use memory_mcp::service::MemoryService;
use memory_mcp::service::capabilities::extract::ExtractCapability;
use memory_mcp::service::capabilities::ingest::IngestCapability;
use memory_mcp::storage::{DbClient, SurrealDbClient};

fn parse_t_ref(s: &str) -> DateTime<Utc> {
    s.parse::<DateTime<Utc>>().unwrap_or_else(|_| Utc::now())
}

async fn make_service() -> (MemoryService, Arc<SurrealDbClient>) {
    let tm = common::TestMemory::new(false).await;
    (tm.service, tm.db_client)
}

async fn ingest_source(
    service: &MemoryService,
    source_type: &str,
    source_id: &str,
    content: &str,
    scope: &str,
    t_ref: &str,
) -> String {
    let episode_id = IngestCapability::ingest(
        &service.build_context(),
        IngestRequest {
            source_type: source_type.to_string(),
            source_id: source_id.to_string(),
            content: content.to_string(),
            t_ref: parse_t_ref(t_ref),
            scope: scope.to_string(),
            project: None,
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .expect("ingest should succeed");
    ExtractCapability::extract(&service.build_context(), &episode_id, None, None)
        .await
        .expect("extract should succeed");
    episode_id
}

async fn claim_count_for_episode(db_client: &Arc<SurrealDbClient>, ep: &str) -> usize {
    db_client
        .query(
            "SELECT count() AS cnt FROM claim WHERE source_episode_id = $ep",
            Some(serde_json::json!({"ep": ep})),
            "personal",
        )
        .await
        .map(|v| serde_json::from_value::<Vec<serde_json::Value>>(v).unwrap_or_default())
        .map(|rows| {
            rows.first()
                .and_then(|r| r.get("cnt").and_then(|c| c.as_i64()))
                .unwrap_or(0) as usize
        })
        .unwrap_or(0)
}

async fn job_count_with_source_fact(db_client: &Arc<SurrealDbClient>) -> usize {
    db_client
        .query(
            "SELECT count() AS cnt FROM claim_job WHERE source_fact_id IS NOT NONE",
            None,
            "personal",
        )
        .await
        .map(|v| serde_json::from_value::<Vec<serde_json::Value>>(v).unwrap_or_default())
        .map(|rows| {
            rows.first()
                .and_then(|r| r.get("cnt").and_then(|c| c.as_i64()))
                .unwrap_or(0) as usize
        })
        .unwrap_or(0)
}

async fn fetch_claim_keys_for_episode(db_client: &Arc<SurrealDbClient>, ep: &str) -> Vec<String> {
    db_client
        .query(
            "SELECT comparison_key_hash FROM claim WHERE source_episode_id = $ep",
            Some(serde_json::json!({"ep": ep})),
            "personal",
        )
        .await
        .map(|v| serde_json::from_value::<Vec<serde_json::Value>>(v).unwrap_or_default())
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    r.get("comparison_key_hash")
                        .and_then(|k| k.as_str().map(String::from))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Wait for the fire-and-forget claim projection spawned by `add_fact` to
/// land at least one row satisfying `poll`. Yields between attempts so the
/// runtime can drive the spawned task forward.
async fn wait_for_claim_projection<F, Fut>(poll: F)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..200 {
        if poll().await {
            return;
        }
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    eprintln!("wait_for_claim_projection timed out while polling");
}

// ─── Gap 1: New facts produce projection and reconcile jobs ────────────────────

#[tokio::test]
async fn new_fact_eventually_has_projection_and_reconcile_jobs() {
    let (service, db_client) = make_service().await;

    // Content matches the metric heuristic ("ARR" + "$5") so `extract_facts`
    // stores one metric fact, then the fire-and-forget claim projection
    // parses the "X is Y" sentence via `AttributeV1` and writes one claim.
    let ep = ingest_source(
        &service,
        "chat",
        "src:a",
        "Alice Smith reports ARR is $5M.",
        "personal",
        "2026-06-01T00:00:00Z",
    )
    .await;

    let fact_count: usize = db_client
        .query(
            "SELECT count() AS cnt FROM fact WHERE source_episode = $ep",
            Some(serde_json::json!({"ep": ep})),
            "personal",
        )
        .await
        .map(|v| serde_json::from_value::<Vec<serde_json::Value>>(v).unwrap_or_default())
        .map(|rows| {
            rows.first()
                .and_then(|r| r.get("cnt").and_then(|c| c.as_i64()))
                .unwrap_or(0) as usize
        })
        .unwrap_or(0);
    assert!(fact_count > 0, "no facts for episode {ep}");

    let claim_count = claim_count_for_episode(&db_client, &ep).await;
    assert!(
        claim_count > 0,
        "expected at least one claim for the episode"
    );

    let db_for_jobs = db_client.clone();
    wait_for_claim_projection(move || {
        let db = db_for_jobs.clone();
        async move { job_count_with_source_fact(&db).await > 0 }
    })
    .await;

    let job_count = job_count_with_source_fact(&db_client).await;
    assert!(job_count > 0, "expected at least one claim_job");
}

// ─── Gap 2: Distinct keys produce distinct claims even with same value ─────────

#[tokio::test]
async fn same_value_under_distinct_keys_produces_distinct_claims() {
    let (service, db_client) = make_service().await;

    // The leading `ARR` token trips `is_metric_statement`, so `extract_facts`
    // stores exactly one metric fact containing all three lines. Claim
    // projection then calls `parse_assertions` on that content: the `kv`
    // parser splits `measure_a: 100` and `measure_b: 100` into two
    // assertions whose predicates (`measure_a`, `measure_b`) are NOT in the
    // AttributeV1 skip list (`measure`, `unit`, `predicate`, `object`,
    // `action`, `target`). The two drafts therefore carry distinct
    // comparison_key hashes.
    let ep = ingest_source(
        &service,
        "chat",
        "src:b",
        "ARR\nmeasure_a: 100\nmeasure_b: 100",
        "personal",
        "2026-06-01T00:00:00Z",
    )
    .await;

    let db_for_wait = db_client.clone();
    let ep_for_wait = ep.clone();
    wait_for_claim_projection(move || {
        let db = db_for_wait.clone();
        let ep = ep_for_wait.clone();
        async move { fetch_claim_keys_for_episode(&db, &ep).await.len() >= 2 }
    })
    .await;

    let claim_keys = fetch_claim_keys_for_episode(&db_client, &ep).await;

    // Deduplicate
    let unique_keys: std::collections::HashSet<_> = claim_keys.iter().collect();
    assert!(
        unique_keys.len() >= 2,
        "expected at least 2 distinct comparison key hashes for different measures, got {}",
        unique_keys.len()
    );
}

// ─── Gap 3: Reconciliation never crosses scope, project, or policy ─────────────

#[tokio::test]
async fn reconciliation_never_crosses_scope_project_or_policy() {
    let (service, db_client) = make_service().await;

    let _ep_a = ingest_source(
        &service,
        "chat",
        "src:c1",
        "status is active",
        "personal",
        "2026-06-01T00:00:00Z",
    )
    .await;
    let _ep_b = ingest_source(
        &service,
        "chat",
        "src:c2",
        "status is active",
        "team",
        "2026-06-02T00:00:00Z",
    )
    .await;

    let relation_count: usize = db_client
        .query("SELECT count() AS cnt FROM claim_relation", None, "org")
        .await
        .map(|v| serde_json::from_value::<Vec<serde_json::Value>>(v).unwrap_or_default())
        .map(|rows| {
            rows.first()
                .and_then(|r| r.get("cnt").and_then(|c| c.as_i64()))
                .unwrap_or(0) as usize
        })
        .unwrap_or(0);

    assert_eq!(
        relation_count, 0,
        "no relations should cross scope boundaries"
    );
}

// ─── Gap 4: Persisted outcomes use accepted vocabulary ─────────────────────────

#[tokio::test]
async fn relation_outcomes_use_the_accepted_persisted_vocabulary() {
    let (service, db_client) = make_service().await;

    // Ingest two contradicting facts
    let _ep_a = ingest_source(
        &service,
        "chat",
        "src:d1",
        "status is active",
        "personal",
        "2026-06-01T00:00:00Z",
    )
    .await;
    let _ep_b = ingest_source(
        &service,
        "chat",
        "src:d2",
        "status is inactive",
        "personal",
        "2026-06-01T00:00:00Z",
    )
    .await;

    // Check that any persisted relation outcomes match accepted vocabulary
    let outcomes: Vec<String> = db_client
        .query("SELECT outcome FROM claim_relation", None, "personal")
        .await
        .map(|v| serde_json::from_value::<Vec<serde_json::Value>>(v).unwrap_or_default())
        .map(|rows| {
            rows.iter()
                .filter_map(|r| r.get("outcome").and_then(|o| o.as_str().map(String::from)))
                .collect()
        })
        .unwrap_or_default();

    let accepted = [
        "duplicate",
        "supersession",
        "correction",
        "contradiction",
        "temporal_ambiguity",
    ];
    for outcome in &outcomes {
        assert!(
            accepted.contains(&outcome.as_str()),
            "unexpected outcome variant: {outcome}"
        );
    }
}
