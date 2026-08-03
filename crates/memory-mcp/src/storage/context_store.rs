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
use crate::storage::helpers::is_missing_table_error;
use crate::storage::queries::BI_TEMPORAL_WHERE;
use crate::storage::{ContextFactQuery, DbClient, GraphDirection};

/// Read-side context assembly store. Holds the `DbClient` adapter and owns
/// the queries that context modules execute across it.
#[derive(Clone)]
pub struct ContextStoreClient {
    db: Arc<dyn DbClient>,
}

impl ContextStoreClient {
    pub fn new(db: Arc<dyn DbClient>) -> Self {
        Self { db }
    }

    /// Facts matching a query at query-time with bi-temporal, scope, project, and fact-type filters.
    pub async fn select_facts_filtered(
        &self,
        query: ContextFactQuery<'_>,
    ) -> Result<Vec<Value>, MemoryError> {
        let ContextFactQuery {
            namespace,
            scope,
            cutoff,
            query_contains,
            limit,
            project,
            fact_types,
        } = query;
        self.db
            .select_facts_filtered(
                namespace,
                scope,
                cutoff,
                query_contains,
                limit,
                project,
                fact_types,
            )
            .await
    }

    /// Facts linked to a set of normalized entity ids.
    pub async fn select_facts_by_entity_links(
        &self,
        namespace: &str,
        scope: &str,
        cutoff: &str,
        entity_links: &[String],
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        self.db
            .select_facts_by_entity_links(namespace, scope, cutoff, entity_links, limit)
            .await
    }

    /// Facts matching a subject-predicate-object triple pattern.
    ///
    /// Searches the `triple` table for rows whose subject, predicate, or
    /// object matches `query_text`, then retrieves the linked `fact` records.
    pub async fn select_facts_by_triple(
        &self,
        namespace: &str,
        query_text: &str,
        cutoff: &str,
        limit: usize,
    ) -> Result<Vec<Value>, MemoryError> {
        let sql = format!(
            "SELECT * FROM fact \
             WHERE fact_id IN ( \
               SELECT source_fact_id FROM triple \
               WHERE namespace = $ns \
                 AND (predicate CONTAINS $query OR object CONTAINS $query OR subject CONTAINS $query) \
             ) \
               AND {BI_TEMPORAL_WHERE} \
             LIMIT $limit"
        );
        let mut vars = json!({
            "ns": "__placeholder__",
            "query": query_text,
            "cutoff": cutoff,
            "limit": limit,
        });
        vars.as_object_mut()
            .map(|map| map.insert("ns".to_string(), json!(namespace)));
        match self.db.query(&sql, Some(vars), namespace).await {
            Ok(value) => Ok(value.as_array().cloned().unwrap_or_default()),
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => Ok(vec![]),
            Err(err) => Err(err),
        }
    }

    /// Approximate nearest-neighbour facts for an embedding query.
    pub async fn select_facts_ann(
        &self,
        namespace: &str,
        scope: &str,
        cutoff: &str,
        query_vec: &[f64],
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        self.db
            .select_facts_ann(namespace, scope, cutoff, query_vec, limit)
            .await
    }

    /// Neighboring edge records around a node, direction-bounded and
    /// cutoff-bounded.
    pub async fn select_edge_neighbors(
        &self,
        namespace: &str,
        node_id: &str,
        cutoff: &str,
        direction: GraphDirection,
    ) -> Result<Vec<Value>, MemoryError> {
        self.db
            .select_edge_neighbors(namespace, node_id, cutoff, direction)
            .await
    }

    /// Entities matching a batch of normalized names (alias-resolution hot path).
    pub async fn select_entities_batch(
        &self,
        namespace: &str,
        normalized_names: &[String],
    ) -> Result<Vec<Value>, MemoryError> {
        if normalized_names.is_empty() {
            return Ok(Vec::new());
        }
        let sql = "SELECT * FROM entity WHERE canonical_name_normalized IN $names \
                   OR aliases CONTAINSANY $names";
        let vars = json!({ "names": normalized_names });
        match self.db.query(sql, Some(vars), namespace).await {
            Ok(value) => Ok(value.as_array().cloned().unwrap_or_default()),
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => Ok(vec![]),
            Err(err) => Err(err),
        }
    }

    /// Communities whose summary matches a free-text hint.
    pub async fn select_communities_matching_summary(
        &self,
        namespace: &str,
        query: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        self.db
            .select_communities_matching_summary(namespace, query)
            .await
    }

    /// Full table scan (used by graph views and the occasional admin operation).
    pub async fn select_table(
        &self,
        table: &str,
        namespace: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        self.db.select_table(table, namespace).await
    }

    /// Episode contents matching a query, bi-temporally scoped.
    pub async fn select_episodes_by_content(
        &self,
        namespace: &str,
        scope: &str,
        cutoff: &str,
        query: Option<&str>,
        limit: i32,
        project: Option<&str>,
    ) -> Result<Vec<Value>, MemoryError> {
        self.db
            .select_episodes_by_content(namespace, scope, cutoff, query, limit, project)
            .await
    }

    /// Active (not-yet-invalidated) facts for a namespace.
    pub async fn select_active_facts(
        &self,
        namespace: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        let (sql, vars) = crate::storage::queries::build_select_active_facts_query(
            &crate::service::normalize_dt(crate::service::now()),
            limit,
        );
        match self.db.query(&sql, Some(vars), namespace).await {
            Ok(value) => Ok(value.as_array().cloned().unwrap_or_default()),
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => Ok(vec![]),
            Err(err) => Err(err),
        }
    }
}

/// Write-side context store — only the access-log path. Keeps narrow
/// ownership of its two operations instead of forwarding `create`/`query` to
/// `DbClient` through a trait.
#[derive(Clone)]
pub struct ContextAccessLogClient {
    db: Arc<dyn DbClient>,
}

impl ContextAccessLogClient {
    pub fn new(db: Arc<dyn DbClient>) -> Self {
        Self { db }
    }

    pub async fn create(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        self.db.create(record_id, content, namespace).await
    }

    pub async fn query(
        &self,
        sql: &str,
        vars: Option<Value>,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        self.db.query(sql, vars, namespace).await
    }
}
