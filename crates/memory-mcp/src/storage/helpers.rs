//! Helper utilities for storage operations.

use std::path::Path;

use regex::Regex;
use serde_json::Value;
use surrealdb::types::Value as SurrealValue;

use crate::error::MemoryError;

pub fn normalize_url(url: &str) -> String {
    let trimmed = url.trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return trimmed.to_string();
    };
    let normalized = format!("{}://{rest}", scheme.to_ascii_lowercase());

    if normalized.starts_with("http://") {
        let base = normalized.replacen("http://", "ws://", 1);
        if base.ends_with("/rpc") {
            return base;
        }
        return format!("{}/rpc", base.trim_end_matches('/'));
    }
    if normalized.starts_with("https://") {
        let base = normalized.replacen("https://", "wss://", 1);
        if base.ends_with("/rpc") {
            return base;
        }
        return format!("{}/rpc", base.trim_end_matches('/'));
    }
    normalized
}

pub fn is_missing_table_error(message: &str) -> bool {
    let lowered = message.to_lowercase();
    lowered.contains("does not exist") && lowered.contains("table")
}

#[cfg(test)]
fn is_table_already_exists_error(message: &str) -> bool {
    let lowered = message.to_lowercase();
    lowered.contains("already exists") && lowered.contains("table")
}

/// Detects "index ... does not exist" errors from SurrealDB.
pub fn is_missing_index_error(message: &str) -> bool {
    let lowered = message.to_lowercase();
    lowered.contains("does not exist") && lowered.contains("index")
}

/// A single-record read result: the record body, if it exists.
///
/// The lookup is always against the process-bound Active Namespace,
/// so a caller never needs the namespace echoed back; the previous
/// `(record, namespace)` tuple carried a half that every caller
/// discarded.
pub type RecordLookup = Option<serde_json::Map<String, serde_json::Value>>;

/// Normalise an owner-scoped single-record read into the record
/// body the provenance helpers expect.
///
/// The owner-scoped accessors (`select_episode`, `select_fact`)
/// return a JSON value; the provenance helpers work on the object
/// form, so this is the one place the conversion happens. It sits
/// with [`RecordLookup`] and [`require_record_kind`] because all
/// three describe the same owner-scoped read discipline.
pub fn owner_scoped_read(
    read: Result<Option<serde_json::Value>, crate::error::MemoryError>,
) -> Result<RecordLookup, crate::error::MemoryError> {
    Ok(read?.and_then(|value| value.as_object().cloned()))
}

/// Require that a record id names a record of exactly `table`.
///
/// The low-level accessors derive their target table from the
/// record-id string, so an owner-scoped accessor must check the
/// prefix itself: without this, a `fact:…` id passed to the
/// episode accessor reads the wrong aggregate. Every owner-scoped
/// read and write goes through this, so the rule has one
/// definition rather than one per store.
///
/// This layer knows which table the caller wanted, which the
/// table-generic validator does not, so it owns the two messages
/// that have to name a correction the caller can act on: the
/// stripped-prefix case, and the wrong-kind case. Both errors are
/// `Validation` because a record id is caller input, and callers
/// that absorb a `Validation` (provenance assembly does) still see
/// the same "no such record" outcome they saw before.
pub fn require_record_kind(record_id: &str, table: &str) -> Result<(), crate::error::MemoryError> {
    let trimmed = record_id.trim();

    // A missing table prefix is a distinct, recoverable mistake from a
    // malformed id, and it is the one an agent makes routinely when it
    // re-types an id a tool returned. Naming the exact string to send
    // back is the whole point: the caller must be able to correct the
    // call without inferring the table from the field name.
    if !trimmed.is_empty() && !trimmed.contains(':') {
        return Err(crate::error::MemoryError::Validation(format!(
            "record_id '{trimmed}' is missing its table prefix; pass '{table}:{trimmed}' \
             exactly as returned, without stripping the prefix"
        )));
    }

    super::queries::validate_record_id(record_id)?;
    let actual = trimmed
        .split_once(':')
        .map(|(kind, _)| kind)
        .unwrap_or_default();
    if actual != table {
        return Err(crate::error::MemoryError::Validation(format!(
            "record_id '{record_id}' names a {actual}, not {} {table}; \
             this accessor only accepts '{table}:<id>'",
            indefinite_article(table)
        )));
    }
    Ok(())
}

/// "a" or "an" for the table name, so the wrong-kind message reads as
/// English instead of "not a episode".
fn indefinite_article(table: &str) -> &'static str {
    match table.chars().next() {
        Some(first) if "aeiou".contains(first) => "an",
        _ => "a",
    }
}

