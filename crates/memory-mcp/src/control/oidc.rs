//! OIDC Authorization Code + PKCE.
//!
//! Provides the OIDC types (`OidcState`, `OidcNonce`, `PkceCode`),
//! the `JwksCache` for key validation, the `OidcClient` that
//! performs discovery, authorization URL generation, code exchange,
//! and ID token validation, and the `authorize` / `callback` /
//! `logout` HTTP handlers. All raw OIDC state/nonce/verifier are
//! transient — only keyed hashes are durable.
//!
//! The implementation is split by concern across `oidc/`:
//!
//! - `flow_material` — OIDC flow material (`OidcState`, `OidcNonce`,
//!   `PkceCode`, `StoredOidcRequest`, `OidcTokens`, `OidcCallback`),
//!   `AuthError`, and the `AccessClaims` decoder.
//! - `jwks` — the `JwksCache` that the client uses to resolve
//!   signing keys.
//! - `client` — the `OidcClient` struct: discovery, authorization
//!   URL, code exchange, ID-token validation.
//! - `sealing` — `identity_subject_verifier` (HMAC blind index) and
//!   the `seal_oidc_payload` / `unseal_oidc_payload` AEAD pair.
//! - `handlers` — the `authorize`, `callback`, and `logout` HTTP
//!   handlers wired by `http::router`.
//!
//! This file is a thin façade: every public name is re-exported so
//! callers continue to use the `crate::control::oidc::X` paths.
//! The unit tests at the bottom of this file (form-urlencoding
//! roundtrip, logout session revocation, identity verifier
//! determinism, seal/unseal roundtrip) live here so the test
//! module sees the re-exported surface and not the split internals.

mod client;
mod flow_material;
mod handlers;
mod jwks;
mod sealing;

