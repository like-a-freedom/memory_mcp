//! SurrealDB client implementation.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Value, json};
use surrealdb::Surreal;
use surrealdb::engine::local::Db;
use surrealdb::engine::local::Mem;
use surrealdb::engine::local::RocksDb;
use surrealdb::engine::remote::ws::{Client, Ws};
use surrealdb::opt::auth::Root;
use surrealdb::types::Value as SurrealValue;

use crate::config::{StorageBackend, SurrealConfig};
use crate::logging::{LogLevel, StdoutLogger};
use crate::service::MemoryError;

use super::helpers::{
    ensure_dir_exists, extract_first_record, extract_records, find_version_in_json,
    is_missing_table_error, normalize_url, surreal_to_json,
};
use super::migrations::{
    MigrationScript, migration_checksum, migration_has_statements, migration_record_id,
    validate_applied_migration, versioned_migrations,
};
use super::queries::{build_create_query, build_select_one_query, build_update_query};

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

    /// Executes a raw SQL query and returns JSON results.
    async fn query(
        &self,
        sql: &str,
        vars: Option<Value>,
        namespace: &str,
    ) -> Result<Value, MemoryError>;

    /// Applies database migrations for a namespace.
    async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError>;
}

/// Storage capability used by Context Assembly.
///
/// This deliberately exposes only retrieval, graph-expansion, provenance, and
/// access-log operations needed by the context domain. The legacy [`DbClient`]
/// remains the infrastructure adapter for now; callers in Context Assembly
/// depend on this narrower seam instead of the full database surface.
pub struct ContextFactQuery<'a> {
    pub namespace: &'a str,
    pub scope: &'a str,
    pub cutoff: &'a str,
    pub query_contains: Option<&'a str>,
    pub limit: i32,
    pub project: Option<&'a str>,
    pub fact_types: &'a [String],
}

/// Unified database client that works with both embedded and remote SurrealDB.
pub struct SurrealDbClient {
    engine: DbEngine,
    logger: StdoutLogger,
    fact_embedding_dimension: usize,
}

