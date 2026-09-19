//! Pre-auth and session CSRF for the local administrator surface.
//!
//! Two distinct token families share one secret (`csrf_key`) but are
//! domain-separated so a pre-auth token can never satisfy a session
//! mutation and vice versa:
//!
//! * **Pre-auth** — a short-lived (5 minute) signed cookie that carries
//!   no identity, plus an HMAC token derived from that cookie value.
//!   Issued by `GET /api/v1/auth/local/csrf` so that public POSTs
//!   (login, activation, reset, challenge inspection) can carry an
//!   `X-CSRF-Token` even before a session exists.
//! * **Session** — an HMAC over `(admin_id, session_id, policy epoch)`.
//!   Returned by `GET /api/v1/admin/session` and required on every
//!   authenticated mutation.
//!
//! Both bind the policy epoch so a rotated key or changed mode
//! invalidates every outstanding token. The pre-auth timestamp is
//! checked against server UTC with a bounded skew allowance; durable
//! session deadlines continue to use database time.
//!
//! Cookies are `__Host-` prefixed, so they are `Secure`, `HttpOnly`,
//! `SameSite=Strict`, `Path=/` and carry no `Domain` attribute. A
//! request presenting the same cookie name twice is rejected outright:
//! ambiguous values are never resolved by "first wins".

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::service::local_admin::contracts::{AdminFence, LocalAdminError, LocalResult};

type HmacSha256 = Hmac<Sha256>;

/// Cookie carrying the signed pre-auth nonce and issue timestamp.
pub const PREAUTH_COOKIE: &str = "__Host-memory_mcp_admin_preauth";

/// Cookie carrying the admin session verifier.
pub const SESSION_COOKIE: &str = "__Host-memory_mcp_admin";

/// Pre-auth material lifetime.
pub const PREAUTH_TTL_SECONDS: i64 = 300;

/// Maximum accepted clock skew, in seconds, in either direction.
/// Replicas must run synchronized clocks; a timestamp outside this
/// window fails closed rather than being clamped.
pub const PREAUTH_MAX_SKEW_SECONDS: i64 = 30;

const PREAUTH_SIGN_DOMAIN: &[u8] = b"local_admin_preauth_sign\0";
const PREAUTH_TOKEN_DOMAIN: &[u8] = b"local_admin_preauth_token\0";
const SESSION_TOKEN_DOMAIN: &[u8] = b"local_admin_session_token\0";

/// Issued pre-auth material: what to set as a cookie and what the
/// client must echo back in `X-CSRF-Token`.
#[derive(Debug)]
pub struct PreauthIssue {
    pub cookie: String,
    pub token: String,
}

/// Verified pre-auth material.
#[derive(Debug, PartialEq, Eq)]
pub struct PreauthVerified {
    /// The raw nonce, useful only for binding a rotated cookie.
    pub nonce: [u8; 32],
    /// Policy epoch the material was signed under.
    pub epoch: u64,
}

fn hmac(key: &[u8; 32]) -> LocalResult<HmacSha256> {
    HmacSha256::new_from_slice(key)
        .map_err(|_| LocalAdminError::InvalidInput("invalid CSRF key".into()))
}

fn hex_32(value: &[u8; 32]) -> String {
    hex::encode(value)
}

/// Issue fresh pre-auth material bound to `epoch`.
///
/// `now_unix` is passed in so callers can inject a deterministic clock
/// in tests; production passes `Utc::now().timestamp()`.
pub fn issue_preauth(key: &[u8; 32], epoch: u64, now_unix: i64) -> LocalResult<PreauthIssue> {
    let nonce = random_32();
    let cookie_value = sign_preauth(key, &nonce, epoch, now_unix)?;
    let token = preauth_token(key, &cookie_value)?;
    Ok(PreauthIssue {
        cookie: format!("{PREAUTH_COOKIE}={cookie_value}"),
        token,
    })
}

fn sign_preauth(
    key: &[u8; 32],
    nonce: &[u8; 32],
    epoch: u64,
    now_unix: i64,
) -> LocalResult<String> {
    let mut mac = hmac(key)?;
    mac.update(PREAUTH_SIGN_DOMAIN);
    mac.update(nonce);
    mac.update(b"\0");
    mac.update(epoch.to_be_bytes().as_slice());
    mac.update(b"\0");
    mac.update(now_unix.to_be_bytes().as_slice());
    let signature = mac.finalize().into_bytes();
    Ok(format!(
        "{}.{}.{}",
        hex_32(nonce),
        now_unix,
        hex::encode(signature)
    ))
}

