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
        let router =
            super::super::router::build_router(state, None).expect("router builds in tests");
        let mut svc = router;
        let req = axum::http::Request::builder()
            .uri("/health/ready")
            .header("host", "localhost")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = svc.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// A test state whose registry always reports reachable, so the readiness
    /// verdict depends only on the shutdown and admission flags.
    async fn live_registry_state() -> Arc<HttpState> {
        let mut state = super::super::HttpState::default_for_test().await;
        let inner = Arc::get_mut(&mut state).expect("single owner");
        inner.registry = super::super::registry::RegistryHandle::in_memory();
        state
    }

    /// Render `ready` through `IntoResponse` and return its status.
    async fn ready_status(state: Arc<HttpState>) -> StatusCode {
        ready(State(state)).await.into_response().status()
    }

    #[tokio::test]
    async fn live_reports_ok() {
        assert_eq!(live().await, "ok");
    }

    #[tokio::test]
    async fn ready_reports_200_when_nothing_is_blocking() {
        let state = live_registry_state().await;

        assert_eq!(ready_status(state).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn ready_reports_503_while_shutting_down() {
        let state = live_registry_state().await;
        state.shutdown.begin();

        assert_eq!(ready_status(state).await, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn ready_reports_503_once_admission_is_closed() {
        let state = live_registry_state().await;
        state.admission.close();

        assert_eq!(ready_status(state).await, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn the_shutdown_state_wins_over_the_admission_state() {
        // Both are 503; the body names the first condition, so the ordering
        // has to be pinned rather than inferred from the status alone.
        let state = live_registry_state().await;
        state.admission.close();
        state.shutdown.begin();

        let response = ready(State(state)).await.into_response();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("json body");

        assert_eq!(value["status"], "shutting_down");
    }

    #[tokio::test]
    async fn the_admission_state_is_reported_when_not_shutting_down() {
        let state = live_registry_state().await;
        state.admission.close();

        let response = ready(State(state)).await.into_response();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("json body");

        assert_eq!(value["status"], "admission_closed");
    }
}
