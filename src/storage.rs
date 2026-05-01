//! Database abstraction layer with SurrealDB support.
//!
//! This module provides a unified interface for database operations,
//! abstracting over embedded and remote (WS) engines.
//!
//! # Architecture
//!
//! The storage module is organized into several submodules:
//!
//! - `client`: [`SurrealDbClient`], [`DbClient`] trait, and engine implementations
//! - `queries`: SQL query builders for all database operations
//! - `helpers`: JSON normalization, URL handling, and record extraction utilities
//! - `migrations`: Schema migration management and validation
//! - `types`: Type definitions like [`GraphDirection`]

mod client;
mod helpers;
mod migrations;
mod queries;
mod types;

// Re-export the public API
pub use client::{DbClient, SurrealDbClient};
pub use helpers::is_missing_index_error;
pub use queries::{
    BI_TEMPORAL_WHERE, active_edge_scan_limit, fact_embedding_dimension_placeholder,
};
pub use types::GraphDirection;
