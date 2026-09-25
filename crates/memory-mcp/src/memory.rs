//! Memory bounded context.
//!
//! Owns episode ingestion, recall and context assembly,
//! explanation, lifecycle and procedures. Memory depends on
//! knowledge and embedding, and nothing depends on memory
//! except transport adapters.
//!
//! Memory does not read another module's canonical tables
//! directly, and it does not hold a shared service container:
//! use cases take consumer-owned ports.

pub mod api;
