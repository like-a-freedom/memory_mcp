//! Outer-most request deadline.
//!
//! Wraps the inner service in `tokio::time::timeout(deadline, ...)`. On
//! timeout, returns 408 + `deadline exceeded` body. Runs as the
//! OUTERMOST request-side layer so all other middleware and the
//! handler itself are bounded by the same deadline.

use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;

use crate::http::HttpState;

/// Wrap the request in the configured deadline.
pub async fn request_deadline(
    axum::extract::State(state): axum::extract::State<Arc<HttpState>>,
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
            *response.status_mut() = StatusCode::REQUEST_TIMEOUT;
            response.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("text/plain; charset=utf-8"),
            );
            response
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::routing::get;
    use std::time::Duration;
    use tower_service::Service;

    async fn slow_stub() -> Response {
        tokio::time::sleep(Duration::from_secs(2)).await;
        Response::new(axum::body::Body::empty())
    }

    #[tokio::test]
    async fn request_deadline_with_tiny_budget_returns_408() {
        // Default config has 120s; override by mutating the inner
        // HttpState (we hold the only Arc reference here).
        let mut cfg = crate::http::config::HttpConfig::default_for_test();
        cfg.request_deadline = Duration::from_millis(1);
        cfg.shutdown_grace = Duration::from_millis(1);
        let mut state = crate::http::HttpState::default_for_test().await;
        let inner = std::sync::Arc::get_mut(&mut state).expect("single owner");
        inner.config = cfg;
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
        assert_eq!(resp.status(), StatusCode::REQUEST_TIMEOUT);
    }
}
