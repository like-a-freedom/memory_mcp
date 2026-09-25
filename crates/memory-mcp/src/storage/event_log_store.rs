//! Platform-owned store for the process event log.
//!
//! The event log records operational events for a namespace. It
//! is not knowledge or memory data, so it does not belong on a
//! domain store. This store deliberately exposes only its own
//! table: it offers no generic table selector, so a caller cannot
//! reach another owner's table through it.

use std::sync::Arc;

use serde_json::Value;

use crate::error::MemoryError;
use crate::storage::{BoundDbClient, DbClient};

pub struct EventLogStoreClient {
    db: BoundDbClient,
}

impl EventLogStoreClient {
    pub fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            db: BoundDbClient::new(db, namespace),
        }
    }

    /// Every event recorded in the bound Active Namespace.
    pub async fn select_event_log(&self) -> Result<Vec<Value>, MemoryError> {
        self.db.select_table("event_log").await
    }
}
