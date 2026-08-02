//! Mock database client for tests, eliminating boilerplate from hand-written mocks.
//!
//! Usage in tests:
//! ```rust,no_run
//! let db = MockDbClient::new()
//!     .expect_select_one("episode:test", Some(json!({"episode_id": "episode:test", "content": "hello"})))
//!     .expect_create("fact:1", json!({"status": "ok"}));
//! let service = MemoryService::new(Arc::new(db), vec!["org".into()], "warn".into(), 50, 100).unwrap();
//! ```

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::Value;

use crate::service::MemoryError;
use crate::storage::{DbClient, GraphDirection};

type SelectOneFn = dyn Fn(&str) -> Result<Option<Value>, MemoryError> + Send + Sync;
type SelectTableFn = dyn Fn(&str) -> Result<Vec<Value>, MemoryError> + Send + Sync;
type QueryFn = dyn Fn() -> Result<Value, MemoryError> + Send + Sync;
type CreateFn = dyn Fn() -> Result<Value, MemoryError> + Send + Sync;
type UpdateFn = dyn Fn() -> Result<Value, MemoryError> + Send + Sync;
type EdgeNeighborsFn =
    dyn Fn(&str, GraphDirection) -> Result<Vec<Value>, MemoryError> + Send + Sync;

/// Configurable mock database client for tests.
///
/// By default, every method returns `Ok(vec![])` or `Ok(None)`.
/// Use the `expect_*` builder methods to override specific calls.
pub struct MockDbClient {
    select_one_responses: Mutex<HashMap<String, Result<Option<Value>, MemoryError>>>,
    select_table_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    facts_filtered_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    facts_entity_links_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    edge_neighbors_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    entity_lookup_responses: Mutex<HashMap<String, Result<Option<Value>, MemoryError>>>,
    entities_by_ids_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    active_facts_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    episodes_by_content_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    communities_by_members_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    communities_matching_summary_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    relate_edge_responses: Mutex<HashMap<String, Result<Value, MemoryError>>>,
    create_responses: Mutex<HashMap<String, Result<Value, MemoryError>>>,
    update_responses: Mutex<HashMap<String, Result<Value, MemoryError>>>,
    query_responses: Mutex<HashMap<String, Result<Value, MemoryError>>>,
    migration_result: Mutex<Result<(), MemoryError>>,
    fallback_select_one: Mutex<Option<Box<SelectOneFn>>>,
    fallback_select_table: Mutex<Option<Box<SelectTableFn>>>,
    fallback_query: Mutex<Option<Box<QueryFn>>>,
    fallback_create: Mutex<Option<Box<CreateFn>>>,
    fallback_update: Mutex<Option<Box<UpdateFn>>>,
    fallback_edges_filtered: Mutex<Option<Box<SelectTableFn>>>,
    fallback_edge_neighbors: Mutex<Option<Box<EdgeNeighborsFn>>>,
    fallback_facts_filtered: Mutex<Option<Box<SelectTableFn>>>,
    fallback_facts_by_entity_links: Mutex<Option<Box<SelectTableFn>>>,
    fallback_facts_ann: Mutex<Option<Box<SelectTableFn>>>,
    fallback_entity_lookup: Mutex<Option<Box<SelectOneFn>>>,
    fallback_entities_batch: Mutex<Option<Box<SelectTableFn>>>,
    fallback_active_facts: Mutex<Option<Box<SelectTableFn>>>,
    fallback_episodes_for_archival: Mutex<Option<Box<SelectTableFn>>>,
    fallback_active_facts_by_episode: Mutex<Option<Box<SelectTableFn>>>,
    fallback_episodes_by_content: Mutex<Option<Box<SelectTableFn>>>,
}

