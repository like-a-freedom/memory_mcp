//! Confidence decay background worker.
//!
//! Periodically marks facts with decayed confidence below threshold as invalid.

use chrono::Utc;
use serde_json::json;
use tokio::time::{self, Duration as TokioDuration};

use crate::service::{MemoryError, MemoryService};
use crate::storage::json_f64;

/// Spawns the decay worker background task.
pub fn spawn_decay_worker(
    service: MemoryService,
    interval_secs: u64,
    threshold: f64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = time::interval(TokioDuration::from_secs(interval_secs));

        let mut event = std::collections::HashMap::new();
        event.insert(
            "op".to_string(),
            serde_json::Value::String("lifecycle.decay.start".to_string()),
        );
        event.insert(
            "interval_secs".to_string(),
            serde_json::Value::Number(serde_json::Number::from(interval_secs)),
        );
        event.insert(
            "threshold".to_string(),
            serde_json::Value::Number(serde_json::Number::from(threshold as i64)),
        );
        service.logger.log(event, crate::logging::LogLevel::Info);

        loop {
            interval.tick().await;
            match run_decay_pass(&service, threshold).await {
                Ok(count) => {
                    let mut event = std::collections::HashMap::new();
                    event.insert(
                        "op".to_string(),
                        serde_json::Value::String("lifecycle.decay.complete".to_string()),
                    );
                    event.insert(
                        "facts_invalidated".to_string(),
                        serde_json::Value::Number(serde_json::Number::from(count)),
                    );
                    service.logger.log(event, crate::logging::LogLevel::Info);
                }
                Err(e) => {
                    let mut event = std::collections::HashMap::new();
                    event.insert(
                        "op".to_string(),
                        serde_json::Value::String("lifecycle.decay.error".to_string()),
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

/// Runs a single decay pass, invalidating facts below threshold.
pub async fn run_decay_pass(service: &MemoryService, threshold: f64) -> Result<usize, MemoryError> {
    let now = Utc::now();
    let namespace = service.default_namespace.clone();

    // Fetch all active facts
    let facts = service.db_client.select_table("fact", &namespace).await?;

    let mut invalidated = 0;

    for record in facts {
        // Skip already invalidated facts
        if record.get("t_invalid").is_some() {
            continue;
        }

        // Compute decayed confidence
        let t_valid = record
            .get("t_valid")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or(now);

        let base_confidence = record.get("confidence").and_then(json_f64).unwrap_or(0.5);

        // Compute decayed confidence using standard decay formula
        let days_since_valid = (now - t_valid).num_days() as f64;
        let decay_rate = 0.693 / 365.0; // ln(2) / half-life (1 year)
        let decayed = base_confidence * (-decay_rate * days_since_valid).exp();

        if decayed < threshold {
            // Mark as invalid
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
                .update(fact_id, payload, &namespace)
                .await?;

            invalidated += 1;
        }
    }

    Ok(invalidated)
}
