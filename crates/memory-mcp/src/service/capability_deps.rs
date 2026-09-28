//! Container-shaped conversions for the memory capability ports.
//!
//! The `*Deps` structs live in `memory/capabilities/deps.rs` because
//! they name only the ports each capability needs. These `From<&MemoryService>`
//! impls are the other half: they read fields off the legacy container, so
//! they are adapter code and live here, on the side that may know the
//! container. This is the split the dependency guard exists to enforce.

use crate::memory::capabilities::deps::{
    ExplainDeps, ExtractDeps, IngestDeps, InvalidateDeps, ResolveDeps,
};

impl From<&crate::service::MemoryService> for IngestDeps {
    fn from(ctx: &crate::service::MemoryService) -> Self {
        Self {
            ingestion_service: ctx.ingestion_service.clone(),
        }
    }
}

impl From<&crate::service::MemoryService> for ResolveDeps {
    fn from(ctx: &crate::service::MemoryService) -> Self {
        Self {
            entity_resolver: ctx.entity_resolver.clone(),
            entity_service: ctx.entity_service.clone(),
            rate_limiter: ctx.rate_limiter.clone(),
        }
    }
}

impl From<&crate::service::MemoryService> for ExplainDeps {
    fn from(ctx: &crate::service::MemoryService) -> Self {
        Self {
            explanation_service: ctx.explanation_service.clone(),
            rate_limiter: ctx.rate_limiter.clone(),
        }
    }
}

impl From<&crate::service::MemoryService> for InvalidateDeps {
    fn from(ctx: &crate::service::MemoryService) -> Self {
        Self {
            db_client: ctx.db_client.clone(),
            active_namespace: ctx.active_namespace.clone(),
            rate_limiter: ctx.rate_limiter.clone(),
            claim_store: ctx.claim_store(),
            context_cache: ctx.context_cache.clone(),
            #[cfg(feature = "streamable-http")]
            outbox_enabled: ctx.outbox_enabled,
        }
    }
}

impl From<&crate::service::MemoryService> for ExtractDeps {
    fn from(ctx: &crate::service::MemoryService) -> Self {
        Self {
            db_client: ctx.db_client.clone(),
            active_namespace: ctx.active_namespace.clone(),
            logger: ctx.logger.clone(),
            entity_extractor: ctx.entity_extractor.clone(),
            entity_service: ctx.entity_service.clone(),
            entity_resolver: ctx.entity_resolver.clone(),
            triple_extractor: ctx.triple_extractor.clone(),
            triple_extraction_semaphore: ctx.triple_extraction_semaphore.clone(),
            embedding_service: ctx.embedding_service(),
            fact_service: ctx.fact_service.clone(),
            claim_service: ctx.claim_service.clone(),
            context_cache: ctx.context_cache.clone(),
            rate_limiter: ctx.rate_limiter.clone(),
            #[cfg(feature = "streamable-http")]
            outbox_enabled: ctx.outbox_enabled,
        }
    }
}
