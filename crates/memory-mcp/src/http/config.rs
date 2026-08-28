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
    #[serde(deserialize_with = "deserialize_hex_32")]
    pub identity_index_key: [u8; 32],
    #[serde(deserialize_with = "deserialize_hex_32")]
    pub control_plane_session_key: [u8; 32],
    #[serde(deserialize_with = "deserialize_hex_32")]
    pub oidc_state_key: [u8; 32],
    #[serde(deserialize_with = "deserialize_hex_32")]
    pub oidc_nonce_key: [u8; 32],
    #[serde(deserialize_with = "deserialize_hex_32")]
    pub csrf_key: [u8; 32],
    pub oidc_issuer: String,
    pub oidc_client_id: String,
    pub oidc_audience: String,
    pub oidc_redirect_uri: String,
    pub oidc_allowed_alg: String,
    pub signup_mode: SignupMode,
    pub enable_control_plane: bool,
    pub enable_control_plane_ui: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SurrealTargetConfig {
    pub url: String,
    pub username: String,
    pub password: String,
    pub database: String,
    pub namespace: String, // separate for control vs. tenant
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SignupMode {
    InviteOnly,
    Open,
}

/// Project-private CIDR parser. Avoids the `cidr` crate dep for a single
/// use site (Task 3.1 plan: "Add `cidr` tiny parser (no extra dep)").
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

impl HttpConfig {
    pub fn validate(&self) -> Result<(), MemoryError> {
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
            identity_index_key: [0; 32],
            control_plane_session_key: [0; 32],
            oidc_state_key: [0; 32],
            oidc_nonce_key: [0; 32],
            csrf_key: [0; 32],
            oidc_issuer: "https://issuer.invalid".into(),
            oidc_client_id: "test-client".into(),
            oidc_audience: "memory-mcp".into(),
            oidc_redirect_uri: "http://localhost/auth/oidc/callback".into(),
            oidc_allowed_alg: "RS256".into(),
            signup_mode: SignupMode::InviteOnly,
            enable_control_plane: false,
            enable_control_plane_ui: false,
        }
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
impl SurrealTargetConfig {
    pub fn default_for_test() -> Self {
        Self {
            url: "mem://".into(),
            username: "root".into(),
            password: "root".into(),
            database: "memory_test".into(),
            namespace: "test".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn parses_ipv4_cidr() {
        let c = TrustedCidr::parse("10.0.0.0/8").unwrap();
        assert!(c.contains(IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3))));
        assert!(!c.contains(IpAddr::V4(Ipv4Addr::new(11, 1, 2, 3))));
    }

    #[test]
    fn parses_ipv6_cidr() {
        let c = TrustedCidr::parse("2001:db8::/32").unwrap();
        assert!(c.contains(IpAddr::V6(Ipv6Addr::new(
            0x2001, 0xdb8, 0, 0, 0, 0, 0, 1
        ))));
        assert!(!c.contains(IpAddr::V6(Ipv6Addr::new(
            0xfe80, 0, 0, 0, 0, 0, 0, 1
        ))));
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
}
