//! Narrow store owning the durable `embedding_state:fact` record (ADR-0043).
//!
//! Every write to the embedding state record goes through this store: it owns
//! the record ID, the typed status vocabulary, the record shape, and the
//! upsert protocol. Startup bootstrap, Embedding Recovery, and Reembed all
//! write through it. The record is the durable crash-resume marker for
//! recovery (ADR-0042), so its schema is a load-bearing invariant owned here.
//!
//! Reads stay JSON-shaped at the decision seam: `decide_embedding_startup`
//! is a pure, exhaustively tested function over the record JSON.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::service::MemoryError;
use crate::storage::{BoundDbClient, DbClient};

/// The single record ID for the embedding state.
pub(crate) const EMBEDDING_STATE_RECORD_ID: &str = "embedding_state:fact";

/// Typed status vocabulary for the `embedding_state:fact` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmbeddingStateStatus {
    /// Embeddings are active and consistent with the stored signature.
    Ready,
    /// Recovery installed a provider but offline facts still need backfill.
    BackfillPending,
    /// A reembed job is rewriting fact embeddings.
    Rebuilding,
    /// A reembed job exceeded its failure quota.
    Failed,
}

impl EmbeddingStateStatus {
    fn as_str(self) -> &'static str {
        match self {
            EmbeddingStateStatus::Ready => "ready",
            EmbeddingStateStatus::BackfillPending => "backfill_pending",
            EmbeddingStateStatus::Rebuilding => "rebuilding",
            EmbeddingStateStatus::Failed => "failed",
        }
    }
}

/// One owner for the `embedding_state:fact` record.
pub(crate) struct EmbeddingStateStoreClient {
    db: BoundDbClient,
}

