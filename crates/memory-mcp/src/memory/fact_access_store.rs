//! Memory-owned fact access log.
//!
//! The access log records retrieval heat for a fact: how often it
//! was served and when it was last read. It is memory's, because
//! memory performs the retrieval that produces the heat, so it
//! lives apart from the knowledge-owned graph reads it used to be
//! bundled with.
//!
//! The method bodies are unchanged from `app_store.rs`.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::error::MemoryError;
use crate::storage::{BoundDbClient, DbClient};

/// Read-side store for the fact access log.
#[derive(Clone)]
pub struct FactAccessStore {
    db: BoundDbClient,
}

impl FactAccessStore {
    pub fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            db: BoundDbClient::new(db, namespace),
        }
    }

    pub fn from_bound(db: BoundDbClient) -> Self {
        Self { db }
    }

    /// Increments fact access metadata without exposing record mutation details
    /// to retrieval or explanation orchestration.
    pub async fn record_fact_access(&self, fact_id: &str, boost: i64) -> Result<(), MemoryError> {
        crate::storage::require_record_kind(fact_id, "fact")?;
        let record = self.db.select_one(fact_id).await?;
        let Some(mut record) = record.and_then(|value| value.as_object().cloned()) else {
            return Ok(());
        };

        let access_count = record
            .get("access_count")
            .and_then(crate::storage::value_helpers::json_i64)
            .unwrap_or(0)
            .saturating_add(boost);
        record.insert("access_count".to_string(), json!(access_count));
        record.insert(
            "last_accessed".to_string(),
            json!(crate::shared::temporal::normalize_dt(
                crate::shared::temporal::now()
            )),
        );

        self.db.update(fact_id, Value::Object(record)).await?;
        Ok(())
    }

    /// Whether any fact linked to `episode_id` was accessed at or after
    /// `hot_cutoff`.
    pub async fn has_recent_fact_access(
        &self,
        episode_id: &str,
        hot_cutoff: &str,
    ) -> Result<bool, MemoryError> {
        let rows = self
            .db
            .query_rows(
                "SELECT fact_id FROM fact \
                 WHERE source_episode = $episode_id \
                 AND last_accessed IS NOT NONE \
                 AND last_accessed >= type::datetime($hot_cutoff) LIMIT 1",
                Some(json!({ "episode_id": episode_id, "hot_cutoff": hot_cutoff })),
            )
            .await?;
        Ok(!rows.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::FactAccessStore;
    use crate::error::MemoryError;
    use crate::service::mock_db::MockDbClient;

    #[tokio::test]
    async fn record_fact_access_rejects_invalid_record_ids() {
        let store = FactAccessStore::new(Arc::new(MockDbClient::new()), "org");

        for bad_id in ["bare-hex-id", "", "episode:xyz"] {
            let result = store.record_fact_access(bad_id, 1).await;
            assert!(
                matches!(result, Err(MemoryError::Validation(_))),
                "expected validation error for '{bad_id}'"
            );
        }
    }
}
