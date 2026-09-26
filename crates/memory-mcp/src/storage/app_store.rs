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

    pub fn from_bound(db: BoundDbClient) -> Self {
        Self { db }
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

    /// Hard-deletes a stale community record.
    ///
    /// This is the only sanctioned hard delete in the codebase: community
    /// records are derived artifacts rebuilt from active edges, so removing
    /// them does not break the bi-temporal audit trail.
    /// The id must be a `community:` record; anything else is rejected.
    pub async fn delete_community(&self, community_id: &str) -> Result<Value, MemoryError> {
        if !community_id.starts_with("community:") {
            return Err(MemoryError::Validation(format!(
                "delete_community expects a 'community:' record id, got '{community_id}'"
            )));
        }
        self.db
            .query(
                "DELETE type::record($record_id);",
                Some(json!({"record_id": community_id})),
            )
            .await
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

    /// Increments fact access metadata without exposing record mutation details
    /// to retrieval or explanation orchestration.
    pub async fn record_fact_access(&self, fact_id: &str, boost: i64) -> Result<(), MemoryError> {
        crate::storage::helpers::require_record_kind(fact_id, "fact")?;
        let record = self.db.select_one(fact_id).await?;
        let Some(mut record) = record.and_then(|value| value.as_object().cloned()) else {
            return Ok(());
        };

        let access_count = record
            .get("access_count")
            .and_then(crate::service::value_helpers::json_i64)
            .unwrap_or(0)
            .saturating_add(boost);
        record.insert("access_count".to_string(), json!(access_count));
        record.insert(
            "last_accessed".to_string(),
            json!(crate::service::normalize_dt(crate::service::now())),
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

    use super::AppStoreClient;
    use crate::service::MemoryError;
    use crate::service::mock_db::MockDbClient;

    #[tokio::test]
    async fn record_fact_access_rejects_invalid_record_ids() {
        let store = AppStoreClient::new(Arc::new(MockDbClient::new()), "org");

        for bad_id in ["bare-hex-id", "", "episode:xyz"] {
            let result = store.record_fact_access(bad_id, 1).await;
            assert!(
                matches!(result, Err(MemoryError::Validation(_))),
                "expected validation error for '{bad_id}'"
            );
        }
    }

    #[tokio::test]
    async fn delete_community_rejects_non_community_record_ids() {
        let store = AppStoreClient::new(Arc::new(MockDbClient::new()), "org");

        for bad_id in ["fact:abc", "episode:xyz", "community", ""] {
            let result = store.delete_community(bad_id).await;
            assert!(
                matches!(result, Err(MemoryError::Validation(_))),
                "expected validation error for '{bad_id}'"
            );
        }
    }
}
