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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::EntityCandidate;
    use crate::service::capabilities::test_support::make_context_base;
    use crate::service::mock_db::MockDbClient;

    #[tokio::test]
    async fn resolve_delegates_to_entity_resolver() {
        let db = MockDbClient::new();
        let ctx = make_context_base(db);
        let candidate = EntityCandidate {
            entity_type: "person".to_string(),
            canonical_name: "Alice Smith".to_string(),
            aliases: vec!["Ali".to_string()],
        };
        let result = ResolveCapability::resolve(&ctx, candidate, None).await;
        assert!(result.is_ok(), "resolve must succeed with a mock db");
        let entity_id = result.unwrap();
        assert!(!entity_id.is_empty(), "entity_id must be non-empty");
    }

    #[tokio::test]
    async fn resolve_respects_rate_limit() {
        let db = MockDbClient::new();
        let ctx = make_context_base(db);
        // Exhaust the rate limiter (0 allowed calls).
        let access = AccessPayload {
            caller_id: Some("spammer".to_string()),
            ..Default::default()
        };
        // RateLimiter::new(100, 100) allows 100 calls; exhaust it.
        for _ in 0..100 {
            let _ = ctx.rate_limiter.allow("spammer");
        }
        let candidate = EntityCandidate {
            entity_type: "person".to_string(),
            canonical_name: "Bob".to_string(),
            aliases: vec![],
        };
        let result = ResolveCapability::resolve(&ctx, candidate, Some(access)).await;
        assert!(result.is_err(), "rate-limited resolve must fail");
    }
}
