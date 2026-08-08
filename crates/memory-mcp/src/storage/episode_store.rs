//! Concrete store for the episode domain: episode reads/writes plus the
//! community/entity lookups community helpers use.
//!
//! Replaces direct `DbClient` consumption in `service/episode/` per
//! The store owns its queries; SQL for episode-domain reads lives here
//! (ADR-0027) rather than on the universal `DbClient`.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::service::MemoryError;
use crate::storage::helpers::is_missing_table_error;
use crate::storage::queries::build_fact_visibility_clause;
use crate::storage::{DbClient, GraphDirection};

#[derive(Clone)]
pub struct EpisodeStoreClient {
    db: Arc<dyn DbClient>,
}

impl EpisodeStoreClient {
    pub fn new(db: Arc<dyn DbClient>) -> Self {
        Self { db }
    }

    pub async fn select_one(
        &self,
        record_id: &str,
        namespace: &str,
    ) -> Result<Option<Value>, MemoryError> {
        self.db.select_one(record_id, namespace).await
    }

    pub async fn create(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        self.db.create(record_id, content, namespace).await
    }

    pub async fn update(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        self.db.update(record_id, content, namespace).await
    }

    pub async fn query(
        &self,
        sql: &str,
        vars: Option<Value>,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        self.db.query(sql, vars, namespace).await
    }

    /// Neighbors around a graph node within a namespace.
    pub async fn select_edge_neighbors(
        &self,
        namespace: &str,
        node_id: &str,
        cutoff: &str,
        direction: GraphDirection,
    ) -> Result<Vec<Value>, MemoryError> {
        let (sql, vars) =
            crate::storage::queries::build_select_edge_neighbors_query(node_id, cutoff, direction);
        match self.db.query(&sql, Some(vars), namespace).await {
            Ok(value) => Ok(value.as_array().cloned().unwrap_or_default()),
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => Ok(vec![]),
            Err(err) => Err(err),
        }
    }

    /// Entities matching a set of canonical id strings.
    pub async fn select_entities_by_ids(
        &self,
        namespace: &str,
        entity_ids: &[String],
    ) -> Result<Vec<Value>, MemoryError> {
        if entity_ids.is_empty() {
            return Ok(Vec::new());
        }
        let sql = "SELECT * FROM entity WHERE entity_id IN $entity_ids";
        let vars = json!({ "entity_ids": entity_ids });
        match self.db.query(sql, Some(vars), namespace).await {
            Ok(value) => Ok(value.as_array().cloned().unwrap_or_default()),
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => Ok(vec![]),
            Err(err) => Err(err),
        }
    }

    /// Active (not-yet-invalidated) facts linked to an episode.
    pub async fn select_active_facts_by_episode(
        &self,
        namespace: &str,
        episode_id: &str,
        cutoff: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        let visibility = build_fact_visibility_clause("$cutoff");
        let sql = format!(
            "SELECT * FROM fact WHERE source_episode = $episode_id AND {visibility} LIMIT $limit"
        );
        let vars = json!({ "episode_id": episode_id, "cutoff": cutoff, "limit": limit });
        match self.db.query(&sql, Some(vars), namespace).await {
            Ok(value) => Ok(value.as_array().cloned().unwrap_or_default()),
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => Ok(vec![]),
            Err(err) => Err(err),
        }
    }

    /// Communities containing any of the listed member entities.
    pub async fn select_communities_by_member_entities(
        &self,
        namespace: &str,
        member_entities: &[String],
    ) -> Result<Vec<Value>, MemoryError> {
        let (sql, vars) =
            crate::storage::queries::build_select_communities_by_member_entities_query(
                member_entities,
            );
        match self.db.query(&sql, Some(vars), namespace).await {
            Ok(value) => Ok(value.as_array().cloned().unwrap_or_default()),
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => Ok(vec![]),
            Err(err) => Err(err),
        }
    }

