#![cfg(all(feature = "streamable-http", feature = "test-fixtures"))]

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware;
use axum::routing::post;
use bytes::Bytes;
use http_body::{Body as HttpBody, Frame, SizeHint};
use memory_mcp::http::config::HttpConfig;
use memory_mcp::http::metrics::{install_recorder, spawn_upkeep_with_ticks};
use memory_mcp::http::middleware::prevalidate_mcp;
use memory_mcp::http::runtime::memory_snapshot::HttpMemorySnapshotTestFixture;
use memory_mcp::http::test_state::HttpStateTestBuilder;
use tokio::sync::{Notify, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tower_service::Service;

struct FailingBody {
    phase: u8,
}

impl HttpBody for FailingBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let result = match self.phase {
            0 => Ok(Frame::data(Bytes::from_static(b"1234"))),
            _ => Err(std::io::Error::other("synthetic stream failure")),
        };
        self.phase = self.phase.saturating_add(1);
        Poll::Ready(Some(result))
    }

    fn is_end_stream(&self) -> bool {
        false
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

struct PendingAfterData {
    started: Arc<Notify>,
    emitted: bool,
}

impl HttpBody for PendingAfterData {
    type Data = Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if self.emitted {
            Poll::Pending
        } else {
            self.emitted = true;
            self.started.notify_one();
            Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(b"123")))))
        }
    }

    fn is_end_stream(&self) -> bool {
        false
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

fn preflight_request(body: Body) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(body)
        .expect("valid test request")
}

fn exposition_sample(exposition: &str, name: &str) -> Option<f64> {
    exposition.lines().find_map(|line| {
        let (sample_name, value) = line.split_once(' ')?;
        (sample_name == name).then(|| value.parse().ok()).flatten()
    })
}

fn assert_gauge_description(exposition: &str, name: &str, help: &str) {
    assert!(
        exposition.contains(&format!("# HELP {name} {help}")),
        "missing HELP for {name}: {exposition}"
    );
    assert!(
        exposition.contains(&format!("# TYPE {name} gauge")),
        "missing gauge TYPE for {name}: {exposition}"
    );
}

async fn run_tick(tick_tx: &mpsc::UnboundedSender<oneshot::Sender<()>>) {
    let (ack_tx, ack_rx) = oneshot::channel();
    tick_tx
        .send(ack_tx)
        .expect("the upkeep worker is waiting for an injected tick");
    tokio::time::timeout(std::time::Duration::from_secs(1), ack_rx)
        .await
        .expect("the injected tick is processed promptly")
        .expect("the upkeep worker acknowledges the tick");
}

