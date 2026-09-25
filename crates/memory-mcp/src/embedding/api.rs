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

/// Outcome of an attempted canonical vector update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorApplication {
    /// The owner record was updated.
    Applied,
    /// The record already carried the target signature, so
    /// no write was issued.
    AlreadyCurrent,
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
