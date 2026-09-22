//! Local admin HTTP handlers.
//!
//! Implements spec §8 exactly: pre-auth and session routes under
//! `/api/v1/auth/local/*`, `/api/v1/auth/config` and
//! `/api/v1/admin/*`.
//!
//! Every handler obeys the same transport contract:
//!
//! * the direct socket peer must be present (fail closed otherwise);
//! * every state-changing request carries an exact allowed `Origin`;
//! * public POSTs carry a pre-auth CSRF token, authenticated mutations
//!   carry a session CSRF token;
//! * bodies are bounded at 16 KiB and must be `application/json`;
//! * response bodies never echo storage errors, hashes or secrets;
//! * auth/session/client/key payloads are `Cache-Control: no-store`.

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::{
    LocalAdminExtension, RECENT_AUTH_WINDOW_SECONDS, REQUEST_ID_HEADER, RequireAdmin,
    auth_context_from_parts, csrf,
};
use crate::http::HttpState;
use crate::service::local_admin::auth::LocalAdminService;
use crate::service::local_admin::client::ClientAdminService;
use crate::service::local_admin::contracts::{
    AdminKeyCreate, AdminPrincipal, AuthAttemptContext, ChallengeKind, ClientCreate,
    ClientStateAction, KeyExpiry, LocalAdminError, LocalResult, PageRequest, RequestContext,
};

const MAX_BODY_BYTES: usize = 16 * 1024; // 16KiB

/// Pre-auth cookie lifetime. Kept in step with `csrf::PREAUTH_TTL_SECONDS`.
const PREAUTH_COOKIE_MAX_AGE: i64 = csrf::PREAUTH_TTL_SECONDS;

// ─── Response plumbing ────────────────────────────────────

/// HTTP error response body.
#[derive(Serialize)]
struct ErrorBody {
    error: ErrorDetail,
    correlation_id: String,
}

#[derive(Serialize)]
struct ErrorDetail {
    code: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    key_id: Option<String>,
}

/// A local-admin rejection that renders the spec §8 error envelope.
/// A rejection that has already been classified against the spec §8 table.
///
/// Guards return this rather than a whole `Response` so the error arm stays
/// small enough to pass by value (`clippy::result_large_err`) while still
/// carrying everything the renderer needs. It is also the extractor
/// (`RequireAdmin`) rejection type, so handler errors and extractor errors are
/// rendered by exactly one table.
pub struct Rejection {
    status: StatusCode,
    code: &'static str,
    message: String,
    key_id: Option<String>,
    /// `Retry-After` in seconds, emitted only on `429`.
    retry_after_seconds: Option<u32>,
}

impl Rejection {
    /// A rejection whose body omits `key_id`.
    fn new(status: StatusCode, code: &'static str, message: &str) -> Self {
        Self {
            status,
            code,
            message: message.to_string(),
            key_id: None,
            retry_after_seconds: None,
        }
    }

    /// A rejection whose body carries the offending key id.
    fn with_key_id(status: StatusCode, code: &'static str, message: &str, key_id: String) -> Self {
        Self {
            status,
            code,
            message: message.to_string(),
            key_id: Some(key_id),
            retry_after_seconds: None,
        }
    }

    /// A `429` that also tells the caller when to retry. Spec §7 requires
    /// the header, not only a sentence in the body.
    fn throttled(retry_after_seconds: u32) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "throttled",
            message: format!("retry after {retry_after_seconds}s"),
            key_id: None,
            retry_after_seconds: Some(retry_after_seconds),
        }
    }
}

impl IntoResponse for Rejection {
    fn into_response(self) -> Response {
        error_response_with_key(
            self.status,
            self.code,
            &self.message,
            self.key_id,
            self.retry_after_seconds,
        )
    }
}

impl From<LocalAdminError> for Rejection {
    fn from(error: LocalAdminError) -> Self {
        map_error(error)
    }
}

/// The single renderer of the spec §8 error envelope: every rejected
/// local-admin request funnels through here. Spec §8 also requires a
/// server-generated request id, emitted both as the `x-request-id`
/// response header and as `correlation_id` in the body.
fn error_response_with_key(
    status: StatusCode,
    code: &'static str,
    message: &str,
    key_id: Option<String>,
    retry_after_seconds: Option<u32>,
) -> Response {
    let request_id = uuid::Uuid::new_v4().to_string();
    let body = ErrorBody {
        error: ErrorDetail {
            code,
            message: message.to_string(),
            key_id,
        },
        correlation_id: request_id.clone(),
    };
    let mut response = (
        status,
        [
            (axum::http::header::CONTENT_TYPE, "application/json"),
            (axum::http::header::CACHE_CONTROL, "no-store"),
        ],
        serde_json::to_string(&body).unwrap_or_default(),
    )
        .into_response();
    if let Ok(value) = axum::http::HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    if let Some(seconds) = retry_after_seconds
        && let Ok(value) = axum::http::HeaderValue::from_str(&seconds.to_string())
    {
        response
            .headers_mut()
            .insert(axum::http::header::RETRY_AFTER, value);
    }
    response
}

