//! Capability for extracting entities and facts from an episode.
//!
//! Delegates to `episode::extract_from_episode`, which operates on
//! [`ExtractDeps`].

use std::time::Instant;

use serde_json::json;

use crate::error::MemoryError;
use crate::logging::LogLevel;
use crate::memory::capabilities::deps::ExtractDeps;
use crate::memory::episode::build_extract_log_result;
use crate::memory::episode::episode_from_record;
use crate::models::{AccessPayload, ExtractResult};
use crate::platform::log_event::duration_ms;
use crate::platform::log_event::log_event;

/// Capability for extracting entities, facts, and relationships.
pub struct ExtractCapability;

impl ExtractCapability {
    /// Extracts using an already-built port.
    ///
    /// A caller that holds an [`ExtractDeps`] — the projection pass, for
    /// one — should not have to hand back a container to get an
    /// extraction done.
    pub(crate) async fn extract_with(
        deps: &ExtractDeps,
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
            &ExtractionPort { deps },
            &crate::memory::retrieval_deps::RateLimitDeps {
                rate_limiter: deps.rate_limiter(),
            },
            &command,
        )
        .await?;
        let episode = extracted.episode;
        let payload = extracted.result;

        deps.logger.log(
            log_event(
                "extract",
                json!({"episode_id": episode_id}),
                build_extract_log_result(
                    episode.as_ref(),
                    payload.entities.len(),
                    &payload.facts,
                    payload.links.len(),
                    payload.warnings.len(),
                ),
                access.as_ref(),
                None,
                Some(duration_ms(timer.elapsed())),
            ),
            LogLevel::Info,
        );
        Ok(payload)
    }
}

/// Adapts the owner-scoped episode lookup and the extraction pipeline
/// to the memory-owned extraction port.
pub(crate) struct ExtractionPort<'a> {
    pub(crate) deps: &'a ExtractDeps,
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
        let result = crate::memory::episode::extract_from_episode(
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
    use crate::memory::capabilities::test_support::make_service_base;
    use crate::service::mock_db::MockDbClient;

    #[tokio::test]
    async fn extract_returns_error_for_missing_episode() {
        let db = MockDbClient::new();
        let svc = make_service_base(db);
        let result = crate::service::memory_container_shims::memory_capabilities_extract::ExtractCapability::extract_from_service(&svc, "episode:nonexistent", None, None).await;
        assert!(result.is_err(), "extract must fail for missing episode");
        match result {
            Err(MemoryError::NotFound(msg)) => {
                assert!(msg.contains("episode_id not found"));
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }
}
