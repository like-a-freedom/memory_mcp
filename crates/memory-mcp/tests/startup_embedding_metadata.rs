mod common;

use std::sync::Arc;

use memory_mcp::MemoryError;
use memory_mcp::knowledge::KnowledgeStoreClient;
use memory_mcp::knowledge::api::FactEmbeddingMetadataReadPort;
use memory_mcp::storage::DbClient;
use memory_mcp::storage::table_scope::OwnedTable;
use serde_json::Value;

struct FixedQueryDb {
    response: Value,
}

#[async_trait::async_trait]
impl DbClient for FixedQueryDb {
    async fn select_one(
        &self,
        _record_id: &str,
        _namespace: &str,
    ) -> Result<Option<Value>, MemoryError> {
        Err(MemoryError::Storage("unexpected select_one".to_string()))
    }

    async fn select_table(
        &self,
        _table: OwnedTable,
        _namespace: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        Err(MemoryError::Storage("unexpected select_table".to_string()))
    }

    async fn create(
        &self,
        _record_id: &str,
        _content: Value,
        _namespace: &str,
        _temporal_fields: &[&str],
    ) -> Result<Value, MemoryError> {
        Err(MemoryError::Storage("unexpected create".to_string()))
    }

    async fn update(
        &self,
        _record_id: &str,
        _content: Value,
        _namespace: &str,
        _temporal_fields: &[&str],
    ) -> Result<Value, MemoryError> {
        Err(MemoryError::Storage("unexpected update".to_string()))
    }

    async fn query(
        &self,
        _sql: &str,
        _vars: Option<Value>,
        _namespace: &str,
    ) -> Result<Value, MemoryError> {
        Ok(self.response.clone())
    }

    async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
        Err(MemoryError::Storage(
            "unexpected apply_migrations".to_string(),
        ))
    }
}

#[tokio::test]
async fn fact_count_includes_rows_without_embeddings() -> Result<(), Box<dyn std::error::Error>> {
    let memory = common::TestMemory::new(false).await;
    common::seed_fact_at(
        &memory.service,
        "org",
        "a fact without semantic embedding",
        chrono::Utc::now(),
    )
    .await;
    let store = KnowledgeStoreClient::new(memory.db_client.clone(), "org");

    assert_eq!(store.count_facts().await?, 1);
    Ok(())
}

#[tokio::test]
async fn dimension_sample_is_limited_to_sixteen_scalar_rows()
-> Result<(), Box<dyn std::error::Error>> {
    let memory = common::TestMemory::new(false).await;
    let timestamp = chrono::Utc::now();
    const STORED_DIMENSION: usize = 1536;
    for index in 0..18 {
        let fact_id = common::seed_fact_at(
            &memory.service,
            "org",
            &format!("fact with embedding sample row {index}"),
            timestamp,
        )
        .await;
        memory
            .db_client
            .update(
                &fact_id,
                serde_json::json!({"embedding": vec![0.5; STORED_DIMENSION]}),
                "org",
                &[],
            )
            .await?;
    }
    let expected = vec![STORED_DIMENSION; 16];
    let store = KnowledgeStoreClient::new(memory.db_client.clone(), "org");

    let dimensions = store.sample_stored_embedding_dimensions(16).await?;

    assert_eq!(dimensions, expected);
    assert_eq!(dimensions.len(), 16);
    Ok(())
}

#[tokio::test]
async fn empty_embedding_reports_dimension_zero() -> Result<(), Box<dyn std::error::Error>> {
    let store = KnowledgeStoreClient::new(
        Arc::new(FixedQueryDb {
            response: serde_json::json!([{"dimension": 0}]),
        }),
        "org",
    );

    assert_eq!(store.sample_stored_embedding_dimensions(16).await?, vec![0]);
    Ok(())
}

#[tokio::test]
async fn malformed_count_or_dimension_response_is_an_error() {
    let store = KnowledgeStoreClient::new(
        Arc::new(FixedQueryDb {
            response: serde_json::json!([{"count": -1, "dimension": -1}]),
        }),
        "org",
    );

    assert!(matches!(
        store.count_facts().await,
        Err(MemoryError::Storage(_))
    ));
    assert!(matches!(
        store.sample_stored_embedding_dimensions(16).await,
        Err(MemoryError::Storage(_))
    ));
}

#[tokio::test]
async fn mixed_dimensions_preserve_startup_refusal_or_recovery()
-> Result<(), Box<dyn std::error::Error>> {
    let store = KnowledgeStoreClient::new(
        Arc::new(FixedQueryDb {
            response: serde_json::json!([
                {"dimension": 1536},
                {"dimension": 2048}
            ]),
        }),
        "org",
    );

    assert_eq!(
        store.sample_stored_embedding_dimensions(16).await?,
        vec![1536, 2048]
    );
    Ok(())
}

#[tokio::test]
async fn none_and_null_embeddings_follow_schema_contract() -> Result<(), Box<dyn std::error::Error>>
{
    let memory = common::TestMemory::new(false).await;
    let timestamp = chrono::Utc::now();
    let _none_fact = common::seed_fact_at(
        &memory.service,
        "org",
        "fact with no embedding field",
        timestamp,
    )
    .await;
    let null_fact = common::seed_fact_at(
        &memory.service,
        "org",
        "fact with null embedding field",
        timestamp,
    )
    .await;
    let vector_fact = common::seed_fact_at(
        &memory.service,
        "org",
        "fact with a stored vector",
        timestamp,
    )
    .await;
    memory
        .db_client
        .update(
            &null_fact,
            serde_json::json!({"embedding": null}),
            "org",
            &[],
        )
        .await?;
    memory
        .db_client
        .update(
            &vector_fact,
            serde_json::json!({"embedding": vec![0.5; 1536]}),
            "org",
            &[],
        )
        .await?;
    let store = KnowledgeStoreClient::new(memory.db_client.clone(), "org");

    assert_eq!(
        store.sample_stored_embedding_dimensions(16).await?,
        vec![1536]
    );
    Ok(())
}

#[tokio::test]
async fn empty_namespace_returns_zero_and_no_dimensions() -> Result<(), Box<dyn std::error::Error>>
{
    let memory = common::TestMemory::new(false).await;
    let store = KnowledgeStoreClient::new(memory.db_client.clone(), "org");

    assert_eq!(store.count_facts().await?, 0);
    assert_eq!(
        store.sample_stored_embedding_dimensions(16).await?,
        Vec::<usize>::new()
    );
    Ok(())
}
