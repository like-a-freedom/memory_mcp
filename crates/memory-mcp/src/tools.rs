//! Protocol-agnostic tool implementations shared by the MCP and CLI adapters.
//!
//! Each submodule exposes an `async fn(&MemoryService, Params) -> Result<ToolResponse<T>, MemoryError>`
//! plus the parameter, response, and parsing types it needs. Nothing in this
//! module imports from `crate::mcp` or from `clap`.

pub mod assemble_context;
pub mod explain;
pub mod extract;
pub mod ingest;
pub mod invalidate;
pub mod params;
pub mod parsers;
pub mod request_id;
pub mod resolve;
pub mod response;

pub use assemble_context::assemble_context;
pub use explain::explain;
pub use extract::extract;
pub use ingest::ingest;
pub use invalidate::invalidate;
pub use resolve::resolve;
