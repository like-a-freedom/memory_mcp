//! Query preprocessing and utility functions.

use chrono::{DateTime, TimeZone, Utc};

mod search;
mod time;

pub use search::{
    normalize_text, preprocess_search_query, query_hard_anchor_terms, query_term_rarity_weight,
    query_term_should_be_soft_anchor, search_query_terms, unique_query_terms,
};
use crate::models::Fact;

pub use time::{bucket_to_five_minutes, bucket_to_hour, normalize_dt, now, parse_iso};

/// Calculate decayed confidence based on fact age.
/// Delegates to [`crate::models::Fact::decayed_confidence`] (single source of truth).
pub fn decayed_confidence(fact: &Fact, now: DateTime<Utc>) -> f64 {
    fact.decayed_confidence(now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Fact;
    use chrono::{TimeZone, Utc};
    use serde_json::json;

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
            index_keys: vec![],
            access_count: 0,
            last_accessed: None,
            entity_links: vec![],
            scope: "org".to_string(),
            policy_tags: vec![],
            provenance: json!({}),
            ft_score: 0.0,
        };
        let now = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let confidence = decayed_confidence(&fact, now);
        assert!(confidence > 0.4 && confidence < 0.6);
    }

    #[test]
    fn decayed_confidence_decision_uses_longer_half_life() {
        let fact = Fact {
            fact_id: "fact:1".to_string(),
            fact_type: "decision".to_string(),
            content: "test decision".to_string(),
            quote: "test decision".to_string(),
            source_episode: "episode:1".to_string(),
            t_valid: Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap(),
            t_ingested: Utc::now(),
            t_invalid: None,
            t_invalid_ingested: None,
            confidence: 1.0,
            index_keys: vec![],
            access_count: 0,
            last_accessed: None,
            entity_links: vec![],
            scope: "org".to_string(),
            policy_tags: vec![],
            provenance: json!({}),
            ft_score: 0.0,
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
            index_keys: vec![],
            access_count: 0,
            last_accessed: None,
            entity_links: vec![],
            scope: "org".to_string(),
            policy_tags: vec![],
            provenance: json!({}),
            ft_score: 0.0,
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
            index_keys: vec![],
            access_count: 0,
            last_accessed: None,
            entity_links: vec![],
            scope: "org".to_string(),
            policy_tags: vec![],
            provenance: json!({}),
            ft_score: 0.0,
        };
        let confidence = decayed_confidence(&fact, Utc::now());
        assert!(confidence > 0.99);
    }
}
