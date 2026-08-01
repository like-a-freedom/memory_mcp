//! Database abstraction layer with SurrealDB support.
//!
//! This module provides a unified interface for database operations,
//! abstracting over embedded and remote (WS) engines.
//!
//! # Architecture
//!
//! The storage module is organized into several submodules:
//!
//! - `agent_memory`: Narrow store for lifecycle events and projection jobs
//! - `claims`: Narrow store for the claim reconciliation pipeline
//! - `client`: [`SurrealDbClient`], [`DbClient`] trait, and engine implementations
//! - `queries`: SQL query builders for all database operations
//! - `helpers`: JSON normalization, URL handling, and record extraction utilities
//! - `migrations`: Schema migration management and validation
//! - `types`: Type definitions like [`GraphDirection`]

mod agent_memory;
pub(crate) mod app_store;
pub(crate) mod claims;
mod client;
pub(crate) mod context_store;
pub(crate) mod episode_store;
pub(crate) mod fact_store;
mod helpers;
mod migrations;
mod procedures;
mod queries;
mod types;

// Re-export the public API
pub use agent_memory::{
    AgentMemoryStore, EventProjectionJobRecord, MemoryCaptureAuditRecord, MemoryEventRecord,
    disposition_str, origin_kind_str, reason_codes_str, source_kind_str, trust_class_str,
};
pub use app_store::AppStoreClient;
pub use client::{AppStore, ContextFactQuery, DbClient, SurrealDbClient};
pub use context_store::{ContextAccessLogClient, ContextStoreClient};
pub use episode_store::EpisodeStoreClient;
pub use fact_store::FactStoreClient;
pub use helpers::is_missing_index_error;
pub use procedures::ProcedureStore;
pub use queries::{
    BI_TEMPORAL_WHERE, active_edge_scan_batch_size, active_edge_scan_limit,
    fact_embedding_dimension_placeholder,
};
pub use types::GraphDirection;
