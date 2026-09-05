//! SSE response header injection.
//!
//! Inject `Cache-Control: no-cache` and `X-Accel-Buffering: no` on
//! responses whose `Content-Type` starts with `text/event-stream`.
//! All other responses pass through untouched. Runs as the OUTERMOST
//! layer so it observes the final response after every other
//! middleware.

use axum::http::header::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;

const NO_CACHE: HeaderValue = HeaderValue::from_static("no-cache");
const NO_BUFFERING: HeaderValue = HeaderValue::from_static("no");

/// Inject `Cache-Control: no-cache` and `X-Accel-Buffering: no` on
/// responses whose `Content-Type` starts with `text/event-stream`.
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
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{StatusCode, header};
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
