//! Local admin request deadline.
//!
//! The generic [`super::deadline::request_deadline`] runs on the base
//! router and therefore does not cover routes merged afterwards. It also
//! reports a timeout as `408 Request Timeout`, whereas the local admin
//! contract in spec §8 states that **local deadline exhaustion is
//! `503`**: the caller cannot distinguish a slow store from an
//! unavailable one, and a browser form should surface "try again"
//! rather than a protocol-level timeout.

use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;

use crate::http::HttpState;

/// Bound a local admin request by the configured deadline, returning
/// `503 temporarily_unavailable` on exhaustion.
pub async fn local_admin_deadline(
    axum::extract::State(state): axum::extract::State<Arc<HttpState>>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    let deadline = state.config.request_deadline;
    match tokio::time::timeout(deadline, next.run(req)).await {
        Ok(response) => response,
        Err(_elapsed) => {
            let mut response = (
                StatusCode::SERVICE_UNAVAILABLE,
                [
                    (axum::http::header::CONTENT_TYPE, "application/json"),
                    (axum::http::header::CACHE_CONTROL, "no-store"),
                ],
                "{\"error\":{\"code\":\"temporarily_unavailable\",\"message\":\"temporarily unavailable\"}}",
            )
                .into_response();
            response.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                axum::http::HeaderValue::from_static("1"),
            );
            response
        }
    }
}

use axum::response::IntoResponse;

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
    async fn deadline_exhaustion_is_service_unavailable() {
        let mut cfg = crate::http::config::HttpConfig::default_for_test();
        cfg.request_deadline = Duration::from_millis(1);
        let mut state = crate::http::HttpState::default_for_test().await;
        let inner = std::sync::Arc::get_mut(&mut state).expect("single owner");
        inner.config = cfg;
        let mut svc =
            Router::new()
                .route("/", get(slow_stub))
                .layer(axum::middleware::from_fn_with_state(
                    state,
                    local_admin_deadline,
                ));
        let req = axum::http::Request::builder()
            .uri("/")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = svc.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
