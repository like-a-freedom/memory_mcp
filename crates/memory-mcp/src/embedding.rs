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
pub(crate) mod backfill_store;
pub mod infra;
pub mod model_artifacts;
pub(crate) mod model_loader;
pub mod providers;
pub mod queries;
pub(crate) mod query_cache;
pub(crate) mod reembed_store;
pub mod runtime;
pub mod service;
pub(crate) mod state_store;
