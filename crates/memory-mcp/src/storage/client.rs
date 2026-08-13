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
    validate_applied_migration, validate_migration_identity, versioned_migrations,
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

        let db = Surreal::new::<RocksDb>((data_dir, cfg))
            .await
            .map_err(|err| {
                MemoryError::Storage(format!("SurrealDB embedded init failed: {err}"))
            })?;

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
    pub async fn apply_migrations_impl(&self, namespace: &str) -> Result<(), MemoryError> {
        self.ensure_active_namespace(namespace)?;
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
            Err(MemoryError::Storage(err_msg)) if is_tolerable_initial_schema_error(&err_msg) => {
                self.logger.log(
                    HashMap::from([(
                        "op".to_string(),
                        Value::String("schema.init.compatibility_conflicts".to_string()),
                    )]),
                    LogLevel::Debug,
                );
            }
            Err(e) => return Err(e),
        }

        // Migration 036 adds the durable runner fields used by all other
        // versioned migrations. Apply its idempotent DDL first; its ledger row
        // is still recorded in the normal ordered loop below.
        self.ensure_migration_runner_schema(namespace).await?;

        for migration in versioned_migrations() {
            self.apply_versioned_migration(namespace, migration).await?;
        }

        self.verify_schema_postconditions(namespace).await?;

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

    async fn ensure_migration_runner_schema(&self, namespace: &str) -> Result<(), MemoryError> {
        let migration = versioned_migrations()
            .iter()
            .find(|migration| migration.file_name == MIGRATION_RUNNER_STATE_FILE)
            .ok_or_else(|| {
                MemoryError::Storage(format!(
                    "migration runner bootstrap `{MIGRATION_RUNNER_STATE_FILE}` is not registered"
                ))
            })?;
        let rendered_sql = render_sql_template(migration.sql, self.fact_embedding_dimension);
        self.execute_raw_query(&rendered_sql, None, namespace).await
    }

    async fn verify_schema_postconditions(&self, namespace: &str) -> Result<(), MemoryError> {
        let db_info = self.query("INFO FOR DB", None, namespace).await?;
        let db_info = first_info_object(&db_info, "database")?;
        let tables = info_names(db_info.get("tables"), "tables")?;
        let analyzers = info_names(db_info.get("analyzers"), "analyzers")?;

        for table in EXPECTED_SCHEMA_TABLES {
            if !tables.contains(*table) {
                return Err(MemoryError::Storage(format!(
                    "schema readiness failed in namespace `{namespace}`: missing table `{table}`"
                )));
            }
        }
        for analyzer in EXPECTED_SCHEMA_ANALYZERS {
            if !analyzers.contains(*analyzer) {
                return Err(MemoryError::Storage(format!(
                    "schema readiness failed in namespace `{namespace}`: missing analyzer `{analyzer}`"
                )));
            }
        }

        for table in EXPECTED_SCHEMA_TABLES {
            let table_info = self
                .query(&format!("INFO FOR TABLE {table}"), None, namespace)
                .await?;
            let table_info = first_info_object(&table_info, table)?;
            let fields = info_names(table_info.get("fields"), "fields")?;
            for field in required_schema_fields(table) {
                if !fields.contains(*field) {
                    return Err(MemoryError::Storage(format!(
                        "schema readiness failed in namespace `{namespace}`: missing field `{field}` on table `{table}`"
                    )));
                }
            }
            let indexes = info_names(table_info.get("indexes"), "indexes")?;
            for index in required_schema_indexes(table) {
                if !indexes.contains(*index) {
                    return Err(MemoryError::Storage(format!(
                        "schema readiness failed in namespace `{namespace}`: missing index `{index}` on table `{table}`"
                    )));
                }
            }
        }

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

        // The runner-state DDL was bootstrapped before this loop. It is safe to
        // execute repeatedly, and its ledger record is created through the same
        // lease path as every other migration.
        let owner = migration_owner();
        let deadline = Instant::now() + Duration::from_secs(MIGRATION_WAIT_SECS);
        loop {
            if let Some(existing) = self.select_one(&record_id, namespace).await? {
                validate_applied_migration_compatibility(
                    &existing,
                    migration.file_name,
                    &checksum,
                )?;
                match migration_status(&existing) {
                    Some("applied") | None => return Ok(()),
                    Some("applying") if migration_lease_is_active(&existing) => {
                        if Instant::now() >= deadline {
                            return Err(MemoryError::Storage(format!(
                                "migration `{}` is already being applied in namespace `{namespace}`; waited {}s",
                                migration.file_name, MIGRATION_WAIT_SECS
                            )));
                        }
                        tokio::time::sleep(Duration::from_millis(MIGRATION_POLL_INTERVAL_MS)).await;
                        continue;
                    }
                    Some("failed") | Some("applying") => {
                        if !self
                            .claim_existing_migration(&record_id, &owner, namespace)
                            .await?
                        {
                            continue;
                        }
                    }
                    Some(status) => {
                        return Err(MemoryError::Storage(format!(
                            "migration `{}` has unsupported ledger status `{status}`",
                            migration.file_name
                        )));
                    }
                }
            } else if self
                .create_migration_lease(
                    &record_id,
                    migration.file_name,
                    &checksum,
                    &owner,
                    namespace,
                )
                .await?
            {
                break;
            }

            if Instant::now() >= deadline {
                return Err(MemoryError::Storage(format!(
                    "could not reserve migration `{}` in namespace `{namespace}`; waited {}s",
                    migration.file_name, MIGRATION_WAIT_SECS
                )));
            }
            tokio::time::sleep(Duration::from_millis(MIGRATION_POLL_INTERVAL_MS)).await;
        }

        let execution = if migration_has_statements(&rendered_sql) {
            self.execute_raw_query(&rendered_sql, None, namespace).await
        } else {
            Ok(())
        };
        if let Err(error) = execution {
            let _ = self
                .mark_migration_failed(&record_id, &error.to_string(), namespace)
                .await;
            return Err(error);
        }

        self.mark_migration_applied(&record_id, migration.file_name, &checksum, namespace)
            .await
    }

    async fn create_migration_lease(
        &self,
        record_id: &str,
        file_name: &str,
        checksum: &str,
        owner: &str,
        namespace: &str,
    ) -> Result<bool, MemoryError> {
        let body = migration_record_body(record_id)?;
        let now = chrono::Utc::now();
        let sql = format!(
            "CREATE script_migration:⟨{body}⟩ SET script_name = $script_name, checksum = $checksum, status = 'applying', owner = $owner, lease_expires_at = type::datetime($lease_expires_at), started_at = type::datetime($started_at), executed_at = type::datetime($executed_at) RETURN *"
        );
        match self
            .query(
                &sql,
                Some(json!({
                    "script_name": file_name,
                    "checksum": checksum,
                    "owner": owner,
                    "lease_expires_at": (now + chrono::Duration::seconds(MIGRATION_LEASE_SECS)).to_rfc3339(),
                    "started_at": now.to_rfc3339(),
                    "executed_at": now.to_rfc3339(),
                })),
                namespace,
            )
            .await
        {
            Ok(_) => Ok(true),
            Err(MemoryError::Storage(message)) if is_record_already_exists_error(&message) => {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    async fn claim_existing_migration(
        &self,
        record_id: &str,
        owner: &str,
        namespace: &str,
    ) -> Result<bool, MemoryError> {
        let Some(body) = record_id.strip_prefix("script_migration:") else {
            return Err(MemoryError::Storage(format!(
                "invalid migration ledger id `{record_id}`"
            )));
        };
        let sql = format!(
            "UPDATE script_migration:⟨{body}⟩ SET status = 'applying', owner = $owner, lease_expires_at = type::datetime($lease_expires_at), started_at = type::datetime($started_at), last_error = NONE WHERE status != 'applying' OR lease_expires_at IS NONE OR lease_expires_at <= time::now() RETURN AFTER"
        );
        let now = chrono::Utc::now();
        let result = self
            .query(
                &sql,
                Some(json!({
                    "owner": owner,
                    "lease_expires_at": (now + chrono::Duration::seconds(MIGRATION_LEASE_SECS)).to_rfc3339(),
                    "started_at": now.to_rfc3339(),
                })),
                namespace,
            )
            .await?;
        Ok(!result.as_array().is_none_or(Vec::is_empty))
    }

    async fn mark_migration_failed(
        &self,
        record_id: &str,
        error: &str,
        namespace: &str,
    ) -> Result<(), MemoryError> {
        self.update(
            record_id,
            json!({
                "status": "failed",
                "owner": Value::Null,
                "lease_expires_at": Value::Null,
                "last_error": error,
            }),
            namespace,
        )
        .await
        .map(|_| ())
    }

    async fn mark_migration_applied(
        &self,
        record_id: &str,
        file_name: &str,
        checksum: &str,
        namespace: &str,
    ) -> Result<(), MemoryError> {
        let body = migration_record_body(record_id)?;
        let sql = format!(
            "UPDATE script_migration:⟨{body}⟩ SET script_name = $script_name, checksum = $checksum, status = 'applied', owner = NONE, lease_expires_at = NONE, last_error = NONE, executed_at = type::datetime($executed_at) RETURN *"
        );
        self.query(
            &sql,
            Some(json!({
                "script_name": file_name,
                "checksum": checksum,
                "executed_at": chrono::Utc::now().to_rfc3339(),
            })),
            namespace,
        )
        .await
        .map(|_| ())
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

const INITIAL_SCHEMA_TABLES: &[&str] = &[
    "episode",
    "entity",
    "fact",
    "edge",
    "community",
    "event_log",
    "task",
    "script_migration",
];

const INITIAL_SCHEMA_FIELDS: &[&str] = &[
    "episode_id",
    "source_type",
    "source_id",
    "content",
    "t_ref",
    "t_ingested",
    "status",
    "archived_at",
    "scope",
    "visibility_scope",
    "policy_tags",
    "entity_id",
    "entity_type",
    "canonical_name",
    "canonical_name_normalized",
    "aliases",
    "fact_id",
    "fact_type",
    "quote",
    "source_episode",
    "t_valid",
    "t_invalid",
    "t_invalid_ingested",
    "confidence",
    "entity_links",
    "embedding",
    "provenance",
    "edge_id",
    "in",
    "relation",
    "out",
    "strength",
    "community_id",
    "member_entities",
    "summary",
    "updated_at",
    "ts",
    "op",
    "args",
    "result",
    "access",
    "transport",
    "content_type",
    "session_vars",
    "title",
    "due_date",
    "script_name",
    "executed_at",
    "checksum",
];

const INITIAL_SCHEMA_ANALYZERS: &[&str] = &["memory_fts"];

const INITIAL_SCHEMA_INDEXES: &[&str] = &[
    "episode_source_id",
    "entity_canonical_name",
    "entity_canonical_name_normalized",
    "entity_aliases",
    "fact_content_search",
    "fact_embedding_hnsw",
    "community_summary_search",
    "edge_relation",
    "edge_in",
    "edge_out",
    "community_members",
];

const INITIAL_SCHEMA_FLEXIBLE_COMPATIBILITY_ERROR: &str =
    "An error occurred: FLEXIBLE can only be used in SCHEMAFULL tables";
const MIGRATION_RUNNER_STATE_FILE: &str = "036_migration_runner_state.surql";
const MIGRATION_LEASE_SECS: i64 = 30;
const MIGRATION_WAIT_SECS: u64 = 5;
const MIGRATION_POLL_INTERVAL_MS: u64 = 100;

const EXPECTED_SCHEMA_TABLES: &[&str] = &[
    "episode",
    "entity",
    "fact",
    "edge",
    "community",
    "event_log",
    "task",
    "script_migration",
    "query_log",
    "embedding_state",
    "embedding_job",
    "triple",
    "memory_event",
    "event_projection_job",
    "memory_capture_audit",
    "procedure_candidate",
    "claim",
    "claim_relation",
    "claim_job",
    "claim_key_alias",
    "claim_policy",
    "entity_extraction_projection",
];

const EXPECTED_SCHEMA_ANALYZERS: &[&str] = &["memory_fts", "memory_fts_ru"];

fn required_schema_fields(table: &str) -> &'static [&'static str] {
    match table {
        "episode" => &[
            "episode_id",
            "source_type",
            "source_id",
            "content",
            "t_ref",
            "t_ingested",
            "policy_tags",
        ],
        "entity" => &[
            "entity_id",
            "entity_type",
            "canonical_name",
            "canonical_name_normalized",
            "aliases",
        ],
        "fact" => &[
            "fact_id",
            "fact_type",
            "content",
            "quote",
            "source_episode",
            "t_valid",
            "t_ingested",
            "confidence",
            "entity_links",
            "policy_tags",
            "provenance",
            "index_keys",
            "access_count",
            "last_accessed",
        ],
        "edge" => &[
            "edge_id",
            "in",
            "relation",
            "out",
            "strength",
            "confidence",
            "provenance",
            "t_valid",
            "t_ingested",
            "origin",
        ],
        "community" => &["community_id", "member_entities", "summary", "updated_at"],
        "event_log" => &[
            "ts",
            "op",
            "args",
            "result",
            "access",
            "transport",
            "content_type",
            "session_vars",
        ],
        "task" => &["status", "title", "due_date"],
        "script_migration" => &[
            "script_name",
            "executed_at",
            "checksum",
            "status",
            "owner",
            "lease_expires_at",
            "started_at",
            "last_error",
        ],
        "query_log" => &[
            "query_log_id",
            "logged_at",
            "query",
            "view_mode",
            "result_count",
            "latency_ms",
            "cache_hit",
        ],
        "embedding_state" => &["status", "updated_at"],
        "embedding_job" => &[
            "job_id",
            "status",
            "target_signature",
            "provider",
            "dimension",
            "namespaces",
            "requested_at",
            "total_facts",
            "processed_facts",
            "succeeded_facts",
            "failed_facts",
            "namespace_progress",
        ],
        "triple" => &[
            "namespace",
            "subject",
            "predicate",
            "object",
            "confidence",
            "source_fact_id",
            "t_ingested",
        ],
        "memory_event" => &[
            "event_id",
            "event_kind",
            "task_fingerprint",
            "disposition",
            "trust_class",
            "origin_kind",
            "created_at",
        ],
        "event_projection_job" => &[
            "job_id",
            "event_id",
            "status",
            "attempts",
            "max_attempts",
            "origin_kind",
            "created_at",
        ],
        "memory_capture_audit" => &[
            "audit_id",
            "event_id",
            "content_hash",
            "content_byte_len",
            "disposition",
            "reason_codes",
            "created_at",
        ],
        "procedure_candidate" => &[
            "candidate_id",
            "namespace",
            "task_fingerprint",
            "normalized_task",
            "status",
            "trust_floor",
            "success_count",
            "failure_count",
            "evidence_count",
            "origin_kind",
            "created_at",
            "updated_at",
        ],
        "claim" => &[
            "claim_id",
            "namespace",
            "source_fact_id",
            "source_episode_id",
            "policy_tags",
            "access_policy_fingerprint",
            "schema_family",
            "schema_version",
            "subject",
            "subject_key",
            "comparison_key",
            "comparison_key_hash",
            "qualifiers",
            "qualifier_hash",
            "slot_fingerprint",
            "value",
            "cardinality",
            "observed_at",
            "validity_source",
            "derivation",
            "extractor_fingerprint",
            "t_ingested",
            "identity_version",
        ],
        "claim_relation" => &[
            "claim_relation_id",
            "left_claim_id",
            "right_claim_id",
            "pair_fingerprint",
            "outcome",
            "schema_family",
            "schema_version",
            "left_fact_id",
            "right_fact_id",
            "reason_code",
            "evidence",
            "evaluator_version",
            "context_fingerprint",
            "evaluated_at",
            "policy_tags",
            "t_ingested",
        ],
        "claim_job" => &[
            "job_id",
            "kind",
            "namespace",
            "extractor_fingerprint",
            "status",
            "cursor",
            "lease_owner",
            "lease_expires_at",
            "processed",
            "succeeded",
            "skipped",
            "failed",
            "retry_count",
            "created_at",
            "updated_at",
        ],
        "claim_key_alias" => &[
            "alias_id",
            "schema_family",
            "canonical_key_hash",
            "alias_key_hash",
            "registry_version",
            "confirmed_by",
            "t_ingested",
        ],
        "claim_policy" => &[
            "policy_id",
            "schema_family",
            "schema_version",
            "policy_fingerprint",
            "definition",
            "t_ingested",
        ],
        "entity_extraction_projection" => &[
            "episode_id",
            "scope",
            "t_ingested",
            "t_created",
            "fingerprint",
            "entity_ids",
        ],
        _ => &[],
    }
}

fn required_schema_indexes(table: &str) -> &'static [&'static str] {
    match table {
        "episode" => &["episode_source_id", "episode_project"],
        "entity" => &[
            "entity_canonical_name",
            "entity_canonical_name_normalized",
            "entity_aliases",
        ],
        "fact" => &[
            "fact_content_search",
            "fact_embedding_hnsw",
            "fact_index_keys_search",
            "fact_project",
            "fact_project_type",
            "fact_claim_backfill_cursor_idx",
        ],
        "edge" => &[
            "edge_relation",
            "edge_in",
            "edge_out",
            "edge_from_to_idx",
            "edge_temporal_idx",
        ],
        "community" => &["community_summary_search", "community_members"],
        "query_log" => &[
            "query_log_scope_logged_at",
            "query_log_logged_at",
            "query_log_scope_resolved_view_logged_at",
        ],
        "triple" => &[
            "triple_subject_idx",
            "triple_predicate_idx",
            "triple_spo_idx",
        ],
        "memory_event" => &[
            "memory_event_id",
            "memory_event_session_kind",
            "memory_event_disposition",
        ],
        "event_projection_job" => &[
            "event_projection_job_id",
            "event_projection_job_status",
            "event_projection_job_lease",
            "event_projection_job_event",
        ],
        "memory_capture_audit" => &[
            "memory_capture_audit_id",
            "memory_capture_audit_event",
            "memory_capture_audit_disposition",
        ],
        "procedure_candidate" => &[
            "procedure_candidate_id",
            "procedure_candidate_scope_project",
            "procedure_candidate_status",
        ],
        "claim" => &["claim_slot_cursor_idx", "claim_source_projection_idx"],
        "claim_relation" => &[
            "claim_relation_left_active_idx",
            "claim_relation_right_active_idx",
            "claim_relation_context_idx",
            "claim_relation_left_fact_active_idx",
            "claim_relation_right_fact_active_idx",
            "claim_relation_schema_outcome_active_idx",
        ],
        "claim_job" => &["claim_job_lease_idx", "claim_job_fact_idx"],
        "claim_key_alias" => &["claim_alias_lookup_idx"],
        "claim_policy" => &["claim_policy_lookup_idx"],
        "entity_extraction_projection" => &[
            "entity_extraction_projection_episode_idx",
            "entity_extraction_projection_ingested_idx",
        ],
        _ => &[],
    }
}

fn first_info_object<'a>(
    value: &'a Value,
    resource: &str,
) -> Result<&'a serde_json::Map<String, Value>, MemoryError> {
    let object = match value {
        Value::Array(values) => values.first(),
        Value::Object(_) => Some(value),
        _ => None,
    }
    .and_then(Value::as_object)
    .ok_or_else(|| {
        MemoryError::Storage(format!(
            "schema readiness failed: INFO FOR {resource} returned no object"
        ))
    })?;
    Ok(object)
}

