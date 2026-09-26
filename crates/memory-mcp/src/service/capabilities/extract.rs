//! Capability for extracting entities and facts from an episode.
//!
//! Delegates to `episode::extract_from_episode`, which operates on
//! `&ServiceContext` after the capability-seam migration.

use std::time::Instant;

use serde_json::json;

use crate::error::MemoryError;
use crate::logging::LogLevel;
use crate::models::{AccessPayload, ExtractResult};
use crate::service::episode::build_extract_log_result;
use crate::service::episode_from_record;
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
        let timer = Instant::now();
        let caller_id = access
            .as_ref()
            .and_then(|payload| payload.caller_id.clone());
        let command = crate::memory::api::ExtractCommand {
            episode_id: episode_id.to_owned(),
            zero_shot_labels,
            caller_id,
        };
        let extracted = crate::memory::api::extract_from_episode(
            &ExtractionPort { ctx },
            &super::ServiceRateLimitPort { ctx },
            &command,
        )
        .await?;
        let episode = extracted.episode;
        let payload = extracted.result;

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

/// Adapts the legacy context's episode lookup and extraction
/// pipeline to the memory-owned extraction port.
///
/// Expiry removal: Phase 5, when the episode extraction pipeline
/// takes narrow ports instead of the shared context.
struct ExtractionPort<'a> {
    ctx: &'a ServiceContext,
}

#[async_trait::async_trait]
impl crate::memory::api::EpisodeExtractionPort for ExtractionPort<'_> {
    async fn episode_exists(&self, episode_id: &str) -> Result<bool, MemoryError> {
        let record = self.ctx.find_episode_record(episode_id).await?;
        Ok(record.is_some())
    }

    async fn extract(
        &self,
        command: &crate::memory::api::ExtractCommand<'_>,
    ) -> Result<crate::memory::api::ExtractedEpisode, MemoryError> {
        // The episode is re-read here because the caller needs it
        // for the log line (its own fields alongside the fact and
        // entity counts), not just a yes/no existence answer.
        let record = self.ctx.find_episode_record(&command.episode_id).await?;
        let episode = record.as_ref().and_then(episode_from_record);
        let result = crate::service::episode::extract_from_episode(
            self.ctx,
            &command.episode_id,
            command.zero_shot_labels,
        )
        .await?;
        Ok(crate::memory::api::ExtractedEpisode { episode, result })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::capabilities::test_support::make_context_base;
    use crate::service::mock_db::MockDbClient;

    #[tokio::test]
    async fn extract_returns_error_for_missing_episode() {
        let db = MockDbClient::new();
        let ctx = make_context_base(db);
        let result = ExtractCapability::extract(&ctx, "episode:nonexistent", None, None).await;
        assert!(result.is_err(), "extract must fail for missing episode");
        match result {
            Err(MemoryError::NotFound(msg)) => {
                assert!(msg.contains("episode_id not found"));
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }
}
