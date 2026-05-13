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

use crate::config::SurrealConfig;
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
use super::queries::{
    active_edge_scan_limit, build_count_facts_needing_reembed_query, build_create_query,
    build_relate_edge_query, build_select_active_facts_by_episode_query,
    build_select_active_facts_query, build_select_communities_by_member_entities_query,
    build_select_communities_matching_summary_query, build_select_edge_neighbors_query,
    build_select_edges_filtered_page_query, build_select_edges_filtered_query,
    build_select_entity_lookup_alias_query, build_select_entity_lookup_canonical_query,
    build_select_episodes_by_content_advanced_query, build_select_episodes_by_content_query,
    build_select_episodes_for_archival_query, build_select_facts_ann_query,
    build_select_facts_by_entity_links_query, build_select_facts_filtered_advanced_query,
    build_select_facts_filtered_query, build_select_facts_needing_reembed_query,
    build_select_one_query, build_update_query, filter_records_by_project,
    filter_records_by_project_and_fact_types,
};
use super::types::GraphDirection;

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

    /// Selects facts with optional project and fact-type filters.
    #[allow(clippy::too_many_arguments)]
    async fn select_facts_filtered_advanced(
        &self,
        namespace: &str,
        scope: &str,
        cutoff: &str,
        query_contains: Option<&str>,
        limit: i32,
        project: Option<&str>,
        fact_types: &[String],
    ) -> Result<Vec<Value>, MemoryError> {
        let records = self
            .select_facts_filtered(namespace, scope, cutoff, query_contains, limit)
            .await?;
        Ok(filter_records_by_project_and_fact_types(
            records, project, fact_types,
        ))
    }

    /// Selects facts that mention any of the supplied entity links using DB-side filtering.
    async fn select_facts_by_entity_links(
        &self,
        namespace: &str,
        scope: &str,
        cutoff: &str,
        entity_links: &[String],
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError>;

    /// Selects nearest-neighbor facts via HNSW ANN index.
    ///
    /// Uses SurrealDB's `<|K,EF|>` operator to leverage the HNSW index
    /// on the `embedding` field, returning only the top-K candidates
    /// with DB-side cosine similarity scoring.
    async fn select_facts_ann(
        &self,
        namespace: &str,
        scope: &str,
        cutoff: &str,
        query_vec: &[f64],
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError>;

    /// Selects edges with DB-side filtering for bi-temporal visibility.
    ///
    /// This helper is retained for compatibility and targeted tests. Production
    /// community rebuilds prefer `select_edges_filtered_page`, while live graph
    /// traversal prefers `select_edge_neighbors` to avoid materializing the full
    /// edge table in one shot.
    async fn select_edges_filtered(
        &self,
        namespace: &str,
        cutoff: &str,
    ) -> Result<Vec<Value>, MemoryError>;

    /// Selects one page of active edges in stable order.
    ///
    /// The default implementation preserves compatibility for test doubles by
    /// delegating to `select_edges_filtered` and slicing the result in memory.
    async fn select_edges_filtered_page(
        &self,
        namespace: &str,
        cutoff: &str,
        start: usize,
        limit: usize,
    ) -> Result<Vec<Value>, MemoryError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let records = self.select_edges_filtered(namespace, cutoff).await?;
        Ok(records.into_iter().skip(start).take(limit).collect())
    }

    /// Selects active graph neighbors for one node without materializing the full edge table.
    async fn select_edge_neighbors(
        &self,
        namespace: &str,
        node_id: &str,
        cutoff: &str,
        direction: GraphDirection,
    ) -> Result<Vec<Value>, MemoryError>;

    /// Selects one entity by canonical name or alias using a parameterized lookup path.
    async fn select_entity_lookup(
        &self,
        namespace: &str,
        normalized_name: &str,
    ) -> Result<Option<Value>, MemoryError>;

    /// Batch entity lookup by multiple normalized names.
    ///
    /// Returns all entities whose `canonical_name_normalized` matches any
    /// of the supplied names, or whose `aliases` contain any of them.
    /// Deduplicates by entity_id.
    async fn select_entities_batch(
        &self,
        namespace: &str,
        names: &[String],
    ) -> Result<Vec<Value>, MemoryError>;

    /// Selects entities by their IDs in a single batch query.
    ///
    /// Returns all entities whose `entity_id` is in the supplied list.
    async fn select_entities_by_ids(
        &self,
        _namespace: &str,
        _entity_ids: &[String],
    ) -> Result<Vec<Value>, MemoryError> {
        Ok(Vec::new())
    }

    /// Selects edges matching a specific (in, relation, out) triple.
    ///
    /// Used for targeted invalidation without full table scans.
    async fn select_edges_for_triple(
        &self,
        _namespace: &str,
        _in_id: &str,
        _relation: &str,
        _out_id: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        Ok(Vec::new())
    }

    /// Selects active (non-invalidated) facts with an optional limit.
    ///
    /// Returns facts where `t_invalid IS NULL`, ordered by `t_valid ASC`.
    /// This avoids full table scans in lifecycle workers.
    async fn select_active_facts(
        &self,
        namespace: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError>;

    /// Counts facts whose embedding metadata does not match the target signature.
    async fn count_facts_needing_reembed(
        &self,
        _namespace: &str,
        _target_signature: &str,
    ) -> Result<usize, MemoryError> {
        Ok(0)
    }

    /// Selects facts needing rewrite in stable `fact_id` order, optionally after a cursor.
    async fn select_facts_needing_reembed(
        &self,
        _namespace: &str,
        _target_signature: &str,
        _last_completed_fact_id: Option<&str>,
        _limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        Ok(Vec::new())
    }

    /// Selects episodes eligible for archival.
    ///
    /// Returns non-archived episodes older than the cutoff, ordered by `t_ref ASC`.
    async fn select_episodes_for_archival(
        &self,
        namespace: &str,
        cutoff: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError>;

    /// Selects active facts linked to a specific episode.
    ///
    /// Returns facts where `source_episode = $episode_id` and `t_invalid IS NULL`
    /// (or `t_invalid > $cutoff`), limited to `limit`.
    async fn select_active_facts_by_episode(
        &self,
        namespace: &str,
        episode_id: &str,
        cutoff: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError>;

    /// Selects episodes whose raw content matches the supplied query.
    ///
    /// Used as a last-resort retrieval fallback for freshly ingested content
    /// that has not yet produced searchable facts.
    async fn select_episodes_by_content(
        &self,
        _namespace: &str,
        _scope: &str,
        _cutoff: &str,
        _query_contains: Option<&str>,
        _limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        Ok(Vec::new())
    }

    /// Selects episodes whose raw content matches the supplied query with an optional project filter.
    async fn select_episodes_by_content_advanced(
        &self,
        namespace: &str,
        scope: &str,
        cutoff: &str,
        query_contains: Option<&str>,
        limit: i32,
        project: Option<&str>,
    ) -> Result<Vec<Value>, MemoryError> {
        let records = self
            .select_episodes_by_content(namespace, scope, cutoff, query_contains, limit)
            .await?;
        Ok(filter_records_by_project(records, project))
    }

    /// Selects communities whose summaries match the supplied query using DB-side search.
    async fn select_communities_matching_summary(
        &self,
        namespace: &str,
        query: &str,
    ) -> Result<Vec<Value>, MemoryError>;

    /// Selects communities that contain any of the given member entities.
    /// Uses array containment check (member_entities CONTAINSANY $members) for index efficiency.
    async fn select_communities_by_member_entities(
        &self,
        namespace: &str,
        member_entities: &[String],
    ) -> Result<Vec<Value>, MemoryError>;

    /// Creates a native graph relation edge while preserving compatibility fields.
    async fn relate_edge(
        &self,
        namespace: &str,
        edge_id: &str,
        from_id: &str,
        to_id: &str,
        content: Value,
    ) -> Result<Value, MemoryError>;

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
        let engine = if config.embedded {
            Self::connect_embedded(config, default_namespace).await?
        } else {
            Self::connect_remote(config, default_namespace).await?
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
        let initial_schema = render_initial_schema_sql(
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
        let rendered_sql = render_migration_sql(migration.sql, self.fact_embedding_dimension);
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

fn render_initial_schema_sql(template: &str, embedding_dimension: usize) -> String {
    template.replace(
        crate::storage::fact_embedding_dimension_placeholder(),
        &embedding_dimension.to_string(),
    )
}

fn render_migration_sql(template: &str, embedding_dimension: usize) -> String {
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

        let (sql, vars) = build_select_facts_filtered_query(scope, cutoff, query_contains, limit);

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

    #[allow(clippy::too_many_arguments)]
    async fn select_facts_filtered_advanced(
        &self,
        namespace: &str,
        scope: &str,
        cutoff: &str,
        query_contains: Option<&str>,
        limit: i32,
        project: Option<&str>,
        fact_types: &[String],
    ) -> Result<Vec<Value>, MemoryError> {
        self.log_op(
            "db.select_facts_filtered_advanced",
            vec![
                ("scope", Value::String(scope.to_string())),
                ("cutoff", Value::String(cutoff.to_string())),
                ("namespace", Value::String(namespace.to_string())),
                ("limit", Value::Number(serde_json::Number::from(limit))),
                ("project", json!(project)),
                (
                    "fact_type_count",
                    Value::Number(serde_json::Number::from(fact_types.len())),
                ),
            ],
        );

        let (sql, vars) = build_select_facts_filtered_advanced_query(
            scope,
            cutoff,
            query_contains,
            limit,
            project,
            fact_types,
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
            "db.select_facts_filtered_advanced.result",
            vec![(
                "count",
                Value::Number(serde_json::Number::from(results.len())),
            )],
        );

        Ok(results)
    }

    async fn select_facts_by_entity_links(
        &self,
        namespace: &str,
        scope: &str,
        cutoff: &str,
        entity_links: &[String],
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        self.log_op(
            "db.select_facts_by_entity_links",
            vec![
                ("scope", Value::String(scope.to_string())),
                ("cutoff", Value::String(cutoff.to_string())),
                ("namespace", Value::String(namespace.to_string())),
                ("limit", Value::Number(serde_json::Number::from(limit))),
                (
                    "entity_link_count",
                    Value::Number(serde_json::Number::from(entity_links.len())),
                ),
            ],
        );

        let (sql, vars) =
            build_select_facts_by_entity_links_query(scope, cutoff, entity_links, limit);

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
            "db.select_facts_by_entity_links.result",
            vec![(
                "count",
                Value::Number(serde_json::Number::from(results.len())),
            )],
        );

        Ok(results)
    }

    async fn select_facts_ann(
        &self,
        namespace: &str,
        scope: &str,
        cutoff: &str,
        query_vec: &[f64],
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        self.log_op(
            "db.select_facts_ann",
            vec![
                ("namespace", Value::String(namespace.to_string())),
                ("scope", Value::String(scope.to_string())),
                ("cutoff", Value::String(cutoff.to_string())),
                ("limit", Value::Number(serde_json::Number::from(limit))),
            ],
        );

        let (sql, vars) = build_select_facts_ann_query(scope, cutoff, query_vec, limit);
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
            "db.select_facts_ann.result",
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

        // Retained for compatibility/test coverage; production community
        // rebuilds prefer paged scans via `select_edges_filtered_page`, while
        // live graph traversal prefers bounded neighbor lookups.
        let (sql, vars) = build_select_edges_filtered_query(cutoff);
        let surreal_val = match self.execute_query(&sql, Some(vars), namespace).await {
            Ok(value) => value,
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => {
                return Ok(Vec::new());
            }
            Err(err) => return Err(err),
        };
        let normalized = surreal_to_json(surreal_val);
        let results = extract_records(normalized);

        // Warn if the edge scan hit the limit — community detection will be incomplete
        if results.len() == active_edge_scan_limit() as usize {
            let mut event = HashMap::new();
            event.insert(
                "op".to_string(),
                Value::String("db.select_edges_filtered.limit_hit".to_string()),
            );
            event.insert(
                "warning".to_string(),
                Value::String(format!(
                    "Edge scan hit limit of {} edges; community detection may be incomplete",
                    active_edge_scan_limit()
                )),
            );
            event.insert(
                "count".to_string(),
                Value::Number(serde_json::Number::from(results.len())),
            );
            self.logger.log(event, LogLevel::Warn);
        }

        self.log_op(
            "db.select_edges_filtered.result",
            vec![(
                "count",
                Value::Number(serde_json::Number::from(results.len())),
            )],
        );

        Ok(results)
    }

    async fn select_edge_neighbors(
        &self,
        namespace: &str,
        node_id: &str,
        cutoff: &str,
        direction: GraphDirection,
    ) -> Result<Vec<Value>, MemoryError> {
        self.log_op(
            "db.select_edge_neighbors",
            vec![
                ("namespace", Value::String(namespace.to_string())),
                ("node_id", Value::String(node_id.to_string())),
                ("cutoff", Value::String(cutoff.to_string())),
                (
                    "direction",
                    Value::String(match direction {
                        GraphDirection::Incoming => "incoming".to_string(),
                        GraphDirection::Outgoing => "outgoing".to_string(),
                    }),
                ),
            ],
        );

        let (sql, vars) = build_select_edge_neighbors_query(node_id, cutoff, direction);
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
            "db.select_edge_neighbors.result",
            vec![(
                "count",
                Value::Number(serde_json::Number::from(results.len())),
            )],
        );

        Ok(results)
    }

    async fn select_edges_filtered_page(
        &self,
        namespace: &str,
        cutoff: &str,
        start: usize,
        limit: usize,
    ) -> Result<Vec<Value>, MemoryError> {
        self.log_op(
            "db.select_edges_filtered_page",
            vec![
                ("cutoff", Value::String(cutoff.to_string())),
                ("namespace", Value::String(namespace.to_string())),
                ("start", Value::Number(serde_json::Number::from(start))),
                ("limit", Value::Number(serde_json::Number::from(limit))),
            ],
        );

        let (sql, vars) = build_select_edges_filtered_page_query(cutoff, limit, start);
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
            "db.select_edges_filtered_page.result",
            vec![
                ("start", Value::Number(serde_json::Number::from(start))),
                (
                    "count",
                    Value::Number(serde_json::Number::from(results.len())),
                ),
            ],
        );

        Ok(results)
    }

    async fn select_entity_lookup(
        &self,
        namespace: &str,
        normalized_name: &str,
    ) -> Result<Option<Value>, MemoryError> {
        self.log_op(
            "db.select_entity_lookup",
            vec![
                ("namespace", Value::String(namespace.to_string())),
                ("name", Value::String(normalized_name.to_string())),
            ],
        );

        let (canonical_sql, canonical_vars) =
            build_select_entity_lookup_canonical_query(normalized_name);
        let canonical_result = match self
            .execute_query(&canonical_sql, Some(canonical_vars), namespace)
            .await
        {
            Ok(value) => {
                let normalized = surreal_to_json(value);
                extract_first_record(normalized)
            }
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => {
                return Ok(None);
            }
            Err(err) => return Err(err),
        };

        if canonical_result.is_some() {
            self.log_op(
                "db.select_entity_lookup.result",
                vec![("found", Value::Bool(true))],
            );
            return Ok(canonical_result);
        }

        let (alias_sql, alias_vars) = build_select_entity_lookup_alias_query(normalized_name);
        let surreal_val = match self
            .execute_query(&alias_sql, Some(alias_vars), namespace)
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
            "db.select_entity_lookup.result",
            vec![("found", Value::Bool(result.is_some()))],
        );

        Ok(result)
    }

    async fn select_entities_batch(
        &self,
        namespace: &str,
        names: &[String],
    ) -> Result<Vec<Value>, MemoryError> {
        if names.is_empty() {
            return Ok(Vec::new());
        }

        self.log_op(
            "db.select_entities_batch",
            vec![
                ("namespace", Value::String(namespace.to_string())),
                (
                    "names_count",
                    Value::Number(serde_json::Number::from(names.len())),
                ),
            ],
        );

        let sql = "SELECT * FROM entity WHERE canonical_name_normalized IN $names OR aliases CONTAINSANY $names";
        let vars = json!({"names": names});

        let surreal_val = match self.execute_query(sql, Some(vars), namespace).await {
            Ok(value) => value,
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => {
                return Ok(Vec::new());
            }
            Err(err) => return Err(err),
        };
        let normalized = surreal_to_json(surreal_val);
        let results = extract_records(normalized);

        self.log_op(
            "db.select_entities_batch.result",
            vec![(
                "count",
                Value::Number(serde_json::Number::from(results.len())),
            )],
        );

        Ok(results)
    }

    async fn select_entities_by_ids(
        &self,
        namespace: &str,
        entity_ids: &[String],
    ) -> Result<Vec<Value>, MemoryError> {
        if entity_ids.is_empty() {
            return Ok(Vec::new());
        }

        self.log_op(
            "db.select_entities_by_ids",
            vec![(
                "count",
                Value::Number(serde_json::Number::from(entity_ids.len())),
            )],
        );

        let sql = "SELECT * FROM entity WHERE entity_id IN $entity_ids";
        let vars = json!({"entity_ids": entity_ids});

        let surreal_val = match self.execute_query(sql, Some(vars), namespace).await {
            Ok(value) => value,
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => {
                return Ok(Vec::new());
            }
            Err(err) => return Err(err),
        };
        let normalized = surreal_to_json(surreal_val);
        Ok(extract_records(normalized))
    }

    async fn select_edges_for_triple(
        &self,
        namespace: &str,
        in_id: &str,
        relation: &str,
        out_id: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        self.log_op(
            "db.select_edges_for_triple",
            vec![
                ("namespace", Value::String(namespace.to_string())),
                ("in_id", Value::String(in_id.to_string())),
                ("relation", Value::String(relation.to_string())),
            ],
        );

        let sql = "SELECT * FROM edge WHERE in = <record> $in_id AND relation = $relation AND out = <record> $out_id";
        let vars = json!({
            "in_id": in_id,
            "relation": relation,
            "out_id": out_id,
        });

        let surreal_val = match self.execute_query(sql, Some(vars), namespace).await {
            Ok(value) => value,
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => {
                return Ok(Vec::new());
            }
            Err(err) => return Err(err),
        };
        let normalized = surreal_to_json(surreal_val);
        Ok(extract_records(normalized))
    }

    async fn select_active_facts(
        &self,
        namespace: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        self.log_op(
            "db.select_active_facts",
            vec![
                ("namespace", Value::String(namespace.to_string())),
                ("limit", Value::Number(serde_json::Number::from(limit))),
            ],
        );

        let (sql, vars) = build_select_active_facts_query(limit);
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
            "db.select_active_facts.result",
            vec![(
                "count",
                Value::Number(serde_json::Number::from(results.len())),
            )],
        );

        Ok(results)
    }

    async fn count_facts_needing_reembed(
        &self,
        namespace: &str,
        target_signature: &str,
    ) -> Result<usize, MemoryError> {
        self.log_op(
            "db.count_facts_needing_reembed",
            vec![
                ("namespace", Value::String(namespace.to_string())),
                (
                    "target_signature",
                    Value::String(target_signature.to_string()),
                ),
            ],
        );

        let (sql, vars) = build_count_facts_needing_reembed_query(target_signature);
        let surreal_val = match self.execute_query(&sql, Some(vars), namespace).await {
            Ok(value) => value,
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => {
                return Ok(0);
            }
            Err(err) => return Err(err),
        };

        let normalized = surreal_to_json(surreal_val);
        let count = extract_first_record(normalized)
            .and_then(|record| record.get("count").cloned())
            .and_then(|value| value.as_u64())
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(0);

        self.log_op(
            "db.count_facts_needing_reembed.result",
            vec![("count", Value::Number(serde_json::Number::from(count)))],
        );

        Ok(count)
    }

    async fn select_facts_needing_reembed(
        &self,
        namespace: &str,
        target_signature: &str,
        last_completed_fact_id: Option<&str>,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        self.log_op(
            "db.select_facts_needing_reembed",
            vec![
                ("namespace", Value::String(namespace.to_string())),
                (
                    "target_signature",
                    Value::String(target_signature.to_string()),
                ),
                (
                    "last_completed_fact_id",
                    last_completed_fact_id
                        .map(|value| Value::String(value.to_string()))
                        .unwrap_or(Value::Null),
                ),
                ("limit", Value::Number(serde_json::Number::from(limit))),
            ],
        );

        let (sql, vars) = build_select_facts_needing_reembed_query(
            target_signature,
            last_completed_fact_id,
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
            "db.select_facts_needing_reembed.result",
            vec![(
                "count",
                Value::Number(serde_json::Number::from(results.len())),
            )],
        );

        Ok(results)
    }

    async fn select_episodes_for_archival(
        &self,
        namespace: &str,
        cutoff: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        self.log_op(
            "db.select_episodes_for_archival",
            vec![
                ("namespace", Value::String(namespace.to_string())),
                ("cutoff", Value::String(cutoff.to_string())),
                ("limit", Value::Number(serde_json::Number::from(limit))),
            ],
        );

        let (sql, vars) = build_select_episodes_for_archival_query(cutoff, limit);
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
            "db.select_episodes_for_archival.result",
            vec![(
                "count",
                Value::Number(serde_json::Number::from(results.len())),
            )],
        );

        Ok(results)
    }

    async fn select_active_facts_by_episode(
        &self,
        namespace: &str,
        episode_id: &str,
        cutoff: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        self.log_op(
            "db.select_active_facts_by_episode",
            vec![
                ("namespace", Value::String(namespace.to_string())),
                ("episode_id", Value::String(episode_id.to_string())),
                ("cutoff", Value::String(cutoff.to_string())),
                ("limit", Value::Number(serde_json::Number::from(limit))),
            ],
        );

        let (sql, vars) = build_select_active_facts_by_episode_query(episode_id, cutoff, limit);
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
            "db.select_active_facts_by_episode.result",
            vec![(
                "count",
                Value::Number(serde_json::Number::from(results.len())),
            )],
        );

        Ok(results)
    }

    async fn select_episodes_by_content(
        &self,
        namespace: &str,
        scope: &str,
        cutoff: &str,
        query_contains: Option<&str>,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        self.log_op(
            "db.select_episodes_by_content",
            vec![
                ("namespace", Value::String(namespace.to_string())),
                ("scope", Value::String(scope.to_string())),
                ("cutoff", Value::String(cutoff.to_string())),
                ("limit", Value::Number(serde_json::Number::from(limit))),
            ],
        );

        let (sql, vars) =
            build_select_episodes_by_content_query(scope, cutoff, query_contains, limit);
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
            "db.select_episodes_by_content.result",
            vec![(
                "count",
                Value::Number(serde_json::Number::from(results.len())),
            )],
        );

        Ok(results)
    }

    async fn select_episodes_by_content_advanced(
        &self,
        namespace: &str,
        scope: &str,
        cutoff: &str,
        query_contains: Option<&str>,
        limit: i32,
        project: Option<&str>,
    ) -> Result<Vec<Value>, MemoryError> {
        self.log_op(
            "db.select_episodes_by_content_advanced",
            vec![
                ("namespace", Value::String(namespace.to_string())),
                ("scope", Value::String(scope.to_string())),
                ("cutoff", Value::String(cutoff.to_string())),
                ("limit", Value::Number(serde_json::Number::from(limit))),
                ("project", json!(project)),
            ],
        );

        let (sql, vars) = build_select_episodes_by_content_advanced_query(
            scope,
            cutoff,
            query_contains,
            limit,
            project,
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
            "db.select_episodes_by_content_advanced.result",
            vec![(
                "count",
                Value::Number(serde_json::Number::from(results.len())),
            )],
        );

        Ok(results)
    }

    async fn select_communities_matching_summary(
        &self,
        namespace: &str,
        query: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        self.log_op(
            "db.select_communities_matching_summary",
            vec![
                ("namespace", Value::String(namespace.to_string())),
                ("query", Value::String(query.to_string())),
            ],
        );

        let (sql, vars) = build_select_communities_matching_summary_query(query);
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
            "db.select_communities_matching_summary.result",
            vec![(
                "count",
                Value::Number(serde_json::Number::from(results.len())),
            )],
        );

        Ok(results)
    }

    async fn select_communities_by_member_entities(
        &self,
        namespace: &str,
        member_entities: &[String],
    ) -> Result<Vec<Value>, MemoryError> {
        self.log_op(
            "db.select_communities_by_member_entities",
            vec![
                ("namespace", Value::String(namespace.to_string())),
                (
                    "member_count",
                    Value::Number(serde_json::Number::from(member_entities.len())),
                ),
            ],
        );

        let (sql, vars) = build_select_communities_by_member_entities_query(member_entities);
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
            "db.select_communities_by_member_entities.result",
            vec![(
                "count",
                Value::Number(serde_json::Number::from(results.len())),
            )],
        );

        Ok(results)
    }

    async fn relate_edge(
        &self,
        namespace: &str,
        edge_id: &str,
        from_id: &str,
        to_id: &str,
        content: Value,
    ) -> Result<Value, MemoryError> {
        self.log_op(
            "db.relate_edge",
            vec![
                ("namespace", Value::String(namespace.to_string())),
                ("edge_id", Value::String(edge_id.to_string())),
                ("from_id", Value::String(from_id.to_string())),
                ("to_id", Value::String(to_id.to_string())),
            ],
        );

        let (sql, vars) = build_relate_edge_query(edge_id, from_id, to_id, content);
        let surreal_val = self.execute_query(&sql, Some(vars), namespace).await?;
        let normalized = surreal_to_json(surreal_val);
        let result = extract_first_record(normalized).unwrap_or(Value::Null);

        self.log_op(
            "db.relate_edge.result",
            vec![("result", Value::String("ok".to_string()))],
        );

        Ok(result)
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
