//! Container conversion for the memory context's retrieval port.
//!
//! `AssembleContextDeps` itself lives in the memory context; the conversion
//! from `MemoryService` lives here, on the adapter side of the dependency
//! direction, so the context never has to name the container.

use crate::memory::retrieval_deps::AssembleContextDeps;
use crate::service::MemoryService;

impl From<&MemoryService> for AssembleContextDeps {
    fn from(ctx: &crate::service::MemoryService) -> Self {
        Self {
            active_namespace: ctx.active_namespace.clone(),
            logger: ctx.logger.clone(),
            knowledge_store: crate::knowledge::KnowledgeStoreClient::new(
                ctx.db_client.clone(),
                ctx.active_namespace.clone(),
            ),
            graph_store: crate::knowledge::graph_store::KnowledgeGraphStore::new(
                ctx.db_client.clone(),
                ctx.active_namespace.clone(),
            ),
            fact_access_store: crate::memory::fact_access_store::FactAccessStore::new(
                ctx.db_client.clone(),
                ctx.active_namespace.clone(),
            ),
            episode_store: crate::memory::EpisodeContextStore::new(
                ctx.db_client.clone(),
                ctx.active_namespace.clone(),
            ),
            context_access_log: crate::storage::ContextAccessLogClient::new(
                ctx.db_client.clone(),
                ctx.active_namespace.clone(),
            ),
            embedding_provider: ctx.embedding_service().embedding_provider().clone(),
            embedding_service: ctx.embedding_service(),
            context_cache: ctx.context_cache.clone(),
            query_logging_enabled: ctx.query_logging_enabled,
            query_log_retention_days: ctx.query_log_retention_days,
            rate_limiter: ctx.rate_limiter.clone(),
        }
    }
}
