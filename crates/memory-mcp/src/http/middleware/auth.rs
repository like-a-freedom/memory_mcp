//! Authentication and CSRF middleware.
//!
//! Three independent authenticators cover the three trust paths:
//! - `authenticate` is the Bearer-API-key path used by `/mcp`.
//! - `authenticate_control_plane_session` is the Secure-cookie path
//!   used by `/api/v1/account/*` and `/api/v1/operator/*`.
//! - `authenticate_control_plane_operator` is the operator allowlist
//!   path layered on top of the session authenticator.
//!
//! `require_control_plane_csrf` is the cross-cutting CSRF check that
//! sits behind the session authenticator on state-changing API calls.

use axum::http::{Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::sync::Arc;

use crate::http::HttpState;
use crate::http::principal::auth::AuthDecision;

/// Build a 401 response with the `WWW-Authenticate: Bearer`
/// challenge. Used by `authenticate` (and any other auth
/// middleware that needs the same shape). The body is empty so
/// the response carries no information about why the request
/// was rejected.
fn unauthorized_response() -> Response {
    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = StatusCode::UNAUTHORIZED;
    response.headers_mut().insert(
        axum::http::header::WWW_AUTHENTICATE,
        axum::http::HeaderValue::from_static("Bearer realm=\"memory-mcp\""),
    );
    response
}

/// Bearer-token authenticator. Wired only
/// on `/mcp`. Returns 401 (with `WWW-Authenticate: Bearer
/// realm="memory-mcp"`) without distinguishing missing,
/// unknown, expired, revoked, or malformed keys. The raw
/// `Authorization` value is never logged or surfaced.
pub async fn authenticate(
    axum::extract::State(state): axum::extract::State<Arc<HttpState>>,
    mut req: axum::extract::Request,
    next: Next,
) -> Response {
    let header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let decision = match header.as_deref() {
        Some(value) => {
            let mut parts = value.split_ascii_whitespace();
            match (parts.next(), parts.next(), parts.next()) {
                (Some(scheme), Some(credentials), None)
                    if scheme.eq_ignore_ascii_case("Bearer") =>
                {
                    state.authenticator.authenticate_bearer(credentials).await
                }
                _ => AuthDecision::Deny,
            }
        }
        None => AuthDecision::Deny,
    };
    let principal = match decision {
        AuthDecision::Allow(principal) => principal,
        _ => return unauthorized_response(),
    };
    req.extensions_mut().insert(principal);
    next.run(req).await
}

/// Authenticate the control-plane Secure cookie and attach the server-side
/// session. This middleware is mounted only on `/api/v1/account/*` and never
/// participates in MCP Bearer authentication.
#[cfg(feature = "control-plane")]
pub async fn authenticate_control_plane_session(
    axum::extract::State(state): axum::extract::State<Arc<HttpState>>,
    mut req: axum::extract::Request,
    next: Next,
) -> Response {
    let cookie = req
        .headers()
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|cookie| {
                let (name, value) = cookie.trim().split_once('=')?;
                (name == "__Host-memory_mcp_session").then_some(value.to_owned())
            })
        });
    let Some(cookie) = cookie else {
        return (StatusCode::UNAUTHORIZED, "control-plane session required").into_response();
    };
    let session = match crate::control::session::resolve_session_record(&state, &cookie).await {
        Ok(session) => session,
        Err(error) => return error.into_response(),
    };
    req.extensions_mut().insert(session);
    next.run(req).await
}

/// Authenticate a control-plane session as an operator by matching one of its
/// durable external identity blind indexes against the immutable deployment
/// allowlist. Account data cannot grant itself this role.
#[cfg(feature = "control-plane")]
pub async fn authenticate_control_plane_operator(
    axum::extract::State(state): axum::extract::State<Arc<HttpState>>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    let Some(session) = req
        .extensions()
        .get::<crate::control::session::ControlPlaneSession>()
        .cloned()
    else {
        return (StatusCode::UNAUTHORIZED, "control-plane session required").into_response();
    };
    let identities = match state
        .registry
        .store_clone()
        .find_external_identities(&session.account_id)
        .await
    {
        Ok(identities) => identities,
        Err(_) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "operator registry unavailable",
            )
                .into_response();
        }
    };
    let is_operator = identities.iter().any(|identity| {
        let entry = format!(
            "{}|{}",
            identity.issuer,
            hex::encode(identity.subject_verifier.0)
        );
        state
            .config
            .operator_identity_allowlist
            .iter()
            .any(|allowed| allowed == &entry)
    });
    if !is_operator {
        return (StatusCode::FORBIDDEN, "operator access required").into_response();
    }
    let mut req = req;
    req.extensions_mut()
        .insert(crate::control::operator::OperatorPrincipal {
            authenticated_at: session.auth_time,
        });
    next.run(req).await
}

