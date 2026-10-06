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

/// `__Host-` requires `Path=/` (cookie spec), which on a shared host sends
/// the credential to every sibling service. Under a base path we scope the
/// cookie instead and must downgrade to `__Secure-`.
pub fn session_cookie_name(base_path: &str) -> &'static str {
    if base_path.is_empty() {
        "__Host-memory_mcp_session"
    } else {
        "__Secure-memory_mcp_session"
    }
}

pub fn session_cookie_path(base_path: &str) -> String {
    if base_path.is_empty() {
        "/".to_owned()
    } else {
        format!("{base_path}/")
    }
}

/// Build a Set-Cookie header value for the session.
pub fn build_session_cookie(cookie_value: String, cfg: &crate::http::config::HttpConfig) -> String {
    format!(
        "{}={cookie_value}; Path={}; Secure; HttpOnly; SameSite=Lax; Max-Age=86400",
        session_cookie_name(&cfg.base_path),
        session_cookie_path(&cfg.base_path),
    )
}

/// Build the Set-Cookie header value that clears the session cookie —
/// clearing must repeat the exact name and path used when setting.
pub fn clear_session_cookie(cfg: &crate::http::config::HttpConfig) -> String {
    format!(
        "{}=; Path={}; Secure; HttpOnly; SameSite=Lax; Max-Age=0",
        session_cookie_name(&cfg.base_path),
        session_cookie_path(&cfg.base_path),
    )
}

/// Accepts both names deliberately: the configured one is preferred, the
/// other keeps a session readable across a config change. Preference must
/// win over header order — a browser sends both during a migration.
pub fn parse_session_cookie<'a>(header_value: &'a str, base_path: &str) -> Option<&'a str> {
    let find = |name: &str| {
        header_value.split(';').find_map(|cookie| {
            let (cookie_name, value) = cookie.trim().split_once('=')?;
            (cookie_name == name).then_some(value)
        })
    };
    find(session_cookie_name(base_path))
        .or_else(|| find("__Host-memory_mcp_session"))
        .or_else(|| find("__Secure-memory_mcp_session"))
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
    let store = state.registry.sessions();
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
    let account = state
        .registry
        .accounts()
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
        .accounts()
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

#[cfg(test)]
mod cookie_tests {
    use super::*;

    #[test]
    fn root_deployments_keep_the_host_prefixed_cookie() {
        let cfg = crate::http::config::HttpConfig::default_for_test();
        assert_eq!(
            build_session_cookie("v".into(), &cfg),
            "__Host-memory_mcp_session=v; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=86400"
        );
    }

    #[test]
    fn a_base_path_scopes_the_cookie_and_drops_the_host_prefix() {
        let mut cfg = crate::http::config::HttpConfig::default_for_test();
        cfg.base_path = "/memory".into();
        let cookie = build_session_cookie("v".into(), &cfg);
        assert!(
            cookie.starts_with("__Secure-memory_mcp_session=v; Path=/memory/;"),
            "got: {cookie}"
        );
        assert!(!cookie.contains("__Host-"), "got: {cookie}");
    }

    #[test]
    fn clearing_uses_the_same_name_and_path_as_setting() {
        let mut cfg = crate::http::config::HttpConfig::default_for_test();
        cfg.base_path = "/memory".into();
        let clear = clear_session_cookie(&cfg);
        assert!(
            clear.starts_with("__Secure-memory_mcp_session=; Path=/memory/;"),
            "got: {clear}"
        );
        assert!(clear.contains("Max-Age=0"), "got: {clear}");
    }

    #[test]
    fn parsing_prefers_the_configured_name_and_accepts_the_other() {
        let hdr = "other=1; __Host-memory_mcp_session=abc; __Secure-memory_mcp_session=def";
        // Preferred name wins even though the other one appears first.
        assert_eq!(parse_session_cookie(hdr, ""), Some("abc"));
        assert_eq!(parse_session_cookie(hdr, "/memory"), Some("def"));
        // The non-configured name still parses (config changes never wedge).
        assert_eq!(
            parse_session_cookie("__Host-memory_mcp_session=abc", "/memory"),
            Some("abc")
        );
    }
}

