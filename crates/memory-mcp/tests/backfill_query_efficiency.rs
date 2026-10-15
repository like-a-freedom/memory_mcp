#![cfg(all(feature = "eval-support", feature = "test-fixtures"))]

mod common;

use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use memory_mcp::embedding::api::{
    CanonicalVectorPort, StoredVector, VectorApplication, VectorIdentity, VectorWritePolicy,
    update_canonical_vector,
};
use memory_mcp::embedding::infra::FactVectorAdapter;
use memory_mcp::embedding::test_fixtures::BackfillQueryFixture;
use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};

async fn seed_fact(memory: &common::TestMemory, content: &str) -> String {
    common::seed_fact_at(&memory.service, "ignored", content, Utc::now()).await
}

async fn set_embedding(
    memory: &common::TestMemory,
    fact_id: &str,
    value: serde_json::Value,
) -> Result<serde_json::Value, memory_mcp::MemoryError> {
    let record_id = fact_id.strip_prefix("fact:").expect("fact record id");
    memory
        .db_client
        .query(
            "UPDATE type::record('fact', $fact_id) SET embedding = $embedding",
            Some(json!({"fact_id": record_id, "embedding": value})),
            "org",
        )
        .await
}

fn backfill_fixture(memory: &common::TestMemory) -> BackfillQueryFixture {
    let db: Arc<dyn DbClient> = memory.db_client.clone();
    BackfillQueryFixture::new(db, "org")
}

#[tokio::test]
async fn missing_embedding_query_keeps_none_and_rejects_null() {
    let memory = common::TestMemory::new(false).await;
    let missing = seed_fact(&memory, "missing vector").await;
    let null_rejected = seed_fact(&memory, "null vector").await;
    let present = seed_fact(&memory, "present vector").await;
    let null_result = set_embedding(&memory, &null_rejected, serde_json::Value::Null).await;
    assert!(null_result.is_err(), "schema must reject NULL embeddings");
    set_embedding(&memory, &present, json!(vec![0.0_f64; 1536]))
        .await
        .expect("present vector state update");

    let rows = backfill_fixture(&memory)
        .select_missing(None, 10)
        .await
        .expect("missing-vector query");

    let mut ids = rows
        .iter()
        .map(|row| row["fact_id"].as_str().expect("fact id"))
        .collect::<Vec<_>>();
    ids.sort_unstable();
    let mut expected = [missing.as_str(), null_rejected.as_str()];
    expected.sort_unstable();
    assert_eq!(ids, expected);
    assert_eq!(
        backfill_fixture(&memory)
            .count_missing()
            .await
            .expect("missing-vector count"),
        2
    );
}

#[tokio::test]
async fn missing_embedding_projection_contains_only_generation_inputs() {
    let memory = common::TestMemory::new(false).await;
    let _missing = seed_fact(&memory, "projection fixture").await;
    let rows = backfill_fixture(&memory)
        .select_missing(None, 10)
        .await
        .expect("missing-vector query");

    assert_eq!(rows.len(), 1);
    let row = rows[0].as_object().expect("object row");
    let mut fields = row.keys().map(String::as_str).collect::<Vec<_>>();
    fields.sort_unstable();
    assert_eq!(fields, ["content", "fact_id", "fact_type", "quote"]);
}

#[tokio::test]
async fn cursor_does_not_skip_missing_vectors() {
    let memory = common::TestMemory::new(false).await;
    let mut expected = vec![
        seed_fact(&memory, "first").await,
        seed_fact(&memory, "second").await,
        seed_fact(&memory, "third").await,
        seed_fact(&memory, "fourth").await,
    ];
    expected.sort_unstable();
    let fixture = backfill_fixture(&memory);
    assert_eq!(fixture.count_missing().await.expect("count"), 4);

    let first_page = fixture.select_missing(None, 2).await.expect("first page");
    assert_eq!(first_page.len(), 2);
    let cursor = first_page[1]["fact_id"].as_str().expect("cursor fact id");

    let second_page = fixture
        .select_missing(Some(cursor), 2)
        .await
        .expect("second page");
    assert_eq!(second_page.len(), 2);
    let first_ids = first_page
        .iter()
        .map(|row| row["fact_id"].as_str().expect("fact id"))
        .collect::<Vec<_>>();
    let second_ids = second_page
        .iter()
        .map(|row| row["fact_id"].as_str().expect("fact id"))
        .collect::<Vec<_>>();
    assert!(first_ids[0] < first_ids[1]);
    assert!(first_ids[1] < second_ids[0]);
    assert!(second_ids[0] < second_ids[1]);
    let mut actual = first_ids
        .into_iter()
        .chain(second_ids)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    actual.sort_unstable();
    assert_eq!(actual, expected);
}