pub fn surreal_to_json(value: SurrealValue) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// Reconstructs a `table:key` record-id string from a serialized SurrealDB
/// record id.
///
/// `surreal_to_json` renders record ids as `{"RecordId": {"table": ..., "key":
/// ...}}`; this is the inverse used when a query returns `id` columns.
pub fn record_id_from_json_value(value: &Value) -> Option<String> {
    let map = value.as_object()?;
    let record_id = map.get("RecordId").and_then(|v| v.as_object())?;
    let table = record_id.get("table").and_then(|v| v.as_str())?;
    let key = record_id.get("key").and_then(|v| v.as_str())?;
    Some(format!("{table}:{key}"))
}

pub fn extract_first_record(value: Value) -> Option<Value> {
    extract_records(value).into_iter().next()
}

pub fn extract_records(value: Value) -> Vec<Value> {
    match value {
        Value::Array(arr) => arr.into_iter().map(unwrap_object_wrapper).collect(),
        Value::Object(mut map) => {
            if let Some(array) = map.remove("Array") {
                return array
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(unwrap_object_wrapper)
                    .collect();
            }
            if let Some(object) = map.remove("Object") {
                return vec![normalize_surreal_json(&object)];
            }
            vec![normalize_surreal_json(&Value::Object(map))]
        }
        Value::Null => Vec::new(),
        other => vec![normalize_surreal_json(&other)],
    }
}

fn unwrap_object_wrapper(value: Value) -> Value {
    match value {
        Value::Object(mut map) => {
            if let Some(object) = map.remove("Object") {
                normalize_surreal_json(&object)
            } else {
                normalize_surreal_json(&Value::Object(map))
            }
        }
        other => normalize_surreal_json(&other),
    }
}

/// Unwrap SurrealDB's tagged JSON representation into plain values.
///
/// SurrealDB serialises `None`, `Strand` and `Decimal` as single-key objects
/// (`{"None": {}}`, `{"Strand": {"String": "…"}}`). Code that reads a record
/// back has to see the value itself, not the tag. This lives in one place
/// because a partial fix here is worse than no fix: the two copies were
/// byte-identical, and a caller routed to one while another was routed to the
/// other would get different values for the same stored record.
pub(crate) fn normalize_surreal_json(v: &Value) -> Value {
    use serde_json::Value as J;

    match v {
        J::Object(map) if map.len() == 1 => {
            let Some((k, val)) = map.iter().next() else {
                return J::Object(map.clone());
            };
            match k.as_str() {
                "None" => v.clone(),
                "Array" => val
                    .as_array()
                    .map(|items| J::Array(items.iter().map(normalize_surreal_json).collect()))
                    .unwrap_or_else(|| val.clone()),
                "Object" => val
                    .as_object()
                    .map(|inner| {
                        J::Object(
                            inner
                                .iter()
                                .map(|(ik, iv)| (ik.clone(), normalize_surreal_json(iv)))
                                .collect(),
                        )
                    })
                    .unwrap_or_else(|| val.clone()),
                "Strand" | "String" => val
                    .as_object()
                    .and_then(|inner| inner.get("String").cloned())
                    .unwrap_or_else(|| val.clone()),
                "Datetime" => val
                    .as_object()
                    .and_then(|inner| inner.get("String").cloned())
                    .unwrap_or_else(|| val.clone()),
                "Number" | "Float" | "Int" | "Decimal" => normalize_surreal_json(val),
                _ => J::Object(
                    map.iter()
                        .map(|(ik, iv)| (ik.clone(), normalize_surreal_json(iv)))
                        .collect(),
                ),
            }
        }
        J::Object(map) => J::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), normalize_surreal_json(v)))
                .collect(),
        ),
        J::Null => J::Null,
        J::Array(arr) => J::Array(arr.iter().map(normalize_surreal_json).collect()),
        _ => v.clone(),
    }
}

