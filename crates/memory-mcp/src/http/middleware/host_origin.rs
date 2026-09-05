//! Host/Origin allowlist middleware.
//!
//! Reads `allowed_hosts` and `allowed_origins` from `HttpState`.
//! Forwarding headers are honored only when the request's peer IP
//! matches a `trusted_proxy_cidrs` entry; otherwise the bare `Host`
//! header is used (defense against spoofed `X-Forwarded-Host` from
//! arbitrary clients).

use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;

use crate::http::HttpState;

/// Enforce the configured host and origin allowlists.
pub async fn host_origin(
    axum::extract::State(state): axum::extract::State<Arc<HttpState>>,
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
mod tests {
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
