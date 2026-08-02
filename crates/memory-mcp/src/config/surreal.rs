//! SurrealDB connection configuration.

use std::env;

use super::constants::*;
use super::embedding::EmbeddingConfig;
use super::helpers::{default_embedded_data_dir, parse_bool_env, parse_comma_list, parse_env};
use super::lifecycle::LifecycleConfig;
use super::ner::NerConfig;
use crate::service::MemoryError;

/// Configuration for SurrealDB connection.
///
/// Supports both embedded (RocksDB) and remote (WebSocket) modes.
/// For local development, prefer embedded mode. For remote deployments, use a
/// dedicated least-privileged database user instead of root credentials.
///
/// # Examples
///
/// ```rust,no_run
/// use memory_mcp::config::SurrealConfig;
///
/// let config = SurrealConfig::from_env().expect("valid config");
/// ```
#[derive(Debug, Clone)]
pub struct SurrealConfig {
    /// Database name.
    pub db_name: String,
    /// Connection URL (optional for embedded mode).
    pub url: Option<String>,
    /// List of namespaces to use.
    pub namespaces: Vec<String>,
    /// Database username.
    pub username: String,
    /// Database password.
    pub password: String,
    /// Logging level (trace, debug, info, warn, error).
    pub log_level: String,
    /// If true, use embedded RocksDB engine.
    pub embedded: bool,
    /// Optional path to RocksDB data directory.
    pub data_dir: Option<String>,
    /// Lifecycle background job configuration.
    pub lifecycle: LifecycleConfig,
    /// Persist `assemble_context` query analytics to the `query_log` table.
    pub query_logging_enabled: bool,
    /// Retention window for persisted `query_log` analytics rows.
    pub query_log_retention_days: u32,
    /// Optional embedding provider configuration.
    pub embedding: EmbeddingConfig,
    /// NER entity extraction provider configuration.
    pub ner: NerConfig,
}

impl SurrealConfig {
    /// Loads configuration from environment variables.
    ///
    /// # Environment Variables
    ///
    /// | Variable | Required | Description |
    /// |----------|----------|-------------|
    /// | `SURREALDB_DB_NAME` | Yes | Database name |
    /// | `SURREALDB_URL` | Yes (remote) | WebSocket URL |
    /// | `SURREALDB_EMBEDDED` | No | Set to "true" for embedded mode |
    /// | `SURREALDB_DATA_DIR` | No | Path to RocksDB data directory |
    /// | `SURREALDB_NAMESPACES` | Yes | Comma-separated namespaces |
    /// | `SURREALDB_USERNAME` | Yes | Database username |
    /// | `SURREALDB_PASSWORD` | Yes | Database password |
    /// | `RUST_LOG` | No | Logging level (default: "info") |
    /// | `QUERY_LOGGING_ENABLED` | No | Set to "true" to persist `assemble_context` analytics (default: false) |
    /// | `QUERY_LOG_RETENTION_DAYS` | No | Days to retain persisted `query_log` analytics (default: 90) |
    ///
    /// Security note: embedded mode is the preferred local default. Remote mode
    /// should be paired with scoped credentials and host-level authentication.
    ///
    /// # Errors
    ///
    /// Returns `MemoryError::ConfigMissing` if a required variable is not set.
    /// Returns `MemoryError::ConfigInvalid` if namespaces is empty.
    pub fn from_env() -> Result<Self, MemoryError> {
        let db_name = env::var("SURREALDB_DB_NAME")
            .map_err(|_| MemoryError::ConfigMissing("SURREALDB_DB_NAME".to_string()))?;

        let embedded = parse_bool_env("SURREALDB_EMBEDDED").unwrap_or(false);

        let url = if embedded {
            env::var("SURREALDB_URL").ok()
        } else {
            Some(
                env::var("SURREALDB_URL")
                    .map_err(|_| MemoryError::ConfigMissing("SURREALDB_URL".to_string()))?,
            )
        };

        let namespaces = parse_comma_list("SURREALDB_NAMESPACES")?;
        if namespaces.is_empty() {
            return Err(MemoryError::ConfigInvalid(
                "SURREALDB_NAMESPACES is empty".to_string(),
            ));
        }

        let username = env::var("SURREALDB_USERNAME")
            .map_err(|_| MemoryError::ConfigMissing("SURREALDB_USERNAME".to_string()))?;
        let password = env::var("SURREALDB_PASSWORD")
            .map_err(|_| MemoryError::ConfigMissing("SURREALDB_PASSWORD".to_string()))?;
        let log_level = env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
        let data_dir = env::var("SURREALDB_DATA_DIR").ok();
        let query_logging_enabled = parse_bool_env("QUERY_LOGGING_ENABLED").unwrap_or(false);
        let query_log_retention_days = parse_env::<u32>("QUERY_LOG_RETENTION_DAYS")?
            .unwrap_or(DEFAULT_QUERY_LOG_RETENTION_DAYS);

        let lifecycle = LifecycleConfig::from_env();
        let embedding = EmbeddingConfig::from_env()?;
        let ner = NerConfig::from_env()?;

        Ok(Self {
            db_name,
            url,
            namespaces,
            username,
            password,
            log_level,
            embedded,
            data_dir,
            lifecycle,
            query_logging_enabled,
            query_log_retention_days,
            embedding,
            ner,
        })
    }