/// Require a valid CSRF header for a cookie-authenticated state-changing API
/// request. The token is bound to the Account and server-side session id.
#[cfg(feature = "control-plane")]
pub async fn require_control_plane_csrf(
    axum::extract::State(state): axum::extract::State<Arc<HttpState>>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    if matches!(
        req.method(),
        &Method::GET | &Method::HEAD | &Method::OPTIONS
    ) {
        return next.run(req).await;
    }
    let Some(session) = req
        .extensions()
        .get::<crate::control::session::ControlPlaneSession>()
    else {
        return (StatusCode::UNAUTHORIZED, "control-plane session required").into_response();
    };
    let token = req
        .headers()
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok());
    let valid = token.is_some_and(|token| {
        crate::control::csrf::verify_csrf(
            &state.config.keys.csrf,
            &session.account_id,
            &session.id,
            token,
        )
        .unwrap_or(false)
    });
    if !valid {
        return (StatusCode::FORBIDDEN, "csrf validation failed").into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::routing::post;
    use tower_service::Service;

    async fn accept_any(_: axum::extract::Request) -> Response {
        Response::new(axum::body::Body::empty())
    }

    #[tokio::test]
    async fn missing_bearer_returns_401_with_www_authenticate() {
        let state = crate::http::HttpState::default_for_test().await;
        let mut svc = Router::new()
            .route("/", post(accept_any))
            .layer(axum::middleware::from_fn_with_state(state, authenticate));
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = svc.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers()
                .get(axum::http::header::WWW_AUTHENTICATE)
                .map(|v| v.to_str().unwrap()),
            Some("Bearer realm=\"memory-mcp\""),
        );
    }

    #[tokio::test]
    async fn non_bearer_scheme_returns_401_with_www_authenticate() {
        let state = crate::http::HttpState::default_for_test().await;
        let mut svc = Router::new()
            .route("/", post(accept_any))
            .layer(axum::middleware::from_fn_with_state(state, authenticate));
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("authorization", "Basic abc")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = svc.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(
            resp.headers()
                .contains_key(axum::http::header::WWW_AUTHENTICATE)
        );
    }

    #[tokio::test]
    async fn router_level_wiring_validates_before_auth_and_keeps_health_public() {
        // Proves the spec-mandated ordering: malformed MCP requests fail
        // before auth, while a valid modern envelope reaches auth. The
        // health endpoint remains unauthenticated.
        use crate::http::router as build_router;
        let state = crate::http::HttpState::default_for_test().await;
        let router = build_router::build_router(state, None);
        let mut svc = router;

        let malformed = axum::http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "localhost")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = svc.call(malformed).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let valid_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "server/discover",
            "params": {
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientInfo": {
                        "name": "middleware-test",
                        "version": "0.0.0"
                    },
                    "io.modelcontextprotocol/clientCapabilities": {}
                }
            }
        });
        let valid = axum::http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "localhost")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "server/discover")
            .body(axum::body::Body::from(valid_body.to_string()))
            .unwrap();
        let resp = svc.call(valid).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(
            resp.headers()
                .contains_key(axum::http::header::WWW_AUTHENTICATE)
        );

        // /health/live is unauthenticated; rebuild a fresh
        // router to drive the second request.
        let state2 = crate::http::HttpState::default_for_test().await;
        let router2 = build_router::build_router(state2, None);
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/health/live")
            .header("host", "localhost")
            .body(axum::body::Body::empty())
            .unwrap();
        let mut svc = router2;
        let resp = svc.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
