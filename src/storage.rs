use async_trait::async_trait;
use serde_json::Value;
use surrealdb::Surreal;
use surrealdb::engine::remote::ws::{Client, Ws};
use surrealdb::opt::auth::Root;
use tokio::sync::Mutex;

use crate::config::SurrealConfig;
use crate::logging::{LogLevel, StdoutLogger};
use crate::service::MemoryError;
use crate::service::migrations_dir;

/// Unified database engine enum that abstracts over local (RocksDB) and remote (WebSocket) connections.
/// This eliminates code duplication from if/else branches throughout the codebase.
pub enum DbEngine {
    Local(Mutex<Surreal<surrealdb::engine::local::Db>>),
    Remote(Mutex<Surreal<Client>>),
}

impl DbEngine {
    /// Execute a query and return the raw SurrealDB response.
    async fn query(
        &self,
        sql: &str,
        namespace: &str,
        database: &str,
    ) -> Result<surrealdb::Response, MemoryError> {
        match self {
            DbEngine::Local(db) => {
                let guard = db.lock().await;
                guard
                    .use_ns(namespace)
                    .use_db(database)
                    .await
                    .map_err(|e| MemoryError::Storage(format!("SurrealDB use failed: {e}")))?;
                guard
                    .query(sql)
                    .await
                    .map_err(|e| MemoryError::Storage(format!("SurrealDB query failed: {e}")))
            }
            DbEngine::Remote(db) => {
                let guard = db.lock().await;
                guard
                    .use_ns(namespace)
                    .use_db(database)
                    .await
                    .map_err(|e| MemoryError::Storage(format!("SurrealDB use failed: {e}")))?;
                guard
                    .query(sql)
                    .await
                    .map_err(|e| MemoryError::Storage(format!("SurrealDB query failed: {e}")))
            }
        }
    }

    /// Execute a query with bound variables.
    async fn query_with_vars(
        &self,
        sql: &str,
        vars: Option<Value>,
        namespace: &str,
        database: &str,
    ) -> Result<surrealdb::Response, MemoryError> {
        match self {
            DbEngine::Local(db) => {
                let guard = db.lock().await;
                guard
                    .use_ns(namespace)
                    .use_db(database)
                    .await
                    .map_err(|e| MemoryError::Storage(format!("SurrealDB use failed: {e}")))?;
                let mut query = guard.query(sql);
                if let Some(vars) = vars {
                    query = query.bind(vars);
                }
                query
                    .await
                    .map_err(|e| MemoryError::Storage(format!("SurrealDB query failed: {e}")))
            }
            DbEngine::Remote(db) => {
                let guard = db.lock().await;
                guard
                    .use_ns(namespace)
                    .use_db(database)
                    .await
                    .map_err(|e| MemoryError::Storage(format!("SurrealDB use failed: {e}")))?;
                let mut query = guard.query(sql);
                if let Some(vars) = vars {
                    query = query.bind(vars);
                }
                query
                    .await
                    .map_err(|e| MemoryError::Storage(format!("SurrealDB query failed: {e}")))
            }
        }
    }

    /// Select all records from a table.
    async fn select_table(
        &self,
        table: &str,
        namespace: &str,
        database: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        match self {
            DbEngine::Local(db) => {
                let guard = db.lock().await;
                guard
                    .use_ns(namespace)
                    .use_db(database)
                    .await
                    .map_err(|e| MemoryError::Storage(format!("SurrealDB use failed: {e}")))?;
                let sql = format!("SELECT * FROM {}", table);
                let mut response = guard.query(sql.as_str()).await.map_err(|e| {
                    MemoryError::Storage(format!("SurrealDB select table failed: {e}"))
                })?;
                let s_val: surrealdb::Value = response.take(0).map_err(|e| {
                    MemoryError::Storage(format!("SurrealDB response deserialize failed: {e}"))
                })?;
                let json = serde_json::to_value(&s_val).map_err(|e| {
                    MemoryError::Storage(format!("SurrealDB -> JSON conversion failed: {e}"))
                })?;
                let normalized = normalize_surreal_json(&json);
                if normalized.is_array() {
                    serde_json::from_value(normalized).map_err(|e| {
                        MemoryError::Storage(format!("Deserializing query array failed: {e}"))
                    })
                } else {
                    Ok(vec![normalized])
                }
            }
            DbEngine::Remote(db) => {
                let guard = db.lock().await;
                guard
                    .use_ns(namespace)
                    .use_db(database)
                    .await
                    .map_err(|e| MemoryError::Storage(format!("SurrealDB use failed: {e}")))?;
                guard.select(table).await.map_err(|e| {
                    MemoryError::Storage(format!("SurrealDB select table failed: {e}"))
                })
            }
        }
    }

    /// Execute a query and deserialize the first result set into type `T`.
    async fn query_get<T: serde::de::DeserializeOwned>(
        &self,
        sql: &str,
        namespace: &str,
        database: &str,
    ) -> Result<T, MemoryError> {
        let mut response = self.query(sql, namespace, database).await?;
        let surreal_val: surrealdb::Value = response.take(0).map_err(|e| {
            MemoryError::Storage(format!("SurrealDB response deserialize failed: {e}"))
        })?;
        let json = serde_json::to_value(&surreal_val).map_err(|e| {
            MemoryError::Storage(format!("SurrealDB -> JSON conversion failed: {e}"))
        })?;

        let normalized = normalize_surreal_json(&json);
        // First try to deserialize directly
        if let Ok(res) = serde_json::from_value(normalized.clone()) {
            return Ok(res);
        }
        // If direct deserialization fails and we have an array of single-field objects,
        // try to extract inner values (e.g., SELECT name FROM ... -> [{"name":"Alice"}])
        if let serde_json::Value::Array(arr) = normalized {
            let inner: Vec<serde_json::Value> = arr
                .into_iter()
                .map(|el| {
                    if let serde_json::Value::Object(map) = el {
                        if map.len() == 1 {
                            map.into_iter()
                                .next()
                                .expect("map with len 1 has one entry")
                                .1
                        } else {
                            serde_json::Value::Object(map)
                        }
                    } else {
                        el
                    }
                })
                .collect();
            if let Ok(res) = serde_json::from_value(serde_json::Value::Array(inner)) {
                return Ok(res);
            }
        }
        Err(MemoryError::Storage(
            "Deserializing query result failed".to_string(),
        ))
    }
}

#[async_trait]
pub trait DbClient: Send + Sync {
    async fn select_one(
        &self,
        record_id: &str,
        namespace: &str,
    ) -> Result<Option<Value>, MemoryError>;
    async fn select_table(&self, table: &str, namespace: &str) -> Result<Vec<Value>, MemoryError>;

