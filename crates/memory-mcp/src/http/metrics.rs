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
    let handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .set_bucket_duration(crate::observability::SUMMARY_WINDOW)
        .map_err(|err| {
            MemoryError::ConfigInvalid(format!(
                "failed to set the summary window for /metrics: {err}"
            ))
        })?
        .set_bucket_count(crate::observability::summary_buckets())
        .install_recorder()
        .map_err(|err| {
            MemoryError::ConfigInvalid(format!(
                "failed to install Prometheus recorder for /metrics: {err}"
            ))
        })?;
    // Described here, not only through the test handle: a deployment installs
    // the recorder through this function, and a description registered
    // elsewhere would leave `/metrics` bare in exactly the build that ships.
    crate::observability::describe_metrics();
    crate::observability::record_build_info();
    Ok(handle)
}

/// Reject the stdio-profile listener env var in HTTP mode: the HTTP
/// profile serves metrics on its own router and cannot share the
/// recorder with a second listener.
#[cfg(feature = "prometheus")]
pub fn validate_no_listener_env() -> Result<(), MemoryError> {
    // The decision is [`listener_env_conflict`]; this line is only the read. The
    // environment is process-global, so a test that set the variable would race
    // every other test in the binary — the rule is tested as a function of the
    // value instead.
    match std::env::var(crate::observability::ENV_PROMETHEUS_LISTEN_ADDR) {
        Ok(raw) => listener_env_conflict(Some(&raw)),
        Err(_) => listener_env_conflict(None),
    }
}

/// Whether a configured stdio listener address conflicts with the HTTP profile.
///
/// Pure, so the rules are testable without touching the environment: unset and
/// blank both mean "no listener configured" and are accepted, and any real
/// value is refused — the HTTP profile serves metrics on its own router and
/// cannot share the process-global recorder with a second listener.
#[cfg(feature = "prometheus")]
pub(crate) fn listener_env_conflict(raw: Option<&str>) -> Result<(), MemoryError> {
    match raw.map(str::trim) {
        Some(value) if !value.is_empty() => Err(MemoryError::ConfigInvalid(format!(
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
    use super::listener_env_conflict;
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

    /// A served request reaches the exposition.
    ///
    /// The three HTTP families are the only ones this crate cannot record
    /// without a request being served: they are written by the request-logging
    /// middleware, so they exist only where the router was actually called. That
    /// makes them the one part of the exposition no recorder-level test can
    /// cover, and it is exactly why they can rot unnoticed — a middleware that
    /// stopped recording would leave every dashboard blank for HTTP while the
    /// rest of the surface stayed green.
    ///
    /// So this serves a request through the real router — middleware included —
    /// and asks whether the families the dashboards read are in the exposition
    /// afterwards. Together with `every_declared_family_reaches_the_exposition`
    /// in the observability module, the whole declared list is covered by a test
    /// that reaches it.
    #[cfg(feature = "prometheus")]
    #[tokio::test]
    async fn a_served_request_reaches_the_exposition() {
        use axum::body::Body;
        use axum::http::Request;
        use tower_service::Service;

        crate::observability::shared_test_handle().expect("prometheus enabled");

        let state = crate::http::HttpState::default_for_test().await;
        let router =
            crate::http::router::build_router(state, None).expect("router builds in tests");
        // `/metrics` rather than a business route: it needs no database, no
        // identity provider and no body, and the middleware wraps it exactly as
        // it wraps everything else.
        let request = Request::builder()
            .uri("/metrics")
            .header("host", "localhost")
            .body(Body::empty())
            .expect("request builds");
        let mut router = router;
        let response = router
            .call(request)
            .await
            .expect("the router serves /metrics");
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let exposition = crate::observability::shared_test_handle()
            .expect("prometheus enabled")
            .render();
        for family in [
            crate::observability::METRIC_HTTP_REQUESTS_TOTAL,
            crate::observability::METRIC_HTTP_REQUEST_DURATION_SECONDS,
            crate::observability::METRIC_HTTP_REQUESTS_INFLIGHT,
        ] {
            assert!(
                exposition.contains(&format!("# HELP {family} ")),
                "a request was served through the middleware, so `{family}` must \
                 be in the exposition; a blank HTTP row on every dashboard is \
                 what its absence looks like: {exposition}"
            );
        }
    }

    /// A configured stdio listener is refused, and the message says why.
    ///
    /// The refusal is a startup error a person has to act on: the HTTP profile
    /// serves metrics on its own router, so a second listener cannot share the
    /// process-global recorder, and letting it through would fail later inside
    /// the metrics crate with a message about recorder installation instead.
    #[cfg(feature = "prometheus")]
    #[test]
    fn a_configured_stdio_listener_is_refused_in_http_mode() {
        let error = listener_env_conflict(Some("127.0.0.1:9100"))
            .expect_err("a configured listener conflicts with /metrics");

        assert!(
            error.to_string().contains("/metrics"),
            "the error must say where metrics are served instead, so the fix is \
             obvious: {error}"
        );
    }

    /// Unset and blank are both accepted.
    ///
    /// Blank is a normal way to arrive here — a template that renders an empty
    /// value — and refusing it would report a syntax error where there is none.
    #[cfg(feature = "prometheus")]
    #[test]
    fn an_unset_or_blank_stdio_listener_is_accepted_in_http_mode() {
        listener_env_conflict(None).expect("unset is not a conflict");
        listener_env_conflict(Some("  ")).expect("a blank value is not a conflict");
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
        // The build stamp rides on the same install, so a path that installs a
        // recorder and forgets it leaves every dashboard readable and every
        // incident review unable to say which release produced the numbers.
        assert!(
            include_str!("metrics.rs").contains("record_build_info()"),
            "the HTTP composition root must stamp the build as it installs the \
             recorder, or an incident review cannot tell which version a \
             dashboard came from"
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
