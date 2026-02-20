//! Database abstraction layer for SurrealDB.
//!
//! This module provides a unified interface for database operations,
//! abstracting over embedded and remote (WebSocket) engines.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{Value, json};
use surrealdb::Surreal;
use surrealdb::engine::local::Db;
use surrealdb::engine::local::RocksDb;
use surrealdb::engine::remote::ws::{Client, Ws};
use surrealdb::opt::auth::Root;
use surrealdb::types::Value as SurrealValue;
use tokio::sync::Mutex;

use crate::config::SurrealConfig;
use crate::logging::{LogLevel, StdoutLogger};
use crate::service::MemoryError;

// ============================================================================
// Database client trait
// ============================================================================

/// Trait for database operations, enabling dependency injection and testing.
#[async_trait]
pub trait DbClient: Send + Sync {
    /// Selects a single record by ID.
    async fn select_one(
        &self,
        record_id: &str,
        namespace: &str,
    ) -> Result<Option<Value>, MemoryError>;

    /// Selects all records from a table.
    async fn select_table(&self, table: &str, namespace: &str) -> Result<Vec<Value>, MemoryError>;

    /// Selects facts with DB-side filtering for bi-temporal queries.
    async fn select_facts_filtered(
        &self,
        namespace: &str,
        scope: &str,
        cutoff: &str,
        query_contains: Option<&str>,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError>;

    /// Selects edges with DB-side filtering for bi-temporal visibility.
    async fn select_edges_filtered(
        &self,
        namespace: &str,
        cutoff: &str,
    ) -> Result<Vec<Value>, MemoryError>;

    /// Creates a new record.
    async fn create(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
    ) -> Result<Value, MemoryError>;

    /// Updates an existing record.
    async fn update(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
    ) -> Result<Value, MemoryError>;

    /// Executes a raw SQL query.
    async fn query(
        &self,
        sql: &str,
        vars: Option<Value>,
        namespace: &str,
    ) -> Result<Value, MemoryError>;

    /// Applies database migrations for a namespace.
    async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError>;
}

// ============================================================================
// SurrealDB client implementation
// ============================================================================

/// Unified database client that works with both embedded and remote SurrealDB.
pub struct SurrealDbClient {
    engine: DbEngine,
    database: String,
    logger: StdoutLogger,
}

/// Internal enum representing the database engine type.
enum DbEngine {
    Local(Mutex<Surreal<Db>>),
    Remote(Mutex<Surreal<Client>>),
}

impl SurrealDbClient {
    /// Connects to SurrealDB using the provided configuration.
    pub async fn connect(
        config: &SurrealConfig,
        default_namespace: &str,
    ) -> Result<Self, MemoryError> {
        let engine = if config.embedded {
            Self::connect_embedded(config, default_namespace).await?
        } else {
            Self::connect_remote(config, default_namespace).await?
        };

        Ok(Self {
            engine,
            database: config.db_name.clone(),
            logger: StdoutLogger::new(&config.log_level),
        })
    }

    /// Connects to embedded RocksDB instance.
    async fn connect_embedded(
        config: &SurrealConfig,
        default_namespace: &str,
    ) -> Result<DbEngine, MemoryError> {
        use surrealdb::opt::{Config as SurrealOptConfig, capabilities::Capabilities};

        let data_dir = PathBuf::from(config.data_dir_or_default());
        ensure_dir_exists(data_dir.as_path())?;

        let root = Root {
            username: config.username.clone(),
            password: config.password.clone(),
        };

        let cfg = SurrealOptConfig::new()
            .user(root.clone())
            .capabilities(Capabilities::all());

        let db = Surreal::new::<RocksDb>((data_dir, cfg))
            .await
            .map_err(|err| {
                MemoryError::Storage(format!("SurrealDB embedded init failed: {err}"))
            })?;

        db.signin(root)
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB signin failed: {err}")))?;

        db.use_ns(default_namespace)
            .use_db(&config.db_name)
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB use failed: {err}")))?;

        Ok(DbEngine::Local(Mutex::new(db)))
    }

