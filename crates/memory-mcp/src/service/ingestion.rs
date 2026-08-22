use std::sync::Arc;

use serde_json::{Value, json};

use crate::logging::{LogLevel, StdoutLogger};
use crate::models::{AccessPayload, IngestRequest};

use super::content_extraction::prepare_ingest_request;
use super::util::{RateLimiter, deterministic_episode_id_v2, validate_ingest_request};
use super::{log_event, normalize_dt, now};
use crate::error::MemoryError;

/// Handles episode ingestion: file parsing, deduplication, and persistence.
#[derive(Clone)]
pub struct IngestionService {
    episode_store: crate::storage::EpisodeStoreClient,
    logger: StdoutLogger,
    rate_limiter: Arc<RateLimiter>,
}

impl IngestionService {
    pub(crate) fn new(
        db_client: Arc<dyn crate::storage::DbClient>,
        active_namespace: String,
        logger: StdoutLogger,
        rate_limiter: Arc<RateLimiter>,
    ) -> Self {
        Self {
            episode_store: crate::storage::EpisodeStoreClient::new(db_client, active_namespace),
            logger,
            rate_limiter,
        }
    }

    pub async fn ingest(
        &self,
        request: IngestRequest,
        access: Option<AccessPayload>,
    ) -> Result<String, MemoryError> {
        self.rate_limiter.check_access(access.as_ref())?;

        let ingest_transport = super::content_extraction::detect_ingest_transport(&request.content);
        let original_source_id = request.source_id.clone();
        let original_content_len = request.content.len();
        self.logger.log(
            log_event(
                "ingest.prepare",
                json!({
                    "source_type": request.source_type,
                    "source_id": request.source_id,
                    "transport": ingest_transport,
                }),
                json!({}),
                access.as_ref(),
                None,
                None,
            ),
            LogLevel::Debug,
        );
        let request = prepare_ingest_request(request).await?;
        self.logger.log(
            log_event(
                "ingest.prepared",
                json!({
                    "transport": ingest_transport,
                    "source_id_rewritten": request.source_id != original_source_id,
                }),
                json!({
                    "source_id": request.source_id,
                    "content_len": request.content.len(),
                    "original_content_len": original_content_len,
                }),
                access.as_ref(),
                None,
                None,
            ),
            LogLevel::Trace,
        );

        validate_ingest_request(&request)?;

        let v2_episode_id =
            deterministic_episode_id_v2(&request.source_type, &request.source_id, request.t_ref);
        let existing = self.episode_store.select_one(&v2_episode_id).await?;
        let episode_id = if existing.is_some() {
            v2_episode_id.clone()
        } else {
            let legacy_matches = self
                .episode_store
                .select_by_source_identity(
                    &request.source_type,
                    &request.source_id,
                    &normalize_dt(request.t_ref),
                    2,
                )
                .await?;
            match legacy_matches.as_slice() {
                [] => v2_episode_id.clone(),
                [record] => crate::service::value_helpers::string_from_value(
                    record
                        .get("episode_id")
                        .or_else(|| record.get("id"))
                        .ok_or_else(|| {
                            MemoryError::Conflict(
                                "legacy episode match has no stable episode_id".to_string(),
                            )
                        })?,
                )
                .ok_or_else(|| {
                    MemoryError::Conflict(
                        "legacy episode match has an unreadable episode_id".to_string(),
                    )
                })?,
                _ => {
                    return Err(MemoryError::Conflict(format!(
                        "ambiguous legacy episode identity for source_type={} source_id={} t_ref={}; refusing to create a duplicate",
                        request.source_type,
                        request.source_id,
                        normalize_dt(request.t_ref),
                    )));
                }
            }
        };

        if existing.is_none() && episode_id == v2_episode_id {
            let t_ingested = request.t_ingested.unwrap_or_else(now);
            let payload = serde_json::Map::from_iter([
                ("episode_id".to_string(), json!(episode_id)),
                ("source_type".to_string(), json!(request.source_type)),
                ("source_id".to_string(), json!(request.source_id)),
                ("content".to_string(), json!(request.content)),
                ("t_ref".to_string(), json!(normalize_dt(request.t_ref))),
                ("t_ingested".to_string(), json!(normalize_dt(t_ingested))),
                ("policy_tags".to_string(), json!(request.policy_tags)),
            ]);
            self.episode_store
                .create(&episode_id, Value::Object(payload))
                .await?;
        } else {
            self.logger.log(
                log_event(
                    "ingest.duplicate",
                    json!({
                        "episode_id": episode_id,
                        "source_id": request.source_id,
                    }),
                    json!({"status": "existing_episode_reused"}),
                    access.as_ref(),
                    None,
                    None,
                ),
                LogLevel::Debug,
            );
        }

        self.logger.log(
            log_event(
                "ingest",
                json!({
                    "source_type": request.source_type,
                    "source_id": request.source_id,
                    "t_ref": normalize_dt(request.t_ref),
                }),
                json!({"episode_id": episode_id}),
                access.as_ref(),
                None,
                None,
            ),
            LogLevel::Info,
        );

        Ok(episode_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::StdoutLogger;
    use crate::models::IngestRequest;
    use crate::service::mock_db::MockDbClient;
    use crate::service::util::RateLimiter;
    use chrono::Utc;
    use std::sync::Arc;

    #[tokio::test]
    async fn ingest_creates_new_episode() {
        let t_ref = Utc::now();
        let expected_id =
            super::super::util::deterministic_episode_id_v2("inline", "test-content", t_ref);

        let db = MockDbClient::new()
            .expect_select_one(&expected_id, None)
            .expect_create(&expected_id, serde_json::Value::Null);

        let svc = IngestionService::new(
            Arc::new(db),
            "org".to_string(),
            StdoutLogger::new("warn"),
            Arc::new(RateLimiter::new(1000, 100)),
        );

        let result = svc
            .ingest(
                IngestRequest {
                    source_type: "inline".into(),
                    source_id: "test-content".into(),
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
    async fn ingest_returns_existing_episode_id_on_v2_duplicate() {
        let t_ref = Utc::now();
        let expected_id =
            super::super::util::deterministic_episode_id_v2("inline", "dup-content", t_ref);

        let db = MockDbClient::new().expect_select_one(
            &expected_id,
            Some(serde_json::json!({"episode_id": &expected_id, "content": "old"})),
        );

        let svc = IngestionService::new(
            Arc::new(db),
            "org".to_string(),
            StdoutLogger::new("warn"),
            Arc::new(RateLimiter::new(1000, 100)),
        );

        let result = svc
            .ingest(
                IngestRequest {
                    source_type: "inline".into(),
                    source_id: "dup-content".into(),
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
}
