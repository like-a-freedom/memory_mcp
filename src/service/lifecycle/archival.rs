//! Episode archival background worker.
//!
//! Periodically marks old episodes as archived when they have no active facts.

use chrono::Utc;
use serde_json::json;
use tokio::time::{self, Duration as TokioDuration};

use crate::service::{MemoryError, MemoryService};

const ARCHIVAL_BATCH_LIMIT: i32 = 500;

/// Spawns the archival worker background task.
pub fn spawn_archival_worker(
    service: MemoryService,
    interval_secs: u64,
    age_days: u32,
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
            interval.tick().await;
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
    let now = Utc::now();
    let cutoff = now - chrono::Duration::days(age_days as i64);
    let cutoff_str = crate::service::normalize_dt(cutoff);
    let mut archived = 0;

    for namespace in &service.namespaces {
        let episodes = service
            .db_client
            .select_episodes_for_archival(namespace, &cutoff_str, ARCHIVAL_BATCH_LIMIT)
            .await?;

        for record in episodes {
            let episode_id = record
                .get("episode_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| MemoryError::Validation("missing episode_id".into()))?;

            let has_active_facts =
                check_episode_has_active_facts(service, episode_id, namespace).await?;
            let has_recent_heat =
                check_episode_has_recent_fact_access(service, episode_id, namespace, age_days)
                    .await?;

            if !has_active_facts && !has_recent_heat {
                let payload = json!({
                    "status": "archived",
                    "archived_at": crate::service::normalize_dt(now),
                });

                service
                    .db_client
                    .update(episode_id, payload, namespace)
                    .await?;

                archived += 1;
            }
        }
    }

    Ok(archived)
}

/// Checks if an episode has any active (non-invalidated) facts.
async fn check_episode_has_active_facts(
    service: &MemoryService,
    episode_id: &str,
    namespace: &str,
) -> Result<bool, MemoryError> {
    let cutoff = crate::service::normalize_dt(Utc::now());
    let facts = service
        .db_client
        .select_active_facts_by_episode(namespace, episode_id, &cutoff, 1)
        .await?;

    Ok(!facts.is_empty())
}

async fn check_episode_has_recent_fact_access(
    service: &MemoryService,
    episode_id: &str,
    namespace: &str,
    age_days: u32,
) -> Result<bool, MemoryError> {
    let hot_cutoff =
        crate::service::normalize_dt(Utc::now() - chrono::Duration::days(age_days as i64));
    let result = service
        .db_client
        .query(
            "SELECT fact_id FROM fact WHERE source_episode = $episode_id AND last_accessed IS NOT NONE AND last_accessed >= type::datetime($hot_cutoff) LIMIT 1",
            Some(json!({"episode_id": episode_id, "hot_cutoff": hot_cutoff})),
            namespace,
        )
        .await?;

    Ok(result.as_array().is_some_and(|rows| !rows.is_empty()))
}
