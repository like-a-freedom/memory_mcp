//! HTTP middleware (ADR-0052 §"Two deployment profiles").
//!
//! Each middleware is `axum::middleware::from_fn`-compatible. Layer
//! ordering matters: layers added LATER wrap layers added EARLIER on
//! the request path. The plan's Task 3.5–3.7 stack is documented
//! in `http::router::build_router`.

use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;

use super::HttpState;
use super::principal::auth::AuthDecision;

/// Reject every non-POST method on `/mcp` (spec §4). Runs before
/// routing; all other paths pass through untouched. Defense in depth
/// on top of axum's own method matcher.
pub async fn reject_non_post_mcp(
    method: Method,
    req: axum::extract::Request,
    next: Next,
) -> Result<Response, (StatusCode, &'static str)> {
    let path = req.uri().path();
    if path == "/mcp" && method != Method::POST {
        return Err((StatusCode::METHOD_NOT_ALLOWED, "POST required"));
    }
    Ok(next.run(req).await)
}

/// Bearer-token authenticator (ADR-0052, plan §4.6). Wired only
/// on `/mcp`. Returns 401 without distinguishing missing,
/// unknown, expired, revoked, or malformed keys. The raw
/// `Authorization` value is never logged or surfaced.
pub async fn authenticate(
    axum::extract::State(state): axum::extract::State<Arc<HttpState>>,
    mut req: axum::extract::Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let decision = match header.as_deref() {
        Some(h) if h.starts_with("Bearer ") => {
            state.authenticator.authenticate_bearer(&h[7..]).await
        }
        _ => AuthDecision::Deny,
    };
    let principal = match decision {
        AuthDecision::Allow(principal) => principal,
        _ => return Err(StatusCode::UNAUTHORIZED),
    };
    req.extensions_mut().insert(principal);
    Ok(next.run(req).await)
}

/// Wraps the inner service in `tokio::time::timeout(deadline, ...)`.
/// On timeout, returns 503 + `deadline exceeded` body. Runs as the
/// OUTERMOST request-side layer so all other middleware and the
/// handler itself are bounded by the same deadline.
pub async fn request_deadline(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::http::HttpState>>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    let deadline = state.config.request_deadline;
    let resp = tokio::time::timeout(deadline, next.run(req)).await;
    match resp {
        Ok(response) => response,
        Err(_elapsed) => {
            let body = format!("deadline exceeded after {:?}", deadline);
            let mut response = Response::new(axum::body::Body::from(body));
            *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
            response.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("text/plain; charset=utf-8"),
            );
            response
        }
    }
}

#[cfg(test)]
mod deadline_tests {
    use super::*;
    use axum::Router;
    use axum::routing::get;
    use std::sync::Arc;
    use std::time::Duration;
    use tower_service::Service;

    async fn slow_stub() -> Response {
        tokio::time::sleep(Duration::from_secs(2)).await;
        Response::new(axum::body::Body::empty())
    }

    #[tokio::test]
    async fn request_deadline_with_tiny_budget_returns_503() {
        // Default config has 120s; override by mutating the inner
        // HttpState (we hold the only Arc reference here).
        let mut cfg = crate::http::config::HttpConfig::default_for_test();
        cfg.request_deadline = Duration::from_millis(1);
        cfg.shutdown_grace = Duration::from_millis(1);
        let mut state = crate::http::HttpState::default_for_test().await;
        let inner = std::sync::Arc::get_mut(&mut state).expect("single owner");
        Arc::get_mut(&mut inner.core)
            .expect("single core owner")
            .config = cfg;
        let mut svc =
            Router::new()
                .route("/", get(slow_stub))
                .layer(axum::middleware::from_fn_with_state(
                    state,
                    request_deadline,
                ));
        let req = axum::http::Request::builder()
            .uri("/")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = svc.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::{get, post};
    use tower_service::Service;

    async fn mcp_stub() -> &'static str {
        "ok"
    }

    fn router() -> Router {
        Router::new()
            .route("/mcp", post(mcp_stub))
            .route("/", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(reject_non_post_mcp))
    }

    #[tokio::test]
    async fn get_on_mcp_returns_405_from_middleware() {
        let mut r = router();
        let req = Request::builder()
            .method(Method::GET)
            .uri("/mcp")
            .body(Body::empty())
            .unwrap();
        let resp = r.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"POST required");
    }

    #[tokio::test]
    async fn delete_on_mcp_returns_405_from_middleware() {
        let mut r = router();
        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/mcp")
            .body(Body::empty())
            .unwrap();
        let resp = r.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"POST required");
    }

    #[tokio::test]
    async fn get_on_other_path_is_allowed() {
        let mut r = router();
        let req = Request::builder()
            .method(Method::GET)
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = r.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}

// =======================================================================
// SSE response header injection (added in Task 3.7)
// =======================================================================

use axum::http::header::HeaderValue;

const NO_CACHE: HeaderValue = HeaderValue::from_static("no-cache");
const NO_BUFFERING: HeaderValue = HeaderValue::from_static("no");

