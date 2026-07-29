use std::time::{Duration, Instant};

use crate::service::MemoryError;

pub(crate) const DEFAULT_BACKGROUND_EMBEDDING_ATTEMPTS: u32 = 3;
pub(crate) const DEFAULT_BACKGROUND_EMBEDDING_INITIAL_DELAY_MS: u64 = 750;
pub(crate) const DEFAULT_QUERY_EMBEDDING_CACHE_SIZE: usize = 128;
pub(crate) const DEFAULT_QUERY_EMBEDDING_CACHE_TTL_SECS: u64 = 300;

#[derive(Debug, Clone)]
pub(crate) struct CachedQueryEmbedding {
    pub(crate) embedding: Vec<f64>,
    pub(crate) expires_at: Instant,
}

#[must_use]
pub(crate) fn is_remote_embedding_provider(provider_name: &str) -> bool {
    matches!(provider_name, "openai-compatible" | "ollama")
}

#[must_use]
pub(crate) fn is_transient_embedding_error(err: &MemoryError) -> bool {
    matches!(err, MemoryError::Transient(_))
}

#[must_use]
pub(crate) fn background_embedding_retry_delay(attempt: u32) -> Duration {
    let multiplier = 1u64 << attempt.saturating_sub(1).min(6);
    Duration::from_millis(DEFAULT_BACKGROUND_EMBEDDING_INITIAL_DELAY_MS.saturating_mul(multiplier))
}

#[must_use]
pub(crate) fn query_embedding_cache_ttl() -> Duration {
    Duration::from_secs(DEFAULT_QUERY_EMBEDDING_CACHE_TTL_SECS)
}
