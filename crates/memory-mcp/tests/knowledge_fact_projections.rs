#![cfg(feature = "eval-support")]

use chrono::{Duration, Utc};
use memory_mcp::knowledge::graph_store::KnowledgeGraphStore;
use memory_mcp::models::Provenance;
use memory_mcp::storage::DbClient;
use serde_json::json;

mod common;

#[tokio::test]
async fn decay_projection_omits_content_and_embedding() {
    let memory = common::TestMemory::new(false).await;
    let fact_id = memory
        .service
        .add_fact(
            "metric",
            "fact body that decay does not need",
            "fact body that decay does not need",
            "episode:decay_projection_fixture",
            Utc::now() - Duration::days(400),
            0.4,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("fact added");
    let embedding = vec![0.125_f32; memory_mcp::config::DEFAULT_EMBEDDING_DIMENSION];
    DbClient::update(
        memory.db_client.as_ref(),
        &fact_id,
        json!({"embedding": embedding}),
        "org",
        &[],
    )
    .await
    .expect("embedding stored");
    let store = KnowledgeGraphStore::new(memory.db_client.clone(), "org");

    let rows = store
        .select_decay_candidates(10)
        .await
        .expect("decay candidates selected");

    let row = rows
        .iter()
        .find(|row| row["fact_id"].as_str() == Some(fact_id.as_str()))
        .expect("fixture fact selected");
    assert_eq!(row["fact_id"].as_str(), Some(fact_id.as_str()));
    assert!(row.get("embedding").is_none());
    assert!(row.get("content").is_none());
    assert_eq!(row["confidence"], 0.4);
}

#[tokio::test]
async fn active_fact_reads_omit_embeddings_and_keep_nonsemantic_fields() {
    let memory = common::TestMemory::new(false).await;
    let fact_id = memory
        .service
        .add_fact(
            "note",
            "synthetic active fact",
            "synthetic active fact",
            "episode:active-fixture",
            Utc::now(),
            0.8,
            vec!["active-fixture".to_string()],
            vec!["persona".to_string()],
            Provenance::manual(),
        )
        .await
        .expect("fact added");
    let embedding = vec![0.125_f32; memory_mcp::config::DEFAULT_EMBEDDING_DIMENSION];
    DbClient::update(
        memory.db_client.as_ref(),
        &fact_id,
        json!({"embedding": embedding}),
        "org",
        &[],
    )
    .await
    .expect("embedding stored");
    let store = KnowledgeGraphStore::new(memory.db_client.clone(), "org");

    let rows = store
        .select_active_facts(10)
        .await
        .expect("active facts selected");

    let row = rows
        .iter()
        .find(|row| row["fact_id"].as_str() == Some(fact_id.as_str()))
        .expect("fixture fact selected");
    assert!(row.get("embedding").is_none());
    assert_eq!(row["content"], "synthetic active fact");
    assert_eq!(row["source_episode"], "episode:active-fixture");
    assert_eq!(row["policy_tags"], json!(["persona"]));
    assert!(row["index_keys"].is_array());
    assert_eq!(row["confidence"], 0.8);
}
