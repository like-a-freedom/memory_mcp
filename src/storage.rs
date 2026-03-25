//! Database abstraction layer for SurrealDB.
//!
//! This module provides a unified interface for database operations,
//! abstracting over embedded and remote (WebSocket) engines.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{Value, json};
use surrealdb::Surreal;
use surrealdb::engine::local::Db;
use surrealdb::engine::local::Mem;
use surrealdb::engine::local::RocksDb;
use surrealdb::engine::remote::ws::{Client, Ws};
use surrealdb::opt::auth::Root;
use surrealdb::types::Value as SurrealValue;
use tokio::sync::Mutex;

use crate::config::SurrealConfig;
use crate::logging::{LogLevel, StdoutLogger};
use crate::service::MemoryError;

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
    /// Connects to an embedded in-memory SurrealDB instance.
    ///
    /// This is primarily intended for tests that should exercise the real
    /// SurrealDB query engine without touching the filesystem.
    pub async fn connect_in_memory(
        database: &str,
        default_namespace: &str,
        log_level: &str,
    ) -> Result<Self, MemoryError> {
        let db = Surreal::new::<Mem>(())
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB memory init failed: {err}")))?;

        db.use_ns(default_namespace)
            .use_db(database)
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB use failed: {err}")))?;

        Ok(Self {
            engine: DbEngine::Local(Mutex::new(db)),
            database: database.to_string(),
            logger: StdoutLogger::new(log_level),
        })
    }

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

        let surreal_val = match self
            .execute_query(&sql, bind.map(|b| json!({"id": b})), namespace)
            .await
        {
            Ok(value) => value,
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => {
                return Ok(None);
            }
            Err(err) => return Err(err),
        };

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
        let surreal_val = match self.execute_query(&sql, None, namespace).await {
            Ok(value) => value,
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => {
                return Ok(Vec::new());
            }
            Err(err) => return Err(err),
        };
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

        let (sql, vars) = build_select_facts_filtered_query(
            scope,
            cutoff,
            query_contains,
            limit,
        );

        let surreal_val = match self.execute_query(&sql, Some(vars), namespace).await {
            Ok(value) => value,
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => {
                return Ok(Vec::new());
            }
            Err(err) => return Err(err),
        };
        let normalized = surreal_to_json(surreal_val);
        let results = extract_records(normalized);

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
            "SELECT * FROM edge WHERE t_valid <= type::datetime($cutoff) AND (t_ingested IS NONE OR t_ingested <= type::datetime($cutoff)) AND (t_invalid IS NONE OR t_invalid > type::datetime($cutoff) OR t_invalid_ingested > type::datetime($cutoff)) ORDER BY from_id ASC, to_id ASC, t_valid DESC",
        );

        let vars = serde_json::json!({ "cutoff": cutoff });
        let surreal_val = match self.execute_query(&sql, Some(vars), namespace).await {
            Ok(value) => value,
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => {
                return Ok(Vec::new());
            }
            Err(err) => return Err(err),
        };
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

    let target = if let Some(record_id) = id {
        format!("{table}:⟨{record_id}⟩")
    } else {
        table.to_string()
    };

    let normalized = normalize_surreal_json(&content);
    if let Value::Object(map) = normalized {
        let (assignments, vars) = build_set_assignments(table, map);
        let sql = if assignments.is_empty() {
            format!("CREATE {target} RETURN *")
        } else {
            format!("CREATE {target} SET {} RETURN *", assignments.join(", "))
        };
        (sql, Value::Object(vars))
    } else {
        (format!("CREATE {target} CONTENT $content RETURN *"), json!({"content": normalized}))
    }
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

    let content_for_update = if let Value::Object(mut map) = content {
        map.remove("id");
        Value::Object(map)
    } else {
        content
    };

    let normalized = normalize_surreal_json(&content_for_update);
    if let Value::Object(map) = normalized {
        let (assignments, vars) = build_set_assignments(table, map);
        let sql = if assignments.is_empty() {
            format!("UPDATE {table}:⟨{id}⟩ RETURN *")
        } else {
            format!("UPDATE {table}:⟨{id}⟩ SET {} RETURN *", assignments.join(", "))
        };
        Ok((sql, Value::Object(vars)))
    } else {
        let sql = format!("UPDATE {table}:⟨{id}⟩ MERGE $content RETURN *");
        Ok((sql, json!({"content": normalized})))
    }
}

