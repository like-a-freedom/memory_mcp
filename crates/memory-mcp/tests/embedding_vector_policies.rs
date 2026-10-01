//! The recovery/backfill and re-embedding workflows share one
//! canonical vector policy: an existing vector is never
//! rewritten unless the workflow explicitly allows it.
//!
//! The second half of the file is about the *other* half of the same
//! question. `update_canonical_vector` already refuses to rewrite a vector the
//! caller happened to be holding; `generate_and_update` is what stops a
//! caller from reaching past it to the provider in the first place, and
//! reports why a write did not happen in a form a metric can carry.

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use memory_mcp::MemoryError;
use memory_mcp::embedding::api::{
    CanonicalVectorPort, EmbeddingGeneration, GenerationOutcome, SkipReason, StoredVector,
    VectorApplication, VectorIdentity, VectorWritePolicy, generate_and_update,
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

// ---------------------------------------------------------------------------
// Generation and write, as one operation.
//
// The adapters below are the two halves of the seam `generate_and_update`
// closes over. A recording generator is the second adapter of
// `EmbeddingGeneration` beside `EmbeddingService`, and `RecordingPort` is
// already the second adapter of the write half.
// ---------------------------------------------------------------------------

/// A generator that returns a fixed outcome and records the text it was asked
/// to embed.
struct RecordingGenerator {
    outcome: GenerationOutcome,
    seen: Mutex<Vec<String>>,
}

impl RecordingGenerator {
    fn generating(vector: Vec<f64>) -> Arc<Self> {
        Arc::new(Self {
            outcome: GenerationOutcome::Generated(vector),
            seen: Mutex::new(Vec::new()),
        })
    }

    fn skipping(reason: SkipReason) -> Arc<Self> {
        Arc::new(Self {
            outcome: GenerationOutcome::Skipped(reason),
            seen: Mutex::new(Vec::new()),
        })
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().expect("seen lock").clone()
    }
}

#[async_trait::async_trait]
impl EmbeddingGeneration for RecordingGenerator {
    async fn generate(&self, input: &str) -> Result<GenerationOutcome, MemoryError> {
        self.seen.lock().expect("seen lock").push(input.to_owned());
        Ok(self.outcome.clone())
    }
}

/// A disabled provider is a configuration state, not a failure: a server
/// started without embeddings must still ingest facts, and must say so in a
/// form a metric can label rather than as an error string.
#[tokio::test]
async fn a_disabled_provider_skips_the_write_and_reports_why() {
    let port = RecordingPort::new();
    let generator = RecordingGenerator::skipping(SkipReason::ProviderDisabled);

    let applied = generate_and_update(
        generator.as_ref(),
        port.as_ref(),
        "fact:f1",
        "note\nhello",
        &identity("sig-a"),
        VectorWritePolicy::FillMissing,
    )
    .await
    .expect("a disabled provider is not an error");

    assert_eq!(
        applied,
        VectorApplication::Skipped(SkipReason::ProviderDisabled),
        "the outcome must name the reason, so a metric can carry it as a bounded label"
    );
    assert!(
        port.calls().is_empty(),
        "no owner write is issued when nothing was generated"
    );
    assert_eq!(
        generator.seen(),
        vec!["note\nhello".to_owned()],
        "the generator is still asked, so the skip is the provider's answer and \
         not a decision taken before it"
    );
}

/// The happy path: a generated vector reaches the owner write, under the
/// policy the caller named. This is the assertion the two bypassed call sites
/// could not make — one skipped the policy entirely, the other skipped the
/// generation contract.
#[tokio::test]
async fn a_generated_vector_is_written_under_the_named_policy() {
    let port = RecordingPort::new();
    let generator = RecordingGenerator::generating(vec![0.4, 0.5, 0.6]);

    let applied = generate_and_update(
        generator.as_ref(),
        port.as_ref(),
        "fact:f1",
        "note\nhello",
        &identity("sig-a"),
        VectorWritePolicy::FillMissing,
    )
    .await
    .expect("generation and write complete");

    assert_eq!(applied, VectorApplication::Applied);
    let calls = port.calls();
    assert_eq!(calls.len(), 1, "exactly one owner write");
    assert_eq!(calls[0].fact_id, "fact:f1");
    assert_eq!(calls[0].vector, vec![0.4, 0.5, 0.6]);
    assert_eq!(calls[0].policy, VectorWritePolicy::FillMissing);
}

/// A vector of the wrong length is a validation failure, not a silent write.
/// The check lives in `prepare_canonical_vector`, and routing generation
/// through `update_canonical_vector` is what makes a caller inherit it for
/// free — which is the point of the seam.
#[tokio::test]
async fn a_mis_dimensioned_vector_is_refused_before_the_write() {
    let port = RecordingPort::new();
    let generator = RecordingGenerator::generating(vec![0.1, 0.2]);

    let outcome = generate_and_update(
        generator.as_ref(),
        port.as_ref(),
        "fact:f1",
        "note\nhello",
        &identity("sig-a"),
        VectorWritePolicy::FillMissing,
    )
    .await;

    assert!(
        outcome.is_err(),
        "a two-dimensional vector must not satisfy a three-dimensional target"
    );
    assert!(
        port.calls().is_empty(),
        "the refusal happens before the owner write, not after"
    );
}

/// Every generation reaches the provider through the port.
///
/// The four call sites this ratchets are the ones the audit found: recovery
/// reached past it to `provider.embed()`, fact orchestration called
/// `generate_embedding` directly, and re-embedding held the vector between the
/// two halves. The scan is a ratchet, not a proof: it cannot see a caller that
/// reaches the provider through a path this grep does not name. What it does
/// is make the next one visible.
///
/// `embedding/` itself is exempt, and that exemption is the point rather than
/// a loophole: the port's adapters live there, and the input limit, the
/// enabled check and the stage timer are all in the same module. A scan that
/// also rejected the adapters would be a scan nobody could satisfy.
#[test]
fn no_caller_outside_embedding_bypasses_the_generation_port() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = manifest.join("src");
    let mut bypasses: Vec<String> = Vec::new();

    let mut stack = vec![src];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path.clone());
                continue;
            }
            if !path.extension().is_some_and(|ext| ext == "rs") {
                continue;
            }
            let relative = path
                .strip_prefix(manifest)
                .unwrap_or(&path)
                .display()
                .to_string();
            if relative.starts_with("src/embedding/") {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            // A comment naming the call it replaced is the documentation of
            // this ratchet, not a violation of it. Skipping `//` lines is what
            // keeps the explanation next to the fix it explains.
            let code: String = text
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            for needle in [".generate_embedding(", ".embed("] {
                for (index, _) in code.match_indices(needle) {
                    let line = &code[..index].rsplit('\n').next().unwrap_or_default();
                    let trimmed = line.trim_start();
                    // A trait or inherent declaration is not a call.
                    if trimmed.starts_with("fn ") || trimmed.starts_with("async fn ") {
                        continue;
                    }
                    let line_number = code[..index].matches('\n').count() + 1;
                    bypasses.push(format!("{relative}:{line_number}: {trimmed}"));
                }
            }
        }
    }

    bypasses.sort();
    assert!(
        bypasses.is_empty(),
        "{} call site(s) generate a vector without going through the port. Each \
         one is a caller that skips the input limit, the disabled-provider check \
         or the write policy:\n\n{}",
        bypasses.len(),
        bypasses.join("\n")
    );
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
