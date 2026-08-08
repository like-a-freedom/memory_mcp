//! Concrete fact store: owns fact persistence queries without exposing the
//! full `DbClient` surface.
//!
//! Replaces the formerly implicit `DbClient` consumption in
//! `service/fact.rs`.

use std::sync::Arc;

use serde_json::Value;

use crate::service::MemoryError;
use crate::storage::DbClient;

/// Narrow store for fact CRUD.
#[derive(Clone)]
pub struct FactStoreClient {
    db: Arc<dyn DbClient>,
}

impl FactStoreClient {
    pub fn new(db: Arc<dyn DbClient>) -> Self {
        Self { db }
    }

    /// Returns the persisted record for `fact_id`, or `None` if absent.
    pub async fn select_one(
        &self,
        fact_id: &str,
        namespace: &str,
    ) -> Result<Option<Value>, MemoryError> {
        self.db.select_one(fact_id, namespace).await
    }

    /// Persists a new fact record. Returns `Value::Null` on success.
    pub async fn create(
        &self,
        fact_id: &str,
        content: Value,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        self.db.create(fact_id, content, namespace).await
    }
}
