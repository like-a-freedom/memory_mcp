//! Configuration management for the Memory MCP system.

pub mod claims;
mod constants;
mod embedding;
pub mod fs_watch;
mod helpers;
mod lifecycle;
pub mod memory;
pub(crate) mod ner;
pub mod secrets;
mod surreal;
mod target;

pub use constants::*;
pub use embedding::{EmbeddingConfig, EmbeddingProviderKind, build_embedding_signature};
pub use fs_watch::{ENV_INGESTION_INBOX, FsWatchConfig};
pub use lifecycle::LifecycleConfig;
pub use memory::CacheLimits;
pub use ner::{
    DEFAULT_ANNO_MAX_INPUT_BYTES, GlinerDeviceKind, MAX_ANNO_INPUT_BYTES, ModelBackedNerConfig,
    NativeGlinerConfig, NerConfig, NerExtractorConfig, NerExtractorKind, SELECTOR_CLASSIC_GLINER,
    SELECTOR_SAUKRAUT_LFM25,
};
pub(crate) use surreal::StorageBackend;
pub use surreal::{ActiveNamespace, SurrealConfig, SurrealConfigBuilder};
pub use target::SurrealTargetConfig;

// Re-exported so a composition root outside `config` (the HTTP deployment
// policy) reads an env var through the same parser the stdio config uses,
// rather than a second hand-rolled one that could disagree about what a
// value means. Gated to the profile that is that composition root.
#[cfg(feature = "streamable-http")]
pub(crate) use helpers::{parse_bool_env, parse_env};

#[cfg(test)]
pub(crate) fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}
