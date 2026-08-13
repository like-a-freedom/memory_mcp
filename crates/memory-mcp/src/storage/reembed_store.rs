//! Narrow reembed store: owns the fact-scan queries behind the batch reembed
//! worker.
//!
//! Per ADR-0027 the SQL for a capability lives next to the store that owns
//! it, not on the universal `DbClient`. This store is the single home for
//! "which facts have a stale embedding signature" — `MemoryService::reembed_all_facts`
//! depends on it instead of reaching through `db_client` directly.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::service::MemoryError;
use crate::storage::helpers::is_missing_table_error;
use crate::storage::{BoundDbClient, DbClient};

/// Read-side store for the batch reembed worker.
#[derive(Clone)]
pub struct ReembedStoreClient {
    db: BoundDbClient,
}

impl ReembedStoreClient {
    pub fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            db: BoundDbClient::new(db, namespace),
        }
    }

    /// Execute schema/index DDL in the process-bound namespace.
    pub async fn execute_ddl(&self, sql: &str) -> Result<Value, MemoryError> {
        self.db.query(sql, None).await
    }

    /// Load a reembed-owned record by ID.
    pub async fn load_record(&self, record_id: &str) -> Result<Option<Value>, MemoryError> {
        self.db.select_one(record_id).await
    }

    /// Create or update a reembed-owned record without exposing routing.
    pub async fn upsert_record(&self, record_id: &str, payload: Value) -> Result<(), MemoryError> {
        if self.db.select_one(record_id).await?.is_some() {
            self.db.update(record_id, payload).await?;
        } else {
            self.db.create(record_id, payload).await?;
        }
        Ok(())
    }

    /// Counts facts whose embedding metadata does not match the target signature.
    pub async fn count_facts_needing_reembed(
        &self,
        target_signature: &str,
    ) -> Result<usize, MemoryError> {
        let sql = "SELECT count() AS count FROM fact WHERE embedding_signature IS NONE \
                   OR embedding_signature IS NULL OR embedding_signature != $target_signature \
                   GROUP ALL";
        let vars = json!({"target_signature": target_signature});
        let result = match self.db.query(sql, Some(vars)).await {
            Ok(value) => value,
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => {
                return Ok(0);
            }
            Err(err) => return Err(err),
        };

        let count = result
            .as_array()
            .and_then(|records| records.first())
            .and_then(|record| record.get("count").cloned())
            .and_then(|value| value.as_u64())
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(0);

        Ok(count)
    }

    /// Selects facts needing rewrite in stable `fact_id` order, optionally after a cursor.
    pub async fn select_facts_needing_reembed(
        &self,
        target_signature: &str,
        last_completed_fact_id: Option<&str>,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        let sql = if last_completed_fact_id.is_some() {
            // SurrealDB 3.0 can incorrectly eliminate rows when the cursor
            // comparison and the stale-signature OR predicate are combined in
            // one WHERE clause. Filtering the stale set in a subquery keeps
            // cursor pagination correct on the MSRV-compatible database.
            "SELECT * FROM (SELECT * FROM fact WHERE embedding_signature IS NONE \
             OR embedding_signature IS NULL OR embedding_signature != $target_signature) \
             WHERE fact_id > $last_completed_fact_id ORDER BY fact_id ASC LIMIT $limit"
                .to_string()
        } else {
            "SELECT * FROM fact WHERE (embedding_signature IS NONE OR embedding_signature IS NULL \
             OR embedding_signature != $target_signature) ORDER BY fact_id ASC LIMIT $limit"
                .to_string()
        };
        let vars = json!({
            "target_signature": target_signature,
            "last_completed_fact_id": last_completed_fact_id,
            "limit": limit,
        });

        match self.db.query(&sql, Some(vars)).await {
            Ok(value) => Ok(value.as_array().cloned().unwrap_or_default()),
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => Ok(vec![]),
            Err(err) => Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use crate::service::normalize_dt;
    use crate::storage::{DbClient, SurrealDbClient, reembed_store::ReembedStoreClient};

    async fn make_db() -> Arc<SurrealDbClient> {
        let db_name = format!(
            "reembed_store_test_{}",
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

    async fn seed_fact(db_client: &Arc<SurrealDbClient>, fact_id: &str, signature: Option<&str>) {
        let now = normalize_dt(chrono::Utc::now());
        // The migrated schema defines an HNSW index over 1536-dim vectors;
        // seeds must match or SurrealDB rejects the insert.
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
                    "embedding_signature": signature,
                    "embedding_updated_at": now,
                }),
                "org",
            )
            .await
            .expect("seed fact should succeed");
    }

    #[tokio::test]
    async fn count_and_select_return_empty_when_fact_table_missing() {
        // Before migrations the `fact` table does not exist; both queries
        // must degrade to empty instead of erroring (same as the old
        // `DbClient` behavior preserved by ADR-0027 relocation).
        let db_name = format!(
            "reembed_store_unmigrated_{}",
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
        let store = ReembedStoreClient::new(db_client.clone(), "org");

        let count = store
            .count_facts_needing_reembed("embsig:target")
            .await
            .expect("count must not error on missing table");
        assert_eq!(count, 0);

        let batch = store
            .select_facts_needing_reembed("embsig:target", None, 10)
            .await
            .expect("select must not error on missing table");
        assert!(batch.is_empty());
    }

    #[tokio::test]
    async fn count_and_select_only_return_stale_signature_facts_in_id_order() {
        let db_client = make_db().await;
        seed_fact(&db_client, "fact:a", Some("embsig:target")).await;
        seed_fact(&db_client, "fact:b", None).await;
        seed_fact(&db_client, "fact:c", Some("embsig:old")).await;
        seed_fact(&db_client, "fact:d", Some("embsig:target")).await;
        let store = ReembedStoreClient::new(db_client.clone(), "org");

        let count = store
            .count_facts_needing_reembed("embsig:target")
            .await
            .expect("count");
        // b (missing signature) and c (stale signature) need reembedding.
        assert_eq!(count, 2);

        let batch = store
            .select_facts_needing_reembed("embsig:target", None, 10)
            .await
            .expect("select");
        let ids: Vec<&str> = batch
            .iter()
            .filter_map(|record| record.get("fact_id").and_then(|v| v.as_str()))
            .collect();
        assert_eq!(ids, vec!["fact:b", "fact:c"]);
    }

    #[tokio::test]
    async fn select_respects_cursor_and_limit() {
        let db_client = make_db().await;
        seed_fact(&db_client, "fact:1", Some("embsig:old")).await;
        seed_fact(&db_client, "fact:2", Some("embsig:old")).await;
        seed_fact(&db_client, "fact:3", Some("embsig:old")).await;
        let store = ReembedStoreClient::new(db_client.clone(), "org");

        let page = store
            .select_facts_needing_reembed("embsig:target", None, 2)
            .await
            .expect("first page");
        let ids: Vec<&str> = page
            .iter()
            .filter_map(|record| record.get("fact_id").and_then(|v| v.as_str()))
            .collect();
        assert_eq!(ids, vec!["fact:1", "fact:2"]);

        let next = store
            .select_facts_needing_reembed("embsig:target", Some("fact:2"), 2)
            .await
            .expect("next page");
        let ids: Vec<&str> = next
            .iter()
            .filter_map(|record| record.get("fact_id").and_then(|v| v.as_str()))
            .collect();
        assert_eq!(ids, vec!["fact:3"]);
    }
}
