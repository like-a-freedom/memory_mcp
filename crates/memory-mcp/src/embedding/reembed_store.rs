//! Narrow reembed store: owns the fact-scan queries behind the batch reembed
//! worker.
//!
//! The SQL for a capability lives next to the store that owns
//! it, not on the universal `DbClient`. This store is the single home for
//! "which facts have a stale embedding signature" — `MemoryService::reembed_all_facts`
//! depends on it instead of reaching through `db_client` directly.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::error::MemoryError;
use crate::storage::{BoundDbClient, DbClient, is_missing_index_error};

/// Name of the fact embedding HNSW index owned by the reembed flow.
pub const EMBEDDING_INDEX_NAME: &str = "fact_embedding_hnsw";

/// Outcome of removing the embedding index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexRemoval {
    /// The index existed and was dropped.
    Removed,
    /// The index did not exist; nothing to drop.
    AlreadyAbsent,
}

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

    /// Drops the fact embedding HNSW index in the process-bound namespace.
    ///
    /// Idempotent: removing an absent index reports [`IndexRemoval::AlreadyAbsent`]
    /// instead of erroring (C2 — the DDL and its tolerance rule live here).
    pub async fn remove_embedding_index(&self) -> Result<IndexRemoval, MemoryError> {
        let sql = format!("REMOVE INDEX {EMBEDDING_INDEX_NAME} ON TABLE fact");
        match self.db.query(&sql, None).await {
            Ok(_) => Ok(IndexRemoval::Removed),
            Err(MemoryError::Storage(message)) if is_missing_index_error(&message) => {
                Ok(IndexRemoval::AlreadyAbsent)
            }
            Err(err) => Err(err),
        }
    }

    /// Extract the `DIMENSION <n>` width from a `DEFINE INDEX … HNSW DIMENSION n`
    /// statement, or `None` when the statement declares no width.
    ///
    /// The statement is SurrealDB's own rendering of the index, so this reads
    /// the schema rather than any value the server was asked about.
    #[cfg(feature = "streamable-http")]
    fn parse_defined_index_dimension(define_statement: &str) -> Option<usize> {
        const MARKER: &str = "DIMENSION ";
        let after = &define_statement[define_statement.find(MARKER)? + MARKER.len()..];
        let width: String = after.chars().take_while(char::is_ascii_digit).collect();
        width.parse().ok()
    }

    /// Read the dimension the namespace's `fact_embedding_hnsw` index was
    /// defined with, or `None` when the index does not exist.
    ///
    /// The index definition is the schema's own record of the vector width
    /// stored facts were written at. A deployment whose provider changed
    /// dimension needs it: without this, an already-provisioned namespace
    /// keeps an index that silently rejects every new embedding.
    ///
    /// Only the HTTP activation path consults it, so a stdio build compiles
    /// without it rather than carrying an unread method.
    #[cfg(feature = "streamable-http")]
    pub async fn embedding_index_dimension(&self) -> Result<Option<usize>, MemoryError> {
        // `INFO FOR TABLE` reports each index as its full `DEFINE` statement
        // rather than as structured fields, so the width is read back out of
        // that text. (`INFO FOR INDEX <name>` needs an `ON TABLE` clause and
        // reports only build status, not the definition.) `BoundDbClient::query`
        // already applies the `Object`/`String` unwrapping the tagged wire form
        // needs, so the record reads as an ordinary object here.
        let rows = self.db.query_rows("INFO FOR TABLE fact", None).await?;
        Ok(rows
            .first()
            .and_then(|record| record.get("indexes"))
            .and_then(|indexes| indexes.get(EMBEDDING_INDEX_NAME))
            .and_then(Value::as_str)
            .and_then(Self::parse_defined_index_dimension))
    }

    /// Counts facts that carry a vector, whatever wrote it.
    ///
    /// This is the predicate that separates a namespace whose gaps backfill
    /// may fill from one whose vectors only a reembed may rewrite, so it
    /// answers "how many hold a vector", not "how many are stale".
    #[cfg(feature = "streamable-http")]
    pub async fn count_stored_vectors(&self) -> Result<usize, MemoryError> {
        let sql = "SELECT count() AS count FROM fact WHERE embedding IS NOT NONE GROUP ALL";
        let rows = self.db.query_rows(sql, None).await?;

        Ok(rows
            .first()
            .and_then(|record| record.get("count").cloned())
            .and_then(|value| value.as_u64())
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(0))
    }

    /// Defines the fact embedding HNSW index with the given vector dimension
    /// in the process-bound namespace (C2).
    pub async fn define_embedding_index(&self, dimension: usize) -> Result<(), MemoryError> {
        let sql = format!(
            "DEFINE INDEX {EMBEDDING_INDEX_NAME} ON TABLE fact FIELDS embedding HNSW DIMENSION {dimension}"
        );
        self.db.query(&sql, None).await.map(|_| ())
    }

    /// Load a reembed-owned record by ID.
    pub async fn load_record(&self, record_id: &str) -> Result<Option<Value>, MemoryError> {
        self.db.select_one(record_id).await
    }

    /// Create or update a reembed job row without exposing routing.
    ///
    /// The temporal columns are the job table's, so they are named here
    /// rather than left to the client to guess from the record id.
    pub async fn upsert_job(&self, record_id: &str, payload: Value) -> Result<(), MemoryError> {
        let fields = crate::embedding::queries::EMBEDDING_JOB_TEMPORAL_FIELDS;
        if self.db.select_one(record_id).await?.is_some() {
            self.db.update(record_id, payload, fields).await?;
        } else {
            self.db.create(record_id, payload, fields).await?;
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
        let rows = self.db.query_rows(sql, Some(vars)).await?;

        let count = rows
            .first()
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

        self.db.query_rows(&sql, Some(vars)).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    #[cfg(feature = "streamable-http")]
    use surrealdb::Surreal;
    #[cfg(feature = "streamable-http")]
    use surrealdb::engine::local::Mem;

    use crate::embedding::reembed_store::ReembedStoreClient;
    use crate::shared::temporal::normalize_dt;
    use crate::storage::{DbClient, SurrealDbClient};

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
                crate::knowledge::queries::FACT_TEMPORAL_FIELDS,
            )
            .await
            .expect("seed fact should succeed");
    }

    /// A client told which width it writes vectors at renders the
    /// `fact_embedding_hnsw` index at that width, and the index's own
    /// definition reports it back.
    ///
    /// This is the regression for the HTTP profile hardcoding 1536 in every
    /// `from_prebound*` constructor: a namespace provisioned by a
    /// 2048-dimension provider was indexed at 1536, and every vector that
    /// provider wrote was rejected by its own index.
    #[cfg(feature = "streamable-http")]
    #[tokio::test]
    async fn a_dimension_aware_namespace_reports_the_index_width_it_was_provisioned_at() {
        let db = Surreal::new::<Mem>(()).await.expect("mem engine");
        db.use_ns("dim_aware").use_db("memory").await.expect("bind");
        let client = Arc::new(SurrealDbClient::from_prebound_mem_with_dimension(
            db,
            "dim_aware",
            "warn",
            2048,
        ));
        client
            .apply_migrations("dim_aware")
            .await
            .expect("migrations apply");

        let store = ReembedStoreClient::new(Arc::clone(&client) as Arc<dyn DbClient>, "dim_aware");
        assert_eq!(
            store
                .embedding_index_dimension()
                .await
                .expect("index probe"),
            Some(2048),
            "the rendered HNSW index must carry the dimension the provider writes"
        );
    }

    /// The default constructor still renders 1536: a stdio deployment with no
    /// explicit dimension keeps the process-wide default.
    #[cfg(feature = "streamable-http")]
    #[tokio::test]
    async fn a_namespace_provisioned_without_a_dimension_reports_the_default_width() {
        let db = Surreal::new::<Mem>(()).await.expect("mem engine");
        db.use_ns("dim_default")
            .use_db("memory")
            .await
            .expect("bind");
        let client = Arc::new(SurrealDbClient::from_prebound_mem(
            db,
            "dim_default",
            "warn",
        ));
        client
            .apply_migrations("dim_default")
            .await
            .expect("migrations apply");

        let store =
            ReembedStoreClient::new(Arc::clone(&client) as Arc<dyn DbClient>, "dim_default");
        assert_eq!(
            store
                .embedding_index_dimension()
                .await
                .expect("index probe"),
            Some(crate::config::DEFAULT_EMBEDDING_DIMENSION)
        );
    }

    /// `None` for an index that was never defined, so the caller cannot
    /// mistake "no index" for a mismatch it should repair.
    #[cfg(feature = "streamable-http")]
    #[tokio::test]
    async fn an_absent_index_reports_no_dimension() {
        let db = Surreal::new::<Mem>(()).await.expect("mem engine");
        db.use_ns("dim_absent")
            .use_db("memory")
            .await
            .expect("bind");
        let client = Arc::new(SurrealDbClient::from_prebound_mem(db, "dim_absent", "warn"));

        let store = ReembedStoreClient::new(Arc::clone(&client) as Arc<dyn DbClient>, "dim_absent");
        assert_eq!(
            store
                .embedding_index_dimension()
                .await
                .expect("index probe"),
            None
        );
    }

    /// The activation reconcile asks "does this namespace store any vector?"
    /// before it may re-declare an index at the deployment's width. Zero means
    /// backfill's business; one or more means a reembed is required, and
    /// silently re-declaring would strand vectors the new index rejects.
    #[cfg(feature = "streamable-http")]
    #[tokio::test]
    async fn stored_vectors_are_counted_so_the_reconcile_can_tell_backfill_from_reembed() {
        let db_client = make_db().await;
        let store = ReembedStoreClient::new(
            Arc::clone(&db_client) as Arc<dyn DbClient>,
            "org".to_string(),
        );

        assert_eq!(
            store
                .count_stored_vectors()
                .await
                .expect("count on an empty namespace"),
            0,
            "a namespace that never embedded has nothing for a reembed to rewrite"
        );

        seed_fact(&db_client, "fact:seed", Some("sig:a")).await;
        assert_eq!(
            store
                .count_stored_vectors()
                .await
                .expect("count after seeding"),
            1
        );
    }

    #[tokio::test]
    async fn count_and_select_return_empty_when_fact_table_missing() {
        // Before migrations the `fact` table does not exist; both queries
        // must degrade to empty instead of erroring (same as the old
        // `DbClient` behavior preserved by store relocation).
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

    #[tokio::test]
    async fn remove_embedding_index_is_idempotent() {
        let db_client = make_db().await;
        let store = ReembedStoreClient::new(db_client.clone(), "org");

        // The migrated schema defines the HNSW index, so the first removal
        // drops it; the second removal must report AlreadyAbsent, not error.
        let first = store.remove_embedding_index().await.expect("first removal");
        assert_eq!(
            first,
            crate::embedding::reembed_store::IndexRemoval::Removed
        );

        let second = store
            .remove_embedding_index()
            .await
            .expect("second removal must be idempotent");
        assert_eq!(
            second,
            crate::embedding::reembed_store::IndexRemoval::AlreadyAbsent
        );
    }

    #[tokio::test]
    async fn define_embedding_index_recreates_dropped_index() {
        let db_client = make_db().await;
        let store = ReembedStoreClient::new(db_client.clone(), "org");

        store.remove_embedding_index().await.expect("drop index");
        store
            .define_embedding_index(1536)
            .await
            .expect("recreate index");

        // A fact with a matching-dimension embedding must be writable again,
        // proving the index is live.
        seed_fact(&db_client, "fact:post_index", Some("embsig:target")).await;
    }
}
