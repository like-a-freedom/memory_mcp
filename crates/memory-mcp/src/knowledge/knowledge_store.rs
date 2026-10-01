//! Knowledge-owned context assembly reads.
//!
//! Knowledge owns facts, entities, communities, claims and triples,
//! so the reads over those tables belong to one store. The episode
//! reads are memory's and live in `episode_context_store.rs`; the
//! retrieval pipeline holds both, which makes the ownership visible
//! in the types rather than only in a comment.
//!
//! The method bodies are unchanged from the single store this split
//! came from, so this is a move rather than a rewrite.

use crate::storage::table_scope::{KnowledgeTables, ReleaseOwnedTable};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::error::MemoryError;
use crate::storage::BI_TEMPORAL_WHERE;
use crate::storage::{BoundDbClient, ContextFactQuery, DbClient, GraphDirection};

/// Read-side store for the knowledge-owned tables consulted during
/// context assembly: `fact`, `entity`, `community`, `triple` and
/// `edge`.
#[derive(Clone)]
pub struct KnowledgeStoreClient {
    db: BoundDbClient,
}

impl KnowledgeStoreClient {
    pub fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            db: BoundDbClient::new(db, namespace),
        }
    }

    /// Facts matching a query at query-time with bi-temporal and fact-type filters.
    pub async fn select_facts_filtered(
        &self,
        query: ContextFactQuery<'_>,
    ) -> Result<Vec<Value>, MemoryError> {
        let ContextFactQuery {
            cutoff,
            query_contains,
            limit,
            fact_types,
        } = query;
        let (sql, vars) = crate::storage::build_select_facts_filtered_query(
            cutoff,
            query_contains,
            limit,
            fact_types,
        );
        self.db.query_rows(&sql, Some(vars)).await
    }

    /// Facts linked to a set of normalized entity ids.
    pub async fn select_facts_by_entity_links(
        &self,
        cutoff: &str,
        entity_links: &[String],
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        let (sql, vars) =
            crate::storage::build_select_facts_by_entity_links_query(cutoff, entity_links, limit);
        self.db.query_rows(&sql, Some(vars)).await
    }

    /// Facts matching a subject-predicate-object triple pattern.
    ///
    /// Searches the `triple` table for rows whose subject, predicate, or
    /// object matches `query_text`, then retrieves the linked `fact` records.
    pub async fn select_facts_by_triple(
        &self,
        query_text: &str,
        cutoff: &str,
        limit: usize,
    ) -> Result<Vec<Value>, MemoryError> {
        let sql = format!(
            "SELECT * FROM fact \
             WHERE fact_id IN ( \
               SELECT source_fact_id FROM triple \
               WHERE (predicate CONTAINS $query OR object CONTAINS $query OR subject CONTAINS $query) \
             ) \
               AND {BI_TEMPORAL_WHERE} \
             LIMIT $limit"
        );
        let vars = json!({
            "query": query_text,
            "cutoff": cutoff,
            "limit": limit,
        });
        self.db.query_rows(&sql, Some(vars)).await
    }

    /// Approximate nearest-neighbour facts for an embedding query.
    pub async fn select_facts_ann(
        &self,
        cutoff: &str,
        query_vec: &[f64],
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        let (sql, vars) = crate::storage::build_select_facts_ann_query(cutoff, query_vec, limit);
        self.db.query_rows(&sql, Some(vars)).await
    }

    /// Neighboring edge records around a node, direction-bounded and
    /// cutoff-bounded.
    pub async fn select_edge_neighbors(
        &self,
        node_id: &str,
        cutoff: &str,
        direction: GraphDirection,
    ) -> Result<Vec<Value>, MemoryError> {
        let (sql, vars) =
            crate::storage::build_select_edge_neighbors_query(node_id, cutoff, direction);
        self.db.query_rows(&sql, Some(vars)).await
    }

    /// Entities matching a batch of normalized names (alias-resolution hot path).
    pub async fn select_entities_batch(
        &self,
        normalized_names: &[String],
    ) -> Result<Vec<Value>, MemoryError> {
        if normalized_names.is_empty() {
            return Ok(Vec::new());
        }
        let sql = "SELECT * FROM entity WHERE canonical_name_normalized IN $names \
                   OR aliases CONTAINSANY $names";
        let vars = json!({ "names": normalized_names });
        self.db.query_rows(sql, Some(vars)).await
    }

    /// Communities whose summary matches a free-text hint.
    pub async fn select_communities_matching_summary(
        &self,
        query: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        let query_literal = crate::storage::surreal_string_literal(query);
        let sql = format!(
            "SELECT *, search::score(1) AS ft_score FROM community WHERE summary @1@ {query_literal} \
             ORDER BY ft_score DESC, summary ASC LIMIT 25"
        );
        let vars = json!({ "query": query });
        self.db.query_rows(&sql, Some(vars)).await
    }

    /// Owner-named full scans.
    ///
    /// These replace the previous caller-supplied `select_table`
    /// escape hatch: an application-facing read names the scope
    /// it wants, never a table string.
    pub async fn scan_facts(&self) -> Result<Vec<Value>, MemoryError> {
        self.db.select_table(KnowledgeTables::table("fact")).await
    }

    /// Active (not-yet-invalidated) facts in the bound Active Namespace.
    pub async fn select_active_facts(&self, limit: i32) -> Result<Vec<Value>, MemoryError> {
        let (sql, vars) = crate::storage::build_select_active_facts_query(
            &crate::shared::temporal::normalize_dt(crate::shared::temporal::now()),
            limit,
        );
        self.db.query_rows(&sql, Some(vars)).await
    }
}
