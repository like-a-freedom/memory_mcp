//! Local Unix-domain socket transport for the host bridge.
//!
//! When lifecycle integration is enabled, the running `memory_mcp` service
//! owns an authenticated local Unix-domain socket. The `memory-mcp-host-bridge`
//! executable reads one versioned host event from stdin and forwards it to
//! that socket.
//!
//! Security requirements:
//! - Unix socket permissions restrict the configured local user;
//! - adapter identity and version are validated;
//! - request size is bounded before JSON parsing;
//! - one event document per request;
//! - no public memory-operation selector;
//! - no caller-provided trust class;
//! - raw secrets are never written to bridge logs.

use std::path::PathBuf;

/// Maximum request size in bytes (256 KiB). Request size is bounded before
/// JSON parsing.
pub const MAX_REQUEST_BYTES: usize = 256 * 1024;

/// The default socket path when lifecycle integration is enabled.
pub const DEFAULT_SOCKET_PATH: &str = "/tmp/memory-mcp-lifecycle.sock";

/// Configuration for the local transport listener.
#[derive(Debug, Clone)]
pub struct TransportConfig {
    /// The Unix-domain socket path.
    pub socket_path: PathBuf,
    /// Maximum request size in bytes.
    pub max_request_bytes: usize,
    /// Whether the transport is enabled.
    pub enabled: bool,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            socket_path: PathBuf::from(DEFAULT_SOCKET_PATH),
            max_request_bytes: MAX_REQUEST_BYTES,
            enabled: false,
        }
    }
}

impl TransportConfig {
    /// Create a new transport config from environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        let enabled = std::env::var("LIFECYCLE_TRANSPORT_ENABLED")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        let socket_path = std::env::var("LIFECYCLE_TRANSPORT_SOCKET")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_SOCKET_PATH));
        let max_request_bytes = std::env::var("LIFECYCLE_TRANSPORT_MAX_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(MAX_REQUEST_BYTES);
        Self {
            socket_path,
            max_request_bytes,
            enabled,
        }
    }
}

/// Validates that a raw request payload is within the size bound.
///
/// Request size is bounded before JSON parsing to prevent unbounded
/// allocation.
#[must_use]
pub fn validate_request_size(payload: &[u8], max_bytes: usize) -> bool {
    payload.len() <= max_bytes
}

/// Validates that an adapter identity is recognized.
#[must_use]
pub fn validate_adapter_identity(adapter_id: &str, adapter_version: &str) -> bool {
    matches!(
        (adapter_id, adapter_version),
        ("claude_code", "1.0") | ("codex", "1.0")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_disabled() {
        let config = TransportConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.socket_path, PathBuf::from(DEFAULT_SOCKET_PATH));
        assert_eq!(config.max_request_bytes, MAX_REQUEST_BYTES);
    }

    #[test]
    fn validate_request_size_rejects_oversized() {
        let small = vec![0u8; 100];
        let large = vec![0u8; MAX_REQUEST_BYTES + 1];
        assert!(validate_request_size(&small, MAX_REQUEST_BYTES));
        assert!(!validate_request_size(&large, MAX_REQUEST_BYTES));
    }

    #[test]
    fn validate_adapter_identity_accepts_known_pairs() {
        assert!(validate_adapter_identity("claude_code", "1.0"));
        assert!(validate_adapter_identity("codex", "1.0"));
    }

    #[test]
    fn validate_adapter_identity_rejects_unknown() {
        assert!(!validate_adapter_identity("rogue", "1.0"));
        assert!(!validate_adapter_identity("claude_code", "99.0"));
    }
}