    /// Select facts with DB-side filtering, sorting by t_valid DESC, and limit.
    /// This pushes filtering logic to the database for better performance.
    ///
    /// # Arguments
    /// - `namespace`: the database namespace
    /// - `scope`: required scope filter
    /// - `cutoff`: facts must have t_valid <= cutoff and (t_invalid IS NULL OR t_invalid > cutoff)
    /// - `query_contains`: optional substring to match in content (case-insensitive)
    /// - `limit`: maximum number of facts to return
    async fn select_facts_filtered(
        &self,
        namespace: &str,
        scope: &str,
        cutoff: &str,
        query_contains: Option<&str>,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError>;

    /// Select edges with DB-side filtering for bi-temporal visibility.
    ///
    /// # Arguments
    /// - `namespace`: the database namespace
    /// - `cutoff`: edges must have t_valid <= cutoff, t_ingested <= cutoff, and
    ///   not be invalidated as-of cutoff (see t_invalid/t_invalid_ingested)
    async fn select_edges_filtered(
        &self,
        namespace: &str,
        cutoff: &str,
    ) -> Result<Vec<Value>, MemoryError>;

    async fn create(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
    ) -> Result<Value, MemoryError>;
    async fn update(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
    ) -> Result<Value, MemoryError>;
    async fn query(
        &self,
        sql: &str,
        vars: Option<Value>,
        namespace: &str,
    ) -> Result<Value, MemoryError>;

    /// Apply migrations from a migrations directory (relative path in the repo).
    /// Apply migrations for the given namespace. Implementations SHOULD look for
    /// a canonical relative `./migrations` directory on disk and apply migrations
    /// from it; if not present, they MAY fall back to embedded migrations compiled
    /// into the binary.
    async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError>;
}

pub struct SurrealDbClient {
    engine: DbEngine,
    database: String,
    logger: StdoutLogger,
    is_local: bool,
}

impl SurrealDbClient {
    pub async fn connect(
        config: &SurrealConfig,
        default_namespace: &str,
    ) -> Result<Self, MemoryError> {
        let (engine, is_local) = if config.embedded {
            // Use RocksDB embedded engine with configurable data dir (persistent)
            use std::path::PathBuf;
            use surrealdb::engine::local::RocksDb;
            use surrealdb::opt::{Config as SurrealOptConfig, capabilities::Capabilities};

            let data_dir = config
                .data_dir
                .clone()
                .unwrap_or_else(|| "./data/surrealdb".to_string());
            let path = PathBuf::from(data_dir);
            if let Some(parent) = path.parent()
                && !parent.exists()
            {
                std::fs::create_dir_all(parent).map_err(|err| {
                    MemoryError::Storage(format!("failed to create data dir: {err}"))
                })?;
            }

            let root = Root {
                username: config.username.as_str(),
                password: config.password.as_str(),
            };
            let cfg = SurrealOptConfig::new()
                .user(root)
                .capabilities(Capabilities::all());
            let db = Surreal::new::<RocksDb>((path.clone(), cfg))
                .await
                .map_err(|err| {
                    MemoryError::Storage(format!("SurrealDB embedded init failed: {err}"))
                })?;
            db.signin(root)
                .await
                .map_err(|err| MemoryError::Storage(format!("SurrealDB signin failed: {err}")))?;
            db.use_ns(default_namespace)
                .use_db(config.db_name.as_str())
                .await
                .map_err(|err| MemoryError::Storage(format!("SurrealDB use failed: {err}")))?;

            (DbEngine::Local(Mutex::new(db)), true)
        } else {
            // Fallback to remote WS connection
            let url = normalize_url(config.url.as_ref().unwrap_or(&"".to_string()));
            let db = Surreal::new::<Ws>(url.as_str())
                .await
                .map_err(|err| MemoryError::Storage(format!("SurrealDB connect failed: {err}")))?;
            db.signin(Root {
                username: config.username.as_str(),
                password: config.password.as_str(),
            })
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB signin failed: {err}")))?;
            db.use_ns(default_namespace)
                .use_db(config.db_name.as_str())
                .await
                .map_err(|err| MemoryError::Storage(format!("SurrealDB use failed: {err}")))?;

            (DbEngine::Remote(Mutex::new(db)), false)
        };

        Ok(Self {
            engine,
            is_local,
            database: config.db_name.clone(),
            logger: StdoutLogger::new(&config.log_level),
        })
    }

    /// Check if using local (embedded) database.
    fn is_local(&self) -> bool {
        self.is_local
    }

    /// Get local database reference (for backward compatibility during migration).
    fn db_local(&self) -> Option<&Mutex<Surreal<surrealdb::engine::local::Db>>> {
        match &self.engine {
            DbEngine::Local(db) => Some(db),
            DbEngine::Remote(_) => None,
        }
    }

    /// Get remote database reference (for backward compatibility during migration).
    fn db_remote(&self) -> Option<&Mutex<Surreal<Client>>> {
        match &self.engine {
            DbEngine::Local(_) => None,
            DbEngine::Remote(db) => Some(db),
        }
    }

    /// Get local database with namespace set (for backward compatibility).
    async fn with_namespace_local(
        &self,
        namespace: &str,
    ) -> Result<surrealdb::Surreal<surrealdb::engine::local::Db>, MemoryError> {
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
            DbEngine::Remote(_) => Err(MemoryError::Storage("local db not configured".to_string())),
        }
    }

    /// Get remote database with namespace set (for backward compatibility).
    async fn with_namespace_remote(
        &self,
        namespace: &str,
    ) -> Result<surrealdb::Surreal<Client>, MemoryError> {
        match &self.engine {
            DbEngine::Local(_) => Err(MemoryError::Storage("remote db not configured".to_string())),
            DbEngine::Remote(db) => {
                let guard = db.lock().await;
                guard
                    .use_ns(namespace)
                    .use_db(&self.database)
                    .await
                    .map_err(|err| MemoryError::Storage(format!("SurrealDB use failed: {err}")))?;
                Ok(guard.clone())
            }
        }
    }

    /// Apply migrations for the given namespace.
    ///
    /// Behavior:
    /// - If a filesystem `./migrations` directory exists in the repo root it will be used.
    /// - Otherwise, fallback to migrations embedded into the binary at compile time.
    pub async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError> {
        use std::collections::HashMap;
        use surrealdb_migrations::MigrationRunner;

        let log_event = |level: LogLevel, stage: &str, source: &str, detail: Option<String>| {
            let mut event = HashMap::new();
            event.insert(
                "op".to_string(),
                serde_json::Value::String("migrations".to_string()),
            );
            event.insert(
                "stage".to_string(),
                serde_json::Value::String(stage.to_string()),
            );
            event.insert(
                "source".to_string(),
                serde_json::Value::String(source.to_string()),
            );
            event.insert(
                "namespace".to_string(),
                serde_json::Value::String(namespace.to_string()),
            );
            if let Some(detail) = detail {
                event.insert("detail".to_string(), serde_json::Value::String(detail));
            }
            self.logger.log(event, level);
        };

        // Filesystem migrations path (repo_root/migrations)
        let fs_dir = migrations_dir()?;
        match migration_source(&fs_dir) {
            MigrationSource::Filesystem(path) => {
                log_event(LogLevel::Info, "start", "filesystem", None);
                // Create a small temp config pointing to the repo root to pass into the runner
                let root = path.parent().unwrap_or(path.as_path());
                let mut cfg = String::new();
                cfg.push_str("[core]\n");
                cfg.push_str(&format!("path = \"{}\"\n", root.to_str().unwrap_or(".")));

                let tmp_dir = std::env::temp_dir();
                let config_path = tmp_dir.join(format!(
                    "surreal_mig_{}.toml",
                    chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
                ));

                std::fs::write(&config_path, cfg).map_err(|err| {
                    MemoryError::Storage(format!("writing temp migration config failed: {err}"))
                })?;

                let res = match &self.engine {
                    DbEngine::Local(db) => {
                        let guard = db.lock().await;
                        guard
                            .use_ns(namespace)
                            .use_db(&self.database)
                            .await
                            .map_err(|e| {
                                MemoryError::Storage(format!("SurrealDB use failed: {e}"))
                            })?;
                        MigrationRunner::new(&*guard)
                            .use_config_file(&config_path)
                            .up()
                            .await
                    }
                    DbEngine::Remote(db) => {
                        let guard = db.lock().await;
                        guard
                            .use_ns(namespace)
                            .use_db(&self.database)
                            .await
                            .map_err(|e| {
                                MemoryError::Storage(format!("SurrealDB use failed: {e}"))
                            })?;
                        MigrationRunner::new(&*guard)
                            .use_config_file(&config_path)
                            .up()
                            .await
                    }
                };

                let _ = std::fs::remove_file(&config_path);
                match res {
                    Ok(()) => {
                        log_event(LogLevel::Info, "done", "filesystem", None);
                        Ok(())
                    }
                    Err(err) => {
                        log_event(
                            LogLevel::Error,
                            "error",
                            "filesystem",
                            Some(err.to_string()),
                        );
                        Err(MemoryError::Storage(format!(
                            "surrealdb-migrations failed: {err}"
                        )))
                    }
                }
            }
            MigrationSource::Embedded => {
                log_event(LogLevel::Info, "start", "embedded", None);
                let res = match &self.engine {
                    DbEngine::Local(db) => {
                        let guard = db.lock().await;
                        guard
                            .use_ns(namespace)
                            .use_db(&self.database)
                            .await
                            .map_err(|e| {
                                MemoryError::Storage(format!("SurrealDB use failed: {e}"))
                            })?;
                        MigrationRunner::new(&*guard)
                            .load_files(&EMBEDDED_MIGRATIONS)
                            .up()
                            .await
                    }
                    DbEngine::Remote(db) => {
                        let guard = db.lock().await;
                        guard
                            .use_ns(namespace)
                            .use_db(&self.database)
                            .await
                            .map_err(|e| {
                                MemoryError::Storage(format!("SurrealDB use failed: {e}"))
                            })?;
                        MigrationRunner::new(&*guard)
                            .load_files(&EMBEDDED_MIGRATIONS)
                            .up()
                            .await
                    }
                };

                match res {
                    Ok(()) => {
                        log_event(LogLevel::Info, "done", "embedded", None);
                        Ok(())
                    }
                    Err(err) => {
                        log_event(LogLevel::Error, "error", "embedded", Some(err.to_string()));
                        Err(MemoryError::Storage(format!(
                            "surrealdb-migrations failed: {err}"
                        )))
                    }
                }
            }
            MigrationSource::None => {
                log_event(LogLevel::Info, "skip", "none", None);
                Ok(())
            }
        }
    }
}

