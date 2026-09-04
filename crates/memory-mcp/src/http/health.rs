//! `/health/live` and `/health/ready` handlers.
//!
//! `live` is a trivial 200 OK. `ready` returns 503 when the process is
//! shutting down, admission is closed, or the registry probe fails.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json::json;

use super::HttpState;

pub async fn live() -> &'static str {
    "ok"
}

pub async fn ready(State(state): State<Arc<HttpState>>) -> impl IntoResponse {
    let (status, body) = if state.shutdown.is_shutting_down() {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"status": "shutting_down"}),
        )
    } else if state.admission.is_closed() {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"status": "admission_closed"}),
        )
    } else if !state.registry.ping().await {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"status": "registry_unreachable"}),
        )
    } else {
        (StatusCode::OK, json!({"status": "ready"}))
    };
    let body = body.to_string();
    (
        status,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_service::Service;

    #[tokio::test]
    async fn ready_returns_ok_when_registry_reachable() {
        // Build a state whose registry is the in-memory backend
        // (which always reports reachable), then construct the
        // router. The `default_for_test` state builds against
        // the production Surreal registry; this test swaps in
        // the in-memory backend to exercise the
        // registry-reachable path.
        let mut state = super::super::HttpState::default_for_test().await;
        let inner = std::sync::Arc::get_mut(&mut state).expect("single owner");
        inner.registry = super::super::registry::RegistryHandle::in_memory();
        let router = super::super::router::build_router(state);
        let mut svc = router;
        let req = axum::http::Request::builder()
            .uri("/health/ready")
            .header("host", "localhost")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = svc.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
