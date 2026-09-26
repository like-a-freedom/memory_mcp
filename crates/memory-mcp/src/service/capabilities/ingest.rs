//! Capability for episode ingestion.

use crate::error::MemoryError;
use crate::models::{AccessPayload, IngestRequest};
use crate::service::service_context::ServiceContext;

/// Capability for ingesting raw source material as an episode.
pub struct IngestCapability;

impl IngestCapability {
    /// Ingests a new episode through the memory-owned use case.
    pub async fn ingest(
        ctx: &ServiceContext,
        request: IngestRequest,
        access: Option<AccessPayload>,
    ) -> Result<String, MemoryError> {
        // The rate-limit charge stays inside the ingestion port: the
        // use case deliberately does not enforce it, so calling both
        // here and there would debit the shared bucket twice.
        crate::memory::api::ingest_episode(
            &IngestionAdapter { ctx },
            request,
            access,
        )
        .await
    }
}

/// Adapts the legacy ingestion service to the memory-owned port.
struct IngestionAdapter<'a> {
    ctx: &'a ServiceContext,
}

#[async_trait::async_trait]
impl crate::memory::api::IngestionPort for IngestionAdapter<'_> {
    async fn ingest_episode(
        &self,
        request: IngestRequest,
        access: Option<AccessPayload>,
    ) -> Result<String, MemoryError> {
        self.ctx.ingestion_service.ingest(request, access).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::models::IngestRequest;
    use crate::service::capabilities::ingest::IngestCapability;
    use crate::service::capabilities::test_support::{
        make_context_base, make_context_with_rate_limiter,
    };
    use crate::service::mock_db::MockDbClient;
    use crate::service::util::RateLimiter;

    #[tokio::test]
    async fn ingest_delegates_to_ingestion_service() {
        let t_ref = chrono::Utc::now();
        let expected_id =
            crate::service::util::deterministic_episode_id_v2("inline", "cap-ingest", t_ref);
        let db = MockDbClient::new()
            .expect_select_one(&expected_id, None)
            .expect_create(&expected_id, serde_json::Value::Null);
        let ctx = make_context_base(db);

        let result = IngestCapability::ingest(
            &ctx,
            IngestRequest {
                source_type: "inline".into(),
                source_id: "cap-ingest".into(),
                content: "hello world".into(),
                t_ref,
                t_ingested: None,
                policy_tags: vec![],
            },
            None,
        )
        .await;

        assert_eq!(result.unwrap(), expected_id);
    }

    #[tokio::test]
    async fn ingest_respects_rate_limit() {
        let ctx =
            make_context_with_rate_limiter(MockDbClient::new(), Arc::new(RateLimiter::new(1, 1)));

        let access = crate::models::AccessPayload {
            caller_id: Some("user-a".into()),
            ..Default::default()
        };
        let t_ref = chrono::Utc::now();
        let request = |source_id: &str| IngestRequest {
            source_type: "inline".into(),
            source_id: source_id.into(),
            content: "c".into(),
            t_ref,
            t_ingested: None,
            policy_tags: vec![],
        };

        let first = IngestCapability::ingest(&ctx, request("first"), Some(access.clone())).await;
        assert!(
            first.is_ok(),
            "the first ingest should consume one token: {first:?}"
        );

        let second = IngestCapability::ingest(&ctx, request("second"), Some(access)).await;
        assert!(matches!(
            second,
            Err(crate::service::MemoryError::Validation(ref msg)) if msg == "rate limit exceeded"
        ));
    }

    #[tokio::test]
    async fn ingest_debits_one_token_per_successful_request() {
        let ctx =
            make_context_with_rate_limiter(MockDbClient::new(), Arc::new(RateLimiter::new(1, 3)));
        let access = crate::models::AccessPayload {
            caller_id: Some("one-token-user".into()),
            ..Default::default()
        };
        let t_ref = chrono::Utc::now();

        for index in 0..3 {
            let result = IngestCapability::ingest(
                &ctx,
                IngestRequest {
                    source_type: "inline".into(),
                    source_id: format!("one-token-{index}"),
                    content: "c".into(),
                    t_ref,
                    t_ingested: None,
                    policy_tags: vec![],
                },
                Some(access.clone()),
            )
            .await;
            assert!(
                result.is_ok(),
                "request {index} should consume exactly one token: {result:?}"
            );
        }

        let exhausted = IngestCapability::ingest(
            &ctx,
            IngestRequest {
                source_type: "inline".into(),
                source_id: "one-token-exhausted".into(),
                content: "c".into(),
                t_ref,
                t_ingested: None,
                policy_tags: vec![],
            },
            Some(access),
        )
        .await;
        assert!(matches!(
            exhausted,
            Err(crate::service::MemoryError::Validation(ref msg)) if msg == "rate limit exceeded"
        ));
    }
}
