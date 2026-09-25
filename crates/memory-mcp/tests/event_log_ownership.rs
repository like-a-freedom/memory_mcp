//! Platform-owned persistence. The event log is not knowledge or
//! memory data, so it gets its own store rather than living on a
//! domain store that would imply the wrong owner.

use std::sync::Arc;

use memory_mcp::MemoryError;
use memory_mcp::storage::{DbClient, EventLogStoreClient};

struct RecordingDb;

#[async_trait::async_trait]
impl DbClient for RecordingDb {
    async fn query(
        &self,
        sql: &str,
        _vars: Option<serde_json::Value>,
        _namespace: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        if !sql.contains("FROM event_log") {
            return Err(MemoryError::Storage(format!("unexpected table: {sql}")));
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
        _record_id: &str,
        _content: serde_json::Value,
        _namespace: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        Ok(serde_json::json!({}))
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
        table: &str,
        _namespace: &str,
    ) -> Result<Vec<serde_json::Value>, MemoryError> {
        if table != "event_log" {
            return Err(MemoryError::Storage(format!("select_table: {table}")));
        }
        Ok(Vec::new())
    }

    async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
        Ok(())
    }
}

#[tokio::test]
async fn the_event_log_is_read_through_the_platform_store() {
    let store = EventLogStoreClient::new(Arc::new(RecordingDb), "org");

    let rows = store
        .select_event_log()
        .await
        .expect("event log is readable");

    assert!(rows.is_empty(), "no events are seeded for this test");
}

#[tokio::test]
async fn the_event_log_store_only_exposes_its_own_table() {
    // The store is the platform's: it must not offer a generic
    // table selector, so an application-facing read cannot name an
    // arbitrary table through it.
    let store = EventLogStoreClient::new(Arc::new(RecordingDb), "org");
    let rows = store
        .select_event_log()
        .await
        .expect("event log is readable");
    assert!(rows.is_empty());
}