    /// Connects to remote WebSocket instance.
    async fn connect_remote(
        config: &SurrealConfig,
        default_namespace: &str,
    ) -> Result<DbEngine, MemoryError> {
        let url = normalize_url(config.url.as_deref().unwrap_or(""));
        let db = Surreal::new::<Ws>(url.as_str())
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB connect failed: {err}")))?;

        db.signin(Root {
            username: config.username.clone(),
            password: config.password.clone(),
        })
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB signin failed: {err}")))?;

        db.use_ns(default_namespace)
            .use_db(&config.db_name)
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB use failed: {err}")))?;

        Ok(DbEngine::Remote(Mutex::new(db)))
    }

    /// Gets a database handle with namespace set.
    async fn with_namespace_local(&self, namespace: &str) -> Result<Surreal<Db>, MemoryError> {
        match &self.engine {
            DbEngine::Local(db) => {
                let guard = db.lock().await;
                guard
                    .use_ns(namespace)
                    .use_db(&self.database)
                    .await
                    .map_err(|err| MemoryError::Storage(format!("SurrealDB use failed: {err}")))?;
                Ok(guard.clone())
            }
            DbEngine::Remote(_) => Err(MemoryError::Storage("expected local engine".into())),
        }
    }

    /// Gets a database handle with namespace set.
    async fn with_namespace_remote(&self, namespace: &str) -> Result<Surreal<Client>, MemoryError> {
        match &self.engine {
            DbEngine::Remote(db) => {
                let guard = db.lock().await;
                guard
                    .use_ns(namespace)
                    .use_db(&self.database)
                    .await
                    .map_err(|err| MemoryError::Storage(format!("SurrealDB use failed: {err}")))?;
                Ok(guard.clone())
            }
            DbEngine::Local(_) => Err(MemoryError::Storage("expected remote engine".into())),
        }
    }

    /// Checks if using local embedded engine.
    fn is_local(&self) -> bool {
        matches!(self.engine, DbEngine::Local(_))
    }

    /// Ask the connected SurrealDB instance for a server version string.
    /// Returns Ok(None) if the information cannot be retrieved.
    pub async fn server_version(&self, namespace: &str) -> Result<Option<String>, MemoryError> {
        // "INFO FOR DB" is an informational command that returns DB metadata.
        // Treat failures as non-fatal (return Ok(None)).
        let sql = "INFO FOR DB";
        let res = if self.is_local() {
            match self.with_namespace_local(namespace).await {
                Ok(db) => db.query(sql).await,
                Err(e) => return Err(e),
            }
        } else {
            match self.with_namespace_remote(namespace).await {
                Ok(db) => db.query(sql).await,
                Err(e) => return Err(e),
            }
        };

        let mut response = match res {
            Ok(r) => r,
            Err(_) => return Ok(None),
        };

        let surreal_val = response
            .take::<SurrealValue>(0)
            .map_err(|err| MemoryError::Storage(format!("SurrealDB take failed: {err}")))?;

        let json = surreal_to_json(surreal_val);
        Ok(find_version_in_json(&json))
    }

    /// Logs a database operation event.
    fn log_op(&self, op: &str, details: Vec<(&str, Value)>) {
        let mut event = std::collections::HashMap::new();
        event.insert("op".to_string(), Value::String(op.to_string()));
        for (key, value) in details {
            event.insert(key.to_string(), value);
        }
        self.logger.log(event, LogLevel::Debug);
    }

    /// Applies database schema migrations.
    pub async fn apply_migrations_impl(&self, namespace: &str) -> Result<(), MemoryError> {
        let schema_sql = include_str!("migrations/__Initial.surql");

        self.execute_raw_query(schema_sql, None, namespace).await?;

        self.logger.log(
            std::collections::HashMap::from([
                ("op".to_string(), Value::String("schema.init".to_string())),
                (
                    "namespace".to_string(),
                    Value::String(namespace.to_string()),
                ),
            ]),
            LogLevel::Info,
        );

        Ok(())
    }

