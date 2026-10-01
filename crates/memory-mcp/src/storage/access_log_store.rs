//! Write-side access-log store.
//!
//! The `query_log` table is a technical concern of the retrieval
//! pipeline rather than a bounded-context record, so it keeps its own
//! narrow store. Ownership is limited to its two operations instead
//! of forwarding `create`/`query` to `DbClient` through a trait.
//!
//! This was previously the lower half of `context_store.rs`, which
//! mixed it with the read-side context assembly queries; the read
//! side is now split by canonical owner into `knowledge_store.rs`
//! and `episode_context_store.rs`.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::error::MemoryError;
use crate::storage::{BoundDbClient, DbClient};

/// Write-side store for the `query_log` table.
#[derive(Clone)]
pub struct ContextAccessLogClient {
    db: BoundDbClient,
}

impl ContextAccessLogClient {
    pub fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            db: BoundDbClient::new(db, namespace),
        }
    }

    pub async fn create(
        &self,
        record_id: &str,
        content: Value,
        temporal_fields: &[&str],
    ) -> Result<Value, MemoryError> {
        self.db.create(record_id, content, temporal_fields).await
    }

    /// Deletes query-log rows older than `cutoff` and returns how many were
    /// removed (the retention DELETE lives in the owning store).
    pub async fn prune_expired_logs(&self, cutoff: &str) -> Result<usize, MemoryError> {
        let deleted = self
            .db
            .query(
                "DELETE query_log WHERE logged_at IS NOT NONE \
                 AND type::datetime(logged_at) < type::datetime($cutoff) RETURN BEFORE",
                Some(json!({ "cutoff": cutoff })),
            )
            .await?;
        Ok(deleted.as_array().map_or(0, Vec::len))
    }
}
