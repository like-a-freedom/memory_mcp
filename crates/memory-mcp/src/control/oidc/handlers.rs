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

#[cfg(test)]
use crate::http::registry::models::{ExternalIdentity, IdentityAudit, new_external_identity_id};

use super::flow_material::{OidcCallback, OidcFlowIntent, OidcNonce, OidcState, PkceCode};
use super::sealing::{identity_subject_verifier, seal_oidc_payload, unseal_oidc_payload};

/// The single line every callback refusal is recorded as.
///
/// A failed login used to be indistinguishable from any other 401: the request
/// logger records only bounded labels, `request_id` and `tenant_fingerprint`
/// are empty for an unauthenticated callback, and the `correlation_id` in the
/// error envelope is never logged — so narrowing a bad login down to one of
/// these branches took latency measurements against a live provider instead of
/// a log line.
///
/// Build the structured event for one refusal on this path.
///
/// An event rather than a rendered line, so it carries what every other log
/// line in the process carries: a timestamp, a level, and the request id. The
/// id is what makes a refusal joinable to its access-log entry, which is the
/// only way to tell one refused sign-in from another.
///
/// Bounded by construction: callers pass a static branch tag and a detail built
/// from labels, lengths and `AuthError`'s display. Never the ID token, the
/// authorization code, the code verifier, the nonce value, or `sub`.
fn rejection_event(
    branch: &'static str,
    detail: &str,
    request_id: Option<uuid::Uuid>,
) -> std::collections::HashMap<String, serde_json::Value> {
    // Every refusal on this path builds its event here, so counting it here
    // counts sign-in failures as a rate. A failed login has no status code an
    // operator watches for — it is a 401 or 403 among thousands of legitimate
    // ones — and without this the only way to see a sign-in loop is to read
    // logs. The branch is the label because it is already a closed set of
    // static words, which is what keeps the series bounded.
    crate::observability::record_auth_refusal(branch);
    let mut event = std::collections::HashMap::new();
    event.insert("op".into(), "oidc.callback_rejected".into());
    event.insert("branch".into(), branch.into());
    if let Some(id) = request_id {
        event.insert("request_id".into(), id.to_string().into());
    }
    for field in detail.split_whitespace() {
        if let Some((key, value)) = field.split_once('=') {
            // A detail cannot relabel the event: the keys the event already
            // sets are not overwritten, so `branch=` inside a detail is ignored
            // rather than shadowing the real branch.
            event
                .entry(key.to_string())
                .or_insert_with(|| value.to_string().into());
        }
    }
    // `kind` quotes an `AuthError`, whose display is prose, so it runs past the
    // space the loop splits on. It is taken whole: cutting it at the first
    // space would report `kind=token` for a refusal that was actually about an
    // algorithm, which is the diagnosis this field exists to give.
    if let Some((_, prose)) = detail.split_once("kind=") {
        event.insert("kind".into(), prose.trim().to_string().into());
    }
    event
}

/// Build the event for one measured stage of the callback.
///
/// A stage is a name and an elapsed time, nothing else. The stages are what
/// make a slow sign-in explicable — which step took the time — and they are
/// recorded at `debug` so they cost nothing until an operator turns the
/// subsystem up to investigate.
fn stage_event(
    stage: &'static str,
    elapsed_ms: f64,
    request_id: Option<uuid::Uuid>,
) -> std::collections::HashMap<String, serde_json::Value> {
    let mut event = std::collections::HashMap::new();
    event.insert("op".into(), "oidc.callback_stage".into());
    event.insert("stage".into(), stage.into());
    event.insert("duration_ms".into(), elapsed_ms.into());
    if let Some(id) = request_id {
        event.insert("request_id".into(), id.to_string().into());
    }
    event
}

/// Measure one stage of the callback and record how long it took.
///
/// Sub-millisecond steps are kept: a stage that reports `0ms` is
/// indistinguishable from one that was never measured, and telling a cache
/// hit from a query is the reason to measure at all.
async fn timed<T, F, Fut>(stage: &'static str, request_id: Option<uuid::Uuid>, body: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let started = std::time::Instant::now();
    let outcome = body().await;
    log_stage(stage, started.elapsed().as_secs_f64() * 1000.0, request_id);
    outcome
}

fn log_stage(stage: &'static str, elapsed_ms: f64, request_id: Option<uuid::Uuid>) {
    use crate::logging::{LogLevel, StdoutLogger};
    StdoutLogger::from_env().log(stage_event(stage, elapsed_ms, request_id), LogLevel::Debug);
}

/// Record one refusal through the deployment's logger.
///
/// The logger is the process-wide one configured from `RUST_LOG`, so a refused
/// sign-in is filtered by the same level as everything else and reaches
/// `MEMORY_LOG_FILE` when one is installed.
fn log_rejection(branch: &'static str, detail: &str, request_id: Option<uuid::Uuid>) {
    use crate::logging::{LogLevel, StdoutLogger};
    StdoutLogger::from_env().log(rejection_event(branch, detail, request_id), LogLevel::Warn);
}

/// Refuse the callback and say why.
///
/// Only for refusals that really are the caller's fault. An error that must
/// keep its own status — a JWKS or provider outage is 503, not 401 — goes
/// through [`reject_erroring`] instead, so the log and the HTTP status agree.
fn reject(branch: &'static str) -> ApiError {
    log_rejection(branch, "", None);
    ApiError::Unauthorized
}