/// Derive the token a client echoes in `X-CSRF-Token` from the cookie
/// value it holds.
pub fn preauth_token(key: &[u8; 32], cookie_value: &str) -> LocalResult<String> {
    let mut mac = hmac(key)?;
    mac.update(PREAUTH_TOKEN_DOMAIN);
    mac.update(cookie_value.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// Verify a presented pre-auth cookie value and token.
///
/// Fails closed on a malformed value, an out-of-range timestamp, an
/// epoch mismatch, a bad signature, or a token that does not match the
/// cookie it claims to accompany.
pub fn verify_preauth(
    key: &[u8; 32],
    cookie_value: &str,
    token: &str,
    expected_epoch: u64,
    now_unix: i64,
) -> LocalResult<PreauthVerified> {
    let mut parts = cookie_value.split('.');
    let (Some(nonce_hex), Some(issued_at), Some(_signature_hex), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(LocalAdminError::InvalidChallenge);
    };
    let nonce: [u8; 32] = hex::decode(nonce_hex)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(LocalAdminError::InvalidChallenge)?;
    let issued_at: i64 = issued_at
        .parse()
        .map_err(|_| LocalAdminError::InvalidChallenge)?;

    // Future or out-of-range timestamps fail closed. The window is
    // symmetric so a replica running slightly ahead is still usable,
    // but never beyond the reviewed skew bound.
    if issued_at > now_unix + PREAUTH_MAX_SKEW_SECONDS {
        return Err(LocalAdminError::InvalidChallenge);
    }
    if now_unix - issued_at > PREAUTH_TTL_SECONDS + PREAUTH_MAX_SKEW_SECONDS {
        return Err(LocalAdminError::InvalidChallenge);
    }

    // The signature is verified by recomputing the whole cookie value,
    // which also binds the epoch and the issue timestamp.
    let expected_cookie = sign_preauth(key, &nonce, expected_epoch, issued_at)?;
    if !constant_time_eq(expected_cookie.as_bytes(), cookie_value.as_bytes()) {
        return Err(LocalAdminError::InvalidChallenge);
    }

    let expected_token = preauth_token(key, cookie_value)?;
    if !constant_time_eq(expected_token.as_bytes(), token.as_bytes()) {
        return Err(LocalAdminError::InvalidChallenge);
    }

    Ok(PreauthVerified {
        nonce,
        epoch: expected_epoch,
    })
}

/// Compute the session CSRF token bound to an admin fence.
pub fn session_token(key: &[u8; 32], fence: &AdminFence) -> LocalResult<String> {
    let mut mac = hmac(key)?;
    mac.update(SESSION_TOKEN_DOMAIN);
    mac.update(fence.admin_id.as_bytes());
    mac.update(b"\0");
    mac.update(fence.session_id.as_bytes());
    mac.update(b"\0");
    mac.update(fence.policy.epoch.to_be_bytes().as_slice());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// Verify a session CSRF token against the presented fence.
pub fn verify_session_token(key: &[u8; 32], fence: &AdminFence, token: &str) -> LocalResult<()> {
    let expected = session_token(key, fence)?;
    if constant_time_eq(expected.as_bytes(), token.as_bytes()) {
        Ok(())
    } else {
        Err(LocalAdminError::Forbidden)
    }
}

/// Parse a single cookie value out of a `Cookie` header.
///
/// Returns `Err` when the name appears more than once so an attacker
/// cannot smuggle a second value past a "first wins" parser.
pub fn parse_cookie(header: &str, name: &str) -> LocalResult<Option<String>> {
    let mut found: Option<String> = None;
    for part in header.split(';') {
        let part = part.trim();
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        if key == name {
            if found.is_some() {
                return Err(LocalAdminError::Forbidden);
            }
            found = Some(value.to_string());
        }
    }
    Ok(found)
}

fn random_32() -> [u8; 32] {
    use rand_core::RngCore;
    let mut buf = [0u8; 32];
    rand_core::OsRng.fill_bytes(&mut buf);
    buf
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Exact-match Origin validation.
///
/// The generic OIDC middleware tolerates an absent `Origin`; local
/// routes must not. Every local POST/DELETE requires a present,
/// unambiguous `Origin` that exactly matches the configured allowlist.
pub fn require_allowed_origin(
    headers: &axum::http::HeaderMap,
    allowed_origins: &[String],
) -> LocalResult<()> {
    let mut values = headers.get_all(axum::http::header::ORIGIN).iter();
    let Some(first) = values.next() else {
        return Err(LocalAdminError::Forbidden);
    };
    if values.next().is_some() {
        // Multiple Origin headers are ambiguous; fail closed.
        return Err(LocalAdminError::Forbidden);
    }
    let Ok(origin) = first.to_str() else {
        return Err(LocalAdminError::Forbidden);
    };
    if origin.contains(',') {
        return Err(LocalAdminError::Forbidden);
    }
    if allowed_origins.iter().any(|allowed| allowed == origin) {
        Ok(())
    } else {
        Err(LocalAdminError::Forbidden)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [7u8; 32];

    #[test]
    fn preauth_roundtrip() {
        let issued = issue_preauth(&KEY, 3, 1_000).expect("issue");
        let cookie_value = issued
            .cookie
            .strip_prefix("__Host-memory_mcp_admin_preauth=")
            .expect("cookie prefix");
        let verified = verify_preauth(&KEY, cookie_value, &issued.token, 3, 1_000).expect("verify");
        assert_eq!(verified.epoch, 3);
    }

    #[test]
    fn preauth_rejects_expired() {
        let issued = issue_preauth(&KEY, 3, 1_000).expect("issue");
        let cookie_value = issued.cookie.split_once('=').expect("kv").1;
        assert!(verify_preauth(&KEY, cookie_value, &issued.token, 3, 9_999).is_err());
    }

    #[test]
    fn preauth_rejects_future_timestamp() {
        let issued = issue_preauth(&KEY, 3, 10_000).expect("issue");
        let cookie_value = issued.cookie.split_once('=').expect("kv").1;
        assert!(verify_preauth(&KEY, cookie_value, &issued.token, 3, 1_000).is_err());
    }

    #[test]
    fn preauth_rejects_wrong_epoch() {
        let issued = issue_preauth(&KEY, 3, 1_000).expect("issue");
        let cookie_value = issued.cookie.split_once('=').expect("kv").1;
        assert!(verify_preauth(&KEY, cookie_value, &issued.token, 4, 1_000).is_err());
    }

    #[test]
    fn preauth_rejects_wrong_token() {
        let issued = issue_preauth(&KEY, 3, 1_000).expect("issue");
        let other = issue_preauth(&KEY, 3, 1_000).expect("issue");
        let cookie_value = issued.cookie.split_once('=').expect("kv").1;
        assert!(verify_preauth(&KEY, cookie_value, &other.token, 3, 1_000).is_err());
    }

    #[test]
    fn duplicate_cookie_is_rejected() {
        let header = format!("{PREAUTH_COOKIE}=a; {PREAUTH_COOKIE}=b");
        assert!(parse_cookie(&header, PREAUTH_COOKIE).is_err());
    }

    #[test]
    fn origin_must_be_present_and_exact() {
        let allowed = vec!["https://localhost:8443".to_string()];

        let mut headers = axum::http::HeaderMap::new();
        assert!(require_allowed_origin(&headers, &allowed).is_err());

        headers.insert(
            axum::http::header::ORIGIN,
            "https://localhost:8443".parse().expect("origin header"),
        );
        assert!(require_allowed_origin(&headers, &allowed).is_ok());

        let mut other = axum::http::HeaderMap::new();
        other.insert(
            axum::http::header::ORIGIN,
            "https://evil.example".parse().expect("origin header"),
        );
        assert!(require_allowed_origin(&other, &allowed).is_err());

        let mut comma = axum::http::HeaderMap::new();
        comma.insert(
            axum::http::header::ORIGIN,
            "https://localhost:8443, https://evil.example"
                .parse()
                .expect("origin header"),
        );
        assert!(require_allowed_origin(&comma, &allowed).is_err());
    }

    #[test]
    fn session_token_binds_epoch_and_session() {
        use crate::http::config::BrowserAuthMode;
        use crate::service::local_admin::contracts::BrowserPolicyFence;

        let fence = AdminFence {
            admin_id: "adm1".into(),
            session_id: "ses1".into(),
            credential_generation: 1,
            policy: BrowserPolicyFence {
                mode: BrowserAuthMode::Local,
                epoch: 2,
            },
        };
        let token = session_token(&KEY, &fence).expect("token");
        assert!(verify_session_token(&KEY, &fence, &token).is_ok());

        let mut other = fence.clone();
        other.policy.epoch = 3;
        assert!(verify_session_token(&KEY, &other, &token).is_err());

        let mut third = fence.clone();
        third.session_id = "ses2".into();
        assert!(verify_session_token(&KEY, &third, &token).is_err());
    }
}
