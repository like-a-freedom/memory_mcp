//! Fact record decoding.
//!
//! The fact table is knowledge's, so its row-to-domain conversion
//! belongs here rather than in the episode record parser. The helpers
//! it uses are the shared row unwrappers in `storage::value_helpers`.

use serde_json::Value;

use crate::storage::value_helpers::{dt_field, f64_field, i64_field, str_array_field, str_field};

#[must_use]
pub fn fact_from_record(record: &Value) -> Option<crate::models::Fact> {
    let map = record.as_object()?;

    let t_valid = dt_field(map, "t_valid")?;

    Some(crate::models::Fact {
        fact_id: str_field(map, "fact_id")?,
        fact_type: str_field(map, "fact_type")?,
        content: str_field(map, "content")?,
        quote: str_field(map, "quote")?,
        source_episode: str_field(map, "source_episode")?,
        t_valid,
        t_ingested: dt_field(map, "t_ingested").unwrap_or(t_valid),
        t_invalid: dt_field(map, "t_invalid"),
        t_invalid_ingested: dt_field(map, "t_invalid_ingested"),
        confidence: f64_field(map, "confidence", 0.0),
        index_keys: str_array_field(map, "index_keys"),
        access_count: i64_field(map, "access_count", 0),
        last_accessed: dt_field(map, "last_accessed"),
        entity_links: str_array_field(map, "entity_links"),
        scope: str_field(map, "scope").unwrap_or_default(),
        policy_tags: str_array_field(map, "policy_tags"),
        provenance: crate::models::Provenance::from_json_value(
            &map.get("provenance").cloned().unwrap_or(Value::Null),
        ),
        ft_score: f64_field(map, "ft_score", 0.0),
    })
}

/// Wrapper that tries direct parsing then falls back to unwrapping "Object" key.
pub fn fact_from_value_or_wrapper(value: &Value) -> Option<crate::models::Fact> {
    fact_from_record(value).or_else(|| value.get("Object").and_then(fact_from_record))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fact_from_record_parses_full_record() {
        let record = json!({
            "fact_id": "fact:test123",
            "fact_type": "note",
            "content": "Test fact",
            "quote": "Test quote",
            "source_episode": "episode:abc",
            "t_valid": "2024-01-15T10:30:00Z",
            "t_ingested": "2024-01-15T10:31:00Z",
            "t_invalid": "2024-01-16T10:30:00Z",
            "confidence": 0.95,
            "entity_links": ["entity:1", "entity:2"],
            "scope": "org",
            "policy_tags": ["tag1"],
            "provenance": {"source": "test"}
        });

        let fact = fact_from_record(&record).unwrap();
        assert_eq!(fact.fact_id, "fact:test123");
        assert_eq!(fact.fact_type, "note");
        assert_eq!(fact.content, "Test fact");
        assert_eq!(fact.quote, "Test quote");
        assert_eq!(fact.source_episode, "episode:abc");
        assert!((fact.confidence - 0.95).abs() < f64::EPSILON);
        assert_eq!(fact.entity_links, vec!["entity:1", "entity:2"]);
        assert_eq!(fact.scope, "org");
        assert_eq!(fact.policy_tags, vec!["tag1"]);
    }

    #[test]
    fn fact_from_record_handles_optional_fields() {
        let record = json!({
            "fact_id": "fact:test123",
            "fact_type": "note",
            "content": "Test",
            "quote": "Quote",
            "source_episode": "episode:abc",
            "t_valid": "2024-01-15T10:30:00Z",
            "scope": "org"
        });

        let fact = fact_from_record(&record).unwrap();
        assert!(fact.t_invalid.is_none());
        assert!(fact.t_invalid_ingested.is_none());
        assert!(fact.entity_links.is_empty());
        assert!(fact.policy_tags.is_empty());
        assert!((fact.confidence).abs() < f64::EPSILON);
    }

    #[test]
    fn fact_from_record_returns_none_for_invalid_record() {
        let record = json!({"invalid": "data"});
        assert!(fact_from_record(&record).is_none());
    }
}
