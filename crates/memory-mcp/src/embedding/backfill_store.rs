//! Narrow fact store for deferred embedding backfill.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::error::MemoryError;
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

    pub(crate) fn from_bound(db: BoundDbClient) -> Self {
        Self { db }
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

    /// Read one fact using a bound record value rather than embedding an
    /// identifier in SQL text. `None` means the fact does not exist; malformed
    /// result shapes are errors rather than being mistaken for absence.
    pub(crate) async fn select_fact(&self, fact_id: &str) -> Result<Option<Value>, MemoryError> {
        let record_id = fact_record_key(fact_id)?;
        let result = self
            .db
            .query(
                "SELECT * FROM type::record('fact', $fact_id)",
                Some(json!({ "fact_id": record_id })),
            )
            .await?;
        let Value::Array(rows) = result else {
            return Err(MemoryError::Storage(format!(
                "fact read for {fact_id} returned a non-array result"
            )));
        };
        match rows.as_slice() {
            [] => Ok(None),
            [Value::Object(_)] => Ok(rows.into_iter().next()),
            [_] => Err(MemoryError::Storage(format!(
                "fact read for {fact_id} returned a non-object row"
            ))),
            _ => Err(MemoryError::Storage(format!(
                "fact read for {fact_id} returned more than one row"
            ))),
        }
    }

    /// Conditionally write embedding fields on one fact, and report whether a
    /// row was written.
    ///
    /// The predicate decides whether an existing vector may be touched, and the
    /// statement evaluates it: a read here and a write there are two statements,
    /// and a concurrent writer can land between them. This used to build the
    /// whole record and hand it to `DbClient::update`, which wrote every field
    /// it was given — so a vector write could restore a `access_count` or a
    /// temporal field a concurrent writer had already changed.
    ///
    /// The record is targeted as a value, `type::record('fact', $fact_id)`,
    /// rather than interpolated into a quoted identifier: an id containing `⟩`
    /// would otherwise escape it.
    pub(crate) async fn apply_embedding_fields(
        &self,
        fact_id: &str,
        fields: Value,
        predicate: &str,
    ) -> Result<bool, MemoryError> {
        let record_id = fact_record_key(fact_id)?;
        let Value::Object(fields) = fields else {
            return Err(MemoryError::Validation(
                "embedding write fields must be an object".to_string(),
            ));
        };
        let (assignments, mut vars) = crate::storage::queries::build_set_assignments(
            crate::knowledge::queries::FACT_TEMPORAL_FIELDS,
            fields,
        );
        if assignments.is_empty() {
            return Err(MemoryError::Validation(
                "embedding write carries no fields".to_string(),
            ));
        }
        vars.insert("fact_id".to_string(), Value::from(record_id));
        let sql = format!(
            "UPDATE type::record('fact', $fact_id) SET {} WHERE {predicate} RETURN AFTER",
            assignments.join(", ")
        );
        let rows = self.db.query(&sql, Some(Value::Object(vars))).await?;
        let Value::Array(rows) = rows else {
            return Err(MemoryError::Storage(format!(
                "embedding write for {fact_id} returned a non-array result"
            )));
        };
        match rows.as_slice() {
            [] => Ok(false),
            [Value::Object(_)] => Ok(true),
            [_] => Err(MemoryError::Storage(format!(
                "embedding write for {fact_id} returned a non-object row"
            ))),
            _ => Err(MemoryError::Storage(format!(
                "embedding write for {fact_id} returned more than one row"
            ))),
        }
    }
}

fn fact_record_key(fact_id: &str) -> Result<&str, MemoryError> {
    let record_id = fact_id.strip_prefix("fact:").ok_or_else(|| {
        MemoryError::Validation(format!("invalid fact id for embedding write: {fact_id}"))
    })?;
    if record_id.is_empty() {
        return Err(MemoryError::Validation(format!(
            "fact id carries no record id for embedding write: {fact_id}"
        )));
    }
    Ok(record_id)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::{Value, json};

    use crate::embedding::backfill_store::EmbeddingBackfillStoreClient;
    use crate::shared::temporal::normalize_dt;
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
            crate::knowledge::queries::FACT_TEMPORAL_FIELDS,
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
            crate::knowledge::queries::FACT_TEMPORAL_FIELDS,
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

    #[tokio::test]
    async fn canonical_write_rejects_malformed_storage_result() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use crate::embedding::api::{VectorIdentity, VectorWritePolicy, update_canonical_vector};
        use crate::embedding::infra::FactVectorAdapter;
        use crate::error::MemoryError;
        use crate::service::mock_db::MockDbClient;

        let identity = VectorIdentity {
            provider: "test".into(),
            model: None,
            dimension: 2,
            signature: "sig:test".into(),
        };
        for policy in [
            VectorWritePolicy::FillMissing,
            VectorWritePolicy::ReplaceStale,
        ] {
            for malformed in [
                json!({}),
                json!([null]),
                json!([17]),
                json!(["bad"]),
                json!([{}, {}]),
            ] {
                let writes = Arc::new(AtomicUsize::new(0));
                let seen_writes = Arc::clone(&writes);
                let response = malformed.clone();
                let db = Arc::new(
                    MockDbClient::new()
                        .expect_query_with(
                            |sql| sql.starts_with("SELECT * FROM type::record('fact'"),
                            |_, _| Ok(json!([{"fact_id": "fact:malformed"}])),
                        )
                        .expect_query_with(
                            |sql| sql.starts_with("UPDATE type::record('fact'"),
                            move |_, _| {
                                seen_writes.fetch_add(1, Ordering::SeqCst);
                                Ok(response.clone())
                            },
                        ),
                );
                let adapter = FactVectorAdapter::new(db, "org");
                let result = update_canonical_vector(
                    &adapter,
                    "fact:malformed",
                    vec![0.1, 0.2],
                    &identity,
                    chrono::Utc::now(),
                    policy,
                )
                .await;

                assert_eq!(
                    writes.load(Ordering::SeqCst),
                    1,
                    "exercise the write result"
                );
                assert!(
                    matches!(result, Err(MemoryError::Storage(_))),
                    "{policy:?} must reject {malformed}, got {result:?}"
                );
            }
        }
    }
}
