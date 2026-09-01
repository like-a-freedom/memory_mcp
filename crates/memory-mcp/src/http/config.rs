//! HTTP configuration loaded exclusively from environment (12-factor).

use std::net::SocketAddr;
use std::time::Duration;

use serde::{Deserialize, Deserializer};

use crate::error::MemoryError;
use crate::http::registry::models::PlanLimits;

pub const DEFAULT_BIND: &str = "0.0.0.0:8080";
pub const DEFAULT_BODY_LIMIT_BYTES: usize = 8 * 1024 * 1024; // 8 MiB
pub const DEFAULT_REQUEST_DEADLINE: Duration = Duration::from_secs(120);
pub const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(30);
pub const DEFAULT_ALLOWED_HOSTS: &[&str] = &[]; // must be set explicitly in production
pub const DEFAULT_ALLOWED_ORIGINS: &[&str] = &[];
pub const DEFAULT_OIDC_ALG: &str = "RS256";

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

pub use crate::config::SurrealTargetConfig;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SignupMode {
    InviteOnly,
    Open,
}

/// Project-private CIDR parser. Avoids the `cidr` crate dep for a single
/// use site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustedCidr {
    V4(std::net::Ipv4Addr, u8),
    V6(std::net::Ipv6Addr, u8),
}

impl TrustedCidr {
    pub fn parse(s: &str) -> Result<Self, MemoryError> {
        if let Some((addr, prefix)) = s.split_once('/') {
            let prefix: u8 = prefix
                .parse()
                .map_err(|_| MemoryError::ConfigInvalid("trusted proxy CIDR".into()))?;
            if let Ok(v4) = addr.parse::<std::net::Ipv4Addr>() {
                if prefix > 32 {
                    return Err(MemoryError::ConfigInvalid(format!(
                        "invalid IPv4 prefix length: {prefix}"
                    )));
                }
                return Ok(Self::V4(v4, prefix));
            }
            if let Ok(v6) = addr.parse::<std::net::Ipv6Addr>() {
                if prefix > 128 {
                    return Err(MemoryError::ConfigInvalid(format!(
                        "invalid IPv6 prefix length: {prefix}"
                    )));
                }
                return Ok(Self::V6(v6, prefix));
            }
        } else if let Ok(v4) = s.parse::<std::net::Ipv4Addr>() {
            return Ok(Self::V4(v4, 32));
        } else if let Ok(v6) = s.parse::<std::net::Ipv6Addr>() {
            return Ok(Self::V6(v6, 128));
        }
        Err(MemoryError::ConfigInvalid(format!(
            "invalid trusted proxy CIDR: {s}"
        )))
    }

    pub fn contains(&self, addr: std::net::IpAddr) -> bool {
        match (self, addr) {
            (Self::V4(net, prefix), std::net::IpAddr::V4(v4)) => {
                let shift = 32u32.saturating_sub(u32::from(*prefix));
                let mask = if shift == 32 { 0 } else { u32::MAX << shift };
                (u32::from(*net) & mask) == (u32::from(v4) & mask)
            }
            (Self::V6(net, prefix), std::net::IpAddr::V6(v6)) => {
                let shift = 128u32.saturating_sub(u32::from(*prefix));
                let mask = if shift == 128 { 0 } else { u128::MAX << shift };
                (u128::from(*net) & mask) == (u128::from(v6) & mask)
            }
            _ => false,
        }
    }
}

impl<'de> Deserialize<'de> for TrustedCidr {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

fn deserialize_duration_secs<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Duration, D::Error> {
    let secs = u64::deserialize(deserializer)?;
    Ok(Duration::from_secs(secs))
}

fn deserialize_hex_32<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 32], D::Error> {
    let raw = String::deserialize(deserializer)?;
    let bytes = hex::decode(raw.trim()).map_err(serde::de::Error::custom)?;
    bytes
        .try_into()
        .map_err(|_| serde::de::Error::custom("hex key must be exactly 32 bytes"))
}

fn require_env(k: &str) -> Result<String, MemoryError> {
    std::env::var(k).map_err(|_| MemoryError::ConfigMissing(k.into()))
}

fn optional_env(k: &str) -> Option<String> {
    std::env::var(k)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn parse_env_or<T>(k: &str, default: T) -> Result<T, MemoryError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match std::env::var(k) {
        Ok(v) => v
            .parse::<T>()
            .map_err(|_| MemoryError::ConfigInvalid(k.into())),
        Err(_) => Ok(default),
    }
}

fn parse_csv(k: &str) -> Result<Vec<String>, MemoryError> {
    match std::env::var(k) {
        Ok(v) => Ok(v
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()),
        Err(_) => Ok(Vec::new()),
    }
}

