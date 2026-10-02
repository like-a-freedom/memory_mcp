//! Query builders for SurrealDB operations.

use serde_json::{Value, json};

use super::helpers::normalize_surreal_json;

const ACTIVE_EDGE_SCAN_BATCH_SIZE: i32 = 10_000;
const FACT_EMBEDDING_DIMENSION_PLACEHOLDER: &str = "__FACT_EMBEDDING_DIMENSION__";

// The SQL for a domain lives in the context that owns that domain:
// `knowledge/src/queries.rs` for fact, edge, community and triple;
// `memory/src/queries.rs` for episode. What stays here is table-generic —
// `select_one`, `create`, `update`, `upsert`, and the record-id and
// scan-size helpers — which is genuinely the platform's job, because it is
// the only part that has no domain in it.

pub fn active_edge_scan_batch_size() -> i32 {
    ACTIVE_EDGE_SCAN_BATCH_SIZE
}

pub fn fact_embedding_dimension_placeholder() -> &'static str {
    FACT_EMBEDDING_DIMENSION_PLACEHOLDER
}

/// Validate a record-id string before it reaches the query builder.
///
/// Accepts either a canonical `<table>:<id>` form (lowercase table, non-empty id)
/// or a plain table name (for callers that target an entire table, e.g. `select_table`).
///
/// Rejects bare hex strings (the bug masked by `build_select_one_query`'s safe noop),
/// empty input, malformed `<table>:<id>` (empty parts), and uppercase tables.
pub fn validate_record_id(record_id: &str) -> Result<(), crate::error::MemoryError> {
    use crate::error::MemoryError;

    let record_id = record_id.trim();

    if record_id.is_empty() {
        return Err(MemoryError::Validation(
            "record_id is empty; expected '<table>:<id>'".to_string(),
        ));
    }

    if let Some(idx) = record_id.find(':') {
        let table = &record_id[..idx];
        let id = &record_id[idx + 1..];

        if table.is_empty() {
            return Err(MemoryError::Validation(format!(
                "record_id has empty table part; expected '<table>:<id>', got ':{id}'"
            )));
        }
        if id.is_empty() {
            return Err(MemoryError::Validation(format!(
                "record_id has empty id part; expected '<table>:<id>', got '{table}:'"
            )));
        }
        if !is_valid_table_name(table) {
            return Err(MemoryError::Validation(format!(
                "record_id table must be lowercase ascii or underscore, got '{table}:{id}'; expected '<table>:<id>'"
            )));
        }
        Ok(())
    } else if is_valid_table_name(record_id) {
        Ok(())
    } else {
        Err(MemoryError::Validation(format!(
            "record_id '{record_id}' is not a valid table name (no ':' separator); expected '<table>:<id>'"
        )))
    }
}

/// Assemble the `SET` half of an update from a record map.
///
/// `temporal_fields` is passed in rather than looked up, and that is the point
/// of the split: which columns are temporal is a property of the table, and the
/// table belongs to a bounded context. The caller — the context — names them;
/// this function only knows how to turn a map plus a list into assignments.
pub(crate) fn build_set_assignments(
    temporal_fields: &[&str],
    map: serde_json::Map<String, Value>,
) -> (Vec<String>, serde_json::Map<String, Value>) {
    let mut entries: Vec<(String, Value)> = map.into_iter().collect();
    entries.sort_by(|(left, _), (right, _)| left.cmp(right));

    let mut assignments = Vec::with_capacity(entries.len());
    let mut vars = serde_json::Map::new();

    for (key, value) in entries {
        match value {
            Value::Null => assignments.push(format!("{key} = NONE")),
            Value::String(raw) if temporal_fields.contains(&key.as_str()) => {
                vars.insert(key.clone(), Value::String(raw));
                assignments.push(format!("{key} = type::datetime(${key})"));
            }
            other => {
                vars.insert(key.clone(), other);
                assignments.push(format!("{key} = ${key}"));
            }
        }
    }

    (assignments, vars)
}

pub fn build_select_one_query(record_id: &str) -> (String, Option<Value>) {
    let record_id = record_id.trim();
    if record_id.is_empty() {
        return ("SELECT * FROM none WHERE false".to_string(), None);
    }
    if let Some(idx) = record_id.find(':') {
        let table = &record_id[..idx];
        let id = &record_id[idx + 1..];
        if !table.trim().is_empty() && !id.trim().is_empty() {
            (format!("SELECT * FROM {table}:⟨{id}⟩"), None)
        } else {
            ("SELECT * FROM none WHERE false".to_string(), None)
        }
    } else if is_valid_table_name(record_id) {
        (format!("SELECT * FROM {record_id}"), None)
    } else {
        ("SELECT * FROM none WHERE false".to_string(), None)
    }
}