/// Record a refusal that carries a bounded detail, and answer 401.
///
/// For refusals that really are the caller's fault. An error that must keep its
/// own status — a JWKS or provider outage is 503, not 401 — goes through
/// [`reject_erroring`] instead, so the log and the HTTP status agree.
fn reject_with(branch: &'static str, detail: impl std::fmt::Display) -> ApiError {
    log_rejection(branch, &detail.to_string(), None);
    ApiError::Unauthorized
}

/// Record a refusal that carries a bounded detail, then return `error`
/// unchanged.
///
/// The status stays whatever `error` already maps to. Collapsing every failure
/// here into `Unauthorized` would relabel a server-side identity-provider
/// outage as a client credential fault — the same masquerade the JWKS
/// algorithm fix removed, reintroduced on the other side.
fn reject_erroring(
    branch: &'static str,
    detail: impl std::fmt::Display,
    error: ApiError,
) -> ApiError {
    log_rejection(branch, &detail.to_string(), error.request_id());
    error
}

/// A stable token for each way ID-token validation can fail.
///
/// The `thiserror` display strings are prose that a rename would silently
/// change; these are what a log query or an alert rule should match on. The
/// `jwks`/`provider` pair is a server-side fault and keeps its 503 — the split
/// an operator acts on.
fn id_token_reason(error: &super::flow_material::AuthError) -> &'static str {
    use super::flow_material::AuthError;
    match error {
        AuthError::MalformedToken => "malformed",
        AuthError::MissingKeyId => "missing_kid",
        AuthError::DisallowedAlgorithm { .. } => "disallowed_alg",
        AuthError::Jwt(_) => "jwt",
        AuthError::Jwks(_) => "jwks",
        AuthError::Provider(_) => "provider",
        AuthError::Sealing => "sealing",
    }
}

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
        .sessions()
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
        .sessions()
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
    request_headers: axum::http::HeaderMap,
    // The id of this request, so a refusal names the same operation the access
    // log records. This is the request an operator most needs to trace and the
    // one that arrives without a session, so it must not be the case that
    // reports an id nothing can be found under.
    //
    // The type here is what `request_log` actually inserts. It asks for
    // `Option<RequestId>` on the belief that axum treats the `Option` as an
    // optional extractor — it does not: `Extension` looks the exact type up in
    // the extensions map and finds `RequestId`, not `Option<RequestId>`, so the
    // extractor is rejected and this route 500s before its body runs. The id
    // cannot be absent: `request_log` is the outermost layer and mints one for
    // every request that reaches a route.
    axum::Extension(request_id): axum::Extension<crate::http::logging::RequestId>,
) -> Result<(axum::http::header::HeaderMap, axum::response::Redirect), ApiError> {
    callback_inner(&state, params, request_headers, Some(request_id))
        .await
        .map_err(|error| error.at_request(request_id.as_uuid()))
}