#[async_trait]
impl DbClient for SurrealDbClient {
    async fn select_one(
        &self,
        record_id: &str,
        namespace: &str,
    ) -> Result<Option<Value>, MemoryError> {
        // Debug: record lookup
        let mut event = std::collections::HashMap::new();
        event.insert("op".to_string(), Value::String("db.select_one".to_string()));
        event.insert(
            "record_id".to_string(),
            Value::String(record_id.to_string()),
        );
        event.insert(
            "namespace".to_string(),
            Value::String(namespace.to_string()),
        );
        self.logger.log(event.clone(), LogLevel::Debug);

        // Use explicit SELECT query to ensure consistent JSON serialization
        // For SurrealDB, avoid "SELECT * FROM episode:..." which sometimes returns
        // empty Arrays for record lookups; use table-level WHERE query by episode_id
        // because episodes store deterministic "episode_id" in the payload.
        let (sql, bind) = if let Some(idx) = record_id.find(':') {
            let table = &record_id[..idx];
            if table == "episode" {
                (
                    format!("SELECT * FROM {} WHERE episode_id = $id", table),
                    Some(Value::String(record_id.to_string())),
                )
            } else {
                (format!("SELECT * FROM {}", record_id), None)
            }
        } else {
            (format!("SELECT * FROM {}", record_id), None)
        };

        // Debug: log the actual SQL being executed
        let mut sql_event = std::collections::HashMap::new();
        sql_event.insert(
            "op".to_string(),
            Value::String("db.select_one.sql".to_string()),
        );
        sql_event.insert("sql".to_string(), Value::String(sql.clone()));
        sql_event.insert(
            "namespace".to_string(),
            Value::String(namespace.to_string()),
        );
        self.logger.log(sql_event, LogLevel::Trace);

        let item: Option<Value> = if self.db_local().is_some() {
            let db = self.with_namespace_local(namespace).await?;
            let mut q = db.query(sql.as_str());
            if let Some(b) = bind.clone() {
                q = q.bind(("id", b));
            }
            let mut response = q.await.map_err(|err| {
                let mut e = event.clone();
                e.insert("error".to_string(), Value::String(err.to_string()));
                self.logger.log(e, LogLevel::Warn);
                MemoryError::Storage(format!("SurrealDB select failed: {err}"))
            })?;
            // Get raw surrealdb::Value and normalize into plain JSON
            let surreal_val: surrealdb::Value = response
                .take(0)
                .map_err(|err| MemoryError::Storage(format!("SurrealDB select failed: {err}")))?;
            let json = serde_json::to_value(&surreal_val).map_err(|err| {
                MemoryError::Storage(format!("SurrealDB -> JSON conversion failed: {err}"))
            })?;
            let normalized = normalize_surreal_json(&json);

            // Debug: log the raw JSON response
            let mut json_event = std::collections::HashMap::new();
            json_event.insert(
                "op".to_string(),
                Value::String("db.select_one.raw_json".to_string()),
            );
            json_event.insert(
                "json_str".to_string(),
                Value::String(
                    serde_json::to_string(&normalized)
                        .unwrap_or_else(|_| "null".to_string())
                        .chars()
                        .take(500)
                        .collect(),
                ),
            );
            self.logger.log(json_event, LogLevel::Trace);

            // If it's an array, take first element
            if let Value::Array(arr) = normalized {
                arr.into_iter().next()
            } else if normalized != Value::Null {
                Some(normalized)
            } else {
                None
            }
        } else {
            let db = self.with_namespace_remote(namespace).await?;
            let mut response = db.query(sql.as_str()).await.map_err(|err| {
                let mut e = event.clone();
                e.insert("error".to_string(), Value::String(err.to_string()));
                self.logger.log(e, LogLevel::Warn);
                MemoryError::Storage(format!("SurrealDB select failed: {err}"))
            })?;
            let surreal_val: surrealdb::Value = response
                .take(0)
                .map_err(|err| MemoryError::Storage(format!("SurrealDB select failed: {err}")))?;
            let json = serde_json::to_value(&surreal_val).map_err(|err| {
                MemoryError::Storage(format!("SurrealDB -> JSON conversion failed: {err}"))
            })?;
            let normalized = normalize_surreal_json(&json);
            if let Value::Array(arr) = normalized {
                arr.into_iter().next()
            } else if normalized != Value::Null {
                Some(normalized)
            } else {
                None
            }
        };

        let mut done = std::collections::HashMap::new();
        done.insert(
            "op".to_string(),
            Value::String("db.select_one.result".to_string()),
        );
        done.insert(
            "record_id".to_string(),
            Value::String(record_id.to_string()),
        );
        done.insert(
            "namespace".to_string(),
            Value::String(namespace.to_string()),
        );
        done.insert("found".to_string(), Value::Bool(item.is_some()));
        if let Some(Value::Object(map)) = item.clone() {
            done.insert(
                "fields".to_string(),
                Value::Number(serde_json::Number::from(map.len())),
            );
            // include small sample of keys
            let keys: Vec<Value> = map
                .keys()
                .take(5)
                .map(|k| Value::String(k.clone()))
                .collect();
            done.insert("sample_keys".to_string(), Value::Array(keys));
            self.logger.log(done, LogLevel::Debug);
        } else {
            self.logger.log(done, LogLevel::Debug);
        }

        Ok(item)
    }

