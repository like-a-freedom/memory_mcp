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
//! # SaaS tenant invariant
//!
//! Memory MCP has one namespace per process in the stdio profile (ADR-0038)
//! and a bounded pool of namespaces in the HTTP SaaS profile (ADR-0052).
//! Namespace MUST never be selected through MCP arguments, URL paths, OAuth
//! claims, or API-key contents. In every profile the Tenant is derived from
//! an `AuthenticatedPrincipal` resolved by authentication, never by request.

pub mod cli;
pub mod config;
pub mod error;
pub mod logging;
pub mod mcp;
pub mod models;
pub mod observability;
pub mod runner;
pub mod service;
pub mod storage;
pub mod tools;

#[cfg(feature = "streamable-http")]
pub mod http;

#[cfg(feature = "eval-support")]
#[doc(hidden)]
pub mod eval_support;

pub use error::{MemoryError, is_transient_db_error};
pub use mcp::MemoryMcp;
pub use service::MemoryService;
pub use service::reembed_options::{ReembedOptions, ReembedOutcome};
