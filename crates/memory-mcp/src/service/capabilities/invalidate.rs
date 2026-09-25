use crate::error::MemoryError;
use crate::models::{AccessPayload, InvalidateRequest};
use crate::service::cache::invalidate_cache;
use crate::service::service_context::ServiceContext;
use crate::storage::CloseTimestamps;

/// Capability for invalidating facts (marking them as outdated).
pub struct InvalidateCapability;

impl InvalidateCapability {
    /// Invalidates a fact by closing both bi-temporal fields.
    ///
    /// The fact is looked up to verify existence, then closed through the
    /// storage close owner: `t_invalid` takes the caller-supplied valid time,
    /// `t_invalid_ingested` defaults to server-side now, and `request.reason`
    /// is persisted to `invalidation_reason`. Derived claims are closed when
    /// the claim pipeline is wired.
    pub async fn invalidate(
        ctx: &ServiceContext,
        request: InvalidateRequest,
        access: Option<AccessPayload>,
    ) -> Result<(), MemoryError> {
        crate::memory::api::invalidate_fact(
            &InvalidationPort { ctx },
            &crate::service::capabilities::ServiceRateLimitPort { ctx },
            &crate::memory::api::InvalidationRequest {
                fact_id: request.fact_id,
                t_invalid: request.t_invalid,
                reason: request.reason,
                caller_id: access.and_then(|payload| payload.caller_id),
            },
        )
        .await
    }
}

/// Adapts the legacy context's record/close/cache accessors to
/// the memory-owned invalidation port.
///
/// Expiry removal: Phase 5, when the fact close becomes
/// knowledge-owned storage rather than a context accessor.
struct InvalidationPort<'a> {
    ctx: &'a ServiceContext,
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
        let record = crate::storage::FactStoreClient::new(
            self.ctx.db_client.clone(),
            self.ctx.active_namespace.clone(),
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
        self.ctx
            .close_store()
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
        self.ctx.close_store().close_claims_for_fact(fact_id).await
    }

    fn claim_pipeline_is_wired(&self) -> bool {
        self.ctx.claim_store.is_some()
    }

    async fn invalidate_assembled_context(&self) -> Result<(), MemoryError> {
        invalidate_cache(&self.ctx.context_cache).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use crate::error::MemoryError;
    use crate::models::{AccessPayload, InvalidateRequest};
    use crate::service::cache::{CacheKey, CacheView};
    use crate::service::capabilities::invalidate::InvalidateCapability;
    use crate::service::mock_db::MockDbClient;
    use crate::service::service_context::ServiceContext;
    use crate::service::util::RateLimiter;

    fn make_context(db: MockDbClient) -> ServiceContext {
        super::super::test_support::make_context_base(db)
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
        let ctx = make_context(db);

        let result = InvalidateCapability::invalidate(&ctx, fact_request("fact:1"), None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn invalidate_returns_not_found_for_missing_fact() {
        let db = MockDbClient::new();
        let ctx = make_context(db);

        let result =
            InvalidateCapability::invalidate(&ctx, fact_request("fact:nonexistent"), None).await;
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
        let ctx = make_context(db);

        let cache_key = CacheKey::new(
            "query",
            chrono::Utc::now(),
            5,
            &[],
            CacheView::default(),
            None,
        );
        {
            let mut guard = ctx.context_cache.write().await;
            guard.put(
                cache_key.clone(),
                vec![crate::models::AssembledContextItem {
                    fact_id: "fact:2".into(),
                    ..Default::default()
                }],
            );
        }

        InvalidateCapability::invalidate(&ctx, fact_request("fact:2"), None)
            .await
            .unwrap();

        let mut guard = ctx.context_cache.write().await;
        assert!(
            guard.get(&cache_key).is_none(),
            "cache should be invalidated for scope 'team'"
        );
    }

    #[tokio::test]
    async fn invalidate_respects_rate_limit() {
        let db = MockDbClient::new();
        let mut ctx = make_context(db);
        ctx.rate_limiter = Arc::new(RateLimiter::new(1, 1));

        let access = AccessPayload {
            caller_id: Some("user-a".into()),
            ..Default::default()
        };
        let _ =
            InvalidateCapability::invalidate(&ctx, fact_request("fact:x"), Some(access.clone()))
                .await;

        let result =
            InvalidateCapability::invalidate(&ctx, fact_request("fact:y"), Some(access)).await;
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
        let ctx = make_context(db);

        let result = InvalidateCapability::invalidate(&ctx, fact_request("fact:3"), None).await;
        assert!(result.is_ok());
    }
}