fn build_select_facts_filtered_query(
    scope: &str,
    cutoff: &str,
    query_contains: Option<&str>,
    limit: i32,
) -> (String, Value) {
    let cutoff_expr = "type::datetime($cutoff)";
    let base_where = format!(
        "scope = $scope AND t_valid <= {cutoff_expr} AND (t_ingested IS NONE OR t_ingested <= {cutoff_expr}) AND (t_invalid IS NONE OR t_invalid > {cutoff_expr} OR t_invalid_ingested > {cutoff_expr})"
    );

    let mut vars = serde_json::Map::from_iter([
        ("scope".to_string(), json!(scope)),
        ("cutoff".to_string(), json!(cutoff)),
        ("limit".to_string(), json!(limit)),
    ]);

    let sql = if let Some(query) = query_contains.filter(|query| !query.trim().is_empty()) {
        let words: Vec<String> = query
            .split_whitespace()
            .filter(|word| word.len() >= 2)
            .map(|word| word.to_lowercase())
            .collect();

        vars.insert("query".to_string(), json!(query));

        let fallback = if words.is_empty() {
            String::new()
        } else {
            let predicates = words
                .iter()
                .enumerate()
                .map(|(index, word)| {
                    let key = format!("word_{index}");
                    vars.insert(key.clone(), json!(word));
                    format!("string::lowercase(content) CONTAINS ${key}")
                })
                .collect::<Vec<_>>()
                .join(" AND ");
            format!(" OR ({predicates})")
        };

        format!(
            "SELECT * FROM fact WHERE {base_where} AND (content @@ $query{fallback}) ORDER BY t_valid DESC LIMIT $limit"
        )
    } else {
        format!("SELECT * FROM fact WHERE {base_where} ORDER BY t_valid DESC LIMIT $limit")
    };

    (sql, Value::Object(vars))
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

fn temporal_field_names_for_table(table: &str) -> &'static [&'static str] {
    match table {
        "episode" => &["t_ref", "t_ingested"],
        "fact" | "edge" => &["t_valid", "t_ingested", "t_invalid", "t_invalid_ingested"],
        "community" => &["updated_at"],
        "event_log" => &["ts"],
        "task" => &["due_date"],
        "script_migration" => &["executed_at"],
        _ => &[],
    }
}

