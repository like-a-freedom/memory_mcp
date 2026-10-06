//! The retrieval port for context assembly.
//!
//! Moved out of `service/capabilities/deps.rs` because the memory context is
//! its only consumer: every field is either a knowledge or memory store client,
//! so it is the context's own dependency set rather than a capability the
//! container hands down.

use std::sync::Arc;

use lru::LruCache;

use tokio::sync::RwLock;

use crate::error::MemoryError;
use crate::logging::StdoutLogger;
use crate::platform::rate_limiter::RateLimiter;

/// What the `assemble_context` pipeline needs.
///
/// This is the retrieval seam the pipeline already had, promoted to
/// a named owner-scoped dependency rather than a field bag on a
/// shared container. The pipeline and the capability that calls it
/// share this one type, so there is a single description of what
/// retrieval depends on.
#[derive(Clone)]
pub struct AssembleContextDeps {
    pub active_namespace: String,
    pub(crate) logger: StdoutLogger,
    pub(crate) knowledge_store: crate::knowledge::KnowledgeStoreClient,
    /// The knowledge graph store, for the app graph and lifecycle
    /// reads that sit alongside retrieval.
    pub(crate) graph_store: crate::knowledge::graph_store::KnowledgeGraphStore,
    /// The reconciliation relations of the facts being assembled, read through
    /// a knowledge-declared port so retrieval never composes claim SQL.
    pub(crate) relation_read: Arc<dyn crate::knowledge::api::RelationReadPort>,
    pub(crate) episode_store: crate::memory::EpisodeContextStore,
    /// The fact access log. Memory owns it: memory performs the
    /// retrieval that produces the heat.
    pub(crate) fact_access_store: crate::memory::fact_access_store::FactAccessStore,
    pub(crate) context_access_log: crate::storage::ContextAccessLogClient,
    /// Whether semantic retrieval is available. Most of the pipeline asks
    /// only this, so the port holds the provider contract directly instead
    /// of routing every call through the embedding service.
    pub(crate) embedding_provider: Arc<dyn crate::embedding::providers::EmbeddingProvider>,
    /// Query embedding generation, used only by semantic retrieval. The
    /// provider trait has no embed operation, so the service stays as the
    /// port for the one caller that needs one.
    pub(crate) embedding_service: crate::embedding::service::EmbeddingService,
    pub(crate) context_cache: Arc<
        RwLock<
            LruCache<
                crate::platform::context_cache_key::CacheKey,
                Vec<crate::models::AssembledContextItem>,
            >,
        >,
    >,
    pub(crate) query_logging_enabled: bool,
    pub(crate) query_log_retention_days: u32,
    pub(crate) rate_limiter: Arc<RateLimiter>,
}

impl AssembleContextDeps {
    /// The knowledge read owner: facts, entities, communities and
    /// edges.
    pub(crate) fn knowledge_store(&self) -> &crate::knowledge::KnowledgeStoreClient {
        &self.knowledge_store
    }

    /// The memory read owner for episodes.
    pub(crate) fn episode_context_store(&self) -> &crate::memory::EpisodeContextStore {
        &self.episode_store
    }

    /// The access-log owner, for query-log writes.
    pub(crate) fn context_access_log(&self) -> &crate::storage::ContextAccessLogClient {
        &self.context_access_log
    }

    /// Whether this deployment records a query log for each assembly.
    pub(crate) fn is_query_logging_enabled(&self) -> bool {
        self.query_logging_enabled
    }

    /// How long query-log rows are kept.
    pub(crate) fn query_log_retention_days(&self) -> u32 {
        self.query_log_retention_days
    }

    /// Record fact access for an assembled item.
    pub(crate) async fn record_fact_access(
        &self,
        fact_id: &str,
        boost: i64,
    ) -> Result<(), crate::error::MemoryError> {
        self.fact_access_store
            .record_fact_access(fact_id, boost)
            .await
    }
}

/// Charges the retrieval rate limit through the memory-owned
/// [`RateLimitPort`], so context assembly reaches the limiter through the
/// context's own port rather than a capability struct in `service`.
pub(crate) struct RateLimitDeps<'a> {
    pub(crate) rate_limiter: &'a Arc<RateLimiter>,
}

impl crate::memory::api::RateLimitPort for RateLimitDeps<'_> {
    fn check(&self, caller: Option<&str>) -> Result<(), MemoryError> {
        let access = caller.map(|caller_id| crate::models::AccessPayload {
            caller_id: Some(caller_id.to_owned()),
            ..Default::default()
        });
        // `check_access` is the single enforcement point, exactly as
        // the container's `enforce_rate_limit` was, so the bucket is
        // charged once and a `None` caller still costs one call.
        self.rate_limiter.check_access(access.as_ref())
    }
}
