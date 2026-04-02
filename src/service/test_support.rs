//! Test support utilities for unit tests.
//!
//! This module provides a unified, configurable mock DbClient implementation
//! that eliminates boilerplate in test code.

use async_trait::async_trait;
use serde_json::Value;

use crate::service::error::MemoryError;
use crate::storage::{DbClient, GraphDirection};

type SelectOneFn = dyn Fn(&str, &str) -> Result<Option<Value>, MemoryError> + Send + Sync;
type SelectTableFn = dyn Fn(&str, &str) -> Result<Vec<Value>, MemoryError> + Send + Sync;
type SelectFactsFilteredFn =
    dyn Fn(&str, &str, &str, Option<&str>, i32) -> Result<Vec<Value>, MemoryError> + Send + Sync;
type SelectEpisodesByContentFn =
    dyn Fn(&str, &str, &str, i32) -> Result<Vec<Value>, MemoryError> + Send + Sync;
type SelectFactsByEntityLinksFn =
    dyn Fn(&str, &str, &str, &[String], i32) -> Result<Vec<Value>, MemoryError> + Send + Sync;
type SelectEdgesFilteredFn = dyn Fn(&str, &str) -> Result<Vec<Value>, MemoryError> + Send + Sync;
type SelectEdgeNeighborsFn =
    dyn Fn(&str, &str, &str, GraphDirection) -> Result<Vec<Value>, MemoryError> + Send + Sync;
type SelectEntityLookupFn = dyn Fn(&str, &str) -> Result<Option<Value>, MemoryError> + Send + Sync;
type SelectEntitiesBatchFn =
    dyn Fn(&str, &[String]) -> Result<Vec<Value>, MemoryError> + Send + Sync;
type SelectFactsAnnFn =
    dyn Fn(&str, &str, &str, &[f64], i32) -> Result<Vec<Value>, MemoryError> + Send + Sync;
type SelectCommunitiesByMemberEntitiesFn =
    dyn Fn(&str, &[String]) -> Result<Vec<Value>, MemoryError> + Send + Sync;
type SelectCommunitiesMatchingSummaryFn =
    dyn Fn(&str, &str) -> Result<Vec<Value>, MemoryError> + Send + Sync;
type RelateEdgeFn =
    dyn Fn(&str, &str, &str, &str, Value) -> Result<Value, MemoryError> + Send + Sync;
type CreateFn = dyn Fn(&str, Value, &str) -> Result<Value, MemoryError> + Send + Sync;
type UpdateFn = dyn Fn(&str, Value, &str) -> Result<Value, MemoryError> + Send + Sync;
type QueryFn = dyn Fn(&str, Option<Value>, &str) -> Result<Value, MemoryError> + Send + Sync;
type SelectActiveFactsFn = dyn Fn(&str, i32) -> Result<Vec<Value>, MemoryError> + Send + Sync;
type SelectEpisodesForArchivalFn =
    dyn Fn(&str, &str, i32) -> Result<Vec<Value>, MemoryError> + Send + Sync;
type SelectActiveFactsByEpisodeFn =
    dyn Fn(&str, &str, &str, i32) -> Result<Vec<Value>, MemoryError> + Send + Sync;
type SelectFactsByEpisodeAnyFn =
    dyn Fn(&str, &str, i32) -> Result<Vec<Value>, MemoryError> + Send + Sync;
type ApplyMigrationsFn = dyn Fn(&str) -> Result<(), MemoryError> + Send + Sync;
type SelectEpisodesByEntityFn =
    dyn Fn(&str, &str, i32) -> Result<Vec<Value>, MemoryError> + Send + Sync;

