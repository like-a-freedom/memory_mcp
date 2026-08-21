//! Core business logic and service orchestration for the Memory MCP system.
//!
//! This module provides the main service layer for memory operations including:
//! - Episode ingestion and management
//! - Entity extraction and resolution
//! - Fact management with bi-temporal validity
//! - Context assembly for queries

pub use core::MemoryService;
pub use embedding::{DisabledEmbeddingProvider, EmbeddingProvider};
#[doc(hidden)]
pub use entity_extraction::VagoLfm2EntityExtractor;
pub use entity_extraction::{
    AnnoEntityExtractor, EntityExtractor, GlinerEntityExtractor, LlmEntityExtractor, NerScheduling,
    RegexEntityExtractor, create_entity_extractor,
};
pub use error::MemoryError;
pub use error::is_transient_db_error;

pub(crate) mod apps;
#[cfg(feature = "mcp-apps")]
pub(crate) use apps::AppCommandInput;
pub use apps::{
    ArchiveCandidatesOutcome, CommitIngestionReviewOutcome, CommitIngestionReviewRequest,
    DiffChange, DiffRequest, DiffSummary, DiffTarget, DiffView, DiffViewRange,
    GraphTraversalBudget, IngestionReviewBundle, IngestionReviewItem, IngestionReviewSource,
    IngestionReviewSummary, LifecycleCommand, LifecycleCommandOutcome, LifecycleDashboard,
    LifecycleDefaults, LifecycleView, PrepareIngestionReviewRequest, RebuildCommunitiesOutcome,
    RecomputeDecayOutcome, RestoreArchivedOutcome,
};
pub mod agent_memory;
mod cache;
#[cfg(any(test, feature = "prometheus"))]
pub mod claims;
#[cfg(not(any(test, feature = "prometheus")))]
pub(crate) mod claims;
mod community;
mod conflict_resolver;
mod content_extraction;
mod context;
mod core;
pub(crate) mod durable_work;
mod embedding;
mod embedding_recovery;
mod embedding_runtime;
mod embedding_service;
mod entity;
mod entity_extraction;
mod entity_resolution;
mod episode;
mod error;
pub(crate) mod explanation;
pub(crate) mod fact;
pub(crate) mod ingestion;
pub(crate) mod lifecycle;
#[doc(hidden)]
pub mod model_artifacts;
mod model_runtime;
mod query;
mod reembed;
pub mod reembed_options;
pub mod reembed_progress;
mod startup;
mod triple_extractor;
mod util;

pub mod procedures;

pub mod capabilities;
mod model_loader;
pub mod service_context;
pub(crate) mod value_helpers;

#[cfg(test)]
pub mod mock_db;

pub(crate) use lifecycle::LifecyclePolicy;

#[cfg(test)]
pub(crate) use apps::edge_neighbor;
#[cfg(feature = "mcp-apps")]
pub(crate) use apps::graph_neighbor_expansion;
pub(crate) use apps::graph_payload;

pub use constants::*;
mod constants {
    /// Default context cache size.
    pub const CONTEXT_CACHE_SIZE: usize = 512;
    /// Maximum number of concurrent fire-and-forget triple extraction tasks.
    /// Prevents unbounded task spawning under bursty fact creation load.
    pub const TRIPLE_EXTRACTION_MAX_CONCURRENCY: usize = 4;
}

/// Re-export fact decay constants for backwards compatibility.
pub use crate::models::Fact;

pub use cache::{CacheKey, invalidate_cache};
#[cfg(feature = "cli-watch")]
pub use content_extraction::watcher::FsWatcher;
pub(crate) use episode::build_extract_log_result;
pub use episode::{episode_from_record, fact_from_record};
pub use lifecycle::{
    LifecycleBackgroundWorkerRuntime, run_archival_pass, run_community_rebuild_pass,
    run_decay_pass, spawn_archival_worker, spawn_community_worker, spawn_decay_worker,
    spawn_workers_from_config,
};
pub use query::{
    bucket_to_five_minutes, bucket_to_hour, decayed_confidence, normalize_dt, normalize_text, now,
    parse_iso, preprocess_search_query,
};
pub use reembed::ReembedSummary;
/// Re-export ids module for direct access.
pub use util::ids;
pub use util::{
    deterministic_community_id, deterministic_edge_id, deterministic_entity_id,
    deterministic_episode_id, deterministic_fact_id, hash_prefix, validate_entity_candidate,
    validate_fact_input, validate_ingest_request,
};

pub(crate) use core::{log_args_with_duration, log_event};
pub(crate) use embedding_runtime::{
    CachedQueryEmbedding, DEFAULT_BACKGROUND_EMBEDDING_ATTEMPTS,
    DEFAULT_QUERY_EMBEDDING_CACHE_SIZE, background_embedding_retry_delay,
    is_remote_embedding_provider, is_transient_embedding_error, query_embedding_cache_ttl,
};
pub(crate) use startup::EmbeddingActivationMode;
