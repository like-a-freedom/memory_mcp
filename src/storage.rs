//! Database abstraction layer for SurrealDB.
//!
//! This module provides a unified interface for database operations,
//! abstracting over embedded and remote (WebSocket) engines.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
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

/// Traversal direction for graph neighbor queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphDirection {
    /// Traverse incoming edges pointing to the supplied node.
    Incoming,
    /// Traverse outgoing edges leaving the supplied node.
    Outgoing,
}

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

    /// Selects facts that mention any of the supplied entity links using DB-side filtering.
    async fn select_facts_by_entity_links(
        &self,
        namespace: &str,
        scope: &str,
        cutoff: &str,
        entity_links: &[String],
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError>;

    /// Selects semantically similar facts using the embedding index when embeddings are available.
    async fn select_facts_by_embedding(
        &self,
        namespace: &str,
        scope: &str,
        cutoff: &str,
        embedding: &[f32],
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError>;

    /// Selects edges with DB-side filtering for bi-temporal visibility.
    ///
    /// This remains part of the production API because community maintenance
    /// currently rebuilds connected components from the active edge set.
    async fn select_edges_filtered(
        &self,
        namespace: &str,
        cutoff: &str,
    ) -> Result<Vec<Value>, MemoryError>;

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

    /// Selects communities whose summaries match the supplied query using DB-side search.
    async fn select_communities_matching_summary(
        &self,
        namespace: &str,
        query: &str,
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
    embedding_dimension: usize,
    logger: StdoutLogger,
}

/// Internal enum representing the database engine type.
enum DbEngine {
    Local(std::collections::HashMap<String, Surreal<Db>>),
    Remote(std::collections::HashMap<String, Surreal<Client>>),
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
        Self::connect_in_memory_with_dimension(database, default_namespace, log_level, 4).await
    }

    /// Connects to an embedded in-memory SurrealDB instance with an explicit
    /// embedding dimension for HNSW index creation.
    pub async fn connect_in_memory_with_dimension(
        database: &str,
        default_namespace: &str,
        log_level: &str,
        embedding_dimension: usize,
    ) -> Result<Self, MemoryError> {
        Self::connect_in_memory_with_namespaces_and_dimension(
            database,
            &[default_namespace.to_string()],
            log_level,
            embedding_dimension,
        )
        .await
    }

    /// Connects to an embedded in-memory SurrealDB instance for multiple namespaces.
    pub async fn connect_in_memory_with_namespaces_and_dimension(
        database: &str,
        namespaces: &[String],
        log_level: &str,
        embedding_dimension: usize,
    ) -> Result<Self, MemoryError> {
        let db = Surreal::new::<Mem>(())
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB memory init failed: {err}")))?;
        let clients = build_local_namespace_clients(&db, namespaces, database).await?;

        Ok(Self {
            engine: DbEngine::Local(clients),
            embedding_dimension,
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
            embedding_dimension: config.embedding_dimension,
            logger: StdoutLogger::new(&config.log_level),
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

        let clients =
            build_local_namespace_clients(&db, &config.namespaces, &config.db_name).await?;

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

        let clients =
            build_remote_namespace_clients(&db, &config.namespaces, &config.db_name).await?;

        Ok(DbEngine::Remote(clients))
    }

    /// Gets a database handle with namespace set.
    async fn with_namespace_local(&self, namespace: &str) -> Result<Surreal<Db>, MemoryError> {
        match &self.engine {
            DbEngine::Local(clients) => clients.get(namespace).cloned().ok_or_else(|| {
                MemoryError::Storage(format!("SurrealDB namespace not initialized: {namespace}"))
            }),
            DbEngine::Remote(_) => Err(MemoryError::Storage("expected local engine".into())),
        }
    }

    /// Gets a database handle with namespace set.
    async fn with_namespace_remote(&self, namespace: &str) -> Result<Surreal<Client>, MemoryError> {
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
        let mut event = std::collections::HashMap::new();
        event.insert("op".to_string(), Value::String(op.to_string()));
        for (key, value) in details {
            event.insert(key.to_string(), value);
        }
        self.logger.log(event, LogLevel::Debug);
    }

    /// Applies database schema migrations.
    pub async fn apply_migrations_impl(&self, namespace: &str) -> Result<(), MemoryError> {
        let initial_schema = render_initial_schema_sql(
            include_str!("migrations/__Initial.surql"),
            self.embedding_dimension,
        );

        self.execute_raw_query(&initial_schema, None, namespace)
            .await?;

        for migration in versioned_migrations() {
            self.apply_versioned_migration(namespace, migration).await?;
        }

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

    async fn apply_versioned_migration(
        &self,
        namespace: &str,
        migration: &MigrationScript,
    ) -> Result<(), MemoryError> {
        let record_id = migration_record_id(migration.file_name);
        let checksum = migration_checksum(migration.sql);

        if let Some(existing) = self.select_one(&record_id, namespace).await? {
            validate_applied_migration(&existing, migration.file_name, &checksum)?;
            return Ok(());
        }

        if migration_has_statements(migration.sql) {
            self.execute_raw_query(migration.sql, None, namespace)
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

async fn build_local_namespace_clients(
    base: &Surreal<Db>,
    namespaces: &[String],
    database: &str,
) -> Result<std::collections::HashMap<String, Surreal<Db>>, MemoryError> {
    let mut clients = std::collections::HashMap::with_capacity(namespaces.len());

    for namespace in namespaces {
        let client = base.clone();
        client
            .use_ns(namespace)
            .use_db(database)
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB use failed: {err}")))?;
        clients.insert(namespace.clone(), client);
    }

    Ok(clients)
}

async fn build_remote_namespace_clients(
    base: &Surreal<Client>,
    namespaces: &[String],
    database: &str,
) -> Result<std::collections::HashMap<String, Surreal<Client>>, MemoryError> {
    let mut clients = std::collections::HashMap::with_capacity(namespaces.len());

    for namespace in namespaces {
        let client = base.clone();
        client
            .use_ns(namespace)
            .use_db(database)
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB use failed: {err}")))?;
        clients.insert(namespace.clone(), client);
    }

    Ok(clients)
}

fn render_initial_schema_sql(template: &str, embedding_dimension: usize) -> String {
    template.replace("{{EMBEDDING_DIMENSION}}", &embedding_dimension.to_string())
}

#[derive(Debug, Clone, Copy)]
struct MigrationScript {
    file_name: &'static str,
    sql: &'static str,
}

fn versioned_migrations() -> &'static [MigrationScript] {
    &[
        MigrationScript {
            file_name: "001_init.surql",
            sql: include_str!("migrations/001_init.surql"),
        },
        MigrationScript {
            file_name: "002_datetime_types.surql",
            sql: include_str!("migrations/002_datetime_types.surql"),
        },
        MigrationScript {
            file_name: "003_edge_indexes.surql",
            sql: include_str!("migrations/003_edge_indexes.surql"),
        },
        MigrationScript {
            file_name: "004_migration_checksums.surql",
            sql: include_str!("migrations/004_migration_checksums.surql"),
        },
        MigrationScript {
            file_name: "005_community_summary_search.surql",
            sql: include_str!("migrations/005_community_summary_search.surql"),
        },
    ]
}

fn migration_record_id(file_name: &str) -> String {
    let slug = file_name
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' => character,
            _ => '_',
        })
        .collect::<String>();
    format!("script_migration:{slug}")
}

fn migration_checksum(sql: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(sql.as_bytes());
    hex::encode(hasher.finalize())
}

fn migration_has_statements(sql: &str) -> bool {
    sql.lines()
        .map(str::trim)
        .any(|line| !line.is_empty() && !line.starts_with("--"))
}

fn validate_applied_migration(
    existing: &Value,
    expected_file_name: &str,
    expected_checksum: &str,
) -> Result<(), MemoryError> {
    let Some(map) = existing.as_object() else {
        return Err(MemoryError::Storage(
            "stored migration bookkeeping record must be an object".to_string(),
        ));
    };

    let applied_name = map
        .get("script_name")
        .and_then(json_string)
        .ok_or_else(|| {
            MemoryError::Storage("applied migration record missing script_name".to_string())
        })?;
    let applied_checksum = map.get("checksum").and_then(json_string).ok_or_else(|| {
        MemoryError::Storage("applied migration record missing checksum".to_string())
    })?;
    let executed_at = map
        .get("executed_at")
        .and_then(json_string)
        .ok_or_else(|| {
            MemoryError::Storage("applied migration record missing executed_at".to_string())
        })?;

    if applied_name != expected_file_name {
        return Err(MemoryError::ConfigInvalid(format!(
            "applied migration name mismatch for {expected_file_name}: found {applied_name}"
        )));
    }

    if applied_checksum != expected_checksum {
        return Err(MemoryError::ConfigInvalid(format!(
            "applied migration {expected_file_name} was modified after execution"
        )));
    }

    if chrono::DateTime::parse_from_rfc3339(executed_at).is_err() {
        return Err(MemoryError::Storage(format!(
            "applied migration {expected_file_name} has invalid executed_at"
        )));
    }

    Ok(())
}

fn json_string(value: &Value) -> Option<&str> {
    if let Some(value) = value.as_str() {
        Some(value)
    } else if let Some(object) = value.as_object() {
        object
            .get("String")
            .and_then(Value::as_str)
            .or_else(|| object.get("Strand").and_then(Value::as_str))
            .or_else(|| {
                object
                    .get("Strand")
                    .and_then(|inner| inner.get("String"))
                    .and_then(Value::as_str)
            })
            .or_else(|| object.get("Datetime").and_then(Value::as_str))
            .or_else(|| {
                object
                    .get("Datetime")
                    .and_then(|inner| inner.get("String"))
                    .and_then(Value::as_str)
            })
    } else {
        None
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

    async fn select_facts_by_embedding(
        &self,
        namespace: &str,
        scope: &str,
        cutoff: &str,
        embedding: &[f32],
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        self.log_op(
            "db.select_facts_by_embedding",
            vec![
                ("scope", Value::String(scope.to_string())),
                ("cutoff", Value::String(cutoff.to_string())),
                ("namespace", Value::String(namespace.to_string())),
                ("limit", Value::Number(serde_json::Number::from(limit))),
                (
                    "embedding_dimension",
                    Value::Number(serde_json::Number::from(embedding.len())),
                ),
            ],
        );

        let (sql, vars) = build_select_facts_by_embedding_query(scope, cutoff, embedding, limit);

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
            "db.select_facts_by_embedding.result",
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

        let (sql, vars) = build_select_entity_lookup_query(normalized_name);
        let surreal_val = match self.execute_query(&sql, Some(vars), namespace).await {
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

fn validate_table_name(table: &str) -> Result<(), MemoryError> {
    const ALLOWED_TABLES: &[&str] = &[
        "community",
        "edge",
        "entity",
        "episode",
        "event_log",
        "fact",
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
        (
            format!("CREATE {target} CONTENT $content RETURN *"),
            json!({"content": normalized}),
        )
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
            format!(
                "UPDATE {table}:⟨{id}⟩ SET {} RETURN *",
                assignments.join(", ")
            )
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
        vars.insert("query".to_string(), json!(query));

        format!(
            "SELECT * FROM fact WHERE {base_where} AND content @@ $query ORDER BY t_valid DESC LIMIT $limit"
        )
    } else {
        format!("SELECT * FROM fact WHERE {base_where} ORDER BY t_valid DESC LIMIT $limit")
    };

    (sql, Value::Object(vars))
}

fn build_select_facts_by_entity_links_query(
    scope: &str,
    cutoff: &str,
    entity_links: &[String],
    limit: i32,
) -> (String, Value) {
    (
        "SELECT * FROM fact WHERE scope = $scope AND t_valid <= type::datetime($cutoff) AND (t_ingested IS NONE OR t_ingested <= type::datetime($cutoff)) AND (t_invalid IS NONE OR t_invalid > type::datetime($cutoff) OR t_invalid_ingested > type::datetime($cutoff)) AND entity_links CONTAINSANY $entity_links ORDER BY t_valid DESC LIMIT $limit".to_string(),
        json!({
            "scope": scope,
            "cutoff": cutoff,
            "entity_links": entity_links,
            "limit": limit,
        }),
    )
}

fn build_select_facts_by_embedding_query(
    scope: &str,
    cutoff: &str,
    embedding: &[f32],
    limit: i32,
) -> (String, Value) {
    let knn_limit = limit.max(1);
    (
        format!(
            "SELECT *, vector::distance::cosine(embedding, $embedding) AS semantic_distance FROM fact WHERE embedding <|{knn_limit},150|> $embedding AND scope = $scope AND t_valid <= type::datetime($cutoff) AND (t_ingested IS NONE OR t_ingested <= type::datetime($cutoff)) AND (t_invalid IS NONE OR t_invalid > type::datetime($cutoff) OR t_invalid_ingested > type::datetime($cutoff)) ORDER BY semantic_distance ASC, t_valid DESC LIMIT $limit"
        ),
        json!({
            "scope": scope,
            "cutoff": cutoff,
            "embedding": embedding,
            "limit": knn_limit,
        }),
    )
}

fn build_select_entity_lookup_query(normalized_name: &str) -> (String, Value) {
    (
        "SELECT * FROM entity WHERE canonical_name_normalized = $name OR aliases CONTAINS $name LIMIT 1"
            .to_string(),
        json!({"name": normalized_name}),
    )
}

fn build_select_communities_matching_summary_query(query: &str) -> (String, Value) {
    (
        "SELECT *, search::score(1) AS ft_score FROM community WHERE summary @1@ $query ORDER BY ft_score DESC, summary ASC LIMIT 25".to_string(),
        json!({"query": query}),
    )
}

fn build_select_edge_neighbors_query(
    node_id: &str,
    cutoff: &str,
    direction: GraphDirection,
) -> (String, Value) {
    let node_field = match direction {
        GraphDirection::Incoming => "to_id",
        GraphDirection::Outgoing => "from_id",
    };

    (
        format!(
            "SELECT * FROM edge WHERE {node_field} = $node_id AND t_valid <= type::datetime($cutoff) AND (t_ingested IS NONE OR t_ingested <= type::datetime($cutoff)) AND (t_invalid IS NONE OR t_invalid > type::datetime($cutoff) OR t_invalid_ingested > type::datetime($cutoff)) ORDER BY from_id ASC, to_id ASC, t_valid DESC"
        ),
        json!({"node_id": node_id, "cutoff": cutoff}),
    )
}

fn build_relate_edge_query(
    edge_id: &str,
    from_id: &str,
    to_id: &str,
    content: Value,
) -> (String, Value) {
    let normalized = normalize_surreal_json(&content);

    if let Value::Object(map) = normalized {
        let (assignments, mut vars) = build_set_assignments("edge", map);
        let mut all_assignments = vec!["id = $edge".to_string()];
        all_assignments.extend(assignments);
        vars.insert("edge_id".to_string(), json!(edge_id));
        vars.insert("from_id".to_string(), json!(from_id));
        vars.insert("to_id".to_string(), json!(to_id));

        (
            format!(
                "LET $from = <record> $from_id; LET $to = <record> $to_id; LET $edge = <record> $edge_id; RELATE $from -> edge -> $to SET {} RETURN *",
                all_assignments.join(", ")
            ),
            Value::Object(vars),
        )
    } else {
        (
            "LET $from = <record> $from_id; LET $to = <record> $to_id; LET $edge = <record> $edge_id; RELATE $from -> edge -> $to SET id = $edge, content = $content RETURN *"
                .to_string(),
            json!({
                "edge_id": edge_id,
                "from_id": from_id,
                "to_id": to_id,
                "content": normalized,
            }),
        )
    }
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

    #[tokio::test]
    async fn connect_in_memory_with_dimension_uses_requested_embedding_dimension() {
        let client =
            SurrealDbClient::connect_in_memory_with_dimension("testdb", "testns", "warn", 768)
                .await
                .expect("connect in memory");

        assert_eq!(client.embedding_dimension, 768);
    }

    #[tokio::test]
    async fn connect_in_memory_with_dimension_applies_hnsw_indexes_with_requested_dimension() {
        let client =
            SurrealDbClient::connect_in_memory_with_dimension("testdb", "testns", "warn", 768)
                .await
                .expect("connect in memory");

        client
            .apply_migrations("testns")
            .await
            .expect("apply migrations");

        let info = client
            .execute_query("INFO FOR TABLE fact", None, "testns")
            .await
            .expect("info for table fact");
        let info_json = surreal_to_json(info);

        assert!(json_contains_text(&info_json, "fact_embedding_hnsw"));
        assert!(json_contains_text(&info_json, "DIMENSION 768"));
    }

    #[test]
    fn validate_table_name_rejects_unexpected_identifier() {
        let error = validate_table_name("fact; DELETE user").expect_err("invalid table");
        assert!(
            matches!(error, MemoryError::ConfigInvalid(message) if message.contains("not an allowed query target"))
        );
    }

    #[tokio::test]
    async fn apply_migrations_records_script_name_checksum_and_executed_at() {
        let client = SurrealDbClient::connect_in_memory("testdb", "testns", "warn")
            .await
            .expect("connect in memory");

        client
            .apply_migrations("testns")
            .await
            .expect("apply migrations");

        let record_id = migration_record_id("004_migration_checksums.surql");
        let record = client
            .select_one(&record_id, "testns")
            .await
            .expect("select migration record")
            .expect("stored migration record");
        let expected_checksum =
            migration_checksum(include_str!("migrations/004_migration_checksums.surql"));

        assert_eq!(
            record.get("script_name").and_then(json_string),
            Some("004_migration_checksums.surql")
        );
        assert_eq!(
            record.get("checksum").and_then(json_string),
            Some(expected_checksum.as_str())
        );
        assert!(record.get("executed_at").and_then(json_string).is_some());
    }

    #[tokio::test]
    async fn apply_migrations_rejects_modified_applied_migration() {
        let client = SurrealDbClient::connect_in_memory("testdb", "testns", "warn")
            .await
            .expect("connect in memory");

        client
            .apply_migrations("testns")
            .await
            .expect("initial apply migrations");

        let record_id = migration_record_id("003_edge_indexes.surql");
        client
            .update(
                &record_id,
                json!({
                    "script_name": "003_edge_indexes.surql",
                    "checksum": "tampered",
                    "executed_at": chrono::Utc::now().to_rfc3339(),
                }),
                "testns",
            )
            .await
            .expect("tamper migration record");

        let error = client
            .apply_migrations("testns")
            .await
            .expect_err("modified applied migration should be rejected");
        assert!(
            matches!(error, MemoryError::ConfigInvalid(message) if message.contains("modified after execution"))
        );
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
        assert_eq!(
            vars.get("t_valid"),
            Some(&json!("2026-03-25T17:07:08.958562Z"))
        );
        assert_eq!(
            vars.get("t_ingested"),
            Some(&json!("2026-03-25T17:07:08.958562Z"))
        );
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
        let (sql, vars) =
            build_select_facts_filtered_query("org", "2026-01-15T00:00:00Z", Some("ARR growth"), 5);

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
    fn build_select_facts_filtered_query_with_text_query_does_not_add_substring_fallback() {
        let (sql, _vars) =
            build_select_facts_filtered_query("org", "2026-01-15T00:00:00Z", Some("ARR growth"), 5);

        assert!(sql.contains("content @@ $query"));
        assert!(
            !sql.contains("CONTAINS"),
            "unexpected substring fallback in query: {sql}"
        );
    }

    #[test]
    fn build_select_facts_by_embedding_query_uses_knn_cosine_search() {
        let embedding = vec![0.1_f32, 0.2, 0.3, 0.4];
        let (sql, vars) =
            build_select_facts_by_embedding_query("org", "2026-01-15T00:00:00Z", &embedding, 5);

        assert!(sql.contains("embedding <|5,150|> $embedding"));
        assert!(sql.contains("vector::distance::cosine(embedding, $embedding)"));
        assert_eq!(vars["scope"], json!("org"));
        assert_eq!(vars["cutoff"], json!("2026-01-15T00:00:00Z"));
        assert_eq!(vars["embedding"], json!(embedding));
        assert_eq!(vars["limit"], json!(5));
    }

    #[test]
    fn build_select_entity_lookup_query_parameterizes_canonical_and_alias_match() {
        let (sql, vars) = build_select_entity_lookup_query("dmitry ivanov");

        assert_eq!(
            sql,
            "SELECT * FROM entity WHERE canonical_name_normalized = $name OR aliases CONTAINS $name LIMIT 1"
        );
        assert_eq!(vars, json!({"name": "dmitry ivanov"}));
    }

    #[test]
    fn build_select_edge_neighbors_query_parameterizes_incoming_lookup() {
        let (sql, vars) = build_select_edge_neighbors_query(
            "entity:openai",
            "2026-01-15T00:00:00Z",
            GraphDirection::Incoming,
        );

        assert!(sql.contains("to_id = $node_id"));
        assert!(sql.contains("t_valid <= type::datetime($cutoff)"));
        assert_eq!(
            vars,
            json!({"node_id": "entity:openai", "cutoff": "2026-01-15T00:00:00Z"})
        );
    }

    #[test]
    fn build_relate_edge_query_uses_native_relate_syntax() {
        let (sql, vars) = build_relate_edge_query(
            "edge:abc123",
            "entity:alice",
            "entity:bob",
            json!({
                "edge_id": "edge:abc123",
                "from_id": "entity:alice",
                "relation": "knows",
                "to_id": "entity:bob",
                "strength": 1.0,
                "confidence": 0.8,
                "provenance": {"source": "manual"},
                "t_valid": "2026-01-15T00:00:00Z",
                "t_ingested": "2026-01-15T00:00:00Z"
            }),
        );

        assert!(sql.starts_with("LET $from = <record> $from_id; LET $to = <record> $to_id; LET $edge = <record> $edge_id; RELATE $from -> edge -> $to SET"));
        assert!(sql.contains("id = $edge"));
        assert_eq!(vars.get("edge_id"), Some(&json!("edge:abc123")));
        assert_eq!(vars.get("from_id"), Some(&json!("entity:alice")));
        assert_eq!(vars.get("to_id"), Some(&json!("entity:bob")));
        assert_eq!(vars.get("relation"), Some(&json!("knows")));
    }

    #[test]
    fn build_select_communities_matching_summary_query_uses_fulltext_search() {
        let (sql, vars) = build_select_communities_matching_summary_query("alice project");

        assert!(sql.contains("FROM community WHERE summary @1@ $query"));
        assert!(sql.contains("search::score(1) AS ft_score"));
        assert!(sql.contains("ORDER BY ft_score DESC, summary ASC"));
        assert_eq!(vars.get("query"), Some(&json!("alice project")));
    }

    fn json_contains_text(value: &Value, expected: &str) -> bool {
        match value {
            Value::String(text) => text.contains(expected),
            Value::Array(values) => values
                .iter()
                .any(|value| json_contains_text(value, expected)),
            Value::Object(map) => map
                .values()
                .any(|value| json_contains_text(value, expected)),
            _ => false,
        }
    }

    #[test]
    fn render_initial_schema_sql_replaces_embedding_dimension_placeholder() {
        let rendered = render_initial_schema_sql(
            "DEFINE INDEX fact_embedding_hnsw ON fact FIELDS embedding HNSW DIMENSION {{EMBEDDING_DIMENSION}} DIST COSINE TYPE F32 EFC 150 M 8;",
            768,
        );

        assert!(rendered.contains("HNSW DIMENSION 768 DIST COSINE"));
        assert!(!rendered.contains("{{EMBEDDING_DIMENSION}}"));
    }

    #[test]
    fn normalize_surreal_json_unwraps_datetime_variants() {
        let datetime = json!({"Datetime": {"String": "2026-01-15T00:00:00Z"}});
        assert_eq!(
            normalize_surreal_json(&datetime),
            json!("2026-01-15T00:00:00Z")
        );
    }
}