/// A unified, configurable mock DbClient for tests.
///
/// All methods return safe defaults (`Ok(None)`, `Ok(vec![])`, `Ok(Value::Null)`, `Ok(())`)
/// unless overridden via the closure fields. This eliminates boilerplate and
/// allows tests to override only the methods they care about.
///
/// # Example
///
/// ```rust,ignore
/// let mut mock = MockDb::default();
/// mock.select_entity_lookup_fn = Box::new(|_ns, name| {
///     Ok(Some(json!({"entity_id": format!("entity:{}", name)})))
/// });
/// ```
pub(crate) struct MockDb {
    pub select_one_fn: Box<SelectOneFn>,
    pub select_table_fn: Box<SelectTableFn>,
    pub select_facts_filtered_fn: Box<SelectFactsFilteredFn>,
    pub select_episodes_by_content_fn: Box<SelectEpisodesByContentFn>,
    pub select_facts_by_entity_links_fn: Box<SelectFactsByEntityLinksFn>,
    pub select_edges_filtered_fn: Box<SelectEdgesFilteredFn>,
    pub select_edge_neighbors_fn: Box<SelectEdgeNeighborsFn>,
    pub select_entity_lookup_fn: Box<SelectEntityLookupFn>,
    pub select_entities_batch_fn: Box<SelectEntitiesBatchFn>,
    pub select_facts_ann_fn: Box<SelectFactsAnnFn>,
    pub select_communities_by_member_entities_fn: Box<SelectCommunitiesByMemberEntitiesFn>,
    pub select_communities_matching_summary_fn: Box<SelectCommunitiesMatchingSummaryFn>,
    pub relate_edge_fn: Box<RelateEdgeFn>,
    pub create_fn: Box<CreateFn>,
    pub update_fn: Box<UpdateFn>,
    pub query_fn: Box<QueryFn>,
    pub select_active_facts_fn: Box<SelectActiveFactsFn>,
    pub select_episodes_for_archival_fn: Box<SelectEpisodesForArchivalFn>,
    pub select_active_facts_by_episode_fn: Box<SelectActiveFactsByEpisodeFn>,
    pub select_facts_by_episode_any_fn: Box<SelectFactsByEpisodeAnyFn>,
    pub apply_migrations_fn: Box<ApplyMigrationsFn>,
    pub select_episodes_by_entity_fn: Box<SelectEpisodesByEntityFn>,
}

impl Default for MockDb {
    fn default() -> Self {
        Self {
            select_one_fn: Box::new(|_, _| Ok(None)),
            select_table_fn: Box::new(|_, _| Ok(vec![])),
            select_facts_filtered_fn: Box::new(|_, _, _, _, _| Ok(vec![])),
            select_episodes_by_content_fn: Box::new(|_, _, _, _| Ok(vec![])),
            select_facts_by_entity_links_fn: Box::new(|_, _, _, _, _| Ok(vec![])),
            select_edges_filtered_fn: Box::new(|_, _| Ok(vec![])),
            select_edge_neighbors_fn: Box::new(|_, _, _, _| Ok(vec![])),
            select_entity_lookup_fn: Box::new(|_, _| Ok(None)),
            select_entities_batch_fn: Box::new(|_, _| Ok(vec![])),
            select_facts_ann_fn: Box::new(|_, _, _, _, _| Ok(vec![])),
            select_communities_by_member_entities_fn: Box::new(|_, _| Ok(vec![])),
            select_communities_matching_summary_fn: Box::new(|_, _| Ok(vec![])),
            relate_edge_fn: Box::new(|_, _, _, _, _| Ok(Value::Null)),
            create_fn: Box::new(|_, _, _| Ok(Value::Null)),
            update_fn: Box::new(|_, _, _| Ok(Value::Null)),
            query_fn: Box::new(|_, _, _| Ok(Value::Null)),
            select_active_facts_fn: Box::new(|_, _| Ok(vec![])),
            select_episodes_for_archival_fn: Box::new(|_, _, _| Ok(vec![])),
            select_active_facts_by_episode_fn: Box::new(|_, _, _, _| Ok(vec![])),
            select_facts_by_episode_any_fn: Box::new(|_, _, _| Ok(vec![])),
            apply_migrations_fn: Box::new(|_| Ok(())),
            select_episodes_by_entity_fn: Box::new(|_, _, _| Ok(vec![])),
        }
    }
}

#[async_trait]
impl DbClient for MockDb {
    async fn select_one(
        &self,
        record_id: &str,
        namespace: &str,
    ) -> Result<Option<Value>, MemoryError> {
        (self.select_one_fn)(record_id, namespace)
    }

