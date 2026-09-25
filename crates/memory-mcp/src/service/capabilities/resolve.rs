//! Capability for resolving entity candidates to canonical IDs.

use crate::error::MemoryError;
use crate::models::{AccessPayload, EntityCandidate};
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
        let (entity_id, _was_created) = crate::memory::api::resolve_entity(
            &ResolverPort {
                resolver: &ctx.entity_resolver,
                entity_service: &ctx.entity_service,
            },
            &super::ServiceRateLimitPort { ctx },
            &crate::memory::api::ResolveCommand {
                candidate,
                caller_id: access.and_then(|payload| payload.caller_id),
            },
        )
        .await?;
        Ok(entity_id)
    }
}

/// Adapts the legacy `EntityResolver`/`EntityService` pair to
/// the memory-owned resolution port.
///
/// Expiry removal: Phase 5, when entity resolution becomes
/// knowledge-owned and the legacy pair is deleted.
struct ResolverPort<'a> {
    resolver: &'a crate::service::entity_resolution::EntityResolver,
    entity_service: &'a crate::service::entity::EntityService,
}

#[async_trait::async_trait]
impl crate::memory::api::EntityResolutionPort for ResolverPort<'_> {
    async fn resolve_or_create(
        &self,
        candidate: crate::models::EntityCandidate,
    ) -> Result<(String, bool), MemoryError> {
        self.resolver
            .resolve_or_create(self.entity_service, candidate)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::EntityCandidate;
    use crate::service::MemoryService;
    use crate::service::capabilities::test_support::make_context_base;
    use crate::service::mock_db::MockDbClient;
    use crate::storage::{DbClient, SurrealDbClient};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

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

    #[derive(Clone)]
    struct RecordingDbClient {
        inner: Arc<SurrealDbClient>,
        select_table_calls: Arc<Mutex<Vec<String>>>,
        create_calls: Arc<Mutex<Vec<String>>>,
    }

    impl RecordingDbClient {
        fn new(inner: Arc<SurrealDbClient>) -> Self {
            Self {
                inner,
                select_table_calls: Arc::new(Mutex::new(Vec::new())),
                create_calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn select_table_calls(&self) -> Vec<String> {
            self.select_table_calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }

        fn create_calls(&self) -> Vec<String> {
            self.create_calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }
    }

    #[async_trait::async_trait]
    impl DbClient for RecordingDbClient {
        async fn select_one(
            &self,
            record_id: &str,
            namespace: &str,
        ) -> Result<Option<Value>, MemoryError> {
            self.inner.select_one(record_id, namespace).await
        }

        async fn select_table(
            &self,
            table: &str,
            namespace: &str,
        ) -> Result<Vec<Value>, MemoryError> {
            self.select_table_calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(table.to_string());
            self.inner.select_table(table, namespace).await
        }

        async fn create(
            &self,
            record_id: &str,
            content: Value,
            namespace: &str,
        ) -> Result<Value, MemoryError> {
            self.create_calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(record_id.to_string());
            self.inner.create(record_id, content, namespace).await
        }

        async fn update(
            &self,
            record_id: &str,
            content: Value,
            namespace: &str,
        ) -> Result<Value, MemoryError> {
            self.inner.update(record_id, content, namespace).await
        }

        async fn query(
            &self,
            sql: &str,
            vars: Option<Value>,
            namespace: &str,
        ) -> Result<Value, MemoryError> {
            self.inner.query(sql, vars, namespace).await
        }

        async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError> {
            self.inner.apply_migrations(namespace).await
        }
    }

    #[tokio::test]
    async fn resolve_uses_indexed_entity_lookup_instead_of_table_scan() {
        let db = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(
                "resolve_indexed_lookup",
                &["org".to_string()],
                "warn",
            )
            .await
            .unwrap(),
        );
        db.apply_migrations("org").await.unwrap();
        db.create(
            "entity:existing",
            json!({
                "entity_id": "entity:existing",
                "entity_type": "person",
                "canonical_name": "Dima Ivanov",
                "canonical_name_normalized": "dima ivanov",
                "aliases": [],
            }),
            "org",
        )
        .await
        .unwrap();

        let recorder = RecordingDbClient::new(db);
        let service = MemoryService::new(
            Arc::new(recorder.clone()),
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .unwrap();

        let resolved = ResolveCapability::resolve(
            &service.build_context(),
            EntityCandidate {
                entity_type: "person".to_string(),
                canonical_name: "Dima Ivanov".to_string(),
                aliases: vec![],
            },
            None,
        )
        .await
        .unwrap();

        assert_eq!(resolved, "entity:existing");
        assert!(
            recorder.select_table_calls().is_empty(),
            "indexed resolve must not scan the entity table"
        );
        assert!(
            recorder.create_calls().is_empty(),
            "indexed resolve must not create an entity"
        );
    }
}
