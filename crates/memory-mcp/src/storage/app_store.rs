//! Concrete app store: owns the queries that MCP apps, lifecycle, and graph
//! expansion need, without exposing the full `DbClient` surface.
//!
//! Replaces the `AppStore` trait seam per ADR-0024 step 4.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::service::MemoryError;
use crate::storage::helpers::is_missing_table_error;
use crate::storage::{DbClient, GraphDirection};

/// Concrete store for app-facing graph + entity + record reads and mutations.
///
/// Unlike the removed `AppStore` trait this is not an interface: it's a real
/// struct that owns its queries. Callers get this — not a trait object — so
/// the call graph is visible without an opaqueness layer.
#[derive(Clone)]
pub struct AppStoreClient {
    db: Arc<dyn DbClient>,
}

impl AppStoreClient {
    pub fn new(db: Arc<dyn DbClient>) -> Self {
        Self { db }
    }

    pub async fn select_entities(&self, namespace: &str) -> Result<Vec<Value>, MemoryError> {
        self.db.select_table("entity", namespace).await
    }

    pub async fn select_entity(
        &self,
        entity_id: &str,
        namespace: &str,
    ) -> Result<Option<Value>, MemoryError> {
        self.db.select_one(entity_id, namespace).await
    }

    pub async fn select_communities(&self, namespace: &str) -> Result<Vec<Value>, MemoryError> {
        self.db.select_table("community", namespace).await
    }

    pub async fn select_facts(&self, namespace: &str) -> Result<Vec<Value>, MemoryError> {
        self.db.select_table("fact", namespace).await
    }

    pub async fn select_edge(
        &self,
        edge_id: &str,
        namespace: &str,
    ) -> Result<Option<Value>, MemoryError> {
        self.db.select_one(edge_id, namespace).await
    }

    pub async fn select_graph_neighbors(
        &self,
        namespace: &str,
        node_id: &str,
        cutoff: &str,
        direction: GraphDirection,
    ) -> Result<Vec<Value>, MemoryError> {
        self.db
            .select_edge_neighbors(namespace, node_id, cutoff, direction)
            .await
    }

    pub async fn select_entity_lookup(
        &self,
        namespace: &str,
        normalized_name: &str,
    ) -> Result<Option<Value>, MemoryError> {
        // Canonical-name index lookup first (fast path), then alias lookup.
        let canonical_sql = "SELECT * FROM entity WHERE canonical_name_normalized = $name LIMIT 1";
        let canonical_result = match self
            .db
            .query(
                canonical_sql,
                Some(json!({ "name": normalized_name })),
                namespace,
            )
            .await
        {
            Ok(value) => value.as_array().and_then(|arr| arr.first()).cloned(),
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => None,
            Err(err) => return Err(err),
        };

        if canonical_result.is_some() {
            return Ok(canonical_result);
        }

        let alias_sql = "SELECT * FROM entity WHERE aliases CONTAINS $name LIMIT 1";
        match self
            .db
            .query(
                alias_sql,
                Some(json!({ "name": normalized_name })),
                namespace,
            )
            .await
        {
            Ok(value) => Ok(value.as_array().and_then(|arr| arr.first()).cloned()),
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => Ok(None),
            Err(err) => Err(err),
        }
    }

    pub async fn select_active_facts(
        &self,
        namespace: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        let (sql, vars) = crate::storage::queries::build_select_active_facts_query(
            &crate::service::normalize_dt(crate::service::now()),
            limit,
        );
        match self.db.query(&sql, Some(vars), namespace).await {
            Ok(value) => Ok(value.as_array().cloned().unwrap_or_default()),
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => Ok(vec![]),
            Err(err) => Err(err),
        }
    }

    pub async fn select_episodes_for_archival(
        &self,
        namespace: &str,
        cutoff: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        let sql = "SELECT * FROM episode WHERE status != 'archived' \
                   AND t_ref < type::datetime($cutoff) ORDER BY t_ref ASC LIMIT $limit";
        let vars = json!({ "cutoff": cutoff, "limit": limit });
        match self.db.query(sql, Some(vars), namespace).await {
            Ok(value) => Ok(value.as_array().cloned().unwrap_or_default()),
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => Ok(vec![]),
            Err(err) => Err(err),
        }
    }

    pub async fn update_record(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        self.db.update(record_id, content, namespace).await
    }
}
