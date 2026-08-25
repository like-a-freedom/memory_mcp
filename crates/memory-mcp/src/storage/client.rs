//! SurrealDB client implementation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
use super::queries::{build_create_query, build_select_one_query, build_update_query};

/// Low-level database operations used by startup, migrations, and test fixtures.
///
/// The namespace argument is intentionally retained at this infrastructure
/// boundary while ordinary production code uses [`BoundDbClient`] and narrow
/// namespace-bound stores. It is not a request-level routing API and must not be
/// threaded into MCP, CLI, lifecycle, or domain method contracts.
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
    pub cutoff: &'a str,
    pub query_contains: Option<&'a str>,
    pub limit: i32,
    pub fact_types: &'a [String],
}

/// Database adapter bound to the Active Namespace selected at startup.
///
/// The low-level `DbClient` remains namespace-parameterized for startup,
/// migrations, adapter compatibility, and explicit test fixtures. This adapter
/// is the production seam for namespace-free stores: callers can perform
/// operations, but cannot choose another namespace per call.
#[derive(Clone)]
pub(crate) struct BoundDbClient {
    db: Arc<dyn DbClient>,
    namespace: String,
}

impl BoundDbClient {
    pub(crate) fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            db,
            namespace: namespace.into(),
        }
    }

    pub(crate) async fn select_one(&self, record_id: &str) -> Result<Option<Value>, MemoryError> {
        self.db.select_one(record_id, &self.namespace).await
    }

    pub(crate) async fn query(&self, sql: &str, vars: Option<Value>) -> Result<Value, MemoryError> {
        self.db.query(sql, vars, &self.namespace).await
    }

    /// Runs a query and returns its result rows.
    ///
    /// Concentrates the store-wide recipe: a missing table degrades to an
    /// empty result set (stores are created lazily by migrations), while any
    /// other storage error propagates.
    pub(crate) async fn query_rows(
        &self,
        sql: &str,
        vars: Option<Value>,
    ) -> Result<Vec<Value>, MemoryError> {
        match self.db.query(sql, vars, &self.namespace).await {
            Ok(value) => Ok(value.as_array().cloned().unwrap_or_default()),
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => Ok(vec![]),
            Err(err) => Err(err),
        }
    }

    /// Runs a query and returns its first result row, if any.
    ///
    /// Same missing-table degradation as [`Self::query_rows`].
    pub(crate) async fn query_first(
        &self,
        sql: &str,
        vars: Option<Value>,
    ) -> Result<Option<Value>, MemoryError> {
        let rows = self.query_rows(sql, vars).await?;
        Ok(rows.into_iter().next())
    }

    pub(crate) async fn select_table(&self, table: &str) -> Result<Vec<Value>, MemoryError> {
        self.db.select_table(table, &self.namespace).await
    }

    pub(crate) async fn create(
        &self,
        record_id: &str,
        content: Value,
    ) -> Result<Value, MemoryError> {
        self.db.create(record_id, content, &self.namespace).await
    }

    pub(crate) async fn update(
        &self,
        record_id: &str,
        content: Value,
    ) -> Result<Value, MemoryError> {
        self.db.update(record_id, content, &self.namespace).await
    }

    pub(crate) fn namespace(&self) -> &str {
        &self.namespace
    }

    pub(crate) async fn apply_migrations(&self) -> Result<(), MemoryError> {
        self.db.apply_migrations(&self.namespace).await
    }
}

/// Unified database client that works with both embedded and remote SurrealDB.
pub struct SurrealDbClient {
    engine: DbEngine,
    active_namespace: String,
    logger: StdoutLogger,
    fact_embedding_dimension: usize,
}

enum DbEngine {
    Local(Arc<Surreal<Db>>),
    Remote(Arc<Surreal<Client>>),
}

impl SurrealDbClient {
    /// Connects to an embedded in-memory SurrealDB instance.
    ///
    /// This is primarily intended for tests that should exercise the real
    /// SurrealDB query engine without touching the filesystem.
    pub async fn connect_in_memory(
        database: &str,
        active_namespace: &str,
        log_level: &str,
    ) -> Result<Self, MemoryError> {
        let db = Surreal::new::<Mem>(())
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB memory init failed: {err}")))?;
        db.use_ns(active_namespace)
            .use_db(database)
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB use_failed: {err}")))?;

        Ok(Self {
            engine: DbEngine::Local(Arc::new(db)),
            active_namespace: active_namespace.to_string(),
            logger: StdoutLogger::new(log_level),
            fact_embedding_dimension: crate::config::DEFAULT_EMBEDDING_DIMENSION,
        })
    }

