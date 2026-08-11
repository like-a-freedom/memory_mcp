//! Shared helpers for unwrapping SurrealDB JSON value wrappers.
//!
//! SurrealDB serializes typed values into tagged JSON objects
//! (e.g. `{"String": "foo"}`, `{"Number": 42}`). These helpers
//! provide a single place to extract primitives across the codebase.

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

/// Extract a plain `&str` from a value that may be a SurrealDB
/// `String`, `Strand`, or `Datetime` wrapper.
pub fn json_string(value: &Value) -> Option<&str> {
    if let Some(s) = value.as_str() {
        return Some(s);
    }
    let obj = value.as_object()?;
    obj.get("String")
        .and_then(Value::as_str)
        .or_else(|| obj.get("Strand").and_then(Value::as_str))
        .or_else(|| {
            obj.get("Strand")
                .and_then(|inner| inner.get("String"))
                .and_then(Value::as_str)
        })
        .or_else(|| obj.get("Datetime").and_then(Value::as_str))
        .or_else(|| {
            obj.get("Datetime")
                .and_then(|inner| inner.get("String"))
                .and_then(Value::as_str)
        })
}

/// Extract an owned `String` from a value, including `RecordId` wrappers.
pub fn string_from_value(value: &Value) -> Option<String> {
    if let Some(s) = json_string(value) {
        return Some(s.to_string());
    }
    // Handle RecordId which json_string doesn't cover
    if let Value::Object(map) = value
        && let Some(Value::Object(record_id)) = map.get("RecordId")
        && let (Some(Value::String(table)), Some(Value::String(key))) =
            (record_id.get("table"), record_id.get("key"))
    {
        return Some(format!("{table}:{key}"));
    }
    None
}

/// Extract an `f64` from a value that may be a `Number`, `Float`,
/// `Int`, `Decimal`, or a parseable string.
pub fn json_f64(value: &Value) -> Option<f64> {
    if let Some(v) = value.as_f64() {
        return Some(v);
    }
    let obj = value.as_object()?;
    obj.get("Number")
        .and_then(json_f64)
        .or_else(|| obj.get("Float").and_then(json_f64))
        .or_else(|| obj.get("Int").and_then(json_f64))
        .or_else(|| obj.get("Decimal").and_then(json_f64))
        .or_else(|| {
            obj.get("String")
                .and_then(Value::as_str)?
                .parse::<f64>()
                .ok()
        })
}

/// Extract an `i64` from a value that may be a `Number`, `Int`,
/// or a parseable string.
pub fn json_i64(value: &Value) -> Option<i64> {
    if let Some(v) = value.as_i64() {
        return Some(v);
    }
    let obj = value.as_object()?;
    obj.get("Number")
        .and_then(json_i64)
        .or_else(|| obj.get("Int").and_then(json_i64))
        .or_else(|| {
            obj.get("String")
                .and_then(Value::as_str)?
                .parse::<i64>()
                .ok()
        })
}

/// Extract an array reference from a value that may be wrapped as `Array`.
pub fn unwrap_array_value(v: &Value) -> Option<&Vec<Value>> {
    if let Some(arr) = v.as_array() {
        return Some(arr);
    }
    v.as_object()?.get("Array").and_then(Value::as_array)
}

// ---------------------------------------------------------------------------
// Convenience helpers for parsing from `serde_json::Map` directly.
// ---------------------------------------------------------------------------

/// Extract a `String` field from a JSON map.
pub fn str_field(map: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    map.get(key).and_then(json_string).map(String::from)
}

/// Extract a `DateTime<Utc>` field from a JSON map.
pub fn dt_field(map: &serde_json::Map<String, Value>, key: &str) -> Option<DateTime<Utc>> {
    map.get(key)
        .and_then(json_string)
        .and_then(|s| s.parse().ok())
}

/// Extract an `f64` field from a JSON map with a default fallback.
pub fn f64_field(map: &serde_json::Map<String, Value>, key: &str, default: f64) -> f64 {
    map.get(key).and_then(json_f64).unwrap_or(default)
}

