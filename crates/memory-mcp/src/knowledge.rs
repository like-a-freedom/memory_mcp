//! Knowledge bounded context.
//!
//! Owns entities, aliases, facts, claims, triples,
//! communities, extraction and reconciliation, and knowledge
//! queries. It distinguishes contradiction, correction,
//! supersession and retraction, and keeps the single
//! bi-temporal close/write implementation.
//!
//! Knowledge does not own episode lifecycle or model runtime
//! management. Reads leave through owner-named scopes rather
//! than caller-supplied tables.

pub mod api;
pub mod claims;
pub mod claims_policy;
mod close_store;
pub(crate) mod community;
pub(crate) mod conflict_resolver;
pub mod diff;
pub mod diff_types;
pub mod entity_extraction;
pub mod entity_resolution;
pub mod entity_service;
mod entity_store;
pub mod fact_parsing;
pub mod fact_service;
mod fact_store;
pub mod graph_store;
pub mod infra;
mod knowledge_store;
mod triple_store;

pub(crate) use close_store::{CloseStoreClient, CloseTimestamps};
pub(crate) use community::CommunityRecord;
pub(crate) use entity_store::EntityStoreClient;
pub use fact_store::FactStoreClient;
pub use graph_store::KnowledgeGraphStore;
pub use knowledge_store::KnowledgeStoreClient;
pub(crate) use triple_store::TripleStoreClient;