impl MockDbClient {
    pub fn new() -> Self {
        Self {
            select_one_responses: Mutex::new(HashMap::new()),
            select_table_responses: Mutex::new(HashMap::new()),
            facts_filtered_responses: Mutex::new(HashMap::new()),
            facts_entity_links_responses: Mutex::new(HashMap::new()),
            edge_neighbors_responses: Mutex::new(HashMap::new()),
            entity_lookup_responses: Mutex::new(HashMap::new()),
            entities_by_ids_responses: Mutex::new(HashMap::new()),
            active_facts_responses: Mutex::new(HashMap::new()),
            episodes_by_content_responses: Mutex::new(HashMap::new()),
            communities_by_members_responses: Mutex::new(HashMap::new()),
            communities_matching_summary_responses: Mutex::new(HashMap::new()),
            relate_edge_responses: Mutex::new(HashMap::new()),
            create_responses: Mutex::new(HashMap::new()),
            update_responses: Mutex::new(HashMap::new()),
            query_responses: Mutex::new(HashMap::new()),
            migration_result: Mutex::new(Ok(())),
            fallback_select_one: Mutex::new(None),
            fallback_select_table: Mutex::new(None),
            fallback_query: Mutex::new(None),
            fallback_create: Mutex::new(None),
            fallback_update: Mutex::new(None),
            fallback_edges_filtered: Mutex::new(None),
            fallback_edge_neighbors: Mutex::new(None),
            fallback_facts_filtered: Mutex::new(None),
            fallback_facts_by_entity_links: Mutex::new(None),
            fallback_facts_ann: Mutex::new(None),
            fallback_entity_lookup: Mutex::new(None),
            fallback_entities_batch: Mutex::new(None),
            fallback_active_facts: Mutex::new(None),
            fallback_episodes_for_archival: Mutex::new(None),
            fallback_active_facts_by_episode: Mutex::new(None),
            fallback_episodes_by_content: Mutex::new(None),
        }
    }

    pub fn expect_select_one(self, record_id: &str, result: Option<Value>) -> Self {
        self.select_one_responses
            .lock()
            .unwrap()
            .insert(record_id.to_string(), Ok(result));
        self
    }

    pub fn expect_select_one_with(
        mut self,
        f: impl Fn(&str) -> Result<Option<Value>, MemoryError> + Send + Sync + 'static,
    ) -> Self {
        self.fallback_select_one = Mutex::new(Some(Box::new(f)));
        self
    }

    pub fn expect_create(self, record_id: &str, result: Value) -> Self {
        self.create_responses
            .lock()
            .unwrap()
            .insert(record_id.to_string(), Ok(result));
        self
    }

    pub fn expect_create_with(
        mut self,
        f: impl Fn() -> Result<Value, MemoryError> + Send + Sync + 'static,
    ) -> Self {
        self.fallback_create = Mutex::new(Some(Box::new(f)));
        self
    }

    pub fn expect_update(self, record_id: &str, result: Value) -> Self {
        self.update_responses
            .lock()
            .unwrap()
            .insert(record_id.to_string(), Ok(result));
        self
    }

    pub fn expect_select_table(self, table: &str, rows: Vec<Value>) -> Self {
        self.select_table_responses
            .lock()
            .unwrap()
            .insert(table.to_string(), Ok(rows));
        self
    }

    pub fn expect_entity_lookup(self, normalized_name: &str, result: Option<Value>) -> Self {
        self.entity_lookup_responses
            .lock()
            .unwrap()
            .insert(normalized_name.to_string(), Ok(result));
        self
    }

    pub fn expect_edge_neighbors(self, node_id: &str, neighbors: Vec<Value>) -> Self {
        self.edge_neighbors_responses
            .lock()
            .unwrap()
            .insert(node_id.to_string(), Ok(neighbors));
        self
    }

    pub fn expect_edge_neighbors_with(
        mut self,
        f: impl Fn(&str, GraphDirection) -> Result<Vec<Value>, MemoryError> + Send + Sync + 'static,
    ) -> Self {
        self.fallback_edge_neighbors = Mutex::new(Some(Box::new(f)));
        self
    }

    pub fn expect_select_table_with(
        mut self,
        f: impl Fn(&str) -> Result<Vec<Value>, MemoryError> + Send + Sync + 'static,
    ) -> Self {
        self.fallback_select_table = Mutex::new(Some(Box::new(f)));
        self
    }

