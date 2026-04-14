//! Configuration management for the Memory MCP system.

mod constants;
mod embedding;
mod helpers;
mod lifecycle;
mod ner;
mod surreal;

pub use constants::*;
pub use embedding::{EmbeddingConfig, EmbeddingProviderKind};
pub use lifecycle::LifecycleConfig;
pub use ner::{NerConfig, NerProviderKind};
pub use surreal::{SurrealConfig, SurrealConfigBuilder};
