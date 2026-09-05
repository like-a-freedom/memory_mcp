//! [`HttpConfig`] type, [`HmacKeys`] secrets bundle, [`SignupMode`]
//! enum, the env loader, and the test fixtures.
//!
//! The validator lives in `validate.rs`; everything else (struct
//! shape, env loading, defaults) is here.

use std::net::SocketAddr;
use std::time::Duration;

use serde::Deserialize;

use crate::error::MemoryError;
use crate::http::registry::models::PlanLimits;

use super::parse::{
    DEFAULT_BIND, DEFAULT_BODY_LIMIT_BYTES, DEFAULT_GLOBAL_REQUEST_LIMIT,
    DEFAULT_MAINTENANCE_PARALLELISM, DEFAULT_OIDC_ALG, DEFAULT_POOL_CAP, DEFAULT_REQUEST_DEADLINE,
    DEFAULT_RUNTIME_ACTIVATION_TIMEOUT, DEFAULT_RUNTIME_CAPACITY_WAIT, DEFAULT_RUNTIME_IDLE_TTL,
    DEFAULT_SHUTDOWN_GRACE, DEFAULT_SUBSCRIPTION_AUTH_RECHECK, DEFAULT_SUBSCRIPTION_LIMIT,
    DEFAULT_SUBSCRIPTION_QUEUE_CAPACITY, DEFAULT_TASK_QUEUE_CAPACITY, DEFAULT_TASK_RETENTION_SECS,
    DEFAULT_TASK_SYNC_MAX_BYTES, TrustedCidr, deserialize_duration_secs, deserialize_hex_32,
    load_signup_plan_limits, optional_env, parse_bool, parse_csv, parse_env_or, parse_hex_32_env,
    require_env,
};
use super::validate::validate;
pub use crate::config::SurrealTargetConfig;

