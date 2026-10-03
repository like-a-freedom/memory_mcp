//! Embedding capability contract: canonical vector updates are
//! prepared here and applied through a narrow owner-approved
//! port, so the vector endpoint never calls embedding again.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use memory_mcp::MemoryError;
use memory_mcp::embedding::api::{
    CanonicalVectorPort, StoredVector, VectorApplication, VectorIdentity, VectorWritePolicy,
    prepare_canonical_vector, update_canonical_vector,
};
use memory_mcp::storage::{DbClient, OwnedTable, SurrealDbClient};

#[derive(Debug, Clone, PartialEq)]
struct WriteCall {
    fact_id: String,
    vector: Vec<f64>,
    identity: VectorIdentity,
    at: DateTime<Utc>,
}

struct RecordingPort {
    calls: Mutex<Vec<WriteCall>>,
    stored_signature: Mutex<Option<String>>,
}

impl RecordingPort {
    fn fresh() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            stored_signature: Mutex::new(None),
        })
    }

    fn calls(&self) -> Vec<WriteCall> {
        self.calls.lock().expect("calls lock").clone()
    }
}

#[async_trait::async_trait]
impl CanonicalVectorPort for RecordingPort {
    async fn stored_fact_vector(&self, _fact_id: &str) -> Result<StoredVector, MemoryError> {
        Ok(
            match self
                .stored_signature
                .lock()
                .expect("signature lock")
                .clone()
            {
                Some(signature) => StoredVector::Present { signature },
                None => StoredVector::Absent,
            },
        )
    }

    async fn apply_fact_vector(
        &self,
        fact_id: &str,
        vector: Vec<f64>,
        identity: VectorIdentity,
        at: DateTime<Utc>,
        _policy: VectorWritePolicy,
    ) -> Result<VectorApplication, MemoryError> {
        *self.stored_signature.lock().expect("signature lock") = Some(identity.signature.clone());
        self.calls.lock().expect("calls lock").push(WriteCall {
            fact_id: fact_id.to_owned(),
            vector,
            identity,
            at,
        });
        Ok(VectorApplication::Applied)
    }
}

fn target(signature: &str, dimension: usize) -> VectorIdentity {
    VectorIdentity {
        provider: "openai".into(),
        model: Some("text-embedding-3-small".into()),
        dimension,
        signature: signature.into(),
    }
}

#[tokio::test]
async fn vector_update_rejects_a_dimension_that_does_not_match_the_target() {
    let port = RecordingPort::fresh();
    let identity = target("sig-a", 3);

    let error = update_canonical_vector(
        port.as_ref(),
        "fact:f1",
        vec![0.1, 0.2],
        &identity,
        Utc::now(),
        VectorWritePolicy::ReplaceStale,
    )
    .await
    .expect_err("a 2-length vector must not be written under a 3-dimension target");

    assert!(
        matches!(&error, MemoryError::Validation(message) if message.contains("dimension")),
        "expected a dimension validation error, got {error:?}"
    );
    assert!(
        port.calls().is_empty(),
        "a rejected vector must not reach the owner port"
    );
}

#[tokio::test]
async fn vector_update_writes_once_and_advances_the_stored_signature() {
    let port = RecordingPort::fresh();
    let identity = target("sig-a", 3);
    let at = Utc::now();

    let applied = update_canonical_vector(
        port.as_ref(),
        "fact:f1",
        vec![0.1, 0.2, 0.3],
        &identity,
        at,
        VectorWritePolicy::ReplaceStale,
    )
    .await
    .expect("first update applies");

    assert_eq!(applied, VectorApplication::Applied);
    let calls = port.calls();
    assert_eq!(calls.len(), 1, "exactly one owner write per update");
    assert_eq!(calls[0].fact_id, "fact:f1");
    assert_eq!(calls[0].vector, vec![0.1, 0.2, 0.3]);
    assert_eq!(calls[0].identity, identity);
    assert_eq!(calls[0].at, at);
}

