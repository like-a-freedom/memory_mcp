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
