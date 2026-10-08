use crate::error::MemoryError;
use crate::knowledge::CloseTimestamps;
use crate::memory::capabilities::deps::InvalidateDeps;
use crate::memory::context_cache::invalidate_cache;

/// Capability for invalidating facts (marking them as outdated).
pub struct InvalidateCapability;

impl InvalidateCapability {}

/// Adapts the close owner, the fact store, the cache and the claim
/// pipeline to the memory-owned invalidation port.
pub(crate) struct InvalidationPort<'a> {
    pub(crate) deps: &'a InvalidateDeps,
}

impl InvalidationPort<'_> {
    /// The close owner, for retracting a fact with its claims.
    ///
    /// Mirrors the container's `close_store`: the outbox is attached
    /// only when this deployment enabled it, so the close and its
    /// invalidation event commit in one transaction.
    fn close_store(&self) -> crate::knowledge::CloseStoreClient {
        let store = crate::knowledge::CloseStoreClient::new(
            self.deps.db_client.clone(),
            self.deps.active_namespace.clone(),
        );
        #[cfg(feature = "streamable-http")]
        if self.deps.outbox_enabled {
            return store.with_outbox();
        }
        store
    }
}

#[async_trait::async_trait]
impl crate::memory::api::InvalidationPort for InvalidationPort<'_> {
    async fn find_record(
        &self,
        record_id: &str,
    ) -> Result<crate::memory::api::StoredRecord, MemoryError> {
        // Owner-scoped: the fact store refuses a non-fact id, so
        // the existence check cannot be satisfied by another
        // owner's record even though the use case already
        // validated the kind.
        let record = crate::knowledge::FactStoreClient::new(
            self.deps.db_client.clone(),
            self.deps.active_namespace.clone(),
        )
        .select_fact(record_id)
        .await?;
        Ok(if record.is_some() {
            crate::memory::api::StoredRecord::Present
        } else {
            crate::memory::api::StoredRecord::Absent
        })
    }

    async fn close_record(
        &self,
        record_id: &str,
        t_invalid: chrono::DateTime<chrono::Utc>,
        t_invalid_ingested: Option<chrono::DateTime<chrono::Utc>>,
        reason: &str,
    ) -> Result<(), MemoryError> {
        self.close_store()
            .close_record(
                record_id,
                &CloseTimestamps {
                    t_invalid: Some(t_invalid),
                    t_invalid_ingested,
                },
                Some(reason),
            )
            .await
    }

    async fn close_claims_for_fact(&self, fact_id: &str) -> Result<(), MemoryError> {
        self.close_store().close_claims_for_fact(fact_id).await
    }

    fn claim_pipeline_is_wired(&self) -> bool {
        self.deps.claim_store.is_some()
    }

    async fn invalidate_assembled_context(&self) -> Result<(), MemoryError> {
        invalidate_cache(&self.deps.context_cache).await;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::error::MemoryError;
    use crate::memory::capabilities::test_support::make_service_base;
    use crate::models::{AccessPayload, InvalidateRequest};
    use crate::platform::context_cache_key::{CacheKey, CacheView};
    use crate::service::mock_db::MockDbClient;

    fn make_service(db: MockDbClient) -> crate::service::MemoryService {
        make_service_base(db)
    }

    fn fact_request(fact_id: &str) -> InvalidateRequest {
        InvalidateRequest {
            fact_id: fact_id.to_string(),
            reason: "outdated".to_string(),
            t_invalid: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn invalidate_sets_t_invalid_and_updates_record() {
        let db = MockDbClient::new()
            .expect_select_one(
                "fact:1",
                Some(json!({"fact_id": "fact:1", "content": "test", "scope": "personal"})),
            )
            .expect_update("fact:1", json!({"ok": true}));
        let svc = make_service(db);

        let result = crate::service::memory_container_shims::memory_capabilities_invalidate::InvalidateCapability::invalidate_from_service(&svc, fact_request("fact:1"), None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn invalidate_returns_not_found_for_missing_fact() {
        let db = MockDbClient::new();
        let svc = make_service(db);

        let result =
            crate::service::memory_container_shims::memory_capabilities_invalidate::InvalidateCapability::invalidate_from_service(&svc, fact_request("fact:nonexistent"), None).await;
        assert!(matches!(result, Err(MemoryError::NotFound(_))));
    }

    #[tokio::test]
    async fn invalidate_invalidates_cache_for_scope() {
        let db = MockDbClient::new()
            .expect_select_one(
                "fact:2",
                Some(json!({"fact_id": "fact:2", "content": "x", "scope": "team"})),
            )
            .expect_update("fact:2", json!({"ok": true}));
        let svc = make_service(db);

        let cache_key = CacheKey::new(
            "query",
            chrono::Utc::now(),
            5,
            &[],
            CacheView::default(),
            None,
        );
        {
            let mut guard = svc.context_cache.write().await;
            let generation = match guard.lookup(&cache_key) {
                crate::memory::context_cache::ContextCacheLookup::Miss(generation) => generation,
                crate::memory::context_cache::ContextCacheLookup::Hit(_) => {
                    panic!("new cache cannot contain a hit")
                }
            };
            assert_eq!(
                guard.insert(
                    generation,
                    cache_key.clone(),
                    &[crate::models::AssembledContextItem {
                        fact_id: "fact:2".into(),
                        ..Default::default()
                    }],
                ),
                crate::memory::context_cache::CacheInsertOutcome::Stored
            );
        }

        crate::service::memory_container_shims::memory_capabilities_invalidate::InvalidateCapability::invalidate_from_service(&svc, fact_request("fact:2"), None)
            .await
            .unwrap();

        let mut guard = svc.context_cache.write().await;
        assert!(matches!(
            guard.lookup(&cache_key),
            crate::memory::context_cache::ContextCacheLookup::Miss(_)
        ));
        assert_eq!(guard.accounted_bytes(), 0);
    }

    #[tokio::test]
    async fn invalidate_respects_rate_limit() {
        // A burst of one: the first invalidation is allowed and the
        // second for the same caller is refused.
        let db = MockDbClient::new();
        let svc = super::super::test_support::make_service_with_rate_limit(db, 1, 1);

        let access = AccessPayload {
            caller_id: Some("user-a".into()),
            ..Default::default()
        };
        let _ =
            crate::service::memory_container_shims::memory_capabilities_invalidate::InvalidateCapability::invalidate_from_service(&svc, fact_request("fact:x"), Some(access.clone()))
                .await;

        let result =
            crate::service::memory_container_shims::memory_capabilities_invalidate::InvalidateCapability::invalidate_from_service(&svc, fact_request("fact:y"), Some(access)).await;
        assert!(
            matches!(result, Err(MemoryError::Validation(ref msg)) if msg == "rate limit exceeded")
        );
    }

    #[tokio::test]
    async fn invalidate_falls_back_to_namespace_when_scope_missing() {
        let db = MockDbClient::new()
            .expect_select_one(
                "fact:3",
                Some(json!({"fact_id": "fact:3", "content": "no scope"})),
            )
            .expect_update("fact:3", json!({"ok": true}));
        let svc = make_service(db);

        let result = crate::service::memory_container_shims::memory_capabilities_invalidate::InvalidateCapability::invalidate_from_service(&svc, fact_request("fact:3"), None).await;
        assert!(result.is_ok());
    }
}