#[tokio::test]
async fn repeating_the_current_signature_is_a_no_op() {
    let port = RecordingPort::fresh();
    let identity = target("sig-a", 3);

    update_canonical_vector(
        port.as_ref(),
        "fact:f1",
        vec![0.1, 0.2, 0.3],
        &identity,
        Utc::now(),
        VectorWritePolicy::ReplaceStale,
    )
    .await
    .expect("first update applies");
    let repeat = update_canonical_vector(
        port.as_ref(),
        "fact:f1",
        vec![0.4, 0.5, 0.6],
        &identity,
        Utc::now(),
        VectorWritePolicy::ReplaceStale,
    )
    .await
    .expect("repeat is not an error");

    assert_eq!(
        repeat,
        VectorApplication::AlreadyCurrent,
        "an unchanged signature must not rewrite the record"
    );
    assert_eq!(
        port.calls().len(),
        1,
        "the redundant write must never reach the owner port"
    );
}

#[tokio::test]
async fn a_changed_signature_rewrites_the_record() {
    let port = RecordingPort::fresh();

    update_canonical_vector(
        port.as_ref(),
        "fact:f1",
        vec![0.1, 0.2, 0.3],
        &target("sig-a", 3),
        Utc::now(),
        VectorWritePolicy::ReplaceStale,
    )
    .await
    .expect("first update applies");

    let rotated = update_canonical_vector(
        port.as_ref(),
        "fact:f1",
        vec![0.7, 0.8, 0.9],
        &target("sig-b", 3),
        Utc::now(),
        VectorWritePolicy::ReplaceStale,
    )
    .await
    .expect("signature change applies");

    assert_eq!(rotated, VectorApplication::Applied);
    let calls = port.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].identity.signature, "sig-b");
}

#[tokio::test]
async fn preparation_never_generates_and_only_validates_identity() {
    let identity = target("sig-a", 3);
    let prepared = prepare_canonical_vector(vec![0.1, 0.2, 0.3], &identity, Utc::now())
        .expect("a matching vector is prepared");
    assert_eq!(prepared.vector, vec![0.1, 0.2, 0.3]);
    assert_eq!(prepared.identity, identity);

    assert!(
        prepare_canonical_vector(vec![0.1], &identity, Utc::now()).is_err(),
        "preparation enforces the target dimension"
    );
    assert!(
        prepare_canonical_vector(Vec::new(), &identity, Utc::now()).is_err(),
        "an empty vector is not a valid embedding"
    );
}

// ─── The owner adapter, against the real storage it writes ───────────────
//
// Everything above tests the capability's decisions with a recording port.
// The tests below test the one thing a recording port cannot: what the write
// statement actually does to a stored fact. A vector write that carries a
// whole record can restore fields another writer changed, and no in-memory
// fake can show that.

/// A `DbClient` that lets another writer land immediately before a write.
///
/// The lost update this pins is real but needs a specific interleaving to
/// observe: a vector write that carries the whole record overwrites whatever
/// changed after its read. Injecting the concurrent change here — rather than
/// hoping to hit a race — makes the test deterministic and keeps it honest
/// about what it proves.
struct InterleavingDb {
    inner: Arc<SurrealDbClient>,
    /// The fact whose concurrent write is still pending, consumed once.
    pending: Mutex<Option<String>>,
}

impl InterleavingDb {
    fn new(inner: Arc<SurrealDbClient>, fact_id: &str) -> Arc<Self> {
        Arc::new(Self {
            inner,
            pending: Mutex::new(Some(fact_id.to_string())),
        })
    }

