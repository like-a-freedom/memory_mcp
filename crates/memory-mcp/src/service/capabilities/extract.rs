//! Capability for extracting entities and facts from an episode.
//!
//! Delegates to `episode::extract_from_episode`, which operates on
//! [`ExtractDeps`].

use std::time::Instant;

use serde_json::json;

use crate::error::MemoryError;
use crate::logging::LogLevel;
use crate::models::{AccessPayload, ExtractResult};
use crate::service::capabilities::deps::ExtractDeps;
use crate::service::episode::build_extract_log_result;
use crate::service::episode_from_record;
use crate::service::log_args_with_duration;
use crate::service::log_event;

/// Capability for extracting entities, facts, and relationships.
pub struct ExtractCapability;

impl ExtractCapability {
    /// Extracts entities and facts from an episode.
    ///
    /// # Arguments
    ///
    /// * `deps` - The extraction pipeline's dependencies.
    /// * `episode_id` - The episode to extract from.
    /// * `access` - Optional access context for authorization.
    /// * `zero_shot_labels` - Optional custom entity labels for GLiNER extraction.
    ///   When provided, these labels override the default NER configuration.
    pub async fn extract(
        service: &crate::service::MemoryService,
        episode_id: &str,
        access: Option<AccessPayload>,
        zero_shot_labels: Option<&[String]>,
    ) -> Result<ExtractResult, MemoryError> {
        let deps = ExtractDeps::from(service);
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
            &ExtractionPort { deps: &deps },
            &super::RateLimitDeps {
                rate_limiter: &deps.rate_limiter(),
            },
            &command,
        )
        .await?;
        let episode = extracted.episode;
        let payload = extracted.result;

        deps.logger.log(
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

/// Adapts the owner-scoped episode lookup and the extraction pipeline
/// to the memory-owned extraction port.
struct ExtractionPort<'a> {
    deps: &'a ExtractDeps,
}

#[async_trait::async_trait]
impl crate::memory::api::EpisodeExtractionPort for ExtractionPort<'_> {
    async fn episode_exists(&self, episode_id: &str) -> Result<bool, MemoryError> {
        let record = self.deps.find_episode_record(episode_id).await?;
        Ok(record.is_some())
    }

    async fn extract(
        &self,
        command: &crate::memory::api::ExtractCommand<'_>,
    ) -> Result<crate::memory::api::ExtractedEpisode, MemoryError> {
        // The episode is re-read here because the caller needs it
        // for the log line (its own fields alongside the fact and
        // entity counts), not just a yes/no existence answer.
        let record = self.deps.find_episode_record(&command.episode_id).await?;
        let episode = record.as_ref().and_then(episode_from_record);
        let result = crate::service::episode::extract_from_episode(
            self.deps,
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
    use crate::service::capabilities::test_support::make_service_base;
    use crate::service::mock_db::MockDbClient;

    #[tokio::test]
    async fn extract_returns_error_for_missing_episode() {
        let db = MockDbClient::new();
        let svc = make_service_base(db);
        let result = ExtractCapability::extract(&svc, "episode:nonexistent", None, None).await;
        assert!(result.is_err(), "extract must fail for missing episode");
        match result {
            Err(MemoryError::NotFound(msg)) => {
                assert!(msg.contains("episode_id not found"));
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }
}
