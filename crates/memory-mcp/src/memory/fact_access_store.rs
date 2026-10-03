//! Memory-owned fact access log.
//!
//! The access log records retrieval heat for a fact: how often it
//! was served and when it was last read. It is memory's, because
//! memory performs the retrieval that produces the heat, so it
//! lives apart from the knowledge-owned graph reads it used to be
//! bundled with.
//!
//! The method bodies are unchanged from `app_store.rs`.

use std::sync::Arc;

use serde_json::json;

use crate::error::MemoryError;
use crate::storage::{BoundDbClient, DbClient};

/// Read-side store for the fact access log.
#[derive(Clone)]
pub struct FactAccessStore {
    db: BoundDbClient,
}

impl FactAccessStore {
    pub fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            db: BoundDbClient::new(db, namespace),
        }
    }

    pub fn from_bound(db: BoundDbClient) -> Self {
        Self { db }
    }

    /// Increments fact access metadata without exposing record mutation details
    /// to retrieval or explanation orchestration.
    ///
    /// The write names only the two fields this store owns. It used to read the
    /// whole fact, change those fields, and write the whole record back — which
    /// meant a retrieval that raced a background vector write restored the
    /// record it had read, dropping the vector. The count is still read first
    /// because the arithmetic needs it; only the write is narrow.
    pub async fn record_fact_access(&self, fact_id: &str, boost: i64) -> Result<(), MemoryError> {
        crate::storage::require_record_kind(fact_id, "fact")?;
        let record_id = fact_id.strip_prefix("fact:").ok_or_else(|| {
            MemoryError::Validation(format!("invalid fact id for access write: {fact_id}"))
        })?;
        if record_id.is_empty() {
            return Err(MemoryError::Validation(format!(
                "fact id carries no record id for access write: {fact_id}"
            )));
        }
        let record = self.db.select_one(fact_id).await?;
        let Some(record) = record.and_then(|value| value.as_object().cloned()) else {
            return Ok(());
        };

        let access_count = record
            .get("access_count")
            .and_then(crate::storage::value_helpers::json_i64)
            .unwrap_or(0)
            .saturating_add(boost);
        let last_accessed = crate::shared::temporal::normalize_dt(crate::shared::temporal::now());

        self.db
            .query(
                "UPDATE type::record('fact', $fact_id) SET access_count = $access_count, \
                 last_accessed = type::datetime($last_accessed)",
                Some(json!({
                    "fact_id": record_id,
                    "access_count": access_count,
                    "last_accessed": last_accessed,
                })),
            )
            .await
            .map(|_| ())
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

    use super::FactAccessStore;
    use crate::error::MemoryError;
    use crate::service::mock_db::MockDbClient;
    use crate::storage::{DbClient, SurrealDbClient};

    #[tokio::test]
    async fn record_fact_access_rejects_invalid_record_ids() {
        let store = FactAccessStore::new(Arc::new(MockDbClient::new()), "org");

        for bad_id in ["bare-hex-id", "", "episode:xyz"] {
            let result = store.record_fact_access(bad_id, 1).await;
            assert!(
                matches!(result, Err(MemoryError::Validation(_))),
                "expected validation error for '{bad_id}'"
            );
        }
    }

    const NAMESPACE: &str = "org";

    async fn make_db() -> Arc<SurrealDbClient> {
        let database = format!(
            "fact_access_store_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        );
        let db = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(
                &database,
                &[NAMESPACE.to_string()],
                "warn",
            )
            .await
            .expect("connect in memory"),
        );
        db.apply_migrations(NAMESPACE).await.expect("migrations");
        db
    }

    async fn seed_fact(db: &Arc<SurrealDbClient>, fact_id: &str, access_count: serde_json::Value) {
        seed_fact_with_vector(db, fact_id, access_count, "sig-old").await
    }

    async fn seed_fact_with_vector(
        db: &Arc<SurrealDbClient>,
        fact_id: &str,
        access_count: serde_json::Value,
        signature: &str,
    ) {
        let now = crate::shared::temporal::normalize_dt(crate::shared::temporal::now());
        db.create(
            fact_id,
            serde_json::json!({
                "fact_id": fact_id,
                "fact_type": "note",
                "content": "access heat",
                "quote": "access heat",
                "source_episode": "episode:seed",
                "t_valid": now,
                "t_ingested": now,
                "confidence": 0.9,
                "index_keys": [],
                "access_count": access_count,
                "entity_links": [],
                "scope": "org",
                "policy_tags": [],
                "provenance": {"source_episode": "episode:seed"},
                "embedding": vec![0.1f64; 1536],
                "embedding_provider": "legacy-test",
                "embedding_dimension": 1536,
                "embedding_signature": signature,
                "embedding_updated_at": now
            }),
            NAMESPACE,
            crate::knowledge::queries::FACT_TEMPORAL_FIELDS,
        )
        .await
        .expect("seed fact");
    }

    async fn read_fact(db: &Arc<SurrealDbClient>, fact_id: &str) -> serde_json::Value {
        db.select_one(fact_id, NAMESPACE)
            .await
            .expect("read fact")
            .unwrap_or_else(|| panic!("{fact_id} must exist"))
    }

    /// A `DbClient` that lets a vector write land immediately before this
    /// store's write. The access update and the vector update touch different
    /// fields, and only a whole-record write can lose one of them.
    struct RacingVectorWriter {
        inner: Arc<SurrealDbClient>,
        pending: std::sync::Mutex<Option<String>>,
    }

    impl RacingVectorWriter {
        fn new(inner: Arc<SurrealDbClient>, fact_id: &str) -> Arc<Self> {
            Arc::new(Self {
                inner,
                pending: std::sync::Mutex::new(Some(fact_id.to_string())),
            })
        }

        async fn race_ahead(&self) -> Result<(), MemoryError> {
            let target = self.pending.lock().expect("pending lock").take();
            let Some(fact_id) = target else {
                return Ok(());
            };
            self.inner
                .query(
                    "UPDATE type::record('fact', $fact_id) SET embedding = $embedding, \
                     embedding_signature = 'sig-concurrent'",
                    Some(serde_json::json!({
                        "fact_id": fact_id.trim_start_matches("fact:"),
                        "embedding": vec![0.1f64; 1536],
                    })),
                    NAMESPACE,
                )
                .await?;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl DbClient for RacingVectorWriter {
        async fn select_one(
            &self,
            record_id: &str,
            namespace: &str,
        ) -> Result<Option<serde_json::Value>, MemoryError> {
            self.inner.select_one(record_id, namespace).await
        }

        async fn select_table(
            &self,
            table: crate::storage::table_scope::OwnedTable,
            namespace: &str,
        ) -> Result<Vec<serde_json::Value>, MemoryError> {
            self.inner.select_table(table, namespace).await
        }

        async fn create(
            &self,
            record_id: &str,
            content: serde_json::Value,
            namespace: &str,
            temporal_fields: &[&str],
        ) -> Result<serde_json::Value, MemoryError> {
            self.inner
                .create(record_id, content, namespace, temporal_fields)
                .await
        }

        async fn update(
            &self,
            record_id: &str,
            content: serde_json::Value,
            namespace: &str,
            temporal_fields: &[&str],
        ) -> Result<serde_json::Value, MemoryError> {
            self.race_ahead().await?;
            self.inner
                .update(record_id, content, namespace, temporal_fields)
                .await
        }

        async fn query(
            &self,
            sql: &str,
            vars: Option<serde_json::Value>,
            namespace: &str,
        ) -> Result<serde_json::Value, MemoryError> {
            self.race_ahead().await?;
            self.inner.query(sql, vars, namespace).await
        }

        async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError> {
            self.inner.apply_migrations(namespace).await
        }
    }

    /// Retrieval heat and vectors are different owners' fields on one record.
    ///
    /// The fact already carries a vector, so a whole-record access write has a
    /// vector in its snapshot to write back; the concurrent writer replaces that
    /// vector's signature before the access write lands. Recording an access
    /// must leave the newer vector alone.
    #[tokio::test]
    async fn fact_access_preserves_concurrent_vector_write() {
        let db = make_db().await;
        let fact_id = "fact:access_race";
        seed_fact_with_vector(&db, fact_id, serde_json::json!(0), "sig-old").await;
        let store = FactAccessStore::new(
            RacingVectorWriter::new(Arc::clone(&db), fact_id) as Arc<dyn DbClient>,
            NAMESPACE,
        );

        store
            .record_fact_access(fact_id, 1)
            .await
            .expect("record access");

        let stored = read_fact(&db, fact_id).await;
        assert_eq!(
            stored["embedding_signature"], "sig-concurrent",
            "the access write must not restore the vector its snapshot carried"
        );
        assert_eq!(
            stored["access_count"], 1,
            "the access write must still land"
        );
    }

    #[tokio::test]
    async fn record_fact_access_leaves_an_absent_record_alone() {
        let db = make_db().await;
        let store = FactAccessStore::new(Arc::clone(&db) as Arc<dyn DbClient>, NAMESPACE);

        store
            .record_fact_access("fact:never_created", 1)
            .await
            .expect("an absent fact is a no-op, not an error");
        assert!(
            db.select_one("fact:never_created", NAMESPACE)
                .await
                .expect("read")
                .is_none(),
            "a no-op read must not create the record"
        );
    }

    /// The count accumulates from what is stored, not from zero. The
    /// `json_i64` coercion the read applies is defensive only: the fact table's
    /// schema types `access_count` as `int`, so a legacy string count cannot be
    /// stored under it and that branch is unreachable here rather than untested.
    #[tokio::test]
    async fn record_fact_access_accumulates_from_the_stored_count() {
        let db = make_db().await;
        let fact_id = "fact:access_coercion";
        seed_fact(&db, fact_id, serde_json::json!(4)).await;
        let store = FactAccessStore::new(Arc::clone(&db) as Arc<dyn DbClient>, NAMESPACE);

        store.record_fact_access(fact_id, 3).await.expect("first");
        store.record_fact_access(fact_id, 2).await.expect("second");

        assert_eq!(read_fact(&db, fact_id).await["access_count"], 9);
    }

    #[tokio::test]
    async fn record_fact_access_saturates_instead_of_wrapping() {
        let db = make_db().await;
        let fact_id = "fact:access_saturation";
        seed_fact(&db, fact_id, serde_json::json!(1)).await;
        let store = FactAccessStore::new(Arc::clone(&db) as Arc<dyn DbClient>, NAMESPACE);

        store
            .record_fact_access(fact_id, i64::MAX)
            .await
            .expect("first");
        store.record_fact_access(fact_id, 10).await.expect("second");

        assert_eq!(read_fact(&db, fact_id).await["access_count"], i64::MAX);
    }
}