struct ConcurrentVectorWriter {
    db: Arc<SurrealDbClient>,
    backfill: FactVectorAdapter,
    injected: AtomicBool,
}

#[async_trait::async_trait]
impl CanonicalVectorPort for ConcurrentVectorWriter {
    async fn stored_fact_vector(
        &self,
        fact_id: &str,
    ) -> Result<StoredVector, memory_mcp::MemoryError> {
        let stored = self.backfill.stored_fact_vector(fact_id).await?;
        if matches!(stored, StoredVector::Absent) && !self.injected.swap(true, Ordering::SeqCst) {
            let record_id = fact_id.strip_prefix("fact:").expect("fact record id");
            self.db
                .query(
                    "UPDATE type::record('fact', $fact_id) \
                     SET embedding = $embedding, embedding_signature = $signature",
                    Some(json!({
                        "fact_id": record_id,
                        "embedding": vector(0.9),
                        "signature": "embsig:concurrent"
                    })),
                    "org",
                )
                .await?;
        }
        Ok(stored)
    }

    async fn apply_fact_vector(
        &self,
        fact_id: &str,
        vector: Vec<f64>,
        identity: VectorIdentity,
        at: chrono::DateTime<Utc>,
        policy: VectorWritePolicy,
    ) -> Result<VectorApplication, memory_mcp::MemoryError> {
        self.backfill
            .apply_fact_vector(fact_id, vector, identity, at, policy)
            .await
    }
}

fn vector(first: f64) -> Vec<f64> {
    let mut vector = vec![0.0; 1536];
    vector[0] = first;
    vector
}

#[tokio::test]
async fn concurrent_fill_preserves_newer_vector() {
    let memory = common::TestMemory::new(false).await;
    let fact_id = seed_fact(&memory, "concurrent vector").await;
    let db: Arc<dyn DbClient> = memory.db_client.clone();
    let adapter = FactVectorAdapter::new(db, "org");
    let port = ConcurrentVectorWriter {
        db: memory.db_client.clone(),
        backfill: adapter,
        injected: AtomicBool::new(false),
    };

    let result = update_canonical_vector(
        &port,
        &fact_id,
        vector(0.2),
        &VectorIdentity {
            provider: "test-provider".to_string(),
            model: Some("target-model".to_string()),
            dimension: 1536,
            signature: "embsig:target".to_string(),
        },
        Utc::now(),
        VectorWritePolicy::FillMissing,
    )
    .await
    .expect("conditional fill completes");

    assert_eq!(result, VectorApplication::AlreadyCurrent);
    let record = memory
        .db_client
        .select_one(&fact_id, "org")
        .await
        .expect("read fact")
        .expect("fact exists");
    assert_eq!(record["embedding_signature"], "embsig:concurrent");
    assert_eq!(record["embedding"][0], 0.9);
}