    /// Returns the first namespace as the default.
    #[must_use]
    pub fn default_namespace(&self) -> Option<&str> {
        self.namespaces.first().map(|s| s.as_str())
    }

    /// Returns the data directory path, using default if not specified.
    #[must_use]
    pub fn data_dir_or_default(&self) -> String {
        self.data_dir
            .clone()
            .unwrap_or_else(default_embedded_data_dir)
    }

    /// Returns the lifecycle configuration.
    #[must_use]
    pub fn lifecycle(&self) -> LifecycleConfig {
        LifecycleConfig::from_env()
    }
}

/// Builder for constructing SurrealConfig programmatically.
///
/// # Examples
///
/// ```rust
/// use memory_mcp::config::SurrealConfigBuilder;
///
/// let config = SurrealConfigBuilder::new()
///     .db_name("memory")
///     .url("ws://localhost:8000")
///     .namespace("personal")
///     .namespace("org")
///     .credentials("user", "pass")
///     .embedded(true)
///     .build();
/// ```
#[derive(Debug)]
pub struct SurrealConfigBuilder {
    db_name: Option<String>,
    url: Option<String>,
    namespaces: Vec<String>,
    username: Option<String>,
    password: Option<String>,
    log_level: String,
    embedded: bool,
    data_dir: Option<String>,
    lifecycle: LifecycleConfig,
    query_logging_enabled: bool,
    query_log_retention_days: u32,
    embedding: EmbeddingConfig,
    ner: NerConfig,
}

impl Default for SurrealConfigBuilder {
    fn default() -> Self {
        Self {
            db_name: None,
            url: None,
            namespaces: Vec::new(),
            username: None,
            password: None,
            log_level: String::new(),
            embedded: false,
            data_dir: None,
            lifecycle: LifecycleConfig::default(),
            query_logging_enabled: false,
            query_log_retention_days: DEFAULT_QUERY_LOG_RETENTION_DAYS,
            embedding: EmbeddingConfig::default(),
            ner: NerConfig::default(),
        }
    }
}

impl SurrealConfigBuilder {
    /// Creates a new builder with default log level.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the database name.
    pub fn db_name(mut self, name: impl Into<String>) -> Self {
        self.db_name = Some(name.into());
        self
    }

    /// Sets the connection URL.
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    /// Adds a namespace to the configuration.
    pub fn namespace(mut self, ns: impl Into<String>) -> Self {
        self.namespaces.push(ns.into());
        self
    }