fn is_valid_table_name(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    s.chars().all(|c| c.is_ascii_lowercase() || c == '_')
}

/// Build SQL query for creating a record.
pub fn build_create_query(
    record_id: &str,
    content: Value,
    temporal_fields: &[&str],
) -> (String, Value) {
    let (table, id) = if let Some(idx) = record_id.find(':') {
        (&record_id[..idx], Some(&record_id[idx + 1..]))
    } else {
        (record_id, None)
    };

    let target = if let Some(record_id) = id {
        format!("{table}:⟨{record_id}⟩")
    } else {
        table.to_string()
    };

    let normalized = normalize_surreal_json(&content);
    if let Value::Object(map) = normalized {
        let (assignments, vars) = build_set_assignments(temporal_fields, map);
        let sql = if assignments.is_empty() {
            format!("CREATE {target} RETURN *")
        } else {
            format!("CREATE {target} SET {} RETURN *", assignments.join(", "))
        };
        (sql, Value::Object(vars))
    } else {
        (
            format!("CREATE {target} CONTENT $content RETURN *"),
            json!({"content": normalized}),
        )
    }
}

/// Build SQL query for updating a record.
pub fn build_update_query(
    record_id: &str,
    content: Value,
    temporal_fields: &[&str],
) -> Result<(String, Value), crate::error::MemoryError> {
    use crate::error::MemoryError;

    let (table, id) = if let Some(idx) = record_id.find(':') {
        (&record_id[..idx], &record_id[idx + 1..])
    } else {
        return Err(MemoryError::Storage(format!(
            "Invalid record_id format: expected 'table:id', got '{record_id}'"
        )));
    };

    let content_for_update = if let Value::Object(mut map) = content {
        map.remove("id");
        Value::Object(map)
    } else {
        content
    };

    let normalized = normalize_surreal_json(&content_for_update);
    if let Value::Object(map) = normalized {
        let (assignments, vars) = build_set_assignments(temporal_fields, map);
        let sql = if assignments.is_empty() {
            format!("UPDATE {table}:⟨{id}⟩ RETURN *")
        } else {
            format!(
                "UPDATE {table}:⟨{id}⟩ SET {} RETURN *",
                assignments.join(", ")
            )
        };
        Ok((sql, Value::Object(vars)))
    } else {
        let sql = format!("UPDATE {table}:⟨{id}⟩ MERGE $content RETURN *");
        Ok((sql, json!({"content": normalized})))
    }
}

/// Build SQL query for upserting (insert-or-update) a record.
///
/// SurrealDB's `UPDATE` does not create a record that does not exist;
/// `UPSERT` with a record ID inserts the record or replaces its fields,
/// which is the idempotent write used by deterministic job records.
/// Build the `UPSERT` for a record the caller names.
///
/// `temporal_fields` is passed in because which columns are temporal is a
/// property of the table, and the table belongs to a bounded context. It used
/// to be a switch over every table in the schema, inside the platform — which
/// is what `the_storage_platform_names_no_domain_table` in
/// `tests/knowledge_read_scopes.rs` reports, and what cost this function the
/// ability to serve a table the switch had not heard of.
pub fn build_upsert_query(
    record_id: &str,
    content: Value,
    temporal_fields: &[&str],
) -> Result<(String, Value), crate::error::MemoryError> {
    use crate::error::MemoryError;

    let (table, id) = if let Some(idx) = record_id.find(':') {
        (&record_id[..idx], &record_id[idx + 1..])
    } else {
        return Err(MemoryError::Storage(format!(
            "Invalid record_id format: expected 'table:id', got '{record_id}'"
        )));
    };

    let content_for_upsert = if let Value::Object(mut map) = content {
        map.remove("id");
        Value::Object(map)
    } else {
        content
    };

    let normalized = normalize_surreal_json(&content_for_upsert);
    if let Value::Object(map) = normalized {
        let (assignments, vars) = build_set_assignments(temporal_fields, map);
        let sql = if assignments.is_empty() {
            format!("UPSERT {table}:⟨{id}⟩ RETURN *")
        } else {
            format!(
                "UPSERT {table}:⟨{id}⟩ SET {} RETURN *",
                assignments.join(", ")
            )
        };
        Ok((sql, Value::Object(vars)))
    } else {
        let sql = format!("UPSERT {table}:⟨{id}⟩ MERGE $content RETURN *");
        Ok((sql, json!({"content": normalized})))
    }
}