impl EmbeddingStateStoreClient {
    pub(crate) fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            db: BoundDbClient::new(db, namespace),
        }
    }

    pub(crate) fn from_bound(db: BoundDbClient) -> Self {
        Self { db }
    }

    /// Loads the current embedding state record, or `None` if absent.
    pub(crate) async fn load_state(&self) -> Result<Option<Value>, MemoryError> {
        self.db.select_one(EMBEDDING_STATE_RECORD_ID).await
    }

    /// Bootstrap/recovery write: a resolved embedding identity with status
    /// `ready` or `backfill_pending`.
    pub(crate) async fn upsert_bootstrap_state(
        &self,
        status: EmbeddingStateStatus,
        active_signature: &str,
        provider: &str,
        model: Option<&str>,
        dimension: usize,
    ) -> Result<(), MemoryError> {
        let payload = json!({
            "status": status.as_str(),
            "active_signature": active_signature,
            "provider": provider,
            "model": model,
            "dimension": dimension,
            "updated_at": chrono::Utc::now().to_rfc3339(),
        });
        self.upsert(payload).await
    }

    /// Reembed write: job lifecycle status with optional identity fields.
    ///
    /// Fields passed as `None` are omitted from the payload; with merge
    /// update semantics the previously stored value survives, matching the
    /// historical `write_embedding_state` behavior.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn upsert_job_state(
        &self,
        status: EmbeddingStateStatus,
        provider: &str,
        model: Option<&str>,
        dimension: Option<usize>,
        active_signature: Option<&str>,
        last_job_id: Option<&str>,
    ) -> Result<(), MemoryError> {
        let mut payload = serde_json::Map::from_iter([
            ("status".to_string(), json!(status.as_str())),
            ("provider".to_string(), json!(provider)),
            ("model".to_string(), json!(model)),
            ("dimension".to_string(), json!(dimension)),
            (
                "updated_at".to_string(),
                json!(chrono::Utc::now().to_rfc3339()),
            ),
        ]);
        if let Some(active_signature) = active_signature {
            payload.insert("active_signature".to_string(), json!(active_signature));
        }
        if let Some(last_job_id) = last_job_id {
            payload.insert("last_job_id".to_string(), json!(last_job_id));
        }
        self.upsert(Value::Object(payload)).await
    }

    async fn upsert(&self, payload: Value) -> Result<(), MemoryError> {
        if self
            .db
            .select_one(EMBEDDING_STATE_RECORD_ID)
            .await?
            .is_some()
        {
            self.db.update(EMBEDDING_STATE_RECORD_ID, payload).await?;
        } else {
            self.db.create(EMBEDDING_STATE_RECORD_ID, payload).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::{Value, json};

    use crate::storage::{
        DbClient, SurrealDbClient,
        embedding_state_store::{EmbeddingStateStatus, EmbeddingStateStoreClient},
    };

    async fn make_db() -> Arc<SurrealDbClient> {
        let db_name = format!(
            "embedding_state_store_test_{}",
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

    #[tokio::test]
    async fn bootstrap_upsert_creates_then_updates_state_record() {
        let db = make_db().await;
        let store = EmbeddingStateStoreClient::new(db.clone(), "org");

        store
            .upsert_bootstrap_state(
                EmbeddingStateStatus::BackfillPending,
                "embsig:test",
                "test-provider",
                Some("test-model"),
                384,
            )
            .await
            .expect("first upsert");

        let state = store.load_state().await.expect("load").expect("present");
        assert_eq!(
            state.get("status").and_then(Value::as_str),
            Some("backfill_pending")
        );
        assert_eq!(
            state.get("active_signature").and_then(Value::as_str),
            Some("embsig:test")
        );
        assert_eq!(state.get("dimension").and_then(Value::as_u64), Some(384));

        // Second write updates in place and flips the status.
        store
            .upsert_bootstrap_state(
                EmbeddingStateStatus::Ready,
                "embsig:test",
                "test-provider",
                Some("test-model"),
                384,
            )
            .await
            .expect("second upsert");

        let state = store.load_state().await.expect("load").expect("present");
        assert_eq!(state.get("status").and_then(Value::as_str), Some("ready"));
    }

    #[tokio::test]
    async fn job_state_omits_absent_optional_fields_and_keeps_prior_values() {
        let db = make_db().await;
        let store = EmbeddingStateStoreClient::new(db.clone(), "org");

        store
            .upsert_bootstrap_state(
                EmbeddingStateStatus::Ready,
                "embsig:old",
                "test-provider",
                None,
                384,
            )
            .await
            .expect("bootstrap");

        // Reembed marks `rebuilding` without a signature: the key is omitted,
        // and merge semantics keep the previously stored signature.
        store
            .upsert_job_state(
                EmbeddingStateStatus::Rebuilding,
                "test-provider",
                None,
                Some(384),
                None,
                Some("embedding_job:fact_reembed"),
            )
            .await
            .expect("rebuilding");

        let state = store.load_state().await.expect("load").expect("present");
        assert_eq!(
            state.get("status").and_then(Value::as_str),
            Some("rebuilding")
        );
        assert_eq!(
            state.get("active_signature").and_then(Value::as_str),
            Some("embsig:old")
        );
        assert_eq!(
            state.get("last_job_id").and_then(Value::as_str),
            Some("embedding_job:fact_reembed")
        );

        // Final ready write carries the new signature.
        store
            .upsert_job_state(
                EmbeddingStateStatus::Ready,
                "test-provider",
                None,
                Some(384),
                Some("embsig:new"),
                Some("embedding_job:fact_reembed"),
            )
            .await
            .expect("ready");

        let state = store.load_state().await.expect("load").expect("present");
        assert_eq!(state.get("status").and_then(Value::as_str), Some("ready"));
        assert_eq!(
            state.get("active_signature").and_then(Value::as_str),
            Some("embsig:new")
        );
    }

    #[tokio::test]
    async fn load_state_returns_none_when_absent() {
        let db = make_db().await;
        let store = EmbeddingStateStoreClient::new(db, "org");
        let state = store.load_state().await.expect("load");
        assert!(state.is_none());
    }

    #[test]
    fn status_vocabulary_serializes_to_stored_strings() {
        assert_eq!(EmbeddingStateStatus::Ready.as_str(), "ready");
        assert_eq!(
            EmbeddingStateStatus::BackfillPending.as_str(),
            "backfill_pending"
        );
        assert_eq!(EmbeddingStateStatus::Rebuilding.as_str(), "rebuilding");
        assert_eq!(EmbeddingStateStatus::Failed.as_str(), "failed");
        // Pin the exact JSON shape for the bootstrap payload.
        let payload = json!({
            "status": EmbeddingStateStatus::Ready.as_str(),
        });
        assert_eq!(payload["status"], "ready");
    }
}
