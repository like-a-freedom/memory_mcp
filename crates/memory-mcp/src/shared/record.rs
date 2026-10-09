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

#[cfg(test)]
mod tests {
    use super::split_record_id;

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
}
