//! Capability for episode ingestion.

use crate::error::MemoryError;
use crate::memory::capabilities::deps::IngestDeps;
use crate::models::{AccessPayload, IngestRequest};

/// Capability for ingesting raw source material as an episode.
pub struct IngestCapability;

impl IngestCapability {
    /// Ingests using an already-built port.
    ///
    /// A caller that already holds an [`IngestDeps`] — the
    /// ingestion-review command, for one — should not have to hand back
    /// a container to ingest an episode.
    pub(crate) async fn ingest_with(
        deps: &IngestDeps,
        request: IngestRequest,
        access: Option<AccessPayload>,
    ) -> Result<String, MemoryError> {
        // The rate-limit charge stays inside the ingestion port: the
        // use case deliberately does not enforce it, so calling both
        // here and there would debit the shared bucket twice.
        crate::memory::api::ingest_episode(
            &IngestionAdapter {
                service: &deps.ingestion_service,
            },
            request,
            access,
        )
        .await
    }
}

/// Adapts the legacy ingestion service to the memory-owned port.
pub(crate) struct IngestionAdapter<'a> {
    pub(crate) service: &'a crate::memory::ingestion::IngestionService,
}

#[async_trait::async_trait]
impl crate::memory::api::IngestionPort for IngestionAdapter<'_> {
    async fn ingest_episode(
        &self,
        request: IngestRequest,
        access: Option<AccessPayload>,
    ) -> Result<String, MemoryError> {
        self.service.ingest(request, access).await
    }
}
#[cfg(test)]
mod tests {
    use crate::memory::capabilities::test_support::{
        make_service_base, make_service_with_rate_limit,
    };
    use crate::models::IngestRequest;
    use crate::service::mock_db::MockDbClient;

    #[tokio::test]
    async fn ingest_delegates_to_ingestion_service() {
        let t_ref = chrono::Utc::now();
        let expected_id =
            crate::shared::ids::deterministic_episode_id_v2("inline", "cap-ingest", t_ref);
        let db = MockDbClient::new()
            .expect_select_one(&expected_id, None)
            .expect_create(&expected_id, serde_json::Value::Null);
        let svc = make_service_base(db);

        let result = crate::service::memory_container_shims::memory_capabilities_ingest::IngestCapability::ingest_from_service(
            &svc,
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
        let svc = make_service_with_rate_limit(MockDbClient::new(), 1, 1);

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

        let first = crate::service::memory_container_shims::memory_capabilities_ingest::IngestCapability::ingest_from_service(&svc, request("first"), Some(access.clone())).await;
        assert!(
            first.is_ok(),
            "the first ingest should consume one token: {first:?}"
        );

        let second = crate::service::memory_container_shims::memory_capabilities_ingest::IngestCapability::ingest_from_service(&svc, request("second"), Some(access)).await;
        assert!(matches!(
            second,
            Err(crate::error::MemoryError::Validation(ref msg)) if msg == "rate limit exceeded"
        ));
    }

    #[tokio::test]
    async fn ingest_debits_one_token_per_successful_request() {
        let svc = make_service_with_rate_limit(MockDbClient::new(), 1, 3);
        let access = crate::models::AccessPayload {
            caller_id: Some("one-token-user".into()),
            ..Default::default()
        };
        let t_ref = chrono::Utc::now();

        for index in 0..3 {
            let result = crate::service::memory_container_shims::memory_capabilities_ingest::IngestCapability::ingest_from_service(
                &svc,
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

        let exhausted = crate::service::memory_container_shims::memory_capabilities_ingest::IngestCapability::ingest_from_service(
            &svc,
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
            Err(crate::error::MemoryError::Validation(ref msg)) if msg == "rate limit exceeded"
        ));
    }
}
