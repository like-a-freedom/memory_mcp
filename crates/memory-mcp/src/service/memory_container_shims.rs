//! Container-shaped entry points for the memory context.
//!
//! Each submodule holds the `From<&MemoryService>` impls and the
//! `fn(&MemoryService, ..)` wrappers for one memory module. They are
//! adapter code: the memory context takes narrow injected ports, and
//! only this side may read the legacy container.

pub mod memory_capabilities_assemble_context;
pub mod memory_capabilities_explain;
pub mod memory_capabilities_extract;
pub mod memory_capabilities_ingest;
pub mod memory_capabilities_invalidate;
pub mod memory_capabilities_resolve;
pub mod memory_ingestion_review;
pub mod memory_lifecycle;
pub mod tool_context_impl;
