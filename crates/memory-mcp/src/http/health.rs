//! `/health/live` and `/health/ready` handlers.
//!
//! `live` is a trivial 200 OK. `ready` returns 503 when the process is
//! shutting down, admission is closed, or the registry probe fails.
//! Phase 3 uses the stub `RegistryHandle::stub()` (always reachable);
//! Phase 4 replaces it with a real control-namespace probe.

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
        let router =
            super::super::router::build_router(super::super::HttpState::default_for_test().await);
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