/// Extract an `i64` field from a JSON map with a default fallback.
pub fn i64_field(map: &serde_json::Map<String, Value>, key: &str, default: i64) -> i64 {
    map.get(key).and_then(json_i64).unwrap_or(default)
}

/// Extract a `Vec<String>` field from a JSON map.
pub fn str_array_field(map: &serde_json::Map<String, Value>, key: &str) -> Vec<String> {
    map.get(key)
        .and_then(unwrap_array_value)
        .map(|values| {
            values
                .iter()
                .filter_map(json_string)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Build a compact JSON representation of an edge record.
pub fn normalized_edge_record(record: &Value) -> Value {
    let Some(map) = record.as_object() else {
        return record.clone();
    };
    json!({
        "edge_id": map
            .get("edge_id")
            .and_then(json_string)
            .or_else(|| map.get("id").and_then(json_string)),
        "in": map.get("in").and_then(json_string),
        "relation": map.get("relation").and_then(json_string),
        "out": map.get("out").and_then(json_string),
        "origin": map.get("origin").cloned().unwrap_or(Value::Null),
        "confidence": map.get("confidence").cloned().unwrap_or(Value::Null),
        "t_valid": map.get("t_valid").cloned().unwrap_or(Value::Null),
        "t_ingested": map.get("t_ingested").cloned().unwrap_or(Value::Null),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_from_value_handles_object_without_expected_keys() {
        let value = json!({"Other": "value"});
        assert_eq!(string_from_value(&value), None);
    }

    #[test]
    fn string_from_value_handles_record_id_missing_fields() {
        let value = json!({"RecordId": {"table": "entity"}});
        assert_eq!(string_from_value(&value), None);
    }

    #[test]
    fn json_string_handles_plain_string() {
        let v = json!("hello");
        assert_eq!(json_string(&v), Some("hello"));
    }

    #[test]
    fn json_string_handles_strand() {
        let v = json!({"Strand": "hello"});
        assert_eq!(json_string(&v), Some("hello"));
    }

    #[test]
    fn json_string_handles_nested_strand_string() {
        let v = json!({"Strand": {"String": "hello"}});
        assert_eq!(json_string(&v), Some("hello"));
    }

    #[test]
    fn json_string_handles_datetime() {
        let v = json!({"Datetime": "2024-01-01T00:00:00Z"});
        assert_eq!(json_string(&v), Some("2024-01-01T00:00:00Z"));
    }

    #[test]
    fn json_string_returns_none_for_number() {
        assert_eq!(json_string(&json!(42)), None);
    }

    #[test]
    fn json_f64_handles_plain_number() {
        assert_eq!(json_f64(&json!(2.5)), Some(2.5));
    }

    #[test]
    fn json_f64_handles_wrapped_number() {
        assert_eq!(json_f64(&json!({"Number": 2.5})), Some(2.5));
    }

    #[test]
    fn json_i64_handles_plain_int() {
        assert_eq!(json_i64(&json!(42)), Some(42));
    }

    #[test]
    fn json_i64_handles_wrapped_int() {
        assert_eq!(json_i64(&json!({"Number": 42})), Some(42));
    }

    #[test]
    fn str_array_field_extracts_strings() {
        let map =
            serde_json::from_str::<serde_json::Map<String, Value>>(r#"{"tags": ["a", "b", "c"]}"#)
                .unwrap();
        assert_eq!(str_array_field(&map, "tags"), vec!["a", "b", "c"]);
    }

    #[test]
    fn str_array_field_returns_empty_for_missing_key() {
        let map = serde_json::from_str::<serde_json::Map<String, Value>>(r#"{}"#).unwrap();
        assert_eq!(str_array_field(&map, "tags"), Vec::<String>::new());
    }

    #[test]
    fn dt_field_parses_datetime() {
        let map = serde_json::from_str::<serde_json::Map<String, Value>>(
            r#"{"t_valid": "2024-06-01T12:00:00Z"}"#,
        )
        .unwrap();
        let dt = dt_field(&map, "t_valid");
        assert!(dt.is_some());
    }

    #[test]
    fn dt_field_returns_none_for_invalid() {
        let map =
            serde_json::from_str::<serde_json::Map<String, Value>>(r#"{"t_valid": "not-a-date"}"#)
                .unwrap();
        assert!(dt_field(&map, "t_valid").is_none());
    }

    #[test]
    fn dt_field_returns_none_for_missing() {
        let map = serde_json::from_str::<serde_json::Map<String, Value>>(r#"{}"#).unwrap();
        assert!(dt_field(&map, "t_valid").is_none());
    }

    #[test]
    fn f64_field_returns_default_for_missing() {
        let map = serde_json::from_str::<serde_json::Map<String, Value>>(r#"{}"#).unwrap();
        assert_eq!(f64_field(&map, "confidence", 0.5), 0.5);
    }

    #[test]
    fn f64_field_parses_plain_number() {
        let map = serde_json::from_str::<serde_json::Map<String, Value>>(r#"{"confidence": 0.8}"#)
            .unwrap();
        assert_eq!(f64_field(&map, "confidence", 0.5), 0.8);
    }

    #[test]
    fn i64_field_returns_default_for_missing() {
        let map = serde_json::from_str::<serde_json::Map<String, Value>>(r#"{}"#).unwrap();
        assert_eq!(i64_field(&map, "count", 0), 0);
    }

    #[test]
    fn i64_field_parses_plain_int() {
        let map =
            serde_json::from_str::<serde_json::Map<String, Value>>(r#"{"count": 42}"#).unwrap();
        assert_eq!(i64_field(&map, "count", 0), 42);
    }

    #[test]
    fn normalized_edge_record_extracts_all_fields() {
        let record = json!({
            "edge_id": "edge:1",
            "id": "edge:alt",
            "in": "entity:alice",
            "relation": "knows",
            "out": "entity:bob",
            "origin": "extracted",
            "confidence": 0.9,
            "t_valid": "2024-01-01T00:00:00Z",
            "t_ingested": "2024-01-02T00:00:00Z",
        });
        let normalized = normalized_edge_record(&record);
        assert_eq!(normalized["edge_id"], "edge:1");
        assert_eq!(normalized["in"], "entity:alice");
        assert_eq!(normalized["relation"], "knows");
        assert_eq!(normalized["out"], "entity:bob");
        assert_eq!(normalized["origin"], "extracted");
        assert_eq!(normalized["confidence"], 0.9);
    }

    #[test]
    fn normalized_edge_record_falls_back_to_id() {
        let record = json!({
            "id": "edge:fallback",
            "in": "entity:alice",
            "relation": "knows",
            "out": "entity:bob",
        });
        let normalized = normalized_edge_record(&record);
        assert_eq!(normalized["edge_id"], "edge:fallback");
        assert!(normalized["origin"].is_null());
        assert!(normalized["confidence"].is_null());
    }

    #[test]
    fn normalized_edge_record_clones_non_object() {
        let record = json!([1, 2, 3]);
        let normalized = normalized_edge_record(&record);
        assert_eq!(normalized, json!([1, 2, 3]));
    }

    #[test]
    fn json_string_returns_none_for_non_string_object() {
        assert!(json_string(&json!({"other": "value"})).is_none());
    }

    #[test]
    fn json_f64_returns_none_for_non_numeric() {
        assert!(json_f64(&json!({"Other": 1})).is_none());
    }

    #[test]
    fn json_i64_returns_none_for_non_numeric() {
        assert!(json_i64(&json!({"Other": 1})).is_none());
    }

    #[test]
    fn unwrap_array_value_extracts_plain_array() {
        let v = json!([1, 2, 3]);
        assert!(unwrap_array_value(&v).is_some());
    }

    #[test]
    fn unwrap_array_value_returns_none_for_object() {
        assert!(unwrap_array_value(&json!({"key": "value"})).is_none());
    }

    #[test]
    fn str_field_extracts_string() {
        let map =
            serde_json::from_str::<serde_json::Map<String, Value>>(r#"{"name": "Alice"}"#).unwrap();
        assert_eq!(str_field(&map, "name"), Some("Alice".to_string()));
    }

    #[test]
    fn str_field_returns_none_for_missing() {
        let map = serde_json::from_str::<serde_json::Map<String, Value>>(r#"{}"#).unwrap();
        assert!(str_field(&map, "name").is_none());
    }
}
