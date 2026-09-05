//! OIDC HTTP handlers.
//!
//! Three routes wired by `http::router`:
//!
//! - `GET /api/v1/auth/authorize` — initiate OIDC login.
//! - `GET /api/v1/auth/callback` — OIDC provider redirects here.
//! - `POST /auth/oidc/logout` — revoke the current browser session
//!   and clear its cookie.
//!
//! The callback delegates to the `application::oidc_signup` workflow
//! for account resolution; it enforces the `SignupMode` policy
//! before delegating so the workflow stays policy-agnostic.

use chrono::Utc;

use crate::control::application::oidc_signup::{OidcSignup, VerifiedExternalIdentity};
use crate::control::error::ApiError;
use crate::control::session::ControlPlaneSession;
use crate::error::MemoryError;
use crate::http::HttpState;
use crate::http::config::SignupMode;
use crate::http::registry::models::SubjectVerifier;

use super::flow_material::{OidcCallback, OidcNonce, OidcState, PkceCode};
use super::sealing::{identity_subject_verifier, seal_oidc_payload, unseal_oidc_payload};

/// GET /api/v1/auth/authorize — initiate OIDC login.
pub async fn authorize(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<HttpState>>,
) -> Result<axum::response::Redirect, ApiError> {
    let pkce = PkceCode::new();
    let state_token = OidcState::new();
    let nonce = OidcNonce::new();

    // Seal the flow material and store keyed hash + ciphertext.
    let state_hash = hex::encode(identity_subject_verifier(
        &state.config.keys.oidc_state,
        "",
        state_token.as_str(),
    )?);
    let (sealed, aead_nonce) =
        seal_oidc_payload(&state.config.keys.oidc_state, &state_token, &nonce, &pkce)?;

    #[cfg(feature = "control-plane")]
    state
        .registry
        .store_clone()
        .store_oidc_request(&state_hash, &sealed, &aead_nonce)
        .await?;

    let oidc = state.oidc_client.as_ref().ok_or(ApiError::Unavailable)?;
    let url = oidc.authorize_url(&state_token, &pkce, &nonce)?;
    Ok(axum::response::Redirect::to(&url))
}

/// POST /auth/oidc/logout — revoke the current browser session and clear its
/// cookie. The route is mounted behind cookie authentication and CSRF.
pub async fn logout(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<HttpState>>,
    axum::extract::Extension(session): axum::extract::Extension<ControlPlaneSession>,
) -> Result<(axum::http::HeaderMap, axum::response::Redirect), ApiError> {
    state
        .registry
        .store_clone()
        .delete_session(&session.cookie_hash)
        .await?;
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::SET_COOKIE,
        "__Host-memory_mcp_session=; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=0"
            .parse()
            .map_err(|_| {
                ApiError::Internal(MemoryError::ConfigInvalid(
                    "invalid logout cookie header".into(),
                ))
            })?,
    );
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok((headers, axum::response::Redirect::to("/")))
}

/// GET /api/v1/auth/callback — OIDC provider redirects here.
pub async fn callback(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<HttpState>>,
    axum::extract::Query(params): axum::extract::Query<OidcCallback>,
) -> Result<(axum::http::header::HeaderMap, axum::response::Redirect), ApiError> {
    // Reject if the provider reported an error.
    if params.error.is_some() {
        return Err(ApiError::Unauthorized);
    }

    // Hash the incoming state to look up the sealed request.
    let state_hash = hex::encode(identity_subject_verifier(
        &state.config.keys.oidc_state,
        "",
        &params.state,
    )?);

    #[cfg(feature = "control-plane")]
    let (sealed, aead_nonce) = state
        .registry
        .store_clone()
        .take_oidc_request(&state_hash)
        .await?
        .ok_or(ApiError::Unauthorized)?;

    #[cfg(not(feature = "control-plane"))]
    {
        let _ = (&sealed, &aead_nonce);
        return Err(ApiError::Unavailable);
    }

    let stored = unseal_oidc_payload(&state.config.keys.oidc_state, &sealed, &aead_nonce)?;
    if stored.state.as_str() != params.state {
        return Err(ApiError::Unauthorized);
    }

    // Reject expired requests (TTL 10 minutes).
    if stored.expires_at < Utc::now() {
        return Err(ApiError::Unauthorized);
    }

    // RFC 9207 issuer check.
    if params
        .iss
        .as_deref()
        .is_some_and(|issuer| issuer != state.config.oidc_issuer)
    {
        return Err(ApiError::Unauthorized);
    }

    let code = params.code.ok_or(ApiError::Unauthorized)?;

    let oidc = state.oidc_client.as_ref().ok_or(ApiError::Unavailable)?;

    let tokens = oidc.exchange_code(code, stored.pkce).await?;
    let claims = oidc.validate_id_token(&tokens.id_token).await?;

    // Validate nonce matches the one we generated for this request.
    if claims.nonce.as_deref() != Some(stored.nonce.as_str()) {
        return Err(ApiError::Unauthorized);
    }

    let subject_verifier_bytes =
        identity_subject_verifier(&state.config.keys.identity_index, &claims.iss, &claims.sub)?;
    let subject_verifier = SubjectVerifier(subject_verifier_bytes);

    // The HTTP callback enforces the signup policy before
    // delegating to the application workflow. The workflow
    // itself is policy-agnostic: it does not know about
    // `SignupMode` and the test suite exercises it without
    // an Axum router.
    if matches!(state.config.signup_mode, SignupMode::InviteOnly) {
        // Look up first; only reject if the identity is
        // genuinely new. An existing account linked to this
        // identity is allowed to re-login even under
        // invite-only policy.
        let store = state.registry.store_clone();
        if store
            .find_account_by_identity(&claims.iss, &subject_verifier)
            .await?
            .is_none()
        {
            return Err(ApiError::Forbidden);
        }
    }

    let account = OidcSignup::new(state.registry.store_clone())
        .resolve_or_create(
            VerifiedExternalIdentity {
                issuer: claims.iss.clone(),
                subject_verifier,
            },
            chrono::Utc::now(),
        )
        .await
        .map_err(ApiError::Internal)?;

    let cookie_value = crate::control::session::generate_session_cookie_value();
    let session = ControlPlaneSession::new(&account, &cookie_value, &state.config)?;
    state.registry.store_clone().store_session(&session).await?;

    let cookie = crate::control::session::build_session_cookie(cookie_value, &state.config);
    let mut headers = axum::http::header::HeaderMap::new();
    headers.insert(
        axum::http::header::SET_COOKIE,
        cookie.parse().map_err(|_| {
            ApiError::Internal(MemoryError::ConfigInvalid("invalid cookie header".into()))
        })?,
    );
    Ok((headers, axum::response::Redirect::to("/")))
}
