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
//! - `resources`: MCP resource catalog and URI helpers for app sessions

pub use handlers::*;
pub use params::*;
pub use response::{AppCommandResult, OpenAppResult};

mod error;
mod handlers;
mod params;
mod parsers;
mod resources;
pub(crate) mod response;
pub(crate) mod session;

pub use error::mcp_error;
