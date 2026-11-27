//! Memory-owned fact access log.
//!
//! The access log records retrieval heat for a fact: how often it
//! was served and when it was last read. It is memory's, because
//! memory performs the retrieval that produces the heat, so it
//! lives apart from the knowledge-owned graph reads it used to be
//! bundled with.
//!
//! Heat-only writes do not return the immutable source payload.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::MemoryError;
use crate::storage::{BoundDbClient, DbClient};

const MAX_ARCHIVAL_EPISODES_PER_RPC: usize = 64;
const MAX_FACT_KEYS_PER_RPC: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FactAccessStats {
    pub access_count: i64,
    pub last_accessed: Option<String>,
}

impl FactAccessStats {
    pub(crate) fn parsed_last_accessed(&self) -> Result<Option<DateTime<Utc>>, MemoryError> {
        self.last_accessed
            .as_deref()
            .map(|value| {
                DateTime::parse_from_rfc3339(value)
                    .map(|timestamp| timestamp.with_timezone(&Utc))
                    .map_err(|_| {
                        MemoryError::Storage("fact access row has an invalid timestamp".into())
                    })
            })
            .transpose()
    }
}

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
    /// The existing side row is updated atomically. Only the first access
    /// projects the fact's source identity, within the same transaction.
    pub async fn record_fact_access(&self, fact_id: &str, boost: i64) -> Result<(), MemoryError> {
        let record_id = fact_key(fact_id)?;
        let last_accessed = crate::shared::temporal::normalize_dt(crate::shared::temporal::now());

        self.db
            .query(
                "BEGIN TRANSACTION; \
                 LET $existing = SELECT VALUE id FROM type::record('fact_access', $fact_id); \
                 IF array::len($existing) > 0 { \
                   UPDATE type::record('fact_access', $fact_id) SET \
                   access_count = IF $boost > 0 THEN \
                     IF access_count > $int_max - $boost THEN $int_max \
                     ELSE access_count + $boost END \
                   ELSE IF $boost < 0 THEN \
                     IF access_count < $int_min - $boost THEN $int_min \
                     ELSE access_count + $boost END \
                   ELSE access_count END, \
                   last_accessed = type::datetime($last_accessed) RETURN NONE; \
                 } ELSE { \
                   LET $source = SELECT VALUE source_episode FROM type::record('fact', $fact_id); \
                   IF array::len($source) > 0 { \
                     CREATE type::record('fact_access', $fact_id) SET \
                     source_episode = $source[0], access_count = $boost, \
                     last_accessed = type::datetime($last_accessed) RETURN NONE; \
                   }; \
                 }; \
                 COMMIT TRANSACTION;",
                Some(json!({
                    "fact_id": record_id,
                    "boost": boost,
                    "int_max": i64::MAX,
                    "int_min": i64::MIN,
                    "last_accessed": last_accessed,
                })),
            )
            .await
            .map(|_| ())
    }

    /// Read the stored heat for the supplied facts. A fact with no access row
    /// is omitted, and a failed storage read is returned to the caller.
    pub async fn read_fact_accesses(
        &self,
        fact_ids: &[String],
    ) -> Result<HashMap<String, FactAccessStats>, MemoryError> {
        let mut keys = fact_ids
            .iter()
            .map(|id| fact_key(id))
            .collect::<Result<Vec<_>, _>>()?;
        keys.sort_unstable();
        keys.dedup();

        let mut records = HashMap::new();
        for batch in keys.chunks(MAX_FACT_KEYS_PER_RPC) {
            let rows = self
                .query_access_rows(
                    "SELECT record::id(id) AS fact_key, access_count, last_accessed \
                     FROM array::map($fact_keys, |$key| type::record('fact_access', $key))",
                    json!({ "fact_keys": batch }),
                )
                .await?;
            for row in rows {
                let key = row["fact_key"].as_str().ok_or_else(|| {
                    MemoryError::Storage("fact access row has an invalid record key".to_string())
                })?;
                let id = format!("fact:{key}");
                let stats: FactAccessStats = serde_json::from_value(row).map_err(|_| {
                    MemoryError::Storage("fact access row has invalid statistics".to_string())
                })?;
                stats.parsed_last_accessed()?;
                records.insert(id, stats);
            }
        }
        Ok(records)
    }

    /// Whether any fact linked to `episode_id` was accessed at or after
    /// `hot_cutoff`.
    pub async fn has_recent_fact_access(
        &self,
        episode_id: &str,
        hot_cutoff: &str,
    ) -> Result<bool, MemoryError> {
        let rows = self
            .query_access_rows(
                "SELECT id FROM fact_access \
                 WHERE source_episode = $episode_id \
                 AND last_accessed IS NOT NONE \
                 AND last_accessed >= type::datetime($hot_cutoff) LIMIT 1",
                json!({ "episode_id": episode_id, "hot_cutoff": hot_cutoff }),
            )
            .await?;
        Ok(!rows.is_empty())
    }

    /// Returns distinct source episodes with a fact accessed at or after the
    /// supplied cutoff, preserving the single-episode heat predicate.
    pub async fn episode_ids_with_recent_fact_access(
        &self,
        episode_ids: &[String],
        hot_cutoff: &str,
    ) -> Result<Vec<String>, MemoryError> {
        let mut hot_episode_ids = BTreeSet::new();
        for batch in episode_ids.chunks(MAX_ARCHIVAL_EPISODES_PER_RPC) {
            let rows = self
                .query_access_rows(
                    "SELECT source_episode FROM fact_access \
                     WHERE source_episode IN $episode_ids \
                     AND last_accessed IS NOT NONE \
                     AND last_accessed >= type::datetime($hot_cutoff) \
                     GROUP BY source_episode",
                    json!({ "episode_ids": batch, "hot_cutoff": hot_cutoff }),
                )
                .await?;
            for row in rows {
                if let Some(episode_id) = row
                    .get("source_episode")
                    .and_then(crate::storage::value_helpers::string_from_value)
                {
                    hot_episode_ids.insert(episode_id);
                }
            }
        }
        Ok(hot_episode_ids.into_iter().collect())
    }

    async fn query_access_rows(
        &self,
        sql: &str,
        bindings: Value,
    ) -> Result<Vec<Value>, MemoryError> {
        match self.db.query(sql, Some(bindings)).await? {
            Value::Array(rows) => Ok(rows),
            _ => Err(MemoryError::Storage(
                "fact access query did not return rows".to_string(),
            )),
        }
    }
}

