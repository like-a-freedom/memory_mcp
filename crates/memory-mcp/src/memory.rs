//! Memory bounded context.
//!
//! Owns episode ingestion, recall and context assembly,
//! explanation, lifecycle and procedures. Memory depends on
//! knowledge and embedding, and nothing depends on memory
//! except transport adapters.
//!
//! Memory does not read another module's canonical tables
//! directly, and it does not hold a shared service container:
//! use cases take consumer-owned ports.

pub mod agent_memory;
pub mod api;
pub mod capabilities;
pub mod community_summary;
pub mod content_extraction;
pub mod context_cache;
pub mod episode;
pub mod episode_context_store;
pub mod episode_store;
pub mod explanation;
pub mod fact_access_store;
pub mod ingestion;
pub mod ingestion_review;
pub mod lifecycle;
pub mod lifecycle_types;
pub mod lifecycle_workers;
pub mod record_parsing;
pub mod retrieval;
pub mod retrieval_deps;
pub use lifecycle_workers::LifecycleBackgroundWorkerRuntime;
pub mod inbox_revision_store;
pub mod procedure_store;
pub mod procedures_service;

pub use inbox_revision_store::InboxRevisionStoreClient;

pub use episode_context_store::EpisodeContextStore;
