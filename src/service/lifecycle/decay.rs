//! Confidence decay background worker.
//!
//! Periodically marks facts with decayed confidence below threshold as invalid.

use chrono::Utc;
use serde_json::json;
use tokio::time::{self, Duration as TokioDuration};

use crate::log_event;
use crate::logging::LogLevel;
use crate::service::{MemoryError, MemoryService};
use crate::storage::{json_f64, json_i64};

/// Spawns the decay worker background task.
pub fn spawn_decay_worker(
    service: MemoryService,
    interval_secs: u64,
    threshold: f64,
    half_life_days: f64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = time::interval(TokioDuration::from_secs(interval_secs));

        service.logger.log(
            log_event!(
                "lifecycle.decay.start",
                "success",
                "interval_secs" => interval_secs,
                "threshold" => threshold,
                "half_life_days" => half_life_days
            ),
            LogLevel::Info,
        );

        loop {
            interval.tick().await;
            match run_decay_pass(&service, threshold, half_life_days).await {
                Ok(count) => {
                    service.logger.log(
                        log_event!(
                            "lifecycle.decay.complete",
                            "success",
                            "facts_invalidated" => count
                        ),
                        LogLevel::Info,
                    );
                }
                Err(e) => {
                    service.logger.log(
                        log_event!("lifecycle.decay.error", "error", "error" => format!("{e}")),
                        LogLevel::Warn,
                    );
                }
            }
        }
    })
}

const DECAY_BATCH_LIMIT: i32 = 1000;

/// Runs a single decay pass, invalidating facts below threshold.
pub async fn run_decay_pass(
    service: &MemoryService,
    threshold: f64,
    half_life_days: f64,
) -> Result<usize, MemoryError> {
    let now = Utc::now();
    let mut invalidated = 0;

    for namespace in &service.namespaces {
        let facts = service
            .db_client
            .select_active_facts(namespace, DECAY_BATCH_LIMIT)
            .await?;

        for record in facts {
            if record
                .get("t_invalid")
                .is_some_and(|value| !value.is_null())
            {
                continue;
            }

            let t_valid = record
                .get("t_valid")
                .and_then(|v| v.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or(now);

            let base_confidence = record.get("confidence").and_then(json_f64).unwrap_or(0.5);
            let access_count = record.get("access_count").and_then(json_i64).unwrap_or(0);
            let last_accessed = record
                .get("last_accessed")
                .and_then(|value| value.as_str())
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                .map(|dt| dt.with_timezone(&Utc));

            let days_since_valid = (now - t_valid).num_days() as f64;
            let decay_rate = (2.0_f64).ln() / half_life_days;
            let decayed = base_confidence * (-decay_rate * days_since_valid).exp();
            let is_hot = access_count > 0
                && last_accessed.is_some_and(|last_accessed| {
                    (now - last_accessed).num_days() as f64 <= half_life_days
                });

            if decayed < threshold && !is_hot {
                let fact_id = record
                    .get("fact_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| MemoryError::Validation("missing fact_id".into()))?;

                let payload = json!({
                    "t_invalid": crate::service::normalize_dt(now),
                    "t_invalid_ingested": crate::service::normalize_dt(now),
                });

                service
                    .db_client
                    .update(fact_id, payload, namespace)
                    .await?;

                invalidated += 1;
            }
        }
    }

    Ok(invalidated)
}
