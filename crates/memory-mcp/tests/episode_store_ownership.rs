//! Episode archival selection is memory-owned: the episode store
//! answers it with an explicitly episode-scoped surface, not a
//! table-agnostic record helper on a shared store.

use std::sync::Arc;

use memory_mcp::MemoryError;
use memory_mcp::storage::{DbClient, EpisodeStoreClient};

struct RecordingDb;

#[async_trait::async_trait]
impl DbClient for RecordingDb {
    async fn query(
        &self,
        sql: &str,
        _vars: Option<serde_json::Value>,
        _namespace: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        if !sql.contains("FROM episode") {
            return Err(MemoryError::Storage(format!("unexpected table in: {sql}")));
        }
        Ok(serde_json::json!([]))
    }

    async fn create(
        &self,
        _record_id: &str,
        _content: serde_json::Value,
        _namespace: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        Ok(serde_json::json!({}))
    }

    async fn update(
        &self,
        record_id: &str,
        _content: serde_json::Value,
        _namespace: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        Err(MemoryError::Storage(format!("update: {record_id}")))
    }

    async fn select_one(
        &self,
        _record_id: &str,
        _namespace: &str,
    ) -> Result<Option<serde_json::Value>, MemoryError> {
        Ok(None)
    }

    async fn select_table(
        &self,
        table: memory_mcp::storage::table_scope::OwnedTable,
        _namespace: &str,
    ) -> Result<Vec<serde_json::Value>, MemoryError> {
        Err(MemoryError::Storage(format!("select_table: {table}")))
    }

    async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
        Ok(())
    }
}

#[tokio::test]
async fn archival_candidates_are_read_through_the_episode_store() {
    let store = EpisodeStoreClient::new(Arc::new(RecordingDb), "org");

    let rows = store
        .select_episodes_for_archival("2026-01-01T00:00:00Z", 100)
        .await
        .expect("the episode store answers its own archival query");

    assert!(rows.is_empty(), "no episodes are seeded for this fake");
}

#[tokio::test]
async fn an_episode_update_refuses_a_non_episode_record_id() {
    let store = EpisodeStoreClient::new(Arc::new(RecordingDb), "org");

    // The generic record helper derived its table from the id
    // string, so `fact:...` was silently accepted here. An
    // episode-scoped write must refuse it.
    let error = store
        .update_episode("fact:abc", serde_json::json!({}))
        .await
        .expect_err("a fact id must not be written as an episode");

    assert!(
        matches!(&error, MemoryError::Validation(message) if message.contains("fact:abc")),
        "the refusal must name the offending id, got {error:?}"
    );
}

#[tokio::test]
async fn a_bare_id_is_refused_before_any_query_runs() {
    let store = EpisodeStoreClient::new(Arc::new(RecordingDb), "org");

    let error = store
        .update_episode("abc123", serde_json::json!({}))
        .await
        .expect_err("a bare id is not an episode record id");

    assert!(matches!(&error, MemoryError::Validation(_)));
}
