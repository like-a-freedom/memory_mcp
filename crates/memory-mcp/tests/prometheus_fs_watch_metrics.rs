//! Integration tests for Prometheus metric recording in filesystem watch.
//!
//! Verifies the exact metric families render when the `prometheus` feature is
//! enabled and that no forbidden identifier (path, hash, episode id) appears
//! as a label or value.

#![cfg(feature = "prometheus")]

use std::sync::OnceLock;

use metrics::{counter, gauge, histogram};
use metrics_exporter_prometheus::PrometheusBuilder;

use memory_mcp::observability::{
    METRIC_FS_WATCH_DEGRADED, METRIC_FS_WATCH_INFLIGHT, METRIC_FS_WATCH_QUEUE_DEPTH,
    METRIC_FS_WATCH_RETRIES_TOTAL, METRIC_FS_WATCH_REVISION_DURATION_SECONDS,
    METRIC_FS_WATCH_REVISIONS_TOTAL, METRIC_FS_WATCH_SCAN_FILES_TOTAL,
};

fn render_handle() -> &'static metrics_exporter_prometheus::PrometheusHandle {
    static HANDLE: OnceLock<metrics_exporter_prometheus::PrometheusHandle> = OnceLock::new();
    HANDLE.get_or_init(|| {
        PrometheusBuilder::new()
            .install_recorder()
            .expect("Prometheus recorder installs once")
    })
}

#[test]
fn filesystem_watch_metric_families_are_exact_and_bounded() {
    let handle = render_handle();

    // Emit every family directly with bounded labels.
    counter!(METRIC_FS_WATCH_REVISIONS_TOTAL, "outcome" => "processed").increment(1);
    counter!(METRIC_FS_WATCH_RETRIES_TOTAL, "stage" => "ingest", "reason" => "model").increment(1);
    counter!(METRIC_FS_WATCH_RETRIES_TOTAL, "stage" => "extract", "reason" => "timeout").increment(1);
    counter!(METRIC_FS_WATCH_SCAN_FILES_TOTAL, "outcome" => "enqueued").increment(1);
    counter!(METRIC_FS_WATCH_SCAN_FILES_TOTAL, "outcome" => "skipped_symlink").increment(1);
    gauge!(METRIC_FS_WATCH_QUEUE_DEPTH).set(4.0);
    gauge!(METRIC_FS_WATCH_INFLIGHT).set(1.0);
    gauge!(METRIC_FS_WATCH_DEGRADED).set(1.0);
    histogram!(METRIC_FS_WATCH_REVISION_DURATION_SECONDS, "outcome" => "processed").record(0.5);

    let output = handle.render();

    assert!(
        output.contains("memory_fs_watch_revisions_total{"),
        "revisions family: {output}"
    );
    assert!(output.contains("outcome=\"processed\""));
    assert!(
        output.contains("memory_fs_watch_retries_total{"),
        "retries family: {output}"
    );
    assert!(output.contains("stage=\"ingest\"") && output.contains("reason=\"model\""));
    assert!(output.contains("stage=\"extract\"") && output.contains("reason=\"timeout\""));
    assert!(
        output.contains("memory_fs_watch_scan_files_total{"),
        "scan family: {output}"
    );
    assert!(output.contains("outcome=\"enqueued\""));
    assert!(output.contains("outcome=\"skipped_symlink\""));
    assert!(output.contains("memory_fs_watch_queue_depth"));
    assert!(output.contains("memory_fs_watch_inflight"));
    assert!(output.contains("memory_fs_watch_degraded"));
    assert!(
        output.contains("memory_fs_watch_revision_duration_seconds"),
        "duration histogram: {output}"
    );
}

#[test]
fn no_identifier_leaks_into_filesystem_watch_labels() {
    let handle = render_handle();

    counter!(METRIC_FS_WATCH_REVISIONS_TOTAL, "outcome" => "processed").increment(1);
    counter!(METRIC_FS_WATCH_RETRIES_TOTAL, "stage" => "ingest", "reason" => "storage").increment(1);
    counter!(METRIC_FS_WATCH_SCAN_FILES_TOTAL, "outcome" => "enqueued").increment(1);

    let output = handle.render();

    // ADR-0050: paths, hashes, episode ids, and error text are never labels.
    for forbidden in [
        "docs/spec.md",
        "inbox_revision:",
        "episode:",
        "fs:",
        "content_sha256",
        "relative_path",
        "last_error",
    ] {
        assert!(
            !output.contains(forbidden),
            "forbidden identifier `{forbidden}` appears in Prometheus output:\n{output}"
        );
    }
}
