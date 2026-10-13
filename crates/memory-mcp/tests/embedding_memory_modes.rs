#![cfg(feature = "eval-support")]

#[path = "common/embedding_memory_modes.rs"]
mod embedding_memory_modes;

use chrono::{Duration, Utc};
use embedding_memory_modes::{AnnEvidence, SemanticFixture, semantic_fixture, semantic_query};
use memory_mcp::models::AssembleContextRequest;
use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;

async fn assembled_fact_ids(fixture: &SemanticFixture) -> Vec<String> {
    assembled_fact_ids_result(fixture)
        .await
        .expect("context assembled through the public use case")
}

async fn assembled_fact_ids_result(
    fixture: &SemanticFixture,
) -> Result<Vec<String>, memory_mcp::MemoryError> {
    let items = AssembleContextCapability::assemble_context_from_service(
        &fixture.service,
        AssembleContextRequest {
            query: semantic_query().to_string(),
            as_of: Some(Utc::now() + Duration::seconds(1)),
            budget: 5,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            fact_types: Vec::new(),
            compact: true,
        },
    )
    .await?;
    Ok(items.into_iter().map(|item| item.fact_id).collect())
}

async fn assert_one_ann_row_for_fixture(fixture: &SemanticFixture) -> serde_json::Value {
    let query_sql = fixture.observed_query_sql().await;
    let ann_query_count = query_sql
        .iter()
        .filter(|sql| sql.contains("vector::similarity::cosine"))
        .count();
    assert_eq!(
        ann_query_count, 1,
        "the controlled fixture must intercept exactly one ANN query; executed SQL: {query_sql:?}"
    );
    let rows = fixture.observed_ann_rows().await;
    assert_eq!(
        rows.len(),
        1,
        "the controlled ANN query must return its seeded row; executed SQL: {:?}",
        query_sql
    );
    assert_eq!(rows[0]["fact_id"], fixture.fact_id);
    rows[0].clone()
}

#[tokio::test]
async fn semantic_missing_score_uses_embedding_fallback() {
    let fixture = semantic_fixture(AnnEvidence::MissingScoreWithStoredVector, 0.7).await;
    let fact_ids = assembled_fact_ids(&fixture).await;

    let ann_row = assert_one_ann_row_for_fixture(&fixture).await;
    assert!(ann_row.get("sem_score").is_none());
    assert!(ann_row["embedding"].is_array());
    assert!(fact_ids.contains(&fixture.fact_id));
}

#[tokio::test]
async fn semantic_candidate_missing_evidence_is_rejected_at_zero_threshold() {
    let fixture = semantic_fixture(AnnEvidence::Missing, 0.0).await;
    let fact_ids = assembled_fact_ids(&fixture).await;

    let ann_row = assert_one_ann_row_for_fixture(&fixture).await;
    assert!(ann_row.get("sem_score").is_none());
    assert!(ann_row.get("embedding").is_none());
    assert!(
        !fact_ids.contains(&fixture.fact_id),
        "the ANN row has no score or usable fallback vector and must not be admitted"
    );
}

#[tokio::test]
async fn semantic_candidate_missing_evidence_is_rejected_at_negative_threshold() {
    let fixture = semantic_fixture(AnnEvidence::Missing, -1.0).await;
    let fact_ids = assembled_fact_ids(&fixture).await;

    let ann_row = assert_one_ann_row_for_fixture(&fixture).await;
    assert!(ann_row.get("sem_score").is_none());
    assert!(ann_row.get("embedding").is_none());
    assert!(
        !fact_ids.contains(&fixture.fact_id),
        "the ANN row has no score or usable fallback vector and must not be admitted"
    );
}

#[tokio::test]
async fn semantic_candidate_wrong_width_fallback_is_rejected() {
    let fixture = semantic_fixture(AnnEvidence::WrongWidthFallback, -1.0).await;
    let fact_ids = assembled_fact_ids(&fixture).await;

    let ann_row = assert_one_ann_row_for_fixture(&fixture).await;
    assert!(ann_row.get("sem_score").is_none());
    assert_eq!(ann_row["embedding"].as_array().map(Vec::len), Some(1));
    assert!(
        !fact_ids.contains(&fixture.fact_id),
        "a fallback vector with the wrong dimension is not semantic evidence"
    );
}

#[tokio::test]
async fn semantic_candidate_malformed_score_without_vector_is_rejected() {
    let fixture = semantic_fixture(AnnEvidence::MalformedScore, -1.0).await;
    let fact_ids = assembled_fact_ids(&fixture).await;

    let ann_row = assert_one_ann_row_for_fixture(&fixture).await;
    assert_eq!(ann_row["sem_score"], "not-a-number");
    assert!(ann_row.get("embedding").is_none());
    assert!(
        !fact_ids.contains(&fixture.fact_id),
        "a malformed score with no fallback vector is not semantic evidence"
    );
}

#[tokio::test]
async fn semantic_candidate_out_of_range_numeric_score_remains_a_storage_error() {
    let fixture = semantic_fixture(AnnEvidence::ExplicitScore(1.01), 0.0).await;
    let result = assembled_fact_ids_result(&fixture).await;

    let ann_row = assert_one_ann_row_for_fixture(&fixture).await;
    assert_eq!(ann_row["sem_score"].as_f64(), Some(1.01));
    assert!(matches!(
        result,
        Err(memory_mcp::MemoryError::Storage(message))
            if message.contains("semantic score") && message.contains("outside the cosine range")
    ));
}

#[tokio::test]
async fn semantic_score_without_embedding_keeps_candidate() {
    let fixture = semantic_fixture(AnnEvidence::ExplicitScore(1.0), 0.7).await;
    let fact_ids = assembled_fact_ids(&fixture).await;

    let ann_row = assert_one_ann_row_for_fixture(&fixture).await;
    assert_eq!(ann_row["sem_score"].as_f64(), Some(1.0));
    assert!(ann_row.get("embedding").is_none());
    assert!(fact_ids.contains(&fixture.fact_id));
}

#[tokio::test]
async fn semantic_candidate_with_explicit_zero_score_is_admitted_at_zero_threshold() {
    let fixture = semantic_fixture(AnnEvidence::ExplicitScore(0.0), 0.0).await;
    let fact_ids = assembled_fact_ids(&fixture).await;

    let ann_row = assert_one_ann_row_for_fixture(&fixture).await;
    assert_eq!(ann_row["sem_score"].as_f64(), Some(0.0));
    assert!(ann_row.get("embedding").is_none());
    assert!(fact_ids.contains(&fixture.fact_id));
}

#[tokio::test]
async fn semantic_candidate_with_explicit_negative_one_score_is_admitted_at_negative_threshold() {
    let fixture = semantic_fixture(AnnEvidence::ExplicitScore(-1.0), -1.0).await;
    let fact_ids = assembled_fact_ids(&fixture).await;

    let ann_row = assert_one_ann_row_for_fixture(&fixture).await;
    assert_eq!(ann_row["sem_score"].as_f64(), Some(-1.0));
    assert!(ann_row.get("embedding").is_none());
    assert!(fact_ids.contains(&fixture.fact_id));
}