    async fn select_table(&self, table: &str, namespace: &str) -> Result<Vec<Value>, MemoryError> {
        let mut event = std::collections::HashMap::new();
        event.insert(
            "op".to_string(),
            Value::String("db.select_table".to_string()),
        );
        event.insert("table".to_string(), Value::String(table.to_string()));
        event.insert(
            "namespace".to_string(),
            Value::String(namespace.to_string()),
        );
        self.logger.log(event.clone(), LogLevel::Debug);

        if self.db_local().is_some() {
            let db = self.with_namespace_local(namespace).await?;
            // Use an explicit SELECT query to ensure consistent serialization across engines
            let sql = format!("SELECT * FROM {}", table);
            let mut response = db.query(sql.as_str()).await.map_err(|err| {
                let mut e = event.clone();
                e.insert("error".to_string(), Value::String(err.to_string()));
                self.logger.log(e, LogLevel::Warn);
                MemoryError::Storage(format!("SurrealDB select table failed: {err}"))
            })?;
            let s_val: surrealdb::Value = response.take(0).map_err(|err| {
                let mut e = event.clone();
                e.insert("error".to_string(), Value::String(err.to_string()));
                self.logger.log(e, LogLevel::Warn);
                MemoryError::Storage(format!("SurrealDB response deserialize failed: {err}"))
            })?;
            let json = serde_json::to_value(&s_val).map_err(|err| {
                let mut e = event.clone();
                e.insert("error".to_string(), Value::String(err.to_string()));
                self.logger.log(e, LogLevel::Warn);
                MemoryError::Storage(format!("SurrealDB -> JSON conversion failed: {err}"))
            })?;
            let normalized = normalize_surreal_json(&json);
            // If the response is an array return its contents, otherwise return a single-element vec
            if normalized.is_array() {
                let arr: Vec<Value> = serde_json::from_value(normalized).map_err(|err| {
                    let mut e = event.clone();
                    e.insert("error".to_string(), Value::String(err.to_string()));
                    self.logger.log(e, LogLevel::Warn);
                    MemoryError::Storage(format!("Deserializing query array failed: {err}"))
                })?;
                let mut done = event.clone();
                done.insert(
                    "count".to_string(),
                    Value::Number(serde_json::Number::from(arr.len())),
                );
                self.logger.log(done, LogLevel::Info);
                return Ok(arr);
            }
            let mut done = event.clone();
            done.insert(
                "count".to_string(),
                Value::Number(serde_json::Number::from(1)),
            );
            self.logger.log(done, LogLevel::Info);
            return Ok(vec![normalized]);
        }
        let db = self.with_namespace_remote(namespace).await?;
        let result: Vec<Value> = db.select(table).await.map_err(|err| {
            let mut e = event.clone();
            e.insert("error".to_string(), Value::String(err.to_string()));
            self.logger.log(e, LogLevel::Warn);
            MemoryError::Storage(format!("SurrealDB select table failed: {err}"))
        })?;
        let mut done = event.clone();
        done.insert(
            "count".to_string(),
            Value::Number(serde_json::Number::from(result.len())),
        );
        self.logger.log(done, LogLevel::Info);
        Ok(result)
    }

    async fn create(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        let mut event = std::collections::HashMap::new();
        event.insert("op".to_string(), Value::String("db.create".to_string()));
        event.insert(
            "record_id".to_string(),
            Value::String(record_id.to_string()),
        );
        event.insert(
            "namespace".to_string(),
            Value::String(namespace.to_string()),
        );
        // include a small summary of content keys/length
        if let Value::Object(map) = &content {
            event.insert(
                "fields".to_string(),
                Value::Number(serde_json::Number::from(map.len())),
            );
        }
        self.logger.log(event.clone(), LogLevel::Debug);

        // Parse record_id into table/id parts; allow table-only for auto-id creation.
        let (table, id) = if let Some(idx) = record_id.find(':') {
            (&record_id[..idx], Some(&record_id[idx + 1..]))
        } else {
            (record_id, None)
        };

        // Use CREATE with explicit record id when provided so SurrealDB will persist
        // deterministic ids; otherwise allow auto-id creation for table-only requests.
        // Use RETURN * to get the created row back from SurrealDB
        let sql = if table == "edge" {
            let from_id = content.get("from_id").and_then(Value::as_str);
            let to_id = content.get("to_id").and_then(Value::as_str);
            if let (Some(from), Some(to), Some(edge_id)) = (from_id, to_id, id) {
                format!(
                    "RELATE {}->{}:{}->{} CONTENT $content RETURN *",
                    from, table, edge_id, to
                )
            } else if let Some(edge_id) = id {
                format!("CREATE {}:{} CONTENT $content RETURN *", table, edge_id)
            } else {
                format!("CREATE {} CONTENT $content RETURN *", table)
            }
        } else if let Some(record_id) = id {
            format!("CREATE {}:{} CONTENT $content RETURN *", table, record_id)
        } else {
            format!("CREATE {} CONTENT $content RETURN *", table)
        };

        let content_for_create = content;

        if self.db_local().is_some() {
            let db = self.with_namespace_local(namespace).await?;
            let mut response = db
                .query(&sql)
                .bind(("content", content_for_create.clone()))
                .await
                .map_err(|err| {
                    let mut e = event.clone();
                    e.insert("error".to_string(), Value::String(err.to_string()));
                    self.logger.log(e, LogLevel::Warn);
                    MemoryError::Storage(format!("SurrealDB create failed: {err}"))
                })?;
            // Debug: log raw response
            let surreal_val: surrealdb::Value = response
                .take(0)
                .map_err(|err| MemoryError::Storage(format!("SurrealDB create failed: {err}")))?;
            let json = serde_json::to_value(&surreal_val).map_err(|err| {
                MemoryError::Storage(format!("SurrealDB -> JSON conversion failed: {err}"))
            })?;
            let normalized = normalize_surreal_json(&json);
            let mut json_event = std::collections::HashMap::new();
            json_event.insert(
                "op".to_string(),
                Value::String("db.create.raw_json".to_string()),
            );
            json_event.insert(
                "json_str".to_string(),
                Value::String(
                    serde_json::to_string(&normalized)
                        .unwrap_or_else(|_| "null".to_string())
                        .chars()
                        .take(500)
                        .collect(),
                ),
            );
            self.logger.log(json_event, LogLevel::Trace);
            // Attempt to parse as an array of rows, fallback to single object
            let result: Vec<Value> = match normalized {
                Value::Array(arr) => arr,
                Value::Null => Vec::new(),
                other => vec![other],
            };
            let mut done = event.clone();
            done.insert("result".to_string(), Value::String("ok".to_string()));
            self.logger.log(done, LogLevel::Info);
            return Ok(result.into_iter().next().unwrap_or(Value::Null));
        }

        let db = self.with_namespace_remote(namespace).await?;
        let mut response = db
            .query(&sql)
            .bind(("content", content_for_create))
            .await
            .map_err(|err| {
                let mut e = event.clone();
                e.insert("error".to_string(), Value::String(err.to_string()));
                self.logger.log(e, LogLevel::Warn);
                MemoryError::Storage(format!("SurrealDB create failed: {err}"))
            })?;
        let result: Vec<Value> = response.take(0).unwrap_or_default();
        let mut done = event.clone();
        done.insert("result".to_string(), Value::String("ok".to_string()));
        self.logger.log(done, LogLevel::Info);
        Ok(result.into_iter().next().unwrap_or(Value::Null))
    }

