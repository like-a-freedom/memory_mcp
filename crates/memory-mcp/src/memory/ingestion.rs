use std::sync::Arc;

use serde_json::{Value, json};

use crate::logging::{LogLevel, StdoutLogger};
use crate::models::{AccessPayload, IngestRequest};

use crate::error::MemoryError;
use crate::memory::content_extraction::prepare_ingest_request;
use crate::platform::log_event::log_event;
use crate::platform::rate_limiter::RateLimiter;
use crate::shared::ids::deterministic_episode_id_v2;
use crate::shared::temporal::{normalize_dt, now};
use crate::shared::validation::validate_ingest_request;

/// Internal ingestion metadata for the filesystem watcher pipeline.
///
/// The public `IngestRequest` and `IngestCapability::ingest` remain unchanged;
/// this seam carries extra fields only for internal callers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct IngestionMetadata {
    pub source_lineage: Option<String>,
    /// Redacted identifier used only by generic ingest logs.
    pub log_source_id: Option<String>,
}

#[cfg(feature = "streamable-http")]
fn is_duplicate_episode_error(error: &MemoryError) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("already exists") || message.contains("duplicate")
}

/// Handles episode ingestion: file parsing, deduplication, and persistence.
#[derive(Clone)]
pub struct IngestionService {
    episode_store: crate::memory::episode_store::EpisodeStoreClient,
    logger: StdoutLogger,
    rate_limiter: Arc<RateLimiter>,
    #[cfg(feature = "streamable-http")]
    outbox_enabled: bool,
}

impl IngestionService {
    pub(crate) fn new(
        db_client: Arc<dyn crate::storage::DbClient>,
        active_namespace: String,
        logger: StdoutLogger,
        rate_limiter: Arc<RateLimiter>,
    ) -> Self {
        Self {
            episode_store: crate::memory::episode_store::EpisodeStoreClient::new(
                db_client,
                active_namespace,
            ),
            logger,
            rate_limiter,
            #[cfg(feature = "streamable-http")]
            outbox_enabled: false,
        }
    }

    #[cfg(feature = "streamable-http")]
    pub(crate) fn with_outbox(mut self) -> Self {
        self.outbox_enabled = true;
        self
    }

    pub async fn ingest(
        &self,
        request: IngestRequest,
        access: Option<AccessPayload>,
    ) -> Result<String, MemoryError> {
        self.ingest_with_metadata(request, access, IngestionMetadata::default())
            .await
    }

    pub(crate) async fn ingest_with_metadata(
        &self,
        request: IngestRequest,
        access: Option<AccessPayload>,
        metadata: IngestionMetadata,
    ) -> Result<String, MemoryError> {
        self.rate_limiter.check_access(access.as_ref())?;

        let ingest_transport =
            crate::memory::content_extraction::detect_ingest_transport(&request.content);
        let original_source_id = request.source_id.clone();
        let original_content_len = request.content.len();
        // Generic ingest logs must never expose full filesystem paths or full
        // hashes; internal callers supply a redacted `log_source_id` instead.
        let log_source_id = metadata
            .log_source_id
            .as_deref()
            .unwrap_or(&request.source_id)
            .to_string();
        self.logger.log(
            log_event(
                "ingest.prepare",
                json!({
                    "source_type": request.source_type,
                    "source_id": log_source_id,
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
                    "source_id": log_source_id,
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
                [record] => crate::storage::value_helpers::string_from_value(
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
            let mut payload = serde_json::Map::from_iter([
                ("episode_id".to_string(), json!(episode_id)),
                ("source_type".to_string(), json!(request.source_type)),
                ("source_id".to_string(), json!(request.source_id)),
                ("content".to_string(), json!(request.content)),
                ("t_ref".to_string(), json!(normalize_dt(request.t_ref))),
                ("t_ingested".to_string(), json!(normalize_dt(t_ingested))),
                ("policy_tags".to_string(), json!(request.policy_tags)),
            ]);
            if let Some(lineage) = metadata.source_lineage.as_deref() {
                let trimmed = lineage.trim();
                if trimmed.is_empty() {
                    return Err(MemoryError::Validation(
                        "source_lineage must be non-empty when provided".to_string(),
                    ));
                }
                payload.insert("source_lineage".to_string(), json!(trimmed.to_string()));
            }
            let content = Value::Object(payload);
            // Whether this call actually wrote. A duplicate is not new
            // knowledge — including one the outbox reports as a swallowed
            // duplicate error — and a freshness clock refreshed by a re-ingest
            // of an unchanged source is exactly the "idle looks fed" failure
            // the clock exists to catch.
            #[cfg(feature = "streamable-http")]
            let wrote = if self.outbox_enabled {
                match self
                    .episode_store
                    .create_with_event(&episode_id, content, "ui://memory/apps/ingestion_review")
                    .await
                {
                    Ok(()) => true,
                    Err(error) if is_duplicate_episode_error(&error) => false,
                    Err(error) => return Err(error),
                }
            } else {
                self.episode_store.create(&episode_id, content).await?;
                true
            };
            #[cfg(not(feature = "streamable-http"))]
            let wrote = {
                self.episode_store.create(&episode_id, content).await?;
                true
            };

            // The knowledge clock, stamped by the write that just landed and by
            // nothing above it.
            //
            // It belongs here rather than in the MCP tool adapter because this
            // is the one place every capture path passes through: the `ingest`
            // tool, the lifecycle hooks, agent-memory capture and the
            // filesystem watcher (`ingest_with_metadata`) all store their
            // episode through this function. Stamped at the transport, the
            // gauge would read absent for a deployment whose knowledge arrives
            // unattended — precisely the one whose freshness nothing else would
            // be able to check.
            if wrote {
                crate::observability::record_knowledge_write();
            }
        } else {
            self.logger.log(
                log_event(
                    "ingest.duplicate",
                    json!({
                        "episode_id": episode_id,
                        "source_id": log_source_id,
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
                    "source_id": log_source_id,
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
