//! Test-only access to embedding-owned persistence queries.

use std::sync::Arc;

use serde_json::Value;

use crate::embedding::backfill_store::EmbeddingBackfillStoreClient;
use crate::error::MemoryError;
use crate::storage::DbClient;

/// Exercises the embedding context's missing-vector selection against a real
/// storage adapter from an integration test.
///
/// This fixture is available only with `test-fixtures`; it does not expose SQL
/// or become part of the MCP or application-facing API.
pub struct BackfillQueryFixture {
    store: EmbeddingBackfillStoreClient,
}

impl BackfillQueryFixture {
    pub fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            store: EmbeddingBackfillStoreClient::new(db, namespace),
        }
    }

    pub async fn select_missing(
        &self,
        last_completed_fact_id: Option<&str>,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        self.store
            .select_facts_missing_embeddings(last_completed_fact_id, limit)
            .await
    }

    pub async fn count_missing(&self) -> Result<usize, MemoryError> {
        self.store.count_facts_missing_embeddings().await
    }
}
