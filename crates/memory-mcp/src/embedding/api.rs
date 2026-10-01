//! Embedding technical capability — public interface.
//!
//! The embedding capability owns model/version/dimension
//! consistency and job state. It does not own facts, claims
//! or episodes.
//!
//! Canonical vector updates flow one way: a caller that
//! already holds a generated vector presents it to
//! [`update_canonical_vector`], which validates it against the
//! resolved target identity and applies it through a narrow
//! owner-approved [`CanonicalVectorPort`]. The vector
//! endpoint never calls embedding generation again, so the
//! job -> port direction cannot form a cycle.

use chrono::{DateTime, Utc};

use crate::error::MemoryError;

/// Resolved identity of the embedding target that produced a
/// vector. The signature folds provider, model and dimension
/// so a stale vector is detectable by comparison alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VectorIdentity {
    pub provider: String,
    pub model: Option<String>,
    pub dimension: usize,
    pub signature: String,
}

/// A vector that passed the target-dimension check and is
/// ready to be applied to a canonical record.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedVector {
    pub vector: Vec<f64>,
    pub identity: VectorIdentity,
    pub at: DateTime<Utc>,
}

/// Why a generation produced no vector.
///
/// A bounded enum, not a string: the reason ends up as a metric label, and an
/// unbounded one would let a caller invent a new time series. The set is
/// closed on purpose — a reason nobody can enumerate is a reason nobody will
/// ever add an alert for.
///
/// There is one variant and not three. "The record is already current" and "a
/// backfill pass is not allowed to replace this vector" are both
/// [`VectorApplication::AlreadyCurrent`], and splitting them here would mean
/// two outcomes for one fact, which is what this module exists to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The provider is configured but disabled, so no vector could exist.
    ProviderDisabled,
}

/// What generation produced, or why it produced nothing.
#[derive(Debug, Clone, PartialEq)]
pub enum GenerationOutcome {
    Generated(Vec<f64>),
    Skipped(SkipReason),
}

/// Generation, behind a port the capability itself declares.
///
/// `EmbeddingService` is the production adapter; a test's recording generator
/// is the second. The trait exists so `generate_and_update` can be observed
/// from outside the crate — `EmbeddingService::generate_embedding` is
/// `pub(crate)`, and a seam that cannot be reached from a test is not a seam.
///
/// Implementations carry the input limit and the disabled check, so a caller
/// cannot skip either by holding a provider directly. That was the whole
/// defect: `service/embedding_recovery.rs` called `provider.embed()` and
/// therefore skipped truncation, the enabled check, and every log line.
#[async_trait::async_trait]
pub trait EmbeddingGeneration: Send + Sync {
    /// Generate a vector for `input`, or say why none was produced.
    async fn generate(&self, input: &str) -> Result<GenerationOutcome, MemoryError>;
}

/// Outcome of an attempted canonical vector update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorApplication {
    /// The owner record was updated.
    Applied,
    /// The record already carried the target signature, so
    /// no write was issued.
    AlreadyCurrent,
    /// Nothing was generated, or the policy refused the write. The
    /// reason travels with the outcome so a caller can label a
    /// metric without string-matching an error.
    Skipped(SkipReason),
}

/// What the owner record currently carries for its vector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredVector {
    /// No vector has ever been written.
    Absent,
    /// A vector exists, produced under this signature.
    Present { signature: String },
}

/// Which workflow is authorising a write.
///
/// Recovery and re-embedding have deliberately different
/// policies, so the rule is named rather than implied by
/// whichever caller happens to run first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorWritePolicy {
    /// Backfill: fill gaps only. A fact that already carries
    /// any vector is left untouched, even when its signature is
    /// stale, because recovery must not discard a usable
    /// vector.
    FillMissing,
    /// Re-embedding: rewrite when the stored signature differs
    /// from the target, and skip when it already matches.
    ReplaceStale,
}

/// Narrow, owner-approved persistence contract for canonical
/// vectors.
///
/// Implementations live in the owning module's infra and are
/// wired by bootstrap. The port exposes only the operations
/// the embedding capability needs: read the currently stored
/// vector state and apply a vector to a fact. It is deliberately
/// not a general record updater.
#[async_trait::async_trait]
pub trait CanonicalVectorPort: Send + Sync {
    /// Vector state currently stored on the fact.
    async fn stored_fact_vector(&self, fact_id: &str) -> Result<StoredVector, MemoryError>;

    /// Apply a validated vector to the fact record.
    ///
    /// `policy` is carried through so the owner can keep a
    /// conditional write atomic: a gap fill must still be a
    /// compare-and-set in storage, not a read-then-write that
    /// a concurrent pass could interleave with.
    async fn apply_fact_vector(
        &self,
        fact_id: &str,
        vector: Vec<f64>,
        identity: VectorIdentity,
        at: DateTime<Utc>,
        policy: VectorWritePolicy,
    ) -> Result<(), MemoryError>;
}