/// Map a domain error onto the spec §8 status/body table. Underlying
/// storage errors are never rendered.
pub fn map_error(e: LocalAdminError) -> Rejection {
    match e {
        LocalAdminError::InvalidInput(msg) => {
            Rejection::new(StatusCode::BAD_REQUEST, "bad_request", &msg)
        }
        LocalAdminError::InvalidCredentials => Rejection::new(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "invalid credentials",
        ),
        LocalAdminError::InvalidChallenge => Rejection::new(
            StatusCode::BAD_REQUEST,
            "invalid_challenge",
            "the code is invalid, expired or already used",
        ),
        LocalAdminError::Unauthenticated => Rejection::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "administrator session required",
        ),
        LocalAdminError::Forbidden => {
            Rejection::new(StatusCode::FORBIDDEN, "forbidden", "request rejected")
        }
        LocalAdminError::ReauthRequired => Rejection::new(
            StatusCode::FORBIDDEN,
            "reauth_required",
            "recent authentication required",
        ),
        LocalAdminError::NotFound => {
            Rejection::new(StatusCode::NOT_FOUND, "not_found", "not found")
        }
        LocalAdminError::StateConflict => Rejection::new(
            StatusCode::CONFLICT,
            "conflict",
            "the client is not in a state that allows this operation",
        ),
        LocalAdminError::VersionConflict => Rejection::new(
            StatusCode::CONFLICT,
            "conflict",
            "the client changed; refresh and retry",
        ),
        LocalAdminError::IdempotencyConflict => Rejection::new(
            StatusCode::CONFLICT,
            "idempotency_conflict",
            "a different request already used this idempotency key",
        ),
        LocalAdminError::KeyCap => Rejection::new(
            StatusCode::CONFLICT,
            "key_cap_reached",
            "the client has reached its active key limit",
        ),
        LocalAdminError::SecretAlreadyIssued { key_id } => Rejection::with_key_id(
            StatusCode::CONFLICT,
            "secret_already_issued",
            "this key was already created; the secret cannot be shown again",
            key_id,
        ),
        LocalAdminError::Throttled {
            retry_after_seconds,
        } => Rejection::throttled(retry_after_seconds),
        LocalAdminError::Unavailable | LocalAdminError::Infrastructure(_) => Rejection::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "temporarily unavailable",
        ),
    }
}

/// Add `Cache-Control: no-store` to a successful response.
fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}

fn json_response<T: Serialize>(status: StatusCode, value: &T) -> Response {
    match serde_json::to_string(value) {
        Ok(body) => (
            status,
            [
                (axum::http::header::CONTENT_TYPE, "application/json"),
                (axum::http::header::CACHE_CONTROL, "no-store"),
            ],
            body,
        )
            .into_response(),
        // No reachable value of these DTOs fails to serialize. If one ever
        // did, fail closed with the sanitized outage answer instead of a
        // success status carrying an empty body.
        Err(_) => error_response_with_key(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "temporarily unavailable",
            None,
            None,
        ),
    }
}

fn no_content() -> Response {
    (
        StatusCode::NO_CONTENT,
        [(axum::http::header::CACHE_CONTROL, "no-store")],
    )
        .into_response()
}

// ─── Request plumbing ─────────────────────────────────────

/// Parse a bounded JSON body from the request.
async fn parse_body<T: serde::de::DeserializeOwned>(body: Body) -> Result<T, Rejection> {
    let bytes = axum::body::to_bytes(body, MAX_BODY_BYTES)
        .await
        .map_err(|_| {
            Rejection::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                "body exceeds the 16KiB limit",
            )
        })?;
    serde_json::from_slice(&bytes).map_err(|_| {
        Rejection::new(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "the request body is not valid JSON for this endpoint",
        )
    })
}

/// Enforce `Content-Type: application/json` on requests that carry a
/// body. Parameters such as `; charset=utf-8` are accepted.
fn require_json_content_type(parts: &Parts) -> Result<(), Rejection> {
    let value = parts
        .headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    let is_json = value
        .and_then(|value| value.split(';').next())
        .map(|media| media.trim().eq_ignore_ascii_case("application/json"))
        .unwrap_or(false);
    if is_json {
        Ok(())
    } else {
        Err(Rejection::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "Content-Type must be application/json",
        ))
    }
}

/// Exact allowed `Origin` on every state-changing local request.
fn require_origin(state: &Arc<HttpState>, headers: &HeaderMap) -> Result<(), Rejection> {
    csrf::require_allowed_origin(headers, &state.config.allowed_origins)
        .map_err(|_| Rejection::new(StatusCode::FORBIDDEN, "forbidden", "origin not allowed"))
}

/// Build the attempt context, failing closed when no direct peer is
/// attached (the server was started without connect-info).
fn attempt_context(parts: &Parts) -> Result<AuthAttemptContext, Rejection> {
    auth_context_from_parts(parts).ok_or_else(|| {
        Rejection::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "the server cannot determine the request source",
        )
    })
}

/// Rejection used when no local-admin extension is installed at all, which
/// the router prevents by mounting these routes only in local mode. Reported
/// as an outage rather than an internal error (spec §8).
pub fn not_configured() -> Rejection {
    Rejection::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "temporarily_unavailable",
        "local administrator authentication is not configured",
    )
}