/// Internal enum representing the database engine type.
enum DbEngine {
    Local(HashMap<String, Arc<Surreal<Db>>>),
    Remote(HashMap<String, Arc<Surreal<Client>>>),
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
        Self::connect_in_memory_with_namespaces(
            database,
            &[default_namespace.to_string()],
            log_level,
        )
        .await
    }

    /// Connects to an embedded in-memory SurrealDB instance for multiple namespaces.
    pub async fn connect_in_memory_with_namespaces(
        database: &str,
        namespaces: &[String],
        log_level: &str,
    ) -> Result<Self, MemoryError> {
        let db = Surreal::new::<Mem>(())
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB memory init failed: {err}")))?;
        let clients = build_namespace_clients(&db, namespaces, database).await?;

        Ok(Self {
            engine: DbEngine::Local(clients),
            logger: StdoutLogger::new(log_level),
            fact_embedding_dimension: crate::config::DEFAULT_EMBEDDING_DIMENSION,
        })
    }

    /// Connects to SurrealDB using the provided configuration.
    pub async fn connect(
        config: &SurrealConfig,
        default_namespace: &str,
    ) -> Result<Self, MemoryError> {
        let engine = match StorageBackend::from_embedded(config.embedded) {
            StorageBackend::Embedded => Self::connect_embedded(config, default_namespace).await?,
            StorageBackend::Remote => Self::connect_remote(config, default_namespace).await?,
        };

        Ok(Self {
            engine,
            logger: StdoutLogger::new(&config.log_level),
            fact_embedding_dimension: config.embedding.fallback_dimension(),
        })
    }

    /// Connects to embedded RocksDB instance.
    async fn connect_embedded(
        config: &SurrealConfig,
        _default_namespace: &str,
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
            .capabilities(Capabilities::default());

        let db = Surreal::new::<RocksDb>((data_dir, cfg))
            .await
            .map_err(|err| {
                MemoryError::Storage(format!("SurrealDB embedded init failed: {err}"))
            })?;

        db.signin(root)
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB signin failed: {err}")))?;

        let clients = build_namespace_clients(&db, &config.namespaces, &config.db_name).await?;

        Ok(DbEngine::Local(clients))
    }

    /// Connects to remote WebSocket instance.
    async fn connect_remote(
        config: &SurrealConfig,
        _default_namespace: &str,
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

        let clients = build_namespace_clients(&db, &config.namespaces, &config.db_name).await?;

        Ok(DbEngine::Remote(clients))
    }

    /// Gets a database handle with namespace set.
    async fn with_namespace_local(&self, namespace: &str) -> Result<Arc<Surreal<Db>>, MemoryError> {
        match &self.engine {
            DbEngine::Local(clients) => clients.get(namespace).cloned().ok_or_else(|| {
                MemoryError::Storage(format!("SurrealDB namespace not initialized: {namespace}"))
            }),
            DbEngine::Remote(_) => Err(MemoryError::Storage("expected local engine".into())),
        }
    }

    /// Gets a database handle with namespace set.
    async fn with_namespace_remote(
        &self,
        namespace: &str,
    ) -> Result<Arc<Surreal<Client>>, MemoryError> {
        match &self.engine {
            DbEngine::Remote(clients) => clients.get(namespace).cloned().ok_or_else(|| {
                MemoryError::Storage(format!("SurrealDB namespace not initialized: {namespace}"))
            }),
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
        let mut event = HashMap::new();
        event.insert("op".to_string(), Value::String(op.to_string()));
        for (key, value) in details {
            event.insert(key.to_string(), value);
        }
        self.logger.log(event, LogLevel::Debug);
    }

    /// Applies database schema migrations.
    pub async fn apply_migrations_impl(&self, namespace: &str) -> Result<(), MemoryError> {
        let initial_schema = render_sql_template(
            include_str!("../../migrations/__Initial.surql"),
            self.fact_embedding_dimension,
        );

        // Initial migration may fail with "table already exists" if database was not cleanly shut down
        // or if tables were created by a previous version. We tolerate this error for idempotency.
        match self
            .execute_raw_query(&initial_schema, None, namespace)
            .await
        {
            Ok(()) => {}
            Err(MemoryError::Storage(err_msg))
                if super::helpers::is_table_already_exists_error(&err_msg) =>
            {
                self.logger.log(
                    HashMap::from([(
                        "op".to_string(),
                        Value::String("schema.init.skipped".to_string()),
                    )]),
                    LogLevel::Debug,
                );
            }
            Err(e) => return Err(e),
        }

        for migration in versioned_migrations() {
            self.apply_versioned_migration(namespace, migration).await?;
        }

        self.logger.log(
            HashMap::from([
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

    async fn apply_versioned_migration(
        &self,
        namespace: &str,
        migration: &MigrationScript,
    ) -> Result<(), MemoryError> {
        let record_id = migration_record_id(migration.file_name);
        let rendered_sql = render_sql_template(migration.sql, self.fact_embedding_dimension);
        let checksum = migration_checksum(&rendered_sql);

        if let Some(existing) = self.select_one(&record_id, namespace).await? {
            validate_applied_migration(&existing, migration.file_name, &checksum)?;
            return Ok(());
        }

        if migration_has_statements(&rendered_sql) {
            self.execute_raw_query(&rendered_sql, None, namespace)
                .await?;
        }

        self.create(
            &record_id,
            json!({
                "script_name": migration.file_name,
                "checksum": checksum,
                "executed_at": chrono::Utc::now().to_rfc3339(),
            }),
            namespace,
        )
        .await?;

        Ok(())
    }

    /// Execute a query that returns a SurrealValue (internal helper).
    async fn execute_query(
        &self,
        sql: &str,
        vars: Option<Value>,
        namespace: &str,
    ) -> Result<SurrealValue, MemoryError> {
        let timer = Instant::now();
        self.logger.log(
            build_db_execute_event(
                "db.execute_query.start",
                namespace,
                sql,
                vars.as_ref(),
                None,
                None,
            ),
            LogLevel::Debug,
        );

        let vars_for_retry = vars.clone();
        let result = with_db_retry("execute_query", &self.logger, || {
            self.execute_sql_with_timing(sql, vars_for_retry.clone(), namespace)
        })
        .await;

        match result {
            Ok(value) => {
                self.logger.log(
                    build_db_execute_event(
                        "db.execute_query.done",
                        namespace,
                        sql,
                        vars.as_ref(),
                        Some(timer.elapsed()),
                        None,
                    ),
                    LogLevel::Debug,
                );
                Ok(value)
            }
            Err(err) => {
                let error_message = err.to_string();
                self.logger.log(
                    build_db_execute_event(
                        "db.execute_query.error",
                        namespace,
                        sql,
                        vars.as_ref(),
                        Some(timer.elapsed()),
                        Some(&error_message),
                    ),
                    LogLevel::Debug,
                );
                Err(err)
            }
        }
    }

    /// Execute a query that doesn't return a value (internal helper).
    async fn execute_raw_query(
        &self,
        sql: &str,
        vars: Option<Value>,
        namespace: &str,
    ) -> Result<(), MemoryError> {
        let timer = Instant::now();
        self.logger.log(
            build_db_execute_event(
                "db.execute_raw_query.start",
                namespace,
                sql,
                vars.as_ref(),
                None,
                None,
            ),
            LogLevel::Debug,
        );

        let vars_for_retry = vars.clone();
        let result = with_db_retry("execute_raw_query", &self.logger, || {
            self.execute_sql_void_with_timing(sql, vars_for_retry.clone(), namespace)
        })
        .await;

        match result {
            Ok(()) => {
                self.logger.log(
                    build_db_execute_event(
                        "db.execute_raw_query.done",
                        namespace,
                        sql,
                        vars.as_ref(),
                        Some(timer.elapsed()),
                        None,
                    ),
                    LogLevel::Debug,
                );
                Ok(())
            }
            Err(err) => {
                let error_message = err.to_string();
                self.logger.log(
                    build_db_execute_event(
                        "db.execute_raw_query.error",
                        namespace,
                        sql,
                        vars.as_ref(),
                        Some(timer.elapsed()),
                        Some(&error_message),
                    ),
                    LogLevel::Debug,
                );
                Err(err)
            }
        }
    }

    /// Shared SQL execution: runs a query and extracts the first SurrealValue result.
    async fn execute_sql_with_timing(
        &self,
        sql: &str,
        vars: Option<Value>,
        namespace: &str,
    ) -> Result<SurrealValue, MemoryError> {
        sql_query_take(self, namespace, sql, vars).await
    }

    /// Shared SQL execution: runs a query and discards the result.
    async fn execute_sql_void_with_timing(
        &self,
        sql: &str,
        vars: Option<Value>,
        namespace: &str,
    ) -> Result<(), MemoryError> {
        sql_query_take(self, namespace, sql, vars).await?;
        Ok(())
    }
}

/// Runs a SurrealDB query with optional variable bindings on the correct engine
/// and extracts the first result value.
async fn sql_query_take(
    client: &SurrealDbClient,
    namespace: &str,
    sql: &str,
    vars: Option<Value>,
) -> Result<SurrealValue, MemoryError> {
    if client.is_local() {
        let db = client.with_namespace_local(namespace).await?;
        run_query_take(&*db, sql, vars).await
    } else {
        let db = client.with_namespace_remote(namespace).await?;
        run_query_take(&*db, sql, vars).await
    }
}

/// Runs a query on any connection and extracts the first result.
async fn run_query_take(
    db: &surrealdb::Surreal<impl surrealdb::Connection>,
    sql: &str,
    vars: Option<Value>,
) -> Result<SurrealValue, MemoryError> {
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

/// Default maximum attempts for database retry on transient errors.
const DEFAULT_DB_RETRY_ATTEMPTS: u32 = 3;
/// Initial delay in milliseconds for database retry backoff (doubles each attempt).
const DEFAULT_DB_RETRY_INITIAL_DELAY_MS: u64 = 200;
/// Per-query timeout to guard against stalled database connections (e.g. WebSocket hang).
const DEFAULT_DB_QUERY_TIMEOUT_SECS: u64 = 30;

/// Runs a fallible database operation with exponential backoff retry and a per-query timeout.
///
/// Each individual attempt is guarded by `DEFAULT_DB_QUERY_TIMEOUT_SECS` to prevent
/// hanging indefinitely on stalled connections (e.g. WebSocket to SurrealDB).
/// Only retries on errors identified as transient by `is_transient_db_error`.
/// Logs each retry attempt via the provided logger with the operation name.
async fn with_db_retry<T, F, Fut>(
    op_name: &str,
    logger: &StdoutLogger,
    f: F,
) -> Result<T, MemoryError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, MemoryError>>,
{
    let mut attempt = 0u32;
    let timeout = Duration::from_secs(DEFAULT_DB_QUERY_TIMEOUT_SECS);
    loop {
        match tokio::time::timeout(timeout, f()).await {
            Ok(Ok(value)) => return Ok(value),
            Ok(Err(err)) => {
                attempt += 1;
                if attempt >= DEFAULT_DB_RETRY_ATTEMPTS
                    || !crate::service::is_transient_db_error(&err)
                {
                    return Err(err);
                }
                let delay_ms =
                    DEFAULT_DB_RETRY_INITIAL_DELAY_MS << attempt.saturating_sub(1).min(6);
                let delay = Duration::from_millis(delay_ms);
                logger.log(
                    std::collections::HashMap::from([
                        (
                            "op".to_string(),
                            serde_json::Value::String(format!("db.{op_name}.retry")),
                        ),
                        (
                            "attempt".to_string(),
                            serde_json::Value::Number(serde_json::Number::from(attempt)),
                        ),
                        (
                            "delay_ms".to_string(),
                            serde_json::Value::Number(serde_json::Number::from(delay_ms)),
                        ),
                        (
                            "max_attempts".to_string(),
                            serde_json::Value::Number(serde_json::Number::from(
                                DEFAULT_DB_RETRY_ATTEMPTS,
                            )),
                        ),
                        (
                            "error".to_string(),
                            serde_json::Value::String(err.to_string()),
                        ),
                    ]),
                    LogLevel::Warn,
                );
                tokio::time::sleep(delay).await;
            }
            Err(_elapsed) => {
                // Timeout elapsed — treat as a transient error for retry purposes
                attempt += 1;
                if attempt >= DEFAULT_DB_RETRY_ATTEMPTS {
                    return Err(MemoryError::Storage(format!(
                        "db.{op_name}: timed out after {DEFAULT_DB_QUERY_TIMEOUT_SECS}s ({DEFAULT_DB_RETRY_ATTEMPTS} attempts)"
                    )));
                }
                let delay_ms =
                    DEFAULT_DB_RETRY_INITIAL_DELAY_MS << attempt.saturating_sub(1).min(6);
                let delay = Duration::from_millis(delay_ms);
                logger.log(
                    std::collections::HashMap::from([
                        (
                            "op".to_string(),
                            serde_json::Value::String(format!("db.{op_name}.timeout")),
                        ),
                        (
                            "attempt".to_string(),
                            serde_json::Value::Number(serde_json::Number::from(attempt)),
                        ),
                        (
                            "delay_ms".to_string(),
                            serde_json::Value::Number(serde_json::Number::from(delay_ms)),
                        ),
                        (
                            "max_attempts".to_string(),
                            serde_json::Value::Number(serde_json::Number::from(
                                DEFAULT_DB_RETRY_ATTEMPTS,
                            )),
                        ),
                    ]),
                    LogLevel::Warn,
                );
                tokio::time::sleep(delay).await;
            }
        }
    }
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn bind_var_count(vars: Option<&Value>) -> usize {
    match vars {
        Some(Value::Object(map)) => map.len(),
        Some(Value::Array(values)) => values.len(),
        Some(Value::Null) | None => 0,
        Some(_) => 1,
    }
}

fn build_db_execute_event(
    op: &str,
    namespace: &str,
    sql: &str,
    vars: Option<&Value>,
    duration: Option<Duration>,
    error: Option<&str>,
) -> HashMap<String, Value> {
    let mut event = HashMap::from([
        ("op".to_string(), json!(op)),
        ("namespace".to_string(), json!(namespace)),
        ("sql".to_string(), json!(sql)),
        ("vars_count".to_string(), json!(bind_var_count(vars))),
    ]);
    if let Some(duration) = duration {
        event.insert("duration_ms".to_string(), json!(duration_ms(duration)));
    }
    if let Some(error) = error {
        event.insert("error".to_string(), json!(error));
    }
    event
}

async fn build_namespace_clients<T: surrealdb::Connection + Clone>(
    base: &Surreal<T>,
    namespaces: &[String],
    database: &str,
) -> Result<HashMap<String, Arc<Surreal<T>>>, MemoryError> {
    let mut clients = HashMap::with_capacity(namespaces.len());

    for namespace in namespaces {
        let client = base.clone();
        client
            .use_ns(namespace)
            .use_db(database)
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB use_failed: {err}")))?;
        clients.insert(namespace.clone(), Arc::new(client));
    }

    Ok(clients)
}

fn render_sql_template(template: &str, embedding_dimension: usize) -> String {
    template.replace(
        crate::storage::fact_embedding_dimension_placeholder(),
        &embedding_dimension.to_string(),
    )
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
        validate_table_name(table)?;
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

    #[allow(clippy::too_many_arguments)]
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

        let surreal_val = self.execute_query(sql, vars, namespace).await?;
        let normalized = surreal_to_json(surreal_val);
        let results = extract_records(normalized);

        self.log_op(
            "db.query.result",
            vec![(
                "count",
                Value::Number(serde_json::Number::from(results.len())),
            )],
        );

        Ok(Value::Array(results))
    }

    async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError> {
        self.apply_migrations_impl(namespace).await
    }
}

fn validate_table_name(table: &str) -> Result<(), MemoryError> {
    const ALLOWED_TABLES: &[&str] = &[
        "community",
        "edge",
        "entity",
        "episode",
        "event_log",
        "fact",
        "query_log",
        "script_migration",
        "task",
    ];

    if ALLOWED_TABLES.contains(&table) {
        Ok(())
    } else {
        Err(MemoryError::ConfigInvalid(format!(
            "table `{table}` is not an allowed query target"
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;
    use crate::logging::StdoutLogger;

    #[tokio::test]
    async fn with_db_retry_succeeds_on_first_attempt() {
        let logger = StdoutLogger::new("warn");
        let result = with_db_retry("test_op", &logger, || async { Ok::<_, MemoryError>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn with_db_retry_retries_on_transient_then_succeeds() {
        let logger = StdoutLogger::new("warn");
        let call_count = Arc::new(AtomicU32::new(0));
        let count = call_count.clone();

        let result = with_db_retry("test_op", &logger, || {
            let count = count.clone();
            async move {
                let n = count.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err(MemoryError::Storage(
                        "Transaction conflict: Resource busy".into(),
                    ))
                } else {
                    Ok::<_, MemoryError>(99)
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), 99);
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn with_db_retry_fails_after_max_attempts() {
        let logger = StdoutLogger::new("warn");
        let call_count = Arc::new(AtomicU32::new(0));
        let count = call_count.clone();

        let result: Result<i32, MemoryError> = with_db_retry("test_op", &logger, || {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Err(MemoryError::Storage(
                    "Transaction conflict: Resource busy".into(),
                ))
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Transaction conflict")
        );
    }

    #[tokio::test]
    async fn with_db_retry_does_not_retry_non_transient() {
        let logger = StdoutLogger::new("warn");
        let call_count = Arc::new(AtomicU32::new(0));
        let count = call_count.clone();

        let result: Result<i32, MemoryError> = with_db_retry("test_op", &logger, || {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Err(MemoryError::Storage("connection refused".into()))
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn with_db_retry_exhaustion_keeps_last_error() {
        let logger = StdoutLogger::new("warn");
        let result: Result<i32, MemoryError> = with_db_retry("test_op", &logger, || async {
            Err(MemoryError::Storage(
                "Transaction conflict: Resource busy".into(),
            ))
        })
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, MemoryError::Storage(_)));
        assert!(err.to_string().contains("Resource busy"));
    }
}
