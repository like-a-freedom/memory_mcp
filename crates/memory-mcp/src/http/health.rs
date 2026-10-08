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
    let (status, readiness) = if state.shutdown.is_shutting_down() {
        (StatusCode::SERVICE_UNAVAILABLE, "shutting_down")
    } else if state.admission.is_closed() {
        (StatusCode::SERVICE_UNAVAILABLE, "admission_closed")
    } else if !state.registry.ping().await {
        (StatusCode::SERVICE_UNAVAILABLE, "registry_unreachable")
    } else {
        (StatusCode::OK, "ready")
    };
    // A probe is cheap and frequent; a transition is an event. Log only when the
    // state actually changed, so a 15-second liveness poll does not flood the
    // stream and a real change is not buried in it.
    if state.readiness.record(readiness) {
        let level = if readiness == "ready" {
            crate::logging::LogLevel::Info
        } else {
            crate::logging::LogLevel::Warn
        };
        crate::logging::emit(
            std::collections::HashMap::from([
                (
                    "op".to_string(),
                    serde_json::Value::String("http.readiness.changed".to_string()),
                ),
                (
                    "state".to_string(),
                    serde_json::Value::String(readiness.to_string()),
                ),
            ]),
            level,
        );
    }
    let body = json!({"status": readiness}).to_string();
    (
        status,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A probe that finds the same state as the last one is not an event; the
    /// first probe, from "unknown", is.
    #[tokio::test]
    async fn a_readiness_transition_is_logged_once() {
        // Serialised against the sibling readiness test: every live capture
        // receives every rendered line, so two tests emitting readiness
        // transitions would read each other's.
        let _serialized = crate::logging::capture::SERIAL.lock().await;
        let state = HttpState::default_for_test().await;
        let sink = crate::logging::capture::install();

        let first = ready(State(state.clone())).await.into_response();
        assert_eq!(first.status(), StatusCode::OK);
        let after_first = sink.lines();
        assert!(
            after_first
                .iter()
                .any(|line| line.contains("op=http.readiness.changed")
                    && line.contains("state=ready")),
            "the first transition must be logged: {after_first:?}"
        );

        sink.clear();
        let second = ready(State(state)).await.into_response();
        assert_eq!(second.status(), StatusCode::OK);
        assert!(
            !sink
                .lines()
                .iter()
                .any(|line| line.contains("op=http.readiness.changed")),
            "a repeat probe is not a transition: {:?}",
            sink.lines()
        );
    }

    /// A probe that finds the process degraded reports the degraded state, so a
    /// readiness flap is visible rather than only its 503 body.
    #[tokio::test]
    async fn a_degraded_probe_logs_the_degraded_state() {
        let _serialized = crate::logging::capture::SERIAL.lock().await;
        let state = HttpState::default_for_test().await;
        // The first probe reaches `ready`, so the next refusal is a real
        // transition rather than the initial unknown -> degraded one.
        let _ = ready(State(state.clone())).await;
        state.admission.close();
        let sink = crate::logging::capture::install();

        let response = ready(State(state)).await.into_response();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let recorded = sink.lines();
        assert!(
            recorded
                .iter()
                .any(|line| line.contains("op=http.readiness.changed")
                    && line.contains("state=admission_closed")
                    && line.contains("WARN")),
            "a degraded transition must be logged at WARN: {recorded:?}"
        );
    }
}
