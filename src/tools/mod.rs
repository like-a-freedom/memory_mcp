//! Protocol-agnostic tool implementations shared by the MCP and CLI adapters.
//!
//! Each submodule exposes an `async fn(&MemoryService, Params) -> Result<ToolResponse<T>, MemoryError>`
//! plus the parameter, response, and parsing types it needs. Nothing in this
//! module imports from `crate::mcp` or from `clap`.

pub mod extract;
pub mod ingest;
pub mod params;
pub mod parsers;
pub mod request_id;
pub mod response;

pub use extract::extract;
pub use ingest::ingest;
