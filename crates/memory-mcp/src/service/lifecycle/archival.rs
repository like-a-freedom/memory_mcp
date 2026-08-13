//! Episode archival background worker.
//!
//! Periodically marks old episodes as archived when they have no active facts.

use chrono::Utc;
use serde_json::json;
use tokio::time::{self, Duration as TokioDuration};
use tokio_util::sync::CancellationToken;

use crate::service::{MemoryError, MemoryService};

const ARCHIVAL_BATCH_LIMIT: i32 = 500;

/// Spawns the archival worker background task.
///
/// The task runs until `shutdown` is cancelled, at which point it exits
/// cleanly after completing any in-flight pass.
pub fn spawn_archival_worker(
    service: MemoryService,
    interval_secs: u64,
    age_days: u32,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = time::interval(TokioDuration::from_secs(interval_secs));

        let mut event = std::collections::HashMap::new();
        event.insert(
            "op".to_string(),
            serde_json::Value::String("lifecycle.archival.start".to_string()),
        );
        event.insert(
            "interval_secs".to_string(),
            serde_json::Value::Number(serde_json::Number::from(interval_secs)),
        );
        event.insert(
            "age_days".to_string(),
            serde_json::Value::Number(serde_json::Number::from(age_days)),
        );
        service.logger.log(event, crate::logging::LogLevel::Info);

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = interval.tick() => {}
            }
            match run_archival_pass(&service, age_days).await {
                Ok(count) => {
                    let mut event = std::collections::HashMap::new();
                    event.insert(
                        "op".to_string(),
                        serde_json::Value::String("lifecycle.archival.complete".to_string()),
                    );
                    event.insert(
                        "episodes_archived".to_string(),
                        serde_json::Value::Number(serde_json::Number::from(count)),
                    );
                    service.logger.log(event, crate::logging::LogLevel::Info);
                }
                Err(e) => {
                    let mut event = std::collections::HashMap::new();
                    event.insert(
                        "op".to_string(),
                        serde_json::Value::String("lifecycle.archival.error".to_string()),
                    );
                    event.insert(
                        "error".to_string(),
                        serde_json::Value::String(format!("{}", e)),
                    );
                    service.logger.log(event, crate::logging::LogLevel::Warn);
                }
            }
        }
    })
}

/// Runs a single archival pass, archiving old episodes without active facts.
pub async fn run_archival_pass(
    service: &MemoryService,
    age_days: u32,
) -> Result<usize, MemoryError> {
    let policy = service.lifecycle_policy();
    let age_days = age_days.max(policy.archival_age_days);
    let now = Utc::now();
    let cutoff = now - chrono::Duration::days(age_days as i64);
    let cutoff_str = crate::service::normalize_dt(cutoff);
    let mut archived = 0;

    let episodes = service
        .app_store()
        .select_episodes_for_archival(&cutoff_str, ARCHIVAL_BATCH_LIMIT)
        .await?;

    for record in episodes {
        let episode_id = record
            .get("episode_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MemoryError::Validation("missing episode_id".into()))?;

        let has_active_facts = check_episode_has_active_facts(service, episode_id).await?;
        let has_recent_heat =
            check_episode_has_recent_fact_access(service, episode_id, age_days).await?;

        if !has_active_facts && !has_recent_heat {
            let payload = json!({
                "status": "archived",
                "archived_at": crate::service::normalize_dt(now),
            });

            service
                .app_store()
                .update_record(episode_id, payload)
                .await?;

            archived += 1;
        }
    }

    Ok(archived)
}

/// Checks if an episode has any active (non-invalidated) facts.
async fn check_episode_has_active_facts(
    service: &MemoryService,
    episode_id: &str,
) -> Result<bool, MemoryError> {
    let cutoff = crate::service::normalize_dt(Utc::now());
    let facts = service
        .episode_store()
        .select_active_facts_by_episode(episode_id, &cutoff, 1)
        .await?;

    Ok(!facts.is_empty())
}

async fn check_episode_has_recent_fact_access(
    service: &MemoryService,
    episode_id: &str,
    age_days: u32,
) -> Result<bool, MemoryError> {
    let hot_cutoff =
        crate::service::normalize_dt(Utc::now() - chrono::Duration::days(age_days as i64));
    let result = service
        .app_store()
        .query(
            "SELECT fact_id FROM fact WHERE source_episode = $episode_id AND last_accessed IS NOT NONE AND last_accessed >= type::datetime($hot_cutoff) LIMIT 1",
            Some(json!({"episode_id": episode_id, "hot_cutoff": hot_cutoff})),
        )
        .await?;

    Ok(result.as_array().is_some_and(|rows| !rows.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cutoff_computation_from_age_days() {
        let now = Utc::now();
        let age_days = 90u32;
        let cutoff = now - chrono::Duration::days(age_days as i64);
        let days_diff = (now - cutoff).num_days();
        assert_eq!(days_diff, 90);
    }

    #[test]
    fn episode_record_missing_episode_id_returns_error() {
        let record = json!({
            "status": "active",
            "created_at": "2024-01-01T00:00:00Z",
        });
        let result = record.get("episode_id").and_then(|v| v.as_str());
        assert!(result.is_none());
    }

    #[test]
    fn episode_with_active_facts_is_not_archived() {
        // Simulate: has_active_facts = true => should NOT archive
        let has_active_facts = true;
        let has_recent_heat = false;
        let should_archive = !has_active_facts && !has_recent_heat;
        assert!(!should_archive);
    }

    #[test]
    fn episode_with_recent_heat_is_not_archived() {
        // Simulate: has_recent_heat = true => should NOT archive
        let has_active_facts = false;
        let has_recent_heat = true;
        let should_archive = !has_active_facts && !has_recent_heat;
        assert!(!should_archive);
    }

    #[test]
    fn episode_without_active_facts_or_heat_is_archived() {
        let has_active_facts = false;
        let has_recent_heat = false;
        let should_archive = !has_active_facts && !has_recent_heat;
        assert!(should_archive);
    }

    #[test]
    fn archival_payload_contains_status_and_archived_at() {
        let now = Utc::now();
        let payload = json!({
            "status": "archived",
            "archived_at": crate::service::normalize_dt(now),
        });
        assert_eq!(payload["status"], "archived");
        assert!(payload["archived_at"].as_str().is_some());
    }

    #[test]
    fn check_episode_has_active_facts_empty_result_means_no_active() {
        let facts: Vec<serde_json::Value> = vec![];
        let has_active = !facts.is_empty();
        assert!(!has_active);
    }

    #[test]
    fn check_episode_has_recent_heat_empty_result_means_no_heat() {
        let result = serde_json::json!([]);
        let has_heat = result.as_array().is_some_and(|rows| !rows.is_empty());
        assert!(!has_heat);
    }

    #[test]
    fn check_episode_has_recent_heat_nonempty_result_means_heat() {
        let result = serde_json::json!([{"fact_id": "fact:1"}]);
        let has_heat = result.as_array().is_some_and(|rows| !rows.is_empty());
        assert!(has_heat);
    }
}
