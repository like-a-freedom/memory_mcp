#![cfg(feature = "eval-support")]

#[path = "support/triple_retrieval_oracle.rs"]
mod oracle;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use memory_mcp::knowledge::KnowledgeStoreClient;
use memory_mcp::storage::{DbClient, SurrealDbClient};
use oracle::{
    FIXED_CUTOFF, FIXTURE_NAMESPACE, SqlOverrideDbClient, corrected_triple_vars,
    corrected_unguarded_triple_sql, seed_triple_fixture,
};
use serde_json::Value;

const TEST_DIMENSION: usize = 17;
static NEXT_DATABASE: AtomicU64 = AtomicU64::new(1);

async fn fixture() -> (Arc<SurrealDbClient>, KnowledgeStoreClient) {
    let database = format!(
        "triple_contract_{}",
        NEXT_DATABASE.fetch_add(1, Ordering::Relaxed)
    );
    let db = Arc::new(
        SurrealDbClient::connect_in_memory_with_dimension(
            &database,
            FIXTURE_NAMESPACE,
            "warn",
            TEST_DIMENSION,
        )
        .await
        .expect("connect dimension-specific embedded database"),
    );
    db.apply_migrations(FIXTURE_NAMESPACE)
        .await
        .expect("apply current schema migrations");
    let db_port: Arc<dyn DbClient> = db.clone();
    seed_triple_fixture(db_port.as_ref(), FIXTURE_NAMESPACE, 8, TEST_DIMENSION)
        .await
        .expect("seed deterministic triple fixture");
    let store = KnowledgeStoreClient::new(db_port, FIXTURE_NAMESPACE);
    (db, store)
}

async fn compare_with_corrected_unguarded_sql(
    db: Arc<dyn DbClient>,
    candidate: &KnowledgeStoreClient,
    query: &str,
    limit: usize,
) -> Vec<Value> {
    let corrected_db: Arc<dyn DbClient> = Arc::new(SqlOverrideDbClient::new(
        db,
        Some(corrected_unguarded_triple_sql()),
    ));
    let corrected_store = KnowledgeStoreClient::new(corrected_db, FIXTURE_NAMESPACE);
    let candidate_rows = candidate
        .select_facts_by_triple(query, FIXED_CUTOFF, limit)
        .await
        .expect("candidate triple query");
    let corrected_rows = corrected_store
        .select_facts_by_triple(query, FIXED_CUTOFF, limit)
        .await
        .expect("frozen corrected unguarded triple query");
    assert_eq!(
        oracle::normalized_rows(&candidate_rows),
        oracle::normalized_rows(&corrected_rows),
        "store and corrected unguarded SQL must return the same normalized multiset"
    );
    for row in &candidate_rows {
        assert!(
            !row.as_object()
                .is_some_and(|object| object.contains_key("embedding")),
            "triple retrieval must not return stored vectors"
        );
        assert_eq!(
            row.get("provenance"),
            corrected_rows
                .iter()
                .find(|corrected| corrected.get("fact_id") == row.get("fact_id"))
                .and_then(|corrected| corrected.get("provenance"))
        );
    }
    candidate_rows
}

#[tokio::test]
async fn fixture_persists_expected_records_and_corrected_oracle_returns_source_fact() {
    let (db, _) = fixture().await;
    let facts = db
        .query("SELECT fact_id FROM fact", None, FIXTURE_NAMESPACE)
        .await
        .expect("select seeded fact IDs");
    let triples = db
        .query(
            "SELECT subject, source_fact_id FROM triple WHERE subject = $subject",
            Some(serde_json::json!({ "subject": "subject-match" })),
            FIXTURE_NAMESPACE,
        )
        .await
        .expect("select matching triple");
    let corrected = db
        .query(
            corrected_unguarded_triple_sql(),
            Some(corrected_triple_vars("subject-match", FIXED_CUTOFF, 40)),
            FIXTURE_NAMESPACE,
        )
        .await
        .expect("execute corrected unguarded SQL directly");

    assert_eq!(
        facts.as_array().map(Vec::len),
        Some(8),
        "seeded facts: {facts}"
    );
    assert!(
        facts.as_array().is_some_and(|rows| rows.iter().any(|row| {
            row.get("fact_id").and_then(Value::as_str) == Some("fact:triple-fixture-0000")
        })),
        "expected source fact was not persisted: {facts}"
    );
    assert_eq!(
        triples.as_array().map(Vec::len),
        Some(1),
        "seeded triple: {triples}"
    );
    assert_eq!(
        triples[0].get("source_fact_id").and_then(Value::as_str),
        Some("fact:triple-fixture-0000"),
        "triple source does not identify the expected fact: {triples}"
    );
    assert_eq!(
        corrected.as_array().map(Vec::len),
        Some(1),
        "corrected unguarded query should return the independently expected fact: {corrected}"
    );
    assert_eq!(
        corrected[0].get("fact_id").and_then(Value::as_str),
        Some("fact:triple-fixture-0000"),
        "corrected unguarded query returned an unexpected source: {corrected}"
    );
}