    /// Sets the username and password.
    pub fn credentials(mut self, username: impl Into<String>, password: impl Into<String>) -> Self {
        self.username = Some(username.into());
        self.password = Some(password.into());
        self
    }

    /// Sets the log level.
    pub fn log_level(mut self, level: impl Into<String>) -> Self {
        self.log_level = level.into();
        self
    }

    /// Enables embedded mode.
    pub fn embedded(mut self, enabled: bool) -> Self {
        self.embedded = enabled;
        self
    }

    /// Sets the data directory for embedded mode.
    pub fn data_dir(mut self, path: impl Into<String>) -> Self {
        self.data_dir = Some(path.into());
        self
    }

    /// Sets the lifecycle configuration.
    pub fn lifecycle_config(mut self, config: LifecycleConfig) -> Self {
        self.lifecycle = config;
        self
    }

    /// Enables or disables persisted query analytics logging.
    pub fn query_logging_enabled(mut self, enabled: bool) -> Self {
        self.query_logging_enabled = enabled;
        self
    }

    /// Sets the retention window for persisted query analytics rows.
    pub fn query_log_retention_days(mut self, days: u32) -> Self {
        self.query_log_retention_days = days;
        self
    }

    /// Sets optional embedding integration configuration.
    pub fn embedding_config(mut self, config: EmbeddingConfig) -> Self {
        self.embedding = config;
        self
    }

    /// Sets NER entity extraction configuration.
    pub fn ner_config(mut self, config: NerConfig) -> Self {
        self.ner = config;
        self
    }

