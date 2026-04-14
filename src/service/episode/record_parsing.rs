use serde_json::Value;

use crate::models::Episode;
use crate::service::query::parse_iso;
use crate::service::value_helpers::{
    dt_field, f64_field, i64_field, json_string, str_array_field, str_field, unwrap_array_value,
};

pub(crate) fn unwrap_record_string(value: &Value) -> Option<String> {
    if let Some(s) = value.as_str() {
        Some(s.to_string())
    } else if let Some(obj) = value.as_object() {
        obj.get("String")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .or_else(|| {
                obj.get("Datetime")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .or_else(|| {
                obj.get("Strand")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .or_else(|| {
                obj.get("Strand")
                    .and_then(|inner| inner.get("String"))
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .or_else(|| {
                obj.get("Datetime")
                    .and_then(|inner| inner.get("String"))
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .or_else(|| {
                obj.get("RecordId").and_then(|record_id| {
                    let record_id = record_id.as_object()?;
                    let table = record_id.get("table")?.as_str()?;
                    let key = record_id.get("key")?.as_str()?;
                    Some(format!("{table}:{key}"))
                })
            })
    } else {
        None
    }
}

/// Parse an episode from a database record.
#[must_use]
pub fn episode_from_record(record: &serde_json::Map<String, Value>) -> Option<Episode> {
    Some(Episode {
        episode_id: json_string(record.get("episode_id")?)?.to_string(),
        source_type: json_string(record.get("source_type")?)?.to_string(),
        source_id: json_string(record.get("source_id")?)?.to_string(),
        content: json_string(record.get("content")?)?.to_string(),
        t_ref: parse_iso(json_string(record.get("t_ref")?)?)?,
        t_ingested: parse_iso(json_string(record.get("t_ingested")?)?)?,
        scope: json_string(record.get("scope")?)?.to_string(),
        visibility_scope: record
            .get("visibility_scope")
            .and_then(json_string)
            .unwrap_or_default()
            .to_string(),
        policy_tags: record
            .get("policy_tags")
            .and_then(unwrap_array_value)
            .map(|values| {
                values
                    .iter()
                    .filter_map(json_string)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// Parse a fact from a database record.
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
        provenance: map.get("provenance").cloned().unwrap_or(Value::Null),
        ft_score: f64_field(map, "ft_score", 0.0),
    })
}

/// Wrapper that tries direct parsing then falls back to unwrapping "Object" key.
pub fn fact_from_value_or_wrapper(value: &Value) -> Option<crate::models::Fact> {
    fact_from_record(value).or_else(|| value.get("Object").and_then(fact_from_record))
}

/// Check if a fact is active at a given cutoff time.
pub fn fact_is_active(fact: &crate::models::Fact, cutoff: chrono::DateTime<chrono::Utc>) -> bool {
    if fact.t_valid > cutoff || fact.t_ingested > cutoff {
        return false;
    }

    match (fact.t_invalid, fact.t_invalid_ingested) {
        (None, _) => true,
        (Some(invalidated_at), _) if invalidated_at > cutoff => true,
        (_, Some(invalidated_ingested_at)) if invalidated_ingested_at > cutoff => true,
        _ => false,
    }
}
