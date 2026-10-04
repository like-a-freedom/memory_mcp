//! HTTP readiness route integration against the real in-memory registry.
#![cfg(all(feature = "streamable-http", feature = "test-fixtures"))]

use std::sync::Arc;

use axum::body::Body;
use memory_mcp::http::HttpState;
use memory_mcp::http::router::build_router;
use memory_mcp::http::test_state::HttpStateTestBuilder;
use serde_json::Value;
use tower_service::Service;

async fn state() -> Arc<HttpState> {
    HttpStateTestBuilder::new()
        .await
        .build()
        .await
        .expect("assemble HTTP state over the in-memory registry")
}

async fn readiness_response(state: Arc<HttpState>) -> axum::response::Response {
    let mut router = build_router(state, None).expect("build HTTP router");
    let request = axum::http::Request::builder()
        .uri("/health/ready")
        .header("host", "localhost")
        .body(Body::empty())
        .expect("readiness request");

    router.call(request).await.expect("route response")
}

async fn readiness_body(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("readiness body reads");
    serde_json::from_slice(&bytes).expect("readiness JSON")
}

#[tokio::test]
async fn readiness_route_reports_a_reachable_registry() {
    let response = readiness_response(state().await).await;

    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert_eq!(readiness_body(response).await["status"], "ready");
}

#[tokio::test]
async fn readiness_route_refuses_new_work_after_shutdown_begins() {
    let state = state().await;
    state.shutdown.begin();

    let response = readiness_response(state).await;

    assert_eq!(
        response.status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(readiness_body(response).await["status"], "shutting_down");
}

#[tokio::test]
async fn readiness_route_reports_closed_admission() {
    let state = state().await;
    state.admission.close();

    let response = readiness_response(state).await;

    assert_eq!(
        response.status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(readiness_body(response).await["status"], "admission_closed");
}

#[tokio::test]
async fn readiness_reports_shutdown_before_closed_admission() {
    let state = state().await;
    state.admission.close();
    state.shutdown.begin();

    let response = readiness_response(state).await;

    assert_eq!(readiness_body(response).await["status"], "shutting_down");
}