fn get_ext(state: &Arc<HttpState>) -> Result<&LocalAdminExtension, Rejection> {
    state.local_admin.as_ref().ok_or_else(not_configured)
}

fn make_auth_service(ext: &LocalAdminExtension) -> LocalAdminService {
    LocalAdminService::new(ext.authority.clone(), ext.hasher.clone())
}

fn make_client_service(ext: &LocalAdminExtension, pepper: &str) -> ClientAdminService {
    ClientAdminService::new(ext.authority.clone(), ext.plan_version, pepper.to_owned())
}

/// Guard a public (pre-session) state-changing request.
fn guard_preauth(
    ext: &LocalAdminExtension,
    state: &Arc<HttpState>,
    parts: &Parts,
) -> Result<(), Rejection> {
    require_origin(state, &parts.headers)?;
    ext.verify_preauth_request(&parts.headers)
        .map_err(|_| Rejection::new(StatusCode::FORBIDDEN, "forbidden", "request rejected"))
}

/// Guard an authenticated state-changing request.
fn guard_session(
    ext: &LocalAdminExtension,
    state: &Arc<HttpState>,
    parts: &Parts,
    principal: &AdminPrincipal,
) -> Result<(), Rejection> {
    require_origin(state, &parts.headers)?;
    ext.verify_session_request(&parts.headers, &principal.fence)
        .map_err(|_| Rejection::new(StatusCode::FORBIDDEN, "forbidden", "request rejected"))
}

/// Reject a mutation when the current session did not authenticate
/// within the recent-auth window.
fn require_recent_auth(principal: &AdminPrincipal) -> Result<(), Rejection> {
    let age = chrono::Utc::now().signed_duration_since(principal.auth_time);
    if age.num_seconds() > RECENT_AUTH_WINDOW_SECONDS {
        Err(map_error(LocalAdminError::ReauthRequired))
    } else {
        Ok(())
    }
}

fn request_ctx(parts: &Parts) -> RequestContext {
    crate::control::local_admin::request_context_from_parts(parts)
}

// ─── Auth config ──────────────────────────────────────────

#[derive(Serialize)]
struct AuthConfigResponse {
    methods: Vec<&'static str>,
}

/// `GET /api/v1/auth/config` — the only public disclosure of how this deployment
/// authenticates browsers.
///
/// It reports the enabled **set**, not a single mode (ADR-0057), and reads it
/// from the configuration the router mounted, so what the login page offers and
/// what the deployment serves can never disagree. The endpoint is internal to the
/// embedded console, so the field was replaced rather than versioned.
pub async fn auth_config(State(state): State<Arc<HttpState>>) -> Response {
    let methods = state
        .config
        .browser_auth_methods()
        .into_iter()
        .map(crate::http::config::BrowserAuthMethod::as_str)
        .collect();
    json_response(StatusCode::OK, &AuthConfigResponse { methods })
}

// ─── Pre-auth CSRF ────────────────────────────────────────

#[derive(Serialize)]
struct PreauthResponse {
    csrf_token: String,
}

