//! MCP protocol handlers and tool implementations.
//!
//! This module provides the Model Context Protocol (MCP) server implementation,
//! exposing memory operations as tools to AI agents.
//!
//! # Architecture
//!
//! The MCP module is organized into several submodules:
//!
//! - `params`: Parameter structures for tool calls
//! - `parsers`: Utility functions for parsing and validation
//! - `handlers`: Individual tool handler implementations
//! - `error`: Error conversion utilities

pub use handlers::*;
pub use parsers::*;

mod error;
mod handlers;
mod params;
mod parsers;

pub use error::mcp_error;
pub use params::*;
pub use parsers::{content_hash, default_scope, parse_context_items, parse_datetime};
