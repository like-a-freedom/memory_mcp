//! Database abstraction layer for SurrealDB.
//!
//! This module provides a unified interface for database operations,
//! abstracting over embedded and remote (WebSocket) engines.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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

const ACTIVE_EDGE_SCAN_LIMIT: i32 = 10_000;
const FACT_EMBEDDING_DIMENSION_PLACEHOLDER: &str = "__FACT_EMBEDDING_DIMENSION__";

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

    /// Selects episodes by full-text content match for retrieval fallback.
    async fn select_episodes_by_content(
        &self,
        namespace: &str,
        scope: &str,
        query_contains: &str,
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
    /// This helper is retained for compatibility and targeted tests. The live
    /// community traversal path prefers `select_edge_neighbors` to avoid
    /// materializing the full edge table.
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

    /// Selects any facts for an episode, including invalidated ones.
    async fn select_facts_by_episode_any(
        &self,
        namespace: &str,
        episode_id: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError>;

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

    /// Selects episodes linked to an entity through active relation edges and facts.
    ///
    /// Traverses: entity → edge(involved_in) → fact → source_episode → episode.
    async fn select_episodes_by_entity(
        &self,
        namespace: &str,
        entity_id: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError>;

    /// Applies database migrations for a namespace.
    async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError>;

    // Embedding migration methods - default implementations for test mocks
    async fn get_embedding_schema(
        &self,
        _namespace: &str,
    ) -> Result<Option<crate::storage::EmbeddingSchema>, MemoryError> {
        Err(MemoryError::Storage(
            "get_embedding_schema not implemented".into(),
        ))
    }
    async fn set_embedding_schema(
        &self,
        _schema: &crate::storage::EmbeddingSchema,
        _namespace: &str,
    ) -> Result<(), MemoryError> {
        Err(MemoryError::Storage(
            "set_embedding_schema not implemented".into(),
        ))
    }
    async fn create_hnsw_index(
        &self,
        _field: &str,
        _index_name: &str,
        _dim: usize,
        _namespace: &str,
    ) -> Result<(), MemoryError> {
        Err(MemoryError::Storage(
            "create_hnsw_index not implemented".into(),
        ))
    }
    async fn drop_hnsw_index(
        &self,
        _index_name: &str,
        _namespace: &str,
    ) -> Result<(), MemoryError> {
        Err(MemoryError::Storage(
            "drop_hnsw_index not implemented".into(),
        ))
    }
    async fn get_facts_pending_reembed(
        &self,
        _limit: usize,
        _namespace: &str,
    ) -> Result<Vec<(String, String)>, MemoryError> {
        Ok(Vec::new())
    }
    async fn set_fact_next_embedding(
        &self,
        _id: &str,
        _vec: Vec<f64>,
        _namespace: &str,
    ) -> Result<(), MemoryError> {
        Err(MemoryError::Storage(
            "set_fact_next_embedding not implemented".into(),
        ))
    }
    async fn apply_cutover(&self, _namespace: &str) -> Result<(), MemoryError> {
        Err(MemoryError::Storage("apply_cutover not implemented".into()))
    }
    async fn clear_next_embeddings(&self, _namespace: &str) -> Result<(), MemoryError> {
        Err(MemoryError::Storage(
            "clear_next_embeddings not implemented".into(),
        ))
    }
    async fn count_facts(&self, _namespace: &str) -> Result<usize, MemoryError> {
        Ok(0)
    }
    async fn get_facts_without_embedding(
        &self,
        _limit: usize,
        _namespace: &str,
    ) -> Result<Vec<(String, String)>, MemoryError> {
        Ok(Vec::new())
    }
    async fn set_fact_embedding(
        &self,
        _id: &str,
        _vec: Vec<f64>,
        _namespace: &str,
    ) -> Result<(), MemoryError> {
        Err(MemoryError::Storage(
            "set_fact_embedding not implemented".into(),
        ))
    }
}

/// Unified database client that works with both embedded and remote SurrealDB.
pub struct SurrealDbClient {
    engine: DbEngine,
    logger: StdoutLogger,
    fact_embedding_dimension: OnceLock<usize>,
}

// Embedding migration types
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingStatus {
    Ready,
    Migrating,
    Cutover,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EmbeddingSchema {
    pub provider: String,
    pub model: String,
    pub dimension: usize,
    pub status: EmbeddingStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_dimension: Option<usize>,
}

impl EmbeddingSchema {
    pub fn from_config(config: &crate::config::EmbeddingConfig) -> Self {
        use crate::config::EmbeddingProviderKind;
        let provider_name = match config.provider {
            EmbeddingProviderKind::Disabled => "disabled",
            EmbeddingProviderKind::LocalCandle => "local-candle",
            EmbeddingProviderKind::OpenAiCompatible => "openai-compatible",
            EmbeddingProviderKind::Ollama => "ollama",
        };
        Self {
            provider: provider_name.to_string(),
            model: config.model.clone().unwrap_or_default(),
            dimension: config
                .dimension
                .unwrap_or(crate::config::DEFAULT_EMBEDDING_DIMENSION),
            status: EmbeddingStatus::Ready,
            target_provider: None,
            target_model: None,
            target_dimension: None,
        }
    }

    pub fn active_matches_config(&self, config: &crate::config::EmbeddingConfig) -> bool {
        use crate::config::EmbeddingProviderKind;
        let provider_name = match config.provider {
            EmbeddingProviderKind::Disabled => "disabled",
            EmbeddingProviderKind::LocalCandle => "local-candle",
            EmbeddingProviderKind::OpenAiCompatible => "openai-compatible",
            EmbeddingProviderKind::Ollama => "ollama",
        };
        let config_dim = config
            .dimension
            .unwrap_or(crate::config::DEFAULT_EMBEDDING_DIMENSION);
        self.provider == provider_name
            && self.model == config.model.as_deref().unwrap_or_default()
            && self.dimension == config_dim
    }

    pub fn target_matches_config(&self, config: &crate::config::EmbeddingConfig) -> bool {
        use crate::config::EmbeddingProviderKind;
        let provider_name = match config.provider {
            EmbeddingProviderKind::Disabled => "disabled",
            EmbeddingProviderKind::LocalCandle => "local-candle",
            EmbeddingProviderKind::OpenAiCompatible => "openai-compatible",
            EmbeddingProviderKind::Ollama => "ollama",
        };
        let config_dim = config
            .dimension
            .unwrap_or(crate::config::DEFAULT_EMBEDDING_DIMENSION);
        self.target_provider.as_deref() == Some(provider_name)
            && self.target_model.as_deref() == Some(config.model.as_deref().unwrap_or_default())
            && self.target_dimension == Some(config_dim)
    }

    pub fn with_status(&self, status: EmbeddingStatus) -> Self {
        Self {
            status,
            provider: self.provider.clone(),
            model: self.model.clone(),
            dimension: self.dimension,
            target_provider: self.target_provider.clone(),
            target_model: self.target_model.clone(),
            target_dimension: self.target_dimension,
        }
    }

    pub fn as_migration_start(&self, config: &crate::config::EmbeddingConfig) -> Self {
        use crate::config::EmbeddingProviderKind;
        let provider_name = match config.provider {
            EmbeddingProviderKind::Disabled => "disabled",
            EmbeddingProviderKind::LocalCandle => "local-candle",
            EmbeddingProviderKind::OpenAiCompatible => "openai-compatible",
            EmbeddingProviderKind::Ollama => "ollama",
        };
        Self {
            provider: self.provider.clone(),
            model: self.model.clone(),
            dimension: self.dimension,
            status: EmbeddingStatus::Migrating,
            target_provider: Some(provider_name.to_string()),
            target_model: Some(self.model.clone()),
            target_dimension: Some(self.dimension),
        }
    }
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
        let clients = build_local_namespace_clients(&db, namespaces, database).await?;

        Ok(Self {
            engine: DbEngine::Local(clients),
            logger: StdoutLogger::new(log_level),
            fact_embedding_dimension: OnceLock::new(),
        })
    }

    /// Connects to an embedded RocksDB SurrealDB instance for multiple namespaces.
    ///
    /// Use this for eval tests that need stability beyond what in-memory DB provides.
    /// The caller is responsible for cleaning up the data directory.
    pub async fn connect_embedded_with_namespaces(
        data_dir: &str,
        namespaces: &[String],
        log_level: &str,
    ) -> Result<Self, MemoryError> {
        use surrealdb::opt::{Config as SurrealOptConfig, capabilities::Capabilities};

        let path = PathBuf::from(data_dir);
        ensure_dir_exists(path.as_path())?;

        let root = Root {
            username: "root".to_string(),
            password: "root".to_string(),
        };

        let cfg = SurrealOptConfig::new()
            .user(root.clone())
            .capabilities(Capabilities::default());

        let db = Surreal::new::<RocksDb>((path, cfg)).await.map_err(|err| {
            MemoryError::Storage(format!("SurrealDB embedded init failed: {err}"))
        })?;

        db.signin(root)
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB signin failed: {err}")))?;

        let clients = build_local_namespace_clients(&db, namespaces, "memory_eval").await?;

        Ok(Self {
            engine: DbEngine::Local(clients),
            logger: StdoutLogger::new(log_level),
            fact_embedding_dimension: OnceLock::new(),
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

        // Initialize dimension from config if provided, otherwise it will be auto-detected later
        let dimension_once = OnceLock::new();
        if let Some(dim) = config.embedding.dimension {
            let _ = dimension_once.set(dim);
        }

        Ok(Self {
            engine,
            logger: StdoutLogger::new(&config.log_level),
            fact_embedding_dimension: dimension_once,
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
        let event = crate::log_event!(op, "success");
        let mut event = event;
        for (key, value) in details {
            event.insert(key.to_string(), value);
        }
        self.logger.log(event, LogLevel::Debug);
    }

    /// Applies database schema migrations.
    pub async fn apply_migrations_impl(&self, namespace: &str) -> Result<(), MemoryError> {
        // Use default dimension for initial schema if not yet detected
        let dimension = self
            .fact_embedding_dimension
            .get()
            .copied()
            .unwrap_or(crate::config::DEFAULT_EMBEDDING_DIMENSION);

        let initial_schema =
            render_initial_schema_sql(include_str!("migrations/__Initial.surql"), dimension);

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
        // Use default dimension for migrations if not yet detected
        let dimension = self
            .fact_embedding_dimension
            .get()
            .copied()
            .unwrap_or(crate::config::DEFAULT_EMBEDDING_DIMENSION);
        let rendered_sql = render_migration_sql(migration.sql, dimension);
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
        self.execute_query_with_retry(sql, vars, namespace, 2).await
    }

    /// Execute query with retry logic for transient errors (e.g., Session not found).
    async fn execute_query_with_retry(
        &self,
        sql: &str,
        vars: Option<Value>,
        namespace: &str,
        max_retries: u32,
    ) -> Result<SurrealValue, MemoryError> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let result = if self.is_local() {
                let db = self.with_namespace_local(namespace).await?;
                let mut q = db.query(sql);
                if let Some(v) = vars.clone() {
                    q = q.bind(v);
                }
                let mut response = q.await.map_err(|err| {
                    MemoryError::Storage(format!("SurrealDB query failed: {err}"))
                })?;
                response
                    .take::<SurrealValue>(0)
                    .map_err(|err| MemoryError::Storage(format!("SurrealDB take failed: {err}")))
            } else {
                let db = self.with_namespace_remote(namespace).await?;
                let mut q = db.query(sql);
                if let Some(v) = vars.clone() {
                    q = q.bind(v);
                }
                let mut response = q.await.map_err(|err| {
                    MemoryError::Storage(format!("SurrealDB query failed: {err}"))
                })?;
                response
                    .take::<SurrealValue>(0)
                    .map_err(|err| MemoryError::Storage(format!("SurrealDB take failed: {err}")))
            };

            match result {
                Ok(value) => return Ok(value),
                Err(MemoryError::Storage(ref msg))
                    if msg.contains("Session not found") && attempt < max_retries =>
                {
                    self.logger.log(
                        [
                            (
                                "op".to_string(),
                                serde_json::json!("storage.retry.session_not_found"),
                            ),
                            ("attempt".to_string(), serde_json::json!(attempt)),
                            ("max_retries".to_string(), serde_json::json!(max_retries)),
                        ]
                        .into_iter()
                        .collect(),
                        crate::logging::LogLevel::Warn,
                    );
                    tokio::time::sleep(std::time::Duration::from_millis((100 * attempt) as u64))
                        .await;
                    continue;
                }
                Err(e) => return Err(e),
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
    template.replace(
        FACT_EMBEDDING_DIMENSION_PLACEHOLDER,
        &embedding_dimension.to_string(),
    )
}

fn render_migration_sql(template: &str, embedding_dimension: usize) -> String {
    template.replace(
        FACT_EMBEDDING_DIMENSION_PLACEHOLDER,
        &embedding_dimension.to_string(),
    )
}

#[derive(Debug, Clone, Copy)]
struct MigrationScript {
    file_name: &'static str,
    sql: &'static str,
}

fn versioned_migrations() -> &'static [MigrationScript] {
    &[
        MigrationScript {
            file_name: "006_simplified_search_redesign.surql",
            sql: include_str!("migrations/006_simplified_search_redesign.surql"),
        },
        MigrationScript {
            file_name: "007_episode_archival_fields.surql",
            sql: include_str!("migrations/007_episode_archival_fields.surql"),
        },
        MigrationScript {
            file_name: "008_fact_semantic_embeddings.surql",
            sql: include_str!("migrations/008_fact_semantic_embeddings.surql"),
        },
        MigrationScript {
            file_name: "009_adaptive_memory_alignment.surql",
            sql: include_str!("migrations/009_adaptive_memory_alignment.surql"),
        },
        MigrationScript {
            file_name: "010_coerce_t_ingested_to_datetime.surql",
            sql: include_str!("migrations/010_coerce_t_ingested_to_datetime.surql"),
        },
        MigrationScript {
            file_name: "011_ingestion_draft.surql",
            sql: include_str!("migrations/011_ingestion_draft.surql"),
        },
        MigrationScript {
            file_name: "012_app_sessions.surql",
            sql: include_str!("migrations/012_app_sessions.surql"),
        },
        MigrationScript {
            file_name: "013_fact_index_keys_fts.surql",
            sql: include_str!("migrations/013_fact_index_keys_fts.surql"),
        },
        MigrationScript {
            file_name: "014_fact_entity_links_typed.surql",
            sql: include_str!("migrations/014_fact_entity_links_typed.surql"),
        },
        MigrationScript {
            file_name: "015_episode_content_fts.surql",
            sql: include_str!("migrations/015_episode_content_fts.surql"),
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

pub(crate) fn json_string(value: &Value) -> Option<&str> {
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

pub(crate) fn json_f64(value: &Value) -> Option<f64> {
    if let Some(value) = value.as_f64() {
        return Some(value);
    }

    let object = value.as_object()?;

    object
        .get("Number")
        .and_then(json_f64)
        .or_else(|| object.get("Float").and_then(json_f64))
        .or_else(|| object.get("Int").and_then(json_f64))
        .or_else(|| object.get("Decimal").and_then(json_f64))
        .or_else(|| {
            object
                .get("String")
                .and_then(Value::as_str)?
                .parse::<f64>()
                .ok()
        })
}

pub(crate) fn json_i64(value: &Value) -> Option<i64> {
    if let Some(value) = value.as_i64() {
        return Some(value);
    }

    let object = value.as_object()?;

    object
        .get("Number")
        .and_then(json_i64)
        .or_else(|| object.get("Int").and_then(json_i64))
        .or_else(|| {
            object
                .get("String")
                .and_then(Value::as_str)?
                .parse::<i64>()
                .ok()
        })
}

impl SurrealDbClient {
    async fn execute(
        &self,
        sql: &str,
        vars: Option<Value>,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        let result = self.execute_query(sql, vars.clone(), namespace).await?;
        Ok(surreal_to_json(result))
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

    async fn select_episodes_by_content(
        &self,
        namespace: &str,
        scope: &str,
        query_contains: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        self.log_op(
            "db.select_episodes_by_content",
            vec![
                ("scope", Value::String(scope.to_string())),
                ("query", Value::String(query_contains.to_string())),
                ("namespace", Value::String(namespace.to_string())),
                ("limit", Value::Number(serde_json::Number::from(limit))),
            ],
        );

        let (sql, vars) = build_select_episodes_by_content_query(scope, query_contains, limit);

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

        // Retained for compatibility/test coverage; production traversal prefers
        // bounded neighbor lookups via `select_edge_neighbors`.
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
        if results.len() == ACTIVE_EDGE_SCAN_LIMIT as usize {
            self.logger.log(
                crate::log_event!(
                    "db.select_edges_filtered.limit_hit",
                    "warn",
                    "warning" => format!("Edge scan hit limit of {} edges; community detection may be incomplete", ACTIVE_EDGE_SCAN_LIMIT),
                    "count" => results.len()
                ),
                LogLevel::Warn,
            );
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

    async fn select_facts_by_episode_any(
        &self,
        namespace: &str,
        episode_id: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        self.log_op(
            "db.select_facts_by_episode_any",
            vec![
                ("namespace", Value::String(namespace.to_string())),
                ("episode_id", Value::String(episode_id.to_string())),
                ("limit", Value::Number(serde_json::Number::from(limit))),
            ],
        );

        let sql = "SELECT * FROM fact WHERE source_episode = $episode_id ORDER BY t_valid DESC, fact_id ASC LIMIT $limit";
        let vars = json!({"episode_id": episode_id, "limit": limit});
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
            "db.select_facts_by_episode_any.result",
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

    async fn select_episodes_by_entity(
        &self,
        namespace: &str,
        entity_id: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        self.log_op(
            "db.select_episodes_by_entity",
            vec![
                ("entity_id", Value::String(entity_id.to_string())),
                ("namespace", Value::String(namespace.to_string())),
            ],
        );

        let sql = "SELECT * FROM episode WHERE episode_id IN (SELECT VALUE source_episode FROM fact WHERE fact_id IN (SELECT VALUE type::string(out) FROM edge WHERE in = <record> $entity_id AND relation = 'involved_in')) ORDER BY t_ref DESC LIMIT $limit";
        let vars = serde_json::json!({
            "entity_id": entity_id,
            "limit": limit,
        });

        let surreal_val = match self.execute_query(sql, Some(vars), namespace).await {
            Ok(value) => value,
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => {
                return Ok(Vec::new());
            }
            Err(err) => return Err(err),
        };

        let results = extract_records(surreal_to_json(surreal_val));
        self.log_op(
            "db.select_episodes_by_entity.result",
            vec![(
                "count",
                Value::Number(serde_json::Number::from(results.len())),
            )],
        );
        Ok(results)
    }

    async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError> {
        self.apply_migrations_impl(namespace).await
    }

    async fn get_embedding_schema(
        &self,
        namespace: &str,
    ) -> Result<Option<EmbeddingSchema>, MemoryError> {
        let result = self
            .select_one("embedding_schema:embedding", namespace)
            .await?;
        match result {
            Some(Value::Object(obj)) => {
                let schema: EmbeddingSchema = serde_json::from_value(json!(obj)).map_err(|e| {
                    MemoryError::Storage(format!("Failed to parse embedding schema: {}", e))
                })?;
                Ok(Some(schema))
            }
            _ => Ok(None),
        }
    }

    async fn set_embedding_schema(
        &self,
        schema: &EmbeddingSchema,
        namespace: &str,
    ) -> Result<(), MemoryError> {
        let schema_json = serde_json::to_value(schema).map_err(|e| {
            MemoryError::Storage(format!("Failed to serialize embedding schema: {}", e))
        })?;
        self.create("embedding_schema:embedding", schema_json, namespace)
            .await?;
        Ok(())
    }

    async fn create_hnsw_index(
        &self,
        field: &str,
        index_name: &str,
        dim: usize,
        namespace: &str,
    ) -> Result<(), MemoryError> {
        self.execute(
            &format!("REMOVE INDEX IF EXISTS {} ON TABLE fact", index_name),
            None,
            namespace,
        )
        .await?;
        self.execute(
            &format!(
                "DEFINE INDEX {} ON TABLE fact FIELDS {} HNSW DIMENSION {}",
                index_name, field, dim
            ),
            None,
            namespace,
        )
        .await?;
        Ok(())
    }

    async fn drop_hnsw_index(&self, index_name: &str, namespace: &str) -> Result<(), MemoryError> {
        self.execute(
            &format!("REMOVE INDEX IF EXISTS {} ON TABLE fact", index_name),
            None,
            namespace,
        )
        .await?;
        Ok(())
    }

    async fn get_facts_pending_reembed(
        &self,
        limit: usize,
        namespace: &str,
    ) -> Result<Vec<(String, String)>, MemoryError> {
        let sql =
            format!("SELECT id, content FROM fact WHERE embedding_next IS NONE LIMIT {limit}");
        let result = self.execute(&sql, None, namespace).await?;
        parse_id_content_rows(&result)
    }

    async fn set_fact_next_embedding(
        &self,
        id: &str,
        vec: Vec<f64>,
        namespace: &str,
    ) -> Result<(), MemoryError> {
        let sql = format!("UPDATE {id} SET embedding_next = $vec");
        let mut vars = serde_json::Map::new();
        vars.insert("vec".to_string(), json!(vec));
        self.execute(&sql, Some(Value::Object(vars)), namespace)
            .await?;
        Ok(())
    }

    async fn apply_cutover(&self, namespace: &str) -> Result<(), MemoryError> {
        // 1. Clear old embeddings for facts that were not re-embedded
        self.execute(
            "UPDATE fact SET embedding = NONE WHERE embedding_next IS NONE",
            None,
            namespace,
        )
        .await?;
        // 2. Copy new embeddings for re-embedded facts
        self.execute(
            "UPDATE fact SET embedding = embedding_next WHERE embedding_next IS NOT NONE",
            None,
            namespace,
        )
        .await?;
        // 3. Clear staging field
        self.execute(
            "UPDATE fact SET embedding_next = NONE WHERE embedding_next IS NOT NONE",
            None,
            namespace,
        )
        .await?;
        Ok(())
    }

    async fn clear_next_embeddings(&self, namespace: &str) -> Result<(), MemoryError> {
        self.execute("UPDATE fact SET embedding_next = NONE", None, namespace)
            .await?;
        Ok(())
    }

    async fn get_facts_without_embedding(
        &self,
        limit: usize,
        namespace: &str,
    ) -> Result<Vec<(String, String)>, MemoryError> {
        let sql = format!("SELECT id, content FROM fact WHERE embedding IS NONE LIMIT {limit}");
        let result = self.execute(&sql, None, namespace).await?;
        parse_id_content_rows(&result)
    }

    async fn set_fact_embedding(
        &self,
        id: &str,
        vec: Vec<f64>,
        namespace: &str,
    ) -> Result<(), MemoryError> {
        let sql = format!("UPDATE {id} SET embedding = $vec");
        let mut vars = serde_json::Map::new();
        vars.insert("vec".to_string(), json!(vec));
        self.execute(&sql, Some(Value::Object(vars)), namespace)
            .await?;
        Ok(())
    }

    async fn count_facts(&self, namespace: &str) -> Result<usize, MemoryError> {
        let result = self
            .execute("SELECT count() FROM fact GROUP ALL", None, namespace)
            .await?;
        if let Some(arr) = result.as_array()
            && let Some(first) = arr.first()
            && let Some(obj) = first.as_object()
            && let Some(count) = obj.get("count").and_then(|v: &Value| v.as_u64())
        {
            return Ok(count as usize);
        }
        Ok(0)
    }
}

/// Parses a SurrealDB result array of `{id, content}` objects into a Vec.
fn parse_id_content_rows(result: &Value) -> Result<Vec<(String, String)>, MemoryError> {
    let mut facts = Vec::new();
    if let Some(arr) = result.as_array() {
        for item in arr {
            if let Some(obj) = item.as_object() {
                let id = obj
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                let content = obj
                    .get("content")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                facts.push((id, content));
            }
        }
    }
    Ok(facts)
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
        if !id.is_empty() {
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
            "SELECT *, search::score(1) AS ft_score FROM fact WHERE {base_where} AND (content @1@ $query OR index_keys @1@ $query) ORDER BY ft_score DESC, t_valid DESC, fact_id ASC LIMIT $limit"
        )
    } else {
        format!(
            "SELECT * FROM fact WHERE {base_where} ORDER BY t_valid DESC, fact_id ASC LIMIT $limit"
        )
    };

    (sql, Value::Object(vars))
}

fn build_select_episodes_by_content_query(scope: &str, query: &str, limit: i32) -> (String, Value) {
    (
        "SELECT *, search::score(1) AS ft_score FROM episode WHERE scope = $scope AND content @1@ $query ORDER BY ft_score DESC, t_ref DESC, episode_id ASC LIMIT $limit".to_string(),
        json!({
            "scope": scope,
            "query": query,
            "limit": limit,
        }),
    )
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

fn build_select_facts_ann_query(
    scope: &str,
    cutoff: &str,
    query_vec: &[f64],
    limit: i32,
) -> (String, Value) {
    // HNSW ef_search defaults to 4 * K for better recall
    let ef_search = (limit * 4).max(16);
    let sql = format!(
        "SELECT *, vector::similarity::cosine(embedding, $query_vec) AS sem_score \
         FROM fact \
         WHERE scope = $scope \
           AND embedding IS NOT NONE \
           AND embedding IS NOT NULL \
           AND t_valid <= type::datetime($cutoff) \
           AND (t_ingested IS NONE OR t_ingested <= type::datetime($cutoff)) \
           AND (t_invalid IS NONE OR t_invalid > type::datetime($cutoff) OR t_invalid_ingested > type::datetime($cutoff)) \
           AND embedding <|{limit}, {ef_search}|> $query_vec \
         ORDER BY sem_score DESC \
         LIMIT $limit"
    );
    (
        sql,
        json!({
            "scope": scope,
            "cutoff": cutoff,
            "query_vec": query_vec,
            "limit": limit,
        }),
    )
}

fn build_select_active_facts_query(limit: i32) -> (String, Value) {
    (
        "SELECT * FROM fact WHERE (t_invalid IS NONE OR t_invalid IS NULL) ORDER BY t_valid ASC LIMIT $limit".to_string(),
        json!({"limit": limit}),
    )
}

fn build_select_episodes_for_archival_query(cutoff: &str, limit: i32) -> (String, Value) {
    (
        "SELECT * FROM episode WHERE status != 'archived' AND t_ref < type::datetime($cutoff) ORDER BY t_ref ASC LIMIT $limit".to_string(),
        json!({"cutoff": cutoff, "limit": limit}),
    )
}

fn build_select_active_facts_by_episode_query(
    episode_id: &str,
    cutoff: &str,
    limit: i32,
) -> (String, Value) {
    (
        "SELECT * FROM fact WHERE source_episode = $episode_id AND (t_invalid IS NONE OR t_invalid IS NULL OR t_invalid > type::datetime($cutoff)) LIMIT $limit".to_string(),
        json!({"episode_id": episode_id, "cutoff": cutoff, "limit": limit}),
    )
}

fn build_select_edges_filtered_query(cutoff: &str) -> (String, Value) {
    (
        format!(
            "SELECT * FROM edge WHERE t_valid <= type::datetime($cutoff) AND (t_ingested IS NONE OR t_ingested <= type::datetime($cutoff)) AND (t_invalid IS NONE OR t_invalid > type::datetime($cutoff) OR t_invalid_ingested > type::datetime($cutoff)) ORDER BY in ASC, out ASC, t_valid DESC LIMIT {ACTIVE_EDGE_SCAN_LIMIT}"
        ),
        json!({ "cutoff": cutoff }),
    )
}

fn build_select_entity_lookup_canonical_query(normalized_name: &str) -> (String, Value) {
    (
        "SELECT * FROM entity WHERE canonical_name_normalized = $name LIMIT 1".to_string(),
        json!({"name": normalized_name}),
    )
}

fn build_select_entity_lookup_alias_query(normalized_name: &str) -> (String, Value) {
    (
        "SELECT * FROM entity WHERE aliases CONTAINS $name LIMIT 1".to_string(),
        json!({"name": normalized_name}),
    )
}

fn build_select_communities_matching_summary_query(query: &str) -> (String, Value) {
    (
        "SELECT *, search::score(1) AS ft_score FROM community WHERE summary @1@ $query ORDER BY ft_score DESC, summary ASC LIMIT 25".to_string(),
        json!({"query": query}),
    )
}

fn build_select_communities_by_member_entities_query(
    member_entities: &[String],
) -> (String, Value) {
    (
        "SELECT * FROM community WHERE member_entities CONTAINSANY $members ORDER BY community_id ASC".to_string(),
        json!({"members": member_entities}),
    )
}

fn build_select_edge_neighbors_query(
    node_id: &str,
    cutoff: &str,
    direction: GraphDirection,
) -> (String, Value) {
    let node_field = match direction {
        // For `RELATE from -> edge -> to`, incoming edges to `node_id` place the
        // node on the `out` side, while outgoing edges place it on `in`.
        GraphDirection::Incoming => "out",
        GraphDirection::Outgoing => "in",
    };

    (
        format!(
            "SELECT * FROM edge WHERE {node_field} = <record> $node_id AND type::datetime(t_valid) <= type::datetime($cutoff) AND (t_ingested IS NONE OR type::datetime(t_ingested) <= type::datetime($cutoff)) AND (t_invalid IS NONE OR type::datetime(t_invalid) > type::datetime($cutoff) OR type::datetime(t_invalid_ingested) > type::datetime($cutoff)) ORDER BY in ASC, out ASC, t_valid DESC"
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
    let edge_record_literal = record_literal(edge_id);

    if let Value::Object(map) = normalized {
        let (assignments, mut vars) = build_set_assignments("edge", map);
        let all_assignments = assignments;
        vars.insert("edge_id".to_string(), json!(edge_id));
        vars.insert("in_id".to_string(), json!(from_id));
        vars.insert("out_id".to_string(), json!(to_id));

        (
            format!(
                "LET $in = <record> $in_id; LET $out = <record> $out_id; RELATE $in -> {edge_record_literal} -> $out SET {} RETURN *",
                all_assignments.join(", ")
            ),
            Value::Object(vars),
        )
    } else {
        (
            format!(
                "LET $in = <record> $in_id; LET $out = <record> $out_id; RELATE $in -> {edge_record_literal} -> $out SET content = $content RETURN *"
            ),
            json!({
                "edge_id": edge_id,
                "in_id": from_id,
                "out_id": to_id,
                "content": normalized,
            }),
        )
    }
}

fn record_literal(record_id: &str) -> String {
    record_id.split_once(':').map_or_else(
        || record_id.to_string(),
        |(table, key)| format!("{table}:⟨{key}⟩"),
    )
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
        "episode" => &["t_ref", "t_ingested", "archived_at"],
        "fact" | "edge" => &[
            "t_valid",
            "t_ingested",
            "t_invalid",
            "t_invalid_ingested",
            "last_accessed",
        ],
        "community" => &["updated_at"],
        "event_log" => &["ts"],
        "task" => &["due_date"],
        "script_migration" => &["executed_at"],
        "draft_ingestion" => &["created_at", "expires_at"],
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
            match value {
                Value::Null => assignments.push(format!("{key} = NONE")),
                other => {
                    vars.insert(key.clone(), other);
                    assignments.push(format!("{key} = ${key}"));
                }
            }
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
    async fn connect_in_memory_with_namespaces_initializes_requested_namespaces() {
        let client = SurrealDbClient::connect_in_memory_with_namespaces(
            "testdb",
            &["testns".to_string(), "alt".to_string()],
            "warn",
        )
        .await
        .expect("connect in memory");

        client
            .apply_migrations("testns")
            .await
            .expect("apply migrations in default namespace");
        client
            .apply_migrations("alt")
            .await
            .expect("apply migrations in secondary namespace");

        let primary = client
            .select_table("event_log", "testns")
            .await
            .expect("select default namespace table");
        let secondary = client
            .select_table("event_log", "alt")
            .await
            .expect("select secondary namespace table");

        assert!(primary.is_empty());
        assert!(secondary.is_empty());
    }

    #[tokio::test]
    async fn connect_in_memory_applies_memory_fts_indexes() {
        let client = SurrealDbClient::connect_in_memory("testdb", "testns", "warn")
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

        assert!(json_contains_text(&info_json, "fact_content_search"));
        assert!(json_contains_text(&info_json, "memory_fts"));
    }

    #[tokio::test]
    async fn connect_in_memory_fresh_install_provisions_full_post_redesign_schema() {
        let client = SurrealDbClient::connect_in_memory("testdb", "testns", "warn")
            .await
            .expect("connect in memory");

        client
            .apply_migrations("testns")
            .await
            .expect("apply migrations");

        let episode_info = client
            .execute_query("INFO FOR TABLE episode", None, "testns")
            .await
            .expect("info for table episode");
        let community_info = client
            .execute_query("INFO FOR TABLE community", None, "testns")
            .await
            .expect("info for table community");
        let fact_info = client
            .execute_query("INFO FOR TABLE fact", None, "testns")
            .await
            .expect("info for table fact");
        let edge_info = client
            .execute_query("INFO FOR TABLE edge", None, "testns")
            .await
            .expect("info for table edge");
        let migration_info = client
            .execute_query("INFO FOR TABLE script_migration", None, "testns")
            .await
            .expect("info for table script_migration");

        let episode_json = surreal_to_json(episode_info);
        let community_json = surreal_to_json(community_info);
        let fact_json = surreal_to_json(fact_info);
        let edge_json = surreal_to_json(edge_info);
        let migration_json = surreal_to_json(migration_info);

        assert!(json_contains_text(&episode_json, "status"));
        assert!(json_contains_text(&episode_json, "archived_at"));
        assert!(json_contains_text(&fact_json, "embedding"));
        assert!(json_contains_text(&fact_json, "fact_embedding_hnsw"));
        assert!(json_contains_text(
            &community_json,
            "community_summary_search"
        ));
        assert!(json_contains_text(&community_json, "memory_fts"));
        assert!(json_contains_text(&edge_json, "edge_in"));
        assert!(json_contains_text(&edge_json, "edge_out"));
        assert!(json_contains_text(&migration_json, "checksum"));
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

        let record_id = migration_record_id("006_simplified_search_redesign.surql");
        let record = client
            .select_one(&record_id, "testns")
            .await
            .expect("select migration record")
            .expect("stored migration record");
        let expected_checksum = migration_checksum(include_str!(
            "migrations/006_simplified_search_redesign.surql"
        ));

        assert_eq!(
            record.get("script_name").and_then(json_string),
            Some("006_simplified_search_redesign.surql")
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

        let record_id = migration_record_id("006_simplified_search_redesign.surql");
        client
            .update(
                &record_id,
                json!({
                    "script_name": "006_simplified_search_redesign.surql",
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

    #[tokio::test]
    async fn apply_migrations_upgrades_legacy_edge_endpoints_to_native_records() {
        let client = SurrealDbClient::connect_in_memory("testdb", "testns", "warn")
            .await
            .expect("connect in memory");

        client
            .query(
                "DEFINE TABLE entity SCHEMAFULL;\nDEFINE FIELD entity_id ON entity TYPE string;\nDEFINE TABLE edge TYPE RELATION;\nDEFINE FIELD edge_id ON edge TYPE string;\nDEFINE FIELD from_id ON edge TYPE string;\nDEFINE FIELD to_id ON edge TYPE string;\nDEFINE FIELD relation ON edge TYPE string;\nDEFINE FIELD t_valid ON edge TYPE datetime;\nDEFINE FIELD t_ingested ON edge TYPE datetime;\nCREATE entity:⟨alice⟩ SET entity_id = 'entity:alice';\nCREATE entity:⟨bob⟩ SET entity_id = 'entity:bob';\nRELATE entity:⟨alice⟩ -> edge:⟨legacy⟩ -> entity:⟨bob⟩ SET edge_id = 'edge:legacy', from_id = 'entity:alice', to_id = 'entity:bob', relation = 'knows', t_valid = type::datetime('2026-01-15T00:00:00Z'), t_ingested = type::datetime('2026-01-15T00:00:00Z');\nUPDATE edge:⟨legacy⟩ SET in = NONE, out = NONE;",
                None,
                "testns",
            )
            .await
            .expect("seed legacy edge record");

        client
            .apply_migrations("testns")
            .await
            .expect("apply migrations");

        let edges = client
            .select_table("edge", "testns")
            .await
            .expect("select migrated edge table");
        assert_eq!(
            edges.len(),
            1,
            "expected one migrated edge row, got: {edges:?}"
        );
        let edge = &edges[0];

        assert_eq!(
            edge.get("in"),
            Some(&json!({"RecordId": {"table": "entity", "key": "alice"}}))
        );
        assert_eq!(
            edge.get("out"),
            Some(&json!({"RecordId": {"table": "entity", "key": "bob"}}))
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
        assert_eq!(sql, "SELECT * FROM fact:⟨abc123⟩");
        assert_eq!(bind, None);
    }

    #[test]
    fn build_select_one_query_with_entity_id() {
        let (sql, bind) = build_select_one_query("entity:xyz789");
        assert_eq!(sql, "SELECT * FROM entity:⟨xyz789⟩");
        assert_eq!(bind, None);
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
    fn versioned_migrations_include_simplified_search_redesign() {
        let file_names = versioned_migrations()
            .iter()
            .map(|migration| migration.file_name)
            .collect::<Vec<_>>();

        assert!(
            file_names.contains(&"006_simplified_search_redesign.surql"),
            "startup migration registry should include the breaking search redesign migration"
        );
        assert!(
            file_names.contains(&"007_episode_archival_fields.surql"),
            "startup migration registry should include the archival schema follow-up migration"
        );
        assert!(
            file_names.contains(&"008_fact_semantic_embeddings.surql"),
            "startup migration registry should include the semantic embedding follow-up migration"
        );
        assert!(
            file_names.contains(&"009_adaptive_memory_alignment.surql"),
            "startup migration registry should include the adaptive memory alignment migration"
        );
        assert!(
            file_names.contains(&"010_coerce_t_ingested_to_datetime.surql"),
            "startup migration registry should include the datetime coercion follow-up migration"
        );
        assert!(
            file_names.contains(&"011_ingestion_draft.surql"),
            "startup migration registry should include the ingestion draft migration"
        );
        assert!(
            file_names.contains(&"012_app_sessions.surql"),
            "startup migration registry should include the app sessions migration"
        );
        assert!(
            file_names.contains(&"013_fact_index_keys_fts.surql"),
            "startup migration registry should include the fact index keys FTS migration"
        );
        assert!(
            file_names.contains(&"014_fact_entity_links_typed.surql"),
            "startup migration registry should include the typed entity links migration"
        );
        assert!(
            file_names.contains(&"015_episode_content_fts.surql"),
            "startup migration registry should include the episode content FTS fallback migration"
        );
    }

    #[test]
    fn versioned_migrations_keep_runtime_upgrade_scripts_in_order() {
        let migrations = versioned_migrations();

        assert_eq!(
            migrations.len(),
            10,
            "runtime migration registry should include redesign, archival, semantic embedding, adaptive memory, datetime coercion, ingestion draft, app sessions, fact index keys FTS, typed entity links, and episode content FTS"
        );
        assert_eq!(
            migrations[0].file_name,
            "006_simplified_search_redesign.surql"
        );
        assert_eq!(migrations[1].file_name, "007_episode_archival_fields.surql");
        assert_eq!(
            migrations[2].file_name,
            "008_fact_semantic_embeddings.surql"
        );
        assert_eq!(
            migrations[3].file_name,
            "009_adaptive_memory_alignment.surql"
        );
        assert_eq!(
            migrations[4].file_name,
            "010_coerce_t_ingested_to_datetime.surql"
        );
        assert_eq!(migrations[5].file_name, "011_ingestion_draft.surql");
        assert_eq!(migrations[6].file_name, "012_app_sessions.surql");
        assert_eq!(migrations[7].file_name, "013_fact_index_keys_fts.surql");
        assert_eq!(migrations[8].file_name, "014_fact_entity_links_typed.surql");
        assert_eq!(migrations[9].file_name, "015_episode_content_fts.surql");
    }

    #[test]
    fn versioned_migration_006_contains_executable_statements() {
        let migration = versioned_migrations()
            .iter()
            .find(|migration| migration.file_name == "006_simplified_search_redesign.surql")
            .expect("migration 006 should be registered");

        assert!(
            migration_has_statements(migration.sql),
            "migration 006 must stay executable for existing databases"
        );
    }

    #[test]
    fn versioned_migration_007_contains_executable_statements() {
        let migration = versioned_migrations()
            .iter()
            .find(|migration| migration.file_name == "007_episode_archival_fields.surql")
            .expect("migration 007 should be registered");

        assert!(
            migration_has_statements(migration.sql),
            "migration 007 must stay executable for existing databases"
        );
    }

    #[test]
    fn versioned_migration_008_contains_executable_statements() {
        let migration = versioned_migrations()
            .iter()
            .find(|migration| migration.file_name == "008_fact_semantic_embeddings.surql")
            .expect("migration 008 should be registered");

        assert!(
            migration_has_statements(migration.sql),
            "migration 008 must stay executable for existing databases"
        );
    }

    #[test]
    fn versioned_migration_010_contains_executable_statements() {
        let migration = versioned_migrations()
            .iter()
            .find(|migration| migration.file_name == "010_coerce_t_ingested_to_datetime.surql")
            .expect("migration 010 should be registered");

        assert!(
            migration_has_statements(migration.sql),
            "migration 010 must stay executable for existing databases"
        );
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
            sql.contains("content @1@ $query"),
            "expected fulltext operator for text search, got: {sql}"
        );
        assert!(sql.contains("search::score(1) AS ft_score"));
        assert_eq!(vars["scope"], json!("org"));
        assert_eq!(vars["cutoff"], json!("2026-01-15T00:00:00Z"));
        assert_eq!(vars["query"], json!("ARR growth"));
        assert_eq!(vars["limit"], json!(5));
    }

    #[test]
    fn build_select_facts_filtered_query_with_text_query_does_not_add_substring_fallback() {
        let (sql, _vars) =
            build_select_facts_filtered_query("org", "2026-01-15T00:00:00Z", Some("ARR growth"), 5);

        assert!(sql.contains("content @1@ $query"));
        assert!(
            !sql.contains("CONTAINS"),
            "unexpected substring fallback in query: {sql}"
        );
    }

    #[test]
    fn build_select_entity_lookup_canonical_query_parameterizes_exact_match() {
        let (sql, vars) = build_select_entity_lookup_canonical_query("dmitry ivanov");

        assert_eq!(
            sql,
            "SELECT * FROM entity WHERE canonical_name_normalized = $name LIMIT 1"
        );
        assert_eq!(vars, json!({"name": "dmitry ivanov"}));
    }

    #[test]
    fn build_select_entity_lookup_alias_query_parameterizes_alias_match() {
        let (sql, vars) = build_select_entity_lookup_alias_query("dmitry ivanov");

        assert_eq!(
            sql,
            "SELECT * FROM entity WHERE aliases CONTAINS $name LIMIT 1"
        );
        assert_eq!(vars, json!({"name": "dmitry ivanov"}));
    }

    #[test]
    fn build_select_edges_filtered_query_applies_hard_limit() {
        let (sql, vars) = build_select_edges_filtered_query("2026-01-15T00:00:00Z");

        assert!(sql.contains("FROM edge WHERE"));
        assert!(sql.contains("ORDER BY in ASC, out ASC, t_valid DESC"));
        assert!(
            sql.contains("LIMIT 10000"),
            "active edge scans should stay bounded, got: {sql}"
        );
        assert_eq!(vars, json!({"cutoff": "2026-01-15T00:00:00Z"}));
    }

    #[test]
    fn build_select_edge_neighbors_query_parameterizes_incoming_lookup() {
        let (sql, vars) = build_select_edge_neighbors_query(
            "entity:openai",
            "2026-01-15T00:00:00Z",
            GraphDirection::Incoming,
        );

        assert!(sql.contains("out = <record> $node_id"));
        assert!(sql.contains("type::datetime(t_valid) <= type::datetime($cutoff)"));
        assert!(sql.contains("type::datetime(t_ingested) <= type::datetime($cutoff)"));
        assert!(sql.contains("ORDER BY in ASC, out ASC, t_valid DESC"));
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
                "relation": "knows",
                "strength": 1.0,
                "confidence": 0.8,
                "provenance": {"source": "manual"},
                "t_valid": "2026-01-15T00:00:00Z",
                "t_ingested": "2026-01-15T00:00:00Z"
            }),
        );

        assert!(sql.starts_with(
            "LET $in = <record> $in_id; LET $out = <record> $out_id; RELATE $in -> edge:⟨abc123⟩ -> $out SET"
        ));
        assert!(sql.contains("RELATE $in -> edge:⟨abc123⟩ -> $out"));
        assert!(!sql.contains("SET id = $edge"));
        assert!(sql.contains("edge_id = $edge_id"));
        assert_eq!(vars.get("edge_id"), Some(&json!("edge:abc123")));
        assert_eq!(vars.get("in_id"), Some(&json!("entity:alice")));
        assert_eq!(vars.get("out_id"), Some(&json!("entity:bob")));
        assert_eq!(vars.get("relation"), Some(&json!("knows")));
    }

    #[test]
    fn record_literal_preserves_table_and_wraps_key() {
        assert_eq!(record_literal("edge:abc123"), "edge:⟨abc123⟩");
        assert_eq!(record_literal("edge"), "edge");
    }

    #[test]
    fn build_select_facts_filtered_query_orders_by_fulltext_score_first() {
        let (sql, vars) = build_select_facts_filtered_query(
            "org",
            "2026-01-15T00:00:00Z",
            Some("atlas launch"),
            5,
        );

        assert!(sql.contains("search::score(1) AS ft_score"));
        assert!(sql.contains("ORDER BY ft_score DESC, t_valid DESC, fact_id ASC"));
        assert_eq!(vars.get("query"), Some(&json!("atlas launch")));
    }

    #[test]
    fn build_select_episodes_by_content_query_uses_fulltext_search() {
        let (sql, vars) = build_select_episodes_by_content_query("org", "osmp nutanix", 7);

        assert!(sql.contains("FROM episode WHERE scope = $scope AND content @1@ $query"));
        assert!(sql.contains("search::score(1) AS ft_score"));
        assert!(sql.contains("ORDER BY ft_score DESC, t_ref DESC, episode_id ASC"));
        assert_eq!(vars.get("scope"), Some(&json!("org")));
        assert_eq!(vars.get("query"), Some(&json!("osmp nutanix")));
        assert_eq!(vars.get("limit"), Some(&json!(7)));
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
    fn render_initial_schema_sql_substitutes_embedding_dimension_placeholder() {
        let rendered = render_initial_schema_sql(
            "DEFINE INDEX fact_embedding_hnsw ON TABLE fact FIELDS embedding HNSW DIMENSION __FACT_EMBEDDING_DIMENSION__;",
            768,
        );

        assert_eq!(
            rendered,
            "DEFINE INDEX fact_embedding_hnsw ON TABLE fact FIELDS embedding HNSW DIMENSION 768;"
        );
    }

    #[test]
    fn normalize_surreal_json_unwraps_datetime_variants() {
        let datetime = json!({"Datetime": {"String": "2026-01-15T00:00:00Z"}});
        assert_eq!(
            normalize_surreal_json(&datetime),
            json!("2026-01-15T00:00:00Z")
        );
    }

    #[test]
    fn json_f64_handles_wrapped_number_variants() {
        assert_eq!(json_f64(&json!(42.5)), Some(42.5));
        assert_eq!(json_f64(&json!({"Number": 42.5})), Some(42.5));
        assert_eq!(json_f64(&json!({"Float": 42.5})), Some(42.5));
        assert_eq!(json_f64(&json!({"Int": 42})), Some(42.0));
        assert_eq!(
            json_f64(&json!({"Decimal": {"String": "42.5"}})),
            Some(42.5)
        );
    }

    #[test]
    fn build_select_active_facts_query_filters_by_t_invalid() {
        let (sql, vars) = build_select_active_facts_query(500);

        assert!(sql.contains("FROM fact WHERE (t_invalid IS NONE OR t_invalid IS NULL)"));
        assert!(sql.contains("ORDER BY t_valid ASC"));
        assert!(sql.contains("LIMIT $limit"));
        assert_eq!(vars.get("limit"), Some(&json!(500)));
    }

    #[test]
    fn build_select_episodes_for_archival_query_filters_by_status_and_age() {
        let (sql, vars) = build_select_episodes_for_archival_query("2025-06-01T00:00:00Z", 100);

        assert!(sql.contains("FROM episode WHERE status != 'archived'"));
        assert!(sql.contains("t_ref < type::datetime($cutoff)"));
        assert!(sql.contains("ORDER BY t_ref ASC"));
        assert!(sql.contains("LIMIT $limit"));
        assert_eq!(vars.get("cutoff"), Some(&json!("2025-06-01T00:00:00Z")));
        assert_eq!(vars.get("limit"), Some(&json!(100)));
    }

    #[test]
    fn build_select_active_facts_by_episode_query_uses_source_episode() {
        let (sql, vars) =
            build_select_active_facts_by_episode_query("episode:abc", "2026-01-15T00:00:00Z", 1);

        assert!(sql.contains("source_episode = $episode_id"));
        assert!(sql.contains(
            "t_invalid IS NONE OR t_invalid IS NULL OR t_invalid > type::datetime($cutoff)"
        ));
        assert_eq!(vars.get("episode_id"), Some(&json!("episode:abc")));
        assert_eq!(vars.get("cutoff"), Some(&json!("2026-01-15T00:00:00Z")));
        assert_eq!(vars.get("limit"), Some(&json!(1)));
    }

    #[test]
    fn parse_id_content_rows_parses_array() {
        let result = json!([
            {"id": "fact:1", "content": "content 1"},
            {"id": "fact:2", "content": "content 2"},
        ]);
        let rows = parse_id_content_rows(&result).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "fact:1");
        assert_eq!(rows[0].1, "content 1");
    }

    #[test]
    fn parse_id_content_rows_handles_empty_array() {
        let result = json!([]);
        let rows = parse_id_content_rows(&result).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn parse_id_content_rows_handles_non_array() {
        let result = json!({"key": "value"});
        let rows = parse_id_content_rows(&result).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn temporal_field_names_for_table_episode() {
        let fields = temporal_field_names_for_table("episode");
        assert!(fields.contains(&"t_ref"));
        assert!(fields.contains(&"t_ingested"));
        assert!(fields.contains(&"archived_at"));
    }

    #[test]
    fn temporal_field_names_for_table_fact() {
        let fields = temporal_field_names_for_table("fact");
        assert!(fields.contains(&"t_valid"));
        assert!(fields.contains(&"t_ingested"));
        assert!(fields.contains(&"t_invalid"));
        assert!(fields.contains(&"t_invalid_ingested"));
        assert!(fields.contains(&"last_accessed"));
    }

    #[test]
    fn temporal_field_names_for_table_unknown() {
        let fields = temporal_field_names_for_table("unknown_table");
        assert!(fields.is_empty());
    }

    #[test]
    fn temporal_field_names_for_table_community() {
        let fields = temporal_field_names_for_table("community");
        assert!(fields.contains(&"updated_at"));
    }

    #[test]
    fn build_set_assignments_handles_temporal_fields() {
        let mut map = serde_json::Map::new();
        map.insert("content".to_string(), json!("test content"));
        map.insert("t_valid".to_string(), json!("2025-01-15T00:00:00Z"));
        map.insert("t_invalid".to_string(), Value::Null);

        let (assignments, vars) = build_set_assignments("fact", map);

        assert!(assignments.iter().any(|a| a.contains("content = $content")));
        assert!(
            assignments
                .iter()
                .any(|a| a.contains("t_valid = type::datetime($t_valid)"))
        );
        assert!(assignments.iter().any(|a| a.contains("t_invalid = NONE")));
        assert!(vars.contains_key("content"));
        assert!(vars.contains_key("t_valid"));
    }

    #[test]
    fn build_set_assignments_handles_non_temporal_fields() {
        let mut map = serde_json::Map::new();
        map.insert("name".to_string(), json!("Alice"));
        map.insert("age".to_string(), json!(30));

        let (assignments, vars) = build_set_assignments("entity", map);

        assert!(assignments.iter().any(|a| a.contains("name = $name")));
        assert!(assignments.iter().any(|a| a.contains("age = $age")));
        assert_eq!(vars.len(), 2);
    }
}
