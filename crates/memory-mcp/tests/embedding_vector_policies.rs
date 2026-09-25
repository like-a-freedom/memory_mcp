//! The recovery/backfill and re-embedding workflows share one
//! canonical vector policy: an existing vector is never
//! rewritten unless the workflow explicitly allows it.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use memory_mcp::MemoryError;
use memory_mcp::embedding::api::{
    CanonicalVectorPort, StoredVector, VectorApplication, VectorIdentity, VectorWritePolicy,
    update_canonical_vector,
};

#[derive(Debug, Clone, PartialEq)]
struct WriteCall {
    fact_id: String,
    vector: Vec<f64>,
    signature: String,
    has_model: bool,
    has_dimension: bool,
    policy: VectorWritePolicy,
}

struct RecordingPort {
    calls: Mutex<Vec<WriteCall>>,
    stored: Mutex<std::collections::HashMap<String, Option<String>>>,
}

impl RecordingPort {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            stored: Mutex::new(std::collections::HashMap::new()),
        })
    }

    fn with_signature(self: &Arc<Self>, fact_id: &str, signature: &str) -> Arc<Self> {
        self.stored
            .lock()
            .expect("stored lock")
            .insert(fact_id.to_owned(), Some(signature.to_owned()));
        Arc::clone(self)
    }

    fn calls(&self) -> Vec<WriteCall> {
        self.calls.lock().expect("calls lock").clone()
    }
}

#[async_trait::async_trait]
impl CanonicalVectorPort for RecordingPort {
    async fn stored_fact_vector(&self, fact_id: &str) -> Result<StoredVector, MemoryError> {
        Ok(
            match self
                .stored
                .lock()
                .expect("stored lock")
                .get(fact_id)
                .cloned()
                .flatten()
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
        _at: DateTime<Utc>,
        policy: VectorWritePolicy,
    ) -> Result<(), MemoryError> {
        self.calls.lock().expect("calls lock").push(WriteCall {
            fact_id: fact_id.to_owned(),
            vector,
            signature: identity.signature.clone(),
            has_model: identity.model.is_some(),
            has_dimension: true,
            policy,
        });
        self.stored
            .lock()
            .expect("stored lock")
            .insert(fact_id.to_owned(), Some(identity.signature));
        Ok(())
    }
}

fn identity(signature: &str) -> VectorIdentity {
    VectorIdentity {
        provider: "openai".into(),
        model: Some("text-embedding-3-small".into()),
        dimension: 3,
        signature: signature.into(),
    }
}

#[tokio::test]
async fn backfill_never_overwrites_a_fact_that_already_has_a_vector() {
    // A recovery pass only fills gaps. A fact that already
    // carries any vector is left alone, even when its
    // signature is stale.
    let port = RecordingPort::new().with_signature("fact:f1", "old-sig");

    let applied = update_canonical_vector(
        port.as_ref(),
        "fact:f1",
        vec![0.1, 0.2, 0.3],
        &identity("new-sig"),
        Utc::now(),
        VectorWritePolicy::FillMissing,
    )
    .await
    .expect("the pass completes");

    assert_eq!(
        applied,
        VectorApplication::AlreadyCurrent,
        "backfill must not rewrite an existing vector"
    );
    assert!(port.calls().is_empty(), "no owner write is issued");
}

#[tokio::test]
async fn backfill_fills_a_fact_that_has_no_vector_yet() {
    let port = RecordingPort::new();

    let applied = update_canonical_vector(
        port.as_ref(),
        "fact:f1",
        vec![0.1, 0.2, 0.3],
        &identity("sig-a"),
        Utc::now(),
        VectorWritePolicy::FillMissing,
    )
    .await
    .expect("the pass completes");

    assert_eq!(applied, VectorApplication::Applied);
    assert_eq!(port.calls().len(), 1);
}

#[tokio::test]
async fn reembed_rewrites_a_stale_vector_but_keeps_the_current_one() {
    let port = RecordingPort::new().with_signature("fact:stale", "sig-a");
    let stale = update_canonical_vector(
        port.as_ref(),
        "fact:stale",
        vec![0.7, 0.8, 0.9],
        &identity("sig-b"),
        Utc::now(),
        VectorWritePolicy::ReplaceStale,
    )
    .await
    .expect("a signature change is applied");
    assert_eq!(stale, VectorApplication::Applied);

    let current = update_canonical_vector(
        port.as_ref(),
        "fact:stale",
        vec![0.1, 0.1, 0.1],
        &identity("sig-b"),
        Utc::now(),
        VectorWritePolicy::ReplaceStale,
    )
    .await
    .expect("repeating the current signature is not an error");
    assert_eq!(
        current,
        VectorApplication::AlreadyCurrent,
        "an up-to-date fact is not rewritten"
    );

    let calls = port.calls();
    assert_eq!(calls.len(), 1, "only the stale fact is rewritten");
    assert_eq!(calls[0].signature, "sig-b");
}

#[tokio::test]
async fn the_policy_is_recorded_on_the_owner_write() {
    // Both workflows must be able to state which policy
    // authorised the write, so the audit trail does not
    // conflate a gap fill with a signature rotation.
    let port = RecordingPort::new();

    update_canonical_vector(
        port.as_ref(),
        "fact:f1",
        vec![0.1, 0.2, 0.3],
        &identity("sig-a"),
        Utc::now(),
        VectorWritePolicy::FillMissing,
    )
    .await
    .expect("fill applies");

    let call = &port.calls()[0];
    assert!(call.has_model, "the model is stored when known");
    assert!(call.has_dimension, "the dimension is always stored");
    assert_eq!(
        call.policy,
        VectorWritePolicy::FillMissing,
        "the owner write records which policy authorised it"
    );
}
