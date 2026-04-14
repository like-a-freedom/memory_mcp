//! Shared configuration constants for the Memory MCP system.

/// Default vector dimension used for fact embeddings.
pub const DEFAULT_EMBEDDING_DIMENSION: usize = 1536;

/// Default vector dimension for the bundled local Candle embedding model.
pub const DEFAULT_LOCAL_CANDLE_EMBEDDING_DIMENSION: usize = 384;

/// Default max input tokens for chunking local embedding requests.
pub const DEFAULT_EMBEDDING_MAX_TOKENS: usize = 384;

/// Default timeout for embedding provider HTTP requests.
pub const DEFAULT_EMBEDDING_TIMEOUT_SECS: u64 = 15;

/// Default cosine similarity threshold for semantic retrieval.
pub const DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD: f64 = 0.7;

/// Default retention window for persisted query analytics rows.
pub const DEFAULT_QUERY_LOG_RETENTION_DAYS: u32 = 90;

/// Default confidence threshold for local NER span acceptance.
pub const DEFAULT_NER_THRESHOLD: f64 = 0.5;

/// Default batch size for local NER inference.
pub const DEFAULT_NER_BATCH_SIZE: usize = 4;

/// Default interval for confidence decay refresh (seconds).
pub(crate) const DEFAULT_DECAY_INTERVAL_SECS: u64 = 3600;

/// Default interval for episode archival refresh (seconds).
pub(crate) const DEFAULT_ARCHIVAL_INTERVAL_SECS: u64 = 86400;

/// Default confidence threshold below which facts are considered invalid.
pub(crate) const DEFAULT_DECAY_THRESHOLD: f64 = 0.3;

/// Default episode age threshold for archival (days).
pub(crate) const DEFAULT_ARCHIVAL_AGE_DAYS: u32 = 90;

/// Default half-life for confidence decay computation (days).
pub(crate) const DEFAULT_DECAY_HALF_LIFE_DAYS: f64 = 365.0;