/// Validate a generated vector against the target identity.
///
/// This is a pure function: it never generates a vector and
/// never touches storage, so the embedding job cannot recurse
/// back into generation through this path.
pub fn prepare_canonical_vector(
    vector: Vec<f64>,
    identity: &VectorIdentity,
    at: DateTime<Utc>,
) -> Result<PreparedVector, MemoryError> {
    if vector.is_empty() {
        return Err(MemoryError::Validation("embedding vector is empty".into()));
    }
    if vector.len() != identity.dimension {
        return Err(MemoryError::Validation(format!(
            "embedding dimension mismatch: provider returned {}, expected {}",
            vector.len(),
            identity.dimension
        )));
    }
    Ok(PreparedVector {
        vector,
        identity: identity.clone(),
        at,
    })
}

/// Apply a generated vector to a canonical fact through the
/// owner-approved port.
///
/// [`VectorWritePolicy`] decides what happens when the record
/// already carries a vector: backfill only fills gaps, while
/// re-embedding rewrites a stale signature. Either way a record
/// that is already current is left untouched, so a repeated
/// job pass is a no-op rather than a redundant write.
pub async fn update_canonical_vector(
    port: &(impl CanonicalVectorPort + ?Sized),
    fact_id: &str,
    vector: Vec<f64>,
    identity: &VectorIdentity,
    at: DateTime<Utc>,
    policy: VectorWritePolicy,
) -> Result<VectorApplication, MemoryError> {
    let prepared = prepare_canonical_vector(vector, identity, at)?;
    match port.stored_fact_vector(fact_id).await? {
        StoredVector::Present { signature } => {
            let is_current = signature == prepared.identity.signature;
            if is_current {
                return Ok(VectorApplication::AlreadyCurrent);
            }
            if policy == VectorWritePolicy::FillMissing {
                return Ok(VectorApplication::AlreadyCurrent);
            }
        }
        StoredVector::Absent => {}
    }
    port.apply_fact_vector(
        fact_id,
        prepared.vector,
        prepared.identity,
        prepared.at,
        policy,
    )
    .await?;
    Ok(VectorApplication::Applied)
}

/// Whether a backfill may keep going after one outcome.
///
/// This is a policy question, so it is answered here rather than at the call
/// site. `service/embedding_recovery.rs` holds the loop; what a loop does with
/// an outcome is the embedding context's decision, and the loop is a transport
/// adapter that should not be making it.
///
/// The distinction matters for one case only. `Applied` and `AlreadyCurrent`
/// both mean the batch advanced. `Skipped` means it did not — and a skip
/// produced by a disabled provider will be produced again for every remaining
/// fact, so continuing would walk the whole table to learn nothing on each
/// row. Stopping is the honest response, and it is also the one a caller can
/// act on: the caller gets to decide whether to retry later or to record that
/// semantic retrieval is degraded, and it can only do that if the loop stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchAdvance {
    /// The row was written, or was already current. Keep going.
    Continue,
    /// Nothing was produced. Stop, and tell the caller why.
    Halt(SkipReason),
}

/// Whether an outcome advances a backfill loop.
///
/// A pure function over the outcome, so the decision can be tested without an
/// embedded database and without a provider.
pub fn batch_advance(application: VectorApplication) -> BatchAdvance {
    match application {
        VectorApplication::Applied | VectorApplication::AlreadyCurrent => BatchAdvance::Continue,
        VectorApplication::Skipped(reason) => BatchAdvance::Halt(reason),
    }
}

/// Generate a vector for `fact_id` and write it under `policy`.
///
/// Every vector the system writes goes through this one function, so the
/// input limit, the disabled-provider check and the generation logging inside
/// an [`EmbeddingGeneration`] adapter cannot be skipped by a caller that
/// already holds a provider. That is the defect this closes:
/// `service/embedding_recovery.rs` called `provider.embed()` directly and so
/// bypassed all three, and `service/fact_orchestration.rs` inlined its own
/// payload build and never consulted the write policy.
///
/// A generation that yields nothing is a [`VectorApplication::Skipped`] with
/// its reason attached, not an error: a server started without an embedding
/// provider is a supported configuration, and it must ingest facts rather
/// than refuse to.
pub async fn generate_and_update(
    generation: &(impl EmbeddingGeneration + ?Sized),
    port: &(impl CanonicalVectorPort + ?Sized),
    fact_id: &str,
    input: &str,
    identity: &VectorIdentity,
    policy: VectorWritePolicy,
) -> Result<VectorApplication, MemoryError> {
    let vector = match generation.generate(input).await? {
        GenerationOutcome::Generated(vector) => vector,
        GenerationOutcome::Skipped(reason) => return Ok(VectorApplication::Skipped(reason)),
    };
    update_canonical_vector(port, fact_id, vector, identity, Utc::now(), policy).await
}
