//! Configuration management for the Memory MCP system.

pub(crate) mod claims;
mod constants;
mod embedding;
pub mod fs_watch;
mod helpers;
mod lifecycle;
pub(crate) mod ner;
mod surreal;
mod target;

pub use constants::*;
pub use embedding::{EmbeddingConfig, EmbeddingProviderKind, build_embedding_signature};
pub use fs_watch::{ENV_INGESTION_INBOX, FsWatchConfig};
pub use lifecycle::LifecycleConfig;
pub use ner::{
    GlinerDeviceKind, ModelBackedNerConfig, NativeGlinerConfig, NerConfig, NerExtractorConfig,
    NerExtractorKind, SELECTOR_CLASSIC_GLINER, SELECTOR_SAUKRAUT_LFM25,
};
pub(crate) use surreal::StorageBackend;
pub use surreal::{ActiveNamespace, SurrealConfig, SurrealConfigBuilder};
pub use target::SurrealTargetConfig;

#[cfg(test)]
pub(crate) fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}
