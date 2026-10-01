//! Mock database client for tests, eliminating boilerplate from hand-written mocks.
//!
//! Usage in tests:
//! ```rust,no_run
//! let db = MockDbClient::new()
//!     .expect_select_one("episode:test", Some(json!({"episode_id": "episode:test", "content": "hello"})))
//!     .expect_create("fact:1", json!({"status": "ok"}));
//! let service = MemoryService::new(Arc::new(db), "org".to_string(), "warn".to_string(), 50, 100).unwrap();
//! ```

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::MemoryError;
use crate::storage::DbClient;
use crate::storage::table_scope::ReleaseOwnedTable;

type SelectOneFn = dyn Fn(&str) -> Result<Option<Value>, MemoryError> + Send + Sync;
type SelectTableFn = dyn Fn(&str) -> Result<Vec<Value>, MemoryError> + Send + Sync;
type QueryResponderFn = dyn Fn(&str, Option<&Value>) -> Result<Value, MemoryError> + Send + Sync;

/// One scripted answer to a `query`.
///
/// The predicate and the responder are separate rather than one closure with
/// an internal `match`, because most callers branch on three or four SQL
/// shapes and want an unmatched one to fall through to the next rule instead
/// of panicking inside a single handler. A test that panics with the SQL
/// printed is a test that fails on the right assertion; a test that panics
/// inside rule two because rule one did not match is a test that lies.
struct QueryRule {
    matches: Box<dyn Fn(&str) -> bool + Send + Sync>,
    respond: Box<QueryResponderFn>,
}

type CreateFn = dyn Fn() -> Result<Value, MemoryError> + Send + Sync;
type UpdateFn = dyn Fn() -> Result<Value, MemoryError> + Send + Sync;

/// Configurable mock database client for tests.
///
/// By default, every method returns `Ok(vec![])`, `Ok(None)` or
/// `Ok(Value::Null)`. Use the `expect_*` builder methods to override specific
/// calls.
pub struct MockDbClient {
    select_one_responses: Mutex<HashMap<String, Result<Option<Value>, MemoryError>>>,
    select_table_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,

    create_responses: Mutex<HashMap<String, Result<Value, MemoryError>>>,
    update_responses: Mutex<HashMap<String, Result<Value, MemoryError>>>,

    query_rules: Mutex<Vec<QueryRule>>,
    no_query: Mutex<bool>,
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

            query_rules: Mutex::new(Vec::new()),
            no_query: Mutex::new(false),
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

    /// Script `query` by SQL shape, passing the bound variables to the
    /// responder as well.
    ///
    /// The responder sees both, which is what the retrieval tests need: they
    /// key on `vars["query"]` and `vars["node_id"]` rather than on the table
    /// name, because those are the queries the production code actually
    /// dispatches on. Rules are tried in the order they were added and the
    /// first match wins; a SQL that matches none returns `Ok(Value::Null)`,
    /// which is the same default an unconfigured client gives.
    pub fn expect_query_with(
        self,
        matches: impl Fn(&str) -> bool + Send + Sync + 'static,
        respond: impl Fn(&str, Option<&Value>) -> Result<Value, MemoryError> + Send + Sync + 'static,
    ) -> Self {
        self.query_rules
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(QueryRule {
                matches: Box::new(matches),
                respond: Box::new(respond),
            });
        self
    }

    /// Answer one exact SQL string.
    ///
    /// A convenience over [`MockDbClient::expect_query_with`] for the common
    /// case where the test knows the whole statement. Prefer the general form
    /// when the query carries a timestamp or an id that varies per run.
    pub fn expect_query(self, sql: &str, result: Value) -> Self {
        let expected = sql.to_string();
        self.expect_query_with(
            move |actual| actual == expected,
            move |_, _| Ok(result.clone()),
        )
    }

    /// Panic if any query reaches `query`, naming the SQL.
    ///
    /// The counterpart to `expect_select_table_panic` for the other read: a
    /// test that asserts a path takes no query at all. This module is
    /// `#[cfg(test)]`-only, so a `panic!` here cannot reach production.
    pub fn expect_no_query(self) -> Self {
        *self.no_query.lock().unwrap_or_else(|p| p.into_inner()) = true;
        self
    }

    /// Answer `select_table` from a closure, for a test whose rows depend on
    /// which table was asked for.
    pub fn expect_select_table_with(
        self,
        f: impl Fn(&str) -> Result<Vec<Value>, MemoryError> + Send + Sync + 'static,
    ) -> Self {
        *self
            .fallback_select_table
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(Box::new(f));
        self
    }