    async fn update(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        let mut event = std::collections::HashMap::new();
        event.insert("op".to_string(), Value::String("db.update".to_string()));
        event.insert(
            "record_id".to_string(),
            Value::String(record_id.to_string()),
        );
        event.insert(
            "namespace".to_string(),
            Value::String(namespace.to_string()),
        );
        if let Value::Object(map) = &content {
            event.insert(
                "fields".to_string(),
                Value::Number(serde_json::Number::from(map.len())),
            );
        }
        self.logger.log(event.clone(), LogLevel::Debug);

        let (table, id) = if let Some(idx) = record_id.find(':') {
            (&record_id[..idx], &record_id[idx + 1..])
        } else {
            return Err(MemoryError::Storage(format!(
                "Invalid record_id format: expected 'table:id', got '{}'",
                record_id
            )));
        };
        let sql = format!("UPDATE {}:{} MERGE $content RETURN *", table, id);
        let content_for_update = if let Value::Object(mut map) = content {
            map.remove("id");
            Value::Object(map)
        } else {
            content
        };

        if self.db_local().is_some() {
            let db = self.with_namespace_local(namespace).await?;
            let mut response = db
                .query(&sql)
                .bind(("content", content_for_update.clone()))
                .await
                .map_err(|err| {
                    let mut e = event.clone();
                    e.insert("error".to_string(), Value::String(err.to_string()));
                    self.logger.log(e, LogLevel::Warn);
                    MemoryError::Storage(format!("SurrealDB update failed: {err}"))
                })?;
            let surreal_val: surrealdb::Value = response
                .take(0)
                .map_err(|err| MemoryError::Storage(format!("SurrealDB update failed: {err}")))?;
            let json = serde_json::to_value(&surreal_val).map_err(|err| {
                MemoryError::Storage(format!("SurrealDB -> JSON conversion failed: {err}"))
            })?;
            let normalized = normalize_surreal_json(&json);
            let result: Vec<Value> = match normalized {
                Value::Array(arr) => arr,
                Value::Null => Vec::new(),
                other => vec![other],
            };
            let mut done = event.clone();
            done.insert("result".to_string(), Value::String("ok".to_string()));
            self.logger.log(done, LogLevel::Info);
            return Ok(result.into_iter().next().unwrap_or(Value::Null));
        }

        let db = self.with_namespace_remote(namespace).await?;
        let mut response = db
            .query(&sql)
            .bind(("content", content_for_update))
            .await
            .map_err(|err| {
                let mut e = event.clone();
                e.insert("error".to_string(), Value::String(err.to_string()));
                self.logger.log(e, LogLevel::Warn);
                MemoryError::Storage(format!("SurrealDB update failed: {err}"))
            })?;
        let result: Vec<Value> = response.take(0).unwrap_or_default();
        let mut done = event.clone();
        done.insert("result".to_string(), Value::String("ok".to_string()));
        self.logger.log(done, LogLevel::Info);
        Ok(result.into_iter().next().unwrap_or(Value::Null))
    }

    async fn query(
        &self,
        sql: &str,
        vars: Option<Value>,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        let mut event = std::collections::HashMap::new();
        event.insert("op".to_string(), Value::String("db.query".to_string()));
        event.insert("sql".to_string(), Value::String(sql.to_string()));
        event.insert(
            "namespace".to_string(),
            Value::String(namespace.to_string()),
        );
        if let Some(Value::Object(map)) = &vars {
            // don't log full vars at info level; include keys or summary
            event.insert(
                "vars_count".to_string(),
                Value::Number(serde_json::Number::from(map.len())),
            );
        }
        self.logger.log(event.clone(), LogLevel::Debug);

        if self.db_local().is_some() {
            let db = self.with_namespace_local(namespace).await?;
            let mut query = db.query(sql);
            if let Some(vars) = vars.clone() {
                query = query.bind(vars);
            }
            let _response = query.await.map_err(|err| {
                let mut e = event.clone();
                e.insert("error".to_string(), Value::String(err.to_string()));
                self.logger.log(e, LogLevel::Warn);
                MemoryError::Storage(format!("SurrealDB query failed: {err}"))
            })?;
            let mut done = event.clone();
            done.insert("result".to_string(), Value::String("ok".to_string()));
            self.logger.log(done, LogLevel::Debug);
            Ok(Value::Null)
        } else {
            let db = self.with_namespace_remote(namespace).await?;
            let mut query = db.query(sql);
            if let Some(vars) = vars {
                query = query.bind(vars);
            }
            let _response = query.await.map_err(|err| {
                let mut e = event.clone();
                e.insert("error".to_string(), Value::String(err.to_string()));
                self.logger.log(e, LogLevel::Warn);
                MemoryError::Storage(format!("SurrealDB query failed: {err}"))
            })?;
            let mut done = event.clone();
            done.insert("result".to_string(), Value::String("ok".to_string()));
            self.logger.log(done, LogLevel::Debug);
            Ok(Value::Null)
        }
    }

