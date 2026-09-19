//! Local admin HTTP handlers.
//!
//! Handles for challenge inspection/creation, login/reauth/logout,
//! client management, and key operations. Uses bounded body reads
//! (16KiB) instead of axum::Json since the json feature is not enabled.

use axum::body::Body;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::extract::State;
use std::sync::Arc;
use serde::{Deserialize, Serialize};

use crate::http::HttpState;
use crate::service::local_admin::contracts::{
    ChallengeKind, LocalAdminError,
};

const MAX_BODY_BYTES: usize = 16 * 1024; // 16KiB

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
}

fn error_response(status: StatusCode, code: &'static str, message: &str) -> Response {
    let body = ErrorBody {
        error: ErrorDetail {
            code,
            message: message.to_string(),
        },
        correlation_id: uuid::Uuid::new_v4().to_string(),
    };
    (
        status,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(&body).unwrap_or_default(),
    )
        .into_response()
}

fn map_error(e: LocalAdminError) -> Response {
    match e {
        LocalAdminError::InvalidInput(msg) => error_response(StatusCode::BAD_REQUEST, "bad_request", &msg),
        LocalAdminError::InvalidCredentials => error_response(StatusCode::UNAUTHORIZED, "unauthorized", "invalid credentials"),
        LocalAdminError::InvalidChallenge => error_response(StatusCode::BAD_REQUEST, "bad_request", "invalid challenge"),
        LocalAdminError::Unauthenticated => error_response(StatusCode::UNAUTHORIZED, "unauthorized", "unauthenticated"),
        LocalAdminError::Forbidden => error_response(StatusCode::FORBIDDEN, "forbidden", "forbidden"),
        LocalAdminError::ReauthRequired => error_response(StatusCode::UNAUTHORIZED, "reauth_required", "recent authentication required"),
        LocalAdminError::NotFound => error_response(StatusCode::NOT_FOUND, "not_found", "not found"),
        LocalAdminError::StateConflict => error_response(StatusCode::CONFLICT, "conflict", "state conflict"),
        LocalAdminError::VersionConflict => error_response(StatusCode::CONFLICT, "conflict", "version conflict"),
        LocalAdminError::IdempotencyConflict => error_response(StatusCode::CONFLICT, "conflict", "idempotency conflict"),
        LocalAdminError::KeyCap => error_response(StatusCode::CONFLICT, "conflict", "key cap reached"),
        LocalAdminError::SecretAlreadyIssued { key_id } => error_response(StatusCode::CONFLICT, "secret_already_issued", &format!("secret already issued for key {key_id}")),
        LocalAdminError::Throttled { retry_after_seconds } => error_response(StatusCode::TOO_MANY_REQUESTS, "throttled", &format!("retry after {retry_after_seconds}s")),
        LocalAdminError::Unavailable => error_response(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable", "temporarily unavailable"),
        LocalAdminError::Infrastructure(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", "internal error"),
    }
}

/// Parse bounded JSON body from request.
async fn parse_body<T: serde::de::DeserializeOwned>(body: Body) -> Result<T, Response> {
    let bytes = axum::body::to_bytes(body, MAX_BODY_BYTES)
        .await
        .map_err(|_| error_response(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large", "body exceeds 16KiB limit"))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| error_response(StatusCode::BAD_REQUEST, "bad_request", &format!("malformed JSON: {e}")))
}

// ─── Challenge handlers ───────────────────────────────────

#[derive(Deserialize)]
pub struct ChallengeRequest {
    pub code: String,
    pub kind: String,
}

#[derive(Serialize)]
pub struct ChallengeResponse {
    pub username: String,
    pub expires_at: String,
}

pub async fn inspect_challenge(
    State(_state): State<Arc<HttpState>>,
    body: Body,
) -> Response {
    let req: ChallengeRequest = match parse_body(body).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let _kind = match req.kind.as_str() {
        "activate" => ChallengeKind::Activate,
        "reset" => ChallengeKind::Reset,
        _ => return error_response(StatusCode::BAD_REQUEST, "bad_request", "kind must be 'activate' or 'reset'"),
    };
    error_response(StatusCode::NOT_IMPLEMENTED, "not_implemented", "challenge handler not yet wired to service")
}

#[derive(Deserialize)]
pub struct FinishChallengeRequest {
    pub code: String,
    pub kind: String,
    pub password: String,
}

pub async fn finish_challenge(
    State(_state): State<Arc<HttpState>>,
    body: Body,
) -> Response {
    let _req: FinishChallengeRequest = match parse_body(body).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    error_response(StatusCode::NOT_IMPLEMENTED, "not_implemented", "finish challenge handler not yet wired to service")
}

// ─── Auth handlers ────────────────────────────────────────

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct LoginResponse {
    pub admin_id: String,
    pub username: String,
    pub auth_time: String,
    pub absolute_expiry: String,
}

pub async fn login(
    State(_state): State<Arc<HttpState>>,
    body: Body,
) -> Response {
    let _req: LoginRequest = match parse_body(body).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    error_response(StatusCode::NOT_IMPLEMENTED, "not_implemented", "login handler not yet wired to service")
}

#[derive(Deserialize)]
pub struct ReauthRequest {
    pub password: String,
}

pub async fn reauth(
    State(_state): State<Arc<HttpState>>,
    body: Body,
) -> Response {
    let _req: ReauthRequest = match parse_body(body).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    error_response(StatusCode::NOT_IMPLEMENTED, "not_implemented", "reauth handler not yet wired to service")
}

pub async fn logout(
    State(_state): State<Arc<HttpState>>,
) -> Response {
    error_response(StatusCode::NOT_IMPLEMENTED, "not_implemented", "logout handler not yet wired to service")
}

// ─── Client handlers ──────────────────────────────────────

pub async fn list_clients(
    State(_state): State<Arc<HttpState>>,
) -> Response {
    error_response(StatusCode::NOT_IMPLEMENTED, "not_implemented", "list clients handler not yet wired")
}

#[derive(Deserialize)]
pub struct CreateClientRequest {
    pub display_name: String,
}

pub async fn create_client(
    State(_state): State<Arc<HttpState>>,
    body: Body,
) -> Response {
    let _req: CreateClientRequest = match parse_body(body).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    error_response(StatusCode::NOT_IMPLEMENTED, "not_implemented", "create client handler not yet wired")
}

pub async fn get_client(
    State(_state): State<Arc<HttpState>>,
    axum::extract::Path(account_id): axum::extract::Path<String>,
) -> Response {
    let _ = account_id;
    error_response(StatusCode::NOT_IMPLEMENTED, "not_implemented", "get client handler not yet wired")
}

pub async fn list_keys(
    State(_state): State<Arc<HttpState>>,
    axum::extract::Path(account_id): axum::extract::Path<String>,
) -> Response {
    let _ = account_id;
    error_response(StatusCode::NOT_IMPLEMENTED, "not_implemented", "list keys handler not yet wired")
}

#[derive(Deserialize)]
pub struct IssueKeyRequest {
    pub name: String,
}

pub async fn issue_key(
    State(_state): State<Arc<HttpState>>,
    axum::extract::Path(account_id): axum::extract::Path<String>,
    body: Body,
) -> Response {
    let _req: IssueKeyRequest = match parse_body(body).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let _ = account_id;
    error_response(StatusCode::NOT_IMPLEMENTED, "not_implemented", "issue key handler not yet wired")
}

pub async fn revoke_key(
    State(_state): State<Arc<HttpState>>,
    axum::extract::Path((account_id, key_id)): axum::extract::Path<(String, String)>,
) -> Response {
    let _ = (account_id, key_id);
    error_response(StatusCode::NOT_IMPLEMENTED, "not_implemented", "revoke key handler not yet wired")
}

#[derive(Deserialize)]
pub struct SetStateRequest {
    pub action: String,
    pub expected_version: u64,
}

pub async fn set_client_state(
    State(_state): State<Arc<HttpState>>,
    axum::extract::Path(account_id): axum::extract::Path<String>,
    body: Body,
) -> Response {
    let _req: SetStateRequest = match parse_body(body).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let _ = account_id;
    error_response(StatusCode::NOT_IMPLEMENTED, "not_implemented", "set client state handler not yet wired")
}
