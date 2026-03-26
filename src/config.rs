//! Configuration management for the Memory MCP system.
//!
//! This module provides configuration loading from environment variables
//! with support for both embedded and remote SurrealDB deployments.

use std::env;
use std::path::PathBuf;

use crate::service::MemoryError;

/// Default vector dimension used for fact embeddings.
pub const DEFAULT_EMBEDDING_DIMENSION: usize = 1536;

/// Default timeout for embedding provider HTTP requests.
pub const DEFAULT_EMBEDDING_TIMEOUT_SECS: u64 = 15;

/// Supported embedding provider kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingProviderKind {
    /// Semantic retrieval is disabled.
    Disabled,
    /// OpenAI-compatible `/embeddings` endpoint.
    OpenAiCompatible,
    /// Ollama `/api/embeddings` endpoint.
    Ollama,
}

/// Configuration for optional embedding provider integration.
#[derive(Debug, Clone)]
pub struct EmbeddingConfig {
    /// Which provider to use.
    pub provider: EmbeddingProviderKind,
    /// Provider base URL.
    pub base_url: Option<String>,
    /// Embedding model name.
    pub model: Option<String>,
    /// Optional API key for OpenAI-compatible providers.
    pub api_key: Option<String>,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
    /// Expected embedding vector dimension.
    pub dimension: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: EmbeddingProviderKind::Disabled,
            base_url: None,
            model: None,
            api_key: None,
            timeout_secs: DEFAULT_EMBEDDING_TIMEOUT_SECS,
            dimension: DEFAULT_EMBEDDING_DIMENSION,
        }
    }
}

impl EmbeddingConfig {
    /// Loads optional embedding provider configuration from environment variables.
    ///
    /// When disabled, the rest of the server keeps working without semantic retrieval.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::ConfigInvalid`] for invalid provider names or dimensions,
    /// and [`MemoryError::ConfigMissing`] when a required variable is absent while
    /// embeddings are enabled.
    pub fn from_env() -> Result<Self, MemoryError> {
        let enabled = parse_bool_env("EMBEDDINGS_ENABLED").unwrap_or(false);
        let timeout_secs =
            parse_u64_env("EMBEDDINGS_TIMEOUT_SECS")?.unwrap_or(DEFAULT_EMBEDDING_TIMEOUT_SECS);
        let dimension = parse_usize_env("SURREALDB_EMBEDDING_DIMENSION")?
            .unwrap_or(DEFAULT_EMBEDDING_DIMENSION);

        if !enabled {
            return Ok(Self {
                timeout_secs,
                dimension,
                ..Self::default()
            });
        }

        let provider = match env::var("EMBEDDINGS_PROVIDER")
            .map_err(|_| MemoryError::ConfigMissing("EMBEDDINGS_PROVIDER".to_string()))?
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "openai" | "openai-compatible" | "openai_compatible" => {
                EmbeddingProviderKind::OpenAiCompatible
            }
            "ollama" => EmbeddingProviderKind::Ollama,
            other => {
                return Err(MemoryError::ConfigInvalid(format!(
                    "unsupported EMBEDDINGS_PROVIDER `{other}`"
                )));
            }
        };

        let model = Some(
            env::var("EMBEDDINGS_MODEL")
                .map_err(|_| MemoryError::ConfigMissing("EMBEDDINGS_MODEL".to_string()))?,
        );

        let base_url = match provider {
            EmbeddingProviderKind::OpenAiCompatible => Some(
                env::var("EMBEDDINGS_BASE_URL")
                    .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
            ),
            EmbeddingProviderKind::Ollama => Some(
                env::var("EMBEDDINGS_BASE_URL")
                    .unwrap_or_else(|_| "http://127.0.0.1:11434".to_string()),
            ),
            EmbeddingProviderKind::Disabled => None,
        };

        Ok(Self {
            provider,
            base_url,
            model,
            api_key: env::var("EMBEDDINGS_API_KEY").ok(),
            timeout_secs,
            dimension,
        })
    }

    /// Returns true when semantic embeddings should be used.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        !matches!(self.provider, EmbeddingProviderKind::Disabled)
    }
}

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
    /// Optional embedding provider configuration.
    pub embedding: EmbeddingConfig,
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

        let lifecycle = LifecycleConfig::from_env();
        let embedding = EmbeddingConfig::from_env()?;

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
            embedding,
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