fn info_names(
    value: Option<&Value>,
    resource: &str,
) -> Result<std::collections::HashSet<String>, MemoryError> {
    value
        .and_then(Value::as_object)
        .map(|object| object.keys().cloned().collect())
        .ok_or_else(|| {
            MemoryError::Storage(format!(
                "schema readiness failed: INFO FOR {resource} returned no `{resource}` map"
            ))
        })
}

fn migration_record_body(record_id: &str) -> Result<&str, MemoryError> {
    let body = record_id.strip_prefix("script_migration:").ok_or_else(|| {
        MemoryError::Storage(format!("invalid migration ledger id `{record_id}`"))
    })?;
    if body.is_empty()
        || !body
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err(MemoryError::Storage(format!(
            "invalid migration ledger id `{record_id}`"
        )));
    }
    Ok(body)
}

fn migration_owner() -> String {
    format!(
        "{}:{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("migration")
    )
}

fn migration_status(record: &Value) -> Option<&str> {
    record.get("status").and_then(Value::as_str)
}

fn migration_lease_is_active(record: &Value) -> bool {
    let Some(lease) = record
        .get("lease_expires_at")
        .and_then(Value::as_str)
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
    else {
        return false;
    };
    lease > chrono::Utc::now()
}

fn validate_applied_migration_compatibility(
    existing: &Value,
    expected_file_name: &str,
    expected_checksum: &str,
) -> Result<(), MemoryError> {
    let status = migration_status(existing);
    if matches!(status, Some("applying") | Some("failed")) {
        return validate_migration_identity(existing, expected_file_name, expected_checksum);
    }

    validate_applied_migration(existing, expected_file_name, expected_checksum)
}