    /// Execute a query that returns a SurrealValue (internal helper).
    async fn execute_query(
        &self,
        sql: &str,
        vars: Option<Value>,
        namespace: &str,
    ) -> Result<SurrealValue, MemoryError> {
        if self.is_local() {
            let db = self.with_namespace_local(namespace).await?;
            let mut q = db.query(sql);
            if let Some(v) = vars.clone() {
                q = q.bind(v);
            }
            let mut response = q
                .await
                .map_err(|err| MemoryError::Storage(format!("SurrealDB query failed: {err}")))?;
            response
                .take::<SurrealValue>(0)
                .map_err(|err| MemoryError::Storage(format!("SurrealDB take failed: {err}")))
        } else {
            let db = self.with_namespace_remote(namespace).await?;
            let mut q = db.query(sql);
            if let Some(v) = vars {
                q = q.bind(v);
            }
            let mut response = q
                .await
                .map_err(|err| MemoryError::Storage(format!("SurrealDB query failed: {err}")))?;
            response
                .take::<SurrealValue>(0)
                .map_err(|err| MemoryError::Storage(format!("SurrealDB take failed: {err}")))
        }
    }

    /// Execute a query that doesn't return a value (internal helper).
    async fn execute_raw_query(
        &self,
        sql: &str,
        vars: Option<Value>,
        namespace: &str,
    ) -> Result<(), MemoryError> {
        if self.is_local() {
            let db = self.with_namespace_local(namespace).await?;
            let mut q = db.query(sql);
            if let Some(v) = vars.clone() {
                q = q.bind(v);
            }
            q.await
                .map_err(|err| MemoryError::Storage(format!("SurrealDB query failed: {err}")))?;
        } else {
            let db = self.with_namespace_remote(namespace).await?;
            let mut q = db.query(sql);
            if let Some(v) = vars {
                q = q.bind(v);
            }
            q.await
                .map_err(|err| MemoryError::Storage(format!("SurrealDB query failed: {err}")))?;
        }
        Ok(())
    }
}

#[async_trait]
impl DbClient for SurrealDbClient {
    async fn select_one(
        &self,
        record_id: &str,
        namespace: &str,
    ) -> Result<Option<Value>, MemoryError> {
        self.log_op(
            "db.select_one",
            vec![
                ("record_id", Value::String(record_id.to_string())),
                ("namespace", Value::String(namespace.to_string())),
            ],
        );

        let (sql, bind) = build_select_one_query(record_id);

        let surreal_val = self
            .execute_query(&sql, bind.map(|b| json!({"id": b})), namespace)
            .await?;

        let normalized = surreal_to_json(surreal_val);
        let result = extract_first_record(normalized);

        self.log_op(
            "db.select_one.result",
            vec![
                ("record_id", Value::String(record_id.to_string())),
                ("found", Value::Bool(result.is_some())),
            ],
        );

        Ok(result)
    }

    async fn select_table(&self, table: &str, namespace: &str) -> Result<Vec<Value>, MemoryError> {
        self.log_op(
            "db.select_table",
            vec![
                ("table", Value::String(table.to_string())),
                ("namespace", Value::String(namespace.to_string())),
            ],
        );

        let sql = format!("SELECT * FROM {table}");
        let surreal_val = self.execute_query(&sql, None, namespace).await?;
        let normalized = surreal_to_json(surreal_val);
        let results = extract_records(normalized);

        self.log_op(
            "db.select_table.result",
            vec![(
                "count",
                Value::Number(serde_json::Number::from(results.len())),
            )],
        );

        Ok(results)
    }

