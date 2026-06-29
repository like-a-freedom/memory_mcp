use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::logging::{LogLevel, StdoutLogger};
use crate::models::{AccessPayload, IngestRequest};

use super::error::MemoryError;
use super::ingest::prepare_ingest_request;
use super::util::{deterministic_episode_id, validate_ingest_request, RateLimiter};
use super::{log_event, normalize_dt, now};

/// Handles episode ingestion: file parsing, deduplication, and persistence.
pub struct IngestionService {
    db_client: Arc<dyn crate::storage::DbClient>,
    namespaces: Vec<String>,
    logger: StdoutLogger,
    rate_limiter: Arc<RateLimiter>,
    default_namespace: String,
}

impl IngestionService {
    pub fn new(
        db_client: Arc<dyn crate::storage::DbClient>,
        namespaces: Vec<String>,
        logger: StdoutLogger,
        rate_limiter: Arc<RateLimiter>,
    ) -> Self {
        let default_namespace = namespaces.first().cloned().unwrap_or_else(|| "org".into());
        Self {
            db_client,
            namespaces,
            logger,
            rate_limiter,
            default_namespace,
        }
    }

    /// Rate limit check matching MemoryService::enforce_rate_limit pattern.
    fn rate_limiter_check(&self, access: Option<&AccessPayload>) -> Result<(), MemoryError> {
        if let Some(access) = access
            && let Some(caller) = &access.caller_id
            && !self.rate_limiter.allow(caller)
        {
            return Err(MemoryError::Validation("rate limit exceeded".into()));
        }
        Ok(())
    }

    pub fn namespace_for_scope(&self, scope: &str) -> String {
        // Delegate to the existing resolve_namespace free function from helpers.rs
        let (ns, fell_back) =
            super::resolve_namespace(&self.namespaces, &self.default_namespace, scope);
        if fell_back {
            self.logger.log_warn_dedup(
                std::collections::HashMap::from([
                    ("op".to_string(), json!("scope.namespace_fallback")),
                    ("scope".to_string(), json!(scope)),
                    ("resolved_namespace".to_string(), json!(&ns)),
                ]),
                &format!("scope.namespace_fallback:{}", ns),
                10,
            );
        }
        ns
    }

    pub async fn ingest(
        &self,
        request: IngestRequest,
        access: Option<AccessPayload>,
    ) -> Result<String, MemoryError> {
        self.rate_limiter_check(access.as_ref())?;

        let ingest_transport = super::ingest::detect_ingest_transport(&request.content);
        let original_source_id = request.source_id.clone();

        self.logger.log(
            log_event(
                "ingest.prepare",
                json!({
                    "source_type": request.source_type,
                    "source_id": request.source_id,
                    "scope": request.scope,
                    "project": request.project,
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
                    "scope": request.scope,
                    "project": request.project,
                    "transport": ingest_transport,
                    "source_id_rewritten": request.source_id != original_source_id,
                }),
                json!({
                    "source_id": request.source_id,
                    "content_len": request.content.len(),
                }),
                access.as_ref(),
                None,
                None,
            ),
            LogLevel::Trace,
        );

        validate_ingest_request(&request)?;

        let episode_id = deterministic_episode_id(
            &request.source_type,
            &request.source_id,
            request.t_ref,
            &request.scope,
        );
        let namespace = self.namespace_for_scope(&request.scope);
        let existing = self.db_client.select_one(&episode_id, &namespace).await?;

        if existing.is_none() {
            let t_ingested = request.t_ingested.unwrap_or_else(now);
            let mut payload = serde_json::Map::from_iter([
                ("episode_id".to_string(), json!(episode_id)),
                ("source_type".to_string(), json!(request.source_type)),
                ("source_id".to_string(), json!(request.source_id)),
                ("content".to_string(), json!(request.content)),
                ("t_ref".to_string(), json!(normalize_dt(request.t_ref))),
                (
                    "t_ingested".to_string(),
                    json!(normalize_dt(t_ingested)),
                ),
                ("scope".to_string(), json!(request.scope.clone())),
                (
                    "visibility_scope".to_string(),
                    json!(
                        request
                            .visibility_scope
                            .unwrap_or_else(|| request.scope.clone())
                    ),
                ),
                ("policy_tags".to_string(), json!(request.policy_tags)),
            ]);
            if let Some(project) = request.project.clone() {
                payload.insert("project".to_string(), json!(project));
            }
            self.db_client
                .create(&episode_id, Value::Object(payload), &namespace)
                .await?;
        } else {
            self.logger.log(
                log_event(
                    "ingest.duplicate",
                    json!({
                        "episode_id": episode_id,
                        "source_id": request.source_id,
                        "scope": request.scope,
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
                    "scope": request.scope,
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
