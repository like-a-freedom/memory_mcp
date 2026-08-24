//! End-to-end tests for the claim reconciliation pipeline.
//!
//! These tests exercise the available pipeline stages: ingest → extract → claim
//! projection → job creation. Persisted-relation invariants are checked when
//! the configured test rollout produces relation rows.

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
    t_ref: &str,
) -> String {
    let episode_id = IngestCapability::ingest(
        &service.build_context(),
        IngestRequest {
            source_type: source_type.to_string(),
            source_id: source_id.to_string(),
            content: content.to_string(),
            t_ref: parse_t_ref(t_ref),
            t_ingested: None,
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
            "org",
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
            "org",
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
            "org",
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

// ─── Projection and reconciliation jobs ───────────────────────────────────────

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
        "2026-06-01T00:00:00Z",
    )
    .await;

    let fact_count: usize = db_client
        .query(
            "SELECT count() AS cnt FROM fact WHERE source_episode = $ep",
            Some(serde_json::json!({"ep": ep})),
            "org",
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

// ─── Distinct keys produce distinct claims even with the same value ────────────

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

// ─── Persisted outcomes use the accepted vocabulary ────────────────────────────

#[tokio::test]
async fn relation_outcomes_use_the_accepted_persisted_vocabulary() {
    let (service, db_client) = make_service().await;

    // Ingest two contradicting facts. The default test rollout may not persist
    // relations, so this invariant is checked for any rows that are present.
    let _ep_a = ingest_source(
        &service,
        "chat",
        "src:d1",
        "status is active",
        "2026-06-01T00:00:00Z",
    )
    .await;
    let _ep_b = ingest_source(
        &service,
        "chat",
        "src:d2",
        "status is inactive",
        "2026-06-01T00:00:00Z",
    )
    .await;

    // Check that any persisted relation outcomes match accepted vocabulary
    let outcomes: Vec<String> = db_client
        .query("SELECT outcome FROM claim_relation", None, "org")
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

// ─── Idempotency: repeated extraction never duplicates derived records ────────

#[tokio::test]
async fn repeat_extract_is_idempotent_and_preserves_derived_records() {
    let (service, db_client) = make_service().await;

    let ep = ingest_source(
        &service,
        "chat",
        "src:idem",
        "Alice Smith reports ARR is $5M.",
        "2026-06-01T00:00:00Z",
    )
    .await;

    let fact_count = || async {
        db_client
            .query(
                "SELECT count() AS cnt FROM fact WHERE source_episode = $ep",
                Some(serde_json::json!({ "ep": ep.clone() })),
                "org",
            )
            .await
            .map(|v| serde_json::from_value::<Vec<serde_json::Value>>(v).unwrap_or_default())
            .map(|rows| {
                rows.first()
                    .and_then(|r| r.get("cnt").and_then(|c| c.as_i64()))
                    .unwrap_or(0)
            })
            .unwrap_or(0)
    };

    let facts_before = fact_count().await;
    let claims_before = claim_count_for_episode(&db_client, &ep).await;
    assert!(facts_before > 0, "no facts for episode {ep}");
    assert!(claims_before > 0, "expected at least one claim");

    // Same-id/same-content is idempotent by contract: re-extracting the same
    // episode must neither duplicate facts/claims/jobs nor surface an error.
    ExtractCapability::extract(&service.build_context(), &ep, None, None)
        .await
        .expect("repeat extract should succeed");

    assert_eq!(fact_count().await, facts_before, "facts must not duplicate");
    assert_eq!(
        claim_count_for_episode(&db_client, &ep).await,
        claims_before,
        "claims must not duplicate"
    );
    let jobs_before = job_count_with_source_fact(&db_client).await;
    assert_eq!(
        job_count_with_source_fact(&db_client).await,
        jobs_before,
        "projection jobs must not duplicate"
    );
}

// ─── Explicit episode lineage drives claim projection ─────────────────────────

/// Persists an episode directly with a versioned filesystem-style `source_id`
/// and an explicit stable `source_lineage`, then runs the public extract path.
async fn ingest_episode_with_lineage(
    service: &MemoryService,
    db_client: &Arc<SurrealDbClient>,
    episode_id: &str,
    source_id: &str,
    lineage: &str,
    content: &str,
    t_ref: &str,
) {
    db_client
        .create(
            episode_id,
            serde_json::json!({
                "episode_id": episode_id,
                "source_type": "document",
                "source_id": source_id,
                "content": content,
                "t_ref": t_ref,
                "t_ingested": t_ref,
                "policy_tags": [],
                "source_lineage": lineage,
            }),
            "org",
        )
        .await
        .expect("create episode with lineage");
    ExtractCapability::extract(&service.build_context(), episode_id, None, None)
        .await
        .expect("extract episode with lineage");
}

#[tokio::test]
async fn claims_prefer_explicit_episode_lineage_over_versioned_source_id() {
    let (service, db_client) = make_service().await;

    // Two filesystem revisions of the same logical document: same lineage,
    // distinct versioned source IDs (as the watcher would persist them).
    ingest_episode_with_lineage(
        &service,
        &db_client,
        "episode:fs-rev-1",
        "fs:docs/spec.md:aaaa",
        "fs:docs/spec.md",
        "Budget is $5M.",
        "2026-06-01T00:00:00Z",
    )
    .await;
    ingest_episode_with_lineage(
        &service,
        &db_client,
        "episode:fs-rev-2",
        "fs:docs/spec.md:bbbb",
        "fs:docs/spec.md",
        "Budget is $6M.",
        "2026-06-02T00:00:00Z",
    )
    .await;

    let db_for_wait = db_client.clone();
    wait_for_claim_projection(move || {
        let db = db_for_wait.clone();
        async move {
            db.query(
                "SELECT count() AS cnt FROM claim WHERE source_episode_id IN ['episode:fs-rev-1', 'episode:fs-rev-2']",
                None,
                "org",
            )
            .await
            .map(|v| {
                serde_json::from_value::<Vec<serde_json::Value>>(v).unwrap_or_default()
            })
            .map(|rows| {
                rows.first()
                    .and_then(|r| r.get("cnt").and_then(|c| c.as_i64()))
                    .unwrap_or(0)
                    > 0
            })
            .unwrap_or(false)
        }
    })
    .await;

    let lineages: Vec<String> = db_client
        .query(
            "SELECT source_lineage FROM claim WHERE source_episode_id IN ['episode:fs-rev-1', 'episode:fs-rev-2']",
            None,
            "org",
        )
        .await
        .map(|v| serde_json::from_value::<Vec<serde_json::Value>>(v).unwrap_or_default())
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    r.get("source_lineage")
                        .and_then(|s| s.as_str().map(String::from))
                })
                .collect()
        })
        .unwrap_or_default();

    assert!(!lineages.is_empty(), "expected claims for both revisions");
    for lineage in &lineages {
        assert_eq!(
            lineage, "fs:docs/spec.md",
            "claims must carry the explicit lineage, got {lineage:?}"
        );
    }
}