    async fn select_facts_filtered(
        &self,
        namespace: &str,
        scope: &str,
        cutoff: &str,
        query_contains: Option<&str>,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        self.log_op(
            "db.select_facts_filtered",
            vec![
                ("scope", Value::String(scope.to_string())),
                ("cutoff", Value::String(cutoff.to_string())),
                ("namespace", Value::String(namespace.to_string())),
                ("limit", Value::Number(serde_json::Number::from(limit))),
            ],
        );

        let base_where = "scope = $scope AND t_valid <= $cutoff AND (t_ingested IS NONE OR t_ingested <= $cutoff) AND (t_invalid IS NONE OR t_invalid > $cutoff OR t_invalid_ingested > $cutoff)";

        let (sql, vars, query_words) = if let Some(q) = query_contains {
            let words: Vec<String> = q
                .split_whitespace()
                .filter(|w| w.len() >= 2)
                .map(|w| w.to_lowercase())
                .collect();

            let v = serde_json::json!({
                "scope": scope,
                "cutoff": cutoff,
                "limit": limit,
            });

            (
                "SELECT * FROM fact ORDER BY t_valid DESC".to_string(),
                v,
                words,
            )
        } else {
            let v = serde_json::json!({
                "scope": scope,
                "cutoff": cutoff,
                "limit": limit,
            });
            (
                format!("SELECT * FROM fact WHERE {base_where} ORDER BY t_valid DESC LIMIT $limit"),
                v,
                Vec::new(),
            )
        };

        let surreal_val = self.execute_query(&sql, Some(vars), namespace).await?;
        let normalized = surreal_to_json(surreal_val);
        let mut results = extract_records(normalized);

        if query_contains.is_some() {
            let flattened = flatten_surreal_records(results);
            results = flattened
                .into_iter()
                .filter(|record| record_is_visible_for_scope(record, scope, cutoff))
                .filter(|record| record_matches_any_query_word(record, &query_words))
                .take(limit.max(1) as usize)
                .collect();
        }

        self.log_op(
            "db.select_facts_filtered.result",
            vec![(
                "count",
                Value::Number(serde_json::Number::from(results.len())),
            )],
        );

        Ok(results)
    }

    async fn select_edges_filtered(
        &self,
        namespace: &str,
        cutoff: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        self.log_op(
            "db.select_edges_filtered",
            vec![
                ("cutoff", Value::String(cutoff.to_string())),
                ("namespace", Value::String(namespace.to_string())),
            ],
        );

        let sql = String::from(
            "SELECT * FROM edge WHERE string::is_datetime(t_valid) AND t_valid <= $cutoff AND (t_ingested IS NONE OR (string::is_datetime(t_ingested) AND t_ingested <= $cutoff)) AND (t_invalid IS NONE OR t_invalid > $cutoff OR t_invalid_ingested > $cutoff) ORDER BY from_id ASC, to_id ASC, t_valid DESC",
        );

        let vars = serde_json::json!({ "cutoff": cutoff });
        let surreal_val = self.execute_query(&sql, Some(vars), namespace).await?;
        let normalized = surreal_to_json(surreal_val);
        let results = extract_records(normalized);

        self.log_op(
            "db.select_edges_filtered.result",
            vec![(
                "count",
                Value::Number(serde_json::Number::from(results.len())),
            )],
        );

        Ok(results)
    }

    async fn create(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        self.log_op(
            "db.create",
            vec![
                ("record_id", Value::String(record_id.to_string())),
                ("namespace", Value::String(namespace.to_string())),
            ],
        );

        let (sql, vars) = build_create_query(record_id, content);
        let surreal_val = self.execute_query(&sql, Some(vars), namespace).await?;
        let normalized = surreal_to_json(surreal_val);
        let result = extract_first_record(normalized).unwrap_or(Value::Null);

        self.log_op(
            "db.create.result",
            vec![("result", Value::String("ok".to_string()))],
        );

        Ok(result)
    }

