//! Default constants and environment-variable parsing helpers
//! for the HTTP configuration.
//!
//! Everything in this file is `pub(super)` so the [`HttpConfig`]
//! loader in `types.rs` can reach it; nothing here is part of the
//! public API. Constants are kept alongside their parsing helpers
//! so a new tunable adds a `DEFAULT_*` next to the line that reads
//! it.

use std::time::Duration;

use serde::{Deserialize, Deserializer};

use crate::error::MemoryError;
use crate::http::registry::models::PlanLimits;

pub const DEFAULT_BIND: &str = "0.0.0.0:8080";
pub const DEFAULT_BODY_LIMIT_BYTES: usize = 8 * 1024 * 1024; // 8 MiB
pub const DEFAULT_REQUEST_DEADLINE: Duration = Duration::from_secs(120);
pub const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(30);
pub const DEFAULT_POOL_CAP: usize = 32;
pub const DEFAULT_RUNTIME_IDLE_TTL: Duration = Duration::from_secs(15 * 60);
pub const DEFAULT_RUNTIME_CAPACITY_WAIT: Duration = Duration::from_secs(2);
pub const DEFAULT_RUNTIME_ACTIVATION_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_GLOBAL_REQUEST_LIMIT: u32 = 256;
pub const DEFAULT_SUBSCRIPTION_LIMIT: u32 = 32;
pub const DEFAULT_MAINTENANCE_PARALLELISM: usize = 4;
pub const DEFAULT_SUBSCRIPTION_QUEUE_CAPACITY: usize = 64;
pub const DEFAULT_SUBSCRIPTION_AUTH_RECHECK: Duration = Duration::from_secs(30);
pub const DEFAULT_TASK_RETENTION_SECS: u64 = 7 * 24 * 60 * 60;
pub const DEFAULT_TASK_QUEUE_CAPACITY: usize = 256;
pub const DEFAULT_TASK_SYNC_MAX_BYTES: usize = 1024 * 1024;
pub const DEFAULT_ALLOWED_HOSTS: &[&str] = &[]; // must be set explicitly in production
pub const DEFAULT_ALLOWED_ORIGINS: &[&str] = &[];
pub const DEFAULT_OIDC_ALG: &str = "RS256";

pub(super) fn require_env(k: &str) -> Result<String, MemoryError> {
    std::env::var(k).map_err(|_| MemoryError::ConfigMissing(k.into()))
}

pub(super) fn optional_env(k: &str) -> Option<String> {
    std::env::var(k)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

pub(super) fn parse_env_or<T>(k: &str, default: T) -> Result<T, MemoryError>
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

pub(super) fn parse_csv(k: &str) -> Result<Vec<String>, MemoryError> {
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

pub(super) fn parse_bool(k: &str, default: bool) -> Result<bool, MemoryError> {
    match std::env::var(k) {
        Ok(v) => v
            .parse::<bool>()
            .map_err(|_| MemoryError::ConfigInvalid(k.into())),
        Err(_) => Ok(default),
    }
}

pub(super) fn parse_hex_32_env(k: &str) -> Result<[u8; 32], MemoryError> {
    let raw = require_env(k)?;
    let bytes = hex::decode(&raw).map_err(|_| MemoryError::ConfigInvalid(k.into()))?;
    bytes
        .try_into()
        .map_err(|_| MemoryError::ConfigInvalid(k.into()))
}

pub(super) fn parse_required_env<T>(key: &str) -> Result<T, MemoryError>
where
    T: std::str::FromStr,
{
    let raw = require_env(key)?;
    raw.parse::<T>()
        .map_err(|_| MemoryError::ConfigInvalid(key.into()))
}

pub(super) fn deserialize_duration_secs<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Duration, D::Error> {
    let secs = u64::deserialize(deserializer)?;
    Ok(Duration::from_secs(secs))
}

pub(super) fn deserialize_hex_32<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<[u8; 32], D::Error> {
    let raw = String::deserialize(deserializer)?;
    let bytes = hex::decode(raw.trim()).map_err(serde::de::Error::custom)?;
    bytes
        .try_into()
        .map_err(|_| serde::de::Error::custom("hex key must be exactly 32 bytes"))
}

pub(super) fn load_signup_plan_limits() -> Result<Option<PlanLimits>, MemoryError> {
    const KEYS: &[&str] = &[
        "MEMORY_MCP_HTTP_MAX_INGESTED_BYTES",
        "MEMORY_MCP_HTTP_MAX_EPISODE_COUNT",
        "MEMORY_MCP_HTTP_INGEST_PER_MINUTE",
        "MEMORY_MCP_HTTP_MAX_OPEN_APP_SESSIONS",
        "MEMORY_MCP_HTTP_MAX_ACTIVE_API_KEYS",
        "MEMORY_MCP_HTTP_PER_TENANT_REQUEST_CONCURRENCY",
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
        assert!(c.contains(IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1))));
        assert!(!c.contains(IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1))));
    }

    #[test]
    fn rejects_malformed() {
        assert!(TrustedCidr::parse("not-an-ip").is_err());
        assert!(TrustedCidr::parse("10.0.0.0/64").is_err());
    }
}