    /// Transitional constructor retained for test and adapter migration.
    ///
    /// The production client has one bound storage session. More than one
    /// namespace is rejected rather than silently routing through the first one.
    pub async fn connect_in_memory_with_namespaces(
        database: &str,
        namespaces: &[String],
        log_level: &str,
    ) -> Result<Self, MemoryError> {
        let [active_namespace] = namespaces else {
            return Err(MemoryError::ConfigInvalid(
                "one active namespace is required for an in-memory client".to_string(),
            ));
        };
        Self::connect_in_memory(database, active_namespace, log_level).await
    }

    /// Connects to SurrealDB using the provided configuration.
    pub async fn connect(config: &SurrealConfig) -> Result<Self, MemoryError> {
        let active_namespace = config.active_namespace().as_str().to_string();
        let engine = match StorageBackend::from_embedded(config.embedded) {
            StorageBackend::Embedded => Self::connect_embedded(config).await?,
            StorageBackend::Remote => Self::connect_remote(config).await?,
        };

        Ok(Self {
            engine,
            active_namespace,
            logger: StdoutLogger::new(&config.log_level),
            fact_embedding_dimension: config.embedding.fallback_dimension(),
        })
    }

    /// Maps an embedded RocksDB initialization error into an actionable startup
    /// error. RocksDB remains the final ownership arbiter; only known lock /
    /// resource-busy signatures are translated.
    fn map_embedded_init_error(data_dir: &Path, message: &str) -> MemoryError {
        let lowered = message.to_ascii_lowercase();
        let is_lock_error = lowered.contains("resource busy")
            || lowered.contains("lock")
            || lowered.contains("would block")
            || lowered.contains("permission denied");
        if is_lock_error {
            MemoryError::ConfigInvalid(format!(
                "embedded data directory `{}` is locked by another Memory MCP process: {message}. \
                 Each stdio client needs a unique SURREALDB_DATA_DIR (changing only the database \
                 name or namespace does not avoid the directory lock), or use a remote SurrealDB.",
                data_dir.display()
            ))
        } else {
            MemoryError::Storage(format!("SurrealDB embedded init failed: {message}"))
        }
    }

    /// Connects to embedded RocksDB instance.
    async fn connect_embedded(config: &SurrealConfig) -> Result<DbEngine, MemoryError> {
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

        let db = Surreal::new::<RocksDb>((data_dir.clone(), cfg))
            .await
            .map_err(|err| Self::map_embedded_init_error(&data_dir, &err.to_string()))?;

        db.signin(root)
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB signin failed: {err}")))?;

        db.use_ns(config.active_namespace().as_str())
            .use_db(&config.db_name)
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB use_failed: {err}")))?;

        Ok(DbEngine::Local(Arc::new(db)))
    }

    /// Connects to remote WebSocket instance.
    async fn connect_remote(config: &SurrealConfig) -> Result<DbEngine, MemoryError> {
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

        db.use_ns(config.active_namespace().as_str())
            .use_db(&config.db_name)
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB use_failed: {err}")))?;

        Ok(DbEngine::Remote(Arc::new(db)))
    }

    fn ensure_active_namespace(&self, namespace: &str) -> Result<(), MemoryError> {
        if namespace == self.active_namespace {
            Ok(())
        } else {
            Err(MemoryError::ConfigInvalid(format!(
                "namespace `{namespace}` is not active; this process is bound to `{}`",
                self.active_namespace
            )))
        }
    }

    fn local_db(&self) -> Result<Arc<Surreal<Db>>, MemoryError> {
        match &self.engine {
            DbEngine::Local(db) => Ok(db.clone()),
            DbEngine::Remote(_) => Err(MemoryError::Storage("expected local engine".into())),
        }
    }