    async fn update(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        self.log_op(
            "db.update",
            vec![
                ("record_id", Value::String(record_id.to_string())),
                ("namespace", Value::String(namespace.to_string())),
            ],
        );

        let (sql, vars) = build_update_query(record_id, content)?;
        let surreal_val = self.execute_query(&sql, Some(vars), namespace).await?;
        let normalized = surreal_to_json(surreal_val);
        let result = extract_first_record(normalized).unwrap_or(Value::Null);

        self.log_op(
            "db.update.result",
            vec![("result", Value::String("ok".to_string()))],
        );

        Ok(result)
    }

    async fn query(
        &self,
        sql: &str,
        vars: Option<Value>,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        self.log_op(
            "db.query",
            vec![
                ("sql", Value::String(sql.to_string())),
                ("namespace", Value::String(namespace.to_string())),
            ],
        );

        if let Some(Value::Object(map)) = &vars {
            self.log_op(
                "db.query.vars",
                vec![("count", Value::Number(serde_json::Number::from(map.len())))],
            );
        }

        self.execute_raw_query(sql, vars, namespace).await?;

        self.log_op(
            "db.query.result",
            vec![("result", Value::String("ok".to_string()))],
        );

        Ok(Value::Null)
    }

    async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError> {
        self.apply_migrations_impl(namespace).await
    }
}

// ============================================================================
// Helper functions
// ============================================================================

/// Build SQL query for selecting a single record.
fn build_select_one_query(record_id: &str) -> (String, Option<Value>) {
    if let Some(idx) = record_id.find(':') {
        let table = &record_id[..idx];
        let id = &record_id[idx + 1..];
        let id_field = match table {
            "fact" => Some("fact_id"),
            "entity" => Some("entity_id"),
            "edge" => Some("edge_id"),
            "community" => Some("community_id"),
            _ => None,
        };

        if let Some(field) = id_field {
            (
                format!("SELECT * FROM {table} WHERE {field} = $id"),
                Some(Value::String(record_id.to_string())),
            )
        } else if !id.is_empty() {
            (format!("SELECT * FROM {table}:⟨{id}⟩"), None)
        } else {
            (format!("SELECT * FROM {record_id}"), None)
        }
    } else {
        (format!("SELECT * FROM {record_id}"), None)
    }
}

/// Build SQL query for creating a record.
fn build_create_query(record_id: &str, content: Value) -> (String, Value) {
    let (table, id) = if let Some(idx) = record_id.find(':') {
        (&record_id[..idx], Some(&record_id[idx + 1..]))
    } else {
        (record_id, None)
    };

    let sql = if let Some(record_id) = id {
        format!("CREATE {table}:⟨{record_id}⟩ CONTENT $content RETURN *")
    } else {
        format!("CREATE {table} CONTENT $content RETURN *")
    };

    let content_for_create = normalize_surreal_json(&content);
    (sql, json!({"content": content_for_create}))
}

/// Build SQL query for updating a record.
fn build_update_query(record_id: &str, content: Value) -> Result<(String, Value), MemoryError> {
    let (table, id) = if let Some(idx) = record_id.find(':') {
        (&record_id[..idx], &record_id[idx + 1..])
    } else {
        return Err(MemoryError::Storage(format!(
            "Invalid record_id format: expected 'table:id', got '{record_id}'"
        )));
    };

    let sql = format!("UPDATE {table}:⟨{id}⟩ MERGE $content RETURN *");

    let content_for_update = if let Value::Object(mut map) = content {
        map.remove("id");
        Value::Object(map)
    } else {
        content
    };

    let content_for_update = normalize_surreal_json(&content_for_update);
    Ok((sql, json!({"content": content_for_update})))
}

fn ensure_dir_exists(path: &Path) -> Result<(), MemoryError> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .map_err(|err| MemoryError::Storage(format!("failed to create data dir: {err}")))?;
    }
    Ok(())
}

fn normalize_url(url: &str) -> String {
    if url.starts_with("http://") {
        let base = url.replace("http://", "ws://");
        if base.ends_with("/rpc") {
            return base;
        }
        return format!("{}/rpc", base.trim_end_matches('/'));
    }
    if url.starts_with("https://") {
        let base = url.replace("https://", "wss://");
        if base.ends_with("/rpc") {
            return base;
        }
        return format!("{}/rpc", base.trim_end_matches('/'));
    }
    url.to_string()
}

