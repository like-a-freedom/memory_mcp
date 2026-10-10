#![cfg(feature = "eval-support")]

use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde_json::json;

#[tokio::test]
async fn configured_embedding_dimension_migrates_and_serves_ann_reads() {
    let client = SurrealDbClient::connect_in_memory_with_dimension(
        "configured_embedding_dimension",
        "main",
        "warn",
        384,
    )
    .await
    .expect("configured in-memory client connects");
    client
        .apply_migrations("main")
        .await
        .expect("migrations define an index for the configured vector dimension");

    let embedding = vec![1.0_f64; 384];
    client
        .create(
            "fact:configured-dimension",
            json!({
                "fact_id": "fact:configured-dimension",
                "fact_type": "decision",
                "content": "The ANN index uses the configured embedding dimension.",
                "quote": "configured embedding dimension",
                "source_episode": "episode:configured-dimension",
                "t_valid": "2026-01-01T00:00:00Z",
                "t_ingested": "2026-01-01T00:00:00Z",
                "confidence": 0.9,
                "entity_links": [],
                "scope": "main",
                "policy_tags": [],
                "provenance": {},
                "embedding": embedding,
            }),
            "main",
            &["t_valid", "t_ingested"],
        )
        .await
        .expect("a configured-dimension vector persists");

    let sql = "SELECT *, vector::similarity::cosine(embedding, $query_vec) AS sem_score \
        FROM fact WHERE embedding IS NOT NONE AND embedding IS NOT NULL \
        AND embedding <|1, 16|> $query_vec ORDER BY sem_score DESC LIMIT $limit";
    let rows = client
        .query(
            sql,
            Some(json!({"query_vec": embedding, "limit": 1})),
            "main",
        )
        .await
        .expect("ANN retrieval accepts and searches a 384-dimensional query vector");

    assert!(
        rows.to_string().contains("fact:configured-dimension"),
        "the configured HNSW index returns the persisted fact: {rows}"
    );
}
