//! Triple-based fact retrieval for structured queries.
//!
//! Searches the `triple` table for rows whose subject, predicate, or object
//! matches a query term, then returns the linked `fact` records via
//! `source_fact_id`. This enables structured queries like "кем работает X?"
//! (who works where?) where the answer lives in a triple's object field.
//!
//! When no triples exist in the database (e.g. extraction has not yet run
//! for the matched facts), the storage layer returns an empty result and
//! this module surfaces it unchanged. The step is best-effort: it runs after
//! lexical, temporal, alias-expansion, and experience tiers.

use crate::error::MemoryError;
use crate::models::Fact;
use crate::service::episode::fact_from_value_or_wrapper;
use crate::service::service_context::RetrievalContext;

/// Collect facts linked via triples matching the given query text.
///
/// Returns facts whose `fact_id` is referenced by a triple row where the
/// subject, predicate, or object matches `query`. If the triple table is
/// missing or empty, returns an empty result (handled gracefully by the
/// storage layer via `is_missing_table_error`).
///
/// This is a best-effort retrieval step called after lexical, temporal,
/// alias-expansion, and experience tiers.
pub(super) async fn collect_triple_facts(
    service: &RetrievalContext,
    cutoff_iso: &str,
    query: &str,
    limit: i32,
) -> Result<Vec<Fact>, MemoryError> {
    let records = service
        .context_store()
        .select_facts_by_triple(query, cutoff_iso, limit as usize)
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB triple query error: {err}")))?;

    let facts: Vec<Fact> = records
        .iter()
        .filter_map(fact_from_value_or_wrapper)
        .collect();

    Ok(facts)
}