#[cfg(test)]
mod platform_tests {
    use super::*;
    use crate::error::MemoryError;

    #[test]
    fn validate_record_id_accepts_episode_with_id() {
        assert!(validate_record_id("episode:abc123").is_ok());
    }

    #[test]
    fn validate_record_id_accepts_fact_with_hex_id() {
        assert!(validate_record_id("fact:52f9d92d20d829840f24294f").is_ok());
    }

    #[test]
    fn validate_record_id_accepts_plain_table() {
        // Used by select_table-style callers (no :id part).
        assert!(validate_record_id("episode").is_ok());
    }

    #[test]
    fn validate_record_id_rejects_bare_hex() {
        let err = validate_record_id("474b2d8b81b3feabf832ef08").unwrap_err();
        match err {
            MemoryError::Validation(msg) => {
                assert!(
                    msg.contains("'<table>:<id>'"),
                    "message should name the expected form, got: {msg}"
                );
                assert!(
                    msg.contains("474b2d8b81b3feabf832ef08"),
                    "message should echo the bad input, got: {msg}"
                );
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn validate_record_id_rejects_bare_hex_with_letters() {
        let err = validate_record_id("072d682d0d467aa94aad684d").unwrap_err();
        assert!(matches!(err, MemoryError::Validation(_)));
    }

    #[test]
    fn validate_record_id_rejects_empty_string() {
        let err = validate_record_id("").unwrap_err();
        match err {
            MemoryError::Validation(msg) => assert!(!msg.is_empty()),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn validate_record_id_rejects_colon_only() {
        let err = validate_record_id(":").unwrap_err();
        assert!(matches!(err, MemoryError::Validation(_)));
    }

    #[test]
    fn validate_record_id_rejects_empty_id_part() {
        // "episode:" — table present but id empty.
        let err = validate_record_id("episode:").unwrap_err();
        match err {
            MemoryError::Validation(msg) => {
                assert!(msg.contains("empty id"), "got: {msg}");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn validate_record_id_rejects_empty_table_part() {
        // ":abc123" — id present but table empty.
        let err = validate_record_id(":abc123").unwrap_err();
        match err {
            MemoryError::Validation(msg) => {
                assert!(msg.contains("empty table"), "got: {msg}");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn validate_record_id_rejects_uppercase_table() {
        // is_valid_table_name requires lowercase; uppercase should be rejected
        // when it appears in a full "Table:id" record id.
        let err = validate_record_id("Episode:abc").unwrap_err();
        assert!(matches!(err, MemoryError::Validation(_)));
    }

    #[test]
    fn build_select_one_query_empty_string_returns_safe_noop() {
        let (sql, bind) = build_select_one_query("");
        assert_eq!(sql, "SELECT * FROM none WHERE false");
        assert!(bind.is_none());
    }

    #[test]
    fn build_select_one_query_table_with_id() {
        let (sql, bind) = build_select_one_query("episode:abc123");
        assert_eq!(sql, "SELECT * FROM episode:⟨abc123⟩");
        assert!(bind.is_none());
    }

    #[test]
    fn build_select_one_query_table_only_returns_safe_noop() {
        let (sql, bind) = build_select_one_query("episode:");
        assert_eq!(sql, "SELECT * FROM none WHERE false");
        assert!(bind.is_none());
    }

    #[test]
    fn build_select_one_query_plain_table() {
        let (sql, bind) = build_select_one_query("episode");
        assert_eq!(sql, "SELECT * FROM episode");
        assert!(bind.is_none());
    }

    #[test]
    fn build_select_one_query_bare_hex_returns_safe_noop() {
        let (sql, bind) = build_select_one_query("474b2d8b81b3feabf832ef08");
        assert_eq!(sql, "SELECT * FROM none WHERE false");
        assert!(bind.is_none());
    }

    #[test]
    fn build_select_one_query_bare_hex_with_letters_returns_safe_noop() {
        let (sql, bind) = build_select_one_query("072d682d0d467aa94aad684d");
        assert_eq!(sql, "SELECT * FROM none WHERE false");
        assert!(bind.is_none());
    }

    #[test]
    fn build_select_one_query_fact_with_id() {
        let (sql, bind) = build_select_one_query("fact:52f9d92d20d829840f24294f");
        assert_eq!(sql, "SELECT * FROM fact:⟨52f9d92d20d829840f24294f⟩");
        assert!(bind.is_none());
    }
}