    pub fn expect_select_table_panic(self, table_name: &str) -> Self {
        let table_name = table_name.to_string();
        self.expect_select_table_with(move |table| {
            panic!("select_table should not be called for {table_name}, got {table}");
        })
    }

    pub fn expect_edges_filtered_with(
        mut self,
        f: impl Fn(&str) -> Result<Vec<Value>, MemoryError> + Send + Sync + 'static,
    ) -> Self {
        self.fallback_edges_filtered = Mutex::new(Some(Box::new(f)));
        self
    }

    pub fn expect_edges_filtered_panic(self) -> Self {
        self.expect_edges_filtered_with(|_| panic!("select_edges_filtered should not be called"))
    }

    pub fn expect_entity_lookup_with(
        mut self,
        f: impl Fn(&str) -> Result<Option<Value>, MemoryError> + Send + Sync + 'static,
    ) -> Self {
        self.fallback_entity_lookup = Mutex::new(Some(Box::new(f)));
        self
    }

    pub fn expect_migration_handler(
        mut self,
        f: impl Fn(&str) -> Result<(), MemoryError> + Send + Sync + 'static,
    ) -> Self {
        self.migration_result = Mutex::new(Ok(()));
        let _ = f;
        self
    }

    pub fn expect_query(self, sql_prefix: &str, result: Value) -> Self {
        self.query_responses
            .lock()
            .unwrap()
            .insert(sql_prefix.to_string(), Ok(result));
        self
    }

    pub fn expect_facts_filtered(self, key: &str, rows: Vec<Value>) -> Self {
        self.facts_filtered_responses
            .lock()
            .unwrap()
            .insert(key.to_string(), Ok(rows));
        self
    }

    pub fn expect_facts_by_entity_links(self, key: &str, rows: Vec<Value>) -> Self {
        self.facts_entity_links_responses
            .lock()
            .unwrap()
            .insert(key.to_string(), Ok(rows));
        self
    }

    pub fn expect_active_facts(self, key: &str, rows: Vec<Value>) -> Self {
        self.active_facts_responses
            .lock()
            .unwrap()
            .insert(key.to_string(), Ok(rows));
        self
    }

    pub fn expect_episodes_by_content(self, key: &str, rows: Vec<Value>) -> Self {
        self.episodes_by_content_responses
            .lock()
            .unwrap()
            .insert(key.to_string(), Ok(rows));
        self
    }

    pub fn expect_communities_matching_summary(self, query: &str, rows: Vec<Value>) -> Self {
        self.communities_matching_summary_responses
            .lock()
            .unwrap()
            .insert(query.to_string(), Ok(rows));
        self
    }

    pub fn expect_communities_by_member_entities(self, key: &str, rows: Vec<Value>) -> Self {
        self.communities_by_members_responses
            .lock()
            .unwrap()
            .insert(key.to_string(), Ok(rows));
        self
    }

    pub fn expect_migration_result(mut self, result: Result<(), MemoryError>) -> Self {
        self.migration_result = Mutex::new(result);
        self
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
        record_id: &str,
        _namespace: &str,
    ) -> Result<Option<Value>, MemoryError> {
        if let Some(resp) = self
            .select_one_responses
            .lock()
            .unwrap()
            .get(record_id)
            .cloned()
        {
            return resp;
        }
        if let Some(ref f) = *self.fallback_select_one.lock().unwrap() {
            return f(record_id);
        }
        Ok(None)
    }

    async fn select_table(&self, table: &str, _namespace: &str) -> Result<Vec<Value>, MemoryError> {
        if let Some(resp) = self
            .select_table_responses
            .lock()
            .unwrap()
            .get(table)
            .cloned()
        {
            return resp;
        }
        if let Some(ref f) = *self.fallback_select_table.lock().unwrap() {
            return f(table);
        }
        Ok(vec![])
    }

