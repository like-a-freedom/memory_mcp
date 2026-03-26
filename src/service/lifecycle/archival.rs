//! Episode archival background worker.
//!
//! Periodically marks old episodes as archived when they have no active facts.

use chrono::{Duration, Utc};
use serde_json::json;
use tokio::time::{self, Duration as TokioDuration};

use crate::service::{MemoryService, MemoryError};

/// Spawns the archival worker background task.
pub fn spawn_archival_worker(
    service: MemoryService,
    interval_secs: u64,
    age_days: u32,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = time::interval(TokioDuration::from_secs(interval_secs));

        let mut event = std::collections::HashMap::new();
        event.insert("op".to_string(), serde_json::Value::String("lifecycle.archival.start".to_string()));
        event.insert(
            "interval_secs".to_string(),
            serde_json::Value::Number(serde_json::Number::from(interval_secs)),
        );
        event.insert(
            "age_days".to_string(),
            serde_json::Value::Number(serde_json::Number::from(age_days)),
        );
        service.logger.log(
            event,
            crate::logging::LogLevel::Info,
        );

        loop {
            interval.tick().await;
            match run_archival_pass(&service, age_days).await {
                Ok(count) => {
                    let mut event = std::collections::HashMap::new();
                    event.insert("op".to_string(), serde_json::Value::String("lifecycle.archival.complete".to_string()));
                    event.insert(
                        "episodes_archived".to_string(),
                        serde_json::Value::Number(serde_json::Number::from(count)),
                    );
                    service.logger.log(
                        event,
                        crate::logging::LogLevel::Info,
                    );
                }
                Err(e) => {
                    let mut event = std::collections::HashMap::new();
                    event.insert("op".to_string(), serde_json::Value::String("lifecycle.archival.error".to_string()));
                    event.insert("error".to_string(), serde_json::Value::String(format!("{}", e)));
                    service.logger.log(
                        event,
                        crate::logging::LogLevel::Warn,
                    );
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
    let cutoff = now - Duration::days(age_days as i64);
    let namespace = service.default_namespace().unwrap_or("memory");

    // Fetch all episodes
    let episodes = service
        .db_client
        .select_table("episode", &namespace)
        .await?;

    let mut archived = 0;

    for record in episodes {
        // Skip already archived episodes
        if let Some(status) = record.get("status").and_then(|v| v.as_str()) {
            if status == "archived" {
                continue;
            }
        }

        // Check episode age
        let t_ref = record
            .get("t_ref")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or(now);

        if t_ref > cutoff {
            continue; // Episode is not old enough
        }

        // Check if episode has any active facts
        let episode_id = record
            .get("episode_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MemoryError::Validation("missing episode_id".into()))?;

        let has_active_facts = check_episode_has_active_facts(service, episode_id, &namespace).await?;

        if !has_active_facts {
            // Archive the episode
            let payload = json!({
                "status": "archived",
                "archived_at": crate::service::normalize_dt(now),
            });

            service
                .db_client
                .update(episode_id, payload, &namespace)
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
    namespace: &str,
) -> Result<bool, MemoryError> {
    // Query facts linked to this episode
    let sql = "SELECT * FROM fact WHERE source_episode = $episode_id AND t_invalid IS NONE";
    let result = service
        .db_client
        .execute_query(
            sql,
            Some(json!({"episode_id": episode_id})),
            namespace,
        )
        .await?;

    // Check if result array is non-empty
    let has_facts = result
        .as_array()
        .map(|arr| !arr.is_empty())
        .unwrap_or(false);

    Ok(has_facts)
}
