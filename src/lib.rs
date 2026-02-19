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

pub mod config;
pub mod logging;
pub mod mcp;
pub mod models;
pub mod service;
pub mod storage;

pub use mcp::MemoryMcp;
pub use service::MemoryService;
