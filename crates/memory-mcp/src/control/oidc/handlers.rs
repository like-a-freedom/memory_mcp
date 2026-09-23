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
use crate::http::registry::models::{
    ExternalIdentity, IdentityAudit, SubjectVerifier, new_external_identity_id,
};

use super::flow_material::{OidcCallback, OidcFlowIntent, OidcNonce, OidcState, PkceCode};
use super::sealing::{identity_subject_verifier, seal_oidc_payload, unseal_oidc_payload};

/// Seal a flow's material under `oidc_state`, store it, and return the provider
/// URL to send the browser to.
///
/// Both flows — signing in and attaching an identity to an Account — go through
/// here, so they differ only in the intent they seal and the URL they return.
async fn begin_flow(
    state: &std::sync::Arc<HttpState>,
    intent: OidcFlowIntent,
) -> Result<String, ApiError> {
    let pkce = PkceCode::new();
    let state_token = OidcState::new();
    let nonce = OidcNonce::new();

    // Seal the flow material and store keyed hash + ciphertext.
    let state_hash = hex::encode(identity_subject_verifier(
        &state.config.keys.oidc_state,
        "",
        state_token.as_str(),
    )?);
    let (sealed, aead_nonce) = seal_oidc_payload(
        &state.config.keys.oidc_state,
        &state_token,
        &nonce,
        &pkce,
        &intent,
    )?;

    #[cfg(feature = "control-plane")]
    let policy = state.browser_policy.as_ref().ok_or(ApiError::Unavailable)?;
    state
        .registry
        .store_clone()
        .store_oidc_request(policy, &state_hash, &sealed, &aead_nonce)
        .await?;

    let oidc = state.oidc_client.as_ref().ok_or(ApiError::Unavailable)?;
    let url = oidc.authorize_url(&state_token, &pkce, &nonce)?;
    Ok(url)
}

/// Start a flow that attaches the next provider-verified identity to
/// `account_id` (ADR-0057).
///
/// Called only from inside an authenticated Account session: the Account travels
/// in the sealed intent, so the callback that completes the flow does not have
/// to trust anything the browser presents beyond the state token.
#[cfg(feature = "control-plane")]
pub async fn start_link_flow(
    state: &std::sync::Arc<HttpState>,
    account_id: &str,
) -> Result<String, ApiError> {
    begin_flow(
        state,
        OidcFlowIntent::Link {
            account_id: account_id.to_owned(),
        },
    )
    .await
}

/// Where the browser lands after an OIDC flow: the console root —
/// origin root at an origin-root deployment, the mount base under a
/// prefix (the SPA lives there; the trailing-slash form `308`-
/// canonicalizes to it, so neither is a redirect chain).
fn console_home(base_path: &str) -> String {
    if base_path.is_empty() {
        "/".to_owned()
    } else {
        base_path.to_owned()
    }
}

/// GET /api/v1/auth/authorize — initiate OIDC login.
pub async fn authorize(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<HttpState>>,
) -> Result<axum::response::Redirect, ApiError> {
    let url = begin_flow(&state, OidcFlowIntent::SignIn).await?;
    Ok(axum::response::Redirect::to(&url))
}

/// POST /auth/oidc/logout — revoke the current browser session and clear its
/// cookie. The route is mounted behind cookie authentication and CSRF.
pub async fn logout(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<HttpState>>,
    axum::extract::Extension(session): axum::extract::Extension<ControlPlaneSession>,
) -> Result<(axum::http::HeaderMap, axum::response::Redirect), ApiError> {
    let policy = state.browser_policy.as_ref().ok_or(ApiError::Unavailable)?;
    state
        .registry
        .store_clone()
        .delete_session(policy, &session.cookie_hash)
        .await?;
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::SET_COOKIE,
        crate::control::session::clear_session_cookie(&state.config)
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
    Ok((
        headers,
        axum::response::Redirect::to(&console_home(&state.config.base_path)),
    ))
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

    // The deployment joined the durable OIDC policy at startup; every
    // flow/session operation below is guarded by it in the same
    // transaction.
    let policy = state.browser_policy.as_ref().ok_or(ApiError::Unavailable)?;

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
        .take_oidc_request(policy, &state_hash)
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

    // A link flow (ADR-0057) attaches the identity to the Account that started
    // it and leaves the session alone: the browser is already signed in. The
    // Account comes from the sealed intent, so nothing the callback received
    // chose it.
    if let OidcFlowIntent::Link { account_id } = stored.intent {
        link_verified_identity(
            &state.registry.store_clone(),
            &account_id,
            &claims.iss,
            subject_verifier,
        )
        .await?;
        return Ok((
            axum::http::header::HeaderMap::new(),
            axum::response::Redirect::to(&console_home(&state.config.base_path)),
        ));
    }

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

    // The deployment joined the durable OIDC policy at startup; the
    // signup bundle is guarded by that fence in one transaction.
    let account = OidcSignup::new(state.registry.store_clone())
        .resolve_or_create(
            policy,
            VerifiedExternalIdentity {
                issuer: claims.iss.clone(),
                subject_verifier,
            },
            chrono::Utc::now(),
        )
        .await
        .map_err(ApiError::Internal)?;

    let cookie_value = crate::control::session::generate_session_cookie_value();
    let session = ControlPlaneSession::new(&account, &cookie_value, policy.epoch, &state.config)?;
    state
        .registry
        .store_clone()
        .store_session(policy, &session)
        .await?;

    let cookie = crate::control::session::build_session_cookie(cookie_value, &state.config);
    let mut headers = axum::http::header::HeaderMap::new();
    headers.insert(
        axum::http::header::SET_COOKIE,
        cookie.parse().map_err(|_| {
            ApiError::Internal(MemoryError::ConfigInvalid("invalid cookie header".into()))
        })?,
    );
    Ok((
        headers,
        axum::response::Redirect::to(&console_home(&state.config.base_path)),
    ))
}

