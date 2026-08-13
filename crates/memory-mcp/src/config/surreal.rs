//! SurrealDB connection configuration.

use std::env;

use super::constants::*;
use super::embedding::EmbeddingConfig;
use super::helpers::{
    default_embedded_data_dir, is_remote_url, normalize_url_scheme, parse_bool_env, parse_env,
    resolve_embedded_data_dir,
};
use super::lifecycle::LifecycleConfig;
use super::ner::NerConfig;
use crate::service::MemoryError;

/// The single SurrealDB namespace selected for the lifetime of a server process.
///
/// Namespace selection is configuration, not request data. Once a value has been
/// parsed at the process boundary, ordinary operations cannot change it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ActiveNamespace(String);

impl ActiveNamespace {
    /// The zero-config namespace.
    pub const DEFAULT: &'static str = "main";

    /// Parses an environment or builder value.
    ///
    /// Empty values and comma-separated lists are rejected here. Other name
    /// validation remains the responsibility of SurrealDB, which keeps this
    /// adapter from inventing a database-specific grammar.
    pub fn new(value: impl AsRef<str>) -> Result<Self, MemoryError> {
        let value = value.as_ref().trim();
        if value.is_empty() {
            return Err(MemoryError::ConfigInvalid(
                "SURREALDB_NAMESPACE must not be empty; choose one namespace name".to_string(),
            ));
        }
        if value.contains(',') {
            return Err(MemoryError::ConfigInvalid(
                "SURREALDB_NAMESPACE accepts exactly one namespace; remove commas and choose one name"
                    .to_string(),
            ));
        }
        Ok(Self(value.to_string()))
    }

    /// Returns the namespace name for the bound storage session.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ActiveNamespace {
    fn default() -> Self {
        Self(Self::DEFAULT.to_string())
    }
}

impl std::fmt::Display for ActiveNamespace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StorageBackend {
    Embedded,
    Remote,
}