fn parse_bool(k: &str, default: bool) -> Result<bool, MemoryError> {
    match std::env::var(k) {
        Ok(v) => v
            .parse::<bool>()
            .map_err(|_| MemoryError::ConfigInvalid(k.into())),
        Err(_) => Ok(default),
    }
}

fn parse_hex_32_env(k: &str) -> Result<[u8; 32], MemoryError> {
    let raw = require_env(k)?;
    let bytes = hex::decode(&raw).map_err(|_| MemoryError::ConfigInvalid(k.into()))?;
    bytes
        .try_into()
        .map_err(|_| MemoryError::ConfigInvalid(k.into()))
}

fn parse_required_env<T>(key: &str) -> Result<T, MemoryError>
where
    T: std::str::FromStr,
{
    let raw = require_env(key)?;
    raw.parse::<T>()
        .map_err(|_| MemoryError::ConfigInvalid(key.into()))
}

fn load_signup_plan_limits() -> Result<Option<PlanLimits>, MemoryError> {
    const KEYS: &[&str] = &[
        "MEMORY_MCP_HTTP_MAX_INGESTED_BYTES",
        "MEMORY_MCP_HTTP_MAX_EPISODE_COUNT",
        "MEMORY_MCP_HTTP_INGEST_PER_MINUTE",
        "MEMORY_MCP_HTTP_MAX_OPEN_APP_SESSIONS",
        "MEMORY_MCP_HTTP_MAX_ACTIVE_API_KEYS",
        "MEMORY_MCP_HTTP_REQUEST_CONCURRENCY",
        "MEMORY_MCP_HTTP_EXTRACTION_CONCURRENCY",
    ];
    let any_set = KEYS.iter().any(|key| optional_env(key).is_some());
    if !any_set {
        return Ok(None);
    }
    Ok(Some(PlanLimits {
        max_ingested_bytes: parse_required_env(KEYS[0])?,
        max_episode_count: parse_required_env(KEYS[1])?,
        ingest_per_minute: parse_required_env(KEYS[2])?,
        max_open_app_sessions: parse_required_env(KEYS[3])?,
        max_active_api_keys: parse_required_env(KEYS[4])?,
        per_tenant_request_concurrency: parse_required_env(KEYS[5])?,
        extraction_concurrency: parse_required_env(KEYS[6])?,
    }))
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

        Ok(Self {
            bind,
            public_base_url,
            trusted_proxy_cidrs,
            allowed_hosts,
            allowed_origins,
            body_limit_bytes,
            request_deadline,
            shutdown_grace,
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
        })
    }

    pub fn validate(&self) -> Result<(), MemoryError> {
        // Reject the test-only bootstrap env var unless the
        // `test-fixtures` feature is enabled (Task 5.8).
        // The literal is intentional: a production build
        // cannot accidentally gain the bootstrap impl by
        // name resolution.
        #[cfg(not(feature = "test-fixtures"))]
        if std::env::var("MEMORY_MCP_HTTP_TEST_BOOTSTRAP")
            .ok()
            .is_some()
        {
            return Err(MemoryError::ConfigInvalid(
                "MEMORY_MCP_HTTP_TEST_BOOTSTRAP is only valid with the test-fixtures feature"
                    .into(),
            ));
        }
        // A test-fixtures binary must identify itself explicitly.
        // This prevents an accidentally released build compiled
        // with the fixture feature from silently selecting the
        // in-memory registry instead of a durable backend.
        #[cfg(all(feature = "test-fixtures", not(test)))]
        if std::env::var("MEMORY_MCP_HTTP_TEST_BOOTSTRAP")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .is_none()
        {
            return Err(MemoryError::ConfigInvalid(
                "test-fixtures HTTP builds require MEMORY_MCP_HTTP_TEST_BOOTSTRAP".into(),
            ));
        }
        if self.bind.ip().is_unspecified() && !self.public_base_url.contains("localhost") {
            eprintln!(
                "memory_mcp::http::config: binding to unspecified address; production must run behind a reverse proxy"
            );
        }
        if self.body_limit_bytes == 0 {
            return Err(MemoryError::ConfigInvalid(
                "MEMORY_MCP_HTTP_BODY_LIMIT must be positive".into(),
            ));
        }
        if self.request_deadline.is_zero() || self.shutdown_grace.is_zero() {
            return Err(MemoryError::ConfigInvalid(
                "HTTP request deadline and shutdown grace must be positive".into(),
            ));
        }
        if self.allowed_hosts.is_empty() {
            return Err(MemoryError::ConfigInvalid(
                "ALLOWED_HOSTS must be explicit in HTTP SaaS profile".into(),
            ));
        }
        if self.allowed_origins.is_empty() {
            return Err(MemoryError::ConfigInvalid(
                "ALLOWED_ORIGINS must be explicit in HTTP SaaS profile".into(),
            ));
        }
        if self.allowed_origins.iter().any(|o| o == "*") {
            return Err(MemoryError::ConfigInvalid(
                "wildcard ALLOWED_ORIGINS is rejected (spec §3.3)".into(),
            ));
        }
        if self.api_key_pepper.len() < 32 {
            return Err(MemoryError::ConfigInvalid(
                "MEMORY_MCP_API_KEY_PEPPER must be ≥32 bytes".into(),
            ));
        }
        if self.signup_mode == SignupMode::Open && !self.open_signup_quotas_set() {
            return Err(MemoryError::ConfigInvalid(
                "open signup requires explicit quota values (spec §12)".into(),
            ));
        }
        if let Some(limits) = &self.signup_plan_limits {
            if limits.max_ingested_bytes > i64::MAX as u64
                || limits.max_episode_count > i64::MAX as u64
            {
                return Err(MemoryError::ConfigInvalid(
                    "HTTP quota counters must fit SurrealDB signed integers".into(),
                ));
            }
            if limits.per_tenant_request_concurrency == 0 || limits.extraction_concurrency == 0 {
                return Err(MemoryError::ConfigInvalid(
                    "HTTP request and extraction concurrency limits must be positive".into(),
                ));
            }
        }
        if self.enable_control_plane
            && (self.oidc_issuer.is_empty()
                || self.oidc_client_id.is_empty()
                || self.oidc_audience.is_empty()
                || self.oidc_redirect_uri.is_empty())
        {
            return Err(MemoryError::ConfigInvalid(
                "control plane requires OIDC issuer, client id, audience, and redirect URI".into(),
            ));
        }
        if self.enable_control_plane
            && !matches!(self.oidc_allowed_alg.as_str(), "RS256" | "ES256" | "EdDSA")
        {
            return Err(MemoryError::ConfigInvalid(
                "OIDC allowed algorithm must be RS256, ES256, or EdDSA".into(),
            ));
        }
        if self.enable_control_plane_ui && !self.enable_control_plane {
            return Err(MemoryError::ConfigInvalid(
                "control-plane UI requires control plane to be enabled".into(),
            ));
        }
        if self.control_db.url == self.tenant_db.url
            && self.control_db.namespace == self.tenant_db.namespace
            && self.control_db.database == self.tenant_db.database
        {
            return Err(MemoryError::ConfigInvalid(
                "control and tenant storage must use different namespace/database bindings".into(),
            ));
        }
        #[cfg(not(any(test, feature = "test-fixtures")))]
        if self.control_db.url.starts_with("mem://") || self.tenant_db.url.starts_with("mem://") {
            return Err(MemoryError::ConfigInvalid(
                "mem:// is test-only; production HTTP SaaS requires remote SurrealDB or documented embedded RocksDB"
                    .into(),
            ));
        }
        #[cfg(not(feature = "control-plane"))]
        if self.enable_control_plane || self.enable_control_plane_ui {
            return Err(MemoryError::ConfigInvalid(
                "control-plane settings require the control-plane feature".into(),
            ));
        }
        #[cfg(not(feature = "control-plane-ui"))]
        if self.enable_control_plane_ui {
            return Err(MemoryError::ConfigInvalid(
                "control-plane UI requires the control-plane-ui feature".into(),
            ));
        }
        // fs-watch is the stdio-only ingestion path. The
        // HTTP SaaS profile must not enable it; a
        // deployment that sets the env var while running
        // the HTTP binary has misconfigured itself.
        if std::env::var("SURREALDB_FS_WATCH_INBOX").is_ok() {
            return Err(MemoryError::ConfigInvalid(
                "SURREALDB_FS_WATCH_INBOX must not be set in the HTTP SaaS profile".into(),
            ));
        }
        Ok(())
    }

    fn open_signup_quotas_set(&self) -> bool {
        self.signup_plan_limits.is_some()
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
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
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
            "MEMORY_MCP_HTTP_REQUEST_CONCURRENCY",
            "MEMORY_MCP_HTTP_EXTRACTION_CONCURRENCY",
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
    fn parses_ipv4_cidr() {
        let c = TrustedCidr::parse("10.0.0.0/8").unwrap();
        assert!(c.contains(IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3))));
        assert!(!c.contains(IpAddr::V4(Ipv4Addr::new(11, 1, 2, 3))));
    }

    #[test]
    fn parses_ipv6_cidr() {
        let c = TrustedCidr::parse("2001:db8::/32").unwrap();
        assert!(c.contains(IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1))));
        assert!(!c.contains(IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1))));
    }

    #[test]
    fn rejects_malformed() {
        assert!(TrustedCidr::parse("not-an-ip").is_err());
        assert!(TrustedCidr::parse("10.0.0.0/64").is_err());
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
            ("MEMORY_MCP_HTTP_REQUEST_CONCURRENCY", "6".into()),
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
