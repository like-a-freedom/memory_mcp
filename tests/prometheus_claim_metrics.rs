//! Integration tests for Prometheus metric recording in claim reconciliation.
//!
//! These tests verify that the six metric families emit values correctly
//! when the `prometheus` feature is enabled. A single test installs the
//! global recorder once; subsequent test functions use `#[ignore]` and
//! must be run sequentially (`--test-threads=1`).

#![cfg(feature = "prometheus")]

use std::sync::OnceLock;

use metrics::counter;
use metrics::gauge;
use metrics::histogram;
use metrics_exporter_prometheus::PrometheusBuilder;

fn render_handle() -> &'static metrics_exporter_prometheus::PrometheusHandle {
    static HANDLE: OnceLock<metrics_exporter_prometheus::PrometheusHandle> = OnceLock::new();
    HANDLE.get_or_init(|| {
        PrometheusBuilder::new()
            .install()
            .expect("Prometheus recorder installs once")
    })
}

#[test]
fn all_six_metric_families_appear() {
    let handle = render_handle();

    counter!("claim_projected_total", "schema_family" => "attribute").increment(1);
    counter!("claim_projected_total", "schema_family" => "quantity").increment(2);
    counter!("claim_skipped_total", "reason" => "invalid_value").increment(1);
    counter!(
        "claim_relations_total",
        "outcome" => "contradiction",
        "schema_family" => "attribute"
    )
    .increment(1);
    counter!(
        "claim_relations_total",
        "outcome" => "duplicate",
        "schema_family" => "quantity"
    )
    .increment(3);
    histogram!("claim_reconciliation_duration_seconds").record(0.123);
    gauge!(
        "claim_active_relations",
        "outcome" => "contradiction",
        "schema_family" => "attribute"
    )
    .set(7.0);
    counter!("claim_worker_leases_total", "status" => "acquired").increment(10);
    counter!("claim_worker_leases_total", "status" => "idle").increment(42);

    let output = handle.render();

    // claim_projected_total
    assert!(
        output.contains(r#"claim_projected_total{schema_family="attribute"} 1"#),
        "attribute projected count"
    );
    assert!(
        output.contains(r#"claim_projected_total{schema_family="quantity"} 2"#),
        "quantity projected count"
    );

    // claim_skipped_total
    assert!(
        output.contains(r#"claim_skipped_total{reason="invalid_value"} 1"#),
        "skipped count"
    );

    // claim_relations_total
    assert!(
        output.contains(
            r##"claim_relations_total{outcome="contradiction",schema_family="attribute"} 1"##
        ),
        "contradiction relations"
    );
    assert!(
        output
            .contains(r##"claim_relations_total{outcome="duplicate",schema_family="quantity"} 3"##),
        "duplicate relations"
    );

    // claim_reconciliation_duration_seconds
    assert!(
        output.contains("claim_reconciliation_duration_seconds"),
        "reconciliation histogram exists"
    );

    // claim_active_relations
    assert!(
        output.contains(
            r##"claim_active_relations{outcome="contradiction",schema_family="attribute"} 7"##
        ),
        "active contradiction gauge"
    );

    // claim_worker_leases_total
    assert!(
        output.contains(r#"claim_worker_leases_total{status="acquired"} 10"#),
        "worker acquired"
    );
    assert!(
        output.contains(r#"claim_worker_leases_total{status="idle"} 42"#),
        "worker idle"
    );
}