#[derive(Debug, Clone, Deserialize)]
pub struct HttpConfig {
    pub bind: SocketAddr,
    pub public_base_url: String,
    pub trusted_proxy_cidrs: Vec<TrustedCidr>,
    pub allowed_hosts: Vec<String>,
    pub allowed_origins: Vec<String>,
    pub body_limit_bytes: usize,
    #[serde(deserialize_with = "deserialize_duration_secs")]
    pub request_deadline: Duration,
    #[serde(deserialize_with = "deserialize_duration_secs")]
    pub shutdown_grace: Duration,
    pub pool_cap: usize,
    #[serde(deserialize_with = "deserialize_duration_secs")]
    pub runtime_idle_ttl: Duration,
    #[serde(deserialize_with = "deserialize_duration_secs")]
    pub runtime_capacity_wait: Duration,
    #[serde(deserialize_with = "deserialize_duration_secs")]
    pub runtime_activation_timeout: Duration,
    pub global_request_limit: u32,
    pub subscription_limit: u32,
    pub maintenance_parallelism: usize,
    pub subscription_queue_capacity: usize,
    #[serde(deserialize_with = "deserialize_duration_secs")]
    pub subscription_auth_recheck: Duration,
    pub task_retention_secs: u64,
    pub task_queue_capacity: usize,
    pub task_sync_max_bytes: usize,
    pub control_db: SurrealTargetConfig,
    pub tenant_db: SurrealTargetConfig,
    pub api_key_pepper: String,
    pub keys: HmacKeys,
    pub oidc_issuer: String,
    pub oidc_client_id: String,
    pub oidc_audience: String,
    pub oidc_redirect_uri: String,
    pub oidc_allowed_alg: String,
    /// Immutable operator allowlist entries encoded as `issuer|subject_verifier`
    /// where the verifier is the hex blind index, never the raw OIDC subject.
    pub operator_identity_allowlist: Vec<String>,
    pub signup_mode: SignupMode,
    pub enable_control_plane: bool,
    pub enable_control_plane_ui: bool,
    /// Explicit plan values for open signup. The plan is persisted in the
    /// durable Registry at startup and is never read from request input.
    #[serde(skip)]
    pub signup_plan_limits: Option<PlanLimits>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct HmacKeys {
    #[serde(deserialize_with = "deserialize_hex_32")]
    pub identity_index: [u8; 32],
    #[serde(deserialize_with = "deserialize_hex_32")]
    pub control_plane_session: [u8; 32],
    #[serde(deserialize_with = "deserialize_hex_32")]
    pub oidc_state: [u8; 32],
    #[serde(deserialize_with = "deserialize_hex_32")]
    pub oidc_nonce: [u8; 32],
    #[serde(deserialize_with = "deserialize_hex_32")]
    pub csrf: [u8; 32],
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SignupMode {
    InviteOnly,
    Open,
}

impl HttpConfig {
    /// Loads the HTTP config from process environment variables.
    pub fn from_env() -> Result<Self, MemoryError> {
        let default_bind: SocketAddr = DEFAULT_BIND
            .parse()
            .map_err(|e| MemoryError::ConfigInvalid(format!("DEFAULT_BIND parse failed: {e}")))?;
        let bind = parse_env_or("MEMORY_MCP_HTTP_BIND", default_bind)?;
        let public_base_url = require_env("MEMORY_MCP_HTTP_PUBLIC_BASE_URL")?;
        let allowed_hosts = parse_csv("ALLOWED_HOSTS")?;
        let allowed_origins = parse_csv("ALLOWED_ORIGINS")?;
        let body_limit_bytes: usize =
            parse_env_or("MEMORY_MCP_HTTP_BODY_LIMIT", DEFAULT_BODY_LIMIT_BYTES)?;
        let request_deadline = Duration::from_secs(parse_env_or(
            "MEMORY_MCP_HTTP_REQUEST_DEADLINE_SECS",
            DEFAULT_REQUEST_DEADLINE.as_secs(),
        )?);
        let shutdown_grace = Duration::from_secs(parse_env_or(
            "MEMORY_MCP_HTTP_SHUTDOWN_GRACE_SECS",
            DEFAULT_SHUTDOWN_GRACE.as_secs(),
        )?);
        let pool_cap = parse_env_or("MEMORY_MCP_HTTP_POOL_CAP", DEFAULT_POOL_CAP)?;
        let runtime_idle_ttl = Duration::from_secs(parse_env_or(
            "MEMORY_MCP_HTTP_RUNTIME_IDLE_TTL_SECS",
            DEFAULT_RUNTIME_IDLE_TTL.as_secs(),
        )?);
        let runtime_capacity_wait = Duration::from_millis(parse_env_or(
            "MEMORY_MCP_HTTP_RUNTIME_CAPACITY_WAIT_MS",
            DEFAULT_RUNTIME_CAPACITY_WAIT.as_millis() as u64,
        )?);
        let runtime_activation_timeout = Duration::from_secs(parse_env_or(
            "MEMORY_MCP_HTTP_RUNTIME_ACTIVATION_TIMEOUT_SECS",
            DEFAULT_RUNTIME_ACTIVATION_TIMEOUT.as_secs(),
        )?);
        let global_request_limit = parse_env_or(
            "MEMORY_MCP_HTTP_GLOBAL_REQUEST_LIMIT",
            DEFAULT_GLOBAL_REQUEST_LIMIT,
        )?;
        let subscription_limit = parse_env_or(
            "MEMORY_MCP_HTTP_SUBSCRIPTION_LIMIT",
            DEFAULT_SUBSCRIPTION_LIMIT,
        )?;
        let maintenance_parallelism = parse_env_or(
            "MEMORY_MCP_HTTP_MAINTENANCE_PARALLELISM",
            DEFAULT_MAINTENANCE_PARALLELISM,
        )?;
        let subscription_queue_capacity = parse_env_or(
            "MEMORY_MCP_HTTP_SUBSCRIPTION_QUEUE_CAPACITY",
            DEFAULT_SUBSCRIPTION_QUEUE_CAPACITY,
        )?;
        let subscription_auth_recheck = Duration::from_secs(parse_env_or(
            "MEMORY_MCP_HTTP_SUBSCRIPTION_AUTH_RECHECK_SECS",
            DEFAULT_SUBSCRIPTION_AUTH_RECHECK.as_secs(),
        )?);
        let task_retention_secs = parse_env_or(
            "MEMORY_MCP_HTTP_TASK_RETENTION_SECS",
            DEFAULT_TASK_RETENTION_SECS,
        )?;
        let task_queue_capacity = parse_env_or(
            "MEMORY_MCP_HTTP_TASK_QUEUE_CAPACITY",
            DEFAULT_TASK_QUEUE_CAPACITY,
        )?;
        let task_sync_max_bytes = parse_env_or(
            "MEMORY_MCP_HTTP_TASK_SYNC_MAX_BYTES",
            DEFAULT_TASK_SYNC_MAX_BYTES,
        )?;
        let trusted_proxy_cidrs = parse_csv("MEMORY_MCP_HTTP_TRUSTED_PROXY_CIDRS")?
            .into_iter()
            .map(|s| TrustedCidr::parse(&s))
            .collect::<Result<Vec<_>, _>>()?;
        let api_key_pepper = require_env("MEMORY_MCP_API_KEY_PEPPER")?;
        let keys = HmacKeys {
            identity_index: parse_hex_32_env("MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY")?,
            control_plane_session: parse_hex_32_env("MEMORY_MCP_HTTP_SESSION_KEY")?,
            oidc_state: parse_hex_32_env("MEMORY_MCP_HTTP_OIDC_STATE_KEY")?,
            oidc_nonce: parse_hex_32_env("MEMORY_MCP_HTTP_OIDC_NONCE_KEY")?,
            csrf: parse_hex_32_env("MEMORY_MCP_HTTP_CSRF_KEY")?,
        };
        let signup_mode = match require_env("MEMORY_MCP_HTTP_SIGNUP_MODE")?.as_str() {
            "invite_only" => SignupMode::InviteOnly,
            "open" => SignupMode::Open,
            other => {
                return Err(MemoryError::ConfigInvalid(format!("signup mode: {other}")));
            }
        };
        let enable_control_plane = parse_bool("MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE", false)?;
        let enable_control_plane_ui = parse_bool("MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI", false)?;
        let signup_plan_limits = load_signup_plan_limits()?;
        let oidc_issuer = optional_env("MEMORY_MCP_HTTP_OIDC_ISSUER").unwrap_or_default();
        let oidc_client_id = optional_env("MEMORY_MCP_HTTP_OIDC_CLIENT_ID").unwrap_or_default();
        let oidc_audience = optional_env("MEMORY_MCP_HTTP_OIDC_AUDIENCE").unwrap_or_default();
        let oidc_redirect_uri =
            optional_env("MEMORY_MCP_HTTP_OIDC_REDIRECT_URI").unwrap_or_default();
        let oidc_allowed_alg = optional_env("MEMORY_MCP_HTTP_OIDC_ALLOWED_ALG")
            .unwrap_or_else(|| DEFAULT_OIDC_ALG.into());
        let operator_identity_allowlist = parse_csv("MEMORY_MCP_HTTP_OPERATOR_IDENTITIES")?;

        let control_db = SurrealTargetConfig {
            url: require_env("SURREALDB_CONTROL_URL")?,
            username: require_env("SURREALDB_CONTROL_USERNAME")?,
            password: require_env("SURREALDB_CONTROL_PASSWORD")?,
            database: require_env("SURREALDB_CONTROL_DB")?,
            namespace: require_env("SURREALDB_CONTROL_NAMESPACE")?,
        };
        let tenant_db = SurrealTargetConfig {
            url: require_env("SURREALDB_TENANT_URL")?,
            username: require_env("SURREALDB_TENANT_USERNAME")?,
            password: require_env("SURREALDB_TENANT_PASSWORD")?,
            database: require_env("SURREALDB_TENANT_DB")?,
            namespace: require_env("SURREALDB_TENANT_NAMESPACE")?,
        };

        let cfg = Self {
            bind,
            public_base_url,
            trusted_proxy_cidrs,
            allowed_hosts,
            allowed_origins,
            body_limit_bytes,
            request_deadline,
            shutdown_grace,
            pool_cap,
            runtime_idle_ttl,
            runtime_capacity_wait,
            runtime_activation_timeout,
            global_request_limit,
            subscription_limit,
            maintenance_parallelism,
            subscription_queue_capacity,
            subscription_auth_recheck,
            task_retention_secs,
            task_queue_capacity,
            task_sync_max_bytes,
            control_db,
            tenant_db,
            api_key_pepper,
            keys,
            oidc_issuer,
            oidc_client_id,
            oidc_audience,
            oidc_redirect_uri,
            oidc_allowed_alg,
            operator_identity_allowlist,
            signup_mode,
            enable_control_plane,
            enable_control_plane_ui,
            signup_plan_limits,
        };
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), MemoryError> {
        validate(self)
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
impl HttpConfig {
    pub fn default_for_test() -> Self {
        let control_db = SurrealTargetConfig::default_for_test();
        let mut tenant_db = SurrealTargetConfig::default_for_test();
        tenant_db.database = "memory_tenant_test".into();
        tenant_db.namespace = "tenant_test".into();
        Self {
            bind: "127.0.0.1:0".parse().expect("test bind"),
            public_base_url: "http://localhost".into(),
            trusted_proxy_cidrs: Vec::new(),
            allowed_hosts: vec!["localhost".into(), "127.0.0.1".into()],
            allowed_origins: vec!["http://localhost".into()],
            body_limit_bytes: DEFAULT_BODY_LIMIT_BYTES,
            request_deadline: DEFAULT_REQUEST_DEADLINE,
            shutdown_grace: DEFAULT_SHUTDOWN_GRACE,
            pool_cap: DEFAULT_POOL_CAP,
            runtime_idle_ttl: DEFAULT_RUNTIME_IDLE_TTL,
            runtime_capacity_wait: DEFAULT_RUNTIME_CAPACITY_WAIT,
            runtime_activation_timeout: DEFAULT_RUNTIME_ACTIVATION_TIMEOUT,
            global_request_limit: DEFAULT_GLOBAL_REQUEST_LIMIT,
            subscription_limit: DEFAULT_SUBSCRIPTION_LIMIT,
            maintenance_parallelism: DEFAULT_MAINTENANCE_PARALLELISM,
            subscription_queue_capacity: DEFAULT_SUBSCRIPTION_QUEUE_CAPACITY,
            subscription_auth_recheck: DEFAULT_SUBSCRIPTION_AUTH_RECHECK,
            task_retention_secs: DEFAULT_TASK_RETENTION_SECS,
            task_queue_capacity: DEFAULT_TASK_QUEUE_CAPACITY,
            task_sync_max_bytes: DEFAULT_TASK_SYNC_MAX_BYTES,
            control_db,
            tenant_db,
            api_key_pepper: "x".repeat(40),
            keys: HmacKeys {
                identity_index: [0; 32],
                control_plane_session: [0; 32],
                oidc_state: [0; 32],
                oidc_nonce: [0; 32],
                csrf: [0; 32],
            },
            oidc_issuer: "https://issuer.invalid".into(),
            oidc_client_id: "test-client".into(),
            oidc_audience: "memory-mcp".into(),
            oidc_redirect_uri: "http://localhost/auth/oidc/callback".into(),
            oidc_allowed_alg: DEFAULT_OIDC_ALG.into(),
            operator_identity_allowlist: Vec::new(),
            signup_mode: SignupMode::InviteOnly,
            enable_control_plane: false,
            enable_control_plane_ui: false,
            signup_plan_limits: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::Mutex;

    // Edition 2024: set_var/remove_var are unsafe. ENV_LOCK serializes all
    // env-mutating tests in this module, which is the safety condition.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_env<F: FnOnce()>(vars: &[(&str, &str)], f: F) {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for k in [
            "MEMORY_MCP_HTTP_BIND",
            "MEMORY_MCP_HTTP_PUBLIC_BASE_URL",
            "ALLOWED_HOSTS",
            "ALLOWED_ORIGINS",
            "MEMORY_MCP_API_KEY_PEPPER",
            "MEMORY_MCP_HTTP_SIGNUP_MODE",
            "MEMORY_MCP_HTTP_BODY_LIMIT",
            "MEMORY_MCP_HTTP_REQUEST_DEADLINE_SECS",
            "MEMORY_MCP_HTTP_SHUTDOWN_GRACE_SECS",
            "MEMORY_MCP_HTTP_TRUSTED_PROXY_CIDRS",
            "SURREALDB_CONTROL_URL",
            "SURREALDB_CONTROL_USERNAME",
            "SURREALDB_CONTROL_PASSWORD",
            "SURREALDB_CONTROL_DB",
            "SURREALDB_CONTROL_NAMESPACE",
            "SURREALDB_TENANT_URL",
            "SURREALDB_TENANT_USERNAME",
            "SURREALDB_TENANT_PASSWORD",
            "SURREALDB_TENANT_DB",
            "SURREALDB_TENANT_NAMESPACE",
            "MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE",
            "MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI",
            "MEMORY_MCP_HTTP_CSRF_KEY",
            "MEMORY_MCP_HTTP_OIDC_STATE_KEY",
            "MEMORY_MCP_HTTP_OIDC_NONCE_KEY",
            "MEMORY_MCP_HTTP_SESSION_KEY",
            "MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY",
            "MEMORY_MCP_HTTP_OPERATOR_IDENTITIES",
            "MEMORY_MCP_HTTP_MAX_INGESTED_BYTES",
            "MEMORY_MCP_HTTP_MAX_EPISODE_COUNT",
            "MEMORY_MCP_HTTP_INGEST_PER_MINUTE",
            "MEMORY_MCP_HTTP_MAX_OPEN_APP_SESSIONS",
            "MEMORY_MCP_HTTP_MAX_ACTIVE_API_KEYS",
            "MEMORY_MCP_HTTP_PER_TENANT_REQUEST_CONCURRENCY",
            "MEMORY_MCP_HTTP_EXTRACTION_CONCURRENCY",
            "MEMORY_MCP_HTTP_POOL_CAP",
            "MEMORY_MCP_HTTP_RUNTIME_IDLE_TTL_SECS",
            "MEMORY_MCP_HTTP_RUNTIME_CAPACITY_WAIT_MS",
            "MEMORY_MCP_HTTP_RUNTIME_ACTIVATION_TIMEOUT_SECS",
            "MEMORY_MCP_HTTP_GLOBAL_REQUEST_LIMIT",
            "MEMORY_MCP_HTTP_SUBSCRIPTION_LIMIT",
            "MEMORY_MCP_HTTP_MAINTENANCE_PARALLELISM",
            "MEMORY_MCP_HTTP_SUBSCRIPTION_QUEUE_CAPACITY",
            "MEMORY_MCP_HTTP_SUBSCRIPTION_AUTH_RECHECK_SECS",
            "MEMORY_MCP_HTTP_TASK_RETENTION_SECS",
            "MEMORY_MCP_HTTP_TASK_QUEUE_CAPACITY",
            "MEMORY_MCP_HTTP_TASK_SYNC_MAX_BYTES",
        ] {
            // SAFETY: serialized by ENV_LOCK; no other thread reads these vars in tests.
            unsafe {
                env::remove_var(k);
            }
        }
        for (k, v) in vars {
            // SAFETY: same as above.
            unsafe {
                env::set_var(k, v);
            }
        }
        f();
        for (k, _) in vars {
            // SAFETY: same as above.
            unsafe {
                env::remove_var(k);
            }
        }
    }

    fn base_required_env() -> Vec<(&'static str, String)> {
        let pepper = "x".repeat(40);
        let key = "0".repeat(64);
        vec![
            ("MEMORY_MCP_HTTP_BIND", "127.0.0.1:8080".into()),
            ("MEMORY_MCP_HTTP_PUBLIC_BASE_URL", "http://localhost".into()),
            ("ALLOWED_HOSTS", "localhost".into()),
            ("ALLOWED_ORIGINS", "http://localhost".into()),
            ("MEMORY_MCP_API_KEY_PEPPER", pepper),
            ("MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY", key.clone()),
            ("MEMORY_MCP_HTTP_SIGNUP_MODE", "invite_only".into()),
            ("MEMORY_MCP_HTTP_CSRF_KEY", key.clone()),
            ("MEMORY_MCP_HTTP_OIDC_STATE_KEY", key.clone()),
            ("MEMORY_MCP_HTTP_OIDC_NONCE_KEY", key.clone()),
            ("MEMORY_MCP_HTTP_SESSION_KEY", key),
            ("SURREALDB_CONTROL_URL", "ws://localhost:8000".into()),
            ("SURREALDB_CONTROL_USERNAME", "root".into()),
            ("SURREALDB_CONTROL_PASSWORD", "root".into()),
            ("SURREALDB_CONTROL_DB", "control".into()),
            ("SURREALDB_CONTROL_NAMESPACE", "control".into()),
            ("SURREALDB_TENANT_URL", "ws://localhost:8000".into()),
            ("SURREALDB_TENANT_USERNAME", "root".into()),
            ("SURREALDB_TENANT_PASSWORD", "root".into()),
            ("SURREALDB_TENANT_DB", "tenant".into()),
            ("SURREALDB_TENANT_NAMESPACE", "tenant".into()),
            ("MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE", "false".into()),
            ("MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI", "false".into()),
        ]
    }

    #[test]
    fn default_for_test_validates() {
        HttpConfig::default_for_test().validate().expect("valid");
    }

    #[test]
    fn http_config_loads_from_env_with_minimum_required() {
        let vars = base_required_env();
        let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        with_env(&refs, || {
            let cfg = HttpConfig::from_env().expect("config loads");
            cfg.validate().expect("valid");
            assert_eq!(cfg.bind.port(), 8080);
            assert_eq!(cfg.allowed_hosts, vec!["localhost".to_string()]);
            assert_eq!(cfg.signup_mode, SignupMode::InviteOnly);
        });
    }

    #[test]
    fn open_signup_loads_explicit_plan_limits() {
        let mut vars = base_required_env();
        vars[6] = ("MEMORY_MCP_HTTP_SIGNUP_MODE", "open".into());
        vars.extend([
            ("MEMORY_MCP_HTTP_MAX_INGESTED_BYTES", "1000".into()),
            ("MEMORY_MCP_HTTP_MAX_EPISODE_COUNT", "10".into()),
            ("MEMORY_MCP_HTTP_INGEST_PER_MINUTE", "3".into()),
            ("MEMORY_MCP_HTTP_MAX_OPEN_APP_SESSIONS", "8".into()),
            ("MEMORY_MCP_HTTP_MAX_ACTIVE_API_KEYS", "2".into()),
            ("MEMORY_MCP_HTTP_PER_TENANT_REQUEST_CONCURRENCY", "6".into()),
            ("MEMORY_MCP_HTTP_EXTRACTION_CONCURRENCY", "4".into()),
        ]);
        let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        with_env(&refs, || {
            let cfg = HttpConfig::from_env().expect("config loads");
            cfg.validate()
                .expect("open signup with explicit quotas is valid");
            let limits = cfg.signup_plan_limits.expect("plan limits");
            assert_eq!(limits.max_ingested_bytes, 1000);
            assert_eq!(limits.ingest_per_minute, 3);
            assert_eq!(limits.extraction_concurrency, 4);
        });
    }

    #[test]
    fn http_config_loads_operational_limits() {
        let mut vars = base_required_env();
        vars.extend([
            ("MEMORY_MCP_HTTP_POOL_CAP", "8".into()),
            ("MEMORY_MCP_HTTP_RUNTIME_IDLE_TTL_SECS", "60".into()),
            ("MEMORY_MCP_HTTP_RUNTIME_CAPACITY_WAIT_MS", "250".into()),
            (
                "MEMORY_MCP_HTTP_RUNTIME_ACTIVATION_TIMEOUT_SECS",
                "10".into(),
            ),
            ("MEMORY_MCP_HTTP_GLOBAL_REQUEST_LIMIT", "20".into()),
            ("MEMORY_MCP_HTTP_SUBSCRIPTION_LIMIT", "3".into()),
            ("MEMORY_MCP_HTTP_MAINTENANCE_PARALLELISM", "2".into()),
            ("MEMORY_MCP_HTTP_SUBSCRIPTION_QUEUE_CAPACITY", "16".into()),
            (
                "MEMORY_MCP_HTTP_SUBSCRIPTION_AUTH_RECHECK_SECS",
                "30".into(),
            ),
            ("MEMORY_MCP_HTTP_TASK_RETENTION_SECS", "3600".into()),
            ("MEMORY_MCP_HTTP_TASK_QUEUE_CAPACITY", "64".into()),
            ("MEMORY_MCP_HTTP_TASK_SYNC_MAX_BYTES", "4096".into()),
        ]);
        let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        with_env(&refs, || {
            let cfg = HttpConfig::from_env().expect("config loads");
            cfg.validate().expect("operational limits are valid");
            assert_eq!(cfg.pool_cap, 8);
            assert_eq!(cfg.runtime_capacity_wait, Duration::from_millis(250));
            assert_eq!(cfg.global_request_limit, 20);
            assert_eq!(cfg.subscription_limit, 3);
            assert_eq!(cfg.subscription_queue_capacity, 16);
            assert_eq!(cfg.task_retention_secs, 3600);
            assert_eq!(cfg.task_queue_capacity, 64);
            assert_eq!(cfg.task_sync_max_bytes, 4096);
        });
    }

    #[test]
    fn http_config_rejects_wildcard_origin() {
        let mut vars = base_required_env();
        vars[3] = ("ALLOWED_ORIGINS", "*".into());
        let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        with_env(&refs, || {
            let cfg = HttpConfig::from_env().expect("parses");
            assert!(matches!(cfg.validate(), Err(MemoryError::ConfigInvalid(_))));
        });
    }

    #[test]
    fn http_config_rejects_empty_origin_allowlist() {
        let mut cfg = HttpConfig::default_for_test();
        cfg.allowed_origins.clear();
        assert!(matches!(
            cfg.validate(),
            Err(MemoryError::ConfigInvalid(message)) if message.contains("ALLOWED_ORIGINS")
        ));
    }

    #[test]
    fn http_config_rejects_zero_limits() {
        let mut cfg = HttpConfig::default_for_test();
        cfg.body_limit_bytes = 0;
        assert!(matches!(
            cfg.validate(),
            Err(MemoryError::ConfigInvalid(message)) if message.contains("BODY_LIMIT")
        ));
        let mut cfg = HttpConfig::default_for_test();
        cfg.request_deadline = std::time::Duration::ZERO;
        assert!(matches!(
            cfg.validate(),
            Err(MemoryError::ConfigInvalid(message)) if message.contains("deadline")
        ));
    }

    #[test]
    fn http_config_rejects_shared_control_and_tenant_binding() {
        let mut cfg = HttpConfig::default_for_test();
        cfg.tenant_db = cfg.control_db.clone();
        assert!(matches!(
            cfg.validate(),
            Err(MemoryError::ConfigInvalid(message)) if message.contains("different namespace/database")
        ));
    }

    #[test]
    fn control_plane_requires_complete_oidc_config() {
        let mut cfg = HttpConfig::default_for_test();
        cfg.enable_control_plane = true;
        cfg.oidc_issuer.clear();
        assert!(matches!(
            cfg.validate(),
            Err(MemoryError::ConfigInvalid(message)) if message.contains("OIDC issuer")
        ));
    }

    #[test]
    fn control_plane_rejects_unknown_oidc_algorithm() {
        let mut cfg = HttpConfig::default_for_test();
        cfg.enable_control_plane = true;
        cfg.oidc_allowed_alg = "none".into();
        assert!(matches!(
            cfg.validate(),
            Err(MemoryError::ConfigInvalid(message)) if message.contains("allowed algorithm")
        ));
    }

    #[test]
    fn control_plane_ui_requires_control_plane() {
        let mut cfg = HttpConfig::default_for_test();
        cfg.enable_control_plane_ui = true;
        assert!(matches!(
            cfg.validate(),
            Err(MemoryError::ConfigInvalid(message)) if message.contains("UI requires control plane")
        ));
    }

    #[test]
    fn csv_allowlists_trim_entries() {
        let mut vars = base_required_env();
        vars[2] = ("ALLOWED_HOSTS", " localhost , 127.0.0.1 ".into());
        vars[3] = ("ALLOWED_ORIGINS", " http://localhost ".into());
        let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        with_env(&refs, || {
            let cfg = HttpConfig::from_env().expect("parses");
            assert_eq!(
                cfg.allowed_hosts,
                vec!["localhost".to_string(), "127.0.0.1".to_string()]
            );
            assert_eq!(cfg.allowed_origins, vec!["http://localhost".to_string()]);
        });
    }

    #[test]
    fn rejects_fs_watch_env_in_http_mode() {
        let mut vars = base_required_env();
        vars.push(("SURREALDB_FS_WATCH_INBOX", "/tmp/inbox".to_string()));
        let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        with_env(&refs, || {
            let cfg = HttpConfig::default_for_test();
            assert!(matches!(
                cfg.validate(),
                Err(MemoryError::ConfigInvalid(ref msg)) if msg.contains("SURREALDB_FS_WATCH_INBOX")
            ));
        });
    }

    #[test]
    fn rejects_open_signup_without_quotas() {
        // open_signup_quotas_set() currently returns false
        // because the per-tenant plan table is not yet
        // wired. Until it is, Open signup must be rejected
        // at startup.
        let mut cfg = HttpConfig::default_for_test();
        cfg.signup_mode = SignupMode::Open;
        assert!(matches!(cfg.validate(), Err(MemoryError::ConfigInvalid(_))));
    }
}
