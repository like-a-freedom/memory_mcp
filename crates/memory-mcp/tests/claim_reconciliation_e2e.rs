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
use memory_mcp::service::memory_container_shims::memory_capabilities_extract::ExtractCapability;
use memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability;
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
    let episode_id = IngestCapability::ingest_from_service(
        service,
        IngestRequest {
            source_context: None,
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
    ExtractCapability::extract_from_service(service, &episode_id, None, None)
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

#[tokio::test]
async fn unscoped_historical_records_do_not_form_comparable_claims() {
    let (service, db) = make_service().await;
    let first = common::seed_fact_at(
        &service,
        "org",
        "Historical superseded workout: {\"date\":\"2026-10-24\",\"min\":95}",
        parse_t_ref("2026-11-01T00:00:00Z"),
    )
    .await;
    let second = common::seed_fact_at(
        &service,
        "org",
        "Historical superseded workout: {\"date\":\"2026-10-29\",\"min\":30}",
        parse_t_ref("2026-11-01T00:00:00Z"),
    )
    .await;
    let rows = db
        .query(
            "SELECT source_fact_id FROM claim WHERE source_fact_id IN $ids",
            Some(serde_json::json!({"ids": [first, second]})),
            "org",
        )
        .await
        .expect("query claims");
    let claims: Vec<serde_json::Value> = serde_json::from_value(rows).expect("claim rows");
    assert!(
        claims.is_empty(),
        "unscoped independent records must not share a claim slot: {claims:?}"
    );
    let rows = db
        .query(
            "SELECT fact_id FROM fact WHERE fact_id IN $ids",
            Some(serde_json::json!({"ids": [first, second]})),
            "org",
        )
        .await
        .expect("query source facts");
    assert_eq!(
        serde_json::from_value::<Vec<serde_json::Value>>(rows)
            .expect("fact rows")
            .len(),
        2
    );
}

#[tokio::test]
async fn source_episode_and_facts_survive_when_record_projection_is_skipped() {
    let (service, db) = make_service().await;
    let episode = ingest_source(
        &service,
        "conversation",
        "source:records",
        "Facts:\n- Historical record: {\"date\":\"2026-10-24\",\"duration\":95}\n- Historical record: {\"date\":\"2026-10-29\",\"duration\":30}",
        "2026-11-01T00:00:00Z",
    ).await;
    let facts = db
        .query(
            "SELECT fact_id FROM fact WHERE source_episode = $episode",
            Some(serde_json::json!({"episode": episode})),
            "org",
        )
        .await
        .expect("read sourced facts");
    assert_eq!(
        serde_json::from_value::<Vec<serde_json::Value>>(facts)
            .expect("facts")
            .len(),
        2
    );
    assert_eq!(claim_count_for_episode(&db, &episode).await, 0);
    let episodes = db
        .query(
            "SELECT episode_id FROM episode WHERE episode_id = $episode",
            Some(serde_json::json!({"episode": episode})),
            "org",
        )
        .await
        .expect("read source episode");
    assert_eq!(
        serde_json::from_value::<Vec<serde_json::Value>>(episodes)
            .expect("episodes")
            .len(),
        1
    );
}

#[tokio::test]
async fn same_subject_conflicting_values_produce_contradiction() {
    let (service, db) = make_service().await;
    let time = parse_t_ref("2026-11-01T00:00:00Z");
    let subject = common::resolve_entity(&service, "person", "Alice Smith")
        .await
        .expect("resolve");
    let first = common::seed_fact_with_links(
        &service,
        "org",
        "Alice Smith reports ARR is $5M.",
        time,
        vec![subject.clone()],
    )
    .await;
    let second = common::seed_fact_with_links(
        &service,
        "org",
        "Alice Smith reports ARR is $7M.",
        time,
        vec![subject],
    )
    .await;
    let rows = db.query(
        "SELECT outcome FROM claim_relation WHERE left_fact_id IN $ids AND right_fact_id IN $ids",
        Some(serde_json::json!({"ids": [first, second]})),
        "org",
    ).await.expect("query relations");
    let relations: Vec<serde_json::Value> = serde_json::from_value(rows).expect("relation rows");
    assert!(
        relations.iter().any(|r| r["outcome"] == "contradiction"),
        "expected real contradiction: {relations:?}"
    );
}

#[tokio::test]
async fn numeric_heading_alone_does_not_establish_single_valued_cardinality() {
    let (service, db) = make_service().await;
    let time = parse_t_ref("2026-11-01T00:00:00Z");
    let first = common::seed_fact_with_links(
        &service,
        "org",
        "Historical session: 95",
        time,
        vec!["entity:athlete".into()],
    )
    .await;
    let second = common::seed_fact_with_links(
        &service,
        "org",
        "Historical session: 30",
        time,
        vec!["entity:athlete".into()],
    )
    .await;
    let rows = db.query("SELECT outcome FROM claim_relation WHERE left_fact_id IN $ids AND right_fact_id IN $ids", Some(serde_json::json!({"ids": [first, second]})), "org").await.expect("relations");
    let relations: Vec<serde_json::Value> = serde_json::from_value(rows).expect("rows");
    assert!(
        !relations.iter().any(|r| r["outcome"] == "contradiction"),
        "a number does not prove one value per subject: {relations:?}"
    );
}

#[tokio::test]
async fn named_subject_in_prose_keeps_a_genuine_conflict() {
    let (service, _) = make_service().await;
    ingest_source(
        &service,
        "chat",
        "named:first",
        "Alice Smith reports ARR is $5M.",
        "2026-06-01T00:00:00Z",
    )
    .await;
    let id = IngestCapability::ingest_from_service(
        &service,
        IngestRequest {
            source_context: None,
            source_type: "chat".into(),
            source_id: "named:second".into(),
            content: "Alice Smith reports ARR is $7M.".into(),
            t_ref: parse_t_ref("2026-06-01T00:00:00Z"),
            t_ingested: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .expect("ingest");
    let result = ExtractCapability::extract_from_service(&service, &id, None, None)
        .await
        .expect("extract");
    assert!(
        !result.warnings.is_empty(),
        "a conflict for the same named subject must remain visible"
    );
}

#[tokio::test]
async fn different_named_subjects_do_not_share_a_claim_slot() {
    let (service, db) = make_service().await;
    let first = ingest_source(
        &service,
        "chat",
        "different:first",
        "Alice Smith reports ARR is $5M.",
        "2026-06-01T00:00:00Z",
    )
    .await;
    let id = IngestCapability::ingest_from_service(
        &service,
        IngestRequest {
            source_context: None,
            source_type: "chat".into(),
            source_id: "different:second".into(),
            content: "Bob Jones reports ARR is $7M.".into(),
            t_ref: parse_t_ref("2026-06-01T00:00:00Z"),
            t_ingested: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .expect("ingest");
    let result = ExtractCapability::extract_from_service(&service, &id, None, None)
        .await
        .expect("extract");
    assert!(claim_count_for_episode(&db, &first).await > 0);
    assert!(claim_count_for_episode(&db, &id).await > 0);
    assert!(
        result.warnings.is_empty(),
        "different subjects must not share a claim slot"
    );
}

#[tokio::test]
async fn a_linked_entity_does_not_make_a_json_blob_a_scalar_claim() {
    let (service, db) = make_service().await;
    let fact = common::seed_fact_with_links(
        &service,
        "org",
        "Historical record: {\"date\":\"2026-10-24\",\"min\":95}",
        parse_t_ref("2026-11-01T00:00:00Z"),
        vec!["entity:service".into()],
    )
    .await;
    let rows = db.query("SELECT source_fact_id, schema_family, comparison_key FROM claim WHERE source_fact_id = $id", Some(serde_json::json!({"id": fact})), "org").await.expect("query claims");
    let claims: Vec<serde_json::Value> = serde_json::from_value(rows).expect("claim rows");
    assert!(
        claims.is_empty(),
        "a JSON record is not a scalar assertion: {claims:?}"
    );
}

#[tokio::test]
async fn a_linked_entity_does_not_make_a_prose_heading_a_scalar_claim() {
    let (service, db) = make_service().await;
    let fact = common::seed_fact_with_links(
        &service,
        "org",
        "Training summary: completed an easy run in the park",
        parse_t_ref("2026-11-01T00:00:00Z"),
        vec!["entity:athlete".into()],
    )
    .await;
    let rows = db
        .query(
            "SELECT source_fact_id FROM claim WHERE source_fact_id = $id",
            Some(serde_json::json!({"id": fact})),
            "org",
        )
        .await
        .expect("query claims");
    let claims: Vec<serde_json::Value> = serde_json::from_value(rows).expect("claim rows");
    assert!(
        claims.is_empty(),
        "prose after a heading is not a scalar assertion: {claims:?}"
    );
}

#[tokio::test]
async fn a_prose_heading_starting_with_a_number_is_not_a_scalar_claim() {
    let (service, db) = make_service().await;
    let fact = common::seed_fact_with_links(
        &service,
        "org",
        "Training summary: 12 km easy run",
        parse_t_ref("2026-11-01T00:00:00Z"),
        vec!["entity:athlete".into()],
    )
    .await;
    let rows = db
        .query(
            "SELECT source_fact_id FROM claim WHERE source_fact_id = $id",
            Some(serde_json::json!({"id": fact})),
            "org",
        )
        .await
        .expect("query claims");
    let claims: Vec<serde_json::Value> = serde_json::from_value(rows).expect("claim rows");
    assert!(
        claims.is_empty(),
        "a number does not turn prose into a scalar claim: {claims:?}"
    );
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

    // The statement contains a named subject and a reported metric.
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

    // Two sourced statements with the same subject and value but different
    // metrics must retain distinct comparison keys.
    let ep = ingest_source(
        &service,
        "chat",
        "src:b",
        "Facts:\n- Alice Smith reports ARR is $5M.\n- Alice Smith reports MRR is $5M.",
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
    let jobs_before = job_count_with_source_fact(&db_client).await;
    ExtractCapability::extract_from_service(&service, &ep, None, None)
        .await
        .expect("repeat extract should succeed");

    assert_eq!(fact_count().await, facts_before, "facts must not duplicate");
    assert_eq!(
        claim_count_for_episode(&db_client, &ep).await,
        claims_before,
        "claims must not duplicate"
    );
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
            memory_mcp::memory::queries::EPISODE_TEMPORAL_FIELDS,
        )
        .await
        .expect("create episode with lineage");
    ExtractCapability::extract_from_service(service, episode_id, None, None)
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
        "Alice Smith reports ARR is $5M.",
        "2026-06-01T00:00:00Z",
    )
    .await;
    ingest_episode_with_lineage(
        &service,
        &db_client,
        "episode:fs-rev-2",
        "fs:docs/spec.md:bbbb",
        "fs:docs/spec.md",
        "Alice Smith reports ARR is $6M.",
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

#[tokio::test]
async fn explicit_schema_fields_in_text_keep_scalar_claims() {
    let (service, db) = make_service().await;
    let subject = common::resolve_entity(&service, "person", "Alice Smith")
        .await
        .expect("subject");
    let fact = common::seed_fact_with_links(
        &service,
        "org",
        "subject: Alice Smith\ndimension: height\nvalue: 180",
        parse_t_ref("2026-11-01T00:00:00Z"),
        vec![subject],
    )
    .await;
    let rows = db
        .query(
            "SELECT source_fact_id FROM claim WHERE source_fact_id = $id",
            Some(serde_json::json!({"id": fact})),
            "org",
        )
        .await
        .expect("claims");
    let rows: Vec<serde_json::Value> = serde_json::from_value(rows).expect("rows");
    assert!(
        !rows.is_empty(),
        "an explicit schema record must remain projectable"
    );
}

#[tokio::test]
async fn arbitrary_equals_heading_does_not_establish_cardinality() {
    let (service, db) = make_service().await;
    let fact = common::seed_fact_with_links(
        &service,
        "org",
        "Historical record = 95",
        parse_t_ref("2026-11-01T00:00:00Z"),
        vec!["entity:subject".into()],
    )
    .await;
    let rows = db
        .query(
            "SELECT source_fact_id FROM claim WHERE source_fact_id = $id",
            Some(serde_json::json!({"id": fact})),
            "org",
        )
        .await
        .expect("claims");
    assert!(
        serde_json::from_value::<Vec<serde_json::Value>>(rows)
            .expect("rows")
            .is_empty()
    );
}

#[tokio::test]
async fn explicit_schema_record_keeps_a_genuine_conflict() {
    let (service, db) = make_service().await;
    let subject = common::resolve_entity(&service, "concept", "device")
        .await
        .expect("subject");
    let first = common::seed_fact_with_links(
        &service,
        "org",
        "subject: device\ndimension: state\nvalue: active",
        parse_t_ref("2026-11-01T00:00:00Z"),
        vec![subject.clone()],
    )
    .await;
    let second = common::seed_fact_with_links(
        &service,
        "org",
        "subject: device\ndimension: state\nvalue: inactive",
        parse_t_ref("2026-11-01T00:00:00Z"),
        vec![subject],
    )
    .await;
    let rows = db.query("SELECT outcome FROM claim_relation WHERE left_fact_id IN $ids AND right_fact_id IN $ids", Some(serde_json::json!({"ids": [first, second]})), "org").await.expect("relations");
    let rows: Vec<serde_json::Value> = serde_json::from_value(rows).expect("rows");
    assert!(rows.iter().any(|row| row["outcome"] == "contradiction"));
}

#[tokio::test]
async fn compatible_descriptions_do_not_invent_single_valued_attributes() {
    let (service, db) = make_service().await;
    let subject = common::resolve_entity(&service, "person", "Alice Smith")
        .await
        .expect("subject");
    let first = common::seed_fact_with_links(
        &service,
        "org",
        "Alice Smith is a runner",
        parse_t_ref("2026-11-01T00:00:00Z"),
        vec![subject.clone()],
    )
    .await;
    let second = common::seed_fact_with_links(
        &service,
        "org",
        "Alice Smith is a coach",
        parse_t_ref("2026-11-01T00:00:00Z"),
        vec![subject],
    )
    .await;
    let rows = db.query("SELECT outcome FROM claim_relation WHERE left_fact_id IN $ids AND right_fact_id IN $ids", Some(serde_json::json!({"ids": [first, second]})), "org").await.expect("relations");
    assert!(
        !serde_json::from_value::<Vec<serde_json::Value>>(rows)
            .expect("rows")
            .iter()
            .any(|row| row["outcome"] == "contradiction")
    );
}