    /// Builds the configuration.
    ///
    /// # Errors
    ///
    /// Returns `MemoryError::ConfigMissing` if required fields are not set.
    /// Returns `MemoryError::ConfigInvalid` if namespaces is empty.
    pub fn build(self) -> Result<SurrealConfig, MemoryError> {
        let db_name = self
            .db_name
            .ok_or_else(|| MemoryError::ConfigMissing("db_name".to_string()))?;
        let username = self
            .username
            .ok_or_else(|| MemoryError::ConfigMissing("username".to_string()))?;
        let password = self
            .password
            .ok_or_else(|| MemoryError::ConfigMissing("password".to_string()))?;

        if self.namespaces.is_empty() {
            return Err(MemoryError::ConfigInvalid(
                "namespaces cannot be empty".to_string(),
            ));
        }

        Ok(SurrealConfig {
            db_name,
            url: self.url,
            namespaces: self.namespaces,
            username,
            password,
            log_level: self.log_level,
            embedded: self.embedded,
            data_dir: self.data_dir,
            lifecycle: self.lifecycle,
            query_logging_enabled: self.query_logging_enabled,
            query_log_retention_days: self.query_log_retention_days,
            embedding: self.embedding,
            ner: self.ner,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::path::PathBuf;

    use super::super::env_lock;
    use super::*;

    #[test]
    fn builder_sets_all_fields() {
        let config = SurrealConfigBuilder::new()
            .db_name("test_db")
            .url("ws://localhost:8000")
            .namespace("personal")
            .namespace("org")
            .credentials("user", "pass")
            .log_level("debug")
            .embedded(true)
            .data_dir("/tmp/data")
            .query_logging_enabled(true)
            .query_log_retention_days(30)
            .build()
            .expect("valid config");

        assert_eq!(config.db_name, "test_db");
        assert_eq!(config.url, Some("ws://localhost:8000".to_string()));
        assert_eq!(config.namespaces, vec!["personal", "org"]);
        assert_eq!(config.username, "user");
        assert_eq!(config.password, "pass");
        assert_eq!(config.log_level, "debug");
        assert!(config.embedded);
        assert_eq!(config.data_dir, Some("/tmp/data".to_string()));
        assert!(config.query_logging_enabled);
        assert_eq!(config.query_log_retention_days, 30);
    }

    #[test]
    fn builder_requires_db_name() {
        let result = SurrealConfigBuilder::new()
            .namespace("test")
            .credentials("u", "p")
            .build();
        assert!(matches!(result, Err(MemoryError::ConfigMissing(_))));
    }

    #[test]
    fn builder_requires_namespaces() {
        let result = SurrealConfigBuilder::new()
            .db_name("test")
            .credentials("u", "p")
            .build();
        assert!(matches!(result, Err(MemoryError::ConfigInvalid(_))));
    }

    #[test]
    fn builder_default_namespace() {
        let config = SurrealConfigBuilder::new()
            .db_name("test")
            .namespace("first")
            .namespace("second")
            .credentials("u", "p")
            .build()
            .expect("valid config");
        assert_eq!(config.default_namespace(), Some("first"));
    }

    #[test]
    fn data_dir_or_default_uses_default() {
        let config = SurrealConfigBuilder::new()
            .db_name("test")
            .namespace("test")
            .credentials("u", "p")
            .build()
            .expect("valid config");
        let default_path = config.data_dir_or_default();
        let expected_prefix = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
            .or_else(|| std::env::current_dir().ok())
            .expect("current_exe or current_dir should be available");

        let default_path_buf = PathBuf::from(default_path);
        assert!(default_path_buf.starts_with(expected_prefix));
        assert!(default_path_buf.ends_with(PathBuf::from("data").join("surrealdb")));
    }

    #[test]
    fn data_dir_or_default_uses_custom() {
        let config = SurrealConfigBuilder::new()
            .db_name("test")
            .namespace("test")
            .credentials("u", "p")
            .data_dir("/custom/path")
            .build()
            .expect("valid config");
        assert_eq!(config.data_dir_or_default(), "/custom/path");
    }

    #[test]
    fn data_dir_or_default_preserves_custom_relative_path() {
        let config = SurrealConfigBuilder::new()
            .db_name("test")
            .namespace("test")
            .credentials("u", "p")
            .data_dir("relative/custom/path")
            .build()
            .expect("valid config");
        assert_eq!(config.data_dir_or_default(), "relative/custom/path");
    }

    #[test]
    fn query_logging_is_disabled_by_default() {
        let config = SurrealConfigBuilder::new()
            .db_name("test")
            .namespace("org")
            .credentials("u", "p")
            .build()
            .expect("valid config");

        assert!(!config.query_logging_enabled);
        assert_eq!(
            config.query_log_retention_days,
            super::super::constants::DEFAULT_QUERY_LOG_RETENTION_DAYS
        );
    }

    #[test]
    fn surreal_config_from_env_can_enable_query_logging() {
        let _guard = env_lock().lock().expect("env lock");

        unsafe {
            env::set_var("SURREALDB_DB_NAME", "memory");
            env::set_var("SURREALDB_NAMESPACES", "org,personal");
            env::set_var("SURREALDB_USERNAME", "root");
            env::set_var("SURREALDB_PASSWORD", "root");
            env::set_var("SURREALDB_EMBEDDED", "true");
            env::set_var("QUERY_LOGGING_ENABLED", "true");
            env::set_var("QUERY_LOG_RETENTION_DAYS", "14");
        }

        let config = SurrealConfig::from_env().expect("config from env");

        unsafe {
            env::remove_var("SURREALDB_DB_NAME");
            env::remove_var("SURREALDB_NAMESPACES");
            env::remove_var("SURREALDB_USERNAME");
            env::remove_var("SURREALDB_PASSWORD");
            env::remove_var("SURREALDB_EMBEDDED");
            env::remove_var("QUERY_LOGGING_ENABLED");
            env::remove_var("QUERY_LOG_RETENTION_DAYS");
        }

        assert!(config.query_logging_enabled);
        assert_eq!(config.query_log_retention_days, 14);
    }
}
