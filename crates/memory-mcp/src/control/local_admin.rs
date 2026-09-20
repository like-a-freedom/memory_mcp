use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use std::sync::Arc;

use crate::http::HttpState;
use crate::service::local_admin::contracts::{
    AdminFence, AdminPrincipal, AuthAttemptContext, LocalAdminError, RequestContext,
};

/// The response request-id header name. Spec §8 requires the server to
/// generate a request id for every local error response; the same id is
/// also rendered into the body as `correlation_id`.
pub const REQUEST_ID_HEADER: &str = "x-request-id";

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
///
/// Rejections are rendered by the *single* spec §8 error renderer in
/// [`handlers`], so an extractor failure and a handler failure cannot
/// disagree about a status or a stable code.
pub struct RequireAdmin(pub AdminPrincipal);

impl FromRequestParts<Arc<HttpState>> for RequireAdmin {
    type Rejection = handlers::Rejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<HttpState>,
    ) -> Result<Self, Self::Rejection> {
        let ext = state
            .local_admin
            .as_ref()
            .ok_or_else(handlers::not_configured)?;

        let cookie_header = parts
            .headers
            .get("cookie")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let cookie_verifier = parse_admin_cookie(cookie_header)?
            .ok_or_else(|| handlers::map_error(LocalAdminError::Unauthenticated))?;

        let principal = crate::service::local_admin::auth::LocalAdminService::new(
            ext.authority.clone(),
            ext.hasher.clone(),
        )
        .resolve(&request_context_from_parts(parts), &cookie_verifier)
        .await
        .map_err(handlers::map_error)?;

        Ok(RequireAdmin(principal))
    }
}

/// Parse the `__Host-memory_mcp_admin=<hex>` cookie from the Cookie
/// header. Duplicate values are rejected rather than resolved by
/// "first wins"; every malformed shape collapses to
/// [`LocalAdminError::Unauthenticated`] so the caller cannot distinguish
/// "absent" from "malformed".
pub fn parse_admin_cookie(cookie_header: &str) -> Result<Option<[u8; 32]>, LocalAdminError> {
    let Some(value) = csrf::parse_cookie(cookie_header, csrf::SESSION_COOKIE)
        .map_err(|_| LocalAdminError::Unauthenticated)?
    else {
        return Ok(None);
    };
    let bytes = hex::decode(value).map_err(|_| LocalAdminError::Unauthenticated)?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| LocalAdminError::Unauthenticated)?;
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
    Some(crate::service::local_admin::policy::normalize_peer_ip(peer))
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
