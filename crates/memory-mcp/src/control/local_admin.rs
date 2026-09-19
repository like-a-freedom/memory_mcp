#![allow(dead_code)]

use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use std::sync::Arc;

use crate::http::HttpState;
use crate::service::local_admin::contracts::{
    AdminFence, AdminPrincipal, AuthAttemptContext, LocalAdminError, RequestContext,
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
    /// Plan version applied to clients created through the local workflow.
    pub plan_version: u32,
}

impl LocalAdminExtension {
    /// Verify a presented pre-auth cookie and token, returning `Ok(())`
    /// on success. Callers that need the nonce use [`csrf::verify_preauth`]
    /// directly.
    pub fn verify_preauth_request(
        &self,
        headers: &axum::http::HeaderMap,
    ) -> Result<(), LocalAdminError> {
        let cookie_header = headers
            .get(axum::http::header::COOKIE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        let cookie_value = csrf::parse_cookie(cookie_header, csrf::PREAUTH_COOKIE)?
            .ok_or(LocalAdminError::Forbidden)?;
        let token = headers
            .get("x-csrf-token")
            .and_then(|value| value.to_str().ok())
            .ok_or(LocalAdminError::Forbidden)?;
        csrf::verify_preauth(
            self.authority.csrf_key(),
            &cookie_value,
            token,
            self.authority.policy().epoch,
            chrono::Utc::now().timestamp(),
        )?;
        Ok(())
    }

    /// Verify a session CSRF token against the presented fence.
    pub fn verify_session_request(
        &self,
        headers: &axum::http::HeaderMap,
        fence: &AdminFence,
    ) -> Result<(), LocalAdminError> {
        let token = headers
            .get("x-csrf-token")
            .and_then(|value| value.to_str().ok())
            .ok_or(LocalAdminError::Forbidden)?;
        csrf::verify_session_token(self.authority.csrf_key(), fence, token)
    }

    /// Compute the session CSRF token bound to a fence.
    pub fn session_token(&self, fence: &AdminFence) -> Result<String, LocalAdminError> {
        csrf::session_token(self.authority.csrf_key(), fence)
    }
}

pub mod csrf;
pub mod handlers;

/// Seconds within which a credential-affecting or client-mutating
/// operation must have re-authenticated.
pub const RECENT_AUTH_WINDOW_SECONDS: i64 = 600;

/// Extractor that resolves the admin principal from the session cookie.
/// Returns 401 if no valid session exists.
pub struct RequireAdmin(pub AdminPrincipal);

impl FromRequestParts<Arc<HttpState>> for RequireAdmin {
    type Rejection = LocalAdminApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<HttpState>,
    ) -> Result<Self, Self::Rejection> {
        let ext = state
            .local_admin
            .as_ref()
            .ok_or(LocalAdminApiError::Internal(
                "local admin not configured".into(),
            ))?;

        let cookie_header = parts
            .headers
            .get("cookie")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let cookie_verifier =
            parse_admin_cookie(cookie_header)?.ok_or(LocalAdminApiError::Unauthorized)?;

        let principal = ext
            .authority
            .store()
            .resolve_session(&cookie_verifier, ext.authority.policy())
            .await
            .map_err(LocalAdminApiError::from)?;

        Ok(RequireAdmin(principal))
    }
}

/// Parse the `__Host-memory_mcp_admin=<hex>` cookie from the Cookie
/// header. Duplicate values are rejected rather than resolved by
/// "first wins".
pub fn parse_admin_cookie(cookie_header: &str) -> Result<Option<[u8; 32]>, LocalAdminApiError> {
    let Some(value) = csrf::parse_cookie(cookie_header, csrf::SESSION_COOKIE)
        .map_err(|_| LocalAdminApiError::Unauthorized)?
    else {
        return Ok(None);
    };
    let bytes = hex::decode(value).map_err(|_| LocalAdminApiError::Unauthorized)?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| LocalAdminApiError::Unauthorized)?;
    Ok(Some(arr))
}

/// Build a `RequestContext` from axum request parts.
pub fn request_context_from_parts(parts: &Parts) -> RequestContext {
    let request_id = parts
        .extensions
        .get::<uuid::Uuid>()
        .copied()
        .unwrap_or_else(uuid::Uuid::new_v4);
    RequestContext { request_id }
}

/// The direct socket peer address, normalized so an IPv4-mapped IPv6
/// peer shares a throttle bucket with its IPv4 form.
///
/// Forwarding headers are deliberately ignored: a client can set
/// `X-Forwarded-For` freely, so honouring it would let a single source
/// mint unlimited throttle buckets. A deployment behind one proxy
/// therefore shares that proxy's limit — an availability trade-off the
/// operator accepts explicitly.
pub fn direct_peer(parts: &Parts) -> Option<std::net::IpAddr> {
    let peer = parts
        .extensions
        .get::<axum::extract::connect_info::ConnectInfo<std::net::SocketAddr>>()?
        .0
        .ip();
    Some(match peer {
        std::net::IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => std::net::IpAddr::V4(v4),
            None => std::net::IpAddr::V6(v6),
        },
        ip => ip,
    })
}

/// Build an `AuthAttemptContext` from axum request parts.
///
/// Returns `None` when no direct peer is attached, so callers can fail
/// closed instead of silently substituting a shared default that would
/// collapse every client into one throttle bucket.
pub fn auth_context_from_parts(parts: &Parts) -> Option<AuthAttemptContext> {
    let source = direct_peer(parts)?;
    Some(AuthAttemptContext {
        request: request_context_from_parts(parts),
        source,
    })
}
