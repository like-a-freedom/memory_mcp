//! Concrete store for the episode domain: episode reads/writes plus the
//! community/entity lookups community helpers use.
//!
//! Replaces direct `DbClient` consumption in `service/episode/` per
//! ADR-0024 step 6.

use std::sync::Arc;

use serde_json::Value;

use crate::service::MemoryError;
use crate::storage::{DbClient, GraphDirection};

#[derive(Clone)]
pub struct EpisodeStoreClient {
    db: Arc<dyn DbClient>,
}

impl EpisodeStoreClient {
    pub fn new(db: Arc<dyn DbClient>) -> Self {
        Self { db }
    }

    pub async fn select_one(
        &self,
        record_id: &str,
        namespace: &str,
    ) -> Result<Option<Value>, MemoryError> {
        self.db.select_one(record_id, namespace).await
    }

    pub async fn create(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        self.db.create(record_id, content, namespace).await
    }

    pub async fn update(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        self.db.update(record_id, content, namespace).await
    }

    pub async fn query(
        &self,
        sql: &str,
        vars: Option<Value>,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        self.db.query(sql, vars, namespace).await
    }

    /// Neighbors around a graph node within a namespace.
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

    /// Entities matching a set of canonical id strings.
    pub async fn select_entities_by_ids(
        &self,
        namespace: &str,
        entity_ids: &[String],
    ) -> Result<Vec<Value>, MemoryError> {
        self.db.select_entities_by_ids(namespace, entity_ids).await
    }

    /// Communities containing any of the listed member entities.
    pub async fn select_communities_by_member_entities(
        &self,
        namespace: &str,
        member_entities: &[String],
    ) -> Result<Vec<Value>, MemoryError> {
        self.db
            .select_communities_by_member_entities(namespace, member_entities)
            .await
    }

    /// Edges whose `in`/`out`/`relation` matches this triple query.
    pub async fn select_edges_for_triple(
        &self,
        namespace: &str,
        in_id: &str,
        relation: &str,
        out_id: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        self.db
            .select_edges_for_triple(namespace, in_id, relation, out_id)
            .await
    }

    /// Link two records through an edge.
    pub async fn relate_edge(
        &self,
        edge_id: &str,
        from_id: &str,
        to_id: &str,
        content: Value,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        self.db
            .relate_edge(namespace, edge_id, from_id, to_id, content)
            .await
    }
}
