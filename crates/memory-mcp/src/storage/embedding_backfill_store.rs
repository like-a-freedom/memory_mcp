//! Narrow fact store for deferred embedding backfill.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::service::MemoryError;
use crate::storage::{BoundDbClient, DbClient};

const DEFAULT_BACKFILL_BATCH_SIZE: i32 = 100;

#[derive(Clone)]
pub(crate) struct EmbeddingBackfillStoreClient {
    db: BoundDbClient,
}

impl EmbeddingBackfillStoreClient {
    pub(crate) fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            db: BoundDbClient::new(db, namespace),
        }
    }

    pub(crate) async fn count_facts_missing_embeddings(&self) -> Result<usize, MemoryError> {
        let rows = self
            .db
            .query_rows(
                "SELECT count() AS count FROM fact WHERE embedding IS NONE GROUP ALL",
                None,
            )
            .await?;
        Ok(rows
            .first()
            .and_then(|row| row.get("count"))
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(0))
    }

    pub(crate) async fn select_facts_missing_embeddings(
        &self,
        last_completed_fact_id: Option<&str>,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        let limit = if limit > 0 {
            limit
        } else {
            DEFAULT_BACKFILL_BATCH_SIZE
        };
        let sql = if last_completed_fact_id.is_some() {
            "SELECT * FROM (SELECT * FROM fact WHERE embedding IS NONE) \
             WHERE fact_id > $last_completed_fact_id ORDER BY fact_id ASC LIMIT $limit"
                .to_string()
        } else {
            "SELECT * FROM fact WHERE embedding IS NONE ORDER BY fact_id ASC LIMIT $limit"
                .to_string()
        };
        self.db
            .query_rows(
                &sql,
                Some(json!({
                    "last_completed_fact_id": last_completed_fact_id,
                    "limit": limit,
                })),
            )
            .await
    }

    pub(crate) async fn update_embedding_fields(
        &self,
        fact_id: &str,
        fields: Value,
    ) -> Result<(), MemoryError> {
        let record_id = fact_id.strip_prefix("fact:").ok_or_else(|| {
            MemoryError::Validation(format!("invalid fact id for backfill: {fact_id}"))
        })?;
        let Value::Object(fields) = fields else {
            return Err(MemoryError::Validation(
                "backfill embedding fields must be an object".to_string(),
            ));
        };
        let model_assignment = if fields.contains_key("embedding_model") {
            ", embedding_model = $embedding_model"
        } else {
            ""
        };
        let sql = format!(
            "UPDATE fact:⟨{record_id}⟩ SET embedding = $embedding, \
             embedding_provider = $embedding_provider, \
             embedding_dimension = $embedding_dimension, \
             embedding_signature = $embedding_signature, \
             embedding_updated_at = type::datetime($embedding_updated_at){model_assignment} \
             WHERE embedding IS NONE RETURN AFTER"
        );
        self.db
            .query(&sql, Some(Value::Object(fields)))
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::{Value, json};

    use super::EmbeddingBackfillStoreClient;
    use crate::service::normalize_dt;
    use crate::storage::{DbClient, SurrealDbClient};

    async fn make_db() -> Arc<SurrealDbClient> {
        let database = format!(
            "embedding_backfill_store_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        );
        let db = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(
                &database,
                &["org".to_string()],
                "warn",
            )
            .await
            .expect("connect in memory"),
        );
        db.apply_migrations("org").await.expect("migrations");
        db
    }

    async fn seed_missing_fact(db: &Arc<SurrealDbClient>, fact_id: &str) {
        let now = normalize_dt(chrono::Utc::now());
        db.create(
            fact_id,
            json!({
                "fact_id": fact_id,
                "fact_type": "note",
                "content": format!("offline {fact_id}"),
                "quote": format!("offline {fact_id}"),
                "source_episode": "episode:seed",
                "t_valid": now,
                "t_ingested": now,
                "confidence": 0.9,
                "index_keys": [],
                "access_count": 0,
                "entity_links": [],
                "scope": "org",
                "policy_tags": [],
                "provenance": {"source_episode": "episode:seed"}
            }),
            "org",
        )
        .await
        .expect("missing fact should be created");
    }

    async fn seed_fact_with_embedding(db: &Arc<SurrealDbClient>, fact_id: &str) {
        let now = normalize_dt(chrono::Utc::now());
        db.create(
            fact_id,
            json!({
                "fact_id": fact_id,
                "fact_type": "note",
                "content": format!("stored {fact_id}"),
                "quote": format!("stored {fact_id}"),
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
                "embedding": vec![0.1f64; 1536],
                "embedding_provider": "legacy-test",
                "embedding_dimension": 1536,
                "embedding_signature": "embsig:old",
                "embedding_updated_at": now
            }),
            "org",
        )
        .await
        .expect("stored fact should be created");
    }

    #[tokio::test]
    async fn narrow_backfill_store_selects_only_facts_without_embedding() {
        let db = make_db().await;
        seed_missing_fact(&db, "fact:missing").await;
        seed_fact_with_embedding(&db, "fact:stale").await;
        let store = EmbeddingBackfillStoreClient::new(db, "org");

        assert_eq!(
            store.count_facts_missing_embeddings().await.expect("count"),
            1
        );
        let rows = store
            .select_facts_missing_embeddings(None, 100)
            .await
            .expect("select");
        let ids: Vec<&str> = rows
            .iter()
            .filter_map(|row| row.get("fact_id").and_then(Value::as_str))
            .collect();
        assert_eq!(ids, vec!["fact:missing"]);
    }

    #[tokio::test]
    async fn narrow_backfill_store_respects_fact_id_cursor() {
        let db = make_db().await;
        seed_missing_fact(&db, "fact:1").await;
        seed_missing_fact(&db, "fact:2").await;
        seed_missing_fact(&db, "fact:3").await;
        let store = EmbeddingBackfillStoreClient::new(db, "org");

        let page = store
            .select_facts_missing_embeddings(Some("fact:1"), 2)
            .await
            .expect("page");
        let ids: Vec<&str> = page
            .iter()
            .filter_map(|row| row.get("fact_id").and_then(Value::as_str))
            .collect();
        assert_eq!(ids, vec!["fact:2", "fact:3"]);
    }
}
