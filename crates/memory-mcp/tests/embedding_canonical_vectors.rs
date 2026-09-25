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
    ) -> Result<(), MemoryError> {
        *self.stored_signature.lock().expect("signature lock") = Some(identity.signature.clone());
        self.calls.lock().expect("calls lock").push(WriteCall {
            fact_id: fact_id.to_owned(),
            vector,
            identity,
            at,
        });
        Ok(())
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