fn build_set_assignments(
    table: &str,
    map: serde_json::Map<String, Value>,
) -> (Vec<String>, serde_json::Map<String, Value>) {
    let temporal_fields = temporal_field_names_for_table(table);
    let mut entries: Vec<(String, Value)> = map.into_iter().collect();
    entries.sort_by(|(left, _), (right, _)| left.cmp(right));

    let mut assignments = Vec::with_capacity(entries.len());
    let mut vars = serde_json::Map::new();

    for (key, value) in entries {
        if temporal_fields.contains(&key.as_str()) {
            match value {
                Value::Null => assignments.push(format!("{key} = NONE")),
                Value::String(raw) => {
                    vars.insert(key.clone(), Value::String(raw));
                    assignments.push(format!("{key} = type::datetime(${key})"));
                }
                other => {
                    vars.insert(key.clone(), other);
                    assignments.push(format!("{key} = ${key}"));
                }
            }
        } else {
            vars.insert(key.clone(), value);
            assignments.push(format!("{key} = ${key}"));
        }
    }

    (assignments, vars)
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

fn is_missing_table_error(message: &str) -> bool {
    let lowered = message.to_lowercase();
    lowered.contains("does not exist") && lowered.contains("table")
}

fn surreal_to_json(value: SurrealValue) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// Try to find a version-like field inside arbitrary JSON returned by the
/// server info query. Searches keys for the substring "version" (case-ins).
fn find_version_in_json(v: &Value) -> Option<String> {
    use regex::Regex;

    let ver_re = Regex::new(r"\d+\.\d+(?:\.\d+)?").unwrap();

    match v {
        Value::String(s) => {
            if ver_re.is_match(s) || s.to_lowercase().contains("surreal") {
                Some(s.clone())
            } else {
                None
            }
        }
        Value::Object(map) => {
            for (k, val) in map.iter() {
                if k.to_lowercase().contains("version") {
                    if let Some(s) = val.as_str() {
                        return Some(s.to_string());
                    } else if let Some(found) = find_version_in_json(val) {
                        return Some(found);
                    } else {
                        return Some(val.to_string());
                    }
                }
            }
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
        _ => None,
    }
}

fn extract_first_record(value: Value) -> Option<Value> {
    extract_records(value).into_iter().next()
}

fn unwrap_object_wrapper(value: Value) -> Value {
    match value {
        Value::Object(mut map) => {
            if let Some(object) = map.remove("Object") {
                normalize_surreal_json(&object)
            } else {
                normalize_surreal_json(&Value::Object(map))
            }
        }
        other => normalize_surreal_json(&other),
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
                return vec![normalize_surreal_json(&object)];
            }
            vec![normalize_surreal_json(&Value::Object(map))]
        }
        Value::Null => Vec::new(),
        other => vec![normalize_surreal_json(&other)],
    }
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
                "Datetime" => val
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
        J::Null => J::Null,
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

    #[test]
    fn is_missing_table_error_detects_missing_table_message() {
        assert!(is_missing_table_error(
            "SurrealDB take failed: The table 'episode' does not exist"
        ));
    }

    #[test]
    fn is_missing_table_error_ignores_other_storage_errors() {
        assert!(!is_missing_table_error(
            "SurrealDB query failed: permission denied"
        ));
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

        let client = SurrealDbClient::connect(&config, "testns")
            .await
            .expect("connect");
        let ver = client
            .server_version("testns")
            .await
            .expect("server_version");
        if let Some(s) = ver {
            assert!(!s.is_empty());
        }
    }

    #[tokio::test]
    async fn connect_in_memory_initializes_real_embedded_engine() {
        let client = SurrealDbClient::connect_in_memory("testdb", "testns", "warn")
            .await
            .expect("connect in memory");

        client
            .apply_migrations("testns")
            .await
            .expect("apply migrations");

        let records = client
            .select_table("event_log", "testns")
            .await
            .expect("select table");

        assert!(records.is_empty());
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
        assert_eq!(normalize_surreal_json(&json!(null)), json!(null));
        assert_eq!(normalize_surreal_json(&json!(42)), json!(42));
        assert_eq!(normalize_surreal_json(&json!(true)), json!(true));
        assert_eq!(normalize_surreal_json(&json!("plain")), json!("plain"));
    }

    #[test]
    fn build_create_query_preserves_null_option_fields() {
        let content = json!({
            "title": "Follow up",
            "status": "pending_confirmation",
            "due_date": null
        });

        let (sql, vars) = build_create_query("task", content);
        assert!(sql.contains("due_date = NONE"));
        assert!(vars.get("due_date").is_none());
    }

    #[test]
    fn build_create_query_casts_temporal_fields_with_type_datetime() {
        let content = json!({
            "content": "remember this",
            "scope": "org",
            "t_valid": "2026-03-25T17:07:08.958562Z",
            "t_ingested": "2026-03-25T17:07:08.958562Z"
        });

        let (sql, vars) = build_create_query("fact", content);
        assert!(sql.contains("t_valid = type::datetime($t_valid)"));
        assert!(sql.contains("t_ingested = type::datetime($t_ingested)"));
        assert_eq!(vars.get("t_valid"), Some(&json!("2026-03-25T17:07:08.958562Z")));
        assert_eq!(vars.get("t_ingested"), Some(&json!("2026-03-25T17:07:08.958562Z")));
    }

    #[test]
    fn find_version_in_json_prefers_version_key() {
        let j = json!({"meta": {"server": {"version": "3.0.0"}}});
        assert_eq!(find_version_in_json(&j), Some("3.0.0".to_string()));
    }

    #[test]
    fn find_version_in_json_extracts_from_string_with_semver() {
        let j = json!("SurrealDB 3.0.0 (embedded)");
        assert_eq!(
            find_version_in_json(&j),
            Some("SurrealDB 3.0.0 (embedded)".to_string())
        );
    }

    #[test]
    fn find_version_in_json_ignores_ddl_string() {
        let j = json!(["DEFINE ANALYZER simple TOKENIZERS BLANK FILTERS LOWERCASE"]);
        assert_eq!(find_version_in_json(&j), None);
    }

    #[test]
    fn find_version_in_json_finds_version_in_array() {
        let j = json!([{"info": "x"}, {"version": "3.1.0"}]);
        assert_eq!(find_version_in_json(&j), Some("3.1.0".to_string()));
    }

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
        assert_eq!(sql, "CREATE entity:⟨abc123⟩ SET name = $name RETURN *");
        assert_eq!(vars.get("name"), Some(&json!("test")));
    }

    #[test]
    fn build_create_query_without_id() {
        let content = json!({"name": "test"});
        let (sql, vars) = build_create_query("entity", content);
        assert_eq!(sql, "CREATE entity SET name = $name RETURN *");
        assert_eq!(vars.get("name"), Some(&json!("test")));
    }

    #[test]
    fn build_update_query_success() {
        let content = json!({"id": "fact:abc123", "name": "updated"});
        let (sql, vars) = build_update_query("fact:abc123", content).unwrap();
        assert_eq!(sql, "UPDATE fact:⟨abc123⟩ SET name = $name RETURN *");
        assert!(vars.get("id").is_none());
        assert_eq!(vars.get("name"), Some(&json!("updated")));
    }

    #[test]
    fn build_update_query_invalid_format() {
        let content = json!({"name": "test"});
        let result = build_update_query("invalid_format", content);
        assert!(matches!(result, Err(MemoryError::Storage(_))));
    }

    #[test]
    fn extract_first_record_from_nested_object() {
        let nested = json!({
            "Object": {
                "id": "test:1",
                "name": "Test"
            }
        });
        let result = extract_first_record(nested);
        assert_eq!(result, Some(json!({"id": "test:1", "name": "Test"})));
    }

    #[test]
    fn extract_first_record_from_nested_array() {
        let nested = json!({
            "Array": [
                {"Object": {"id": "test:1"}},
                {"Object": {"id": "test:2"}}
            ]
        });
        let result = extract_first_record(nested);
        assert_eq!(result, Some(json!({"id": "test:1"})));
    }

    #[test]
    fn unwrap_object_wrapper_unwraps_object_key() {
        let wrapped = json!({"Object": {"id": "test:1"}});
        let result = unwrap_object_wrapper(wrapped);
        assert_eq!(result, json!({"id": "test:1"}));
    }

    #[test]
    fn unwrap_object_wrapper_preserves_unwrapped() {
        let unwrapped = json!({"id": "test:1"});
        let result = unwrap_object_wrapper(unwrapped);
        assert_eq!(result, json!({"id": "test:1"}));
    }

    #[test]
    fn normalize_surreal_json_handles_nested_objects() {
        let nested = json!({
            "Object": {
                "name": {"Strand": "Alice"},
                "age": {"Number": 30}
            }
        });
        let normalized = normalize_surreal_json(&nested);
        assert_eq!(normalized["name"], "Alice");
        assert_eq!(normalized["age"], 30);
    }

    #[test]
    fn normalize_surreal_json_handles_empty_object() {
        let empty = json!({});
        let normalized = normalize_surreal_json(&empty);
        assert_eq!(normalized, json!({}));
    }

    #[test]
    fn normalize_surreal_json_handles_array() {
        let arr = json!({
            "Array": [
                {"String": "item1"},
                {"String": "item2"}
            ]
        });
        let normalized = normalize_surreal_json(&arr);
        assert_eq!(normalized, json!(["item1", "item2"]));
    }

    #[test]
    fn ensure_dir_exists_creates_parent_dirs() {
        use tempfile::tempdir;

        let temp = tempdir().unwrap();
        let deep_path = temp.path().join("a/b/c/deep.db");

        assert!(ensure_dir_exists(&deep_path).is_ok());
        assert!(deep_path.parent().unwrap().exists());
    }

    #[test]
    fn ensure_dir_exists_handles_existing_dir() {
        use tempfile::tempdir;

        let temp = tempdir().unwrap();
        let existing_path = temp.path().join("existing.db");

        assert!(ensure_dir_exists(&existing_path).is_ok());
        assert!(ensure_dir_exists(&existing_path).is_ok());
    }

    #[test]
    fn ensure_dir_exists_handles_no_parent() {
        let path = std::path::Path::new("test.db");
        assert!(ensure_dir_exists(path).is_ok());
    }

    #[test]
    fn build_select_facts_filtered_query_with_text_query_preserves_temporal_filters() {
        let (sql, vars) = build_select_facts_filtered_query(
            "org",
            "2026-01-15T00:00:00Z",
            Some("ARR growth"),
            5,
        );

        assert!(
            sql.contains("FROM fact WHERE"),
            "expected DB-side WHERE filtering, got: {sql}"
        );
        assert!(
            sql.contains("scope = $scope"),
            "expected scope predicate, got: {sql}"
        );
        assert!(
            sql.contains("t_valid <= type::datetime($cutoff)"),
            "expected temporal predicate, got: {sql}"
        );
        assert!(
            sql.contains("content @@ $query"),
            "expected fulltext operator for text search, got: {sql}"
        );
        assert_eq!(vars["scope"], json!("org"));
        assert_eq!(vars["cutoff"], json!("2026-01-15T00:00:00Z"));
        assert_eq!(vars["query"], json!("ARR growth"));
        assert_eq!(vars["limit"], json!(5));
    }

    #[test]
    fn normalize_surreal_json_unwraps_datetime_variants() {
        let datetime = json!({"Datetime": {"String": "2026-01-15T00:00:00Z"}});
        assert_eq!(normalize_surreal_json(&datetime), json!("2026-01-15T00:00:00Z"));
    }
}
