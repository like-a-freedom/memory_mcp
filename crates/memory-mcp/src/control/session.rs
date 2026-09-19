//! Control Plane Session.
//!
//! Server-side session record with keyed cookie hash,
//! idle + absolute expiry. The raw cookie value is never
//! sent to the registry store.

use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlPlaneSession {
    pub id: String,
    /// Keyed hash of the raw cookie value.
    pub cookie_hash: String,
    pub account_id: String,
    /// The durable browser-auth policy epoch this session was created
    /// under.
    ///
    /// COMPATIBILITY BREAK (deliberate, documented): this field is
    /// additive but *required*. Sessions written before the epoch column
    /// existed decode as `None` and fail authentication, so every
    /// existing OIDC browser user must log in again once the deployment
    /// starts enforcing the epoch. Newly created sessions always carry
    /// `Some(current_epoch)`.
    pub browser_policy_epoch: Option<u64>,
    pub auth_time: DateTime<Utc>,
    pub idle_expiry: DateTime<Utc>,
    pub absolute_expiry: DateTime<Utc>,
}

impl ControlPlaneSession {
    /// Create a new session with 30-minute idle / 24-hour absolute expiry.
    ///
    /// `browser_policy_epoch` is the durable OIDC policy epoch joined at
    /// startup; the new session always records it.
    pub fn new(
        account: &Account,
        raw_cookie: &str,
        browser_policy_epoch: u64,
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
            browser_policy_epoch: Some(browser_policy_epoch),
            auth_time: now,
            idle_expiry: now + chrono::Duration::minutes(30),
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

/// Resolve and refresh a server-side session from a raw cookie value.
pub async fn resolve_session_record(
    state: &crate::http::HttpState,
    cookie_value: &str,
) -> Result<ControlPlaneSession, super::error::ApiError> {
    // The durable OIDC fence is joined at startup. A missing fence means
    // the session surface is not an OIDC deployment.
    let policy = state
        .browser_policy
        .as_ref()
        .ok_or(super::error::ApiError::Unauthorized)?;
    let cookie_hash = hex::encode(
        keyed_session_hash(
            &state.config.keys.control_plane_session,
            cookie_value.as_bytes(),
        )
        .map_err(super::error::ApiError::Internal)?,
    );
    let store = state.registry.store_clone();
    let session = store
        .find_session(policy, &cookie_hash)
        .await
        .map_err(super::error::ApiError::Internal)?
        .ok_or(super::error::ApiError::Unauthorized)?;
    // Defense in depth: the storage query already filters by epoch, but a
    // session that predates the epoch column (or carries a stale one) must
    // never authenticate.
    if session.browser_policy_epoch != Some(policy.epoch) {
        return Err(super::error::ApiError::Unauthorized);
    }
    let account = store
        .find_account_by_id(&session.account_id)
        .await
        .map_err(super::error::ApiError::Internal)?
        .ok_or(super::error::ApiError::Unauthorized)?;
    if account.status != crate::http::registry::models::AccountStatus::Active {
        return Err(super::error::ApiError::Unauthorized);
    }
    let now = Utc::now();
    if session.absolute_expiry <= now || session.idle_expiry <= now {
        return Err(super::error::ApiError::Unauthorized);
    }
    // The idle deadline is computed from database time inside the
    // conditional write; the row is never recreated when missing or
    // expired, so a stale cookie cannot resurrect a session.
    store
        .touch_session(policy, &session.id, &cookie_hash)
        .await
        .map_err(super::error::ApiError::Internal)?;
    Ok(session)
}

/// Resolve a session from a raw cookie value and return its Account.
pub async fn resolve_session(
    state: &crate::http::HttpState,
    cookie_value: &str,
) -> Result<Account, super::error::ApiError> {
    let session = resolve_session_record(state, cookie_value).await?;
    state
        .registry
        .store_clone()
        .find_account_by_id(&session.account_id)
        .await
        .map_err(super::error::ApiError::Internal)?
        .ok_or(super::error::ApiError::Unauthorized)
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