    /// Apply the pending concurrent change, if this call targets it.
    async fn race_ahead_of(&self, namespace: &str) -> Result<(), MemoryError> {
        let target = self.pending.lock().expect("pending lock").take();
        let Some(fact_id) = target else {
            return Ok(());
        };
        // A retrieval heat update: the access log owns this field, and a
        // vector write has no business rewriting it.
        self.inner
            .update(
                &fact_id,
                serde_json::json!({ "access_count": 7 }),
                namespace,
                memory_mcp::knowledge::queries::FACT_TEMPORAL_FIELDS,
            )
            .await?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl DbClient for InterleavingDb {
    async fn select_one(
        &self,
        record_id: &str,
        namespace: &str,
    ) -> Result<Option<serde_json::Value>, MemoryError> {
        self.inner.select_one(record_id, namespace).await
    }

    async fn select_table(
        &self,
        table: OwnedTable,
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
        self.race_ahead_of(namespace).await?;
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
        self.race_ahead_of(namespace).await?;
        self.inner.query(sql, vars, namespace).await
    }

    async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError> {
        self.inner.apply_migrations(namespace).await
    }
}

const ADAPTER_NAMESPACE: &str = "org";
/// The fact table's HNSW index dimension, which the migrations fix. A vector of
/// any other length is refused by the engine, not by this port.
const ADAPTER_DIMENSION: usize = 1536;

async fn adapter_db() -> Arc<SurrealDbClient> {
    let name = format!(
        "embedding_adapter_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    );
    let db = Arc::new(
        SurrealDbClient::connect_in_memory_with_namespaces(
            &name,
            &[ADAPTER_NAMESPACE.to_string()],
            "warn",
        )
        .await
        .expect("connect in memory"),
    );
    db.apply_migrations(ADAPTER_NAMESPACE)
        .await
        .expect("migrations");
    db
}

fn adapter_identity(signature: &str) -> VectorIdentity {
    VectorIdentity {
        provider: "openai".to_string(),
        model: Some("text-embedding-3-small".to_string()),
        dimension: ADAPTER_DIMENSION,
        signature: signature.to_string(),
    }
}

async fn seed_adapter_fact(db: &Arc<SurrealDbClient>, fact_id: &str) -> serde_json::Value {
    let now = memory_mcp::shared::temporal::normalize_dt(memory_mcp::shared::temporal::now());
    db.create(
        fact_id,
        serde_json::json!({
            "fact_id": fact_id,
            "fact_type": "note",
            "content": format!("content of {fact_id}"),
            "quote": format!("quote of {fact_id}"),
            "source_episode": "episode:seed",
            "t_valid": now,
            "t_ingested": now,
            "confidence": 0.9,
            "index_keys": ["alpha"],
            "access_count": 0,
            "entity_links": [],
            "scope": "org",
            "policy_tags": [],
            "provenance": {"source_episode": "episode:seed"}
        }),
        ADAPTER_NAMESPACE,
        memory_mcp::knowledge::queries::FACT_TEMPORAL_FIELDS,
    )
    .await
    .expect("seed fact")
}

async fn read_fact(db: &Arc<SurrealDbClient>, fact_id: &str) -> serde_json::Value {
    db.select_one(fact_id, ADAPTER_NAMESPACE)
        .await
        .expect("read fact")
        .unwrap_or_else(|| panic!("{fact_id} must exist"))
}

fn adapter(db: Arc<dyn DbClient>) -> memory_mcp::embedding::infra::FactVectorAdapter {
    memory_mcp::embedding::infra::FactVectorAdapter::new(db, ADAPTER_NAMESPACE)
}

/// A vector write must change embedding fields and nothing else. The access
/// counter is written by retrieval, and a re-embed that restores an older
/// count loses real work.
#[tokio::test]
async fn replace_stale_preserves_concurrent_fact_access() {
    let db = adapter_db().await;
    let fact_id = "fact:adapter_race";
    seed_adapter_fact(&db, fact_id).await;
    let racing: Arc<dyn DbClient> = InterleavingDb::new(Arc::clone(&db), fact_id);
    let port = adapter(racing);

    let applied = update_canonical_vector(
        &port,
        fact_id,
        vec![0.1; ADAPTER_DIMENSION],
        &adapter_identity("sig-new"),
        Utc::now(),
        VectorWritePolicy::ReplaceStale,
    )
    .await
    .expect("vector write succeeds");
    assert_eq!(applied, VectorApplication::Applied);

    let stored = read_fact(&db, fact_id).await;
    assert_eq!(
        stored["access_count"], 7,
        "the vector write must not restore the access count it did not own"
    );
    assert_eq!(
        stored["embedding_signature"], "sig-new",
        "the vector write must still land"
    );
    assert_eq!(stored["embedding_model"], "text-embedding-3-small");
    assert_eq!(stored["embedding_dimension"], ADAPTER_DIMENSION);
    assert!(
        stored["content"]
            .as_str()
            .is_some_and(|text| text.contains("adapter_race"))
    );
    assert_eq!(stored["scope"], "org");
    assert_eq!(stored["index_keys"][0], "alpha");
}

/// A gap fill must not overwrite a vector that already exists, even when it
/// raced: the second writer's answer is "already current", not "applied".
#[tokio::test]
async fn fill_missing_loser_reports_already_current() {
    let db = adapter_db().await;
    let fact_id = "fact:adapter_gap";
    seed_adapter_fact(&db, fact_id).await;
    let port = adapter(Arc::clone(&db) as Arc<dyn DbClient>);

    let winner = update_canonical_vector(
        &port,
        fact_id,
        vec![0.1; ADAPTER_DIMENSION],
        &adapter_identity("sig-winner"),
        Utc::now(),
        VectorWritePolicy::FillMissing,
    )
    .await
    .expect("first fill writes");
    assert_eq!(winner, VectorApplication::Applied);

    let loser = update_canonical_vector(
        &port,
        fact_id,
        vec![0.9; ADAPTER_DIMENSION],
        &adapter_identity("sig-loser"),
        Utc::now(),
        VectorWritePolicy::FillMissing,
    )
    .await
    .expect("second fill is a refusal, not an error");
    assert_eq!(
        loser,
        VectorApplication::AlreadyCurrent,
        "a refused predicate is not a write"
    );
    assert_eq!(
        read_fact(&db, fact_id).await["embedding_signature"],
        "sig-winner"
    );
}

/// The predicate cannot distinguish "already has a vector" from "no such
/// fact", so the adapter must answer that question separately.
#[tokio::test]
async fn canonical_write_missing_fact_is_not_found() {
    let db = adapter_db().await;
    let port = adapter(Arc::clone(&db) as Arc<dyn DbClient>);

    let error = update_canonical_vector(
        &port,
        "fact:does_not_exist",
        vec![0.1; ADAPTER_DIMENSION],
        &adapter_identity("sig-new"),
        Utc::now(),
        VectorWritePolicy::ReplaceStale,
    )
    .await
    .expect_err("a write for an absent fact is an error");
    assert!(
        matches!(error, MemoryError::NotFound(_)),
        "expected NotFound, got {error:?}"
    );
}

/// A record id is data, not SQL. An id carrying the identifier delimiter must
/// not reach a different record — it must simply not exist.
#[tokio::test]
async fn vector_write_rejects_a_forged_record_target() {
    let db = adapter_db().await;
    let innocent = "fact:adapter_innocent";
    seed_adapter_fact(&db, innocent).await;
    let port = adapter(Arc::clone(&db) as Arc<dyn DbClient>);

    let forged = format!(
        "fact:x⟩ SET embedding_signature = 'sig-forged' WHERE fact_id = '{innocent}' REMOVE"
    );
    let outcome = update_canonical_vector(
        &port,
        &forged,
        vec![0.1; ADAPTER_DIMENSION],
        &adapter_identity("sig-forged"),
        Utc::now(),
        VectorWritePolicy::ReplaceStale,
    )
    .await;

    assert!(
        outcome.is_err(),
        "a forged record target must not be written as SQL: {outcome:?}"
    );
    let stored = read_fact(&db, innocent).await;
    assert!(
        stored
            .get("embedding")
            .is_none_or(serde_json::Value::is_null),
        "the forged id must not reach another record: {stored}"
    );
}