impl StorageBackend {
    pub(crate) const fn from_embedded(embedded: bool) -> Self {
        if embedded {
            Self::Embedded
        } else {
            Self::Remote
        }
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
    /// The one namespace used by this server process.
    pub namespace: ActiveNamespace,
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
    /// Environment variables whose values were supplied by zero-config defaults.
    pub(crate) defaulted_variables: Vec<&'static str>,
    /// Existing legacy executable-relative data directory selected for compatibility.
    pub(crate) legacy_data_dir: Option<String>,
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
    /// | `SURREALDB_DB_NAME` | No | Database name (default: `memory`) |
    /// | `SURREALDB_URL` | Yes (remote) | WebSocket URL |
    /// | `SURREALDB_EMBEDDED` | No | Set to `true` or `false`; inferred from URL when unset |
    /// | `SURREALDB_DATA_DIR` | No | Path to RocksDB data directory (user data directory by default) |
    /// | `SURREALDB_NAMESPACE` | No | One namespace (default: `main`) |
    /// | `SURREALDB_USERNAME` | Yes (remote) | Database username (`root` in embedded mode by default) |
    /// | `SURREALDB_PASSWORD` | Yes (remote) | Database password (`root` in embedded mode by default) |
    /// | `RUST_LOG` | No | Logging level (default: "info") |
    /// | `QUERY_LOGGING_ENABLED` | No | Set to "true" to persist `assemble_context` analytics (default: false) |
    /// | `QUERY_LOG_RETENTION_DAYS` | No | Days to retain persisted `query_log` analytics (default: 90) |
    ///
    /// Security note: embedded mode is the preferred local default. Remote mode
    /// should be paired with scoped credentials and host-level authentication.
    ///
    /// # Errors
    ///
    /// Returns `MemoryError::ConfigMissing` when remote mode lacks a URL or
    /// explicit credentials. Returns `MemoryError::ConfigInvalid` if the namespace
    /// is explicitly empty or contains a comma.
    pub fn from_env() -> Result<Self, MemoryError> {
        let raw_url = env::var("SURREALDB_URL").ok();
        let url = raw_url
            .as_deref()
            .map(normalize_url_scheme)
            .filter(|value| !value.is_empty());
        let embedded_was_explicit = env::var("SURREALDB_EMBEDDED").is_ok();
        let embedded = parse_bool_env("SURREALDB_EMBEDDED")
            .unwrap_or_else(|| !is_remote_url(raw_url.as_deref()));
        let db_name_was_explicit = env::var("SURREALDB_DB_NAME").is_ok();
        let db_name = env::var("SURREALDB_DB_NAME").unwrap_or_else(|_| "memory".into());
        if env::var_os("SURREALDB_NAMESPACES").is_some() {
            return Err(MemoryError::ConfigInvalid(
                "SURREALDB_NAMESPACES was removed; use SURREALDB_NAMESPACE with exactly one name"
                    .to_string(),
            ));
        }
        let namespace_was_explicit = env::var("SURREALDB_NAMESPACE").is_ok();
        let namespace = env::var("SURREALDB_NAMESPACE")
            .map(ActiveNamespace::new)
            .unwrap_or_else(|_| Ok(ActiveNamespace::default()))?;
        let username_was_explicit = env::var("SURREALDB_USERNAME").is_ok();
        let password_was_explicit = env::var("SURREALDB_PASSWORD").is_ok();
        let username = env::var("SURREALDB_USERNAME").unwrap_or_else(|_| "root".into());
        let password = env::var("SURREALDB_PASSWORD").unwrap_or_else(|_| "root".into());

        if !embedded && !is_remote_url(url.as_deref()) {
            return Err(MemoryError::ConfigMissing("SURREALDB_URL".to_string()));
        }
        if !embedded
            && (env::var("SURREALDB_USERNAME")
                .ok()
                .is_none_or(|value| value.trim().is_empty())
                || env::var("SURREALDB_PASSWORD")
                    .ok()
                    .is_none_or(|value| value.trim().is_empty()))
        {
            return Err(MemoryError::ConfigMissing(
                "SURREALDB_USERNAME and SURREALDB_PASSWORD are required for remote mode"
                    .to_string(),
            ));
        }

        let mut defaulted_variables = Vec::new();
        if !db_name_was_explicit {
            defaulted_variables.push("SURREALDB_DB_NAME");
        }
        if !namespace_was_explicit {
            defaulted_variables.push("SURREALDB_NAMESPACE");
        }
        if !embedded_was_explicit {
            defaulted_variables.push("SURREALDB_EMBEDDED");
        }
        if embedded && !username_was_explicit {
            defaulted_variables.push("SURREALDB_USERNAME");
        }
        if embedded && !password_was_explicit {
            defaulted_variables.push("SURREALDB_PASSWORD");
        }

        let (data_dir, legacy_data_dir) = if let Ok(explicit) = env::var("SURREALDB_DATA_DIR") {
            (Some(explicit), None)
        } else if embedded {
            let resolution = resolve_embedded_data_dir();
            defaulted_variables.push("SURREALDB_DATA_DIR");
            (Some(resolution.path), resolution.legacy_path)
        } else {
            (None, None)
        };

        let log_level = env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
        let query_logging_enabled = parse_bool_env("QUERY_LOGGING_ENABLED").unwrap_or(false);
        let query_log_retention_days = parse_env::<u32>("QUERY_LOG_RETENTION_DAYS")?
            .unwrap_or(DEFAULT_QUERY_LOG_RETENTION_DAYS);

        let lifecycle = LifecycleConfig::from_env();
        let embedding = EmbeddingConfig::from_env()?;
        let ner = NerConfig::from_env()?;

        Ok(Self {
            db_name,
            url,
            namespace,
            username,
            password,
            log_level,
            embedded,
            data_dir,
            defaulted_variables,
            legacy_data_dir,
            lifecycle,
            query_logging_enabled,
            query_log_retention_days,
            embedding,
            ner,
        })
    }

    /// Returns the active namespace selected for this process.
    #[must_use]
    pub fn active_namespace(&self) -> &ActiveNamespace {
        &self.namespace
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
///     .namespace("main")
///     .credentials("user", "pass")
///     .embedded(true)
///     .build();
/// ```
#[derive(Debug)]
pub struct SurrealConfigBuilder {
    db_name: Option<String>,
    url: Option<String>,
    namespace: Option<String>,
    username: Option<String>,
    password: Option<String>,
    log_level: String,
    embedded: bool,
    data_dir: Option<String>,
    defaulted_variables: Vec<&'static str>,
    legacy_data_dir: Option<String>,
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
            namespace: None,
            username: None,
            password: None,
            log_level: String::new(),
            embedded: false,
            data_dir: None,
            defaulted_variables: Vec::new(),
            legacy_data_dir: None,
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

    /// Sets the one namespace used by the configuration.
    pub fn namespace(mut self, namespace: impl Into<String>) -> Self {
        self.namespace = Some(namespace.into());
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
    /// Returns `MemoryError::ConfigInvalid` if the namespace is empty or invalid.
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

        let namespace = self
            .namespace
            .map(ActiveNamespace::new)
            .unwrap_or_else(|| Ok(ActiveNamespace::default()))?;

        Ok(SurrealConfig {
            db_name,
            url: self.url,
            namespace,
            username,
            password,
            log_level: self.log_level,
            embedded: self.embedded,
            data_dir: self.data_dir,
            defaulted_variables: self.defaulted_variables,
            legacy_data_dir: self.legacy_data_dir,
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

    use tempfile::TempDir;

    use super::super::env_lock;
    use super::*;

    const SURREAL_CONFIG_ENV_KEYS: &[&str] = &[
        "SURREALDB_URL",
        "SURREALDB_EMBEDDED",
        "SURREALDB_DB_NAME",
        "SURREALDB_NAMESPACE",
        "SURREALDB_NAMESPACES",
        "SURREALDB_USERNAME",
        "SURREALDB_PASSWORD",
        "SURREALDB_DATA_DIR",
        "RUST_LOG",
        "QUERY_LOGGING_ENABLED",
        "QUERY_LOG_RETENTION_DAYS",
        "LIFECYCLE_ENABLED",
        "LIFECYCLE_DECAY_INTERVAL_SECS",
        "LIFECYCLE_ARCHIVAL_INTERVAL_SECS",
        "LIFECYCLE_DECAY_THRESHOLD",
        "LIFECYCLE_ARCHIVAL_AGE_DAYS",
        "LIFECYCLE_DECAY_HALF_LIFE_DAYS",
        "EMBEDDINGS_ENABLED",
        "EMBEDDINGS_PROVIDER",
        "EMBEDDINGS_TIMEOUT_SECS",
        "SURREALDB_EMBEDDING_DIMENSION",
        "EMBEDDINGS_MAX_TOKENS",
        "EMBEDDINGS_SIMILARITY_THRESHOLD",
        "EMBEDDINGS_MODEL_DIR",
        "EMBEDDINGS_MODEL",
        "EMBEDDINGS_BASE_URL",
        "EMBEDDINGS_API_KEY",
        "NER_EXTRACTOR",
        "NER_CACHE_DIR",
        "NER_LABELS",
        "NER_THRESHOLD",
        "NER_MAX_CONCURRENCY",
        "NER_IDLE_UNLOAD_SECS",
        "GLINER_BATCH_SIZE",
        "GLINER_MAX_BATCH_TOKENS",
        "GLINER_DEVICE",
        "NER_PROVIDER",
        "NER_MODEL",
        "NER_MODEL_DIR",
        "NER_BATCH_SIZE",
        "NER_MAX_BATCH_TOKENS",
        "NER_DEVICE",
        "GLINER_IDLE_UNLOAD_SECS",
        "XDG_DATA_HOME",
        "HOME",
    ];

    struct EnvSnapshot {
        keys: &'static [&'static str],
        values: Vec<Option<String>>,
    }

    impl EnvSnapshot {
        fn capture(keys: &'static [&'static str]) -> Self {
            Self {
                keys,
                values: keys.iter().map(|key| env::var(key).ok()).collect(),
            }
        }
    }

    impl Drop for EnvSnapshot {
        fn drop(&mut self) {
            for (key, value) in self.keys.iter().zip(&self.values) {
                match value {
                    Some(value) => unsafe { env::set_var(key, value) },
                    None => unsafe { env::remove_var(key) },
                }
            }
        }
    }

    fn clear_surreal_environment() {
        for key in SURREAL_CONFIG_ENV_KEYS {
            unsafe { env::remove_var(key) };
        }
    }

    #[test]
    fn active_namespace_trims_surrounding_whitespace() {
        let namespace = ActiveNamespace::new("  main  ").expect("trimmed namespace is valid");

        assert_eq!(namespace.as_str(), "main");
    }

    #[test]
    fn active_namespace_rejects_empty_value() {
        let error = ActiveNamespace::new("").expect_err("empty namespace must be rejected");

        assert!(matches!(error, MemoryError::ConfigInvalid(message) if message.contains("empty")));
    }

    #[test]
    fn active_namespace_rejects_whitespace_only_value() {
        let error =
            ActiveNamespace::new(" \t\n ").expect_err("whitespace-only namespace must be rejected");

        assert!(matches!(error, MemoryError::ConfigInvalid(message) if message.contains("empty")));
    }

    #[test]
    fn active_namespace_rejects_comma_separated_values() {
        let error = ActiveNamespace::new("main,personal")
            .expect_err("comma-separated namespaces must be rejected");

        assert!(matches!(error, MemoryError::ConfigInvalid(message) if message.contains("comma")));
    }

    #[test]
    fn storage_backend_selection_follows_embedded_flag() {
        assert_eq!(
            StorageBackend::from_embedded(true),
            StorageBackend::Embedded
        );
        assert_eq!(StorageBackend::from_embedded(false), StorageBackend::Remote);
    }

    #[test]
    fn builder_sets_all_fields() {
        let config = SurrealConfigBuilder::new()
            .db_name("test_db")
            .url("ws://localhost:8000")
            .namespace("main")
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
        assert_eq!(config.active_namespace().as_str(), "main");
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
    fn builder_defaults_namespace_to_main() {
        let config = SurrealConfigBuilder::new()
            .db_name("test")
            .credentials("u", "p")
            .build()
            .expect("namespace should have a zero-config default");

        assert_eq!(config.active_namespace().as_str(), "main");
    }

    #[test]
    fn builder_sets_namespace() {
        let config = SurrealConfigBuilder::new()
            .db_name("test")
            .namespace("main")
            .credentials("u", "p")
            .build()
            .expect("valid config");
        assert_eq!(config.active_namespace().as_str(), "main");
    }

    #[test]
    fn data_dir_or_default_uses_default() {
        let _lock = env_lock().lock().expect("environment lock");
        let _snapshot = EnvSnapshot::capture(SURREAL_CONFIG_ENV_KEYS);
        clear_surreal_environment();
        let xdg_data_home = TempDir::new().expect("temporary XDG data directory");
        unsafe { env::set_var("XDG_DATA_HOME", xdg_data_home.path()) };
        let expected_path = xdg_data_home.path().join("memory_mcp");
        std::fs::create_dir_all(&expected_path).expect("create deterministic data directory");

        let config = SurrealConfigBuilder::new()
            .db_name("test")
            .namespace("test")
            .credentials("u", "p")
            .build()
            .expect("valid config");

        assert_eq!(
            config.data_dir_or_default(),
            expected_path.to_string_lossy()
        );
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
            .namespace("main")
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
        let _lock = env_lock().lock().expect("env lock");
        let _snapshot = EnvSnapshot::capture(SURREAL_CONFIG_ENV_KEYS);
        clear_surreal_environment();
        unsafe {
            env::set_var("SURREALDB_DB_NAME", "memory");
            env::set_var("SURREALDB_NAMESPACE", "main");
            env::set_var("SURREALDB_USERNAME", "root");
            env::set_var("SURREALDB_PASSWORD", "root");
            env::set_var("SURREALDB_EMBEDDED", "true");
            env::set_var("QUERY_LOGGING_ENABLED", "true");
            env::set_var("QUERY_LOG_RETENTION_DAYS", "14");
        }

        let config = SurrealConfig::from_env().expect("config from env");

        assert!(config.query_logging_enabled);
        assert_eq!(config.query_log_retention_days, 14);
    }

    #[test]
    fn from_env_applies_zero_config_embedded_defaults() {
        let _lock = env_lock().lock().expect("environment lock");
        let _snapshot = EnvSnapshot::capture(SURREAL_CONFIG_ENV_KEYS);
        clear_surreal_environment();
        unsafe { env::set_var("SURREALDB_DATA_DIR", "/tmp/memory-mcp-zero-config-test") };

        let config = SurrealConfig::from_env().expect("empty environment should be valid");

        assert_eq!(config.db_name, "memory");
        assert_eq!(config.active_namespace().as_str(), "main");
        assert_eq!(config.username, "root");
        assert_eq!(config.password, "root");
        assert!(config.embedded);
        assert_eq!(config.url, None);
        assert_eq!(
            config.data_dir_or_default(),
            "/tmp/memory-mcp-zero-config-test"
        );
        assert_eq!(
            config.defaulted_variables,
            vec![
                "SURREALDB_DB_NAME",
                "SURREALDB_NAMESPACE",
                "SURREALDB_EMBEDDED",
                "SURREALDB_USERNAME",
                "SURREALDB_PASSWORD",
            ]
        );
        assert!(config.legacy_data_dir.is_none());
    }

    #[test]
    fn ordinary_runtime_defaults_to_local_first_without_provider_selection() {
        let _lock = env_lock().lock().expect("environment lock");
        let _snapshot = EnvSnapshot::capture(SURREAL_CONFIG_ENV_KEYS);
        clear_surreal_environment();

        let config = SurrealConfig::from_env().expect("zero-config defaults");

        assert!(config.embedded);
        assert_eq!(config.db_name, "memory");
        assert_eq!(config.active_namespace().as_str(), "main");
        assert_eq!(config.username, "root");
        assert_eq!(config.password, "root");
        assert!(matches!(
            config.ner.extractor,
            super::super::ner::NerExtractorConfig::Anno
        ));
        assert_eq!(
            config.embedding.provider,
            super::super::embedding::EmbeddingProviderKind::Disabled
        );
    }

    #[test]
    fn ordinary_runtime_accepts_canonical_advanced_provider_overrides() {
        let _lock = env_lock().lock().expect("environment lock");
        let _snapshot = EnvSnapshot::capture(SURREAL_CONFIG_ENV_KEYS);
        clear_surreal_environment();
        unsafe {
            env::set_var("NER_EXTRACTOR", super::super::ner::SELECTOR_CLASSIC_GLINER);
            env::set_var("EMBEDDINGS_ENABLED", "true");
            env::set_var("EMBEDDINGS_PROVIDER", "local-candle");
        }

        let config = SurrealConfig::from_env().expect("canonical provider overrides");

        assert!(matches!(
            config.ner.extractor,
            super::super::ner::NerExtractorConfig::ClassicGliner(_)
        ));
        assert_eq!(
            config.embedding.provider,
            super::super::embedding::EmbeddingProviderKind::LocalCandle
        );
    }

    #[test]
    fn from_env_uses_new_user_data_path_and_records_all_storage_defaults() {
        let _lock = env_lock().lock().expect("environment lock");
        let _snapshot = EnvSnapshot::capture(SURREAL_CONFIG_ENV_KEYS);
        clear_surreal_environment();
        let xdg_data_home = TempDir::new().expect("temporary XDG data directory");
        unsafe { env::set_var("XDG_DATA_HOME", xdg_data_home.path()) };
        let expected_path = xdg_data_home.path().join("memory_mcp");
        std::fs::create_dir_all(&expected_path).expect("create new data directory");

        let config = SurrealConfig::from_env().expect("zero-config environment should be valid");

        assert_eq!(config.data_dir.as_deref(), expected_path.to_str());
        assert_eq!(
            config.defaulted_variables,
            vec![
                "SURREALDB_DB_NAME",
                "SURREALDB_NAMESPACE",
                "SURREALDB_EMBEDDED",
                "SURREALDB_USERNAME",
                "SURREALDB_PASSWORD",
                "SURREALDB_DATA_DIR",
            ]
        );
    }

    #[test]
    fn builder_created_config_has_no_default_provenance() {
        let config = SurrealConfigBuilder::new()
            .db_name("test")
            .namespace("main")
            .credentials("u", "p")
            .build()
            .expect("valid config");

        assert!(config.defaulted_variables.is_empty());
        assert!(config.legacy_data_dir.is_none());
    }

    #[test]
    fn from_env_rejects_remote_url_without_explicit_credentials() {
        let _lock = env_lock().lock().expect("environment lock");
        let _snapshot = EnvSnapshot::capture(SURREAL_CONFIG_ENV_KEYS);
        clear_surreal_environment();
        unsafe { env::set_var("SURREALDB_URL", "ws://localhost:8000") };

        let error = SurrealConfig::from_env().expect_err("remote credentials are required");

        assert!(
            matches!(error, MemoryError::ConfigMissing(message) if message.contains("USERNAME"))
        );
    }

    #[test]
    fn from_env_rejects_remote_url_with_empty_credentials() {
        let _lock = env_lock().lock().expect("environment lock");
        let _snapshot = EnvSnapshot::capture(SURREAL_CONFIG_ENV_KEYS);
        clear_surreal_environment();
        unsafe {
            env::set_var("SURREALDB_URL", "ws://localhost:8000");
            env::set_var("SURREALDB_USERNAME", " ");
            env::set_var("SURREALDB_PASSWORD", "secret");
        }

        let error = SurrealConfig::from_env().expect_err("remote credentials must be non-empty");

        assert!(
            matches!(error, MemoryError::ConfigMissing(message) if message.contains("USERNAME"))
        );
    }

    #[test]
    fn from_env_accepts_remote_url_with_explicit_credentials() {
        let _lock = env_lock().lock().expect("environment lock");
        let _snapshot = EnvSnapshot::capture(SURREAL_CONFIG_ENV_KEYS);
        clear_surreal_environment();
        unsafe {
            env::set_var("SURREALDB_URL", "  HTTPS://localhost:8000  ");
            env::set_var("SURREALDB_USERNAME", "memory_user");
            env::set_var("SURREALDB_PASSWORD", "secret");
        }

        let config =
            SurrealConfig::from_env().expect("explicit remote credentials should be valid");

        assert!(!config.embedded);
        assert_eq!(config.url.as_deref(), Some("https://localhost:8000"));
        assert_eq!(config.username, "memory_user");
        assert_eq!(config.password, "secret");
        assert_eq!(
            config.defaulted_variables,
            vec![
                "SURREALDB_DB_NAME",
                "SURREALDB_NAMESPACE",
                "SURREALDB_EMBEDDED",
            ]
        );
        assert!(config.data_dir.is_none());
    }

    #[test]
    fn from_env_rejects_legacy_plural_namespace_env_when_non_empty() {
        let _lock = env_lock().lock().expect("environment lock");
        let _snapshot = EnvSnapshot::capture(SURREAL_CONFIG_ENV_KEYS);
        clear_surreal_environment();
        unsafe { env::set_var("SURREALDB_NAMESPACES", "org,personal") };

        let error = SurrealConfig::from_env().expect_err("legacy plural env must be rejected");

        assert!(
            matches!(error, MemoryError::ConfigInvalid(message) if message.contains("SURREALDB_NAMESPACES"))
        );
    }

    #[test]
    fn from_env_rejects_legacy_plural_namespace_env_when_empty() {
        let _lock = env_lock().lock().expect("environment lock");
        let _snapshot = EnvSnapshot::capture(SURREAL_CONFIG_ENV_KEYS);
        clear_surreal_environment();
        unsafe { env::set_var("SURREALDB_NAMESPACES", "") };

        let error =
            SurrealConfig::from_env().expect_err("empty legacy plural env must be rejected");

        assert!(
            matches!(error, MemoryError::ConfigInvalid(message) if message.contains("SURREALDB_NAMESPACES"))
        );
    }

    #[test]
    fn from_env_rejects_legacy_plural_and_singular_namespace_env_together() {
        let _lock = env_lock().lock().expect("environment lock");
        let _snapshot = EnvSnapshot::capture(SURREAL_CONFIG_ENV_KEYS);
        clear_surreal_environment();
        unsafe {
            env::set_var("SURREALDB_NAMESPACES", "org");
            env::set_var("SURREALDB_NAMESPACE", "main");
        }

        let error = SurrealConfig::from_env().expect_err("both namespace envs must be rejected");

        assert!(
            matches!(error, MemoryError::ConfigInvalid(message) if message.contains("SURREALDB_NAMESPACES"))
        );
    }

    #[test]
    fn explicit_embedded_false_without_remote_url_is_invalid() {
        let _lock = env_lock().lock().expect("environment lock");
        let _snapshot = EnvSnapshot::capture(SURREAL_CONFIG_ENV_KEYS);
        clear_surreal_environment();
        unsafe { env::set_var("SURREALDB_EMBEDDED", "false") };

        let error = SurrealConfig::from_env().expect_err("remote mode needs a URL");

        assert!(
            matches!(error, MemoryError::ConfigMissing(message) if message.contains("SURREALDB_URL"))
        );
    }

    #[test]
    fn explicit_embedded_override_wins_over_remote_url() {
        let _lock = env_lock().lock().expect("environment lock");
        let _snapshot = EnvSnapshot::capture(SURREAL_CONFIG_ENV_KEYS);
        clear_surreal_environment();
        unsafe {
            env::set_var("SURREALDB_URL", "https://localhost:8000");
            env::set_var("SURREALDB_EMBEDDED", "true");
        }

        let config = SurrealConfig::from_env().expect("explicit embedded mode should win");

        assert!(config.embedded);
        assert_eq!(config.url.as_deref(), Some("https://localhost:8000"));
    }
}
