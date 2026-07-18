//! Configuration management for the Memory MCP system.

pub(crate) mod claims;
mod constants;
mod embedding;
mod helpers;
mod lifecycle;
mod ner;
mod surreal;

pub use constants::*;
pub use embedding::{EmbeddingConfig, EmbeddingProviderKind, build_embedding_signature};
pub use lifecycle::LifecycleConfig;
pub use ner::{NerConfig, NerDeviceKind, NerProviderKind};
pub use surreal::{SurrealConfig, SurrealConfigBuilder};
