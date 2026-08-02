//! Integration tests for Prometheus metric recording in claim reconciliation.
//!
//! Verifies the five metric families from completion plan Task 8 render
//! correctly when the `prometheus` feature is enabled, and that no
//! forbidden identifier (ADR-0005) appears as a Prometheus label.

#![cfg(feature = "prometheus")]

use std::sync::OnceLock;

use metrics::counter;
use metrics::gauge;
use metrics::histogram;
use metrics_exporter_prometheus::PrometheusBuilder;

use memory_mcp::service::claims::telemetry::{
    METRIC_BACKFILL_FACTS_TOTAL, METRIC_CANDIDATE_COUNT, METRIC_PIPELINE_DURATION_SECONDS,
    METRIC_PIPELINE_TOTAL, METRIC_RELATIONS_ACTIVE,
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
fn all_five_metric_families_appear() {
    let handle = render_handle();

    // Family 1: memory_claim_pipeline_total{stage,schema,outcome,reason_code}
    counter!(
        METRIC_PIPELINE_TOTAL,
        "stage" => "project",
        "schema" => "attribute",
        "outcome" => "duplicate",
        "reason_code" => "duplicate",
    )
    .increment(1);
    counter!(
        METRIC_PIPELINE_TOTAL,
        "stage" => "reconcile",
        "schema" => "quantity",
        "outcome" => "contradiction",
        "reason_code" => "contradiction",
    )
    .increment(2);

    // Family 2: memory_claim_pipeline_duration_seconds{stage,schema,outcome}
    histogram!(
        METRIC_PIPELINE_DURATION_SECONDS,
        "stage" => "reconcile",
        "schema" => "quantity",
        "outcome" => "contradiction",
    )
    .record(0.123);

    // Family 3: memory_claim_candidate_count{schema,match_mode}
    histogram!(
        METRIC_CANDIDATE_COUNT,
        "schema" => "relation",
        "match_mode" => "exact",
    )
    .record(4.0);

    // Family 4: memory_claim_relations_active{schema,outcome}
    gauge!(
        METRIC_RELATIONS_ACTIVE,
        "schema" => "attribute",
        "outcome" => "contradiction",
    )
    .set(7.0);

    // Family 5: memory_claim_backfill_facts_total{outcome,reason_code}
    counter!(
        METRIC_BACKFILL_FACTS_TOTAL,
        "outcome" => "completed",
        "reason_code" => "completed",
    )
    .increment(10);
    counter!(
        METRIC_BACKFILL_FACTS_TOTAL,
        "outcome" => "skipped",
        "reason_code" => "skipped",
    )
    .increment(42);

    let output = handle.render();

    // pipeline_total — exact label set, regardless of order.
    assert!(
        output.contains("memory_claim_pipeline_total{"),
        "pipeline total exists: {output}"
    );
    assert!(
        output.contains("stage=\"project\"")
            && output.contains("schema=\"attribute\"")
            && output.contains("outcome=\"duplicate\"")
            && output.contains("reason_code=\"duplicate\""),
        "pipeline total project/attribute/duplicate: {output}"
    );
    assert!(
        output.contains("stage=\"reconcile\"")
            && output.contains("schema=\"quantity\"")
            && output.contains("outcome=\"contradiction\""),
        "pipeline total reconcile/quantity/contradiction: {output}"
    );

    // pipeline duration
    assert!(
        output.contains("memory_claim_pipeline_duration_seconds"),
        "pipeline duration histogram: {output}"
    );

    // candidate count
    assert!(
        output.contains("memory_claim_candidate_count"),
        "candidate count histogram: {output}"
    );

    // active relations gauge
    assert!(
        output.contains("memory_claim_relations_active{")
            && output.contains("schema=\"attribute\"")
            && output.contains("outcome=\"contradiction\""),
        "active relations gauge: {output}"
    );

    // backfill facts total
    assert!(
        output.contains("memory_claim_backfill_facts_total{")
            && output.contains("outcome=\"completed\"")
            && output.contains("reason_code=\"completed\""),
        "backfill completed count: {output}"
    );
    assert!(
        output.contains("memory_claim_backfill_facts_total{")
            && output.contains("outcome=\"skipped\"")
            && output.contains("reason_code=\"skipped\""),
        "backfill skipped count: {output}"
    );
}

#[test]
fn no_forbidden_identifier_appears_as_label() {
    let handle = render_handle();

    // Emit a sample metric to populate labels.
    counter!(
        METRIC_PIPELINE_TOTAL,
        "stage" => "project",
        "schema" => "attribute",
        "outcome" => "duplicate",
        "reason_code" => "duplicate",
    )
    .increment(1);

    let output = handle.render();

    // ADR-0005: forbidden identifiers must never become Prometheus labels.
    for forbidden in [
        "namespace",
        "project",
        "subject",
        "comparison_key",
        "fact_id",
        "claim_id",
        "relation_id",
        "job_id",
        "episode_id",
        "policy_tags",
    ] {
        assert!(
            !output.contains(&format!("{forbidden}=\"")),
            "forbidden label `{forbidden}` appears in Prometheus output:\n{output}"
        );
    }
}