/// Configuration for background lifecycle jobs.
///
/// Controls confidence decay refresh and episode archival workers.
/// Both workers are disabled by default and must be explicitly enabled.
///
/// # Environment Variables
///
/// | Variable | Default | Description |
/// |----------|---------|-------------|
/// | `LIFECYCLE_ENABLED` | false | Enable background workers |
/// | `LIFECYCLE_DECAY_INTERVAL_SECS` | 3600 | Decay job interval (seconds) |
/// | `LIFECYCLE_ARCHIVAL_INTERVAL_SECS` | 86400 | Archival job interval (seconds) |
/// | `LIFECYCLE_DECAY_THRESHOLD` | 0.3 | Confidence threshold for invalidation |
/// | `LIFECYCLE_ARCHIVAL_AGE_DAYS` | 90 | Days before episode archival |
/// | `LIFECYCLE_DECAY_HALF_LIFE_DAYS` | 365 | Half-life (days) for decay computation |
///
/// # Examples
///
/// ```rust,no_run
/// use memory_mcp::config::LifecycleConfig;
///
/// let config = LifecycleConfig::from_env();
/// if config.enabled {
///     // Start background workers
/// }
/// ```
#[derive(Debug, Clone)]
pub struct LifecycleConfig {
    /// Enable background lifecycle workers.
    pub enabled: bool,
    /// Interval for decay refresh job (seconds).
    pub decay_interval_secs: u64,
    /// Interval for episode archival job (seconds).
    pub archival_interval_secs: u64,
    /// Confidence threshold below which facts are marked invalid.
    pub decay_confidence_threshold: f64,
    /// Days after which episodes are archived (no active facts).
    pub archival_age_days: u32,
    /// Half-life in days for confidence decay computation.
    pub decay_half_life_days: f64,
}

impl Default for LifecycleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            decay_interval_secs: 3600,
            archival_interval_secs: 86400,
            decay_confidence_threshold: 0.3,
            archival_age_days: 90,
            decay_half_life_days: 365.0,
        }
    }
}

