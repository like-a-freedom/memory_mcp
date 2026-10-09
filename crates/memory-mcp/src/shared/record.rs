//! Splitting a record id into the table and key halves an index bound needs.
//!
//! On SurrealDB 3.3.0 a `<record> $param` cast folds to a literal *after*
//! index selection, so the planner emits a TableScan for the predicate's
//! table. Binding the two halves separately and constructing the record
//! in-query with `type::record($table, $key)` produces an IndexScan at plan
//! time instead. See the 2026-10-09 query-performance plan, Appendix A.
//!
//! The invariant: split on the **first** colon only, because a record key
//! may itself contain colons (`entity:odd:key` is the table `entity` and the
//! key `odd:key`). An empty half is not a record id — `":x"` names no table
//! and `"x:"` names no key — so those return `None` and their callers keep
//! the legacy cast path rather than building a half-formed bound.

/// Splits a record id into `(table, key)` on the first colon.
///
/// Returns `None` when the id has no colon or either half is empty, which is
/// the signal that this is not a record id and the caller must fall back.
pub(crate) fn split_record_id(record_id: &str) -> Option<(&str, &str)> {
    let (table, key) = record_id.split_once(':')?;
    if table.is_empty() || key.is_empty() {
        return None;
    }
    Some((table, key))
}

/// Reads back the node record id a graph-neighbor builder bound.
///
/// Test doubles standing in for a graph read key their fixture on the node
/// being looked up, which the builder binds as `node_table`/`node_key`. Only
/// that shape is read: an id that could not be split is bound as `node_id` and
/// is not a record id, so returning `""` for it keeps a double from silently
/// accepting the fallback path when the test means to exercise the record one.
#[cfg(test)]
#[must_use]
pub(crate) fn bound_node_id(vars: &serde_json::Value) -> String {
    match (
        vars.get("node_table").and_then(serde_json::Value::as_str),
        vars.get("node_key").and_then(serde_json::Value::as_str),
    ) {
        (Some(table), Some(key)) => format!("{table}:{key}"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{bound_node_id, split_record_id};

    #[test]
    fn split_record_id_splits_on_first_colon_and_keeps_remainder() {
        assert_eq!(
            split_record_id("entity:odd:key"),
            Some(("entity", "odd:key"))
        );
        assert_eq!(split_record_id("t:ab"), Some(("t", "ab")));
        assert_eq!(split_record_id("bare"), None);
        assert_eq!(split_record_id(":x"), None);
        assert_eq!(split_record_id("x:"), None);
    }

    #[test]
    fn bound_node_id_rebuilds_the_split_binding_and_ignores_the_cast() {
        let split = serde_json::json!({"node_table": "entity", "node_key": "odd:key"});
        assert_eq!(bound_node_id(&split), "entity:odd:key");
        assert_eq!(bound_node_id(&serde_json::json!({"node_id": "noid"})), "");
        assert_eq!(bound_node_id(&serde_json::json!({})), "");
    }
}
