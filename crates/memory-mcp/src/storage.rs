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
pub(crate) mod helpers;
pub(crate) mod migrations;
pub(crate) mod queries;
pub mod table_scope;
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
// `BoundDbClient` appears in `operations::api::RetainedTenantWork`, so an
// external implementor of that trait must be able to name the type. It is the
// namespace-pinned adapter: its methods take no namespace, which is exactly
// the property the operations workflow is relying on when it hands one to the
// sweep.
pub use client::BoundDbClient;
pub(crate) use client::is_record_already_exists_error;
pub use client::{ContextFactQuery, DbClient, SurrealDbClient};
pub use event_log_store::EventLogStoreClient;
pub use helpers::{
    RecordLookup, is_missing_index_error, owner_scoped_read, record_id_from_json_value,
    require_record_kind,
};
// What stays re-exported from `queries` is what is genuinely table-generic.
// The domain builders moved to `knowledge::queries` and `memory::queries`, and
// `BI_TEMPORAL_WHERE` to `shared::temporal`, where the three modules that
// filter on it can import it without going through the platform.
pub use queries::{active_edge_scan_batch_size, fact_embedding_dimension_placeholder};
pub(crate) use queries::{build_upsert_query, validate_record_id};

// `build_create_query` is called from `knowledge/fact_store.rs` under
// `streamable-http`; without that feature the re-export has no user,
// which the compiler reports. Gated to match its one caller.
#[cfg(feature = "streamable-http")]
pub(crate) use queries::build_create_query;

pub use table_scope::{
    EmbeddingTables, KnowledgeTables, MemoryTables, OwnedTable, PlatformTables, ReleaseOwnedTable,
    TableOwner, table_owners,
};

/// Every table the migration sequence creates.
///
/// Re-exported so a test can require the selectable set to equal it. The list
/// being private is why the allowlist this replaced could name ten of these
/// twenty-three without anything noticing.
pub fn expected_schema_tables() -> &'static [&'static str] {
    migrations::EXPECTED_SCHEMA_TABLES
}

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
