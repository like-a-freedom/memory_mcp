//! The technical platform for persistence: engine, client, queries and
//! schema.
//!
//! Nothing here owns domain data. Every canonical table's store lives
//! with the bounded context that owns that table — knowledge, memory,
//! embedding — and reaches the engine through the [`DbClient`] port
//! below. What remains is what no context can own because it is not
//! domain knowledge: the client, the query builders, the migration
//! runner, the row-unwrapping helpers, and the platform's own event and
//! access logs.
//!
//! # Architecture
//!
//! - `client`: [`SurrealDbClient`], [`DbClient`] trait, and engine implementations
//! - `queries`: SQL query builders for all database operations
//! - `helpers`: JSON normalization, URL handling, and record extraction utilities
//! - `migrations`: Schema migration management and validation
//! - `value_helpers`: row unwrapping for the Surreal value shapes
//! - `types`: Type definitions like [`GraphDirection`]
//! - `access_log_store` and `event_log_store`: the platform's own
//!   logs, with no domain owner. They stay here by design.

pub mod access_log_store;
pub(crate) mod client;
pub mod event_log_store;
mod helpers;
pub(crate) mod migrations;
mod queries;
mod types;
pub(crate) mod value_helpers;

// Re-export the public API
pub use crate::memory::agent_memory::{
    AgentMemoryStore, EventProjectionJobRecord, MemoryCaptureAuditRecord, MemoryEventRecord,
    disposition_str, origin_kind_str, reason_codes_str, source_kind_str, trust_class_str,
};

/// Compatibility re-export: the inbox revision store moved to the
/// memory context, which owns the inbox revision table. One-way, with
/// no internal consumers; retired with the compatibility surface.
pub use crate::memory::inbox_revision_store;
pub use crate::memory::inbox_revision_store::InboxRevisionStoreClient;
pub use access_log_store::ContextAccessLogClient;
#[cfg(any(test, feature = "test-fixtures"))]
pub use client::BoundDbClient;
#[cfg(not(any(test, feature = "test-fixtures")))]
pub(crate) use client::BoundDbClient;
pub(crate) use client::is_record_already_exists_error;
pub use client::{ContextFactQuery, DbClient, SurrealDbClient};
pub use event_log_store::EventLogStoreClient;
pub use helpers::{
    RecordLookup, is_missing_index_error, owner_scoped_read, record_id_from_json_value,
    require_record_kind,
};
pub use queries::{
    BI_TEMPORAL_WHERE, active_edge_scan_batch_size, active_edge_scan_limit,
    fact_embedding_dimension_placeholder,
};
// `build_create_query` is called from `knowledge/fact_store.rs` under
// `streamable-http`; without that feature the re-export has no user,
// which the compiler reports. Gated to match its one caller.
#[cfg(feature = "streamable-http")]
pub(crate) use queries::build_create_query;
pub(crate) use queries::{
    build_fact_visibility_clause, build_relate_edge_query, build_select_active_facts_query,
    build_select_communities_by_member_entities_query, build_select_edge_neighbors_query,
    build_select_edges_filtered_page_query, build_select_episodes_by_content_query,
    build_select_facts_ann_query, build_select_facts_by_entity_links_query,
    build_select_facts_filtered_query, build_upsert_query, surreal_string_literal,
    validate_record_id,
};
pub use types::GraphDirection;

/// Compatibility aliases for external Rust callers (integration tests,
/// the eval harness). These types moved to the context that owns their
/// canonical table. They are one-way delegating re-exports with no
/// internal consumers; they are retired with the compatibility surface,
/// not silently removed here.
pub use crate::knowledge::FactStoreClient;
pub use crate::memory::episode_store::EpisodeStoreClient;
pub use crate::memory::fact_access_store::FactAccessStore;
pub use crate::memory::procedure_store::ProcedureStore;