#[tokio::test]
async fn empty_triple_table_matches_corrected_unguarded_sql() {
    let (db, store) = fixture().await;
    db.query("DELETE triple", None, FIXTURE_NAMESPACE)
        .await
        .expect("clear triple rows");
    let db_port: Arc<dyn DbClient> = db;

    assert!(
        compare_with_corrected_unguarded_sql(db_port, &store, "match", 40)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn populated_table_without_match_matches_corrected_unguarded_sql() {
    let (db, store) = fixture().await;
    let db_port: Arc<dyn DbClient> = db;

    assert!(
        compare_with_corrected_unguarded_sql(db_port, &store, "unmatched-sentinel", 40)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn subject_match_returns_the_independently_expected_fact() {
    let (db, store) = fixture().await;
    let db_port: Arc<dyn DbClient> = db;
    let rows = compare_with_corrected_unguarded_sql(db_port, &store, "subject-match", 40).await;

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("fact_id").and_then(Value::as_str),
        Some("fact:triple-fixture-0000")
    );
    assert!(rows[0].get("provenance").is_some_and(Value::is_object));
}

#[tokio::test]
async fn predicate_match_returns_its_expected_fact() {
    let (db, store) = fixture().await;
    let db_port: Arc<dyn DbClient> = db;
    let rows = compare_with_corrected_unguarded_sql(db_port, &store, "predicate-match", 40).await;

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("fact_id").and_then(Value::as_str),
        Some("fact:triple-fixture-0001")
    );
}

#[tokio::test]
async fn object_match_returns_its_expected_fact() {
    let (db, store) = fixture().await;
    let db_port: Arc<dyn DbClient> = db;
    let rows = compare_with_corrected_unguarded_sql(db_port, &store, "object-match", 40).await;

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("fact_id").and_then(Value::as_str),
        Some("fact:triple-fixture-0002")
    );
}

#[tokio::test]
async fn duplicate_source_returns_each_fact_once() {
    let (db, store) = fixture().await;
    let db_port: Arc<dyn DbClient> = db;
    let rows = compare_with_corrected_unguarded_sql(db_port, &store, "match", 40).await;
    let source_zero_count = rows
        .iter()
        .filter(|row| {
            row.get("fact_id").and_then(Value::as_str) == Some("fact:triple-fixture-0000")
        })
        .count();

    assert_eq!(source_zero_count, 1);
    assert_eq!(rows.len(), 3);
}

#[tokio::test]
async fn dangling_source_is_ignored_when_its_triple_matches() {
    let (db, store) = fixture().await;
    let db_port: Arc<dyn DbClient> = db;

    assert!(
        compare_with_corrected_unguarded_sql(db_port, &store, "quote 'snow' 日本語", 40)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn quoted_unicode_input_matches_without_interpolation() {
    let (db, store) = fixture().await;
    let db_port: Arc<dyn DbClient> = db;

    assert!(
        compare_with_corrected_unguarded_sql(db_port, &store, "quote 'snow' 日本語", 40)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn missing_triple_table_keeps_empty_handling() {
    let (db, store) = fixture().await;
    db.query("REMOVE TABLE triple", None, FIXTURE_NAMESPACE)
        .await
        .expect("remove triple table in isolated fixture");
    let db_port: Arc<dyn DbClient> = db;

    assert!(
        compare_with_corrected_unguarded_sql(db_port, &store, "match", 40)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn missing_fact_table_keeps_empty_handling() {
    let (db, store) = fixture().await;
    db.query("REMOVE TABLE fact", None, FIXTURE_NAMESPACE)
        .await
        .expect("remove fact table in isolated fixture");
    let db_port: Arc<dyn DbClient> = db;

    assert!(
        compare_with_corrected_unguarded_sql(db_port, &store, "match", 40)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn zero_limit_returns_no_facts() {
    let (db, store) = fixture().await;
    let db_port: Arc<dyn DbClient> = db;

    assert!(
        compare_with_corrected_unguarded_sql(db_port, &store, "match", 0)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn limit_one_selects_an_eligible_fact_without_requiring_row_order() {
    let (db, store) = fixture().await;
    let db_port: Arc<dyn DbClient> = db;
    let rows = compare_with_corrected_unguarded_sql(db_port, &store, "match", 1).await;

    assert_eq!(rows.len(), 1);
    assert!(
        rows[0]
            .get("fact_id")
            .and_then(Value::as_str)
            .is_some_and(|id| {
                [
                    "fact:triple-fixture-0000",
                    "fact:triple-fixture-0001",
                    "fact:triple-fixture-0002",
                ]
                .contains(&id)
            })
    );
}

#[tokio::test]
async fn limit_three_returns_all_eligible_facts() {
    let (db, store) = fixture().await;
    let db_port: Arc<dyn DbClient> = db;
    let rows = compare_with_corrected_unguarded_sql(db_port, &store, "match", 3).await;

    assert_eq!(rows.len(), 3);
}

#[tokio::test]
async fn limit_forty_returns_only_eligible_facts() {
    let (db, store) = fixture().await;
    let db_port: Arc<dyn DbClient> = db;
    let rows = compare_with_corrected_unguarded_sql(db_port, &store, "match", 40).await;

    assert_eq!(rows.len(), 3);
}

async fn set_subject_fact_temporal_fields(
    db: &dyn DbClient,
    t_valid: &str,
    t_ingested: &str,
    t_invalid: Option<&str>,
    t_invalid_ingested: Option<&str>,
) {
    let mut assignments = vec![
        "t_valid = type::datetime($t_valid)",
        "t_ingested = type::datetime($t_ingested)",
    ];
    let mut vars = serde_json::json!({
        "t_valid": t_valid,
        "t_ingested": t_ingested,
    });

    if let Some(t_invalid) = t_invalid {
        assignments.push("t_invalid = type::datetime($t_invalid)");
        vars["t_invalid"] = Value::String(t_invalid.to_owned());
    }
    if let Some(t_invalid_ingested) = t_invalid_ingested {
        assignments.push("t_invalid_ingested = type::datetime($t_invalid_ingested)");
        vars["t_invalid_ingested"] = Value::String(t_invalid_ingested.to_owned());
    }

    let sql = format!(
        "UPDATE type::record(\"fact\", \"triple-fixture-0000\") SET {}",
        assignments.join(", ")
    );
    db.query(&sql, Some(vars), FIXTURE_NAMESPACE)
        .await
        .expect("update seeded fact temporal fields");
}

#[tokio::test]
async fn fact_valid_at_cutoff_is_included() {
    let (db, store) = fixture().await;
    set_subject_fact_temporal_fields(
        db.as_ref(),
        FIXED_CUTOFF,
        "2026-11-13T14:00:00Z",
        None,
        None,
    )
    .await;

    let rows = store
        .select_facts_by_triple("subject-match", FIXED_CUTOFF, 40)
        .await
        .expect("query subject fact at cutoff");

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("fact_id").and_then(Value::as_str),
        Some("fact:triple-fixture-0000")
    );
}

#[tokio::test]
async fn fact_valid_after_cutoff_is_excluded() {
    let (db, store) = fixture().await;
    set_subject_fact_temporal_fields(
        db.as_ref(),
        "2026-11-13T15:52:35.270Z",
        "2026-11-13T14:00:00Z",
        None,
        None,
    )
    .await;

    let rows = store
        .select_facts_by_triple("subject-match", FIXED_CUTOFF, 40)
        .await
        .expect("query future-valid subject fact");

    assert!(rows.is_empty());
}

#[tokio::test]
async fn fact_ingested_after_cutoff_is_excluded() {
    let (db, store) = fixture().await;
    set_subject_fact_temporal_fields(
        db.as_ref(),
        "2026-11-13T14:00:00Z",
        "2026-11-13T15:52:35.270Z",
        None,
        None,
    )
    .await;

    let rows = store
        .select_facts_by_triple("subject-match", FIXED_CUTOFF, 40)
        .await
        .expect("query late-ingested subject fact");

    assert!(rows.is_empty());
}

#[tokio::test]
async fn fact_invalidated_at_cutoff_is_excluded() {
    let (db, store) = fixture().await;
    set_subject_fact_temporal_fields(
        db.as_ref(),
        "2026-11-13T14:00:00Z",
        "2026-11-13T14:00:00Z",
        Some(FIXED_CUTOFF),
        Some("2026-11-13T15:52:35.268Z"),
    )
    .await;

    let rows = store
        .select_facts_by_triple("subject-match", FIXED_CUTOFF, 40)
        .await
        .expect("query invalidated subject fact at cutoff");

    assert!(rows.is_empty());
}

#[tokio::test]
async fn fact_invalidation_recorded_after_cutoff_remains_visible() {
    let (db, store) = fixture().await;
    set_subject_fact_temporal_fields(
        db.as_ref(),
        "2026-11-13T14:00:00Z",
        "2026-11-13T14:00:00Z",
        Some("2026-11-13T15:00:00Z"),
        Some("2026-11-13T15:52:35.270Z"),
    )
    .await;

    let rows = store
        .select_facts_by_triple("subject-match", FIXED_CUTOFF, 40)
        .await
        .expect("query fact before its invalidation was recorded");

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("fact_id").and_then(Value::as_str),
        Some("fact:triple-fixture-0000")
    );
}