fn is_record_already_exists_error(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    lowered.contains("already exists")
        && (lowered.contains("record") || lowered.contains("script_migration"))
}

fn is_tolerable_initial_schema_error(message: &str) -> bool {
    let details = message
        .strip_prefix("SurrealDB query statement errors:\n")
        .unwrap_or(message);

    !details.is_empty()
        && details.lines().all(|line| {
            let error = line.split_once(": ").map_or(line, |(_, error)| error);
            is_tolerable_initial_schema_conflict(error)
        })
}

fn is_tolerable_initial_schema_conflict(error: &str) -> bool {
    if error == INITIAL_SCHEMA_FLEXIBLE_COMPATIBILITY_ERROR {
        return true;
    }

    let Some((kind, remainder)) = [
        ("table", error.strip_prefix("The table '")),
        ("field", error.strip_prefix("The field '")),
        ("analyzer", error.strip_prefix("The analyzer '")),
        ("index", error.strip_prefix("The index '")),
    ]
    .into_iter()
    .find_map(|(kind, remainder)| remainder.map(|remainder| (kind, remainder))) else {
        return false;
    };

    let Some(name) = remainder.strip_suffix("' already exists") else {
        return false;
    };

    match kind {
        "table" => INITIAL_SCHEMA_TABLES.contains(&name),
        "field" => INITIAL_SCHEMA_FIELDS.contains(&name),
        "analyzer" => INITIAL_SCHEMA_ANALYZERS.contains(&name),
        "index" => INITIAL_SCHEMA_INDEXES.contains(&name),
        _ => false,
    }
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
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::logging::StdoutLogger;

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

    #[test]
    fn migration_compatibility_allows_recovery_without_executed_at() {
        for status in ["applying", "failed"] {
            let existing = serde_json::json!({
                "script_name": "032_scope_free_active_namespace_expand.surql",
                "checksum": "expected-checksum",
                "status": status
            });

            validate_applied_migration_compatibility(
                &existing,
                "032_scope_free_active_namespace_expand.surql",
                "expected-checksum",
            )
            .expect("recoverable migration records do not need executed_at");
        }
    }

    #[test]
    fn migration_compatibility_rejects_changed_recovery_record() {
        let existing = serde_json::json!({
            "script_name": "032_scope_free_active_namespace_expand.surql",
            "checksum": "expected-checksum",
            "status": "failed"
        });

        let error = validate_applied_migration_compatibility(
            &existing,
            "032_scope_free_active_namespace_expand.surql",
            "different-checksum",
        )
        .expect_err("recovery must not bypass checksum validation");
        assert!(error.to_string().contains("modified"));
    }

    #[test]
    fn migration_lease_activity_is_conservative() {
        let future = (chrono::Utc::now() + chrono::Duration::minutes(1)).to_rfc3339();
        let past = (chrono::Utc::now() - chrono::Duration::minutes(1)).to_rfc3339();

        assert!(migration_lease_is_active(&serde_json::json!({
            "lease_expires_at": future
        })));
        assert!(!migration_lease_is_active(&serde_json::json!({
            "lease_expires_at": past
        })));
        assert!(!migration_lease_is_active(&serde_json::json!({
            "lease_expires_at": "not-a-datetime"
        })));
        assert!(!migration_lease_is_active(&serde_json::json!({})));
    }

    #[test]
    fn initial_schema_tolerates_known_idempotent_definition_conflicts() {
        let message = [
            "statement 0: The table 'episode' already exists",
            "statement 1: The field 'episode_id' already exists",
            "statement 2: The analyzer 'memory_fts' already exists",
            "statement 3: The index 'fact_content_search' already exists",
            "statement 4: An error occurred: FLEXIBLE can only be used in SCHEMAFULL tables",
        ]
        .join("\n");
        let message = format!("SurrealDB query statement errors:\n{message}");

        for line in message.lines().skip(1) {
            let error = line.split_once(": ").map_or(line, |(_, error)| error);
            assert!(
                is_tolerable_initial_schema_conflict(error),
                "unexpectedly rejected known conflict: {error:?}"
            );
        }
        assert!(is_tolerable_initial_schema_error(&message));
        assert!(is_tolerable_initial_schema_error(
            "The table 'episode' already exists"
        ));
    }

    #[test]
    fn initial_schema_rejects_unknown_or_mixed_definition_errors() {
        assert!(!is_tolerable_initial_schema_error(
            "SurrealDB query statement errors:\\nstatement 0: The table 'episode' already exists\\nstatement 1: analyzer error"
        ));
        assert!(!is_tolerable_initial_schema_error(
            "The table 'future_table' already exists"
        ));
        assert!(!is_tolerable_initial_schema_error(
            "The field 'future_field' already exists"
        ));
        assert!(!is_tolerable_initial_schema_error(
            "The analyzer 'future_analyzer' already exists"
        ));
        assert!(!is_tolerable_initial_schema_error(
            "The index 'future_index' already exists"
        ));
    }

    #[tokio::test]
    async fn schema_postconditions_reject_missing_required_resources() {
        let client = SurrealDbClient::connect_in_memory("test_db", "test", "warn")
            .await
            .expect("in-memory db");

        client
            .query(
                "DEFINE TABLE episode SCHEMAFULL; DEFINE FIELD episode_id ON episode TYPE string;",
                None,
                "test",
            )
            .await
            .expect("seed partial schema");

        let error = client
            .verify_schema_postconditions("test")
            .await
            .expect_err("partial schema must fail readiness");
        let message = error.to_string();
        assert!(message.contains("missing table"));
        assert!(message.contains("entity"));
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
