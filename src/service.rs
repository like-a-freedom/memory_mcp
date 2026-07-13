//! Core business logic and service orchestration for the Memory MCP system.
//!
//! This module provides the main service layer for memory operations including:
//! - Episode ingestion and management
//! - Entity extraction and resolution
//! - Fact management with bi-temporal validity
//! - Context assembly for queries

#[cfg(feature = "mcp-apps")]
pub(crate) use apps::{apply_ingestion_review_edit, apply_ingestion_review_status};
pub use core::MemoryService;
pub use embedding::{DisabledEmbeddingProvider, EmbeddingProvider};
pub use entity_extraction::{
    AnnoEntityExtractor, EntityExtractor, GlinerEntityExtractor, LlmEntityExtractor,
    RegexEntityExtractor, create_entity_extractor,
};
pub use error::MemoryError;
pub use error::is_transient_db_error;

mod apps;
#[cfg(feature = "mcp-apps")]
pub(crate) use apps::execute_lifecycle_command;
#[cfg(feature = "mcp-apps")]
pub(crate) use apps::{AppCommand, AppCommandInput};
pub use apps::{
    ArchiveCandidatesOutcome, CommitIngestionReviewOutcome, CommitIngestionReviewRequest,
    DiffChange, DiffRequest, DiffSummary, DiffTarget, DiffView, DiffViewRange,
    GraphTraversalBudget, IngestionReviewBundle, IngestionReviewItem, IngestionReviewSource,
    IngestionReviewSummary, LifecycleCommand, LifecycleCommandOutcome, LifecycleDashboard,
    LifecycleDefaults, LifecycleView, PrepareIngestionReviewRequest, RebuildCommunitiesOutcome,
    RecomputeDecayOutcome, RestoreArchivedOutcome,
};
mod cache;
mod conflict_resolver;
mod context;
mod core;
mod embedding;
mod embedding_runtime;
mod entity;
mod entity_extraction;
mod entity_resolution;
mod episode;
mod error;
mod ingest;
pub(crate) mod ingestion;
pub(crate) mod lifecycle;
mod query;
mod reembed;
mod scope;
mod startup;
mod triple_extractor;
mod util;

mod model_loader;
pub(crate) mod value_helpers;

#[cfg(test)]
pub mod mock_db;

pub(crate) use scope::{LifecyclePolicy, MemoryScope};

pub use constants::*;
mod constants {
    /// Default context cache size.
    pub const CONTEXT_CACHE_SIZE: usize = 512;
}

/// Re-export fact decay constants for backwards compatibility.
pub use crate::models::Fact;

pub use cache::{CacheKey, invalidate_cache_by_scope};
pub(crate) use episode::build_extract_log_result;
pub use episode::{episode_from_record, fact_from_record};
#[cfg(feature = "cli-watch")]
pub use ingest::watcher::FsWatcher;
pub use lifecycle::{
    run_archival_pass, run_community_rebuild_pass, run_decay_pass, spawn_archival_worker,
    spawn_community_worker, spawn_decay_worker,
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
    deterministic_episode_id, deterministic_fact_id, hash_prefix, is_document_action_item,
    is_experience_statement, is_metric_statement, is_promise_statement,
    is_summary_like_note_candidate, validate_entity_candidate, validate_fact_input,
    validate_ingest_request,
};

pub(crate) use core::{log_args_with_duration, log_event};
pub(crate) use embedding_runtime::{
    CachedQueryEmbedding, DEFAULT_BACKGROUND_EMBEDDING_ATTEMPTS,
    DEFAULT_QUERY_EMBEDDING_CACHE_SIZE, background_embedding_retry_delay,
    is_remote_embedding_provider, is_transient_embedding_error, query_embedding_cache_ttl,
};
pub(crate) use startup::EmbeddingActivationMode;
