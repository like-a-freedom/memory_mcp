//! Top-level axum router builder.

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};

use super::HttpState;

pub fn build_router(state: Arc<HttpState>) -> Router {
    let mcp_route = post(super::transport::mcp_handler).layer(
        axum::middleware::from_fn_with_state(state.clone(), super::middleware::authenticate),
    );
    let router = Router::new()
        .route("/health/live", get(super::health::live))
        .route("/health/ready", get(super::health::ready))
        .route("/mcp", mcp_route)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            super::middleware::request_deadline,
        ))
        .layer(axum::middleware::from_fn(
            super::middleware::reject_non_post_mcp,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            super::middleware::host_origin,
        ))
        .layer(axum::middleware::from_fn(
            super::middleware::inject_sse_headers,
        ))
        .layer(axum::middleware::from_fn(super::logging::request_log));
    #[cfg(feature = "prometheus")]
    let router = router.route("/metrics", get(super::metrics::prometheus));
    router.with_state(state)
}

#[cfg(test)]
pub mod test_helpers {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use tower_service::Service;

    /// Drive a single request through the router. Caller specifies
    /// method, URI, and (method, host) header set used by the
    /// host-origin middleware.
    pub async fn dispatch(
        router: Router,
        method: Method,
        uri: &str,
        headers: &[(&str, &str)],
    ) -> axum::response::Response {
        let mut svc = router;
        let mut b = Request::builder().method(method).uri(uri);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let req = b.body(Body::empty()).expect("request builder");
        svc.call(req).await.expect("dispatch")
    }
}
