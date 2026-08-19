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
use crate::storage::DbClient;

type SelectOneFn = dyn Fn(&str) -> Result<Option<Value>, MemoryError> + Send + Sync;
type SelectTableFn = dyn Fn(&str) -> Result<Vec<Value>, MemoryError> + Send + Sync;

type CreateFn = dyn Fn() -> Result<Value, MemoryError> + Send + Sync;
type UpdateFn = dyn Fn() -> Result<Value, MemoryError> + Send + Sync;

/// Configurable mock database client for tests.
///
/// By default, every method returns `Ok(vec![])` or `Ok(None)`.
/// Use the `expect_*` builder methods to override specific calls.
pub struct MockDbClient {
    select_one_responses: Mutex<HashMap<String, Result<Option<Value>, MemoryError>>>,
    select_table_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,

    create_responses: Mutex<HashMap<String, Result<Value, MemoryError>>>,
    update_responses: Mutex<HashMap<String, Result<Value, MemoryError>>>,

    migration_result: Mutex<Result<(), MemoryError>>,
    fallback_select_one: Mutex<Option<Box<SelectOneFn>>>,
    fallback_select_table: Mutex<Option<Box<SelectTableFn>>>,

    fallback_create: Mutex<Option<Box<CreateFn>>>,
    fallback_update: Mutex<Option<Box<UpdateFn>>>,
}

impl MockDbClient {
    pub fn new() -> Self {
        Self {
            select_one_responses: Mutex::new(HashMap::new()),
            select_table_responses: Mutex::new(HashMap::new()),

            create_responses: Mutex::new(HashMap::new()),
            update_responses: Mutex::new(HashMap::new()),

            migration_result: Mutex::new(Ok(())),
            fallback_select_one: Mutex::new(None),
            fallback_select_table: Mutex::new(None),

            fallback_create: Mutex::new(None),
            fallback_update: Mutex::new(None),
        }
    }

    pub fn expect_select_one(self, record_id: &str, result: Option<Value>) -> Self {
        self.select_one_responses
            .lock()
            .unwrap_or_else(|p| p.into_inner())
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
            .unwrap_or_else(|p| p.into_inner())
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
            .unwrap_or_else(|p| p.into_inner())
            .insert(record_id.to_string(), Ok(result));
        self
    }

    pub fn expect_select_table(self, table: &str, rows: Vec<Value>) -> Self {
        self.select_table_responses
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(table.to_string(), Ok(rows));
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

    pub fn expect_migration_handler(
        mut self,
        f: impl Fn(&str) -> Result<(), MemoryError> + Send + Sync + 'static,
    ) -> Self {
        self.migration_result = Mutex::new(Ok(()));
        let _ = f;
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
            .unwrap_or_else(|p| p.into_inner())
            .get(record_id)
            .cloned()
        {
            return resp;
        }
        if let Some(ref f) = *self
            .fallback_select_one
            .lock()
            .unwrap_or_else(|p| p.into_inner())
        {
            return f(record_id);
        }
        Ok(None)
    }

    async fn select_table(&self, table: &str, _namespace: &str) -> Result<Vec<Value>, MemoryError> {
        if let Some(resp) = self
            .select_table_responses
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(table)
            .cloned()
        {
            return resp;
        }
        if let Some(ref f) = *self
            .fallback_select_table
            .lock()
            .unwrap_or_else(|p| p.into_inner())
        {
            return f(table);
        }
        Ok(vec![])
    }

    #[allow(clippy::too_many_arguments)]
    async fn create(
        &self,
        record_id: &str,
        _content: Value,
        _namespace: &str,
    ) -> Result<Value, MemoryError> {
        if let Some(resp) = self
            .create_responses
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(record_id)
            .cloned()
        {
            return resp;
        }
        if let Some(ref f) = *self
            .fallback_create
            .lock()
            .unwrap_or_else(|p| p.into_inner())
        {
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
            .unwrap_or_else(|p| p.into_inner())
            .get(record_id)
            .cloned()
        {
            return resp;
        }
        if let Some(ref f) = *self
            .fallback_update
            .lock()
            .unwrap_or_else(|p| p.into_inner())
        {
            return f();
        }
        Ok(Value::Null)
    }

    async fn query(
        &self,
        _sql: &str,
        _vars: Option<Value>,
        _namespace: &str,
    ) -> Result<Value, MemoryError> {
        Ok(Value::Null)
    }

    async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
        self.migration_result
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
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
