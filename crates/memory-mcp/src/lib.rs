//! Memory MCP - A Rust implementation of the Memory Model Context Protocol server.
//!
//! This crate provides a long-term memory system for AI agents, featuring:
//! - Episode storage and retrieval
//! - Entity extraction and deduplication
//! - Fact management with bi-temporal validity
//! - Context assembly for queries
//! - Integration with SurrealDB (embedded or remote)
//!
//! # Architecture
//!
//! The crate is organized into several modules:
//!
//! - `mcp`: MCP protocol handlers and tool implementations
//! - `service`: Core business logic and orchestration
//! - `storage`: Database abstraction layer with SurrealDB support
//! - `models`: Data structures and types
//! - `config`: Configuration management
//! - `logging`: Structured logging utilities
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use memory_mcp::MemoryService;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let service = MemoryService::new_from_env().await?;
//!     // Use the service...
//!     Ok(())
//! }
//! ```
//!

pub mod cli;
pub mod config;
/// The pure error vocabulary. Re-exported here so the many existing
/// `crate::error::MemoryError` paths stay valid; the definition
/// lives in the shared kernel.
pub use shared::error;
pub mod logging;
pub mod mcp;
pub mod models;
pub mod observability;
pub mod platform;
pub mod runner;
pub mod shared;

/// # SaaS tenant invariant
///
/// Memory MCP has one namespace per process in the stdio profile
/// and a bounded pool of namespaces in the HTTP SaaS profile.
/// Namespace MUST never be selected through MCP arguments, URL paths, OAuth
/// claims, or API-key contents. In every profile the Tenant is derived from
/// an `AuthenticatedPrincipal` resolved by authentication, never by request.
pub mod service;
pub mod storage;
pub mod tools;

/// Embedding technical capability. Available in every profile
/// because the local stdio server also embeds facts; it owns
/// model/version/dimension consistency, never canonical records.
pub mod embedding;

/// Knowledge bounded context. Owns entities, aliases, facts,
/// claims, triples, communities and knowledge queries,
/// including the single bi-temporal close implementation.
pub mod knowledge;

/// Memory bounded context. Owns episode ingestion, recall and
/// assembly, explanation, lifecycle and procedures. Capabilities
/// depend on consumer-owned ports rather than a shared
/// service container.
pub mod memory;

#[cfg(feature = "streamable-http")]
pub mod http;
#[cfg(feature = "ui")]
pub mod ui;

#[cfg(feature = "streamable-http")]
pub mod bootstrap;
#[cfg(feature = "streamable-http")]
pub mod tenancy;

#[cfg(feature = "control-plane")]
pub mod identity;
#[cfg(feature = "control-plane")]
pub mod operations;
#[cfg(feature = "streamable-http")]
pub mod provisioning;

#[cfg(feature = "control-plane")]
pub mod control;

#[cfg(feature = "eval-support")]
#[doc(hidden)]
pub mod eval_support;

pub use error::MemoryError;
pub use mcp::MemoryMcp;
pub use platform::persistence::db_errors::is_transient_db_error;
pub use service::MemoryService;
pub use service::reembed_options::{ReembedOptions, ReembedOutcome};