#[tokio::test]
#[ignore = "isolated local SurrealDB query-shape matrix; run with --ignored --nocapture"]
async fn benchmark_backfill_shapes_across_density_scale_and_dimension() {
    let indexed_wide = "SELECT * FROM fact WHERE embedding IS NONE ORDER BY fact_id ASC LIMIT 100";
    let indexed_narrow = "SELECT fact_id, fact_type, content, quote FROM fact \
        WHERE embedding IS NONE ORDER BY fact_id ASC LIMIT 100";
    let scan_wide =
        "SELECT * FROM fact WITH NOINDEX WHERE embedding IS NONE ORDER BY fact_id ASC LIMIT 100";
    let scan_narrow = "SELECT fact_id, fact_type, content, quote FROM fact WITH NOINDEX \
        WHERE embedding IS NONE ORDER BY fact_id ASC LIMIT 100";
    let indexed_count = "SELECT count() AS count FROM fact WHERE embedding IS NONE GROUP ALL";
    let scan_count =
        "SELECT count() AS count FROM fact WITH NOINDEX WHERE embedding IS NONE GROUP ALL";
    let candidates = [
        indexed_wide,
        indexed_narrow,
        scan_wide,
        scan_narrow,
        indexed_count,
        scan_count,
    ];

    let dimensions_and_rows = [
        (17_usize, 411_usize),
        (384, 411),
        (768, 411),
        (1536, 411),
        (2048, 0),
        (2048, 1),
        (2048, 100),
        (2048, 411),
        (2048, 10_000),
        (3072, 411),
    ];
    for (scenario, (dimension, row_count)) in dimensions_and_rows.into_iter().enumerate() {
        let database = format!("backfill_query_matrix_{scenario}");
        let db = Arc::new(
            SurrealDbClient::connect_in_memory_with_dimension(&database, "org", "warn", dimension)
                .await
                .expect("connect dimension-matched database"),
        );
        db.apply_migrations("org").await.expect("apply migrations");

        let vector = vec![0.1_f64; dimension];
        let mut fact_ids = Vec::with_capacity(row_count);
        for index in 0..row_count {
            let fact_id = format!("fact:backfill-bench-{index:05}");
            let content = format!(
                "benchmark fact {index} {}",
                "x".repeat(if index % 10 == 0 { 4096 } else { 32 })
            );
            let signature = format!("embsig:benchmark-model-{}", index % 3);
            let now = Utc::now();
            db.create(
                &fact_id,
                json!({
                    "fact_id": fact_id,
                    "fact_type": "note",
                    "content": content,
                    "quote": content,
                    "source_episode": "episode:benchmark",
                    "t_valid": now,
                    "t_ingested": now,
                    "confidence": 0.9,
                    "index_keys": [],
                    "access_count": 0,
                    "entity_links": [],
                    "scope": "org",
                    "policy_tags": [],
                    "provenance": {"source_episode": "episode:benchmark"},
                    "embedding": vector,
                    "embedding_provider": "benchmark",
                    "embedding_dimension": dimension,
                    "embedding_signature": signature,
                    "embedding_updated_at": now
                }),
                "org",
                &["t_valid", "t_ingested", "embedding_updated_at"],
            )
            .await
            .expect("seed dimension-matched fact");
            fact_ids.push(fact_id);
        }

        let pending_counts = [0_usize, 1.min(row_count), row_count.div_ceil(2), row_count];
        let pending_order = evenly_spaced_order(row_count);
        let mut previous_pending = 0;
        for pending in pending_counts {
            let newly_missing = pending_order[previous_pending..pending]
                .iter()
                .map(|index| fact_ids[*index].as_str())
                .collect::<Vec<_>>();
            if !newly_missing.is_empty() {
                db.query(
                    "UPDATE fact SET embedding = NONE WHERE fact_id IN $fact_ids",
                    Some(json!({"fact_ids": newly_missing})),
                    "org",
                )
                .await
                .expect("mark requested pending density");
            }
            previous_pending = pending;

            let mut samples = vec![Vec::<u128>::new(); candidates.len()];
            let mut result_bytes = vec![0_usize; candidates.len()];
            for round in 0..6 {
                let offset = round % candidates.len();
                for position in 0..candidates.len() {
                    let index = (position + offset) % candidates.len();
                    let started = Instant::now();
                    let result = db
                        .query(candidates[index], None, "org")
                        .await
                        .expect("query candidate shape");
                    let elapsed = started.elapsed().as_micros();
                    if round == 0 {
                        result_bytes[index] = serde_json::to_vec(&result)
                            .expect("encode query result")
                            .len();
                    }
                    samples[index].push(elapsed);
                }
            }

            let summaries = samples
                .iter_mut()
                .enumerate()
                .map(|(index, timings)| {
                    timings.sort_unstable();
                    json!({
                        "query": candidates[index],
                        "median_us": timings[timings.len() / 2],
                        "result_bytes": result_bytes[index]
                    })
                })
                .collect::<Vec<_>>();
            let baseline_plan = db
                .query(&format!("{indexed_wide} EXPLAIN"), None, "org")
                .await
                .expect("explain baseline query");
            let scan_plan = db
                .query(&format!("{scan_wide} EXPLAIN"), None, "org")
                .await
                .expect("explain table scan");
            let indexed_count_plan = db
                .query(&format!("{indexed_count} EXPLAIN"), None, "org")
                .await
                .expect("explain indexed count");
            let scan_count_plan = db
                .query(&format!("{scan_count} EXPLAIN"), None, "org")
                .await
                .expect("explain table-scan count");
            println!(
                "rows={row_count} d={dimension} pending={pending} baseline_plan={baseline_plan} scan_plan={scan_plan} indexed_count_plan={indexed_count_plan} scan_count_plan={scan_count_plan} candidates={}",
                serde_json::to_string(&summaries).expect("encode summaries")
            );
        }
    }
}

fn evenly_spaced_order(row_count: usize) -> Vec<usize> {
    if row_count < 2 {
        return (0..row_count).collect();
    }
    let mut step = (row_count * 618 / 1000).max(1);
    while greatest_common_divisor(step, row_count) != 1 {
        step += 1;
        if step >= row_count {
            step = 1;
        }
    }
    (0..row_count)
        .map(|ordinal| ordinal * step % row_count)
        .collect()
}

fn greatest_common_divisor(mut left: usize, mut right: usize) -> usize {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}
