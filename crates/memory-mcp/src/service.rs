//! Core business logic and service orchestration for the Memory MCP system.
//!
//! This module provides the main service layer for memory operations including:
//! - Episode ingestion and management
//! - Entity extraction and resolution
//! - Fact management with bi-temporal validity
//! - Context assembly for queries

pub use crate::embedding::providers::{DisabledEmbeddingProvider, EmbeddingProvider};
pub use crate::knowledge::entity_extraction::NerBuildContext;
#[doc(hidden)]
pub use crate::knowledge::entity_extraction::VagoLfm2EntityExtractor;
pub use crate::knowledge::entity_extraction::{
    AnnoEntityExtractor, EntityExtractor, GlinerEntityExtractor, LlmEntityExtractor, NerScheduling,
    RegexEntityExtractor, create_entity_extractor,
};
pub use core::MemoryService;
pub mod entity_extraction_gliner {
    //! Public re-exports of the Classic GLiNER backend helpers used by
    //! real-fixture integration tests. Production callers should construct
    //! the extractor through the normal backend registry, not these items.
    pub use crate::knowledge::entity_extraction::gliner;
    pub use gliner::{CLASSIC_GLINER_SPEC, build_from_store};
}
// Re-exported from the neutral `crate::error` home (ADR-0045).
pub use crate::error::MemoryError;

pub mod agent_memory;
pub mod apps;
pub mod capability_deps;
pub mod cli;
pub mod memory_container_shims;
/// Compatibility re-exports for the embedding-owned modules that moved
/// out of `service/`. One-way, with no internal consumers; retired with
/// the compatibility surface.
pub use crate::embedding::model_artifacts;
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

/// Compatibility re-export: the procedural-memory use cases moved to
/// the memory context, which owns the procedure table. One-way, with no
/// internal consumers.
pub use crate::memory::procedures_service as procedures;

/// Compatibility alias for the claim use cases, which moved to the
/// knowledge context that owns the claim domain. This is a one-way
/// re-export with no internal consumers; it is retired with the
/// compatibility surface, not silently removed here.
#[cfg(any(test, feature = "prometheus"))]
pub mod claims {
    pub use crate::knowledge::claims_policy::telemetry;
}
mod core;
mod embedding_recovery;
pub(crate) mod fact_orchestration;
#[cfg(feature = "fs-watch")]
pub mod fs_watch;
pub(crate) mod model_artifact_refresh;
#[doc(hidden)]
mod query;
mod reembed;
pub mod reembed_options;
pub mod reembed_progress;
pub mod retrieval_deps_from_container;
mod startup;

#[cfg(feature = "control-plane")]
pub mod credential_material;

#[cfg(test)]
pub mod mock_db;

pub(crate) use crate::memory::lifecycle_workers::LifecyclePolicy;
pub(crate) use apps::LifecycleOperation;

#[cfg(test)]
pub(crate) use apps::edge_neighbor;
#[cfg(feature = "mcp-apps")]
pub(crate) use apps::graph_neighbor_expansion;
pub(crate) use apps::graph_payload;

pub use constants::*;
#[cfg(feature = "control-plane")]
pub mod local_admin;

mod constants {
    /// Default context cache size.
    pub const CONTEXT_CACHE_SIZE: usize = 512;
    /// Maximum number of concurrent fire-and-forget triple extraction tasks.
    /// Prevents unbounded task spawning under bursty fact creation load.
    pub const TRIPLE_EXTRACTION_MAX_CONCURRENCY: usize = 4;
}

/// Re-export fact decay constants for backwards compatibility.
pub use crate::models::Fact;

pub(crate) use crate::memory::episode::build_extract_log_result;
pub use crate::memory::episode::{episode_from_record, fact_from_record};
/// Re-export the deterministic-id module for direct access.
pub use crate::shared::ids;
// The decay and archival passes themselves take `LifecycleHandles`, a
// crate-private port, so they cannot leave the crate. The public
// `decay_pass` and `archival_pass` wrappers are the service-shaped
// entry points; `spawn_workers_from_config` starts the background
// workers.
pub use crate::shared::ids::{
    deterministic_community_id, deterministic_edge_id, deterministic_entity_id,
    deterministic_episode_id, deterministic_episode_id_v2, deterministic_fact_id, hash_prefix,
};
pub use crate::shared::validation::{
    validate_entity_candidate, validate_fact_input, validate_ingest_request,
};
pub use query::{
    bucket_to_five_minutes, bucket_to_hour, decayed_confidence, normalize_dt, normalize_text, now,
    parse_iso, preprocess_search_query,
};
pub use reembed::ReembedSummary;

pub(crate) use crate::embedding::runtime::{
    CachedQueryEmbedding, DEFAULT_QUERY_EMBEDDING_CACHE_SIZE, is_remote_embedding_provider,
};
pub(crate) use startup::EmbeddingActivationMode;
