//! Core business logic and service orchestration for the Memory MCP system.
//!
//! This module provides the main service layer for memory operations including:
//! - Episode ingestion and management
//! - Entity extraction and resolution
//! - Fact management with bi-temporal validity
//! - Context assembly for queries
//!
//! ```compile_fail
//! use memory_mcp::service::NullEmbedder;
//!
//! let _ = NullEmbedder;
//! ```
//!
//! ```compile_fail
//! use memory_mcp::service::EmbeddingProvider;
//! ```

pub use core::MemoryService;
pub use entity_extraction::{EntityExtractor, RegexEntityExtractor};
pub use error::MemoryError;

mod cache;
mod context;
mod core;
mod entity_extraction;
mod episode;
mod error;
mod ids;
mod query;
mod validation;

pub use constants::*;
mod constants {
    /// Half-life in days for metric and promise fact confidence decay.
    pub const METRIC_HALF_LIFE_DAYS: f64 = 365.0;

    /// Half-life in days for general fact confidence decay.
    pub const DEFAULT_HALF_LIFE_DAYS: f64 = 180.0;

    /// Scaling factor for confidence rounding.
    pub const CONFIDENCE_SCALE: f64 = 10000.0;

    /// Default context cache size.
    pub const CONTEXT_CACHE_SIZE: usize = 512;
}

pub use cache::{CacheKey, SafeMutex, invalidate_cache_by_scope};
pub use episode::{episode_from_record, extract_from_episode, fact_from_record};
pub use ids::{
    deterministic_community_id, deterministic_edge_id, deterministic_entity_id,
    deterministic_episode_id, deterministic_fact_id, hash_prefix,
};
pub use query::{
    bucket_to_hour, decayed_confidence, normalize_dt, normalize_text, now, parse_iso,
    preprocess_search_query,
};
pub use validation::{validate_entity_candidate, validate_fact_input, validate_ingest_request};

pub(crate) use core::log_event;
