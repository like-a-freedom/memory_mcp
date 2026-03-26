//! Query preprocessing and utility functions.

use chrono::{DateTime, Utc};
use regex::Regex;

/// Normalize text by lowercasing and collapsing whitespace.
pub fn normalize_text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Normalize a datetime to RFC3339 string.
pub fn normalize_dt(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339()
}

/// Parse an ISO 8601 datetime string.
pub fn parse_iso(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
}

/// Get current UTC time.
pub fn now() -> DateTime<Utc> {
    Utc::now()
}

/// Bucket cutoff to the start of the hour for better cache hit rate.
pub fn bucket_to_hour(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:00:00Z").to_string()
}

/// Preprocess a search query by stripping episode references, boolean operators,
/// quoted phrases, and collapsing whitespace.
pub fn preprocess_search_query(raw: &str) -> String {
    static EPISODE_REF: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static QUOTED: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();

    let episode_re = EPISODE_REF.get_or_init(|| {
        Regex::new(r"(?i)episode:[a-z0-9_-]+").expect("episode_ref regex is valid")
    });
    let quoted_re =
        QUOTED.get_or_init(|| Regex::new(r#""([^"]*)""#).expect("quoted regex is valid"));

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Fact;
    use chrono::{Datelike, TimeZone};
    use serde_json::json;

    #[test]
    fn normalize_text_lowercases_and_collapses_whitespace() {
        assert_eq!(normalize_text("  Hello   WORLD  "), "hello world");
        assert_eq!(normalize_text("Test"), "test");
        assert_eq!(normalize_text(""), "");
    }

    #[test]
    fn normalize_dt_formats_as_rfc3339() {
        let dt = Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap();
        let result = normalize_dt(dt);
        assert!(result.starts_with("2024-01-15T10:30:00"));
    }

    #[test]
    fn parse_iso_parses_valid_datetime() {
        let result = parse_iso("2024-01-15T10:30:00Z");
        assert!(result.is_some());
        let dt = result.unwrap();
        assert_eq!(dt.year(), 2024);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 15);
    }

    #[test]
    fn parse_iso_returns_none_for_invalid_datetime() {
        assert!(parse_iso("invalid").is_none());
        assert!(parse_iso("").is_none());
        assert!(parse_iso("2024-13-45").is_none());
    }

    #[test]
    fn bucket_to_hour_rounds_down_to_hour() {
        let dt = Utc.with_ymd_and_hms(2024, 1, 15, 10, 45, 30).unwrap();
        let result = bucket_to_hour(dt);
        assert_eq!(result, "2024-01-15T10:00:00Z");
    }

    #[test]
    fn preprocess_search_query_strips_episode_references() {
        let result = preprocess_search_query("query episode:abc123 more");
        assert_eq!(result, "query more");
    }

    #[test]
    fn preprocess_search_query_strips_boolean_operators() {
        let result = preprocess_search_query("hello OR world AND test NOT foo");
        assert_eq!(result, "hello world test foo");
    }

    #[test]
    fn preprocess_search_query_handles_quoted_phrases() {
        let result = preprocess_search_query(r#"search "quoted phrase" terms"#);
        assert!(result.contains("quoted"));
        assert!(result.contains("phrase"));
    }

    #[test]
    fn preprocess_search_query_drops_short_words() {
        let result = preprocess_search_query("a an I be to of query");
        assert_eq!(result, "an be to of query");
    }

    #[test]
    fn preprocess_search_query_case_insensitive_episode_ref() {
        let result = preprocess_search_query("test EPISODE:ABC123 query");
        assert_eq!(result, "test query");
    }

    #[test]
    fn decayed_confidence_metric_uses_longer_half_life() {
        let fact = Fact {
            fact_id: "fact:1".to_string(),
            fact_type: "metric".to_string(),
            content: "test".to_string(),
            quote: "test".to_string(),
            source_episode: "episode:1".to_string(),
            t_valid: Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap(),
            t_ingested: Utc::now(),
            t_invalid: None,
            t_invalid_ingested: None,
            confidence: 1.0,
            entity_links: vec![],
            scope: "org".to_string(),
            policy_tags: vec![],
            provenance: json!({}),
        };
        let now = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let confidence = decayed_confidence(&fact, now);
        assert!(confidence > 0.4 && confidence < 0.6);
    }

    #[test]
    fn decayed_confidence_general_uses_shorter_half_life() {
        let fact = Fact {
            fact_id: "fact:1".to_string(),
            fact_type: "note".to_string(),
            content: "test".to_string(),
            quote: "test".to_string(),
            source_episode: "episode:1".to_string(),
            t_valid: Utc.with_ymd_and_hms(2023, 7, 1, 0, 0, 0).unwrap(),
            t_ingested: Utc::now(),
            t_invalid: None,
            t_invalid_ingested: None,
            confidence: 1.0,
            entity_links: vec![],
            scope: "org".to_string(),
            policy_tags: vec![],
            provenance: json!({}),
        };
        let now = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let confidence = decayed_confidence(&fact, now);
        assert!(confidence > 0.4 && confidence < 0.6);
    }

    #[test]
    fn decayed_confidence_fresh_fact_has_high_confidence() {
        let fact = Fact {
            fact_id: "fact:1".to_string(),
            fact_type: "note".to_string(),
            content: "test".to_string(),
            quote: "test".to_string(),
            source_episode: "episode:1".to_string(),
            t_valid: Utc::now(),
            t_ingested: Utc::now(),
            t_invalid: None,
            t_invalid_ingested: None,
            confidence: 1.0,
            entity_links: vec![],
            scope: "org".to_string(),
            policy_tags: vec![],
            provenance: json!({}),
        };
        let confidence = decayed_confidence(&fact, Utc::now());
        assert!(confidence > 0.99);
    }
}