/// Try to find a version-like field inside arbitrary JSON returned by the
/// server info query. Searches keys for the substring "version" (case-ins).
pub fn find_version_in_json(v: &Value) -> Option<String> {
    use std::sync::LazyLock;

    static VERSION_RE: LazyLock<Result<Regex, regex::Error>> =
        LazyLock::new(|| Regex::new(r"\d+\.\d+(?:\.\d+)?"));

    match v {
        Value::String(s) => {
            let version_match = VERSION_RE.as_ref().is_ok_and(|regex| regex.is_match(s));
            if version_match || s.to_lowercase().contains("surreal") {
                Some(s.clone())
            } else {
                None
            }
        }
        Value::Object(map) => {
            for (k, val) in map.iter() {
                if k.to_lowercase().contains("version") {
                    if let Some(s) = val.as_str() {
                        return Some(s.to_string());
                    } else if let Some(found) = find_version_in_json(val) {
                        return Some(found);
                    } else {
                        return Some(val.to_string());
                    }
                }
            }
            for (_, val) in map.iter() {
                if let Some(found) = find_version_in_json(val) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(arr) => {
            for it in arr.iter() {
                if let Some(found) = find_version_in_json(it) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

pub fn ensure_dir_exists(path: &Path) -> Result<(), MemoryError> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .map_err(|err| MemoryError::Storage(format!("failed to create data dir: {err}")))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_table_already_exists_error_detects_correct_message() {
        assert!(is_table_already_exists_error(
            "The table 'episode' already exists"
        ));
        assert!(is_table_already_exists_error("table already exists"));
        assert!(is_table_already_exists_error("TABLE ALREADY EXISTS"));
    }

    #[test]
    fn test_is_table_already_exists_error_rejects_wrong_message() {
        assert!(!is_table_already_exists_error(
            "The table 'episode' does not exist"
        ));
        assert!(!is_table_already_exists_error("already exists"));
        assert!(!is_table_already_exists_error("table created"));
    }

    #[test]
    fn require_record_kind_names_the_exact_id_to_pass_back_for_a_stripped_prefix() {
        // A caller that strips the table prefix is the most common
        // record-id mistake, and the message is the only thing that can
        // get them to retry correctly. It must name the exact id to use.
        let bare = "d1a2438bcfb3380ffb913ec4";
        let err = require_record_kind(bare, "episode").unwrap_err();
        match err {
            MemoryError::Validation(msg) => {
                assert!(
                    msg.contains(&format!("episode:{bare}")),
                    "message must name the exact id to pass back, got: {msg}"
                );
                assert!(
                    msg.contains("episode:") && msg.contains("prefix"),
                    "message must explain the missing prefix, got: {msg}"
                );
                assert!(
                    msg.contains(bare),
                    "message must echo the bad input, got: {msg}"
                );
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn require_record_kind_names_the_expected_table_for_a_wrong_kind() {
        // Cross-kind refusal must say which kind was expected, so the
        // caller can correct the prefix without guessing. This is the
        // `fact:`-prefixed-episode-id case.
        let err = require_record_kind("fact:52f9d92d20d829840f24294f", "episode").unwrap_err();
        match err {
            MemoryError::Validation(msg) => {
                assert!(
                    msg.contains("fact") && msg.contains("episode"),
                    "message must name both the actual and expected kind, got: {msg}"
                );
                assert!(
                    !msg.contains("not a episode"),
                    "message must not use the 'a episode' article, got: {msg}"
                );
                assert!(
                    msg.contains("52f9d92d20d829840f24294f"),
                    "message must echo the bad input, got: {msg}"
                );
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn require_record_kind_accepts_the_exact_id_it_names() {
        // The control: whatever the message tells the caller to send must
        // actually be accepted. A message that names an id the validator
        // then refuses would send the caller into a loop.
        let bare = "d1a2438bcfb3380ffb913ec4";
        assert!(
            require_record_kind(&format!("episode:{bare}"), "episode").is_ok(),
            "the id named in the error message must be accepted"
        );
    }

    #[test]
    fn require_record_kind_still_rejects_an_empty_id_part() {
        // Tightening the message must not loosen the rule.
        let err = require_record_kind("episode:", "episode").unwrap_err();
        assert!(matches!(err, MemoryError::Validation(_)));
    }

    #[test]
    fn record_id_from_json_value_reconstructs_table_key() {
        let value = serde_json::json!({"RecordId": {"table": "triple", "key": "abc123"}});
        assert_eq!(
            record_id_from_json_value(&value).as_deref(),
            Some("triple:abc123")
        );
    }

    #[test]
    fn record_id_from_json_value_rejects_non_record_values() {
        assert_eq!(
            record_id_from_json_value(&serde_json::json!("triple:x")),
            None
        );
        assert_eq!(
            record_id_from_json_value(&serde_json::json!({"RecordId": {"table": "triple"}})),
            None
        );
    }

    #[test]
    fn normalize_url_trims_and_normalizes_schemes() {
        assert_eq!(
            normalize_url("  HTTPS://db.example.com  "),
            "wss://db.example.com/rpc"
        );
        assert_eq!(
            normalize_url("  WS://db.example.com/rpc  "),
            "ws://db.example.com/rpc"
        );
        assert_eq!(
            normalize_url("  WSS://db.example.com  "),
            "wss://db.example.com"
        );
    }
}
