//! Concrete store for the episode domain: episode reads/writes plus the
//! community/entity lookups community helpers use.
//!
//! Replaces direct `DbClient` consumption in `service/episode/`.
//! The store owns its queries; SQL for episode-domain reads lives here
//! rather than on the universal `DbClient`.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::service::MemoryError;
use crate::storage::queries::build_fact_visibility_clause;
use crate::storage::{BoundDbClient, DbClient, GraphDirection};

#[derive(Clone)]
pub struct EpisodeStoreClient {
    db: BoundDbClient,
}

impl EpisodeStoreClient {
    pub fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            db: BoundDbClient::new(db, namespace),
        }
    }

    pub async fn select_one(&self, record_id: &str) -> Result<Option<Value>, MemoryError> {
        self.db.select_one(record_id).await
    }

    /// Returns the total number of episodes in the bound Active Namespace.
    pub async fn count_episodes(&self) -> Result<i32, MemoryError> {
        let result = self
            .db
            .query("SELECT count() FROM episode GROUP ALL", None)
            .await?;
        Ok(result
            .as_array()
            .and_then(|rows| rows.first())
            .and_then(|row| row.get("count"))
            .and_then(Value::as_i64)
            .unwrap_or(0) as i32)
    }

    pub async fn create(&self, record_id: &str, content: Value) -> Result<Value, MemoryError> {
        self.db.create(record_id, content).await
    }

    /// Create an episode and publish its app invalidation in the same tenant
    /// transaction. This method is used only by the HTTP runtime; stdio keeps
    /// the historical direct-create path because its namespace has no outbox
    /// contract.
    #[cfg(feature = "streamable-http")]
    pub(crate) async fn create_with_event(
        &self,
        record_id: &str,
        content: Value,
        resource_id: &str,
    ) -> Result<(), MemoryError> {
        let Some((table, key)) = record_id.split_once(':') else {
            return Err(MemoryError::Validation(
                "episode record id must include a table prefix".into(),
            ));
        };
        if table != "episode" || key.is_empty() {
            return Err(MemoryError::Validation(
                "episode record id has an invalid table prefix".into(),
            ));
        }
        let object = content.as_object().ok_or_else(|| {
            MemoryError::Validation("episode content must be a JSON object".into())
        })?;
        let required_string = |field: &str| {
            object
                .get(field)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| MemoryError::Validation(format!("episode content needs {field}")))
        };
        let episode_id = required_string("episode_id")?;
        let source_type = required_string("source_type")?;
        let source_id = required_string("source_id")?;
        let episode_text = required_string("content")?;
        let t_ref = required_string("t_ref")?;
        let t_ingested = required_string("t_ingested")?;
        let policy_tags = object
            .get("policy_tags")
            .cloned()
            .ok_or_else(|| MemoryError::Validation("episode content needs policy_tags".into()))?;
        let source_lineage = object.get("source_lineage").cloned();
        let source_lineage_assignment = if source_lineage.is_some() {
            ", source_lineage = $source_lineage"
        } else {
            ""
        };
        let mutation = crate::http::subscriptions::outbox::TenantMutation::new(
            format!(
                "CREATE type::record('episode', $episode_key) SET \
                 episode_id = $episode_id, source_type = $source_type, source_id = $source_id, \
                 content = $episode_text, t_ref = type::datetime($t_ref), \
                 t_ingested = type::datetime($t_ingested), policy_tags = $policy_tags{source_lineage_assignment}"
            ),
            json!({
                "episode_key": key,
                "episode_id": episode_id,
                "source_type": source_type,
                "source_id": source_id,
                "episode_text": episode_text,
                "t_ref": t_ref,
                "t_ingested": t_ingested,
                "policy_tags": policy_tags,
                "source_lineage": source_lineage,
            }),
        )?;
        crate::http::subscriptions::outbox::commit_tenant_mutation_with_event(
            &self.db,
            mutation,
            crate::http::subscriptions::outbox::TenantChangeEvent {
                sequence: 0,
                resource_id: resource_id.to_owned(),
                revision: 1,
                change_kind: "episode_created".into(),
                created_at: chrono::Utc::now(),
            },
        )
        .await
    }

    pub async fn update(&self, record_id: &str, content: Value) -> Result<Value, MemoryError> {
        self.db.update(record_id, content).await
    }

    /// Persists an entity extraction projection row (the CREATE
    /// statement lives in the owning store, not in the service layer).
    ///
    /// `record_body` is the two-part `episode-key:projection-suffix` body;
    /// `⟨...⟩` keeps it a single id string. `type::datetime(...)` mirrors the
    /// query builder's temporal-field handling: SurrealDB does not coerce
    /// RFC3339 strings into `datetime`-typed schema fields.
    pub async fn create_extraction_projection(
        &self,
        record_body: &str,
        vars: Value,
    ) -> Result<(), MemoryError> {
        let sql = format!(
            "CREATE entity_extraction_projection:⟨{record_body}⟩ SET \
             episode_id = $episode_id, \
             t_ingested = type::datetime($t_ingested), t_created = type::datetime($t_created), \
             fingerprint = $fingerprint, entity_ids = $entity_ids RETURN *"
        );
        self.db.query(&sql, Some(vars)).await?;
        Ok(())
    }

    /// Finds episodes by the stable source identity used by both legacy and
    /// scope-free episode IDs. The caller decides how many matches are safe.
    pub async fn select_by_source_identity(
        &self,
        source_type: &str,
        source_id: &str,
        t_ref: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        let sql = "SELECT * FROM episode WHERE source_type = $source_type AND source_id = $source_id AND t_ref = type::datetime($t_ref) ORDER BY episode_id ASC LIMIT $limit";
        let vars = json!({
            "source_type": source_type,
            "source_id": source_id,
            "t_ref": t_ref,
            "limit": limit,
        });
        self.db.query_rows(sql, Some(vars)).await
    }

    /// Neighbors around a graph node in the Active Namespace.
    pub async fn select_edge_neighbors(
        &self,
        node_id: &str,
        cutoff: &str,
        direction: GraphDirection,
    ) -> Result<Vec<Value>, MemoryError> {
        let (sql, vars) =
            crate::storage::queries::build_select_edge_neighbors_query(node_id, cutoff, direction);
        self.db.query_rows(&sql, Some(vars)).await
    }

    /// Entities matching a set of canonical id strings.
    pub async fn select_entities_by_ids(
        &self,
        entity_ids: &[String],
    ) -> Result<Vec<Value>, MemoryError> {
        if entity_ids.is_empty() {
            return Ok(Vec::new());
        }
        let sql = "SELECT * FROM entity WHERE entity_id IN $entity_ids";
        let vars = json!({ "entity_ids": entity_ids });
        self.db.query_rows(sql, Some(vars)).await
    }

    /// Active (not-yet-invalidated) facts linked to an episode.
    pub async fn select_active_facts_by_episode(
        &self,
        episode_id: &str,
        cutoff: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        let visibility = build_fact_visibility_clause("$cutoff");
        let sql = format!(
            "SELECT * FROM fact WHERE source_episode = $episode_id AND {visibility} LIMIT $limit"
        );
        let vars = json!({ "episode_id": episode_id, "cutoff": cutoff, "limit": limit });
        self.db.query_rows(&sql, Some(vars)).await
    }

    /// Communities containing any of the listed member entities.
    pub async fn select_communities_by_member_entities(
        &self,
        member_entities: &[String],
    ) -> Result<Vec<Value>, MemoryError> {
        let (sql, vars) =
            crate::storage::queries::build_select_communities_by_member_entities_query(
                member_entities,
            );
        self.db.query_rows(&sql, Some(vars)).await
    }

    /// Edges whose `in`/`out`/`relation` matches this triple query.
    ///
    /// Used for targeted invalidation without full table scans.
    pub async fn select_edges_for_triple(
        &self,
        in_id: &str,
        relation: &str,
        out_id: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        let sql = "SELECT * FROM edge WHERE in = <record> $in_id AND relation = $relation \
                   AND out = <record> $out_id";
        let vars = json!({ "in_id": in_id, "relation": relation, "out_id": out_id });
        self.db.query_rows(sql, Some(vars)).await
    }

    /// Link two records through an edge in the Active Namespace.
    pub async fn relate_edge(
        &self,
        edge_id: &str,
        from_id: &str,
        to_id: &str,
        content: Value,
    ) -> Result<Value, MemoryError> {
        let (sql, vars) =
            crate::storage::queries::build_relate_edge_query(edge_id, from_id, to_id, content);
        match self.db.query(&sql, Some(vars)).await {
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
        #[cfg(feature = "streamable-http")]
        db_client
            .execute_migration_script(
                include_str!("../../migrations/042_tenant_change_event.surql"),
                "org",
            )
            .await
            .expect("apply HTTP outbox migration");
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

    #[cfg(feature = "streamable-http")]
    #[tokio::test]
    async fn create_with_event_commits_episode_and_invalidation_atomically() {
        let db_client = make_db().await;
        let store = EpisodeStoreClient::new(db_client.clone(), "org");
        store
            .create_with_event(
                "episode:outbox_test",
                json!({
                    "episode_id": "episode:outbox_test",
                    "source_type": "inline",
                    "source_id": "outbox_test",
                    "content": "hello",
                    "policy_tags": [],
                    "t_ref": chrono::Utc::now().to_rfc3339(),
                    "t_ingested": chrono::Utc::now().to_rfc3339(),
                }),
                "ui://memory/apps/ingestion_review",
            )
            .await
            .expect("episode and event commit");

        let episode = db_client
            .select_one("episode:outbox_test", "org")
            .await
            .expect("select episode")
            .expect("episode exists");
        assert_eq!(episode["content"], "hello");
        let events = db_client
            .query(
                "SELECT * FROM tenant_change_event WHERE resource_id = $resource",
                Some(json!({"resource": "ui://memory/apps/ingestion_review"})),
                "org",
            )
            .await
            .expect("select event");
        let events: Vec<serde_json::Value> = serde_json::from_value(events).expect("parse events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["change_kind"], "episode_created");
    }

    #[tokio::test]
    async fn count_episodes_returns_zero_for_empty_store() {
        let db_client = make_db().await;
        let store = EpisodeStoreClient::new(db_client, "org");

        let count = store.count_episodes().await.expect("count episodes");

        assert_eq!(count, 0);
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
        let store = EpisodeStoreClient::new(db_client.clone(), "org");
        let cutoff = normalize_dt(chrono::Utc::now());

        let facts = store
            .select_active_facts_by_episode("episode:seed", &cutoff, 10)
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
        let store = EpisodeStoreClient::new(db_client.clone(), "org");
        let cutoff = normalize_dt(chrono::Utc::now() + chrono::Duration::seconds(1));

        let facts = store
            .select_active_facts_by_episode("episode:seed", &cutoff, 2)
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