/// `GET /api/v1/auth/local/csrf` — establish the short-lived pre-auth
/// cookie and return the token the client must echo in
/// `X-CSRF-Token` on public POSTs.
pub async fn preauth_csrf(State(state): State<Arc<HttpState>>) -> Response {
    let ext = match get_ext(&state) {
        Ok(ext) => ext,
        Err(rejection) => return rejection.into_response(),
    };
    let issued = match csrf::issue_preauth(
        ext.authority.csrf_key(),
        ext.authority.policy().epoch,
        chrono::Utc::now().timestamp(),
    ) {
        Ok(issued) => issued,
        Err(error) => return map_error(error).into_response(),
    };
    let cookie = format!(
        "{}; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age={}",
        issued.cookie, PREAUTH_COOKIE_MAX_AGE
    );
    let mut response = json_response(
        StatusCode::OK,
        &PreauthResponse {
            csrf_token: issued.token,
        },
    );
    if let Ok(value) = axum::http::HeaderValue::from_str(&cookie) {
        response
            .headers_mut()
            .append(axum::http::header::SET_COOKIE, value);
    }
    response
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChallengeRequest {
    pub code: String,
    pub kind: String,
}

#[derive(Serialize)]
pub struct ChallengeResponse {
    pub username: String,
    pub expires_at: String,
}

fn parse_kind(raw: &str) -> Result<ChallengeKind, Rejection> {
    match raw {
        "activate" => Ok(ChallengeKind::Activate),
        "reset" => Ok(ChallengeKind::Reset),
        // A wrong kind is indistinguishable from an invalid code so the
        // caller cannot probe challenge state.
        _ => Err(map_error(LocalAdminError::InvalidChallenge)),
    }
}

/// `POST /api/v1/auth/local/challenge` — validate a code without
/// consuming it.
pub async fn inspect_challenge(
    State(state): State<Arc<HttpState>>,
    request: axum::extract::Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let ext = match get_ext(&state) {
        Ok(ext) => ext,
        Err(rejection) => return rejection.into_response(),
    };
    if let Err(rejection) = guard_preauth(ext, &state, &parts) {
        return rejection.into_response();
    }
    if let Err(rejection) = require_json_content_type(&parts) {
        return rejection.into_response();
    }
    let context = match attempt_context(&parts) {
        Ok(context) => context,
        Err(rejection) => return rejection.into_response(),
    };
    let req: ChallengeRequest = match parse_body(body).await {
        Ok(req) => req,
        Err(rejection) => return rejection.into_response(),
    };
    let kind = match parse_kind(&req.kind) {
        Ok(kind) => kind,
        Err(rejection) => return rejection.into_response(),
    };
    let service = make_auth_service(ext);
    match service.inspect_challenge(&context, &req.code, kind).await {
        Ok(view) => json_response(
            StatusCode::OK,
            &ChallengeResponse {
                username: view.username,
                expires_at: view.expires_at.to_rfc3339(),
            },
        ),
        Err(error) => map_error(error).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChallengePasswordRequest {
    pub code: String,
    pub password: String,
}

async fn finish_challenge(
    state: Arc<HttpState>,
    request: axum::extract::Request,
    kind: ChallengeKind,
) -> Response {
    let (parts, body) = request.into_parts();
    let ext = match get_ext(&state) {
        Ok(ext) => ext,
        Err(rejection) => return rejection.into_response(),
    };
    if let Err(rejection) = guard_preauth(ext, &state, &parts) {
        return rejection.into_response();
    }
    if let Err(rejection) = require_json_content_type(&parts) {
        return rejection.into_response();
    }
    let context = match attempt_context(&parts) {
        Ok(context) => context,
        Err(rejection) => return rejection.into_response(),
    };
    let req: ChallengePasswordRequest = match parse_body(body).await {
        Ok(req) => req,
        Err(rejection) => return rejection.into_response(),
    };
    let service = make_auth_service(ext);
    match service
        .finish_challenge(&context, &req.code, kind, req.password)
        .await
    {
        Ok(()) => no_content(),
        Err(error) => map_error(error).into_response(),
    }
}

/// `POST /api/v1/auth/local/activate` — atomic activation, no session.
pub async fn activate(
    State(state): State<Arc<HttpState>>,
    request: axum::extract::Request,
) -> Response {
    finish_challenge(state, request, ChallengeKind::Activate).await
}

/// `POST /api/v1/auth/local/reset` — atomic password reset, no session.
pub async fn reset(
    State(state): State<Arc<HttpState>>,
    request: axum::extract::Request,
) -> Response {
    finish_challenge(state, request, ChallengeKind::Reset).await
}

// ─── Login / session ──────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// `POST /api/v1/auth/local/login` — 204 with a new admin cookie.
pub async fn login(
    State(state): State<Arc<HttpState>>,
    request: axum::extract::Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let ext = match get_ext(&state) {
        Ok(ext) => ext,
        Err(rejection) => return rejection.into_response(),
    };
    if let Err(rejection) = guard_preauth(ext, &state, &parts) {
        return rejection.into_response();
    }
    if let Err(rejection) = require_json_content_type(&parts) {
        return rejection.into_response();
    }
    let context = match attempt_context(&parts) {
        Ok(context) => context,
        Err(rejection) => return rejection.into_response(),
    };
    let req: LoginRequest = match parse_body(body).await {
        Ok(req) => req,
        Err(rejection) => return rejection.into_response(),
    };
    let service = make_auth_service(ext);
    match service.login(&context, &req.username, req.password).await {
        Ok(login) => {
            let mut response = no_content();
            append_set_cookie(&mut response, &session_cookie(&login.cookie, None));
            // Rotate the pre-auth cookie away on success: it has no
            // further purpose and must not be replayable.
            append_set_cookie(&mut response, &clear_preauth_cookie());
            response
        }
        Err(error) => map_error(error).into_response(),
    }
}

#[derive(Serialize)]
struct SessionResponse {
    admin_id: String,
    username: String,
    auth_time: String,
    absolute_expiry: String,
    csrf_token: String,
}

/// `GET /api/v1/admin/session`.
pub async fn session(
    RequireAdmin(principal): RequireAdmin,
    State(state): State<Arc<HttpState>>,
) -> Response {
    let ext = match get_ext(&state) {
        Ok(ext) => ext,
        Err(rejection) => return rejection.into_response(),
    };
    let csrf_token = match ext.session_token(&principal.fence) {
        Ok(token) => token,
        Err(error) => return map_error(error).into_response(),
    };
    no_store(json_response(
        StatusCode::OK,
        &SessionResponse {
            admin_id: principal.fence.admin_id.clone(),
            username: principal.username.clone(),
            auth_time: principal.auth_time.to_rfc3339(),
            absolute_expiry: principal.absolute_expiry.to_rfc3339(),
            csrf_token,
        },
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReauthRequest {
    pub password: String,
}

/// `POST /api/v1/admin/reauth` — rotate session and CSRF.
pub async fn reauth(
    RequireAdmin(principal): RequireAdmin,
    State(state): State<Arc<HttpState>>,
    request: axum::extract::Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let ext = match get_ext(&state) {
        Ok(ext) => ext,
        Err(rejection) => return rejection.into_response(),
    };
    if let Err(rejection) = guard_session(ext, &state, &parts, &principal) {
        return rejection.into_response();
    }
    if let Err(rejection) = require_json_content_type(&parts) {
        return rejection.into_response();
    }
    let context = match attempt_context(&parts) {
        Ok(context) => context,
        Err(rejection) => return rejection.into_response(),
    };
    let req: ReauthRequest = match parse_body(body).await {
        Ok(req) => req,
        Err(rejection) => return rejection.into_response(),
    };
    let service = make_auth_service(ext);
    match service
        .reauthenticate(&context, &principal, req.password)
        .await
    {
        Ok(login) => {
            let mut response = no_content();
            // Max-Age reflects the *remaining* absolute lifetime, which
            // rotation preserves; it is never a fresh 24 hours.
            let remaining = login
                .principal
                .absolute_expiry
                .signed_duration_since(chrono::Utc::now())
                .num_seconds()
                .max(0);
            append_set_cookie(
                &mut response,
                &session_cookie(&login.cookie, Some(remaining)),
            );
            response
        }
        Err(error) => map_error(error).into_response(),
    }
}

/// `POST /api/v1/admin/logout`.
///
/// A request with a valid session must carry CSRF. A request with no
/// session may still return 204, but it must not revoke anything else.
pub async fn logout(
    State(state): State<Arc<HttpState>>,
    request: axum::extract::Request,
) -> Response {
    let (parts, _body) = request.into_parts();
    let ext = match get_ext(&state) {
        Ok(ext) => ext,
        Err(rejection) => return rejection.into_response(),
    };
    if let Err(rejection) = require_origin(&state, &parts.headers) {
        return rejection.into_response();
    }
    let cookie_header = parts
        .headers
        .get(axum::http::header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let verifier = match crate::control::local_admin::parse_admin_cookie(cookie_header) {
        Ok(Some(verifier)) => verifier,
        // No session: 204 with no side effect beyond clearing a cookie
        // the caller may or may not hold.
        Ok(None) => return clear_session_response(),
        Err(error) => return map_error(error).into_response(),
    };
    let service = make_auth_service(ext);
    let principal = match service.resolve(&request_ctx(&parts), &verifier).await {
        Ok(principal) => principal,
        // An expired, revoked or unverifiable cookie is not a session to
        // revoke, so logout stays a no-op for it. Every session read in this
        // module goes through the service ([`LocalAdminService::resolve`]).
        Err(_) => return clear_session_response(),
    };
    if let Err(rejection) = ext
        .verify_session_request(&parts.headers, &principal.fence)
        .map_err(|_| Rejection::new(StatusCode::FORBIDDEN, "forbidden", "request rejected"))
    {
        return rejection.into_response();
    }
    match service.logout(&request_ctx(&parts), &principal).await {
        Ok(()) => clear_session_response(),
        Err(error) => map_error(error).into_response(),
    }
}

fn clear_session_response() -> Response {
    let mut response = no_content();
    append_set_cookie(&mut response, &clear_session_cookie());
    response
}

fn append_set_cookie(response: &mut Response, cookie: &str) {
    if let Ok(value) = axum::http::HeaderValue::from_str(cookie) {
        response
            .headers_mut()
            .append(axum::http::header::SET_COOKIE, value);
    }
}

/// The admin session cookie. `Max-Age` is omitted for a session-scoped
/// cookie; the reauth path passes the remaining absolute lifetime.
fn session_cookie(cookie: &str, max_age: Option<i64>) -> String {
    match max_age {
        Some(seconds) => {
            format!("{cookie}; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age={seconds}")
        }
        None => format!("{cookie}; Path=/; Secure; HttpOnly; SameSite=Strict"),
    }
}

fn clear_session_cookie() -> String {
    format!(
        "{}; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0",
        csrf::SESSION_COOKIE
    )
}

fn clear_preauth_cookie() -> String {
    format!(
        "{}; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0",
        csrf::PREAUTH_COOKIE
    )
}

// ─── Client routes ────────────────────────────────────────

#[derive(Deserialize)]
pub struct PageQuery {
    pub after: Option<String>,
    pub limit: Option<u16>,
}

impl PageQuery {
    fn into_request(self) -> LocalResult<PageRequest> {
        let limit = self.limit.unwrap_or(50);
        if limit == 0 || limit > 100 {
            return Err(LocalAdminError::InvalidInput(
                "limit must be between 1 and 100".into(),
            ));
        }
        Ok(PageRequest {
            after: self.after.filter(|cursor| !cursor.is_empty()),
            limit,
        })
    }
}

fn idempotency_key(headers: &HeaderMap) -> Result<uuid::Uuid, Rejection> {
    let raw = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            Rejection::new(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "Idempotency-Key header is required",
            )
        })?;
    uuid::Uuid::parse_str(raw.trim()).map_err(|_| {
        Rejection::new(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "Idempotency-Key must be a UUID",
        )
    })
}

/// Trim outer whitespace and validate a user-supplied display or key
/// name: 1-100 Unicode scalar values, at most 400 UTF-8 bytes, no
/// control characters.
fn validate_name(raw: &str) -> LocalResult<String> {
    let trimmed = raw.trim();
    let scalars = trimmed.chars().count();
    if scalars == 0 {
        return Err(LocalAdminError::InvalidInput(
            "name must not be empty".into(),
        ));
    }
    if scalars > 100 {
        return Err(LocalAdminError::InvalidInput(
            "name must be at most 100 characters".into(),
        ));
    }
    if trimmed.len() > 400 {
        return Err(LocalAdminError::InvalidInput(
            "name must be at most 400 bytes".into(),
        ));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(LocalAdminError::InvalidInput(
            "name must not contain control characters".into(),
        ));
    }
    Ok(trimmed.to_string())
}

/// `GET /api/v1/admin/clients`
pub async fn list_clients(
    RequireAdmin(principal): RequireAdmin,
    State(state): State<Arc<HttpState>>,
    Query(query): Query<PageQuery>,
) -> Response {
    let ext = match get_ext(&state) {
        Ok(ext) => ext,
        Err(rejection) => return rejection.into_response(),
    };
    let page = match query.into_request() {
        Ok(page) => page,
        Err(error) => return map_error(error).into_response(),
    };
    let service = make_client_service(ext, &state.config.api_key_pepper);
    match service.list(&principal.fence, page).await {
        Ok(items) => no_store(json_response(StatusCode::OK, &items)),
        Err(error) => map_error(error).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateClientRequest {
    pub display_name: String,
}

/// `POST /api/v1/admin/clients`
pub async fn create_client(
    RequireAdmin(principal): RequireAdmin,
    State(state): State<Arc<HttpState>>,
    request: axum::extract::Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let ext = match get_ext(&state) {
        Ok(ext) => ext,
        Err(rejection) => return rejection.into_response(),
    };
    if let Err(rejection) = guard_session(ext, &state, &parts, &principal) {
        return rejection.into_response();
    }
    if let Err(rejection) = require_recent_auth(&principal) {
        return rejection.into_response();
    }
    if let Err(rejection) = require_json_content_type(&parts) {
        return rejection.into_response();
    }
    let operation_id = match idempotency_key(&parts.headers) {
        Ok(id) => id,
        Err(rejection) => return rejection.into_response(),
    };
    let req: CreateClientRequest = match parse_body(body).await {
        Ok(req) => req,
        Err(rejection) => return rejection.into_response(),
    };
    let display_name = match validate_name(&req.display_name) {
        Ok(name) => name,
        Err(error) => return map_error(error).into_response(),
    };

    let service = make_client_service(ext, &state.config.api_key_pepper);
    let ctx = request_ctx(&parts);
    let command = ClientCreate {
        display_name,
        operation_id,
    };
    match service.create(&principal.fence, &ctx, command).await {
        Ok(view) => {
            let mut response = no_store(json_response(StatusCode::ACCEPTED, &view));
            if let Ok(value) = axum::http::HeaderValue::from_str(&format!(
                "/api/v1/admin/clients/{}",
                view.account_id
            )) {
                response
                    .headers_mut()
                    .insert(axum::http::header::LOCATION, value);
            }
            response
        }
        Err(error) => map_error(error).into_response(),
    }
}

/// `GET /api/v1/admin/clients/{account_id}`
pub async fn get_client(
    RequireAdmin(principal): RequireAdmin,
    State(state): State<Arc<HttpState>>,
    Path(account_id): Path<String>,
) -> Response {
    let ext = match get_ext(&state) {
        Ok(ext) => ext,
        Err(rejection) => return rejection.into_response(),
    };
    let service = make_client_service(ext, &state.config.api_key_pepper);
    match service.get(&principal.fence, &account_id).await {
        Ok(view) => no_store(json_response(StatusCode::OK, &view)),
        Err(error) => map_error(error).into_response(),
    }
}

/// `GET /api/v1/admin/clients/{account_id}/keys`
pub async fn list_keys(
    RequireAdmin(principal): RequireAdmin,
    State(state): State<Arc<HttpState>>,
    Path(account_id): Path<String>,
    Query(query): Query<PageQuery>,
) -> Response {
    let ext = match get_ext(&state) {
        Ok(ext) => ext,
        Err(rejection) => return rejection.into_response(),
    };
    let page = match query.into_request() {
        Ok(page) => page,
        Err(error) => return map_error(error).into_response(),
    };
    let service = make_client_service(ext, &state.config.api_key_pepper);
    match service.keys(&principal.fence, &account_id, page).await {
        Ok(items) => no_store(json_response(StatusCode::OK, &items)),
        Err(error) => map_error(error).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssueKeyRequest {
    pub name: String,
    pub expiry: KeyExpiry,
}

#[derive(Serialize)]
pub struct IssuedKeyResponse {
    pub id: String,
    pub name: String,
    pub secret: String,
    pub expires_at: Option<String>,
}

/// `POST /api/v1/admin/clients/{account_id}/keys`
pub async fn issue_key(
    RequireAdmin(principal): RequireAdmin,
    State(state): State<Arc<HttpState>>,
    Path(account_id): Path<String>,
    request: axum::extract::Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let ext = match get_ext(&state) {
        Ok(ext) => ext,
        Err(rejection) => return rejection.into_response(),
    };
    if let Err(rejection) = guard_session(ext, &state, &parts, &principal) {
        return rejection.into_response();
    }
    if let Err(rejection) = require_recent_auth(&principal) {
        return rejection.into_response();
    }
    if let Err(rejection) = require_json_content_type(&parts) {
        return rejection.into_response();
    }
    let operation_id = match idempotency_key(&parts.headers) {
        Ok(id) => id,
        Err(rejection) => return rejection.into_response(),
    };
    let req: IssueKeyRequest = match parse_body(body).await {
        Ok(req) => req,
        Err(rejection) => return rejection.into_response(),
    };
    let name = match validate_name(&req.name) {
        Ok(name) => name,
        Err(error) => return map_error(error).into_response(),
    };
    if let KeyExpiry::Days { days } = req.expiry
        && !(1..=3650).contains(&days)
    {
        return map_error(LocalAdminError::InvalidInput(
            "expiry days must be between 1 and 3650".into(),
        ))
        .into_response();
    }

    // The credential is generated and revealed exactly once by the service;
    // only its verifier is persisted.
    let command = AdminKeyCreate {
        name: name.clone(),
        expiry: req.expiry.clone(),
        operation_id,
    };
    let service = make_client_service(ext, &state.config.api_key_pepper);
    match service
        .issue_key(&principal.fence, &request_ctx(&parts), &account_id, command)
        .await
    {
        Ok(issued) => no_store(json_response(
            StatusCode::CREATED,
            &IssuedKeyResponse {
                id: issued.id,
                name: issued.name,
                secret: issued.secret,
                expires_at: issued.expires_at.map(|value| value.to_rfc3339()),
            },
        )),
        Err(error) => map_error(error).into_response(),
    }
}

/// `DELETE /api/v1/admin/clients/{account_id}/keys/{key_id}`
pub async fn revoke_key(
    RequireAdmin(principal): RequireAdmin,
    State(state): State<Arc<HttpState>>,
    Path((account_id, key_id)): Path<(String, String)>,
    request: axum::extract::Request,
) -> Response {
    let (parts, _body) = request.into_parts();
    let ext = match get_ext(&state) {
        Ok(ext) => ext,
        Err(rejection) => return rejection.into_response(),
    };
    if let Err(rejection) = guard_session(ext, &state, &parts, &principal) {
        return rejection.into_response();
    }
    if let Err(rejection) = require_recent_auth(&principal) {
        return rejection.into_response();
    }
    let service = make_client_service(ext, &state.config.api_key_pepper);
    match service
        .revoke_key(&principal.fence, &request_ctx(&parts), &account_id, &key_id)
        .await
    {
        Ok(()) => no_store(StatusCode::NO_CONTENT.into_response()),
        Err(error) => map_error(error).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetStateRequest {
    pub expected_version: u64,
}

async fn set_client_state(
    state: Arc<HttpState>,
    principal: AdminPrincipal,
    parts: Parts,
    account_id: String,
    body: Body,
    action: ClientStateAction,
) -> Response {
    let ext = match get_ext(&state) {
        Ok(ext) => ext,
        Err(rejection) => return rejection.into_response(),
    };
    if let Err(rejection) = guard_session(ext, &state, &parts, &principal) {
        return rejection.into_response();
    }
    if let Err(rejection) = require_recent_auth(&principal) {
        return rejection.into_response();
    }
    if let Err(rejection) = require_json_content_type(&parts) {
        return rejection.into_response();
    }
    let req: SetStateRequest = match parse_body(body).await {
        Ok(req) => req,
        Err(rejection) => return rejection.into_response(),
    };
    let service = make_client_service(ext, &state.config.api_key_pepper);
    match service
        .set_state(
            &principal.fence,
            &request_ctx(&parts),
            &account_id,
            req.expected_version,
            action,
        )
        .await
    {
        Ok(()) => no_content(),
        Err(error) => map_error(error).into_response(),
    }
}

/// `POST /api/v1/admin/clients/{account_id}/suspend`
pub async fn suspend_client(
    RequireAdmin(principal): RequireAdmin,
    State(state): State<Arc<HttpState>>,
    Path(account_id): Path<String>,
    request: axum::extract::Request,
) -> Response {
    let (parts, body) = request.into_parts();
    set_client_state(
        state,
        principal,
        parts,
        account_id,
        body,
        ClientStateAction::Suspend,
    )
    .await
}

/// `POST /api/v1/admin/clients/{account_id}/resume`
pub async fn resume_client(
    RequireAdmin(principal): RequireAdmin,
    State(state): State<Arc<HttpState>>,
    Path(account_id): Path<String>,
    request: axum::extract::Request,
) -> Response {
    let (parts, body) = request.into_parts();
    set_client_state(
        state,
        principal,
        parts,
        account_id,
        body,
        ClientStateAction::Resume,
    )
    .await
}

#[cfg(test)]
mod tests {
    //! The spec §8 error table, asserted at the single renderer.

    use super::*;
    use crate::error::MemoryError;

    fn status_of(error: LocalAdminError) -> (StatusCode, &'static str) {
        let rejection = map_error(error);
        (rejection.status, rejection.code)
    }

    #[test]
    fn spec_status_table_is_exhaustive() {
        // Every arm of the domain error type maps to the documented status
        // and stable code. A storage/adapter failure is a 503 outage, not a
        // 500: spec §8 lists no 5xx other than `temporarily_unavailable`.
        assert_eq!(
            status_of(LocalAdminError::InvalidInput("x".into())),
            (StatusCode::BAD_REQUEST, "bad_request")
        );
        assert_eq!(
            status_of(LocalAdminError::InvalidChallenge),
            (StatusCode::BAD_REQUEST, "invalid_challenge")
        );
        assert_eq!(
            status_of(LocalAdminError::InvalidCredentials),
            (StatusCode::UNAUTHORIZED, "invalid_credentials")
        );
        assert_eq!(
            status_of(LocalAdminError::Unauthenticated),
            (StatusCode::UNAUTHORIZED, "unauthorized")
        );
        assert_eq!(
            status_of(LocalAdminError::Forbidden),
            (StatusCode::FORBIDDEN, "forbidden")
        );
        assert_eq!(
            status_of(LocalAdminError::ReauthRequired),
            (StatusCode::FORBIDDEN, "reauth_required")
        );
        assert_eq!(
            status_of(LocalAdminError::NotFound),
            (StatusCode::NOT_FOUND, "not_found")
        );
        assert_eq!(
            status_of(LocalAdminError::StateConflict),
            (StatusCode::CONFLICT, "conflict")
        );
        assert_eq!(
            status_of(LocalAdminError::VersionConflict),
            (StatusCode::CONFLICT, "conflict")
        );
        assert_eq!(
            status_of(LocalAdminError::IdempotencyConflict),
            (StatusCode::CONFLICT, "idempotency_conflict")
        );
        assert_eq!(
            status_of(LocalAdminError::KeyCap),
            (StatusCode::CONFLICT, "key_cap_reached")
        );
        assert_eq!(
            status_of(LocalAdminError::SecretAlreadyIssued {
                key_id: "key:7".into()
            }),
            (StatusCode::CONFLICT, "secret_already_issued")
        );
        assert_eq!(
            status_of(LocalAdminError::Throttled {
                retry_after_seconds: 9
            }),
            (StatusCode::TOO_MANY_REQUESTS, "throttled")
        );
        assert_eq!(
            status_of(LocalAdminError::Unavailable),
            (StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable")
        );
        assert_eq!(
            status_of(LocalAdminError::Infrastructure(MemoryError::Storage(
                "registry is down".into()
            ))),
            (StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable")
        );
    }

    #[test]
    fn infrastructure_detail_is_never_rendered() {
        let rejection = map_error(LocalAdminError::Infrastructure(MemoryError::Storage(
            "connection refused to ws://db.internal:8000".into(),
        )));
        assert!(
            !rejection.message.contains("db.internal"),
            "a storage error must not reach the wire: {}",
            rejection.message
        );
    }

    #[tokio::test]
    async fn every_error_carries_a_matching_request_id_header() {
        let response = map_error(LocalAdminError::NotFound).into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let header = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .expect("spec §8 requires a request-id header")
            .to_str()
            .expect("ascii request id")
            .to_owned();
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("error body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("json envelope");
        assert_eq!(
            body["correlation_id"],
            serde_json::json!(header),
            "the header and the body must carry the same id"
        );
    }

    #[tokio::test]
    async fn throttling_sends_retry_after_and_never_a_second_id() {
        let response = map_error(LocalAdminError::Throttled {
            retry_after_seconds: 17,
        })
        .into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::RETRY_AFTER)
                .expect("spec §7 requires Retry-After on 429"),
            "17"
        );
    }

    #[test]
    fn omitted_expiry_is_invalid_rather_than_non_expiring() {
        // Spec §8: "missing/null expiry is invalid rather than accidental
        // non-expiring issuance" — the strict DTO has no default.
        assert!(serde_json::from_str::<KeyExpiry>(r#"{"kind":"days","days":30}"#).is_ok());
        assert!(serde_json::from_str::<KeyExpiry>(r#"{"kind":"never"}"#).is_ok());
        assert!(
            serde_json::from_str::<KeyExpiry>(r#"{"kind":"unknown"}"#).is_err(),
            "an unknown expiry kind must be rejected"
        );
        assert!(
            serde_json::from_str::<KeyExpiry>(r#"{"kind":"days"}"#).is_err(),
            "a day-count expiry without days must be rejected"
        );
        assert!(
            serde_json::from_str::<KeyExpiry>(r#"{"kind":"never","bogus":30}"#).is_err(),
            "a stray field must not be silently ignored"
        );
        assert!(
            serde_json::from_str::<KeyExpiry>(r#"{"days":30}"#).is_err(),
            "a missing kind must be rejected"
        );
        assert!(
            serde_json::from_str::<IssueKeyRequest>(
                r#"{"name":"k","expiry":{"kind":"never"},"extra":1}"#,
            )
            .is_err(),
            "unknown request fields must be rejected"
        );
    }

    #[test]
    fn page_limits_match_the_documented_bounds() {
        let too_small = PageQuery {
            after: None,
            limit: Some(0),
        };
        assert!(too_small.into_request().is_err());
        let too_large = PageQuery {
            after: None,
            limit: Some(101),
        };
        assert!(too_large.into_request().is_err());
        let default = PageQuery {
            after: None,
            limit: None,
        }
        .into_request()
        .expect("default page");
        assert_eq!(default.limit, 50);
        let max = PageQuery {
            after: None,
            limit: Some(100),
        }
        .into_request()
        .expect("maximum page");
        assert_eq!(max.limit, 100);
    }
}