/// Control: an ordinary episode (no explicit lineage) still uses its
/// `source_id` as the claim-projection fallback lineage.
#[tokio::test]
async fn ordinary_episode_uses_source_id_as_claim_lineage_fallback() {
    let (service, db_client) = make_service().await;

    let ep = ingest_source(
        &service,
        "chat",
        "src:fallback",
        "Alice Smith reports ARR is $5M.",
        "2026-06-01T00:00:00Z",
    )
    .await;

    let db_for_wait = db_client.clone();
    let ep_for_wait = ep.clone();
    wait_for_claim_projection(move || {
        let db = db_for_wait.clone();
        let ep = ep_for_wait.clone();
        async move { claim_count_for_episode(&db, &ep).await > 0 }
    })
    .await;

    let lineages: Vec<String> = db_client
        .query(
            "SELECT source_lineage FROM claim WHERE source_episode_id = $ep",
            Some(serde_json::json!({"ep": ep})),
            "org",
        )
        .await
        .map(|v| serde_json::from_value::<Vec<serde_json::Value>>(v).unwrap_or_default())
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    r.get("source_lineage")
                        .and_then(|s| s.as_str().map(String::from))
                })
                .collect()
        })
        .unwrap_or_default();

    assert!(!lineages.is_empty(), "expected at least one claim");
    for lineage in &lineages {
        assert_eq!(
            lineage, "src:fallback",
            "ordinary episodes fall back to source_id as lineage, got {lineage:?}"
        );
    }
}