/// Inject `Cache-Control: no-cache` and `X-Accel-Buffering: no` on
/// responses whose `Content-Type` starts with `text/event-stream`.
/// All other responses pass through untouched. Runs as the OUTERMOST
/// layer so it observes the final response after every other
/// middleware.
pub async fn inject_sse_headers(req: axum::extract::Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    let is_sse = resp
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("text/event-stream"));
    if is_sse {
        let headers = resp.headers_mut();
        headers
            .entry(axum::http::header::CACHE_CONTROL)
            .or_insert(NO_CACHE);
        headers.entry("x-accel-buffering").or_insert(NO_BUFFERING);
    }
    resp
}

#[cfg(test)]
mod sse_headers_tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::header;
    use axum::routing::get;
    use tower_service::Service;

    async fn sse_stub() -> (
        StatusCode,
        [(header::HeaderName, &'static str); 1],
        &'static str,
    ) {
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/event-stream")],
            "data: {}\n\n",
        )
    }

    async fn json_stub() -> (
        StatusCode,
        [(header::HeaderName, &'static str); 1],
        &'static str,
    ) {
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            "{}",
        )
    }

    #[tokio::test]
    async fn sse_responses_get_no_cache_and_no_buffering_headers() {
        let mut r = Router::new()
            .route("/sse", get(sse_stub))
            .layer(axum::middleware::from_fn(inject_sse_headers));
        let req = axum::http::Request::builder()
            .uri("/sse")
            .body(Body::empty())
            .unwrap();
        let resp = r.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("cache-control")
                .map(|v| v.to_str().unwrap()),
            Some("no-cache")
        );
        assert_eq!(
            resp.headers()
                .get("x-accel-buffering")
                .map(|v| v.to_str().unwrap()),
            Some("no")
        );
    }

    #[tokio::test]
    async fn json_responses_are_not_modified() {
        let mut r = Router::new()
            .route("/json", get(json_stub))
            .layer(axum::middleware::from_fn(inject_sse_headers));
        let req = axum::http::Request::builder()
            .uri("/json")
            .body(Body::empty())
            .unwrap();
        let resp = r.call(req).await.unwrap();
        assert!(resp.headers().get("x-accel-buffering").is_none());
        assert!(resp.headers().get("cache-control").is_none());
    }
}

/// Host/Origin allowlist middleware. Reads `allowed_hosts` and
/// `allowed_origins` from `HttpState`. Forwarding headers are honored
/// only when the request's peer IP matches a `trusted_proxy_cidrs`
/// entry; otherwise the bare `Host` header is used (defense against
/// spoofed `X-Forwarded-Host` from arbitrary clients).
pub async fn host_origin(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::http::HttpState>>,
    req: axum::extract::Request,
    next: Next,
) -> Result<Response, (StatusCode, &'static str)> {
    let headers = req.headers();
    let peer = req
        .extensions()
        .get::<axum::extract::connect_info::ConnectInfo<std::net::SocketAddr>>()
        .map(|info| info.0.ip());
    let trusted_peer = peer.is_some_and(|ip| {
        state
            .config
            .trusted_proxy_cidrs
            .iter()
            .any(|c| c.contains(ip))
    });
    let host_header = if trusted_peer {
        headers
            .get("x-forwarded-host")
            .or_else(|| headers.get("host"))
    } else {
        headers.get("host")
    };
    let host = host_header.and_then(|h| h.to_str().ok()).unwrap_or("");
    if !state
        .config
        .allowed_hosts
        .iter()
        .any(|h| h.eq_ignore_ascii_case(host))
    {
        return Err((StatusCode::FORBIDDEN, "host not allowed"));
    }
    if let Some(origin) = headers.get("origin").and_then(|h| h.to_str().ok())
        && !state.config.allowed_origins.iter().any(|o| o == origin)
    {
        return Err((StatusCode::FORBIDDEN, "origin not allowed"));
    }
    Ok(next.run(req).await)
}

#[cfg(test)]
mod host_origin_tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use tower_service::Service;

    async fn echo() -> &'static str {
        "ok"
    }

    async fn router_with_host_origin() -> Router {
        let state = crate::http::HttpState::default_for_test().await;
        Router::new()
            .route("/", get(echo))
            .layer(axum::middleware::from_fn_with_state(state, host_origin))
    }

    #[tokio::test]
    async fn rejects_disallowed_origin() {
        let mut r = router_with_host_origin().await;
        let req = Request::builder()
            .uri("/")
            .header("host", "localhost")
            .header("origin", "https://evil.example")
            .body(Body::empty())
            .unwrap();
        let resp = r.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn allows_missing_origin_for_non_browser() {
        let mut r = router_with_host_origin().await;
        let req = Request::builder()
            .uri("/")
            .header("host", "localhost")
            .body(Body::empty())
            .unwrap();
        let resp = r.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn rejects_disallowed_host() {
        let mut r = router_with_host_origin().await;
        let req = Request::builder()
            .uri("/")
            .header("host", "evil.example")
            .body(Body::empty())
            .unwrap();
        let resp = r.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}

#[cfg(test)]
mod auth_tests {
    use super::*;
    use axum::Router;
    use axum::routing::post;
    use tower_service::Service;

    async fn accept_any(_: axum::extract::Request) -> Response {
        Response::new(axum::body::Body::empty())
    }

    #[tokio::test]
    async fn missing_bearer_returns_401() {
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
    }

    #[tokio::test]
    async fn non_bearer_scheme_returns_401() {
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
    }
}
