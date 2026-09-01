//! HTTP configuration loaded exclusively from environment (12-factor).

use std::net::SocketAddr;
use std::time::Duration;

use serde::{Deserialize, Deserializer};

use crate::error::MemoryError;

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
    pub signup_mode: SignupMode,
    pub enable_control_plane: bool,
    pub enable_control_plane_ui: bool,
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
        let oidc_issuer = optional_env("MEMORY_MCP_HTTP_OIDC_ISSUER").unwrap_or_default();
        let oidc_client_id = optional_env("MEMORY_MCP_HTTP_OIDC_CLIENT_ID").unwrap_or_default();
        let oidc_audience = optional_env("MEMORY_MCP_HTTP_OIDC_AUDIENCE").unwrap_or_default();
        let oidc_redirect_uri =
            optional_env("MEMORY_MCP_HTTP_OIDC_REDIRECT_URI").unwrap_or_default();
        let oidc_allowed_alg = optional_env("MEMORY_MCP_HTTP_OIDC_ALLOWED_ALG")
            .unwrap_or_else(|| DEFAULT_OIDC_ALG.into());

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
            signup_mode,
            enable_control_plane,
            enable_control_plane_ui,
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
        if self.bind.ip().is_unspecified() && !self.public_base_url.contains("localhost") {
            eprintln!(
                "memory_mcp::http::config: binding to unspecified address; production must run behind a reverse proxy"
            );
        }
        if self.allowed_hosts.is_empty() {
            return Err(MemoryError::ConfigInvalid(
                "ALLOWED_HOSTS must be explicit in HTTP SaaS profile".into(),
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
        false // Phase 6 will replace
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
impl HttpConfig {
    pub fn default_for_test() -> Self {
        Self {
            bind: "127.0.0.1:0".parse().expect("test bind"),
            public_base_url: "http://localhost".into(),
            trusted_proxy_cidrs: Vec::new(),
            allowed_hosts: vec!["localhost".into(), "127.0.0.1".into()],
            allowed_origins: vec!["http://localhost".into()],
            body_limit_bytes: DEFAULT_BODY_LIMIT_BYTES,
            request_deadline: DEFAULT_REQUEST_DEADLINE,
            shutdown_grace: DEFAULT_SHUTDOWN_GRACE,
            control_db: SurrealTargetConfig::default_for_test(),
            tenant_db: SurrealTargetConfig::default_for_test(),
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
            signup_mode: SignupMode::InviteOnly,
            enable_control_plane: false,
            enable_control_plane_ui: false,
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