    async fn select_table(&self, table: &str, namespace: &str) -> Result<Vec<Value>, MemoryError> {
        (self.select_table_fn)(table, namespace)
    }

    async fn select_facts_filtered(
        &self,
        namespace: &str,
        scope: &str,
        cutoff: &str,
        query_contains: Option<&str>,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        (self.select_facts_filtered_fn)(namespace, scope, cutoff, query_contains, limit)
    }

    async fn select_episodes_by_content(
        &self,
        namespace: &str,
        scope: &str,
        query_contains: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        (self.select_episodes_by_content_fn)(namespace, scope, query_contains, limit)
    }

    async fn select_facts_by_entity_links(
        &self,
        namespace: &str,
        scope: &str,
        cutoff: &str,
        entity_links: &[String],
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        (self.select_facts_by_entity_links_fn)(namespace, scope, cutoff, entity_links, limit)
    }

    async fn select_edges_filtered(
        &self,
        namespace: &str,
        cutoff: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        (self.select_edges_filtered_fn)(namespace, cutoff)
    }

    async fn select_edge_neighbors(
        &self,
        namespace: &str,
        node_id: &str,
        cutoff: &str,
        direction: GraphDirection,
    ) -> Result<Vec<Value>, MemoryError> {
        (self.select_edge_neighbors_fn)(namespace, node_id, cutoff, direction)
    }

    async fn select_entity_lookup(
        &self,
        namespace: &str,
        normalized_name: &str,
    ) -> Result<Option<Value>, MemoryError> {
        (self.select_entity_lookup_fn)(namespace, normalized_name)
    }

    async fn select_entities_batch(
        &self,
        namespace: &str,
        names: &[String],
    ) -> Result<Vec<Value>, MemoryError> {
        (self.select_entities_batch_fn)(namespace, names)
    }

    async fn select_facts_ann(
        &self,
        namespace: &str,
        scope: &str,
        cutoff: &str,
        query_vec: &[f64],
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        (self.select_facts_ann_fn)(namespace, scope, cutoff, query_vec, limit)
    }

    async fn select_communities_by_member_entities(
        &self,
        namespace: &str,
        member_entities: &[String],
    ) -> Result<Vec<Value>, MemoryError> {
        (self.select_communities_by_member_entities_fn)(namespace, member_entities)
    }

    async fn select_communities_matching_summary(
        &self,
        namespace: &str,
        query: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        (self.select_communities_matching_summary_fn)(namespace, query)
    }

    async fn relate_edge(
        &self,
        namespace: &str,
        edge_id: &str,
        from_id: &str,
        to_id: &str,
        content: Value,
    ) -> Result<Value, MemoryError> {
        (self.relate_edge_fn)(namespace, edge_id, from_id, to_id, content)
    }

    async fn create(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        (self.create_fn)(record_id, content, namespace)
    }

    async fn update(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        (self.update_fn)(record_id, content, namespace)
    }

    async fn query(
        &self,
        sql: &str,
        vars: Option<Value>,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        (self.query_fn)(sql, vars, namespace)
    }

    async fn select_active_facts(
        &self,
        namespace: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        (self.select_active_facts_fn)(namespace, limit)
    }

    async fn select_episodes_for_archival(
        &self,
        namespace: &str,
        cutoff: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        (self.select_episodes_for_archival_fn)(namespace, cutoff, limit)
    }

    async fn select_active_facts_by_episode(
        &self,
        namespace: &str,
        episode_id: &str,
        cutoff: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        (self.select_active_facts_by_episode_fn)(namespace, episode_id, cutoff, limit)
    }

    async fn select_facts_by_episode_any(
        &self,
        namespace: &str,
        episode_id: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        (self.select_facts_by_episode_any_fn)(namespace, episode_id, limit)
    }

    async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError> {
        (self.apply_migrations_fn)(namespace)
    }

    async fn select_episodes_by_entity(
        &self,
        namespace: &str,
        entity_id: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        (self.select_episodes_by_entity_fn)(namespace, entity_id, limit)
    }
}
