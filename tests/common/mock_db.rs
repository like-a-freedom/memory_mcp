use async_trait::async_trait;
use memory_mcp::models::MemoryError;
use memory_mcp::storage::DbClient;
use serde_json::Value;

/// Mock DbClient for testing. Defaults all methods to return empty/None results,
/// with fluent setters to override specific behaviors.
pub struct MockDbClient;

impl MockDbClient {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MockDbClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DbClient for MockDbClient {
    async fn select_one(
        &self,
        _record_id: &str,
        _namespace: &str,
    ) -> Result<Option<Value>, MemoryError> {
        Ok(None)
    }

    async fn select_table(
        &self,
        _table: &str,
        _namespace: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        Ok(vec![])
    }

    async fn select_facts_filtered(
        &self,
        _namespace: &str,
        _scope: &str,
        _cutoff: &str,
        _query_contains: Option<&str>,
        _limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        Ok(vec![])
    }

    async fn select_facts_by_entity_links(
        &self,
        _namespace: &str,
        _scope: &str,
        _cutoff: &str,
        _entity_links: &[String],
        _limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        Ok(vec![])
    }

    async fn select_facts_ann(
        &self,
        _namespace: &str,
        _scope: &str,
        _cutoff: &str,
        _query_vec: &[f32],
        _limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        Ok(vec![])
    }

    async fn select_active_facts(
        &self,
        _limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        Ok(vec![])
    }

    async fn count_facts_needing_reembed(
        &self,
        _target_signature: &str,
    ) -> Result<(String, Value), MemoryError> {
        Ok(("0".to_string(), json!({})))
    }

    async fn select_facts_needing_reembed(
        &self,
        _namespace: &str,
        _target_signature: &str,
        _limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        Ok(vec![])
    }

    async fn select_edges_filtered(
        &self,
        _cutoff: &str,
    ) -> Result<(String, Value), MemoryError> {
        Ok(("0".to_string(), json!({})))
    }

    async fn select_edges_filtered_page(
        &self,
        _cutoff: &str,
        _limit: i32,
        _start: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        Ok(vec![])
    }

    async fn select_edge_neighbors(
        &self,
        _node_id: &str,
        _cutoff: &str,
        _direction: &str,
    ) -> Result<(String, Value), MemoryError> {
        Ok(("0".to_string(), json!({})))
    }

    async fn select_entity_lookup_canonical(
        &self,
        _normalized_name: &str,
    ) -> Result<Value, MemoryError> {
        Ok(json!({}))
    }

    async fn select_entity_lookup_alias(
        &self,
        _normalized_name: &str,
    ) -> Result<Value, MemoryError> {
        Ok(json!({}))
    }

    async fn select_communities_matching_summary(
        &self,
        _query: &str,
    ) -> Result<(String, Value), MemoryError> {
        Ok(("0".to_string(), json!({})))
    }

    async fn select_communities_by_member_entities(
        &self,
        _member_entities: &[String],
    ) -> Result<(String, Value), MemoryError> {
        Ok(("0".to_string(), json!({})))
    }

    async fn select_episodes_for_archival(
        &self,
        _cutoff: &str,
        _limit: i32,
    ) -> Result<(String, Value), MemoryError> {
        Ok(("0".to_string(), json!({})))
    }

    async fn select_active_facts_by_episode(
        &self,
        _episode_id: &str,
        _cutoff: &str,
        _limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        Ok(vec![])
    }

    async fn select_episodes_by_content(
        &self,
        _namespace: &str,
        _scope: &str,
        _cutoff: &str,
        _query_contains: Option<&str>,
        _limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        Ok(vec![])
    }

    async fn relate_edge(
        &self,
        _edge_id: &str,
        _from_id: &str,
        _to_id: &str,
        _content: Value,
    ) -> Result<(), MemoryError> {
        Ok(())
    }

    async fn create(
        &self,
        _record_id: &str,
        _content: Value,
        _namespace: &str,
    ) -> Result<(), MemoryError> {
        Ok(())
    }

    async fn update(
        &self,
        _record_id: &str,
        _content: Value,
        _namespace: &str,
    ) -> Result<Option<Value>, MemoryError> {
        Ok(None)
    }

    async fn execute_raw_query(
        &self,
        _sql: &str,
        _bindings: Option<Value>,
        _namespace: &str,
    ) -> Result<(), MemoryError> {
        Ok(())
    }

    async fn apply_migrations(
        &self,
        _namespace: &str,
    ) -> Result<(), MemoryError> {
        Ok(())
    }

    async fn query_raw(
        &self,
        _sql: &str,
        _bindings: Option<Value>,
    ) -> Result<Value, MemoryError> {
        Ok(json!([]))
    }
}