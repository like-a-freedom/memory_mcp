//! Query preprocessing and utility functions.

use chrono::{DateTime, Utc};
use regex::Regex;

/// Normalize text by lowercasing and collapsing whitespace.
#[must_use]
pub fn normalize_text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Normalize a datetime to RFC3339 string.
#[must_use]
pub fn normalize_dt(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339()
}

/// Parse an ISO 8601 datetime string.
#[must_use]
pub fn parse_iso(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
}

/// Get current UTC time.
#[must_use]
pub fn now() -> DateTime<Utc> {
    Utc::now()
}

/// Bucket cutoff to the start of the hour for better cache hit rate.
#[must_use]
pub fn bucket_to_hour(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:00:00Z").to_string()
}

/// Preprocess a search query by stripping episode references, boolean operators,
/// quoted phrases, and collapsing whitespace.
#[must_use]
pub fn preprocess_search_query(raw: &str) -> String {
    static EPISODE_REF: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static QUOTED: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();

    let episode_re = EPISODE_REF.get_or_init(|| {
        Regex::new(r"(?i)episode:[a-z0-9_-]+").expect("episode_ref regex is valid")
    });
    let quoted_re = QUOTED.get_or_init(|| Regex::new(r#""([^"]*)""#).expect("quoted regex is valid"));

    let s = episode_re.replace_all(raw, " ");
    let s = quoted_re.replace_all(&s, " $1 ");

    s.split_whitespace()
        .filter(|w| {
            let upper = w.to_uppercase();
            upper != "OR" && upper != "AND" && upper != "NOT" && w.len() >= 2
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Calculate decayed confidence based on fact age.
#[must_use]
pub fn decayed_confidence(fact: &crate::models::Fact, now: DateTime<Utc>) -> f64 {
    let half_life_days = if fact.fact_type == "metric" || fact.fact_type == "promise" {
        super::METRIC_HALF_LIFE_DAYS
    } else {
        super::DEFAULT_HALF_LIFE_DAYS
    };
    let delta_days = (now - fact.t_valid).num_days().max(0) as f64;
    let decay = 0.5_f64.powf(delta_days / half_life_days);
    (fact.confidence * decay * super::CONFIDENCE_SCALE).round() / super::CONFIDENCE_SCALE
}
