//! One owner for the `triple` table.
//!
//! Semantic triples are extracted from facts during episode ingestion.
//! All reads and writes on the table go through this narrow store so the
//! service layer expresses intent (create / find-conflicting / close)
//! instead of supplying SQL. Closes delegate to the bi-temporal close
//! owner (ADR-0039).

use std::sync::Arc;

use serde_json::json;

use crate::service::MemoryError;

use super::client::{BoundDbClient, DbClient};
use super::close::{CloseStoreClient, CloseTimestamps};
use super::helpers::record_id_from_json_value;

/// Narrow store that owns every read/write on the `triple` table in the
/// Active Namespace.
#[derive(Clone)]
pub(crate) struct TripleStoreClient {
    db: BoundDbClient,
}

impl TripleStoreClient {
    pub(crate) fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            db: BoundDbClient::new(db, namespace),
        }
    }

    /// Persists a semantic triple row and returns its record id.
    pub(crate) async fn create_triple(
        &self,
        subject: &str,
        predicate: &str,
        object: &str,
        confidence: f64,
        source_fact_id: &str,
    ) -> Result<String, MemoryError> {
        let row = self
            .db
            .create(
                "triple",
                json!({
                    "subject": subject,
                    "predicate": predicate,
                    "object": object,
                    "confidence": confidence,
                    "source_fact_id": source_fact_id,
                }),
            )
            .await?;
        row.get("id")
            .and_then(record_id_from_json_value)
            .ok_or_else(|| MemoryError::Storage("triple create returned no record id".to_string()))
    }

    /// Active triples with the same (subject, predicate) but a different
    /// object — the conflict set for singleton-predicate supersession.
    pub(crate) async fn find_conflicting_triple_ids(
        &self,
        subject: &str,
        predicate: &str,
        exclude_object: &str,
    ) -> Result<Vec<String>, MemoryError> {
        let sql = "SELECT id FROM triple \
            WHERE subject = $subject AND predicate = $predicate \
            AND object != $object AND t_invalid IS NONE LIMIT 10";
        let vars = json!({
            "subject": subject,
            "predicate": predicate,
            "object": exclude_object,
        });
        let rows = self.db.query_rows(sql, Some(vars)).await?;
        Ok(rows
            .iter()
            .filter_map(|row| row.get("id").and_then(record_id_from_json_value))
            .collect())
    }

    /// Closes a triple via the bi-temporal close owner (ADR-0039): both
    /// `t_invalid` and `t_invalid_ingested` are set together. Triples carry
    /// no `invalidation_reason` field, so no reason is persisted.
    pub(crate) async fn close_triple(&self, triple_id: &str) -> Result<(), MemoryError> {
        CloseStoreClient::from_bound(self.db.clone())
            .close_record(triple_id, &CloseTimestamps::now(), None)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn embedded_triple_store() -> (TripleStoreClient, Arc<crate::storage::SurrealDbClient>) {
        let db_name = format!(
            "triple_store_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        );
        let db_client = Arc::new(
            crate::storage::SurrealDbClient::connect_in_memory_with_namespaces(
                &db_name,
                &["org".to_string()],
                "error",
            )
            .await
            .expect("connect in memory db"),
        );
        db_client
            .apply_migrations("org")
            .await
            .expect("apply migrations");
        let store = TripleStoreClient::new(db_client.clone(), "org");
        (store, db_client)
    }

    async fn select_triple(
        db_client: &Arc<crate::storage::SurrealDbClient>,
        triple_id: &str,
    ) -> serde_json::Value {
        db_client
            .select_one(triple_id, "org")
            .await
            .expect("select triple")
            .expect("triple must exist")
    }

    #[tokio::test]
    async fn create_triple_persists_all_fields_and_returns_record_id() {
        let (store, db_client) = embedded_triple_store().await;

        let triple_id = store
            .create_triple("alice", "works_at", "acme", 0.87, "fact:src-1")
            .await
            .expect("create triple should succeed");

        assert!(
            triple_id.starts_with("triple:"),
            "returned id must address the triple table, got {triple_id}"
        );

        let stored = select_triple(&db_client, &triple_id).await;
        assert_eq!(
            stored.get("subject").and_then(|v| v.as_str()),
            Some("alice")
        );
        assert_eq!(
            stored.get("predicate").and_then(|v| v.as_str()),
            Some("works_at")
        );
        assert_eq!(stored.get("object").and_then(|v| v.as_str()), Some("acme"));
        assert_eq!(
            stored.get("confidence").and_then(|v| v.as_f64()),
            Some(0.87)
        );
        assert_eq!(
            stored.get("source_fact_id").and_then(|v| v.as_str()),
            Some("fact:src-1")
        );
        assert!(
            stored.get("t_invalid").is_none_or(|v| v.is_null()),
            "new triples must be active: {stored}"
        );
    }

    #[tokio::test]
    async fn find_conflicting_triple_ids_returns_active_same_subject_predicate_other_object() {
        let (store, _db_client) = embedded_triple_store().await;

        let conflicting = store
            .create_triple("alice", "works_at", "acme", 0.9, "fact:1")
            .await
            .expect("seed triple");
        let _same_object = store
            .create_triple("alice", "works_at", "acme hq", 0.9, "fact:2")
            .await
            .expect("seed triple");
        let _other_predicate = store
            .create_triple("alice", "lives_in", "berlin", 0.9, "fact:3")
            .await
            .expect("seed triple");
        let _other_subject = store
            .create_triple("bob", "works_at", "globex", 0.9, "fact:4")
            .await
            .expect("seed triple");

        let found = store
            .find_conflicting_triple_ids("alice", "works_at", "acme hq")
            .await
            .expect("find conflicting triples");

        assert_eq!(found, vec![conflicting]);
    }

    #[tokio::test]
    async fn find_conflicting_triple_ids_skips_closed_triples() {
        let (store, _db_client) = embedded_triple_store().await;

        let closed = store
            .create_triple("alice", "works_at", "acme", 0.9, "fact:1")
            .await
            .expect("seed triple");
        store.close_triple(&closed).await.expect("close triple");
        let active = store
            .create_triple("alice", "works_at", "globex", 0.9, "fact:2")
            .await
            .expect("seed triple");

        let found = store
            .find_conflicting_triple_ids("alice", "works_at", "initech")
            .await
            .expect("find conflicting triples");

        assert_eq!(
            found,
            vec![active],
            "closed triples must not appear in the conflict set"
        );
    }

    #[tokio::test]
    async fn close_triple_sets_both_bitemporal_fields() {
        let (store, db_client) = embedded_triple_store().await;

        let triple_id = store
            .create_triple("alice", "works_at", "acme", 0.9, "fact:1")
            .await
            .expect("seed triple");
        store.close_triple(&triple_id).await.expect("close triple");

        let stored = select_triple(&db_client, &triple_id).await;
        assert!(
            stored.get("t_invalid").is_some_and(|v| !v.is_null()),
            "t_invalid must be closed: {stored}"
        );
        assert!(
            stored
                .get("t_invalid_ingested")
                .is_some_and(|v| !v.is_null()),
            "t_invalid_ingested must be closed whenever t_invalid is (ADR-0039): {stored}"
        );
    }
}
