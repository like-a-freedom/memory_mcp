//! Concrete fact store: owns fact persistence queries without exposing the
//! full `DbClient` surface.
//!
//! Replaces the formerly implicit `DbClient` consumption in
//! `service/fact.rs`.

use std::sync::Arc;

use serde_json::Value;

use crate::service::MemoryError;
use crate::storage::{BoundDbClient, DbClient};

/// Narrow store for fact CRUD.
#[derive(Clone)]
pub struct FactStoreClient {
    db: BoundDbClient,
}

impl FactStoreClient {
    pub fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            db: BoundDbClient::new(db, namespace),
        }
    }

    /// Returns the persisted record for `fact_id`, or `None` if absent.
    pub async fn select_one(&self, fact_id: &str) -> Result<Option<Value>, MemoryError> {
        self.db.select_one(fact_id).await
    }

    /// Persists a new fact record. Returns `Value::Null` on success.
    pub async fn create(&self, fact_id: &str, content: Value) -> Result<Value, MemoryError> {
        self.db.create(fact_id, content).await
    }

    /// Updates a fact record in the Active Namespace.
    pub async fn update(&self, fact_id: &str, content: Value) -> Result<Value, MemoryError> {
        self.db.update(fact_id, content).await
    }
}
