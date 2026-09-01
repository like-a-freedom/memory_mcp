//! Prometheus scrape surface for the HTTP profile.
//!
//! The HTTP profile serves metrics on its own axum `/metrics` route.
//! The recorder is installed exactly once at startup; the stdio
//! profile's `MEMORY_PROMETHEUS_LISTEN_ADDR` env var is rejected
//! because two scrape surfaces for one recorder is a configuration
//! error.

#[cfg(feature = "prometheus")]
use crate::error::MemoryError;

/// Install the process-wide recorder and return its render handle.
///
/// Fails when a recorder was already installed (e.g. something else
/// called `metrics_exporter_prometheus` install paths in this
/// process) — the HTTP composition root treats that as a startup
/// error, never a panic.
#[cfg(feature = "prometheus")]
pub fn install_recorder() -> Result<metrics_exporter_prometheus::PrometheusHandle, MemoryError> {
    metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .map_err(|err| {
            MemoryError::ConfigInvalid(format!(
                "failed to install Prometheus recorder for /metrics: {err}"
            ))
        })
}

/// Reject the stdio-profile listener env var in HTTP mode: the HTTP
/// profile serves metrics on its own router and cannot share the
/// recorder with a second listener.
#[cfg(feature = "prometheus")]
pub fn validate_no_listener_env() -> Result<(), MemoryError> {
    match std::env::var(crate::observability::ENV_PROMETHEUS_LISTEN_ADDR) {
        Ok(v) if !v.trim().is_empty() => Err(MemoryError::ConfigInvalid(format!(
            "{} must not be set in the HTTP profile; metrics are served on /metrics",
            crate::observability::ENV_PROMETHEUS_LISTEN_ADDR
        ))),
        _ => Ok(()),
    }
}

/// `/metrics` handler. Renders the recorder's current state with the
/// Prometheus text exposition format.
#[cfg(feature = "prometheus")]
pub async fn prometheus(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::http::HttpState>>,
) -> (
    axum::http::StatusCode,
    [(axum::http::header::HeaderName, &'static str); 1],
    String,
) {
    let body = state
        .metrics_handle
        .as_ref()
        .map(|h| h.render())
        .unwrap_or_default();
    (
        axum::http::StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        body,
    )
}

/// No-op `/metrics` handler when the `prometheus` feature is off.
#[cfg(not(feature = "prometheus"))]
pub async fn prometheus() -> (axum::http::StatusCode, &'static str) {
    (axum::http::StatusCode::NOT_FOUND, "metrics disabled")
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "prometheus")]
    #[tokio::test]
    async fn metrics_route_returns_prometheus_text() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower_service::Service;

        let state = crate::http::HttpState::default_for_test().await;
        let router = crate::http::router::build_router(state);
        let req = Request::builder()
            .uri("/metrics")
            .header("host", "localhost")
            .body(Body::empty())
            .unwrap();
        let mut svc = router;
        let resp = svc.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(std::str::from_utf8(&body).is_ok());
    }
}