    /// Panic if `select_table` is called for `table_name`, naming the table
    /// actually asked for.
    ///
    /// Used by the semantic-retrieval test, which asserts the provider-backed
    /// path takes the indexed vector query rather than scanning `fact`.
    pub fn expect_select_table_panic(self, table_name: &str) -> Self {
        let table_name = table_name.to_string();
        self.expect_select_table_with(move |table| {
            panic!("select_table should not be called for {table_name}, got {table}");
        })
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

    async fn select_table(
        &self,
        table: crate::storage::table_scope::OwnedTable,
        _namespace: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        let table = table.as_str();
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
        _temporal_fields: &[&str],
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
        _temporal_fields: &[&str],
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
        sql: &str,
        vars: Option<Value>,
        _namespace: &str,
    ) -> Result<Value, MemoryError> {
        if *self.no_query.lock().unwrap_or_else(|p| p.into_inner()) {
            panic!("query should not be called, got {sql}");
        }
        let rules = self.query_rules.lock().unwrap_or_else(|p| p.into_inner());
        for rule in rules.iter() {
            if (rule.matches)(sql) {
                return (rule.respond)(sql, vars.as_ref());
            }
        }
        Ok(Value::Null)
    }

    /// Always succeeds. `expect_migration_result` was cut in the 2026-09-30
    /// consolidation because no test set it: the one migration failure a test
    /// wants to exercise happens against a real database, where
    /// `apply_migrations` really runs the scripts. A mock that cannot fail
    /// where no test needed it to is a knob, not a seam.
    async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
        Ok(())
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
        // `query_log`, not a made-up name: `ReleaseOwnedTable::table`
        // carries a `debug_assert` that the context declared the table, and a
        // test that spelled a table nobody owns trips it — which is what
        // happened the first time.
        assert!(
            db.select_table(
                crate::storage::table_scope::PlatformTables::table("query_log"),
                "org",
            )
            .await
            .unwrap()
            .is_empty()
        );
    }

    #[tokio::test]
    async fn mock_db_client_returns_expected_values() {
        let db = MockDbClient::new()
            .expect_select_one("episode:1", Some(json!({"episode_id": "episode:1"})))
            .expect_create("fact:1", json!({"status": "ok"}));

        let result = db.select_one("episode:1", "org").await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap()["episode_id"], "episode:1");

        let result = db.create("fact:1", json!({}), "org", &[]).await.unwrap();
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

    /// The retrieval fakes this seam was added for dispatch on the SQL and on
    /// the bound variables, not on the table name. This is the test that says
    /// both are visible and that the first matching rule wins.
    #[tokio::test]
    async fn query_dispatches_on_sql_and_vars() {
        let db = MockDbClient::new()
            .expect_query_with(
                |sql| sql.contains("search::score"),
                |_, _| Ok(json!({"kind": "score"})),
            )
            .expect_query_with(
                |sql| sql.contains("FROM community"),
                |_, vars| {
                    let node = vars
                        .and_then(|v| v.get("node_id").cloned())
                        .unwrap_or(Value::Null);
                    Ok(json!({"kind": "community", "node_id": node}))
                },
            );

        assert_eq!(
            db.query("SELECT * FROM fact WHERE search::score > $q", None, "org")
                .await
                .unwrap()["kind"],
            "score"
        );
        assert_eq!(
            db.query(
                "SELECT * FROM community",
                Some(json!({"node_id": "c:1"})),
                "org"
            )
            .await
            .unwrap()["node_id"],
            "c:1"
        );
        // Unmatched SQL falls through every rule to the default, rather than
        // panicking inside whichever rule happened to be checked last.
        assert_eq!(
            db.query("SELECT * FROM task", None, "org").await.unwrap(),
            Value::Null
        );
    }

    #[tokio::test]
    async fn an_exact_query_is_answered_without_a_predicate() {
        let db = MockDbClient::new().expect_query("SELECT * FROM task", json!({"n": 1}));
        assert_eq!(
            db.query("SELECT * FROM task", None, "org").await.unwrap()["n"],
            1
        );
        assert_eq!(
            db.query("SELECT * FROM other", None, "org").await.unwrap(),
            Value::Null
        );
    }

    #[tokio::test]
    #[should_panic(expected = "query should not be called")]
    async fn expect_no_query_fails_when_a_query_arrives() {
        let db = MockDbClient::new().expect_no_query();
        let _ = db.query("SELECT 1", None, "org").await;
    }
}
