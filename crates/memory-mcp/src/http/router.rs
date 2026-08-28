//! Top-level axum router builder.

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

use super::HttpState;

pub fn build_router(state: Arc<HttpState>) -> Router {
    Router::new()
        .route("/health/live", get(super::health::live))
        .route("/health/ready", get(super::health::ready))
        .route("/mcp", post(super::transport::mcp_handler))
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
        .with_state(state)
}
