//! Capability for episode ingestion.

use crate::models::{AccessPayload, IngestRequest};
use crate::service::error::MemoryError;
use crate::service::service_context::ServiceContext;

/// Capability for ingesting raw source material as an episode.
pub struct IngestCapability;

impl IngestCapability {
    /// Ingests a new episode, delegating to `IngestionService`.
    pub async fn ingest(
        ctx: &ServiceContext,
        request: IngestRequest,
        access: Option<AccessPayload>,
    ) -> Result<String, MemoryError> {
        ctx.enforce_rate_limit(access.as_ref())?;
        ctx.ingestion_service.ingest(request, access).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::models::IngestRequest;
    use crate::service::capabilities::ingest::IngestCapability;
    use crate::service::capabilities::test_support::make_context_base;
    use crate::service::mock_db::MockDbClient;
    use crate::service::util::RateLimiter;

    #[tokio::test]
    async fn ingest_delegates_to_ingestion_service() {
        let t_ref = chrono::Utc::now();
        let expected_id =
            crate::service::util::deterministic_episode_id("inline", "cap-ingest", t_ref, "org");
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
                scope: "org".into(),
                project: None,
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await;

        assert_eq!(result.unwrap(), expected_id);
    }

    #[tokio::test]
    async fn ingest_respects_rate_limit() {
        let db = MockDbClient::new();
        let mut ctx = make_context_base(db);
        ctx.rate_limiter = Arc::new(RateLimiter::new(1, 1));

        let access = crate::models::AccessPayload {
            caller_id: Some("user-a".into()),
            ..Default::default()
        };
        let t_ref = chrono::Utc::now();
        let request = || IngestRequest {
            source_type: "inline".into(),
            source_id: "x".into(),
            content: "c".into(),
            t_ref,
            scope: "org".into(),
            project: None,
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
        };

        let _ = IngestCapability::ingest(&ctx, request(), Some(access.clone())).await;
        let result = IngestCapability::ingest(&ctx, request(), Some(access)).await;
        assert!(matches!(
            result,
            Err(crate::service::MemoryError::Validation(ref msg)) if msg == "rate limit exceeded"
        ));
    }
}
