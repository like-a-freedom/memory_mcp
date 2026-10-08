#![cfg(all(
    feature = "prometheus",
    feature = "streamable-http",
    feature = "test-fixtures"
))]

fn metric_sample(exposition: &str, name: &str) -> Option<f64> {
    exposition.lines().find_map(|line| {
        let (sample_name, value) = line.split_once(' ')?;
        (sample_name == name).then(|| value.parse().ok()).flatten()
    })
}

#[tokio::test]
async fn upkeep_refreshes_summary_without_scrape() {
    use std::time::Duration;

    use memory_mcp::http::metrics::{install_recorder, spawn_upkeep_with_ticks};
    use memory_mcp::http::runtime::memory_snapshot::HttpMemorySnapshotTestFixture;
    use memory_mcp::observability::{
        METRIC_HTTP_REQUEST_DURATION_SECONDS, SUMMARY_BUCKET_DURATION, summary_buckets,
    };
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    let handle = install_recorder().expect("this isolated test process installs one recorder");
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

    metrics::histogram!(
        METRIC_HTTP_REQUEST_DURATION_SECONDS,
        "method" => "GET",
        "outcome" => "success",
        "route" => "/healthz"
    )
    .record(0.25);

    run_tick(&tick_tx).await;

    let exposition = handle.render();
    assert!(
        exposition.contains(METRIC_HTTP_REQUEST_DURATION_SECONDS),
        "the sample recorded without a scrape must remain exposed after upkeep: {exposition}"
    );
    assert!(
        exposition.contains("quantile=\"0.5\""),
        "the summary quantile must be exposed after upkeep: {exposition}"
    );
    assert_eq!(
        SUMMARY_BUCKET_DURATION * summary_buckets().get(),
        Duration::from_secs(300),
        "five 60-second buckets make a five-minute rolling summary window"
    );
    assert_eq!(
        metric_sample(&exposition, "memory_http_preflight_reserved_requests"),
        Some(1.0)
    );
    assert_eq!(
        metric_sample(&exposition, "memory_http_preflight_reserved_bytes"),
        Some(7.0)
    );
    assert_eq!(
        metric_sample(&exposition, "memory_http_tenant_runtime_count"),
        Some(0.0)
    );
    assert_eq!(
        metric_sample(&exposition, "memory_http_context_cache_accounted_bytes"),
        Some(0.0)
    );
    assert_eq!(
        metric_sample(&exposition, "memory_http_query_cache_accounted_bytes"),
        Some(0.0)
    );
    assert_eq!(
        metric_sample(
            &exposition,
            "memory_http_background_embedding_admitted_tasks"
        ),
        Some(0.0)
    );
    assert_eq!(
        metric_sample(
            &exposition,
            "memory_http_background_embedding_running_tasks"
        ),
        Some(0.0)
    );
    assert_eq!(
        metric_sample(
            &exposition,
            "memory_http_background_embedding_retained_bytes"
        ),
        Some(0.0)
    );

    drop(reservation);
    run_tick(&tick_tx).await;
    let after_release = handle.render();
    assert_eq!(
        metric_sample(&after_release, "memory_http_preflight_reserved_requests"),
        Some(0.0)
    );
    assert_eq!(
        metric_sample(&after_release, "memory_http_preflight_reserved_bytes"),
        Some(0.0)
    );

    cancellation.cancel();
    upkeep
        .await
        .expect("upkeep worker exits after cancellation");
}

async fn run_tick(tick_tx: &tokio::sync::mpsc::UnboundedSender<tokio::sync::oneshot::Sender<()>>) {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    tick_tx
        .send(ack_tx)
        .expect("the upkeep worker is waiting for an injected tick");
    tokio::time::timeout(std::time::Duration::from_secs(1), ack_rx)
        .await
        .expect("the injected tick is processed promptly")
        .expect("the upkeep worker acknowledges the tick");
}
