//! Embedding technical capability.
//!
//! Owns embeddings, model artifacts and runtime, recovery and
//! backfill, and re-embedding job state. It owns model,
//! version and dimension consistency.
//!
//! It does not own facts, claims or episodes: canonical vector
//! writes leave through the owner-approved
//! [`api::CanonicalVectorPort`].

pub mod api;
pub mod infra;
