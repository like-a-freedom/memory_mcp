#![allow(dead_code)]

use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use std::sync::Arc;

use crate::http::HttpState;
use crate::service::local_admin::contracts::{
    AdminPrincipal, AuthAttemptContext, LocalAdminError, RequestContext,
};

/// HTTP error mapping for local admin operations.
pub enum LocalAdminApiError {
    BadRequest(String),
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict(String),
    Throttled(u32),
    ServiceUnavailable,
    Internal(String),
}

impl IntoResponse for LocalAdminApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, "bad_request", msg),
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "unauthorized".into(),
            ),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden", "forbidden".into()),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found", "not found".into()),
            Self::Conflict(msg) => (StatusCode::CONFLICT, "conflict", msg),
            Self::Throttled(secs) => (
                StatusCode::TOO_MANY_REQUESTS,
                "throttled",
                format!("retry after {secs}s"),
            ),
            Self::ServiceUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "temporarily_unavailable",
                "temporarily unavailable".into(),
            ),
            Self::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error", msg),
        };
        let body = serde_json::json!({
            "error": {"code": code, "message": message},
            "correlation_id": uuid::Uuid::new_v4().to_string(),
        });
        (
            status,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            body.to_string(),
        )
            .into_response()
    }
}

impl From<LocalAdminError> for LocalAdminApiError {
    fn from(e: LocalAdminError) -> Self {
        match e {
            LocalAdminError::InvalidInput(msg) => Self::BadRequest(msg),
            LocalAdminError::InvalidCredentials => Self::Unauthorized,
            LocalAdminError::InvalidChallenge => Self::BadRequest("invalid challenge".into()),
            LocalAdminError::Unauthenticated => Self::Unauthorized,
            LocalAdminError::Forbidden => Self::Forbidden,
            LocalAdminError::ReauthRequired => Self::Unauthorized,
            LocalAdminError::NotFound => Self::NotFound,
            LocalAdminError::StateConflict => Self::Conflict("state conflict".into()),
            LocalAdminError::VersionConflict => Self::Conflict("version conflict".into()),
            LocalAdminError::IdempotencyConflict => Self::Conflict("idempotency conflict".into()),
            LocalAdminError::KeyCap => Self::Conflict("key cap reached".into()),
            LocalAdminError::SecretAlreadyIssued { key_id } => {
                Self::Conflict(format!("secret already issued for key {key_id}"))
            }
            LocalAdminError::Throttled {
                retry_after_seconds,
            } => Self::Throttled(retry_after_seconds),
            LocalAdminError::Unavailable => Self::ServiceUnavailable,
            LocalAdminError::Infrastructure(_) => Self::Internal("internal error".into()),
        }
    }
}

/// Extension that carries the local admin service, available when mode=local.
#[derive(Clone)]
pub struct LocalAdminExtension {
    pub authority: Arc<crate::service::local_admin::auth::LocalAdminAuthority>,
    pub hasher: Arc<crate::service::local_admin::password::PasswordHasher>,
}

/// Extractor that resolves the admin principal from the session cookie.
/// Returns 401 if no valid session exists.
pub struct RequireAdmin(pub AdminPrincipal);

impl FromRequestParts<Arc<HttpState>> for RequireAdmin {
    type Rejection = LocalAdminApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &Arc<HttpState>,
    ) -> Result<Self, Self::Rejection> {
        let ext =
            parts
                .extensions
                .get::<LocalAdminExtension>()
                .ok_or(LocalAdminApiError::Internal(
                    "local admin extension not mounted".into(),
                ))?;

        let cookie_header = parts
            .headers
            .get("cookie")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let cookie_verifier =
            parse_admin_cookie(cookie_header).ok_or(LocalAdminApiError::Unauthorized)?;

        let principal = ext
            .authority
            .store()
            .resolve_session(&cookie_verifier, ext.authority.policy())
            .await
            .map_err(LocalAdminApiError::from)?;

        Ok(RequireAdmin(principal))
    }
}

/// Parse the `__Host-memory_mcp_admin=<hex>` cookie from the Cookie header.
fn parse_admin_cookie(cookie_header: &str) -> Option<[u8; 32]> {
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some(val) = part.strip_prefix("__Host-memory_mcp_admin=") {
            let bytes = hex::decode(val).ok()?;
            let arr: [u8; 32] = bytes.try_into().ok()?;
            return Some(arr);
        }
    }
    None
}

pub mod handlers;

/// Build a `RequestContext` from axum request parts.
pub fn request_context_from_parts(parts: &Parts) -> RequestContext {
    let request_id = parts
        .extensions
        .get::<uuid::Uuid>()
        .copied()
        .unwrap_or_else(uuid::Uuid::new_v4);
    RequestContext { request_id }
}

/// Build an `AuthAttemptContext` from axum request parts.
pub fn auth_context_from_parts(parts: &Parts) -> AuthAttemptContext {
    let source = parts
        .extensions
        .get::<std::net::IpAddr>()
        .copied()
        .unwrap_or_else(|| std::net::IpAddr::from([127, 0, 0, 1]));
    AuthAttemptContext {
        request: request_context_from_parts(parts),
        source,
    }
}