impl LifecycleConfig {
    /// Loads lifecycle configuration from environment variables.
    ///
    /// # Environment Variables
    ///
    /// | Variable | Default | Description |
    /// |----------|---------|-------------|
    /// | `LIFECYCLE_ENABLED` | false | Enable background workers |
    /// | `LIFECYCLE_DECAY_INTERVAL_SECS` | 3600 | Decay job interval |
    /// | `LIFECYCLE_ARCHIVAL_INTERVAL_SECS` | 86400 | Archival job interval |
    /// | `LIFECYCLE_DECAY_THRESHOLD` | 0.3 | Confidence threshold |
    /// | `LIFECYCLE_ARCHIVAL_AGE_DAYS` | 90 | Episode age threshold |
    /// | `LIFECYCLE_DECAY_HALF_LIFE_DAYS` | 365 | Half-life for decay |
    ///
    /// # Examples
    ///
    /// ```rust
    /// use memory_mcp::config::LifecycleConfig;
    ///
    /// let config = LifecycleConfig::from_env();
    /// assert!(!config.enabled); // disabled by default
    /// ```
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            enabled: parse_bool_env("LIFECYCLE_ENABLED").unwrap_or(false),
            decay_interval_secs: env::var("LIFECYCLE_DECAY_INTERVAL_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3600),
            archival_interval_secs: env::var("LIFECYCLE_ARCHIVAL_INTERVAL_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(86400),
            decay_confidence_threshold: env::var("LIFECYCLE_DECAY_THRESHOLD")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.3),
            archival_age_days: env::var("LIFECYCLE_ARCHIVAL_AGE_DAYS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(90),
            decay_half_life_days: env::var("LIFECYCLE_DECAY_HALF_LIFE_DAYS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(365.0),
        }
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
    lifecycle: LifecycleConfig,
    embedding: EmbeddingConfig,
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

    /// Sets optional embedding integration configuration.
    pub fn embedding_config(mut self, config: EmbeddingConfig) -> Self {
        self.embedding = config;
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
            embedding: self.embedding,
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

fn parse_u64_env(var_name: &str) -> Result<Option<u64>, MemoryError> {
    env::var(var_name)
        .ok()
        .map(|value| {
            value.parse::<u64>().map_err(|_| {
                MemoryError::ConfigInvalid(format!("{var_name} must be an unsigned integer"))
            })
        })
        .transpose()
}

fn parse_usize_env(var_name: &str) -> Result<Option<usize>, MemoryError> {
    env::var(var_name)
        .ok()
        .map(|value| {
            value.parse::<usize>().map_err(|_| {
                MemoryError::ConfigInvalid(format!("{var_name} must be a positive integer"))
            })
        })
        .transpose()
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
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

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

    #[test]
    fn lifecycle_config_defaults() {
        let config = LifecycleConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.decay_interval_secs, 3600);
        assert_eq!(config.archival_interval_secs, 86400);
        assert_eq!(config.decay_confidence_threshold, 0.3);
        assert_eq!(config.archival_age_days, 90);
        assert_eq!(config.decay_half_life_days, 365.0);
    }

    #[test]
    fn embedding_config_defaults_to_disabled() {
        let config = EmbeddingConfig::default();

        assert!(!config.is_enabled());
        assert_eq!(config.provider, EmbeddingProviderKind::Disabled);
        assert_eq!(config.dimension, DEFAULT_EMBEDDING_DIMENSION);
        assert_eq!(config.timeout_secs, DEFAULT_EMBEDDING_TIMEOUT_SECS);
    }

    #[test]
    fn embedding_config_from_env_supports_ollama() {
        let _guard = env_lock().lock().expect("env lock");

        unsafe {
            env::set_var("EMBEDDINGS_ENABLED", "true");
            env::set_var("EMBEDDINGS_PROVIDER", "ollama");
            env::set_var("EMBEDDINGS_MODEL", "nomic-embed-text");
            env::set_var("EMBEDDINGS_TIMEOUT_SECS", "7");
            env::set_var("SURREALDB_EMBEDDING_DIMENSION", "768");
        }

        let config = EmbeddingConfig::from_env().expect("embedding config");

        unsafe {
            env::remove_var("EMBEDDINGS_ENABLED");
            env::remove_var("EMBEDDINGS_PROVIDER");
            env::remove_var("EMBEDDINGS_MODEL");
            env::remove_var("EMBEDDINGS_TIMEOUT_SECS");
            env::remove_var("SURREALDB_EMBEDDING_DIMENSION");
        }

        assert!(config.is_enabled());
        assert_eq!(config.provider, EmbeddingProviderKind::Ollama);
        assert_eq!(config.base_url.as_deref(), Some("http://127.0.0.1:11434"));
        assert_eq!(config.model.as_deref(), Some("nomic-embed-text"));
        assert_eq!(config.timeout_secs, 7);
        assert_eq!(config.dimension, 768);
    }

    #[test]
    fn embedding_config_from_env_requires_model_when_enabled() {
        let _guard = env_lock().lock().expect("env lock");

        unsafe {
            env::set_var("EMBEDDINGS_ENABLED", "true");
            env::set_var("EMBEDDINGS_PROVIDER", "openai-compatible");
        }

        let error = EmbeddingConfig::from_env().expect_err("missing model should error");

        unsafe {
            env::remove_var("EMBEDDINGS_ENABLED");
            env::remove_var("EMBEDDINGS_PROVIDER");
        }

        assert!(matches!(error, MemoryError::ConfigMissing(name) if name == "EMBEDDINGS_MODEL"));
    }

    #[test]
    fn lifecycle_config_from_env() {
        let _guard = env_lock().lock().expect("env lock");

        unsafe {
            env::set_var("LIFECYCLE_ENABLED", "true");
            env::set_var("LIFECYCLE_DECAY_INTERVAL_SECS", "1800");
            env::set_var("LIFECYCLE_ARCHIVAL_INTERVAL_SECS", "43200");
            env::set_var("LIFECYCLE_DECAY_THRESHOLD", "0.5");
            env::set_var("LIFECYCLE_ARCHIVAL_AGE_DAYS", "60");
            env::set_var("LIFECYCLE_DECAY_HALF_LIFE_DAYS", "180");
        }

        let config = LifecycleConfig::from_env();

        unsafe {
            env::remove_var("LIFECYCLE_ENABLED");
            env::remove_var("LIFECYCLE_DECAY_INTERVAL_SECS");
            env::remove_var("LIFECYCLE_ARCHIVAL_INTERVAL_SECS");
            env::remove_var("LIFECYCLE_DECAY_THRESHOLD");
            env::remove_var("LIFECYCLE_ARCHIVAL_AGE_DAYS");
            env::remove_var("LIFECYCLE_DECAY_HALF_LIFE_DAYS");
        }

        assert!(config.enabled);
        assert_eq!(config.decay_interval_secs, 1800);
        assert_eq!(config.archival_interval_secs, 43200);
        assert_eq!(config.decay_confidence_threshold, 0.5);
        assert_eq!(config.archival_age_days, 60);
        assert_eq!(config.decay_half_life_days, 180.0);
    }
}
