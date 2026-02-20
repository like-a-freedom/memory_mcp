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
    pub async fn connect(config: &SurrealConfig, default_namespace: &str) -> Result<Self, MemoryError> {
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
            .map_err(|err| MemoryError::Storage(format!("SurrealDB embedded init failed: {err}")))?;

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

        match &self.engine {
            DbEngine::Local(db) => {
                let guard = db.lock().await;
                guard
                    .use_ns(namespace)
                    .use_db(&self.database)
                    .await
                    .map_err(|e| MemoryError::Storage(format!("SurrealDB use failed: {e}")))?;
                guard.query(schema_sql).await.map_err(|e| {
                    MemoryError::Storage(format!("Schema initialization failed: {e}"))
                })?;
            }
            DbEngine::Remote(db) => {
                let guard = db.lock().await;
                guard
                    .use_ns(namespace)
                    .use_db(&self.database)
                    .await
                    .map_err(|e| MemoryError::Storage(format!("SurrealDB use failed: {e}")))?;
                guard.query(schema_sql).await.map_err(|e| {
                    MemoryError::Storage(format!("Schema initialization failed: {e}"))
                })?;
            }
        }

        self.logger.log(
            std::collections::HashMap::from([
                ("op".to_string(), Value::String("schema.init".to_string())),
                ("namespace".to_string(), Value::String(namespace.to_string())),
            ]),
            LogLevel::Info,
        );

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
        self.log_op("db.select_one", vec![
            ("record_id", Value::String(record_id.to_string())),
            ("namespace", Value::String(namespace.to_string())),
        ]);

        let (sql, bind) = if let Some(idx) = record_id.find(':') {
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
        };

        let surreal_val = if self.is_local() {
            let db = self.with_namespace_local(namespace).await?;
            let mut q = db.query(&sql);
            if let Some(b) = bind.clone() {
                q = q.bind(("id", b));
            }
            let mut response = q.await.map_err(|err| {
                MemoryError::Storage(format!("SurrealDB query failed: {err}"))
            })?;
            response
                .take::<SurrealValue>(0)
                .map_err(|err| MemoryError::Storage(format!("SurrealDB take failed: {err}")))?
        } else {
            let db = self.with_namespace_remote(namespace).await?;
            let mut q = db.query(&sql);
            if let Some(b) = bind {
                q = q.bind(("id", b));
            }
            let mut response = q.await.map_err(|err| {
                MemoryError::Storage(format!("SurrealDB query failed: {err}"))
            })?;
            response
                .take::<SurrealValue>(0)
                .map_err(|err| MemoryError::Storage(format!("SurrealDB take failed: {err}")))?
        };

        let normalized = surreal_to_json(surreal_val);
        let result = extract_first_record(normalized);

        self.log_op("db.select_one.result", vec![
            ("record_id", Value::String(record_id.to_string())),
            ("found", Value::Bool(result.is_some())),
        ]);

        Ok(result)
    }

    async fn select_table(&self, table: &str, namespace: &str) -> Result<Vec<Value>, MemoryError> {
        self.log_op("db.select_table", vec![
            ("table", Value::String(table.to_string())),
            ("namespace", Value::String(namespace.to_string())),
        ]);

        let sql = format!("SELECT * FROM {table}");
        let surreal_val = if self.is_local() {
            let db = self.with_namespace_local(namespace).await?;
            let mut response = db.query(&sql).await.map_err(|err| {
                MemoryError::Storage(format!("SurrealDB query failed: {err}"))
            })?;
            response
                .take::<SurrealValue>(0)
                .map_err(|err| MemoryError::Storage(format!("SurrealDB take failed: {err}")))?
        } else {
            let db = self.with_namespace_remote(namespace).await?;
            let mut response = db.query(&sql).await.map_err(|err| {
                MemoryError::Storage(format!("SurrealDB query failed: {err}"))
            })?;
            response
                .take::<SurrealValue>(0)
                .map_err(|err| MemoryError::Storage(format!("SurrealDB take failed: {err}")))?
        };

        let normalized = surreal_to_json(surreal_val);
        let results = extract_records(normalized);

        self.log_op("db.select_table.result", vec![
            ("count", Value::Number(serde_json::Number::from(results.len()))),
        ]);

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
        self.log_op("db.select_facts_filtered", vec![
            ("scope", Value::String(scope.to_string())),
            ("cutoff", Value::String(cutoff.to_string())),
            ("namespace", Value::String(namespace.to_string())),
            ("limit", Value::Number(serde_json::Number::from(limit))),
        ]);

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

        let surreal_val = if self.is_local() {
            let db = self.with_namespace_local(namespace).await?;
            let mut response = db.query(&sql).bind(vars).await.map_err(|err| {
                MemoryError::Storage(format!("SurrealDB query failed: {err}"))
            })?;
            response
                .take::<SurrealValue>(0)
                .map_err(|err| MemoryError::Storage(format!("SurrealDB take failed: {err}")))?
        } else {
            let db = self.with_namespace_remote(namespace).await?;
            let mut response = db.query(&sql).bind(vars).await.map_err(|err| {
                MemoryError::Storage(format!("SurrealDB query failed: {err}"))
            })?;
            response
                .take::<SurrealValue>(0)
                .map_err(|err| MemoryError::Storage(format!("SurrealDB take failed: {err}")))?
        };

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

        self.log_op("db.select_facts_filtered.result", vec![
            ("count", Value::Number(serde_json::Number::from(results.len()))),
        ]);

        Ok(results)
    }

    async fn select_edges_filtered(
        &self,
        namespace: &str,
        cutoff: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        self.log_op("db.select_edges_filtered", vec![
            ("cutoff", Value::String(cutoff.to_string())),
            ("namespace", Value::String(namespace.to_string())),
        ]);

        let sql = String::from(
            "SELECT * FROM edge WHERE string::is_datetime(t_valid) AND t_valid <= $cutoff AND (t_ingested IS NONE OR (string::is_datetime(t_ingested) AND t_ingested <= $cutoff)) AND (t_invalid IS NONE OR t_invalid > $cutoff OR t_invalid_ingested > $cutoff) ORDER BY from_id ASC, to_id ASC, t_valid DESC",
        );

        let vars = serde_json::json!({ "cutoff": cutoff });

        let surreal_val = if self.is_local() {
            let db = self.with_namespace_local(namespace).await?;
            let mut response = db.query(&sql).bind(vars).await.map_err(|err| {
                MemoryError::Storage(format!("SurrealDB query failed: {err}"))
            })?;
            response
                .take::<SurrealValue>(0)
                .map_err(|err| MemoryError::Storage(format!("SurrealDB take failed: {err}")))?
        } else {
            let db = self.with_namespace_remote(namespace).await?;
            let mut response = db.query(&sql).bind(vars).await.map_err(|err| {
                MemoryError::Storage(format!("SurrealDB query failed: {err}"))
            })?;
            response
                .take::<SurrealValue>(0)
                .map_err(|err| MemoryError::Storage(format!("SurrealDB take failed: {err}")))?
        };

        let normalized = surreal_to_json(surreal_val);
        let results = extract_records(normalized);

        self.log_op("db.select_edges_filtered.result", vec![
            ("count", Value::Number(serde_json::Number::from(results.len()))),
        ]);

        Ok(results)
    }

    async fn create(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        self.log_op("db.create", vec![
            ("record_id", Value::String(record_id.to_string())),
            ("namespace", Value::String(namespace.to_string())),
        ]);

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

        let surreal_val = if self.is_local() {
            let db = self.with_namespace_local(namespace).await?;
            let mut response = db.query(&sql).bind(("content", content_for_create.clone())).await.map_err(|err| {
                MemoryError::Storage(format!("SurrealDB query failed: {err}"))
            })?;
            response
                .take::<SurrealValue>(0)
                .map_err(|err| MemoryError::Storage(format!("SurrealDB take failed: {err}")))?
        } else {
            let db = self.with_namespace_remote(namespace).await?;
            let mut response = db.query(&sql).bind(("content", content_for_create)).await.map_err(|err| {
                MemoryError::Storage(format!("SurrealDB query failed: {err}"))
            })?;
            response
                .take::<SurrealValue>(0)
                .map_err(|err| MemoryError::Storage(format!("SurrealDB take failed: {err}")))?
        };

        let normalized = surreal_to_json(surreal_val);
        let result = extract_first_record(normalized).unwrap_or(Value::Null);

        self.log_op("db.create.result", vec![
            ("result", Value::String("ok".to_string())),
        ]);

        Ok(result)
    }

    async fn update(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        self.log_op("db.update", vec![
            ("record_id", Value::String(record_id.to_string())),
            ("namespace", Value::String(namespace.to_string())),
        ]);

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

        let surreal_val = if self.is_local() {
            let db = self.with_namespace_local(namespace).await?;
            let mut response = db.query(&sql).bind(("content", content_for_update.clone())).await.map_err(|err| {
                MemoryError::Storage(format!("SurrealDB query failed: {err}"))
            })?;
            response
                .take::<SurrealValue>(0)
                .map_err(|err| MemoryError::Storage(format!("SurrealDB take failed: {err}")))?
        } else {
            let db = self.with_namespace_remote(namespace).await?;
            let mut response = db.query(&sql).bind(("content", content_for_update)).await.map_err(|err| {
                MemoryError::Storage(format!("SurrealDB query failed: {err}"))
            })?;
            response
                .take::<SurrealValue>(0)
                .map_err(|err| MemoryError::Storage(format!("SurrealDB take failed: {err}")))?
        };

        let normalized = surreal_to_json(surreal_val);
        let result = extract_first_record(normalized).unwrap_or(Value::Null);

        self.log_op("db.update.result", vec![
            ("result", Value::String("ok".to_string())),
        ]);

        Ok(result)
    }

    async fn query(
        &self,
        sql: &str,
        vars: Option<Value>,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        self.log_op("db.query", vec![
            ("sql", Value::String(sql.to_string())),
            ("namespace", Value::String(namespace.to_string())),
        ]);

        if let Some(Value::Object(map)) = &vars {
            self.log_op("db.query.vars", vec![
                ("count", Value::Number(serde_json::Number::from(map.len()))),
            ]);
        }

        if self.is_local() {
            let db = self.with_namespace_local(namespace).await?;
            let mut q = db.query(sql);
            if let Some(v) = vars.clone() {
                q = q.bind(v);
            }
            q.await.map_err(|err| {
                MemoryError::Storage(format!("SurrealDB query failed: {err}"))
            })?;
        } else {
            let db = self.with_namespace_remote(namespace).await?;
            let mut q = db.query(sql);
            if let Some(v) = vars {
                q = q.bind(v);
            }
            q.await.map_err(|err| {
                MemoryError::Storage(format!("SurrealDB query failed: {err}"))
            })?;
        }

        self.log_op("db.query.result", vec![
            ("result", Value::String("ok".to_string())),
        ]);

        Ok(Value::Null)
    }

    async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError> {
        self.apply_migrations_impl(namespace).await
    }
}

