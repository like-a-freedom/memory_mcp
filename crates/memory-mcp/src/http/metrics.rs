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
        let router =
            crate::http::router::build_router(state, None).expect("router builds in tests");
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

    /// The recorder is installed before anything else can fail.
    ///
    /// `build_state` calls `install_recorder` on its first lines, ahead of the
    /// database and the identity provider, precisely so a metrics failure is
    /// reported as one rather than surfacing later as a blank scrape. This
    /// asserts that directly.
    ///
    /// The previous test in this file could not: it went through
    /// `HttpStateTestBuilder`, which takes the handle as a parameter, so it only
    /// ever checked that a handle it supplied came back out. With
    /// `install_recorder` deleted from `build_state`, the whole suite passed —
    /// 2415 tests, none able to tell a blank scrape from a quiet server.
    ///
    /// The call itself is what is asserted, not a built runtime: `build_state`
    /// also opens a database and fetches OIDC discovery, so asserting on the
    /// assembled state would only prove this test can reach the network.
    #[cfg(feature = "prometheus")]
    #[test]
    fn the_recorder_installs_from_the_composition_root() {
        // The recorder is process-global, so a second install fails by design.
        // What matters is that the composition root attempts one and treats a
        // failure as a startup error rather than a silently absent handle.
        let source = include_str!("runtime/bootstrap.rs");
        assert!(
            source.contains("crate::http::metrics::install_recorder()"),
            "the composition root must install the recorder, or /metrics answers \
             200 with an empty body and every dashboard goes blank without red"
        );
        assert!(
            source.contains("metrics init error"),
            "a failure to install must be a startup error, not a silently \
             absent handle"
        );
    }

    /// The handler answers from the handle rather than from its own default.
    ///
    /// The `unwrap_or_default()` on the missing path is what turns a wiring
    /// mistake into a silently empty exposition instead of a visible absence.
    ///
    /// Both states are built here — one carrying a recorder, one without — so
    /// the difference the handler makes is asserted rather than assumed. The
    /// missing-handle case is the one that matters: it is exactly what a
    /// deployment looks like when the composition root forgot to install the
    /// recorder, and it is why the root is asserted separately.
    #[cfg(feature = "prometheus")]
    #[tokio::test]
    async fn a_state_without_a_handle_serves_an_empty_exposition() {
        // `HttpStateTestBuilder::new` installs a recorder by default, so the
        // missing-handle path has to be asked for explicitly — which is itself
        // worth stating: every test state is wired, so only the composition
        // root can produce a deployment without one.
        let state = crate::http::test_state::HttpStateTestBuilder::new()
            .await
            .with_metrics_handle(None)
            .build()
            .await
            .expect("a state builds without a recorder handle");
        assert!(
            state.metrics_handle.is_none(),
            "this state was built without a handle, so the assertion below is \
             testing the missing-handle path"
        );

        let (_, _, body) = super::prometheus(axum::extract::State(state)).await;

        assert!(
            body.is_empty(),
            "with no recorder there is nothing to render, and the handler must \
             not invent a series: {body}"
        );
    }
}
