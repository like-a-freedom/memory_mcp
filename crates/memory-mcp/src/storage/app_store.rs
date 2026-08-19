//! Concrete app store: owns the queries that MCP apps, lifecycle, and graph
//! expansion need, without exposing the full `DbClient` surface.
//!
//! Replaces the `AppStore` trait seam.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::service::MemoryError;
use crate::storage::{BoundDbClient, DbClient, GraphDirection};

/// Concrete store for app-facing graph + entity + record reads and mutations.
///
/// Unlike the removed `AppStore` trait this is not an interface: it's a real
/// struct that owns its queries. Callers get this — not a trait object — so
/// the call graph is visible without an opaqueness layer.
#[derive(Clone)]
pub struct AppStoreClient {
    db: BoundDbClient,
}

impl AppStoreClient {
    pub fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            db: BoundDbClient::new(db, namespace),
        }
    }

    pub(crate) fn from_bound(db: BoundDbClient) -> Self {
        Self { db }
    }

    pub async fn select_record(&self, record_id: &str) -> Result<Option<Value>, MemoryError> {
        self.db.select_one(record_id).await
    }

    /// Looks up a validated record in the process-bound Active Namespace.
    pub(crate) async fn find_record_by_id(
        &self,
        record_id: &str,
    ) -> Result<(Option<serde_json::Map<String, Value>>, Option<String>), MemoryError> {
        crate::storage::validate_record_id(record_id)?;
        let record = self.select_record(record_id).await?;
        Ok((
            record.and_then(|value| value.as_object().cloned()),
            Some(self.db.namespace().to_string()),
        ))
    }

    pub async fn select_records(&self, table: &str) -> Result<Vec<Value>, MemoryError> {
        self.db.select_table(table).await
    }

    pub async fn select_entities(&self) -> Result<Vec<Value>, MemoryError> {
        self.db.select_table("entity").await
    }

    pub async fn select_entity(&self, entity_id: &str) -> Result<Option<Value>, MemoryError> {
        self.db.select_one(entity_id).await
    }

    pub async fn select_entities_by_ids(
        &self,
        entity_ids: &[String],
    ) -> Result<Vec<Value>, MemoryError> {
        if entity_ids.is_empty() {
            return Ok(Vec::new());
        }
        let sql = "SELECT entity_id, canonical_name, aliases FROM entity WHERE entity_id IN $ids";
        self.db
            .query_rows(sql, Some(json!({"ids": entity_ids})))
            .await
    }

    pub async fn select_communities(&self) -> Result<Vec<Value>, MemoryError> {
        self.db.select_table("community").await
    }

    pub async fn select_community(&self, community_id: &str) -> Result<Option<Value>, MemoryError> {
        self.db.select_one(community_id).await
    }

    pub async fn upsert_community(
        &self,
        community_id: &str,
        content: Value,
    ) -> Result<(), MemoryError> {
        if self.db.select_one(community_id).await?.is_some() {
            self.db.update(community_id, content).await?;
        } else {
            self.db.create(community_id, content).await?;
        }
        Ok(())
    }

    pub async fn delete_record(&self, record_id: &str) -> Result<Value, MemoryError> {
        self.db
            .query(
                "DELETE type::record($record_id);",
                Some(json!({"record_id": record_id})),
            )
            .await
    }

    pub async fn select_facts(&self) -> Result<Vec<Value>, MemoryError> {
        self.db.select_table("fact").await
    }

    pub async fn select_edge(&self, edge_id: &str) -> Result<Option<Value>, MemoryError> {
        self.db.select_one(edge_id).await
    }

    pub async fn select_graph_neighbors(
        &self,
        node_id: &str,
        cutoff: &str,
        direction: GraphDirection,
    ) -> Result<Vec<Value>, MemoryError> {
        let (sql, vars) =
            crate::storage::queries::build_select_edge_neighbors_query(node_id, cutoff, direction);
        self.db.query_rows(&sql, Some(vars)).await
    }

    /// One page of active edges in stable order.
    pub async fn select_edges_filtered_page(
        &self,
        cutoff: &str,
        start: usize,
        limit: usize,
    ) -> Result<Vec<Value>, MemoryError> {
        let (sql, vars) =
            crate::storage::queries::build_select_edges_filtered_page_query(cutoff, limit, start);
        self.db.query_rows(&sql, Some(vars)).await
    }

    pub async fn select_entity_lookup(
        &self,
        normalized_name: &str,
    ) -> Result<Option<Value>, MemoryError> {
        // Canonical-name index lookup first (fast path), then alias lookup.
        let canonical_sql = "SELECT * FROM entity WHERE canonical_name_normalized = $name LIMIT 1";
        let canonical_result = self
            .db
            .query_first(canonical_sql, Some(json!({ "name": normalized_name })))
            .await?;

        if canonical_result.is_some() {
            return Ok(canonical_result);
        }

        let alias_sql = "SELECT * FROM entity WHERE aliases CONTAINS $name LIMIT 1";
        self.db
            .query_first(alias_sql, Some(json!({ "name": normalized_name })))
            .await
    }

    pub async fn select_active_facts(&self, limit: i32) -> Result<Vec<Value>, MemoryError> {
        let (sql, vars) = crate::storage::queries::build_select_active_facts_query(
            &crate::service::normalize_dt(crate::service::now()),
            limit,
        );
        self.db.query_rows(&sql, Some(vars)).await
    }

    pub async fn select_episodes_for_archival(
        &self,
        cutoff: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        let sql = "SELECT * FROM episode WHERE status != 'archived' \
                   AND t_ref < type::datetime($cutoff) ORDER BY t_ref ASC LIMIT $limit";
        let vars = json!({ "cutoff": cutoff, "limit": limit });
        self.db.query_rows(sql, Some(vars)).await
    }

    pub async fn update_record(
        &self,
        record_id: &str,
        content: Value,
    ) -> Result<Value, MemoryError> {
        self.db.update(record_id, content).await
    }

    pub async fn query(&self, sql: &str, vars: Option<Value>) -> Result<Value, MemoryError> {
        self.db.query(sql, vars).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::AppStoreClient;
    use crate::service::MemoryError;
    use crate::service::mock_db::MockDbClient;

    #[tokio::test]
    async fn find_record_by_id_rejects_invalid_record_ids() {
        let store = AppStoreClient::new(Arc::new(MockDbClient::new()), "org");

        let result = store.find_record_by_id("bare-hex-id").await;

        assert!(matches!(result, Err(MemoryError::Validation(_))));
    }

    #[tokio::test]
    async fn find_record_by_id_returns_record_and_active_namespace() {
        let db = MockDbClient::new().expect_select_one(
            "fact:known",
            Some(json!({"fact_id": "fact:known", "content": "remembered"})),
        );
        let store = AppStoreClient::new(Arc::new(db), "org");

        let (record, namespace) = store
            .find_record_by_id("fact:known")
            .await
            .expect("record lookup should succeed");

        assert_eq!(
            record.and_then(|map| map.get("fact_id").cloned()),
            Some(json!("fact:known"))
        );
        assert_eq!(namespace.as_deref(), Some("org"));
    }
}