/// Attach a provider-verified identity to an Account (ADR-0057).
///
/// The identity is the one the provider just attested to, so this is a
/// proof-of-ownership attachment rather than an assertion. It is idempotent for
/// an identity the Account already holds, and refused when *another* Account
/// holds it — including when that other link was made by whoever guessed the
/// subject first, which is why the old body-supplied route was not safe to
/// keep. `(issuer, subject_verifier)` is unique in the durable schema, so two
/// racing flows cannot both succeed.
#[cfg(feature = "control-plane")]
async fn link_verified_identity(
    store: &std::sync::Arc<dyn crate::http::registry::storage::RegistryStore>,
    account_id: &str,
    issuer: &str,
    subject_verifier: SubjectVerifier,
) -> Result<(), MemoryError> {
    match store
        .find_account_by_identity(issuer, &subject_verifier)
        .await?
    {
        Some(existing) if existing.id == account_id => return Ok(()),
        Some(_) => {
            return Err(MemoryError::Conflict(
                "this identity is already linked to another account".into(),
            ));
        }
        None => {}
    }
    let identity = ExternalIdentity {
        id: new_external_identity_id(),
        issuer: issuer.to_owned(),
        subject_verifier,
        account_id: account_id.to_owned(),
        created_at: Utc::now(),
    };
    // The Account holder is the actor: the browser is already signed in, and the
    // identity being attached is the one the provider just attested to.
    store
        .link_external_identity(
            &identity,
            &IdentityAudit::by_account(account_id, Utc::now()),
        )
        .await?;
    Ok(())
}

#[cfg(all(test, feature = "control-plane"))]
mod tests {
    use super::*;
    use crate::http::registry::models::{Account, AccountStatus};
    use crate::http::registry::storage::{InMemoryStore, RegistryStore};
    use std::sync::Arc;

    /// Every post-flow redirect lands on the console root *inside the
    /// mount base* — dumping a signed-in browser at the origin root of a
    /// shared host would hand it to a sibling service or a 404.
    #[test]
    fn console_home_lands_inside_the_mount_base() {
        assert_eq!(console_home(""), "/");
        assert_eq!(console_home("/memory"), "/memory");
    }

    const ISSUER: &str = "https://idp.example.com";

    async fn store_with_two_accounts() -> Arc<dyn RegistryStore> {
        let store = InMemoryStore::default();
        for id in ["acct_one", "acct_two"] {
            store
                .write_account(&Account {
                    id: id.to_owned(),
                    status: AccountStatus::Active,
                    tenant_id: format!("ten_{id}"),
                    created_at: Utc::now(),
                })
                .await
                .expect("write account");
        }
        Arc::new(store)
    }

    fn verifier(byte: u8) -> SubjectVerifier {
        SubjectVerifier([byte; 32])
    }

    #[tokio::test]
    async fn a_verified_identity_is_linked_to_the_account_that_started_the_flow() {
        let store = store_with_two_accounts().await;
        link_verified_identity(&store, "acct_one", ISSUER, verifier(0xA1))
            .await
            .expect("link");

        let identities = store
            .find_external_identities("acct_one")
            .await
            .expect("list");
        assert_eq!(identities.len(), 1);
        assert_eq!(identities[0].issuer, ISSUER);
        assert_eq!(identities[0].subject_verifier, verifier(0xA1));
    }

    /// Re-running the flow for an identity this Account already holds is a
    /// no-op, not a duplicate row: a browser that re-completes the flow must not
    /// accumulate links to the same identity.
    #[tokio::test]
    async fn linking_the_same_identity_twice_is_idempotent() {
        let store = store_with_two_accounts().await;
        for _ in 0..2 {
            link_verified_identity(&store, "acct_one", ISSUER, verifier(0xA1))
                .await
                .expect("link");
        }
        assert_eq!(
            store
                .find_external_identities("acct_one")
                .await
                .expect("list")
                .len(),
            1
        );
    }

    /// The identity belongs to whoever the provider says it belongs to, so a
    /// second Account cannot attach an identity the first already holds. This is
    /// the case the body-supplied route could not distinguish from the first.
    #[tokio::test]
    async fn an_identity_held_by_another_account_is_refused() {
        let store = store_with_two_accounts().await;
        link_verified_identity(&store, "acct_one", ISSUER, verifier(0xA1))
            .await
            .expect("first link");

        let refused = link_verified_identity(&store, "acct_two", ISSUER, verifier(0xA1)).await;
        assert!(
            matches!(refused, Err(MemoryError::Conflict(_))),
            "a second account must be refused"
        );
        assert!(
            store
                .find_external_identities("acct_two")
                .await
                .expect("list")
                .is_empty(),
            "the refusal must not leave a partial link"
        );
    }
}
