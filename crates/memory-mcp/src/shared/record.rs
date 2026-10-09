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

/// Reads back the record id a query builder bound.
///
/// A builder that could split the id binds `node_table`/`node_key`; one that
/// could not falls back to binding `node_id`. Test doubles standing in for a
/// graph read key on the node being looked up, and this returns it under either
/// shape so they need not care which path the builder took.
#[cfg(test)]
#[must_use]
pub(crate) fn node_id_from_vars(vars: &serde_json::Value) -> String {
    if let Some(id) = vars.get("node_id").and_then(serde_json::Value::as_str) {
        return id.to_string();
    }
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
    use super::{node_id_from_vars, split_record_id};

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
    fn node_id_from_vars_reads_either_binding_shape() {
        let split = serde_json::json!({"node_table": "entity", "node_key": "odd:key"});
        assert_eq!(node_id_from_vars(&split), "entity:odd:key");
        let cast = serde_json::json!({"node_id": "noid"});
        assert_eq!(node_id_from_vars(&cast), "noid");
        assert_eq!(node_id_from_vars(&serde_json::json!({})), "");
    }
}