    /// Edges whose `in`/`out`/`relation` matches this triple query.
    ///
    /// Used for targeted invalidation without full table scans.
    pub async fn select_edges_for_triple(
        &self,
        namespace: &str,
        in_id: &str,
        relation: &str,
        out_id: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        let sql = "SELECT * FROM edge WHERE in = <record> $in_id AND relation = $relation \
                   AND out = <record> $out_id";
        let vars = json!({ "in_id": in_id, "relation": relation, "out_id": out_id });
        match self.db.query(sql, Some(vars), namespace).await {
            Ok(value) => Ok(value.as_array().cloned().unwrap_or_default()),
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => Ok(vec![]),
            Err(err) => Err(err),
        }
    }

    /// Link two records through an edge.
    pub async fn relate_edge(
        &self,
        edge_id: &str,
        from_id: &str,
        to_id: &str,
        content: Value,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        let (sql, vars) =
            crate::storage::queries::build_relate_edge_query(edge_id, from_id, to_id, content);
        match self.db.query(&sql, Some(vars), namespace).await {
            Ok(value) => Ok(value
                .as_array()
                .and_then(|rows| rows.first().cloned())
                .unwrap_or(Value::Null)),
            Err(err) => Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use crate::service::normalize_dt;
    use crate::storage::{DbClient, SurrealDbClient, episode_store::EpisodeStoreClient};

    async fn make_db() -> Arc<SurrealDbClient> {
        let db_name = format!(
            "episode_store_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(
                &db_name,
                &["org".to_string()],
                "warn",
            )
            .await
            .expect("connect in memory db"),
        );
        db_client
            .apply_migrations("org")
            .await
            .expect("apply migrations");
        db_client
    }

    async fn seed_fact(db_client: &Arc<SurrealDbClient>, fact_id: &str, invalidated: bool) {
        let now = normalize_dt(chrono::Utc::now());
        let embedding = vec![0.1f64; 1536];
        db_client
            .create(
                fact_id,
                json!({
                    "fact_id": fact_id,
                    "fact_type": "note",
                    "content": format!("content {fact_id}"),
                    "quote": format!("content {fact_id}"),
                    "source_episode": "episode:seed",
                    "t_valid": now,
                    "t_ingested": now,
                    "confidence": 0.9,
                    "index_keys": [],
                    "access_count": 0,
                    "entity_links": [],
                    "scope": "org",
                    "policy_tags": [],
                    "provenance": {"source_episode": "episode:seed"},
                    "embedding": embedding,
                    "embedding_provider": "legacy-test",
                    "embedding_model": "legacy-model",
                    "embedding_dimension": 1536,
                    "embedding_signature": Some("embsig:test"),
                    "embedding_updated_at": now,
                    "t_invalid": if invalidated {
                        Some(now)
                    } else {
                        None
                    },
                }),
                "org",
            )
            .await
            .expect("seed fact should succeed");
    }

    #[tokio::test]
    async fn active_facts_by_episode_returns_empty_when_fact_table_missing() {
        let db_name = format!(
            "episode_store_unmigrated_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(
                &db_name,
                &["org".to_string()],
                "warn",
            )
            .await
            .expect("connect in memory db"),
        );
        let store = EpisodeStoreClient::new(db_client.clone());
        let cutoff = normalize_dt(chrono::Utc::now());

        let facts = store
            .select_active_facts_by_episode("org", "episode:seed", &cutoff, 10)
            .await
            .expect("must not error on missing table");
        assert!(facts.is_empty());
    }

    #[tokio::test]
    async fn active_facts_by_episode_filters_invalidated_and_limits() {
        let db_client = make_db().await;
        seed_fact(&db_client, "fact:1", false).await;
        seed_fact(&db_client, "fact:2", true).await;
        seed_fact(&db_client, "fact:3", false).await;
        let store = EpisodeStoreClient::new(db_client.clone());
        let cutoff = normalize_dt(chrono::Utc::now() + chrono::Duration::seconds(1));

        let facts = store
            .select_active_facts_by_episode("org", "episode:seed", &cutoff, 2)
            .await
            .expect("select active facts");
        let ids: Vec<&str> = facts
            .iter()
            .filter_map(|record| record.get("fact_id").and_then(|v| v.as_str()))
            .collect();
        // Invalidated fact:2 excluded; limit 2 caps the page.
        assert_eq!(ids, vec!["fact:1", "fact:3"]);
    }
}
