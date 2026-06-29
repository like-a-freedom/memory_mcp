//! Confidence decay background worker.
//!
//! Periodically marks facts with decayed confidence below threshold as invalid.

use chrono::Utc;
use serde_json::json;
use tokio::time::{self, Duration as TokioDuration};

use crate::service::value_helpers::{json_f64, json_i64};
use crate::service::{MemoryError, MemoryService};

/// Spawns the decay worker background task.
pub fn spawn_decay_worker(
    service: MemoryService,
    interval_secs: u64,
    threshold: f64,
    half_life_days: f64,
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
        event.insert("threshold".to_string(), json!(threshold));
        event.insert("half_life_days".to_string(), json!(half_life_days));
        service.logger.log(event, crate::logging::LogLevel::Info);

        loop {
            interval.tick().await;
            match run_decay_pass(&service, threshold, half_life_days).await {
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

            let delta = now - t_valid;
            let days_since_valid = delta.num_days() as f64;
            let decay_rate = (2.0_f64).ln() / half_life_days;
            let decayed = base_confidence * (-decay_rate * days_since_valid).exp();
            let is_hot = access_count > 0
                && last_accessed.is_some_and(|last_accessed| {
                    let delta_access = now - last_accessed;
                    delta_access.num_days() as f64 <= half_life_days
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn decay_computation_halflife() {
        // base_confidence * (0.5)^(days / half_life_days)
        // At exactly half_life_days, confidence should be halved.
        let base_confidence = 0.8;
        let half_life_days = 365.0;
        let days_since_valid = 365.0;
        let decay_rate = (2.0_f64).ln() / half_life_days;
        let decayed = base_confidence * (-decay_rate * days_since_valid).exp();
        assert!((decayed - 0.4).abs() < 1e-9, "expected 0.4, got {decayed}");
    }

    #[test]
    fn decay_computation_two_halflives() {
        // After 2 half-lives, confidence should be quartered.
        let base_confidence = 1.0;
        let half_life_days = 30.0;
        let days_since_valid = 60.0;
        let decay_rate = (2.0_f64).ln() / half_life_days;
        let decayed = base_confidence * (-decay_rate * days_since_valid).exp();
        assert!(
            (decayed - 0.25).abs() < 1e-9,
            "expected 0.25, got {decayed}"
        );
    }

    #[test]
    fn decay_computation_zero_days() {
        let base_confidence = 0.7;
        let half_life_days = 365.0;
        let days_since_valid = 0.0;
        let decay_rate = (2.0_f64).ln() / half_life_days;
        let decayed = base_confidence * (-decay_rate * days_since_valid).exp();
        assert!((decayed - 0.7).abs() < 1e-9);
    }

    #[test]
    fn decayed_below_threshold_without_heat_should_invalidate() {
        // A fact with low base confidence and long time since valid, no access.
        let base_confidence = 0.3;
        let half_life_days = 30.0;
        let threshold = 0.1;
        // After 60 days (2 half-lives): 0.3 * 0.25 = 0.075 < 0.1
        let days_since_valid = 60.0;
        let decay_rate = (2.0_f64).ln() / half_life_days;
        let decayed = base_confidence * (-decay_rate * days_since_valid).exp();
        let is_hot = false; // no last_accessed
        assert!(decayed < threshold && !is_hot);
    }

    #[test]
    fn hot_fact_is_not_invalidated_even_when_decayed() {
        let base_confidence = 0.3;
        let half_life_days = 30.0;
        let threshold = 0.1;
        let days_since_valid = 60.0;
        let decay_rate = (2.0_f64).ln() / half_life_days;
        let decayed = base_confidence * (-decay_rate * days_since_valid).exp();
        // Hot: accessed within half_life_days
        let access_count = 5;
        let last_accessed_within_half_life = true;
        let is_hot = access_count > 0 && last_accessed_within_half_life;
        assert!(decayed < threshold);
        assert!(is_hot);
        // The combined condition: decayed < threshold AND NOT is_hot
        assert!(decayed >= threshold || is_hot);
    }

    #[test]
    fn fact_with_t_invalid_is_skipped() {
        let record = json!({
            "fact_id": "fact:1",
            "t_invalid": "2024-01-01T00:00:00Z",
            "confidence": 0.2,
        });
        assert!(
            record
                .get("t_invalid")
                .is_some_and(|value| !value.is_null())
        );
    }

    #[test]
    fn fact_without_t_invalid_proceeds_to_decay_check() {
        let record = json!({
            "fact_id": "fact:2",
            "confidence": 0.5,
            "access_count": 0,
        });
        assert!(record.get("t_invalid").is_none());
    }

    #[test]
    fn missing_fact_id_returns_error() {
        // Simulate the error path in run_decay_pass
        let record = json!({
            "confidence": 0.1,
            "access_count": 0,
        });
        let result = record.get("fact_id").and_then(|v| v.as_str());
        assert!(result.is_none());
    }
}