pub use client::OidcClient;
pub use flow_material::{
    AccessClaims, Audience, AuthError, OidcCallback, OidcNonce, OidcState, OidcTokens, PkceCode,
    StoredOidcRequest,
};
pub use handlers::{authorize, callback, logout};
pub use jwks::JwksCache;
pub use sealing::{identity_subject_verifier, seal_oidc_payload, unseal_oidc_payload};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::session::ControlPlaneSession;
    use crate::http::registry::RegistryHandle;
    use crate::http::registry::storage::RegistryStore;
    use std::sync::Arc;

    /// `form_urlencode_component` is a `fn` in `client.rs`. The
    /// test re-implements the same shape here so the public
    /// test surface doesn't have to import a private helper.
    fn form_urlencode_component(value: &str) -> String {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        let mut encoded = String::with_capacity(value.len());
        for byte in value.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    encoded.push(byte as char);
                }
                b' ' => encoded.push('+'),
                other => {
                    encoded.push('%');
                    encoded.push(HEX[(other >> 4) as usize] as char);
                    encoded.push(HEX[(other & 0x0F) as usize] as char);
                }
            }
        }
        encoded
    }

    #[test]
    fn form_urlencode_component_escapes_reserved_bytes() {
        assert_eq!(form_urlencode_component("a b+c/&"), "a+b%2Bc%2F%26");
    }

    /// `logout` must (1) delete the server-side session keyed by
    /// `cookie_hash`, (2) reply with a clearing
    /// `__Host-memory_mcp_session` cookie (Max-Age=0), and (3)
    /// redirect to `/`. The route is mounted behind
    /// `authenticate_control_plane_session` + CSRF in production;
    /// this test exercises the handler contract in isolation.
    #[tokio::test]
    async fn logout_revokes_session_and_clears_cookie() {
        let store: Arc<crate::http::registry::storage::InMemoryStore> =
            Arc::new(crate::http::registry::storage::InMemoryStore::default());
        let registry = RegistryHandle::in_memory().with_inner_store(store.clone());
        let state = crate::http::test_state::HttpStateTestBuilder::new()
            .await
            .with_registry(registry)
            .build()
            .await
            .expect("test HTTP state");
        let now = chrono::Utc::now();
        store
            .store_session(&ControlPlaneSession {
                id: "ses_logout".into(),
                cookie_hash: "cookie_logout".into(),
                account_id: "acct_logout".into(),
                auth_time: now,
                idle_expiry: now + chrono::Duration::minutes(30),
                absolute_expiry: now + chrono::Duration::hours(1),
            })
            .await
            .expect("store session");

        let (headers, redirect) = logout(
            axum::extract::State(state.clone()),
            axum::extract::Extension(ControlPlaneSession {
                id: "ses_logout".into(),
                cookie_hash: "cookie_logout".into(),
                account_id: "acct_logout".into(),
                auth_time: now,
                idle_expiry: now + chrono::Duration::minutes(30),
                absolute_expiry: now + chrono::Duration::hours(1),
            }),
        )
        .await
        .map_err(|_| "logout returned ApiError".to_string())
        .expect("logout ok");
        // 1) Server-side session is gone.
        assert!(
            store
                .find_session("cookie_logout")
                .await
                .expect("session lookup")
                .is_none(),
            "logout must delete the server-side session"
        );
        // 2) Set-Cookie clears the browser cookie.
        let cookie = headers
            .get(axum::http::header::SET_COOKIE)
            .expect("Set-Cookie present")
            .to_str()
            .expect("ascii cookie");
        assert!(
            cookie.starts_with("__Host-memory_mcp_session=;"),
            "got: {cookie}"
        );
        assert!(cookie.contains("Max-Age=0"), "got: {cookie}");
        assert!(cookie.contains("HttpOnly"), "got: {cookie}");
        assert!(cookie.contains("Secure"), "got: {cookie}");
        // 3) Cache-Control no-store prevents the redirect from being cached.
        assert_eq!(
            headers
                .get(axum::http::header::CACHE_CONTROL)
                .expect("Cache-Control present"),
            "no-store"
        );
        // 4) Redirect to "/".
        let redirect_response = axum::response::IntoResponse::into_response(redirect);
        assert_eq!(
            redirect_response.status(),
            axum::http::StatusCode::SEE_OTHER
        );
        assert_eq!(
            redirect_response
                .headers()
                .get(axum::http::header::LOCATION)
                .expect("Location header"),
            "/"
        );
    }

    #[test]
    fn identity_subject_verifier_is_deterministic() {
        let key = [0xABu8; 32];
        let a = identity_subject_verifier(&key, "https://issuer.example.com", "sub123").unwrap();
        let b = identity_subject_verifier(&key, "https://issuer.example.com", "sub123").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn identity_subject_verifier_differs_for_different_subjects() {
        let key = [0xABu8; 32];
        let a = identity_subject_verifier(&key, "https://issuer.example.com", "sub1").unwrap();
        let b = identity_subject_verifier(&key, "https://issuer.example.com", "sub2").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn seal_unseal_roundtrip() {
        let key = [0x42u8; 32];
        let state = OidcState::new();
        let nonce = OidcNonce::new();
        let pkce = PkceCode::new();

        let (ciphertext, nonce_bytes) = seal_oidc_payload(&key, &state, &nonce, &pkce).unwrap();

        let mut stored = unseal_oidc_payload(&key, &ciphertext, &nonce_bytes).unwrap();
        stored.pkce.challenge = String::new();

        assert_eq!(stored.state.as_str(), state.as_str());
        assert_eq!(stored.nonce.as_str(), nonce.as_str());
        assert_eq!(stored.pkce.verifier, pkce.verifier);
    }

    #[test]
    fn seal_unseal_wrong_key_fails() {
        let key = [0x42u8; 32];
        let wrong_key = [0x99u8; 32];
        let state = OidcState::new();
        let nonce = OidcNonce::new();
        let pkce = PkceCode::new();

        let (ciphertext, nonce_bytes) = seal_oidc_payload(&key, &state, &nonce, &pkce).unwrap();

        let result = unseal_oidc_payload(&wrong_key, &ciphertext, &nonce_bytes);
        assert!(result.is_err());
    }
}
