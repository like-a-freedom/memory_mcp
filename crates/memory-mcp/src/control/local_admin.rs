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
        base_path: &str,
    ) -> Result<(), LocalAdminError> {
        let cookie_header = headers
            .get(axum::http::header::COOKIE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        let cookie_value =
            csrf::parse_cookie_preferred(cookie_header, csrf::preauth_cookie_names(base_path))?
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
            .ok_or_else(|| handlers::not_configured().at(parts))?;

        let cookie_header = parts
            .headers
            .get("cookie")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let cookie_verifier = parse_admin_cookie(cookie_header, &state.config.base_path)
            .map_err(|error| handlers::map_error(error).at(parts))?
            .ok_or_else(|| handlers::map_error(LocalAdminError::Unauthenticated).at(parts))?;

        let principal = crate::service::local_admin::auth::LocalAdminService::new(
            ext.authority.clone(),
            ext.hasher.clone(),
        )
        .resolve(&request_context_from_parts(parts), &cookie_verifier)
        .await
        .map_err(|error| handlers::map_error(error).at(parts))?;

        Ok(RequireAdmin(principal))
    }
}

/// Parse the admin-session cookie (either mount name — see
/// [`csrf::session_cookie_names`]) from the Cookie header, preferring the
/// name for `base_path`. Duplicate values are rejected rather than resolved
/// by "first wins"; every malformed shape collapses to
/// [`LocalAdminError::Unauthenticated`] so the caller cannot distinguish
/// "absent" from "malformed".
pub fn parse_admin_cookie(
    cookie_header: &str,
    base_path: &str,
) -> Result<Option<[u8; 32]>, LocalAdminError> {
    let Some(value) =
        csrf::parse_cookie_preferred(cookie_header, csrf::session_cookie_names(base_path))
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

/// Mint the one request id per local-admin request and stamp it on the
/// response. The audit `RequestContext`, the `x-request-id` header and the
/// error envelope's `correlation_id` all read this value, so one reported id
/// identifies one request end to end (spec §10).
pub async fn attach_request_id(
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let request_id = req
        .extensions()
        .get::<uuid::Uuid>()
        .copied()
        .unwrap_or_else(uuid::Uuid::new_v4);
    req.extensions_mut().insert(request_id);
    let mut response = next.run(req).await;
    if !response.headers().contains_key(REQUEST_ID_HEADER)
        && let Ok(value) = axum::http::HeaderValue::from_str(&request_id.to_string())
    {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    response
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

#[cfg(test)]
mod request_id_tests {
    //! One request id per request: the error envelope's `correlation_id` and
    //! the `x-request-id` header name the same value the audit trail records.

    use super::attach_request_id;
    use crate::control::local_admin::handlers;
    use crate::service::local_admin::contracts::LocalAdminError;
    use axum::response::IntoResponse;

    async fn render_rejection(request: axum::extract::Request) -> axum::response::Response {
        let (parts, _body) = request.into_parts();
        handlers::map_error(LocalAdminError::Unauthenticated)
            .at(&parts)
            .into_response()
    }

    #[tokio::test]
    async fn the_error_envelope_carries_the_requests_id() {
        use tower_service::Service;

        let mut app = axum::Router::new()
            .route(
                "/api/v1/admin/session",
                axum::routing::get(render_rejection),
            )
            .layer(axum::middleware::from_fn(attach_request_id));
        let request = axum::extract::Request::builder()
            .uri("/api/v1/admin/session")
            .body(axum::body::Body::empty())
            .expect("request");
        let response = app.call(request).await.expect("response");

        let header = response
            .headers()
            .get(super::REQUEST_ID_HEADER)
            .expect("x-request-id header")
            .to_str()
            .expect("ascii id")
            .to_owned();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("error body");
        let envelope: serde_json::Value = serde_json::from_slice(&body).expect("json envelope");
        assert_eq!(
            envelope["correlation_id"],
            header.as_str(),
            "body and header must name one request id"
        );
    }
}