    async fn select_facts_filtered(
        &self,
        namespace: &str,
        scope: &str,
        cutoff: &str,
        query_contains: Option<&str>,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        let mut event = std::collections::HashMap::new();
        event.insert(
            "op".to_string(),
            Value::String("db.select_facts_filtered".to_string()),
        );
        event.insert("scope".to_string(), Value::String(scope.to_string()));
        event.insert("cutoff".to_string(), Value::String(cutoff.to_string()));
        event.insert(
            "namespace".to_string(),
            Value::String(namespace.to_string()),
        );
        event.insert(
            "limit".to_string(),
            Value::Number(serde_json::Number::from(limit)),
        );
        self.logger.log(event.clone(), LogLevel::Debug);

        // Build SURQL query with parameterized WHERE clause
        // Filter: scope, bi-temporal cutoff, optional full-text search
        // Strategy: use SurrealDB full-text search (@@) via fact_content_search index,
        // then fall back to per-word CONTAINS OR if FTS returns no results.
        let base_where = "scope = $scope AND string::is::datetime(t_valid) AND t_valid <= $cutoff AND (t_ingested IS NONE OR (string::is::datetime(t_ingested) AND t_ingested <= $cutoff)) AND (t_invalid IS NONE OR t_invalid > $cutoff OR t_invalid_ingested > $cutoff)";

        // Primary query: full-text search with @@ operator (uses SEARCH index)
        let (sql_fts, sql_fallback) = if let Some(q) = query_contains {
            // Build per-word fallback: each word matched via case-insensitive CONTAINS (OR)
            let words: Vec<&str> = q.split_whitespace().filter(|w| w.len() >= 2).collect();
            let fallback_clause = if words.is_empty() {
                String::new()
            } else {
                let word_conditions: Vec<String> = words
                    .iter()
                    .enumerate()
                    .map(|(i, _)| {
                        format!(
                            "string::lowercase(content) CONTAINS string::lowercase($word{})",
                            i
                        )
                    })
                    .collect();
                format!(" AND ({})", word_conditions.join(" OR "))
            };

            let primary = format!(
                "SELECT * FROM fact WHERE {} AND content @@ $query ORDER BY t_valid DESC LIMIT $limit",
                base_where
            );
            let fallback = format!(
                "SELECT * FROM fact WHERE {}{} ORDER BY t_valid DESC LIMIT $limit",
                base_where, fallback_clause
            );
            (Some(primary), Some(fallback))
        } else {
            (None, None)
        };

        let sql_no_query = format!(
            "SELECT * FROM fact WHERE {} ORDER BY t_valid DESC LIMIT $limit",
            base_where
        );

        let mut vars_base = serde_json::json!({
            "scope": scope,
            "cutoff": cutoff,
            "limit": limit
        });

        // Add query + per-word params
        if let Some(q) = query_contains {
            vars_base["query"] = serde_json::Value::String(q.to_string());
            let words: Vec<&str> = q.split_whitespace().filter(|w| w.len() >= 2).collect();
            for (i, word) in words.iter().enumerate() {
                vars_base[format!("word{}", i)] = serde_json::Value::String(word.to_string());
            }
        }

        let vars = vars_base;

        // Helper: execute a SURQL query and return normalized Vec<Value>
        async fn execute_query(
            db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
            sql: &str,
            vars: serde_json::Value,
        ) -> Result<Vec<Value>, MemoryError> {
            let mut response = db.query(sql).bind(vars).await.map_err(|err| {
                MemoryError::Storage(format!("SurrealDB select_facts_filtered failed: {err}"))
            })?;
            let surreal_val: surrealdb::Value = response.take(0).map_err(|err| {
                MemoryError::Storage(format!("SurrealDB response deserialize failed: {err}"))
            })?;
            let json = serde_json::to_value(&surreal_val).map_err(|err| {
                MemoryError::Storage(format!("SurrealDB -> JSON conversion failed: {err}"))
            })?;
            let normalized = normalize_surreal_json(&json);
            Ok(match normalized {
                Value::Array(arr) => arr,
                Value::Null => Vec::new(),
                other => vec![other],
            })
        }

        async fn execute_query_remote(
            db: &surrealdb::Surreal<Client>,
            sql: &str,
            vars: serde_json::Value,
        ) -> Result<Vec<Value>, MemoryError> {
            let mut response = db.query(sql).bind(vars).await.map_err(|err| {
                MemoryError::Storage(format!("SurrealDB select_facts_filtered failed: {err}"))
            })?;
            let surreal_val: surrealdb::Value = response.take(0).map_err(|err| {
                MemoryError::Storage(format!("SurrealDB response deserialize failed: {err}"))
            })?;
            let json = serde_json::to_value(&surreal_val).map_err(|err| {
                MemoryError::Storage(format!("SurrealDB -> JSON conversion failed: {err}"))
            })?;
            let normalized = normalize_surreal_json(&json);
            Ok(match normalized {
                Value::Array(arr) => arr,
                Value::Null => Vec::new(),
                other => vec![other],
            })
        }

        // Determine which SQL to run: FTS primary → fallback → no-query
        let results = if self.db_local().is_some() {
            let db = self.with_namespace_local(namespace).await?;
            if let Some(ref fts_sql) = sql_fts {
                // Try full-text search first
                let fts_results = execute_query(&db, fts_sql, vars.clone()).await;
                match fts_results {
                    Ok(r) if !r.is_empty() => r,
                    _ => {
                        // Fallback to per-word CONTAINS OR
                        if let Some(ref fb_sql) = sql_fallback {
                            self.logger.log(
                                {
                                    let mut e = event.clone();
                                    e.insert(
                                        "fallback".to_string(),
                                        Value::String("per_word_contains".to_string()),
                                    );
                                    e
                                },
                                LogLevel::Debug,
                            );
                            execute_query(&db, fb_sql, vars.clone())
                                .await
                                .unwrap_or_default()
                        } else {
                            Vec::new()
                        }
                    }
                }
            } else {
                execute_query(&db, &sql_no_query, vars.clone())
                    .await
                    .unwrap_or_default()
            }
        } else {
            let db = self.with_namespace_remote(namespace).await?;
            if let Some(ref fts_sql) = sql_fts {
                let fts_results = execute_query_remote(&db, fts_sql, vars.clone()).await;
                match fts_results {
                    Ok(r) if !r.is_empty() => r,
                    _ => {
                        if let Some(ref fb_sql) = sql_fallback {
                            self.logger.log(
                                {
                                    let mut e = event.clone();
                                    e.insert(
                                        "fallback".to_string(),
                                        Value::String("per_word_contains".to_string()),
                                    );
                                    e
                                },
                                LogLevel::Debug,
                            );
                            execute_query_remote(&db, fb_sql, vars.clone())
                                .await
                                .unwrap_or_default()
                        } else {
                            Vec::new()
                        }
                    }
                }
            } else {
                execute_query_remote(&db, &sql_no_query, vars.clone())
                    .await
                    .unwrap_or_default()
            }
        };

        let mut done = event.clone();
        done.insert(
            "count".to_string(),
            Value::Number(serde_json::Number::from(results.len())),
        );
        self.logger.log(done, LogLevel::Info);
        Ok(results)
    }

    async fn select_edges_filtered(
        &self,
        namespace: &str,
        cutoff: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        let mut event = std::collections::HashMap::new();
        event.insert(
            "op".to_string(),
            Value::String("db.select_edges_filtered".to_string()),
        );
        event.insert("cutoff".to_string(), Value::String(cutoff.to_string()));
        event.insert(
            "namespace".to_string(),
            Value::String(namespace.to_string()),
        );
        self.logger.log(event.clone(), LogLevel::Debug);

        // Filter: t_valid <= cutoff AND t_ingested <= cutoff
        // and (t_invalid IS NULL OR t_invalid > cutoff OR t_invalid_ingested > cutoff)
        let sql = String::from(
            "SELECT * FROM edge WHERE string::is::datetime(t_valid) AND t_valid <= $cutoff AND (t_ingested IS NONE OR (string::is::datetime(t_ingested) AND t_ingested <= $cutoff)) AND (t_invalid IS NONE OR t_invalid > $cutoff OR t_invalid_ingested > $cutoff) ORDER BY from_id ASC, to_id ASC, t_valid DESC",
        );

        let vars = serde_json::json!({
            "cutoff": cutoff,
        });

        if self.db_local().is_some() {
            let db = self.with_namespace_local(namespace).await?;
            let mut response = db.query(&sql).bind(vars).await.map_err(|err| {
                let mut e = event.clone();
                e.insert("error".to_string(), Value::String(err.to_string()));
                self.logger.log(e, LogLevel::Warn);
                MemoryError::Storage(format!("SurrealDB select_edges_filtered failed: {err}"))
            })?;

            let surreal_val: surrealdb::Value = response.take(0).map_err(|err| {
                MemoryError::Storage(format!("SurrealDB response deserialize failed: {err}"))
            })?;
            let json = serde_json::to_value(&surreal_val).map_err(|err| {
                MemoryError::Storage(format!("SurrealDB -> JSON conversion failed: {err}"))
            })?;
            let normalized = normalize_surreal_json(&json);

            let results: Vec<Value> = match normalized {
                Value::Array(arr) => arr,
                Value::Null => Vec::new(),
                other => vec![other],
            };

            let mut done = event.clone();
            done.insert(
                "count".to_string(),
                Value::Number(serde_json::Number::from(results.len())),
            );
            self.logger.log(done, LogLevel::Info);
            return Ok(results);
        }

        let db = self.with_namespace_remote(namespace).await?;
        let mut response = db.query(&sql).bind(vars).await.map_err(|err| {
            let mut e = event.clone();
            e.insert("error".to_string(), Value::String(err.to_string()));
            self.logger.log(e, LogLevel::Warn);
            MemoryError::Storage(format!("SurrealDB select_edges_filtered failed: {err}"))
        })?;

        let surreal_val: surrealdb::Value = response.take(0).map_err(|err| {
            MemoryError::Storage(format!("SurrealDB response deserialize failed: {err}"))
        })?;
        let json = serde_json::to_value(&surreal_val).map_err(|err| {
            MemoryError::Storage(format!("SurrealDB -> JSON conversion failed: {err}"))
        })?;
        let normalized = normalize_surreal_json(&json);

        let results: Vec<Value> = match normalized {
            Value::Array(arr) => arr,
            Value::Null => Vec::new(),
            other => vec![other],
        };

        let mut done = event.clone();
        done.insert(
            "count".to_string(),
            Value::Number(serde_json::Number::from(results.len())),
        );
        self.logger.log(done, LogLevel::Info);
        Ok(results)
    }

    async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError> {
        // Delegate to the inherent method implemented on SurrealDbClient
        SurrealDbClient::apply_migrations(self, namespace).await
    }
}

enum MigrationSource {
    Filesystem(std::path::PathBuf),
    Embedded,
    None,
}

fn migration_source(fs_dir: &std::path::Path) -> MigrationSource {
    if fs_dir.exists() {
        return MigrationSource::Filesystem(fs_dir.to_path_buf());
    }
    if EMBEDDED_MIGRATIONS.files().next().is_some() {
        return MigrationSource::Embedded;
    }
    MigrationSource::None
}