    fn remote_db(&self) -> Result<Arc<Surreal<Client>>, MemoryError> {
        match &self.engine {
            DbEngine::Remote(db) => Ok(db.clone()),
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
        self.ensure_active_namespace(namespace)?;
        let sql = "INFO FOR DB";
        let res = if self.is_local() {
            self.local_db()?.query(sql).await
        } else {
            self.remote_db()?.query(sql).await
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
    ///
    /// The migration runtime lives in [`super::migrations`] (C5); this method
    /// keeps the namespace gate and delegates the run.
    pub async fn apply_migrations_impl(&self, namespace: &str) -> Result<(), MemoryError> {
        self.ensure_active_namespace(namespace)?;
        super::migrations::run_migrations(self, namespace).await
    }

    /// Executes a migration script that returns no result (C5 seam: the
    /// migration runtime in `migrations.rs` is the only consumer).
    pub(crate) async fn execute_migration_script(
        &self,
        sql: &str,
        namespace: &str,
    ) -> Result<(), MemoryError> {
        self.execute_raw_query(sql, None, namespace).await
    }

    /// Embedding dimension used to render migration SQL templates (C5 seam).
    pub(crate) fn migration_embedding_dimension(&self) -> usize {
        self.fact_embedding_dimension
    }

    /// Logger for migration events (C5 seam).
    pub(crate) fn migration_logger(&self) -> &StdoutLogger {
        &self.logger
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
    client.ensure_active_namespace(namespace)?;
    if client.is_local() {
        let db = client.local_db()?;
        run_query_take(&*db, sql, vars).await
    } else {
        let db = client.remote_db()?;
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

    let mut statement_errors = response.take_errors().into_iter().collect::<Vec<_>>();
    if !statement_errors.is_empty() {
        statement_errors.sort_by_key(|(index, _)| *index);
        let details = statement_errors
            .into_iter()
            .map(|(index, error)| format!("statement {index}: {error}"))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(MemoryError::Storage(format!(
            "SurrealDB query statement errors:\n{details}"
        )));
    }

    response
        .take::<SurrealValue>(0)
        .map_err(|err| MemoryError::Storage(format!("SurrealDB take failed: {err}")))
}

pub(crate) fn is_record_already_exists_error(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    lowered.contains("already exists")
        && (lowered.contains("record") || lowered.contains("script_migration"))
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
        "inbox_revision",
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
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::logging::StdoutLogger;

    #[test]
    fn embedded_init_error_translates_lock_signatures_actionably() {
        let data_dir = PathBuf::from("/tmp/locked-data-dir");

        for message in [
            "IO error: ... LOCK ... Resource busy",
            "Resource busy: lock held by another process",
            "database would block",
            "LOCK: lock already held",
        ] {
            let error = SurrealDbClient::map_embedded_init_error(&data_dir, message);
            let text = error.to_string();
            assert!(
                matches!(error, MemoryError::ConfigInvalid(_)),
                "lock-like error must be ConfigInvalid: {message}"
            );
            assert!(
                text.contains("/tmp/locked-data-dir"),
                "must mention the data directory: {text}"
            );
            assert!(
                text.contains("SURREALDB_DATA_DIR"),
                "must mention unique SURREALDB_DATA_DIR: {text}"
            );
            assert!(
                text.contains("remote SurrealDB"),
                "must mention remote SurrealDB: {text}"
            );
        }
    }

    #[test]
    fn embedded_init_error_retains_generic_wording_for_unrelated_failures() {
        let data_dir = PathBuf::from("/tmp/data-dir");
        let error =
            SurrealDbClient::map_embedded_init_error(&data_dir, "unsupported rocksdb options");
        assert!(matches!(error, MemoryError::Storage(_)));
        assert!(error.to_string().contains("SurrealDB embedded init failed"));
        assert!(!error.to_string().contains("SURREALDB_DATA_DIR"));
    }

    #[derive(Clone, Default)]
    struct RecordingDbClient {
        namespaces: Arc<Mutex<Vec<String>>>,
    }

    impl RecordingDbClient {
        fn record(&self, namespace: &str) {
            self.namespaces
                .lock()
                .expect("recording mutex should not be poisoned")
                .push(namespace.to_string());
        }

        fn recorded_namespaces(&self) -> Vec<String> {
            self.namespaces
                .lock()
                .expect("recording mutex should not be poisoned")
                .clone()
        }
    }

    #[async_trait]
    impl DbClient for RecordingDbClient {
        async fn select_one(
            &self,
            _record_id: &str,
            namespace: &str,
        ) -> Result<Option<Value>, MemoryError> {
            self.record(namespace);
            Ok(None)
        }

        async fn select_table(
            &self,
            _table: &str,
            namespace: &str,
        ) -> Result<Vec<Value>, MemoryError> {
            self.record(namespace);
            Ok(Vec::new())
        }

        async fn create(
            &self,
            _record_id: &str,
            _content: Value,
            namespace: &str,
        ) -> Result<Value, MemoryError> {
            self.record(namespace);
            Ok(Value::Null)
        }

        async fn update(
            &self,
            _record_id: &str,
            _content: Value,
            namespace: &str,
        ) -> Result<Value, MemoryError> {
            self.record(namespace);
            Ok(Value::Null)
        }

        async fn query(
            &self,
            _sql: &str,
            _vars: Option<Value>,
            namespace: &str,
        ) -> Result<Value, MemoryError> {
            self.record(namespace);
            Ok(Value::Null)
        }

        async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError> {
            self.record(namespace);
            Ok(())
        }
    }

    #[tokio::test]
    async fn bound_db_client_routes_every_operation_to_its_startup_namespace() {
        let recorder = Arc::new(RecordingDbClient::default());
        let bound = BoundDbClient::new(recorder.clone(), "main");

        bound
            .select_one("episode:test")
            .await
            .expect("bound select_one should succeed");
        bound
            .select_table("episode")
            .await
            .expect("bound select_table should succeed");
        bound
            .create("episode:test", serde_json::json!({"content": "test"}))
            .await
            .expect("bound create should succeed");
        bound
            .update("episode:test", serde_json::json!({"status": "active"}))
            .await
            .expect("bound update should succeed");
        bound
            .query("SELECT * FROM episode", None)
            .await
            .expect("bound query should succeed");

        assert_eq!(
            recorder.recorded_namespaces(),
            vec!["main", "main", "main", "main", "main"]
        );
    }

    /// Scripted `DbClient` fake for exercising `BoundDbClient` query recipes.
    #[derive(Clone)]
    enum QueryBehavior {
        Rows(Value),
        MissingTable,
        OtherError,
    }

    struct ScriptedQueryClient {
        behavior: QueryBehavior,
    }

    #[async_trait]
    impl DbClient for ScriptedQueryClient {
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
            Ok(Vec::new())
        }

        async fn create(
            &self,
            _record_id: &str,
            _content: Value,
            _namespace: &str,
        ) -> Result<Value, MemoryError> {
            Ok(Value::Null)
        }

        async fn update(
            &self,
            _record_id: &str,
            _content: Value,
            _namespace: &str,
        ) -> Result<Value, MemoryError> {
            Ok(Value::Null)
        }

        async fn query(
            &self,
            _sql: &str,
            _vars: Option<Value>,
            _namespace: &str,
        ) -> Result<Value, MemoryError> {
            match &self.behavior {
                QueryBehavior::Rows(value) => Ok(value.clone()),
                QueryBehavior::MissingTable => Err(MemoryError::Storage(
                    "The table 'fact' does not exist".to_string(),
                )),
                QueryBehavior::OtherError => {
                    Err(MemoryError::Storage("connection lost".to_string()))
                }
            }
        }

        async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    fn scripted_bound_client(behavior: QueryBehavior) -> BoundDbClient {
        BoundDbClient::new(Arc::new(ScriptedQueryClient { behavior }), "main")
    }

    #[tokio::test]
    async fn query_rows_returns_rows_when_query_succeeds() {
        let rows = serde_json::json!([{"id": "fact:1"}, {"id": "fact:2"}]);
        let bound = scripted_bound_client(QueryBehavior::Rows(rows.clone()));

        let result = bound
            .query_rows("SELECT * FROM fact", None)
            .await
            .expect("query_rows should succeed");

        assert_eq!(result, rows.as_array().cloned().unwrap_or_default());
    }

    #[tokio::test]
    async fn query_rows_returns_empty_when_table_missing() {
        let bound = scripted_bound_client(QueryBehavior::MissingTable);

        let result = bound
            .query_rows("SELECT * FROM fact", None)
            .await
            .expect("missing table must degrade to empty rows");

        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn query_rows_propagates_other_errors() {
        let bound = scripted_bound_client(QueryBehavior::OtherError);

        let error = bound
            .query_rows("SELECT * FROM fact", None)
            .await
            .expect_err("non-missing-table errors must propagate");

        assert!(matches!(error, MemoryError::Storage(message) if message == "connection lost"));
    }

    #[tokio::test]
    async fn query_first_returns_first_row_when_query_succeeds() {
        let rows = serde_json::json!([{"id": "fact:1"}, {"id": "fact:2"}]);
        let bound = scripted_bound_client(QueryBehavior::Rows(rows));

        let result = bound
            .query_first("SELECT * FROM fact LIMIT 1", None)
            .await
            .expect("query_first should succeed");

        assert_eq!(result, Some(serde_json::json!({"id": "fact:1"})));
    }

    #[tokio::test]
    async fn query_first_returns_none_for_empty_or_missing_table() {
        let bound = scripted_bound_client(QueryBehavior::Rows(serde_json::json!([])));
        assert_eq!(
            bound
                .query_first("SELECT * FROM fact LIMIT 1", None)
                .await
                .expect("empty rows should succeed"),
            None
        );

        let bound = scripted_bound_client(QueryBehavior::MissingTable);
        assert_eq!(
            bound
                .query_first("SELECT * FROM fact LIMIT 1", None)
                .await
                .expect("missing table must degrade to None"),
            None
        );
    }

    #[tokio::test]
    async fn query_first_propagates_other_errors() {
        let bound = scripted_bound_client(QueryBehavior::OtherError);

        let error = bound
            .query_first("SELECT * FROM fact LIMIT 1", None)
            .await
            .expect_err("non-missing-table errors must propagate");

        assert!(matches!(error, MemoryError::Storage(message) if message == "connection lost"));
    }

    #[tokio::test]
    async fn query_rejects_error_in_later_statement() {
        let client = SurrealDbClient::connect_in_memory("test_db", "test", "warn")
            .await
            .expect("in-memory db");

        assert_eq!(client.active_namespace, "test");

        client
            .query(
                "DEFINE ANALYZER memory_fts TOKENIZERS class FILTERS lowercase;",
                None,
                "test",
            )
            .await
            .expect("pre-seed analyzer");

        let result = client
            .query(
                "RETURN 1; DEFINE ANALYZER memory_fts TOKENIZERS class FILTERS lowercase;",
                None,
                "test",
            )
            .await;

        let error = result.expect_err("later statement error must not be ignored");
        assert!(error.to_string().contains("memory_fts"));
    }

    #[tokio::test]
    async fn multi_namespace_constructor_is_rejected() {
        let result = SurrealDbClient::connect_in_memory_with_namespaces(
            "namespace_binding",
            &["main".to_string(), "org".to_string()],
            "warn",
        )
        .await;

        assert!(matches!(
            result,
            Err(MemoryError::ConfigInvalid(message)) if message.contains("one active namespace")
        ));
    }

    #[tokio::test]
    async fn namespace_mismatch_is_rejected_before_query_execution() {
        let client = SurrealDbClient::connect_in_memory("namespace_binding", "main", "warn")
            .await
            .expect("in-memory db");

        let error = client
            .select_one("episode:missing", "org")
            .await
            .expect_err("a bound client must reject another namespace");

        assert!(
            matches!(error, MemoryError::ConfigInvalid(message) if message.contains("not active"))
        );
    }

    #[tokio::test]
    async fn scope_free_records_are_writable_after_expand_migration() {
        let client = SurrealDbClient::connect_in_memory("scope_free_migration", "main", "warn")
            .await
            .expect("in-memory db");

        client
            .apply_migrations_impl("main")
            .await
            .expect("all migrations, including 032, should apply");

        client
            .create(
                "episode:scope-free",
                serde_json::json!({
                    "episode_id": "episode:scope-free",
                    "source_type": "test",
                    "source_id": "scope-free-source",
                    "content": "scope-free episode",
                    "t_ref": "2026-08-12T00:00:00Z",
                    "t_ingested": "2026-08-12T00:00:00Z",
                    "policy_tags": []
                }),
                "main",
            )
            .await
            .expect("episode without scope or visibility_scope should be accepted");

        client
            .create(
                "fact:scope-free",
                serde_json::json!({
                    "fact_id": "fact:scope-free",
                    "fact_type": "note",
                    "content": "scope-free fact",
                    "quote": "scope-free fact",
                    "source_episode": "episode:scope-free",
                    "t_valid": "2026-08-12T00:00:00Z",
                    "t_ingested": "2026-08-12T00:00:00Z",
                    "confidence": 1.0,
                    "entity_links": [],
                    "policy_tags": [],
                    "provenance": {}
                }),
                "main",
            )
            .await
            .expect("fact without scope should be accepted");

        let episode = client
            .select_one("episode:scope-free", "main")
            .await
            .expect("select episode")
            .expect("episode exists");
        assert!(episode.get("scope").is_none());
        assert!(episode.get("visibility_scope").is_none());

        let fact = client
            .select_one("fact:scope-free", "main")
            .await
            .expect("select fact")
            .expect("fact exists");
        assert!(fact.get("scope").is_none());
    }

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