// ============================================================================
// Helper functions
// ============================================================================

fn ensure_dir_exists(path: &Path) -> Result<(), MemoryError> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent).map_err(|err| {
            MemoryError::Storage(format!("failed to create data dir: {err}"))
        })?;
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
                "Array" => val.as_array()
                    .map(|items| J::Array(items.iter().map(normalize_surreal_json).collect()))
                    .unwrap_or_else(|| val.clone()),
                "Object" => val.as_object()
                    .map(|inner| {
                        J::Object(inner.iter().map(|(ik, iv)| (ik.clone(), normalize_surreal_json(iv))).collect())
                    })
                    .unwrap_or_else(|| val.clone()),
                "Strand" | "String" => {
                    val.as_object()
                        .and_then(|inner| inner.get("String").cloned())
                        .unwrap_or_else(|| val.clone())
                }
                "Number" | "Float" | "Int" | "Decimal" => normalize_surreal_json(val),
                _ => J::Object(
                    map.iter()
                        .map(|(ik, iv)| (ik.clone(), normalize_surreal_json(iv)))
                        .collect(),
                ),
            }
        }
        J::Object(map) => {
            J::Object(map.iter().map(|(k, v)| (k.clone(), normalize_surreal_json(v))).collect())
        }
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
        assert_eq!(normalize_url("http://localhost:8000"), "ws://localhost:8000/rpc");
        assert_eq!(normalize_url("https://db.local"), "wss://db.local/rpc");
        assert_eq!(normalize_url("https://db.local/rpc"), "wss://db.local/rpc");
        assert_eq!(normalize_url("ws://custom"), "ws://custom");
    }

    #[test]
    fn normalize_url_handles_trailing_slash() {
        assert_eq!(normalize_url("http://db.local/"), "ws://db.local/rpc");
        assert_eq!(normalize_url("https://db.local/"), "wss://db.local/rpc");
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
}