#[tokio::test]
async fn preflight_histogram_records_completed_body_bytes_once() {
    let handle = install_recorder().expect("isolated test process installs one recorder");
    let fixture = HttpMemorySnapshotTestFixture::new()
        .await
        .expect("snapshot source fixture builds");
    let reservation = fixture
        .reserve_preflight(7)
        .expect("preflight reservation is within the test budget");
    let cancellation = CancellationToken::new();
    let (tick_tx, tick_rx) = mpsc::unbounded_channel();
    let upkeep = spawn_upkeep_with_ticks(
        handle.clone(),
        cancellation.clone(),
        tick_rx,
        fixture.source(),
    );
    let mut config = HttpConfig::default_for_test();
    config.preflight_bytes = 7;
    config.body_limit_bytes = 1024;
    let state = HttpStateTestBuilder::new()
        .await
        .with_config(config)
        .with_metrics_handle(Some(handle.clone()))
        .build()
        .await
        .expect("HTTP state with the isolated recorder");
    let mut router = Router::new()
        .route("/mcp", post(|| async { StatusCode::ACCEPTED }))
        .layer(middleware::from_fn_with_state(state, prevalidate_mcp));
    let request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(Body::from(r#"{"a":]}"#))
        .expect("valid test request");

    let response = router.call(request).await.expect("preflight response");
    run_tick(&tick_tx).await;
    let exposition = handle.render();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(exposition.contains(
        "# HELP memory_http_preflight_body_bytes Bytes exposed to the HTTP preflight collector"
    ));
    assert!(exposition.contains("# TYPE memory_http_preflight_body_bytes summary"));
    assert_gauge_description(
        &exposition,
        "memory_http_preflight_reserved_requests",
        "HTTP preflight requests with a live body reservation",
    );
    assert_gauge_description(
        &exposition,
        "memory_http_preflight_reserved_bytes",
        "Bytes currently reserved in the HTTP preflight budget",
    );
    assert_gauge_description(
        &exposition,
        "memory_http_tenant_runtime_count",
        "Resident ready or draining tenant runtimes",
    );
    assert_gauge_description(
        &exposition,
        "memory_http_context_cache_accounted_bytes",
        "Owner-accounted bytes in context caches of resident runtimes",
    );
    assert_gauge_description(
        &exposition,
        "memory_http_query_cache_accounted_bytes",
        "Owner-accounted bytes in query caches of resident runtimes",
    );
    assert_gauge_description(
        &exposition,
        "memory_http_background_embedding_admitted_tasks",
        "Background embedding tasks admitted by the process-wide coordinator",
    );
    assert_gauge_description(
        &exposition,
        "memory_http_background_embedding_running_tasks",
        "Background embedding tasks currently running",
    );
    assert_gauge_description(
        &exposition,
        "memory_http_background_embedding_retained_bytes",
        "Bytes retained by admitted background embedding tasks",
    );
    assert_eq!(
        exposition_sample(&exposition, "memory_http_preflight_reserved_requests"),
        Some(1.0)
    );
    assert_eq!(
        exposition_sample(&exposition, "memory_http_preflight_reserved_bytes"),
        Some(7.0)
    );
    assert_eq!(
        exposition_sample(&exposition, "memory_http_tenant_runtime_count"),
        Some(0.0)
    );
    assert_eq!(
        exposition_sample(&exposition, "memory_http_context_cache_accounted_bytes"),
        Some(0.0)
    );
    assert_eq!(
        exposition_sample(&exposition, "memory_http_query_cache_accounted_bytes"),
        Some(0.0)
    );
    assert_eq!(
        exposition_sample(
            &exposition,
            "memory_http_background_embedding_admitted_tasks"
        ),
        Some(0.0)
    );
    assert_eq!(
        exposition_sample(
            &exposition,
            "memory_http_background_embedding_running_tasks"
        ),
        Some(0.0)
    );
    assert_eq!(
        exposition_sample(
            &exposition,
            "memory_http_background_embedding_retained_bytes"
        ),
        Some(0.0)
    );

    assert_eq!(
        exposition_sample(&exposition, "memory_http_preflight_body_bytes_sum"),
        Some(7.0)
    );
    assert_eq!(
        exposition_sample(&exposition, "memory_http_preflight_body_bytes_count"),
        Some(1.0)
    );

    let partial_refusal = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(Body::from("12345678"))
        .expect("valid partial-refusal request");
    let response = router
        .call(partial_refusal)
        .await
        .expect("refusal response");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let after_refusal = handle.render();
    assert!(after_refusal.contains(
        "# HELP memory_http_preflight_refusals_total HTTP preflight refusals by closed reason"
    ));
    assert!(after_refusal.contains("# TYPE memory_http_preflight_refusals_total counter"));
    assert_eq!(
        exposition_sample(&after_refusal, "memory_http_preflight_body_bytes_sum"),
        Some(7.0)
    );
    assert_eq!(
        exposition_sample(&after_refusal, "memory_http_preflight_body_bytes_count"),
        Some(1.0)
    );
    assert!(after_refusal.lines().any(|line| {
        line.starts_with(
            "memory_http_preflight_refusals_total{reason=\"aggregate_byte_capacity\"} 1",
        )
    }));

    let read_error = preflight_request(Body::new(FailingBody { phase: 0 }));
    let response = router.call(read_error).await.expect("body-read response");
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let after_read_error = handle.render();
    assert_eq!(
        exposition_sample(&after_read_error, "memory_http_preflight_body_bytes_sum"),
        Some(7.0)
    );
    assert_eq!(
        exposition_sample(&after_read_error, "memory_http_preflight_body_bytes_count"),
        Some(1.0)
    );
    assert_eq!(
        after_read_error
            .lines()
            .filter(|line| line.starts_with("memory_http_preflight_refusals_total{"))
            .count(),
        1,
        "stream errors are not capacity or body-limit refusals: {after_read_error}"
    );

    let started = Arc::new(Notify::new());
    let pending_request = preflight_request(Body::new(PendingAfterData {
        started: Arc::clone(&started),
        emitted: false,
    }));
    let mut pending_router = router.clone();
    let pending = tokio::spawn(async move {
        pending_router
            .call(pending_request)
            .await
            .expect("cancelled body collection response")
    });
    started.notified().await;
    pending.abort();
    let _ = pending.await;

    let after_cancel = handle.render();
    assert_eq!(
        exposition_sample(&after_cancel, "memory_http_preflight_body_bytes_count"),
        Some(1.0),
        "cancelled body collection is not a completed observation"
    );
    let retry = router
        .call(preflight_request(Body::from(r#"{"a":]}"#)))
        .await
        .expect("reservation released after cancellation");
    assert_eq!(retry.status(), StatusCode::BAD_REQUEST);
    let after_retry = handle.render();
    assert_eq!(
        exposition_sample(&after_retry, "memory_http_preflight_body_bytes_sum"),
        Some(14.0)
    );
    assert_eq!(
        exposition_sample(&after_retry, "memory_http_preflight_body_bytes_count"),
        Some(2.0)
    );

    drop(reservation);
    cancellation.cancel();
    upkeep
        .await
        .expect("upkeep worker exits after cancellation");
}
