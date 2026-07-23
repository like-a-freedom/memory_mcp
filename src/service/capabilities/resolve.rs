//! Capability for resolving entity candidates to canonical IDs.

use crate::models::{AccessPayload, EntityCandidate};
use crate::service::error::MemoryError;
use crate::service::service_context::ServiceContext;

/// Capability for fuzzy entity resolution and deduplication.
pub struct ResolveCapability;

impl ResolveCapability {
    /// Resolves an entity candidate, returning the canonical entity ID.
    ///
    /// Uses fuzzy matching via `EntityResolver` to deduplicate entities
    /// with similar names (e.g., "Иван Петров" vs "I. Petrov").
    pub async fn resolve(
        ctx: &ServiceContext,
        candidate: EntityCandidate,
        access: Option<AccessPayload>,
    ) -> Result<String, MemoryError> {
        ctx.enforce_rate_limit(access.as_ref())?;
        let namespace = ctx.default_namespace.clone();
        let (entity_id, _was_created) = ctx
            .entity_resolver
            .resolve_or_create(&ctx.entity_service, candidate, &namespace)
            .await?;
        Ok(entity_id)
    }
}