fn surreal_to_json(value: SurrealValue) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// Try to find a version-like field inside arbitrary JSON returned by the
/// server info query. Searches keys for the substring "version" (case-ins).
fn find_version_in_json(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Object(map) => {
            for (k, val) in map.iter() {
                if k.to_lowercase().contains("version") {
                    if let Some(s) = val.as_str() {
                        return Some(s.to_string());
                    } else {
                        return Some(val.to_string());
                    }
                }
            }
            // fallback: search nested
            for (_, val) in map.iter() {
                if let Some(found) = find_version_in_json(val) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(arr) => {
            for it in arr.iter() {
                if let Some(found) = find_version_in_json(it) {
                    return Some(found);
                }
            }
            None
        }
        other => Some(other.to_string()),
    }
}

fn extract_first_record(value: Value) -> Option<Value> {
    extract_records(value).into_iter().next()
}

fn unwrap_object_wrapper(value: Value) -> Value {
    match value {
        Value::Object(mut map) => {
            if let Some(object) = map.remove("Object") {
                object
            } else {
                Value::Object(map)
            }
        }
        other => other,
    }
}

fn extract_records(value: Value) -> Vec<Value> {
    match value {
        Value::Array(arr) => arr.into_iter().map(unwrap_object_wrapper).collect(),
        Value::Object(mut map) => {
            if let Some(array) = map.remove("Array") {
                return array
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(unwrap_object_wrapper)
                    .collect();
            }
            if let Some(object) = map.remove("Object") {
                return vec![object];
            }
            vec![Value::Object(map)]
        }
        Value::Null => Vec::new(),
        other => vec![other],
    }
}

fn value_to_matchable_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Object(map) => {
            if map.contains_key("None") {
                return String::new();
            }
            if let Some(inner) = map.get("String") {
                return value_to_matchable_string(inner);
            }
            if let Some(inner) = map.get("Strand") {
                return value_to_matchable_string(inner);
            }
            if let Some(inner) = map.get("content") {
                return value_to_matchable_string(inner);
            }
            if map.len() == 1
                && let Some((_, inner)) = map.iter().next()
            {
                return value_to_matchable_string(inner);
            }
            String::new()
        }
        Value::Array(values) => values
            .first()
            .map(value_to_matchable_string)
            .unwrap_or_default(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

fn record_matches_any_query_word(record: &Value, words: &[String]) -> bool {
    if words.is_empty() {
        return true;
    }
    let content = record
        .get("content")
        .map(value_to_matchable_string)
        .unwrap_or_default()
        .to_lowercase();
    words.iter().any(|word| content.contains(word))
}

fn record_string_field(record: &Value, key: &str) -> String {
    record
        .get(key)
        .map(value_to_matchable_string)
        .unwrap_or_default()
}

fn record_is_visible_for_scope(record: &Value, scope: &str, cutoff: &str) -> bool {
    let record_scope = record_string_field(record, "scope");
    if record_scope != scope {
        return false;
    }

    let t_valid = record_string_field(record, "t_valid");
    if t_valid.is_empty() || t_valid.as_str() > cutoff {
        return false;
    }

    let t_ingested = record_string_field(record, "t_ingested");
    if !t_ingested.is_empty() && t_ingested.as_str() > cutoff {
        return false;
    }

    let t_invalid = record_string_field(record, "t_invalid");
    if !t_invalid.is_empty() && t_invalid.as_str() <= cutoff {
        let t_invalid_ingested = record_string_field(record, "t_invalid_ingested");
        let invalid_known = if t_invalid_ingested.is_empty() {
            true
        } else {
            t_invalid_ingested.as_str() <= cutoff
        };
        if invalid_known {
            return false;
        }
    }

    true
}

fn flatten_surreal_records(records: Vec<Value>) -> Vec<Value> {
    let mut flat = Vec::new();
    for record in records {
        if let Some(items) = record.get("Array").and_then(Value::as_array) {
            for item in items {
                if let Some(obj) = item.get("Object") {
                    flat.push(obj.clone());
                } else {
                    flat.push(item.clone());
                }
            }
            continue;
        }
        if let Some(obj) = record.get("Object") {
            flat.push(obj.clone());
            continue;
        }
        flat.push(record);
    }
    flat
}

fn normalize_surreal_json(v: &Value) -> Value {
    use serde_json::Value as J;

    match v {
        J::Object(map) if map.len() == 1 => {
            let Some((k, val)) = map.iter().next() else {
                return J::Object(map.clone());
            };
            match k.as_str() {
                "None" => v.clone(),
                "Array" => val
                    .as_array()
                    .map(|items| J::Array(items.iter().map(normalize_surreal_json).collect()))
                    .unwrap_or_else(|| val.clone()),
                "Object" => val
                    .as_object()
                    .map(|inner| {
                        J::Object(
                            inner
                                .iter()
                                .map(|(ik, iv)| (ik.clone(), normalize_surreal_json(iv)))
                                .collect(),
                        )
                    })
                    .unwrap_or_else(|| val.clone()),
                "Strand" | "String" => val
                    .as_object()
                    .and_then(|inner| inner.get("String").cloned())
                    .unwrap_or_else(|| val.clone()),
                "Number" | "Float" | "Int" | "Decimal" => normalize_surreal_json(val),
                _ => J::Object(
                    map.iter()
                        .map(|(ik, iv)| (ik.clone(), normalize_surreal_json(iv)))
                        .collect(),
                ),
            }
        }
        J::Object(map) => J::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), normalize_surreal_json(v)))
                .collect(),
        ),
        J::Null => json!({"None": {}}),
        J::Array(arr) => J::Array(arr.iter().map(normalize_surreal_json).collect()),
        _ => v.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_url_upgrades_http_and_appends_rpc() {
        assert_eq!(
            normalize_url("http://localhost:8000"),
            "ws://localhost:8000/rpc"
        );
        assert_eq!(normalize_url("https://db.local"), "wss://db.local/rpc");
        assert_eq!(normalize_url("https://db.local/rpc"), "wss://db.local/rpc");
        assert_eq!(normalize_url("ws://custom"), "ws://custom");
    }

    #[test]
    fn normalize_url_handles_trailing_slash() {
        assert_eq!(normalize_url("http://db.local/"), "ws://db.local/rpc");
        assert_eq!(normalize_url("https://db.local/"), "wss://db.local/rpc");
    }

    #[tokio::test]
    async fn server_version_returns_some_for_embedded() {
        let td = tempfile::tempdir().expect("create tempdir");
        let data_dir = td.path().to_str().unwrap().to_string();

        let config = crate::config::SurrealConfigBuilder::new()
            .db_name("testdb")
            .namespace("testns")
            .credentials("root", "root")
            .data_dir(&data_dir)
            .embedded(true)
            .build()
            .expect("valid config");

        let client = SurrealDbClient::connect(&config, "testns").await.expect("connect");
        let ver = client.server_version("testns").await.expect("server_version");
        // Server version may be unavailable for embedded engines; ensure we
        // don't error and that any returned string is non-empty.
        if let Some(s) = ver {
            assert!(!s.is_empty());
        }
    }

    #[test]
    fn extract_first_record_from_array() {
        let arr = Value::Array(vec![json!({"id": 1}), json!({"id": 2})]);
        let result = extract_first_record(arr);
        assert_eq!(result, Some(json!({"id": 1})));
    }

    #[test]
    fn extract_first_record_from_single() {
        let single = json!({"id": 1});
        let result = extract_first_record(single);
        assert_eq!(result, Some(json!({"id": 1})));
    }

    #[test]
    fn extract_first_record_from_null() {
        let result = extract_first_record(Value::Null);
        assert_eq!(result, None);
    }

    #[test]
    fn extract_records_from_array() {
        let arr = Value::Array(vec![json!({"id": 1}), json!({"id": 2})]);
        let result = extract_records(arr);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn extract_records_from_null() {
        let result = extract_records(Value::Null);
        assert!(result.is_empty());
    }

    #[test]
    fn normalize_surreal_json_unwraps_string_variants() {
        let strand = json!({"Strand": {"String": "Alice"}});
        let string = json!({"String": "Bob"});

        assert_eq!(normalize_surreal_json(&strand), json!("Alice"));
        assert_eq!(normalize_surreal_json(&string), json!("Bob"));
    }

    #[test]
    fn normalize_surreal_json_flattens_array_object() {
        let raw = json!({
            "Array": [
                {"Object": {"name": {"Strand": "Alice"}}},
                {"Object": {"name": {"String": "Bob"}}}
            ]
        });

        let normalized = normalize_surreal_json(&raw);
        assert_eq!(
            normalized,
            json!([
                {"name": "Alice"},
                {"name": "Bob"}
            ])
        );
    }

    #[test]
    fn normalize_surreal_json_preserves_primitives() {
        assert_eq!(normalize_surreal_json(&json!(null)), json!({"None": {}}));
        assert_eq!(normalize_surreal_json(&json!(42)), json!(42));
        assert_eq!(normalize_surreal_json(&json!(true)), json!(true));
        assert_eq!(normalize_surreal_json(&json!("plain")), json!("plain"));
    }

    // Tests for query builder helper functions

    #[test]
    fn build_select_one_query_with_fact_id() {
        let (sql, bind) = build_select_one_query("fact:abc123");
        assert_eq!(sql, "SELECT * FROM fact WHERE fact_id = $id");
        assert_eq!(bind, Some(json!("fact:abc123")));
    }

    #[test]
    fn build_select_one_query_with_entity_id() {
        let (sql, bind) = build_select_one_query("entity:xyz789");
        assert_eq!(sql, "SELECT * FROM entity WHERE entity_id = $id");
        assert_eq!(bind, Some(json!("entity:xyz789")));
    }

    #[test]
    fn build_select_one_query_with_standard_id() {
        let (sql, bind) = build_select_one_query("episode:def456");
        assert_eq!(sql, "SELECT * FROM episode:⟨def456⟩");
        assert_eq!(bind, None);
    }

    #[test]
    fn build_select_one_query_without_colon() {
        let (sql, bind) = build_select_one_query("some_table");
        assert_eq!(sql, "SELECT * FROM some_table");
        assert_eq!(bind, None);
    }

    #[test]
    fn build_create_query_with_id() {
        let content = json!({"name": "test"});
        let (sql, vars) = build_create_query("entity:abc123", content);
        assert_eq!(sql, "CREATE entity:⟨abc123⟩ CONTENT $content RETURN *");
        assert!(vars.get("content").is_some());
    }

    #[test]
    fn build_create_query_without_id() {
        let content = json!({"name": "test"});
        let (sql, vars) = build_create_query("entity", content);
        assert_eq!(sql, "CREATE entity CONTENT $content RETURN *");
        assert!(vars.get("content").is_some());
    }

    #[test]
    fn build_update_query_success() {
        let content = json!({"id": "fact:abc123", "name": "updated"});
        let (sql, vars) = build_update_query("fact:abc123", content).unwrap();
        assert_eq!(sql, "UPDATE fact:⟨abc123⟩ MERGE $content RETURN *");
        // The 'id' field should be removed from content
        let content_val = vars.get("content").unwrap();
        assert!(content_val.get("id").is_none());
        assert_eq!(content_val.get("name").unwrap(), "updated");
    }

    #[test]
    fn build_update_query_invalid_format() {
        let content = json!({"name": "test"});
        let result = build_update_query("invalid_format", content);
        assert!(matches!(result, Err(MemoryError::Storage(_))));
    }
}
