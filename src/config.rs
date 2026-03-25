//! Configuration management for the Memory MCP system.
//!
//! This module provides configuration loading from environment variables
//! with support for both embedded and remote SurrealDB deployments.

use std::env;
use std::path::PathBuf;

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
    /// | `LOG_LEVEL` | No | Logging level (default: "warn") |
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
        let log_level = env::var("LOG_LEVEL").unwrap_or_else(|_| "warn".to_string());
        let data_dir = env::var("SURREALDB_DATA_DIR").ok();

        Ok(Self {
            db_name,
            url,
            namespaces,
            username,
            password,
            log_level,
            embedded,
            data_dir,
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
}

/// Returns the default embedded SurrealDB path.
///
/// If no explicit path is configured, we store DB files next to the running
/// executable to make runtime behavior independent from process working
/// directory.
fn default_embedded_data_dir() -> String {
    let base_dir = env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
        .or_else(|| env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    base_dir
        .join("data")
        .join("surrealdb")
        .to_string_lossy()
        .to_string()
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
#[derive(Debug, Default)]
pub struct SurrealConfigBuilder {
    db_name: Option<String>,
    url: Option<String>,
    namespaces: Vec<String>,
    username: Option<String>,
    password: Option<String>,
    log_level: String,
    embedded: bool,
    data_dir: Option<String>,
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
        })
    }
}

/// Parses a boolean environment variable.
///
/// Recognizes "1", "true", "yes" (case-insensitive) as true.
fn parse_bool_env(var_name: &str) -> Option<bool> {
    env::var(var_name)
        .ok()
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes"))
}

/// Parses a comma-separated list from an environment variable.
///
/// # Errors
///
/// Returns `MemoryError::ConfigMissing` if the variable is not set.
fn parse_comma_list(var_name: &str) -> Result<Vec<String>, MemoryError> {
    let raw = env::var(var_name).map_err(|_| MemoryError::ConfigMissing(var_name.to_string()))?;

    Ok(raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect())
}

#[cfg(test)]
mod tests {
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
}