/// Scenario tests for `resolve_session_record`: the ways a browser session ends.
/// Each drives the real store, so the observable behaviour — resolves, or is
/// refused — is what a signed-in user actually experiences.
#[cfg(test)]
mod resolve_session_tests {
    use super::*;
    use crate::http::registry::models::{Account, AccountStatus};

    /// A composed OIDC-shaped state with one Active Account.
    ///
    /// Composition reconciles the browser policy into the durable store (the
    /// production path), so a session minted under that fence is storable —
    /// which also means composition runs discovery, supplied here against a
    /// loopback provider.
    async fn state_with_account() -> (std::sync::Arc<crate::http::HttpState>, Account) {
        use crate::control::oidc::test_provider::{MockProvider, MockProviderConfig};

        let provider = MockProvider::spawn(MockProviderConfig::default()).await;
        let mut config = crate::http::config::HttpConfig::default_for_test();
        config.oidc_issuer = provider.base_url().to_string();
        let state = crate::http::test_state::HttpStateTestBuilder::new()
            .await
            .with_config(config)
            .without_prejoined_browser_policy()
            .build()
            .await
            .expect("composed state");
        let account = Account {
            id: "acct_session".into(),
            status: AccountStatus::Active,
            tenant_id: "ten_session".into(),
            created_at: Utc::now(),
            display_name: None,
        };
        state
            .registry
            .accounts()
            .write_account(&account)
            .await
            .expect("seed account");
        (state, account)
    }

    /// Mint and store a session for `account` under the deployment's fence.
    async fn store_session(
        state: &crate::http::HttpState,
        account: &Account,
        cookie: &str,
    ) -> ControlPlaneSession {
        let policy = state.browser_policy.as_ref().expect("oidc fence").clone();
        let session = ControlPlaneSession::new(account, cookie, policy.epoch, &state.config)
            .expect("session");
        state
            .registry
            .sessions()
            .store_session(&policy, &session)
            .await
            .expect("store session");
        session
    }

    #[tokio::test]
    async fn a_live_session_resolves() {
        let (state, account) = state_with_account().await;
        let cookie = "live-cookie";
        store_session(&state, &account, cookie).await;

        assert!(
            resolve_session_record(&state, cookie).await.is_ok(),
            "a live session must resolve"
        );
    }

    #[tokio::test]
    async fn a_session_for_a_suspended_account_does_not_resolve() {
        let (state, account) = state_with_account().await;
        let cookie = "suspended-cookie";
        store_session(&state, &account, cookie).await;
        // The Account is suspended after the session was minted, so the next
        // request must not authenticate even though the session row is live.
        state
            .registry
            .accounts()
            .transition_account_state(&account.id, AccountStatus::Active, AccountStatus::Suspended)
            .await
            .expect("suspend account");

        assert!(
            resolve_session_record(&state, cookie).await.is_err(),
            "a suspended Account must end its sessions"
        );
    }

    #[tokio::test]
    async fn an_expired_session_does_not_resolve() {
        let (state, account) = state_with_account().await;
        let cookie = "expired-cookie";
        let policy = state.browser_policy.as_ref().expect("oidc fence").clone();
        let mut session = ControlPlaneSession::new(&account, cookie, policy.epoch, &state.config)
            .expect("session");
        session.idle_expiry = Utc::now() - chrono::Duration::seconds(1);
        session.absolute_expiry = Utc::now() - chrono::Duration::seconds(1);
        state
            .registry
            .sessions()
            .store_session(&policy, &session)
            .await
            .expect("store session");

        assert!(
            resolve_session_record(&state, cookie).await.is_err(),
            "an expired session must not resolve"
        );
    }

    #[tokio::test]
    async fn an_unknown_cookie_does_not_resolve() {
        let (state, _account) = state_with_account().await;

        assert!(
            resolve_session_record(&state, "never-issued")
                .await
                .is_err(),
            "a cookie no session owns must not resolve"
        );
    }
}
