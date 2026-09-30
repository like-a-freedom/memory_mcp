//! Integration tests for Prometheus metric recording in claim reconciliation.
//!
//! Verifies the five metric families render
//! correctly when the `prometheus` feature is enabled, and that no
//! forbidden identifier appears as a Prometheus label.

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

    // Family 3: memory_claim_candidates_considered{schema,match_mode}
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
        output.contains("memory_claim_candidates_considered"),
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

/// A failure on the claim pipeline has to be selectable, or an alert for it
/// matches an empty vector and never fires.
///
/// The recorded `outcome` is a *reconciliation outcome* — duplicate,
/// supersession, contradiction — and `outcome_label` collapses anything else
/// to `other`. So a post-projection failure, which is recorded as
/// `outcome="failed"`, reaches the exposition as `outcome="other"` like every
/// other non-reconciliation value. The reason it was a failure is in
/// `reason_code`, which is where the bounded error buckets live.
///
/// An alert written against `outcome="error"` therefore matches nothing at
/// all: the label value does not exist, `sum()` of empty is empty, and
/// `empty > 0` never fires. Nothing reports that — a rule on a metric that
/// resolves can be perfectly well-formed and completely dead.
#[test]
fn a_claim_pipeline_failure_is_selectable_by_its_reason_code() {
    let handle = render_handle();

    // The shape production actually emits for a non-fatal projection failure.
    counter!(
        METRIC_PIPELINE_TOTAL,
        "stage" => "project",
        "schema" => "attribute",
        "outcome" => "other",
        "reason_code" => "internal",
    )
    .increment(1);

    let output = handle.render();
    assert!(
        output.contains(r#"outcome="other""#) && output.contains(r#"reason_code="internal""#),
        "a projection failure must be reachable by its reason code, since `error` \
         is not a value the outcome vocabulary has: {output}"
    );
    assert!(
        !output.contains(r#"outcome="error""#),
        "`outcome=\"error\"` must not appear: it is not in the vocabulary, so a \
         rule selecting it is dead code that looks alive"
    );
}

/// The family's name must not end in the suffix the exporter would otherwise
/// append, because that is what makes its count series ambiguous.
///
/// `metrics_exporter_prometheus` appends `_count` to a summary's count line
/// only when the name does not already end in `count`
/// (`add_suffix_if_missing`). A family called `…_candidate_count` therefore
/// keeps its count on the *bare* name — the same name its seven quantile lines
/// carry. A PromQL selector matches by metric name, so
/// `rate(memory_claim_candidates_considered[…])` selects the count **and** every
/// quantile, and a mean computed from it divides by eight series instead of
/// one.
///
/// The name is a fixed decision either way, so this pins it: a name ending in
/// `_count` does not merely read untidy, it silently changes what a query
/// selects.
#[test]
fn the_candidate_count_name_leaves_room_for_an_explicit_count_series() {
    assert!(
        !METRIC_CANDIDATE_COUNT.ends_with("count"),
        "`{}` ends in `count`, so the exporter leaves its observation count on \
         the bare name — shared with the quantile lines — and every selector \
         for it matches those too. Rename the family so `_count` is appended \
         explicitly.",
        METRIC_CANDIDATE_COUNT
    );
    assert!(
        METRIC_CANDIDATE_COUNT.contains("candidates"),
        "and the name should say what is counted: {:?}",
        METRIC_CANDIDATE_COUNT
    );
}

/// A count of candidates is recorded into the histogram family, and the
/// recorder renders every histogram as a Prometheus **summary** — quantiles
/// over a rolling window, with `_sum` and a bare count series beside them.
/// That is the only shape available here: the exporter switches to bucket
/// exposition process-wide when buckets are configured, which would cost
/// every duration metric its quantile series.
///
/// So the count is recoverable exactly, from `_sum / _count`, and a panel must
/// use that rather than a quantile. A quantile cannot answer the question this
/// family exists for: every observation below the first reported quantile
/// collapses to 0, so "no candidates" and "a handful" are the same number.
#[test]
fn candidate_count_is_recoverable_as_a_mean() {
    let handle = render_handle();

    let read = |output: &str, suffix: &str| {
        let series = r#"schema="commitment",match_mode="exact""#;
        output
            .lines()
            .find(|line| line.starts_with(&format!("{METRIC_CANDIDATE_COUNT}{suffix}{{{series}}}")))
            .and_then(|line| line.split_whitespace().last())
            .and_then(|value| value.parse::<f64>().ok())
    };

    // A schema no other test in this file emits, so the numbers below are
    // this test's alone rather than a sum with a neighbour's.
    let before = handle.render();
    for _ in 0..3 {
        histogram!(
            METRIC_CANDIDATE_COUNT,
            "schema" => "commitment",
            "match_mode" => "exact",
        )
        .record(0.0);
    }
    for _ in 0..2 {
        histogram!(
            METRIC_CANDIDATE_COUNT,
            "schema" => "commitment",
            "match_mode" => "exact",
        )
        .record(4.0);
    }
    let after = handle.render();

    assert_eq!(
        // `_count`, explicitly. The family's name is chosen so the exporter
        // appends it, which is what keeps the count off the bare name its
        // quantile lines share — see the naming test above.
        read(&after, "_count").unwrap_or(0.0) - read(&before, "_count").unwrap_or(0.0),
        5.0,
        "every observation must be counted, on its own `_count` series: {after}"
    );
    // 3 zeros and 2 fours: a mean of 1.6 keeps the zeros visible, which is
    // what a quantile cannot do.
    assert_eq!(
        read(&after, "_sum").unwrap_or(0.0) - read(&before, "_sum").unwrap_or(0.0),
        8.0,
        "the values must be preserved exactly, so the mean is real: {after}"
    );
}
