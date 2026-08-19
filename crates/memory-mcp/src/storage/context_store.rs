//! Narrow context assembly store over `Arc<dyn DbClient>`.
//!
//! Capability seams replaced by concrete structs per ADR-0024. Queries are
//! owned here, not on a trait that forwards to `DbClient`. Context Assembly
//! consumers (ranking, lexical, semantic, temporal, graph, alias expansion,
//! views, experience, triple, logging) depend on this struct instead of the
//! `ContextStore` / `ContextAccessLog` trait objects.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::service::MemoryError;
use crate::storage::queries::BI_TEMPORAL_WHERE;
use crate::storage::{BoundDbClient, ContextFactQuery, DbClient, GraphDirection};

/// Read-side context assembly store. Holds the `DbClient` adapter and owns
/// the queries that context modules execute across it.
#[derive(Clone)]
pub struct ContextStoreClient {
    db: BoundDbClient,
}

impl ContextStoreClient {
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
        let (sql, vars) = crate::storage::queries::build_select_facts_filtered_query(
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
        let (sql, vars) = crate::storage::queries::build_select_facts_by_entity_links_query(
            cutoff,
            entity_links,
            limit,
        );
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
        let (sql, vars) =
            crate::storage::queries::build_select_facts_ann_query(cutoff, query_vec, limit);
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
            crate::storage::queries::build_select_edge_neighbors_query(node_id, cutoff, direction);
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
        let query_literal = crate::storage::queries::surreal_string_literal(query);
        let sql = format!(
            "SELECT *, search::score(1) AS ft_score FROM community WHERE summary @1@ {query_literal} \
             ORDER BY ft_score DESC, summary ASC LIMIT 25"
        );
        let vars = json!({ "query": query });
        self.db.query_rows(&sql, Some(vars)).await
    }

    /// Full table scan (used by graph views and the occasional admin operation).
    pub async fn select_table(&self, table: &str) -> Result<Vec<Value>, MemoryError> {
        self.db.select_table(table).await
    }

    /// Episode contents matching a query, bi-temporally scoped.
    pub async fn select_episodes_by_content(
        &self,
        cutoff: &str,
        query: Option<&str>,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        let (sql, vars) =
            crate::storage::queries::build_select_episodes_by_content_query(cutoff, query, limit);
        self.db.query_rows(&sql, Some(vars)).await
    }

    /// Active (not-yet-invalidated) facts in the bound Active Namespace.
    pub async fn select_active_facts(&self, limit: i32) -> Result<Vec<Value>, MemoryError> {
        let (sql, vars) = crate::storage::queries::build_select_active_facts_query(
            &crate::service::normalize_dt(crate::service::now()),
            limit,
        );
        self.db.query_rows(&sql, Some(vars)).await
    }
}

/// Write-side context store — only the access-log path. Keeps narrow
/// ownership of its two operations instead of forwarding `create`/`query` to
/// `DbClient` through a trait.
#[derive(Clone)]
pub struct ContextAccessLogClient {
    db: BoundDbClient,
}

impl ContextAccessLogClient {
    pub fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            db: BoundDbClient::new(db, namespace),
        }
    }

    pub async fn create(&self, record_id: &str, content: Value) -> Result<Value, MemoryError> {
        self.db.create(record_id, content).await
    }

    pub async fn query(&self, sql: &str, vars: Option<Value>) -> Result<Value, MemoryError> {
        self.db.query(sql, vars).await
    }
}