fn fact_key(fact_id: &str) -> Result<&str, MemoryError> {
    crate::storage::require_record_kind(fact_id, "fact")?;
    fact_id
        .strip_prefix("fact:")
        .filter(|key| !key.is_empty())
        .ok_or_else(|| MemoryError::Validation("expected a fact id with its table prefix".into()))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{FactAccessStats, FactAccessStore};
    use crate::error::MemoryError;
    use crate::service::mock_db::MockDbClient;
    use crate::storage::{DbClient, SurrealDbClient};
    use chrono::{DateTime, TimeZone, Utc};

    #[tokio::test]
    async fn record_fact_access_rejects_invalid_record_ids() {
        let store = FactAccessStore::new(Arc::new(MockDbClient::new()), "org");

        let result = store.record_fact_access("episode:xyz", 1).await;

        assert!(matches!(result, Err(MemoryError::Validation(_))));
    }

    const NAMESPACE: &str = "org";

    fn fixed_at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 3, 12, 0, 0)
            .single()
            .expect("fixed UTC time")
    }

    // Integration evidence: these scenarios use a real embedded SurrealDB to
    // verify the access statement, transaction arithmetic, and field isolation.
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

    async fn seed_access(db: &SurrealDbClient, fact_id: &str, access_count: i64) {
        let key = fact_id.strip_prefix("fact:").expect("test fact id");
        db.create(
            &format!("fact_access:{key}"),
            serde_json::json!({"source_episode": "episode:seed", "access_count": access_count}),
            NAMESPACE,
            &[],
        )
        .await
        .expect("seed heat");
    }

    async fn access_count(store: &FactAccessStore, fact_id: &str) -> i64 {
        store
            .read_fact_accesses(&[fact_id.to_string()])
            .await
            .expect("read heat")[fact_id]
            .access_count
    }

    async fn seed_fact_with_vector(
        db: &Arc<SurrealDbClient>,
        fact_id: &str,
        access_count: serde_json::Value,
        signature: &str,
    ) {
        let now = crate::shared::temporal::normalize_dt(fixed_at());
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

    async fn read_fact_if_present(
        db: &Arc<SurrealDbClient>,
        fact_id: &str,
    ) -> Option<serde_json::Value> {
        let db_client: Arc<dyn DbClient> = Arc::clone(db) as Arc<dyn DbClient>;
        let reader = crate::knowledge::infra::KnowledgeReadAdapter::new(db_client, NAMESPACE);
        crate::knowledge::api::owned_fact_scan(&reader)
            .await
            .expect("scan facts")
            .into_iter()
            .find(|record| {
                record.get("fact_id").and_then(serde_json::Value::as_str) == Some(fact_id)
            })
    }

    async fn read_fact(db: &Arc<SurrealDbClient>, fact_id: &str) -> serde_json::Value {
        read_fact_if_present(db, fact_id)
            .await
            .unwrap_or_else(|| panic!("{fact_id} must exist"))
    }

    #[tokio::test]
    async fn first_access_persists_heat_without_mutating_the_fact_record() {
        let db = make_db().await;
        let id = "fact:access_first";
        seed_fact(&db, id, serde_json::json!(0)).await;
        let before = db.select_one(id, NAMESPACE).await.unwrap().unwrap();
        let store = FactAccessStore::new(Arc::clone(&db) as Arc<dyn DbClient>, NAMESPACE);

        store
            .record_fact_access(id, 3)
            .await
            .expect("record access");

        let after = db.select_one(id, NAMESPACE).await.unwrap().unwrap();
        assert_eq!(after["access_count"], 0, "heat is stored separately");
        assert_eq!(after, before);
        let stats = db
            .query(
                "SELECT access_count, last_accessed, source_episode FROM fact_access:access_first",
                None,
                NAMESPACE,
            )
            .await
            .expect("read persisted heat");
        assert_eq!(stats[0]["access_count"], 3);
        assert_eq!(stats[0]["source_episode"], "episode:seed");
        assert!(stats[0]["last_accessed"].is_string());
    }

    #[tokio::test]
    async fn read_fact_accesses_only_returns_requested_existing_heat() {
        let db = make_db().await;
        seed_fact(&db, "fact:cold", serde_json::json!(0)).await;
        seed_fact(&db, "fact:hot", serde_json::json!(0)).await;
        seed_access(&db, "fact:hot", 7).await;
        seed_access(&db, "fact:unrequested", 16).await;
        let store = FactAccessStore::new(db, NAMESPACE);

        let stats = store
            .read_fact_accesses(&[
                "fact:hot".into(),
                "fact:cold".into(),
                "fact:nonexistent".into(),
                "fact:hot".into(),
            ])
            .await
            .expect("read heat");

        assert_eq!(stats.len(), 1);
        assert_eq!(
            stats["fact:hot"],
            FactAccessStats {
                access_count: 7,
                last_accessed: None
            }
        );
    }

    #[tokio::test]
    async fn read_fact_accesses_propagates_storage_failure_without_empty_fallback() {
        let db = MockDbClient::new().expect_query_with(
            |sql| sql.contains("fact_access"),
            |_, _| Err(MemoryError::Storage("controlled heat read failure".into())),
        );
        let store = FactAccessStore::new(Arc::new(db), NAMESPACE);

        let result = store.read_fact_accesses(&["fact:a".into()]).await;

        assert!(
            matches!(result, Err(MemoryError::Storage(message)) if message == "controlled heat read failure")
        );
    }

    #[tokio::test]
    async fn read_fact_accesses_binds_at_most_sixty_four_distinct_keys_per_request() {
        let batches = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = batches.clone();
        let db = MockDbClient::new().expect_query_with(
            |sql| sql.contains("fact_access"),
            move |_, args| {
                let bindings = args.expect("exact key bindings");
                seen.lock()
                    .expect("query batches")
                    .push(bindings["fact_keys"].clone());
                Ok(serde_json::json!([]))
            },
        );
        let store = FactAccessStore::new(Arc::new(db), NAMESPACE);
        let keys = (0..65).map(|n| format!("heat_{n:02}")).collect::<Vec<_>>();
        let mut ids = keys
            .iter()
            .map(|key| format!("fact:{key}"))
            .collect::<Vec<_>>();
        ids.push("fact:heat_29".into());

        let result = store.read_fact_accesses(&ids).await.expect("read heat");

        assert!(result.is_empty());
        assert_eq!(
            *batches.lock().expect("query batches"),
            vec![
                serde_json::json!(&keys[..64]),
                serde_json::json!(&keys[64..])
            ]
        );
    }

    #[tokio::test]
    async fn read_fact_accesses_returns_an_error_for_a_malformed_timestamp() {
        let db = MockDbClient::new().expect_query_with(
            |sql| sql.contains("fact_access"),
            |_, _| Ok(serde_json::json!([{"fact_key": "a", "access_count": 1, "last_accessed": "not a timestamp"}])),
        );
        let store = FactAccessStore::new(Arc::new(db), NAMESPACE);

        let result = store.read_fact_accesses(&["fact:a".into()]).await;

        assert!(
            matches!(result, Err(MemoryError::Storage(message)) if message.contains("invalid timestamp"))
        );
    }

    /// Commits competing count/vector changes immediately before the access
    /// write, after a legacy whole-record read would have captured stale fields.
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
            FactAccessStore::new(self.inner.clone(), NAMESPACE)
                .record_fact_access(&fact_id, 1)
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

    /// Integration: both external changes must survive the focal access write.
    /// The database decorator fixes their ordering rather than relying on two
    /// concurrent tasks happening to read the same old count.
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
            access_count(&store, fact_id).await,
            2,
            "the external increment and this access must both land"
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
            read_fact_if_present(&db, "fact:never_created")
                .await
                .is_none(),
            "a no-op read must not create the record"
        );
        assert!(
            store
                .read_fact_accesses(&["fact:never_created".into()])
                .await
                .expect("read heat")
                .is_empty()
        );
    }

    /// The count accumulates from what is stored, including under simultaneous
    /// retrievals.
    #[tokio::test]
    async fn record_fact_access_accumulates_from_the_stored_count() {
        let db = make_db().await;
        let fact_id = "fact:access_coercion";
        seed_fact(&db, fact_id, serde_json::json!(0)).await;
        seed_access(&db, fact_id, 4).await;
        let store = FactAccessStore::new(Arc::clone(&db) as Arc<dyn DbClient>, NAMESPACE);

        store.record_fact_access(fact_id, 3).await.expect("first");
        store.record_fact_access(fact_id, 2).await.expect("second");

        assert_eq!(access_count(&store, fact_id).await, 9);
    }

    #[tokio::test]
    async fn concurrent_access_updates_do_not_lose_increments() {
        let db = make_db().await;
        let fact_id = "fact:access_concurrent";
        seed_fact(&db, fact_id, serde_json::json!(0)).await;
        let store = FactAccessStore::new(Arc::clone(&db) as Arc<dyn DbClient>, NAMESPACE);
        let gate = tokio::sync::Barrier::new(4);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::try_join!(
                async {
                    gate.wait().await;
                    store.record_fact_access(fact_id, 1).await
                },
                async {
                    gate.wait().await;
                    store.record_fact_access(fact_id, 1).await
                },
                async {
                    gate.wait().await;
                    store.record_fact_access(fact_id, 1).await
                },
                async {
                    gate.wait().await;
                    store.record_fact_access(fact_id, 1).await
                },
            )
        })
        .await
        .expect("concurrent updates complete within the bound")
        .expect("all access updates succeed");

        assert_eq!(access_count(&store, fact_id).await, 4);
    }

    #[tokio::test]
    async fn record_fact_access_saturates_instead_of_wrapping() {
        let db = make_db().await;
        let fact_id = "fact:access_saturation";
        seed_fact(&db, fact_id, serde_json::json!(0)).await;
        seed_access(&db, fact_id, 1).await;
        let store = FactAccessStore::new(Arc::clone(&db) as Arc<dyn DbClient>, NAMESPACE);

        store
            .record_fact_access(fact_id, i64::MAX)
            .await
            .expect("first");
        store.record_fact_access(fact_id, 10).await.expect("second");

        assert_eq!(access_count(&store, fact_id).await, i64::MAX);
    }

    #[tokio::test]
    async fn record_fact_access_saturates_at_the_lower_bound() {
        let db = make_db().await;
        let fact_id = "fact:access_lower_saturation";
        seed_fact(&db, fact_id, serde_json::json!(0)).await;
        seed_access(&db, fact_id, i64::MIN).await;
        let store = FactAccessStore::new(Arc::clone(&db) as Arc<dyn DbClient>, NAMESPACE);

        store
            .record_fact_access(fact_id, -1)
            .await
            .expect("lower bound");

        assert_eq!(access_count(&store, fact_id).await, i64::MIN);
    }

    #[tokio::test]
    async fn record_fact_access_binds_forged_record_ids_as_values() {
        let db = make_db().await;
        let innocent = "fact:access_innocent";
        seed_fact(&db, innocent, serde_json::json!(0)).await;
        let store = FactAccessStore::new(Arc::clone(&db) as Arc<dyn DbClient>, NAMESPACE);
        let forged =
            "fact:x⟩ SET access_count = 77; UPDATE fact:⟨access_innocent⟩ SET access_count = 99"
                .to_string();

        store
            .record_fact_access(&forged, 1)
            .await
            .expect("a non-existent id is a no-op, not SQL");

        assert_eq!(read_fact(&db, innocent).await["access_count"], 0);
        assert!(
            store
                .read_fact_accesses(&[innocent.to_string(), forged])
                .await
                .expect("read heat")
                .is_empty()
        );
    }
}