    async fn select_facts_filtered(
        &self,
        _namespace: &str,
        scope: &str,
        _cutoff: &str,
        query_contains: Option<&str>,
        _limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        let key = format!("{}/{}", scope, query_contains.unwrap_or(""));
        if let Some(resp) = self
            .facts_filtered_responses
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
        {
            return resp;
        }
        if let Some(ref f) = *self.fallback_facts_filtered.lock().unwrap() {
            return f("");
        }
        Ok(vec![])
    }

    async fn select_facts_by_entity_links(
        &self,
        _namespace: &str,
        _scope: &str,
        _cutoff: &str,
        entity_links: &[String],
        _limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        let key = entity_links.join(",");
        if let Some(resp) = self
            .facts_entity_links_responses
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
        {
            return resp;
        }
        if let Some(ref f) = *self.fallback_facts_by_entity_links.lock().unwrap() {
            return f("");
        }
        Ok(vec![])
    }

    async fn select_facts_ann(
        &self,
        _namespace: &str,
        _scope: &str,
        _cutoff: &str,
        _query_vec: &[f64],
        _limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        if let Some(ref f) = *self.fallback_facts_ann.lock().unwrap() {
            return f("");
        }
        Ok(vec![])
    }

    async fn select_edges_filtered(
        &self,
        _namespace: &str,
        _cutoff: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        if let Some(ref f) = *self.fallback_edges_filtered.lock().unwrap() {
            return f("");
        }
        Ok(vec![])
    }

    async fn select_edge_neighbors(
        &self,
        _namespace: &str,
        node_id: &str,
        _cutoff: &str,
        _direction: GraphDirection,
    ) -> Result<Vec<Value>, MemoryError> {
        if let Some(resp) = self
            .edge_neighbors_responses
            .lock()
            .unwrap()
            .get(node_id)
            .cloned()
        {
            return resp;
        }
        if let Some(ref f) = *self.fallback_edge_neighbors.lock().unwrap() {
            return f(node_id, _direction);
        }
        Ok(vec![])
    }

    async fn select_entity_lookup(
        &self,
        _namespace: &str,
        normalized_name: &str,
    ) -> Result<Option<Value>, MemoryError> {
        if let Some(resp) = self
            .entity_lookup_responses
            .lock()
            .unwrap()
            .get(normalized_name)
            .cloned()
        {
            return resp;
        }
        if let Some(ref f) = *self.fallback_entity_lookup.lock().unwrap() {
            return f(normalized_name);
        }
        Ok(None)
    }

    async fn select_entities_batch(
        &self,
        _namespace: &str,
        _names: &[String],
    ) -> Result<Vec<Value>, MemoryError> {
        if let Some(ref f) = *self.fallback_entities_batch.lock().unwrap() {
            return f("");
        }
        Ok(vec![])
    }

    async fn select_active_facts(
        &self,
        _namespace: &str,
        _limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        if let Some(ref f) = *self.fallback_active_facts.lock().unwrap() {
            return f("");
        }
        Ok(vec![])
    }

    async fn select_episodes_for_archival(
        &self,
        _namespace: &str,
        _cutoff: &str,
        _limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        if let Some(ref f) = *self.fallback_episodes_for_archival.lock().unwrap() {
            return f("");
        }
        Ok(vec![])
    }

    async fn select_active_facts_by_episode(
        &self,
        _namespace: &str,
        _episode_id: &str,
        _cutoff: &str,
        _limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        if let Some(ref f) = *self.fallback_active_facts_by_episode.lock().unwrap() {
            return f("");
        }
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
        if let Some(ref f) = *self.fallback_episodes_by_content.lock().unwrap() {
            return f("");
        }
        Ok(vec![])
    }

    async fn select_communities_matching_summary(
        &self,
        _namespace: &str,
        query: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        if let Some(resp) = self
            .communities_matching_summary_responses
            .lock()
            .unwrap()
            .get(query)
            .cloned()
        {
            return resp;
        }
        Ok(vec![])
    }