use include_dir::{Dir, include_dir};

static EMBEDDED_MIGRATIONS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/src/migrations");

impl SurrealDbClient {
    /// Execute query and deserialize the first result set into type `T`.
    pub async fn query_get<T: serde::de::DeserializeOwned>(
        &self,
        sql: &str,
        namespace: &str,
    ) -> Result<T, MemoryError> {
        // The SurrealDB `Response::take` API requires a type that implements
        // `opt::QueryResult<T>` for the index type (usize). That does not hold
        // for arbitrary `T`. To keep things simple and consistent across both
        // embedded (RocksDB) and remote engines we first take the raw
        // `surrealdb::sql::Value` and then deserialize it into `T` via
        // serde_json conversions.
        if self.db_local().is_some() {
            let db = self.with_namespace_local(namespace).await?;
            let mut response = db
                .query(sql)
                .await
                .map_err(|err| MemoryError::Storage(format!("SurrealDB query failed: {err}")))?;
            let s_val: surrealdb::Value = response.take(0).map_err(|err| {
                MemoryError::Storage(format!("SurrealDB response deserialize failed: {err}"))
            })?;
            let json = serde_json::to_value(&s_val).map_err(|err| {
                MemoryError::Storage(format!("SurrealDB -> JSON conversion failed: {err}"))
            })?;

            let normalized = normalize_surreal_json(&json);
            // First try to deserialize directly
            if let Ok(res) = serde_json::from_value(normalized.clone()) {
                return Ok(res);
            }
            // If direct deserialization fails and we have an array of single-field objects,
            // try to extract inner values (e.g., SELECT name FROM ... -> [{"name":"Alice"}])
            if let serde_json::Value::Array(arr) = normalized {
                let inner: Vec<serde_json::Value> = arr
                    .into_iter()
                    .map(|el| {
                        if let serde_json::Value::Object(map) = el {
                            if map.len() == 1 {
                                map.into_iter()
                                    .next()
                                    .expect("map with len 1 has one entry")
                                    .1
                            } else {
                                serde_json::Value::Object(map)
                            }
                        } else {
                            el
                        }
                    })
                    .collect();
                if let Ok(res) = serde_json::from_value(serde_json::Value::Array(inner)) {
                    return Ok(res);
                }
            }
            Err(MemoryError::Storage(
                "Deserializing query result failed".to_string(),
            ))
        } else {
            let db = self.with_namespace_remote(namespace).await?;
            let mut response = db
                .query(sql)
                .await
                .map_err(|err| MemoryError::Storage(format!("SurrealDB query failed: {err}")))?;
            let s_val: surrealdb::Value = response.take(0).map_err(|err| {
                MemoryError::Storage(format!("SurrealDB response deserialize failed: {err}"))
            })?;
            let json = serde_json::to_value(&s_val).map_err(|err| {
                MemoryError::Storage(format!("SurrealDB -> JSON conversion failed: {err}"))
            })?;

            let normalized = normalize_surreal_json(&json);
            // First try to deserialize directly
            if let Ok(res) = serde_json::from_value(normalized.clone()) {
                return Ok(res);
            }
            // If direct deserialization fails and we have an array of single-field objects,
            // try to extract inner values (e.g., SELECT name FROM ... -> [{"name":"Alice"}])
            if let serde_json::Value::Array(arr) = normalized {
                let inner: Vec<serde_json::Value> = arr
                    .into_iter()
                    .map(|el| {
                        if let serde_json::Value::Object(map) = el {
                            if map.len() == 1 {
                                map.into_iter().next().unwrap().1
                            } else {
                                serde_json::Value::Object(map)
                            }
                        } else {
                            el
                        }
                    })
                    .collect();
                if let Ok(res) = serde_json::from_value(serde_json::Value::Array(inner)) {
                    return Ok(res);
                }
            }
            Err(MemoryError::Storage(
                "Deserializing query result failed".to_string(),
            ))
        }
    }
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

/// Normalize a SurrealDB-tagged JSON value into plain JSON.
///
/// SurrealDB serializes SQL values using tagged enums, e.g.:
/// {"Array":[{"Object":{"name":{"Strand":"Alice"}}}]}
/// This helper converts that into: [{"name":"Alice"}]
fn normalize_surreal_json(v: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        J::Object(map) if map.len() == 1 => {
            let (k, val) = map.iter().next().expect("map with len 1 has one entry");
            match k.as_str() {
                "Array" => {
                    if let J::Array(items) = val {
                        let arr: Vec<J> = items.iter().map(normalize_surreal_json).collect();
                        J::Array(arr)
                    } else {
                        val.clone()
                    }
                }
                "Object" => {
                    if let J::Object(inner) = val {
                        let mut out = serde_json::Map::new();
                        for (ik, iv) in inner.iter() {
                            out.insert(ik.clone(), normalize_surreal_json(iv));
                        }
                        J::Object(out)
                    } else {
                        val.clone()
                    }
                }
                "Strand" => {
                    // Strand holds a string, sometimes wrapped as {"String": "..."}
                    if let J::Object(inner) = val {
                        if let Some(s) = inner.get("String") {
                            s.clone()
                        } else {
                            val.clone()
                        }
                    } else {
                        val.clone()
                    }
                }
                "String" => val.clone(),
                _ => normalize_surreal_json(val),
            }
        }
        J::Array(arr) => J::Array(arr.iter().map(normalize_surreal_json).collect()),
        _ => v.clone(),
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;

    #[test]
    fn migration_source_prefers_filesystem() {
        let dir = tempfile::tempdir().expect("tempdir");
        let migrations_dir = dir.path().join("migrations");
        std::fs::create_dir_all(&migrations_dir).expect("create migrations dir");

        let source = migration_source(&migrations_dir);
        assert!(matches!(source, MigrationSource::Filesystem(path) if path == migrations_dir));
    }

    #[test]
    fn migration_source_falls_back_to_embedded_when_fs_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let migrations_dir = dir.path().join("missing");

        let source = migration_source(&migrations_dir);
        assert!(matches!(source, MigrationSource::Embedded));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;

    // === Normalization tests ===

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
    fn normalize_surreal_json_handles_nested_structures() {
        let nested = json!({
            "Object": {
                "user": {"Object": {
                    "name": {"Strand": "Alice"},
                    "age": 30
                }},
                "tags": {"Array": [
                    {"String": "rust"},
                    {"Strand": "async"}
                ]}
            }
        });

        let normalized = normalize_surreal_json(&nested);
        assert_eq!(
            normalized,
            json!({
                "user": {
                    "name": "Alice",
                    "age": 30
                },
                "tags": ["rust", "async"]
            })
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

    // === Mock DbClient for unit tests ===

    #[derive(Default)]
    struct MockDbClient {
        data: Mutex<std::collections::HashMap<String, Value>>,
    }

    impl MockDbClient {
        fn new() -> Self {
            Self::default()
        }

        fn insert(&self, key: &str, value: Value) {
            self.data.lock().unwrap().insert(key.to_string(), value);
        }
    }

    #[async_trait]
    impl DbClient for MockDbClient {
        async fn select_one(
            &self,
            record_id: &str,
            _namespace: &str,
        ) -> Result<Option<Value>, crate::service::MemoryError> {
            Ok(self.data.lock().unwrap().get(record_id).cloned())
        }

        async fn select_table(
            &self,
            table: &str,
            _namespace: &str,
        ) -> Result<Vec<Value>, crate::service::MemoryError> {
            let data = self.data.lock().unwrap();
            Ok(data
                .iter()
                .filter(|(k, _)| k.starts_with(&format!("{}:", table)))
                .map(|(_, v)| v.clone())
                .collect())
        }

        async fn create(
            &self,
            record_id: &str,
            mut content: Value,
            _namespace: &str,
        ) -> Result<Value, crate::service::MemoryError> {
            if let Value::Object(ref mut map) = content {
                map.insert("id".to_string(), Value::String(record_id.to_string()));
            }
            self.insert(record_id, content.clone());
            Ok(content)
        }

        async fn update(
            &self,
            record_id: &str,
            content: Value,
            _namespace: &str,
        ) -> Result<Value, crate::service::MemoryError> {
            let mut data = self.data.lock().unwrap();
            let existing = data
                .get_mut(record_id)
                .ok_or_else(|| crate::service::MemoryError::NotFound("record not found".into()))?;

            if let (Value::Object(existing_map), Value::Object(update_map)) =
                (existing, content.clone())
            {
                for (k, v) in update_map {
                    existing_map.insert(k, v);
                }
            }
            Ok(data.get(record_id).cloned().unwrap_or(Value::Null))
        }

        async fn query(
            &self,
            _sql: &str,
            _vars: Option<Value>,
            _namespace: &str,
        ) -> Result<Value, crate::service::MemoryError> {
            Ok(Value::Null)
        }

        async fn select_facts_filtered(
            &self,
            _namespace: &str,
            scope: &str,
            cutoff: &str,
            query_contains: Option<&str>,
            limit: i32,
        ) -> Result<Vec<Value>, crate::service::MemoryError> {
            let data = self.data.lock().unwrap();
            let query_lower = query_contains.map(|q| q.to_lowercase());

            let mut filtered: Vec<Value> = data
                .iter()
                .filter(|(k, _)| k.starts_with("fact:"))
                .map(|(_, v)| v.clone())
                .filter(|f| {
                    let f_scope = f.get("scope").and_then(Value::as_str).unwrap_or_default();
                    let t_valid = f.get("t_valid").and_then(Value::as_str).unwrap_or_default();
                    let t_ingested = f
                        .get("t_ingested")
                        .and_then(Value::as_str)
                        .unwrap_or(t_valid);
                    let t_invalid = f.get("t_invalid").and_then(Value::as_str);
                    let t_invalid_ingested = f.get("t_invalid_ingested").and_then(Value::as_str);

                    if f_scope != scope {
                        return false;
                    }
                    if t_valid > cutoff {
                        return false;
                    }
                    if t_ingested > cutoff {
                        return false;
                    }
                    if let Some(inv) = t_invalid
                        && !inv.is_empty()
                        && inv <= cutoff
                    {
                        let invalid_known = t_invalid_ingested
                            .map(|ingested| !ingested.is_empty() && ingested <= cutoff)
                            .unwrap_or(true);
                        if invalid_known {
                            return false;
                        }
                    }
                    if let Some(ref q) = query_lower {
                        let content = f
                            .get("content")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_lowercase();
                        if !content.contains(q) {
                            return false;
                        }
                    }
                    true
                })
                .collect();

            filtered.sort_by(|a, b| {
                let ta = a.get("t_valid").and_then(Value::as_str).unwrap_or_default();
                let tb = b.get("t_valid").and_then(Value::as_str).unwrap_or_default();
                tb.cmp(ta)
            });

            filtered.truncate(limit.max(1) as usize);
            Ok(filtered)
        }

        async fn select_edges_filtered(
            &self,
            _namespace: &str,
            cutoff: &str,
        ) -> Result<Vec<Value>, crate::service::MemoryError> {
            let data = self.data.lock().unwrap();

            let mut filtered: Vec<Value> = data
                .iter()
                .filter(|(k, _)| k.starts_with("edge:"))
                .map(|(_, v)| v.clone())
                .filter(|e| {
                    let t_valid = e.get("t_valid").and_then(Value::as_str).unwrap_or_default();
                    let t_ingested = e
                        .get("t_ingested")
                        .and_then(Value::as_str)
                        .unwrap_or(t_valid);
                    let t_invalid = e.get("t_invalid").and_then(Value::as_str);
                    let t_invalid_ingested = e.get("t_invalid_ingested").and_then(Value::as_str);

                    if t_valid > cutoff {
                        return false;
                    }
                    if t_ingested > cutoff {
                        return false;
                    }
                    if let Some(inv) = t_invalid
                        && !inv.is_empty()
                        && inv <= cutoff
                    {
                        let invalid_known = t_invalid_ingested
                            .map(|ingested| !ingested.is_empty() && ingested <= cutoff)
                            .unwrap_or(true);
                        if invalid_known {
                            return false;
                        }
                    }
                    true
                })
                .collect();

            filtered.sort_by(|a, b| {
                let from_a = a.get("from_id").and_then(Value::as_str).unwrap_or_default();
                let from_b = b.get("from_id").and_then(Value::as_str).unwrap_or_default();
                let to_a = a.get("to_id").and_then(Value::as_str).unwrap_or_default();
                let to_b = b.get("to_id").and_then(Value::as_str).unwrap_or_default();
                from_a.cmp(from_b).then_with(|| to_a.cmp(to_b))
            });

            Ok(filtered)
        }

        async fn apply_migrations(
            &self,
            _namespace: &str,
        ) -> Result<(), crate::service::MemoryError> {
            Ok(())
        }
    }

    // === DbClient method tests ===

    #[tokio::test]
    async fn select_one_returns_existing_record() {
        let client = MockDbClient::new();
        let record = json!({"name": "Alice", "age": 30});
        client.insert("user:1", record.clone());

        let result = client.select_one("user:1", "test").await.unwrap();
        assert_eq!(result, Some(record));
    }

    #[tokio::test]
    async fn select_one_returns_none_for_missing_record() {
        let client = MockDbClient::new();
        let result = client.select_one("user:999", "test").await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn select_table_returns_all_matching_records() {
        let client = MockDbClient::new();
        client.insert("user:1", json!({"name": "Alice"}));
        client.insert("user:2", json!({"name": "Bob"}));
        client.insert("post:1", json!({"title": "Hello"}));

        let users = client.select_table("user", "test").await.unwrap();
        assert_eq!(users.len(), 2);
    }

    #[tokio::test]
    async fn select_table_returns_empty_for_missing_table() {
        let client = MockDbClient::new();
        let result = client.select_table("nonexistent", "test").await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn create_inserts_record_with_id() {
        let client = MockDbClient::new();
        let content = json!({"name": "Alice"});

        let result = client.create("user:1", content, "test").await.unwrap();
        assert_eq!(result["id"], "user:1");
        assert_eq!(result["name"], "Alice");

        let stored = client.select_one("user:1", "test").await.unwrap();
        assert!(stored.is_some());
    }

    #[tokio::test]
    async fn create_handles_table_only_id() {
        let client = MockDbClient::new();
        let content = json!({"title": "Post"});

        let result = client.create("post", content, "test").await.unwrap();
        assert_eq!(result["id"], "post");
    }

    #[tokio::test]
    async fn update_merges_content_into_existing_record() {
        let client = MockDbClient::new();
        client.insert("user:1", json!({"name": "Alice", "age": 30}));

        let update = json!({"age": 31, "city": "NYC"});
        let result = client.update("user:1", update, "test").await.unwrap();

        assert_eq!(result["name"], "Alice");
        assert_eq!(result["age"], 31);
        assert_eq!(result["city"], "NYC");
    }

    #[tokio::test]
    async fn update_returns_error_for_missing_record() {
        let client = MockDbClient::new();
        let update = json!({"age": 31});

        let result = client.update("user:999", update, "test").await;
        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(crate::service::MemoryError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn query_executes_without_error() {
        let client = MockDbClient::new();
        let result = client.query("SELECT * FROM user", None, "test").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn apply_migrations_succeeds() {
        let client = MockDbClient::new();
        let result = client.apply_migrations("test").await;
        assert!(result.is_ok());
    }
}
