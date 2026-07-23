//! Capability for extracting entities and facts from an episode.
//!
//! Delegates to `episode::extract_from_episode`, which operates on
//! `&ServiceContext` after the capability-seam migration.

use std::time::Instant;

use serde_json::json;

use crate::logging::LogLevel;
use crate::models::{AccessPayload, ExtractResult};
use crate::service::episode::build_extract_log_result;
use crate::service::episode_from_record;
use crate::service::error::MemoryError;
use crate::service::log_args_with_duration;
use crate::service::log_event;
use crate::service::service_context::ServiceContext;

/// Capability for extracting entities, facts, and relationships.
pub struct ExtractCapability;

impl ExtractCapability {
    /// Extracts entities and facts from an episode.
    ///
    /// # Arguments
    ///
    /// * `ctx` - Shared service context.
    /// * `episode_id` - The episode to extract from.
    /// * `access` - Optional access context for authorization.
    /// * `zero_shot_labels` - Optional custom entity labels for GLiNER extraction.
    ///   When provided, these labels override the default NER configuration.
    pub async fn extract(
        ctx: &ServiceContext,
        episode_id: &str,
        access: Option<AccessPayload>,
        zero_shot_labels: Option<&[String]>,
    ) -> Result<ExtractResult, MemoryError> {
        ctx.enforce_rate_limit(access.as_ref())?;
        let timer = Instant::now();
        let (record, _) = ctx.find_episode_record(episode_id).await?;
        if record.is_none() {
            return Err(MemoryError::NotFound(format!(
                "episode_id not found: {episode_id}"
            )));
        }
        let episode = record.as_ref().and_then(episode_from_record);
        let payload =
            crate::service::episode::extract_from_episode(ctx, episode_id, zero_shot_labels)
                .await?;
        ctx.logger.log(
            log_event(
                "extract",
                log_args_with_duration(json!({"episode_id": episode_id}), timer.elapsed()),
                build_extract_log_result(
                    episode.as_ref(),
                    payload.entities.len(),
                    &payload.facts,
                    payload.links.len(),
                    payload.warnings.len(),
                ),
                access.as_ref(),
                None,
                None,
            ),
            LogLevel::Info,
        );
        Ok(payload)
    }
}