    async fn select_communities_by_member_entities(
        &self,
        _namespace: &str,
        member_entities: &[String],
    ) -> Result<Vec<Value>, MemoryError> {
        let key = member_entities.join(",");
        if let Some(resp) = self
            .communities_by_members_responses
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
        {
            return resp;
        }
        Ok(vec![])
    }

    async fn relate_edge(
        &self,
        _namespace: &str,
        edge_id: &str,
        _from_id: &str,
        _to_id: &str,
        _content: Value,
    ) -> Result<Value, MemoryError> {
        if let Some(resp) = self
            .relate_edge_responses
            .lock()
            .unwrap()
            .get(edge_id)
            .cloned()
        {
            return resp;
        }
        Ok(Value::Null)
    }

    async fn create(
        &self,
        record_id: &str,
        _content: Value,
        _namespace: &str,
    ) -> Result<Value, MemoryError> {
        if let Some(resp) = self
            .create_responses
            .lock()
            .unwrap()
            .get(record_id)
            .cloned()
        {
            return resp;
        }
        if let Some(ref f) = *self.fallback_create.lock().unwrap() {
            return f();
        }
        Ok(Value::Null)
    }

    async fn update(
        &self,
        record_id: &str,
        _content: Value,
        _namespace: &str,
    ) -> Result<Value, MemoryError> {
        if let Some(resp) = self
            .update_responses
            .lock()
            .unwrap()
            .get(record_id)
            .cloned()
        {
            return resp;
        }
        if let Some(ref f) = *self.fallback_update.lock().unwrap() {
            return f();
        }
        Ok(Value::Null)
    }

    async fn query(
        &self,
        sql: &str,
        _vars: Option<Value>,
        _namespace: &str,
    ) -> Result<Value, MemoryError> {
        for (prefix, result) in self.query_responses.lock().unwrap().iter() {
            if sql.starts_with(prefix.as_str()) {
                return result.clone();
            }
        }
        if let Some(ref f) = *self.fallback_query.lock().unwrap() {
            return f();
        }
        Ok(Value::Null)
    }

    async fn select_entities_by_ids(
        &self,
        _namespace: &str,
        entity_ids: &[String],
    ) -> Result<Vec<Value>, MemoryError> {
        let key = entity_ids.join(",");
        if let Some(resp) = self
            .entities_by_ids_responses
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
        {
            return resp;
        }
        Ok(vec![])
    }

    async fn select_facts_by_triple(
        &self,
        _namespace: &str,
        _query_text: &str,
        _cutoff: &str,
        _limit: usize,
    ) -> Result<Vec<Value>, MemoryError> {
        Ok(vec![])
    }

    async fn count_facts_needing_reembed(
        &self,
        _namespace: &str,
        _target_signature: &str,
    ) -> Result<usize, MemoryError> {
        Ok(0)
    }

    async fn select_facts_needing_reembed(
        &self,
        _namespace: &str,
        _target_signature: &str,
        _last_completed_fact_id: Option<&str>,
        _limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        Ok(vec![])
    }

    async fn select_edges_for_triple(
        &self,
        _namespace: &str,
        _in_id: &str,
        _relation: &str,
        _out_id: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        Ok(vec![])
    }

    async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
        self.migration_result.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn mock_db_client_defaults_to_empty() {
        let db = MockDbClient::new();
        assert_eq!(db.select_one("test", "org").await.unwrap(), None);
        assert!(db.select_table("test", "org").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn mock_db_client_returns_expected_values() {
        let db = MockDbClient::new()
            .expect_select_one("episode:1", Some(json!({"episode_id": "episode:1"})))
            .expect_create("fact:1", json!({"status": "ok"}));

        let result = db.select_one("episode:1", "org").await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap()["episode_id"], "episode:1");

        let result = db.create("fact:1", json!({}), "org").await.unwrap();
        assert_eq!(result["status"], "ok");
    }

    #[tokio::test]
    async fn mock_db_client_fallback_works() {
        let db =
            MockDbClient::new().expect_select_one_with(|_id| Ok(Some(json!({"fallback": true}))));

        let result = db.select_one("any:id", "org").await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap()["fallback"], true);
    }
}
