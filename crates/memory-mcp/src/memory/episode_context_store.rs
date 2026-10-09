//! Memory-owned episode reads for context assembly.
//!
//! Memory owns the episode lifecycle, so the episode reads the
//! retrieval pipeline performs belong here rather than in the
//! knowledge store. The method bodies are unchanged from the single
//! store this split came from.
//!
//! `select_episodes_via_entity` is the deliberate exception: it joins
//! `episode`, `fact` and `edge`, so it spans two owners. It is kept
//! here with its reasoning recorded rather than split on a table
//! boundary that would be arbitrary. See
//! `docs/architecture/decisions/0001-typed-record-accessors.md`.

use crate::memory::queries::{
    build_select_episodes_by_ids_query, build_select_fact_ids_via_entity_query,
    build_select_source_episodes_via_facts_query,
};
use crate::shared::record::split_record_id;
use crate::storage::table_scope::{MemoryTables, ReleaseOwnedTable};
use std::sync::Arc;

use serde_json::Value;

use crate::error::MemoryError;
use crate::storage::{BoundDbClient, DbClient};

/// Read-side store for the `episode` table during context assembly.
#[derive(Clone)]
pub struct EpisodeContextStore {
    db: BoundDbClient,
}

impl EpisodeContextStore {
    pub fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            db: BoundDbClient::new(db, namespace),
        }
    }

    pub(crate) fn from_bound(db: BoundDbClient) -> Self {
        Self { db }
    }

    /// Full scan of the `episode` table.
    ///
    /// Owner-named, so a caller cannot name a table.
    pub async fn scan_episodes(&self) -> Result<Vec<Value>, MemoryError> {
        self.db.select_table(MemoryTables::table("episode")).await
    }

    /// Episode contents matching a query, bi-temporally scoped.
    pub async fn select_episodes_by_content(
        &self,
        cutoff: &str,
        query: Option<&str>,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        let (sql, vars) =
            crate::memory::queries::build_select_episodes_by_content_query(cutoff, query, limit);
        self.db.query_rows(&sql, Some(vars)).await
    }

    /// Episodes linked to an entity through the fact→edge graph
    /// (`entity ←edge← fact →episode`), newest first (graph-shaped
    /// read-model queries belong to this store).
    ///
    /// Resolved as three bounded lookups — fact ids from `edge`, then
    /// `source_episode` from `fact`, then the episode rows — rather than one
    /// query nesting two `IN (subquery)` layers. On SurrealDB 3.3.0 a nested
    /// `IN` is answered with a table scan of the outer table even when the
    /// inner predicates are indexable, so the single-query shape read whole
    /// tables per call. See the 2026-10-09 query-performance plan, Appendix A.
    pub async fn select_episodes_via_entity(
        &self,
        entity_id: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        let Some((entity_table, entity_key)) = split_record_id(entity_id) else {
            // Not a record id, so there is no table/key pair to bind an index
            // bound from. Such a value cannot be an edge endpoint either — every
            // endpoint is written as a record from its two parts — so it matches
            // no edges and therefore no episodes. Answering empty is also what
            // the previous single nested-`IN` query returned for these values.
            return Ok(Vec::new());
        };

        let (fact_ids_sql, fact_ids_vars) =
            build_select_fact_ids_via_entity_query(entity_table, entity_key);
        let fact_rows = self
            .db
            .query_rows(&fact_ids_sql, Some(fact_ids_vars))
            .await?;
        let fact_ids = fact_rows
            .iter()
            .filter_map(|row| row["fact_id"].as_str().map(str::to_string))
            .collect::<Vec<_>>();
        if fact_ids.is_empty() {
            return Ok(Vec::new());
        }

        let (episodes_sql, episodes_vars) = build_select_source_episodes_via_facts_query(&fact_ids);
        let episode_id_rows = self
            .db
            .query_rows(&episodes_sql, Some(episodes_vars))
            .await?;
        let episode_ids = episode_id_rows
            .iter()
            .filter_map(|row| row.get("source_episode"))
            .filter(|value| !value.is_null())
            .cloned()
            .collect::<Vec<_>>();
        if episode_ids.is_empty() {
            return Ok(Vec::new());
        }

        let (rows_sql, rows_vars) = build_select_episodes_by_ids_query(&episode_ids);
        self.db.query_rows(&rows_sql, Some(rows_vars)).await
    }
}
