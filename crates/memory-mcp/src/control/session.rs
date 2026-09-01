//! Control Plane Session (Task 10.2, spec §5.3).
//!
//! Server-side session record with keyed cookie hash,
//! idle + absolute expiry. The raw cookie value is never
//! sent to the registry store.

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::error::MemoryError;
use crate::http::registry::models::Account;

/// Compute a keyed HMAC of the raw cookie value.
pub fn keyed_session_hash(key: &[u8; 32], raw: &[u8]) -> Result<[u8; 32], MemoryError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .map_err(|_| MemoryError::ConfigInvalid("invalid session HMAC key".into()))?;
    mac.update(raw);
    Ok(mac.finalize().into_bytes().into())
}

/// Generate a random 32-byte hex cookie value.
pub fn generate_session_cookie_value() -> String {
    hex::encode(rand::random::<[u8; 32]>())
}

/// Server-side session record.
#[derive(Debug, Clone)]
pub struct ControlPlaneSession {
    pub id: String,
    /// Keyed hash of the raw cookie value.
    pub cookie_hash: String,
    pub account_id: String,
    pub auth_time: DateTime<Utc>,
    pub idle_expiry: DateTime<Utc>,
    pub absolute_expiry: DateTime<Utc>,
}

impl ControlPlaneSession {
    /// Create a new session with 1h idle / 24h absolute expiry.
    pub fn new(
        account: &Account,
        raw_cookie: &str,
        cfg: &crate::http::config::HttpConfig,
    ) -> Result<Self, MemoryError> {
        let now = Utc::now();
        Ok(Self {
            id: uuid::Uuid::new_v4().to_string(),
            cookie_hash: hex::encode(keyed_session_hash(
                &cfg.keys.control_plane_session,
                raw_cookie.as_bytes(),
            )?),
            account_id: account.id.clone(),
            auth_time: now,
            idle_expiry: now + chrono::Duration::hours(1),
            absolute_expiry: now + chrono::Duration::hours(24),
        })
    }
}

/// Build a Set-Cookie header value for the session.
pub fn build_session_cookie(
    cookie_value: String,
    _cfg: &crate::http::config::HttpConfig,
) -> String {
    format!(
        "__Host-memory_mcp_session={cookie_value}; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=86400",
    )
}

/// Resolve a session from a raw cookie value.
pub async fn resolve_session(
    state: &crate::http::HttpState,
    cookie_value: &str,
) -> Result<Account, super::error::ApiError> {
    let cookie_hash = hex::encode(
        keyed_session_hash(
            &state.config.keys.control_plane_session,
            cookie_value.as_bytes(),
        )
        .map_err(super::error::ApiError::Internal)?,
    );
    let session = state
        .registry
        .store_clone()
        .find_session(&cookie_hash)
        .await
        .map_err(super::error::ApiError::Internal)?
        .ok_or(super::error::ApiError::Unauthorized)?;
    if session.absolute_expiry < Utc::now() {
        return Err(super::error::ApiError::Unauthorized);
    }
    if session.idle_expiry < Utc::now() {
        return Err(super::error::ApiError::Unauthorized);
    }
    let account = state
        .registry
        .store_clone()
        .find_account_by_id(&session.account_id)
        .await
        .map_err(super::error::ApiError::Internal)?
        .ok_or(super::error::ApiError::Unauthorized)?;
    Ok(account)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyed_session_hash_is_deterministic() {
        let key = [0xABu8; 32];
        let raw = b"test-cookie-value";
        let a = keyed_session_hash(&key, raw).unwrap();
        let b = keyed_session_hash(&key, raw).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn keyed_session_hash_differs_for_different_keys() {
        let key_a = [0xABu8; 32];
        let key_b = [0x99u8; 32];
        let raw = b"test-cookie-value";
        let a = keyed_session_hash(&key_a, raw).unwrap();
        let b = keyed_session_hash(&key_b, raw).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn generate_session_cookie_value_is_unique() {
        let a = generate_session_cookie_value();
        let b = generate_session_cookie_value();
        assert_ne!(a, b);
        assert_eq!(a.len(), 64);
    }
}