/// The callback's body, kept separate so every refusal passes back through the
/// one place that binds the request's id.
async fn callback_inner(
    state: &std::sync::Arc<HttpState>,
    params: OidcCallback,
    request_headers: axum::http::HeaderMap,
    request_id: Option<crate::http::logging::RequestId>,
) -> Result<(axum::http::header::HeaderMap, axum::response::Redirect), ApiError> {
    let request_uuid = request_id
        .as_ref()
        .map(crate::http::logging::RequestId::as_uuid);
    // Reject if the provider reported an error.
    if params.error.is_some() {
        return Err(reject("provider_error"));
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
        .sessions()
        .take_oidc_request(policy, &state_hash)
        .await?
        .ok_or_else(|| reject("take_oidc_request"))?;

    #[cfg(not(feature = "control-plane"))]
    {
        let _ = (&sealed, &aead_nonce);
        return Err(ApiError::Unavailable);
    }

    // The unseal is synchronous, so it is measured around the call rather than
    // around a future: the point is the work, not the await.
    let unseal_started = std::time::Instant::now();
    let stored = unseal_oidc_payload(&state.config.keys.oidc_state, &sealed, &aead_nonce);
    log_stage(
        "unseal",
        unseal_started.elapsed().as_secs_f64() * 1000.0,
        request_uuid,
    );
    let stored = stored?;
    if stored.state.as_str() != params.state {
        return Err(reject("state_mismatch"));
    }

    // No TTL check here — the registry enforces the deadline on consume; see
    // `StoredOidcRequest`.

    // RFC 9207 issuer check. Opportunistic defense-in-depth: the `iss`
    // parameter is optional and some providers never send it (Rauthy does
    // not), so `is_some_and` skips the check when absent — the binding one is
    // the id_token `iss` validation in `client::validate_id_token`. When the
    // parameter is present both sides are normalized (see
    // `client::normalize_issuer`): providers disagree about a trailing slash
    // on the issuer path.
    if params
        .iss
        .as_deref()
        .is_some_and(|issuer| !super::client::issuers_match(issuer, &state.config.oidc_issuer))
    {
        return Err(reject("rfc9207_issuer_mismatch"));
    }

    let code = params.code.ok_or_else(|| reject("missing_code"))?;

    let oidc = state.oidc_client.as_ref().ok_or(ApiError::Unavailable)?;

    // The exchange is the one stage that leaves the process, so its duration
    // is the number that separates "the provider answered" from "the provider
    // never answered" — the difference a support conversation asks about
    // first, and previously the only way to get was to time requests by hand.
    let tokens = timed("exchange", request_uuid, || async {
        oidc.exchange_code(code, stored.pkce).await
    })
    .await?;
    // The one branch worth reading first: `DisallowedAlgorithm` means our
    // configuration and the provider disagree, while `Jwt` means the token
    // itself failed. `reason` is a stable token to grep and to alert on; the
    // `AuthError` display names the same distinction in prose.
    let claims = match timed("validate", request_uuid, || async {
        oidc.validate_id_token(&tokens.id_token).await
    })
    .await
    {
        Ok(claims) => claims,
        Err(error) => {
            let detail = format!("reason={} kind={error}", id_token_reason(&error));
            return Err(reject_erroring("id_token", detail, ApiError::from(error)));
        }
    };

    // Validate nonce matches the one we generated for this request.
    if claims.nonce.as_deref() != Some(stored.nonce.as_str()) {
        // Shape, never content: the nonce is a secret, but whether one arrived
        // and whether its length matches is the whole diagnosis.
        return Err(reject_with(
            "nonce",
            format_args!(
                "token_present={} token_len={} stored_len={}",
                claims.nonce.is_some(),
                claims.nonce.as_deref().map(str::len).unwrap_or(0),
                stored.nonce.as_str().len()
            ),
        ));
    }

    let subject_verifier_bytes =
        identity_subject_verifier(&state.config.keys.identity_index, &claims.iss, &claims.sub)?;
    let subject_verifier = SubjectVerifier(subject_verifier_bytes);

    // A link flow (ADR-0057) attaches the identity to the Account that started
    // it and leaves the session alone: the browser is already signed in. The
    // Account comes from the sealed intent, so nothing the callback received
    // chose it.
    match stored.intent {
        OidcFlowIntent::Link { account_id } => {
            let actor = account_id.clone();
            crate::identity::api::link_verified_identity(
                state.verified_identity_transactions.as_ref(),
                &crate::identity::api::VerifiedIdentityCommand {
                    account_id,
                    issuer: claims.iss.clone(),
                    subject_verifier: subject_verifier.0,
                    actor,
                    mode: crate::identity::api::LinkMode::Add,
                },
                Utc::now(),
            )
            .await
            .map_err(|error| match error {
                crate::identity::api::IdentityError::Persistence(error) => {
                    ApiError::Internal(error)
                }
                _ => ApiError::Conflict,
            })?;
            return Ok((
                axum::http::header::HeaderMap::new(),
                axum::response::Redirect::to(&console_home(&state.config.base_path)),
            ));
        }
        // An invitation is the same link flow with the administrator as its
        // initiator: the Account and the inviter travel in the sealed intent.
        // Accepting one without a session is the first login, so the browser
        // is signed in as the bound Account unless it already holds a session.
        OidcFlowIntent::Invite {
            account_id,
            invited_by,
            replace,
        } => {
            return accept_invitation(
                state,
                &account_id,
                &invited_by,
                replace,
                &claims.iss,
                subject_verifier,
                &request_headers,
            )
            .await;
        }
        OidcFlowIntent::SignIn => {}
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
        if state
            .registry
            .accounts()
            .find_account_by_identity(&claims.iss, &subject_verifier)
            .await?
            .is_none()
        {
            // Logged like every other refusal here, and still 403: the id
            // provider authenticated the caller, so answering 401 would claim
            // the credentials were bad. This branch used to answer silently,
            // which made it the one refusal an operator could not tell from a
            // working deployment — the browser said "forbidden" and the logs
            // said nothing. Neither the issuer nor the subject is recorded;
            // the pair identifies a person, and "not invited" is the whole
            // diagnosis.
            return Err(reject_erroring(
                "signup_invite_only",
                format_args!("mode=invite_only"),
                ApiError::Forbidden,
            ));
        }
    }

    // The deployment joined the durable OIDC policy at startup; the
    // signup bundle is guarded by that fence in one transaction.
    let account = OidcSignup::new(state.registry.accounts(), state.registry.provisioning())
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

    let headers = issue_session(state, &account, policy).await?;
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
#[cfg(all(test, feature = "control-plane"))]
async fn link_verified_identity(
    accounts: &std::sync::Arc<dyn crate::http::registry::storage::AccountStore>,
    identities: &std::sync::Arc<dyn crate::http::registry::storage::IdentityStore>,
    account_id: &str,
    issuer: &str,
    subject_verifier: SubjectVerifier,
) -> Result<(), MemoryError> {
    // Self-service link: the Account holder is the actor (the browser is
    // already signed in, and the identity is the one the provider attested).
    attach_verified_identity(
        accounts,
        identities,
        account_id,
        issuer,
        subject_verifier,
        &IdentityAudit::by_account(account_id, Utc::now()),
    )
    .await
}

/// Attach a provider-verified identity under the caller's audit actor, with
/// the same proof-of-ownership rules as [`link_verified_identity`]: idempotent
/// for an identity the Account already holds, refused when *another* Account
/// holds the tuple.
#[cfg(all(test, feature = "control-plane"))]
async fn attach_verified_identity(
    accounts: &std::sync::Arc<dyn crate::http::registry::storage::AccountStore>,
    identities: &std::sync::Arc<dyn crate::http::registry::storage::IdentityStore>,
    account_id: &str,
    issuer: &str,
    subject_verifier: SubjectVerifier,
    audit: &IdentityAudit,
) -> Result<(), MemoryError> {
    match accounts
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
    identities.link_external_identity(&identity, audit).await?;
    Ok(())
}

/// Issue an identity invitation (ADR-0057): a sealed provider round trip that
/// attaches the next attested identity to `account_id`. The returned
/// authorize URL *is* the invitation — it is completed by whoever owns the
/// identity, within the flow's TTL. `replace` swaps the Account's single
/// mis-bound identity instead of adding beside it.
#[cfg(feature = "control-plane")]
pub async fn start_invite_flow(
    state: &std::sync::Arc<HttpState>,
    account_id: &str,
    invited_by: &str,
    replace: bool,
) -> Result<String, ApiError> {
    crate::identity::api::validate_identity_invitation(
        state.identity_invitation_port.as_ref(),
        &crate::identity::api::IdentityInvitationCommand {
            account_id: account_id.to_owned(),
            invited_by: invited_by.to_owned(),
            replace,
        },
    )
    .await
    .map_err(|error| match error {
        crate::identity::api::IdentityError::NotFound => ApiError::NotFound,
        crate::identity::api::IdentityError::LastIdentityOrConflict => ApiError::Conflict,
        crate::identity::api::IdentityError::Persistence(error) => ApiError::Internal(error),
        _ => ApiError::Internal(MemoryError::ConfigInvalid(
            "invalid identity invitation".into(),
        )),
    })?;
    begin_flow(
        state,
        OidcFlowIntent::Invite {
            account_id: account_id.to_owned(),
            invited_by: invited_by.to_owned(),
            replace,
        },
    )
    .await
}

/// Accept an identity invitation: attach the attested identity (adding beside
/// the Account's identities, or replacing a single mis-bound one) and, when the
/// browser holds no session yet, sign it in as the bound Account — the
/// acceptance is the first login (R2). An existing session is left alone.
#[cfg(feature = "control-plane")]
pub(crate) async fn accept_invitation(
    state: &std::sync::Arc<HttpState>,
    account_id: &str,
    invited_by: &str,
    replace: bool,
    issuer: &str,
    subject_verifier: SubjectVerifier,
    request_headers: &axum::http::header::HeaderMap,
) -> Result<(axum::http::header::HeaderMap, axum::response::Redirect), ApiError> {
    crate::identity::api::link_verified_identity(
        state.verified_identity_transactions.as_ref(),
        &crate::identity::api::VerifiedIdentityCommand {
            account_id: account_id.to_owned(),
            issuer: issuer.to_owned(),
            subject_verifier: subject_verifier.0,
            actor: invited_by.to_owned(),
            mode: if replace {
                crate::identity::api::LinkMode::Replace
            } else {
                crate::identity::api::LinkMode::Add
            },
        },
        Utc::now(),
    )
    .await
    .map_err(|error| match error {
        crate::identity::api::IdentityError::Persistence(error) => ApiError::Internal(error),
        _ => ApiError::Conflict,
    })?;

    let cookie_value = request_headers
        .get(axum::http::header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|raw| {
            crate::control::session::parse_session_cookie(raw, &state.config.base_path)
        });
    let session = crate::identity::api::ensure_invitation_session(
        state.invitation_session_port.as_ref(),
        account_id,
        cookie_value,
    )
    .await
    .map_err(ApiError::Internal)?;
    let headers = if let Some(session) = session {
        let cookie =
            crate::control::session::build_session_cookie(session.cookie_value, &state.config);
        let mut headers = axum::http::header::HeaderMap::new();
        headers.insert(
            axum::http::header::SET_COOKIE,
            cookie.parse().map_err(|_| {
                ApiError::Internal(MemoryError::ConfigInvalid("invalid cookie header".into()))
            })?,
        );
        headers
    } else {
        axum::http::header::HeaderMap::new()
    };
    Ok((
        headers,
        axum::response::Redirect::to(&console_home(&state.config.base_path)),
    ))
}

/// Mint a control-plane session for the Account and the Set-Cookie header
/// that carries it. Shared by sign-in and invitation acceptance.
#[cfg(feature = "control-plane")]
async fn issue_session(
    state: &std::sync::Arc<HttpState>,
    account: &crate::http::registry::models::Account,
    policy: &crate::http::registry::models::BrowserPolicyFence,
) -> Result<axum::http::header::HeaderMap, ApiError> {
    let cookie_value = crate::control::session::generate_session_cookie_value();
    let session = ControlPlaneSession::new(account, &cookie_value, policy.epoch, &state.config)?;
    state
        .registry
        .sessions()
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
    Ok(headers)
}

#[cfg(all(test, feature = "control-plane"))]
mod tests {
    use super::super::flow_material::AuthError;
    use super::*;
    use crate::logging::{LogLevel, StdoutLogger};

    /// Render an event the way the process would, so a test asserts the line an
    /// operator reads rather than the structure behind it.
    fn render(event: &std::collections::HashMap<String, serde_json::Value>) -> String {
        StdoutLogger::format_event_line(event, LogLevel::Warn)
    }

    use crate::http::registry::models::{Account, AccountStatus};
    use crate::http::registry::storage::{AccountStore, InMemoryStore};
    use std::sync::Arc;

    /// Every post-flow redirect lands on the console root *inside the
    /// mount base* — dumping a signed-in browser at the origin root of a
    /// shared host would hand it to a sibling service or a 404.
    #[test]
    fn console_home_lands_inside_the_mount_base() {
        assert_eq!(console_home(""), "/");
        assert_eq!(console_home("/memory"), "/memory");
    }

    /// The refusal line is the only signal a failed login leaves, so it has to
    /// be greppable and stable, and it has to carry nothing secret. Both
    /// halves matter: a branch tag that drifts makes the log useless, and a
    /// detail that leaks the token or the nonce would turn a diagnostic into a
    /// disclosure.
    #[test]
    fn a_refusal_line_names_its_branch_and_leaks_nothing() {
        let minimal = render(&rejection_event("state_mismatch", "", None));
        assert!(
            minimal.contains("op=oidc.callback_rejected"),
            "every refusal is one greppable operation: {minimal}"
        );
        assert!(minimal.contains("branch=state_mismatch"), "{minimal}");
        assert!(
            render(&rejection_event(
                "id_token",
                "kind=token algorithm is not allowed",
                None
            ))
            .contains(r#"token algorithm is not allowed"#),
            "the prose must not be cut at the first space"
        );
        // Every branch the callback can refuse on. A new refusal without a
        // tag here is a refusal an operator cannot tell apart from the rest.
        for branch in [
            "provider_error",
            "take_oidc_request",
            "state_mismatch",
            "rfc9207_issuer_mismatch",
            "missing_code",
            "id_token",
            "nonce",
        ] {
            let event = rejection_event(branch, "", None);
            assert_eq!(
                event.get("branch").and_then(|v| v.as_str()),
                Some(branch),
                "the branch must survive into the event"
            );
        }
    }

    /// The nonce detail is the one place a real value exists, so it reports
    /// shape and never content: `present` separates "the provider sent no
    /// nonce" from "the provider sent a different one", and the lengths are
    /// enough to tell them apart without ever recording the nonce itself.
    #[test]
    fn a_nonce_detail_reports_shape_rather_than_value() {
        let line = render(&rejection_event(
            "nonce",
            &format!(
                "token_present={} token_len={} stored_len={}",
                true,
                OidcNonce::new().as_str().len(),
                OidcNonce::new().as_str().len()
            ),
            None,
        ));

        assert!(line.contains("token_present=true"), "{line}");
        assert!(!line.contains("nonce="), "must not echo a nonce: {line}");
        // `branch=nonce` is the tag; a `nonce=<value>` field would be the leak.
        assert_eq!(line.matches("nonce=").count(), 0, "{line}");
    }

    /// A refused sign-in is the one incident with no alert: it is a 401 or a
    /// 403 among thousands of legitimate ones, so nothing in the request
    /// metrics moves, and a deployment in a sign-in loop looks exactly like a
    /// quiet one. Every refusal on this path builds its event in one function,
    /// so counting there covers the whole surface rather than the branches
    /// somebody remembered to instrument.
    #[tokio::test]
    #[cfg(feature = "prometheus")]
    async fn a_refused_sign_in_is_counted_for_the_exporter() {
        let exposition = crate::observability::tests::exposed(|| async {
            let _ = reject("state_mismatch");
            let _ = reject("nonce");
            crate::observability::tests::render()
        })
        .await;

        assert!(
            exposition.contains(crate::observability::METRIC_AUTH_REFUSALS_TOTAL),
            "a sign-in failure must be countable: {exposition}"
        );
        assert!(
            exposition.contains(r#"branch="state_mismatch""#),
            "and attributed to the reason it failed: {exposition}"
        );
        assert!(
            exposition.contains(r#"branch="nonce""#),
            "every refusal branch must count, not only the first: {exposition}"
        );
    }

    /// Every refusal on this path records a branch tag, and the sign-up gate is
    /// the one that used to be missing: it answered 403 with nothing in the
    /// logs, so a deployment that looked broken and a deployment that was
    /// working under `invite_only` were indistinguishable from the browser.
    #[test]
    fn the_signup_gate_records_its_own_branch() {
        let event = rejection_event("signup_invite_only", "mode=invite_only", None);
        assert_eq!(
            event.get("branch").and_then(|v| v.as_str()),
            Some("signup_invite_only")
        );
    }

    /// The gate refuses with 403, not 401: the provider authenticated the
    /// caller, so a 401 would tell them to retry with credentials that are
    /// already good.
    #[test]
    fn the_signup_gate_keeps_its_403() {
        let refused = reject_erroring(
            "signup_invite_only",
            format_args!("mode=invite_only"),
            ApiError::Forbidden,
        );
        let response = axum::response::IntoResponse::into_response(refused);
        assert_eq!(
            response.status(),
            axum::http::StatusCode::FORBIDDEN,
            "the gate must not relabel a policy refusal as a credential one"
        );
    }

    /// A refusal has to be findable the way every other log line is. These used
    /// to go to stderr as free text with no timestamp, no level and no request
    /// id, so the one class of event an operator most needs — a sign-in that
    /// was refused — was the one that could not be joined to the access log or
    /// filtered by level.
    ///
    /// The rendered line is the observable: it is what an operator greps.
    #[test]
    fn a_refusal_renders_as_a_structured_event() {
        let line = StdoutLogger::format_event_line(
            &rejection_event(
                "take_oidc_request",
                "reason=expired",
                Some(uuid::Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap()),
            ),
            crate::logging::LogLevel::Warn,
        );

        assert!(line.contains("WARN"), "the line must carry a level: {line}");
        assert!(
            line.contains("req=11111111"),
            "the line must carry the request id: {line}"
        );
        assert!(
            line.contains("op=oidc.callback_rejected"),
            "the line must be greppable by operation: {line}"
        );
        assert!(line.contains("branch=take_oidc_request"), "{line}");
        assert!(line.contains("reason=expired"), "{line}");
        // A timestamp is what makes the line sortable against every other.
        assert!(
            line.starts_with('[') && line.contains('T'),
            "the line must start with a timestamp: {line}"
        );
    }

    /// The event must never carry the values the module documents as secret —
    /// the token, the code, the nonce, the subject. A refusal line is the most
    /// read line in a deployment, so it is the one most worth asserting.
    #[test]
    fn a_refusal_event_carries_no_secret() {
        let event = rejection_event(
            "id_token",
            "token_present=1 token_len=0 stored_len=64",
            None,
        );
        let line = StdoutLogger::format_event_line(&event, crate::logging::LogLevel::Warn);
        assert!(!line.contains("token="), "{line}");
        assert!(!line.contains("nonce="), "{line}");
        assert!(!line.contains("code="), "{line}");
    }

    /// The ID-token detail is the provider's error string. `AuthError` is
    /// already a bounded `thiserror` display — a variant label plus two public
    /// header values, never the token — so it can be logged verbatim.
    ///
    /// The algorithm and key id the token arrived with are the point: a refusal
    /// that does not name them has to be diagnosed from configuration alone.
    #[test]
    fn an_id_token_detail_names_the_algorithm_and_key_id() {
        let line = render(&rejection_event(
            "id_token",
            &format!(
                "kind={}",
                AuthError::DisallowedAlgorithm {
                    alg: "RS384".into(),
                    kid: "key-7".into(),
                }
            ),
            None,
        ));

        assert!(
            line.contains(r#"kind="token algorithm is not allowed: alg=RS384 kid=key-7""#),
            "the prose must survive whole, quoted as one value: {line}"
        );
        assert!(line.contains("alg=RS384"), "{line}");
        assert!(line.contains("kid=key-7"), "{line}");
    }

    /// Each variant gets a token that does not move when the `thiserror` prose
    /// is reworded — a log rule or an alert has to survive that.
    #[test]
    fn every_id_token_failure_has_a_stable_reason_token() {
        assert_eq!(id_token_reason(&AuthError::MalformedToken), "malformed");
        assert_eq!(id_token_reason(&AuthError::MissingKeyId), "missing_kid");
        assert_eq!(
            id_token_reason(&AuthError::DisallowedAlgorithm {
                alg: "RS384".into(),
                kid: "k".into(),
            }),
            "disallowed_alg"
        );
        assert_eq!(
            id_token_reason(&AuthError::Jwks("unknown key id".into())),
            "jwks"
        );
        assert_eq!(
            id_token_reason(&AuthError::Provider("boom".into())),
            "provider"
        );
    }

    /// A JWKS or provider outage is server-side and must keep its 503. Logging
    /// a rejection must not flatten it into a 401: that would relabel an
    /// identity-provider failure as a bad credential and send an operator
    /// hunting the wrong thing — the exact masquerade unifying the algorithm
    /// lists was meant to remove.
    #[test]
    fn a_logged_rejection_keeps_the_status_its_error_maps_to() {
        let outage = reject_erroring(
            "id_token",
            "reason=jwks",
            ApiError::from(AuthError::Jwks("unknown key id".into())),
        );
        let refused = reject_erroring(
            "id_token",
            "reason=jwt",
            ApiError::from(AuthError::Jwt(
                jsonwebtoken::errors::ErrorKind::InvalidToken.into(),
            )),
        );

        assert!(
            matches!(outage, ApiError::Unavailable),
            "a JWKS outage must stay 503, not become 401"
        );
        assert!(
            matches!(refused, ApiError::Unauthorized),
            "an invalid token is still 401"
        );
        // And a plain caller-fault refusal is still 401.
        assert!(matches!(reject("state_mismatch"), ApiError::Unauthorized));
    }

    const ISSUER: &str = "https://idp.example.com";

    async fn store_with_two_accounts() -> (
        crate::http::registry::RegistryHandle,
        crate::http::registry::RegistryStores,
    ) {
        let store = Arc::new(InMemoryStore::default());
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
        let handle =
            crate::http::registry::RegistryHandle::in_memory().with_inner_store(store.clone());
        (
            handle,
            crate::http::registry::RegistryStores::from_backend(store),
        )
    }

    fn verifier(byte: u8) -> SubjectVerifier {
        SubjectVerifier([byte; 32])
    }

    #[tokio::test]
    async fn a_verified_identity_is_linked_to_the_account_that_started_the_flow() {
        let (_registry, stores) = store_with_two_accounts().await;
        link_verified_identity(
            &stores.accounts(),
            &stores.identities(),
            "acct_one",
            ISSUER,
            verifier(0xA1),
        )
        .await
        .expect("link");

        let identities = stores
            .identities()
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
        let (_registry, stores) = store_with_two_accounts().await;
        for _ in 0..2 {
            link_verified_identity(
                &stores.accounts(),
                &stores.identities(),
                "acct_one",
                ISSUER,
                verifier(0xA1),
            )
            .await
            .expect("link");
        }
        assert_eq!(
            stores
                .identities()
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
    async fn an_invitation_for_an_unknown_account_is_refused() {
        let (registry, stores) = store_with_two_accounts().await;
        let policy = stores
            .browser_policy()
            .reconcile_browser_policy(&[crate::http::config::BrowserAuthMethod::Oidc], None)
            .await
            .expect("reconcile browser policy");
        let state = crate::http::test_state::HttpStateTestBuilder::new()
            .await
            .with_registry(registry)
            .with_browser_policy(policy)
            .build()
            .await
            .expect("test HTTP state");

        let refused = start_invite_flow(&state, "acct_missing", "admin_root", false).await;
        assert!(matches!(refused, Err(ApiError::NotFound)));
    }

    /// Invitation acceptance (R2): a browser with no control-plane session is
    /// signed in as the bound Account — the provider just attested ownership,
    /// so the acceptance is the first login (ADR-0057 invitations).
    #[tokio::test]
    async fn an_invitation_acceptance_signs_the_browser_in_when_it_has_no_session() {
        let (registry, stores) = store_with_two_accounts().await;
        let policy = stores
            .browser_policy()
            .reconcile_browser_policy(&[crate::http::config::BrowserAuthMethod::Oidc], None)
            .await
            .expect("reconcile browser policy");
        let state = crate::http::test_state::HttpStateTestBuilder::new()
            .await
            .with_registry(registry)
            .with_browser_policy(policy.clone())
            .build()
            .await
            .expect("test HTTP state");

        let (headers, redirect) = accept_invitation(
            &state,
            "acct_one",
            "admin_root",
            false,
            "https://idp.example.com",
            verifier(0xA7),
            &axum::http::HeaderMap::new(),
        )
        .await
        .ok()
        .expect("invitation acceptance");

        assert!(
            headers.get(axum::http::header::SET_COOKIE).is_some(),
            "an acceptance without a session must sign the browser in"
        );
        let redirect_response = axum::response::IntoResponse::into_response(redirect);
        assert_eq!(
            redirect_response.status(),
            axum::http::StatusCode::SEE_OTHER
        );
    }

    #[tokio::test]
    async fn an_invitation_acceptance_leaves_an_existing_session_alone() {
        let (registry, stores) = store_with_two_accounts().await;
        let policy = stores
            .browser_policy()
            .reconcile_browser_policy(&[crate::http::config::BrowserAuthMethod::Oidc], None)
            .await
            .expect("reconcile browser policy");
        let config = crate::http::config::HttpConfig::default_for_test();
        let state = crate::http::test_state::HttpStateTestBuilder::new()
            .await
            .with_config(config.clone())
            .with_registry(registry)
            .with_browser_policy(policy.clone())
            .build()
            .await
            .expect("test HTTP state");

        let cookie_value = "existing-session-cookie-value";
        let cookie_hash = hex::encode(
            crate::control::session::keyed_session_hash(
                &config.keys.control_plane_session,
                cookie_value.as_bytes(),
            )
            .expect("session hash"),
        );
        let now = Utc::now();
        stores
            .sessions()
            .store_session(
                &policy,
                &ControlPlaneSession {
                    id: "ses_existing".into(),
                    cookie_hash,
                    account_id: "acct_one".into(),
                    browser_policy_epoch: Some(policy.epoch),
                    auth_time: now,
                    idle_expiry: now + chrono::Duration::minutes(30),
                    absolute_expiry: now + chrono::Duration::hours(1),
                },
            )
            .await
            .expect("store session");
        let mut request_headers = axum::http::HeaderMap::new();
        request_headers.insert(
            axum::http::header::COOKIE,
            format!(
                "{}={cookie_value}",
                crate::control::session::session_cookie_name(&config.base_path)
            )
            .parse()
            .expect("cookie header"),
        );

        let (headers, _redirect) = accept_invitation(
            &state,
            "acct_one",
            "admin_root",
            false,
            "https://idp.example.com",
            verifier(0xA8),
            &request_headers,
        )
        .await
        .ok()
        .expect("invitation acceptance");

        assert!(
            headers.get(axum::http::header::SET_COOKIE).is_none(),
            "an already-signed-in browser keeps its session"
        );
    }

    /// Q1 remediation: a replacing acceptance swaps the mis-bound identity.
    #[tokio::test]
    async fn a_replacing_invitation_swaps_the_misbound_identity() {
        let (registry, stores) = store_with_two_accounts().await;
        let policy = stores
            .browser_policy()
            .reconcile_browser_policy(&[crate::http::config::BrowserAuthMethod::Oidc], None)
            .await
            .expect("reconcile browser policy");
        let state = crate::http::test_state::HttpStateTestBuilder::new()
            .await
            .with_registry(registry)
            .with_browser_policy(policy)
            .build()
            .await
            .expect("test HTTP state");

        // The account is mis-bound to somebody else's identity.
        link_verified_identity(
            &state.registry.accounts(),
            &state.registry.identities(),
            "acct_one",
            "https://idp.example.com",
            verifier(0xB1),
        )
        .await
        .expect("mis-bound link");

        let (_headers, _redirect) = accept_invitation(
            &state,
            "acct_one",
            "admin_root",
            true,
            "https://idp.example.com",
            verifier(0xB2),
            &axum::http::HeaderMap::new(),
        )
        .await
        .ok()
        .expect("replacing acceptance");

        let identities = stores
            .identities()
            .find_external_identities("acct_one")
            .await
            .expect("list");
        assert_eq!(identities.len(), 1, "replace swaps, never accumulates");
        assert_eq!(identities[0].subject_verifier, verifier(0xB2));
    }

    #[tokio::test]
    async fn an_identity_held_by_another_account_is_refused() {
        let (_registry, stores) = store_with_two_accounts().await;
        link_verified_identity(
            &stores.accounts(),
            &stores.identities(),
            "acct_one",
            ISSUER,
            verifier(0xA1),
        )
        .await
        .expect("first link");

        let refused = link_verified_identity(
            &stores.accounts(),
            &stores.identities(),
            "acct_two",
            ISSUER,
            verifier(0xA1),
        )
        .await;
        assert!(
            matches!(refused, Err(MemoryError::Conflict(_))),
            "a second account must be refused"
        );
        assert!(
            stores
                .identities()
                .find_external_identities("acct_two")
                .await
                .expect("list")
                .is_empty(),
            "the refusal must not leave a partial link"
        );
    }

    /// A stage's duration has to reach the log under the same request id as
    /// the refusal it might explain. Without it, narrowing a sign-in failure
    /// means timing requests against a live provider — which is what the
    /// refusal branches exist to avoid — and a stage measured under a
    /// different id cannot be joined to the request that took that long.
    #[test]
    fn a_stage_duration_is_recorded_under_the_requests_own_id() {
        let request_id = uuid::Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();

        let line = render(&stage_event("exchange", 189.4, Some(request_id)));

        assert!(line.contains("op=oidc.callback_stage"), "{line}");
        assert!(line.contains("stage=exchange"), "{line}");
        assert!(line.contains("duration_ms=189.4"), "{line}");
        assert!(line.contains("req=11111111"), "{line}");
    }

    /// A stage is measured, not narrated: only its name and the elapsed time
    /// are recorded. Anything a stage touches — the code, the token, the
    /// provider's response — must not reach the log through this path.
    #[test]
    fn a_stage_event_carries_only_its_name_and_elapsed() {
        let line = render(&stage_event("exchange", 189.4, None));

        assert!(!line.contains("code="), "{line}");
        assert!(!line.contains("token="), "{line}");
        assert!(!line.contains("sub="), "{line}");
    }

    /// `timed` is the only thing that measures, so it is what the callback uses
    /// and what the test drives: an event built by hand proves the shape but
    /// not that a stage is ever recorded. Here the stage is measured and the
    /// body still runs, so a change that stopped measuring — or stopped
    /// running the body — fails here.
    #[tokio::test]
    async fn measuring_a_stage_returns_its_result_and_records_the_stage() {
        let request_id = uuid::Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();
        let sink = crate::logging::capture::install();
        // A stage is recorded at `debug`, so it reaches the log only when its
        // subsystem is turned up — the same thing an operator does to
        // investigate. The test turns it up for them.
        crate::logging::capture::with_level("oidc=debug", || async {
            let outcome = timed("exchange", Some(request_id), || async { 7usize }).await;
            assert_eq!(outcome, 7, "measuring must not change the result");
        })
        .await;

        let recorded = sink.lines();
        assert!(
            recorded.iter().any(|line| {
                line.contains("op=oidc.callback_stage")
                    && line.contains("stage=exchange")
                    && line.contains("req=11111111")
            }),
            "the measured stage must reach the log: {recorded:?}"
        );
    }

    /// A stage that fails is still measured. The stage that failed is the one
    /// an operator needs the duration of — a refused exchange is exactly when
    /// knowing it took four seconds matters — so measuring only successes
    /// would drop the half of the log that gets read.
    #[tokio::test]
    async fn a_failing_stage_is_measured_too() {
        let sink = crate::logging::capture::install();

        let outcome: Result<(), &str> =
            crate::logging::capture::with_level("oidc=debug", || async {
                timed("exchange", None, || async { Err("refused") }).await
            })
            .await;

        assert_eq!(outcome, Err("refused"), "the failure must still propagate");
        assert!(
            sink.lines()
                .iter()
                .any(|line| line.contains("stage=exchange")),
            "a failed stage must still be measured: {:?}",
            sink.lines()
        );
    }

    /// Stages are what an operator turns up to investigate, so they must not
    /// appear at the default level. A callback emits four or five of them, and
    /// a deployment logging them by default drowns the refusals that matter
    /// more than the timings do.
    #[test]
    fn a_stage_is_below_the_default_level() {
        let logger = StdoutLogger::from_env_with(|key| match key {
            "RUST_LOG" => Some("info".to_string()),
            _ => None,
        });

        assert!(
            !logger.is_event_enabled(LogLevel::Debug, "oidc.callback_stage"),
            "a stage must not be reported at the default level"
        );
        assert!(
            StdoutLogger::from_env_with(|key| match key {
                "RUST_LOG" => Some("oidc=debug".to_string()),
                _ => None,
            })
            .is_event_enabled(LogLevel::Debug, "oidc.callback_stage"),
            "a stage appears when its subsystem is turned up"
        );
    }
}
