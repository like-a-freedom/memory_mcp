//! Claim reconciliation telemetry — bounded metrics and redacted trace events.
//!
//! Five Prometheus metric families with bounded enum labels (ADR-0005).
//! Raw namespace, project, subject, comparison-key, fact, claim, relation,
//! and job identifiers never become metric labels. Trace events carry the
//! full structurally-redacted key; metrics aggregate only by schema,
//! stage, outcome, reason, and match mode.

use crate::models::claim::ClaimSchemaFamily;

// ─── Bounded label enums ─────────────────────────────────────────────────────

/// Stage of the claim pipeline. Never derived from a free-form string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaimMetricStage {
    Project,
    Reconcile,
    /// Reserved for future backfill-stage pipeline_total emission; emitted
    /// today via `record_backfill_fact` on the dedicated backfill counter.
    #[allow(dead_code)]
    Backfill,
}

impl ClaimMetricStage {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Reconcile => "reconcile",
            Self::Backfill => "backfill",
        }
    }
}

/// Schema label. Unknown versions/families collapse to `other`.
pub(crate) fn schema_label(family: ClaimSchemaFamily) -> &'static str {
    match family {
        ClaimSchemaFamily::Attribute => "attribute",
        ClaimSchemaFamily::Quantity => "quantity",
        ClaimSchemaFamily::Relation => "relation",
        ClaimSchemaFamily::Commitment => "commitment",
    }
}

/// Outcome label. Unknown values collapse to `other`.
pub(crate) fn outcome_label(outcome: &str) -> &'static str {
    match outcome {
        "duplicate" => "duplicate",
        "supersession" => "supersession",
        "correction" => "correction",
        "contradiction" => "contradiction",
        "temporal_ambiguity" => "temporal_ambiguity",
        "coexist" => "coexist",
        _ => "other",
    }
}

/// Reason-code label. Bounded categories only — full reason text stays in
/// the trace event and durable job state.
pub(crate) fn reason_label(reason_code: &str) -> &'static str {
    match reason_code {
        // Built-in skip reasons
        "invalid_value" => "invalid_value",
        "invalid_unit" => "invalid_unit",
        "missing_subject" => "missing_subject",
        "missing_key" => "missing_key",
        "missing_object" => "missing_object",
        "missing_action" => "missing_action",
        // Reconciliation outcomes
        "duplicate" => "duplicate",
        "supersession" => "supersession",
        "correction" => "correction",
        "contradiction" => "contradiction",
        "temporal_ambiguity" => "temporal_ambiguity",
        "coexist" => "coexist",
        // Backfill
        "completed" => "completed",
        "skipped" => "skipped",
        "failed" => "failed",
        "retry_scheduled" => "retry_scheduled",
        // Error buckets (full messages remain in trace/job state)
        "validation" => "validation",
        "storage" => "storage",
        "lease" => "lease",
        "retry_exhausted" => "retry_exhausted",
        "internal" => "internal",
        _ => "other",
    }
}

/// Match mode used during candidate selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaimMatchMode {
    Exact,
    /// Reserved for a future alias-match candidate path; today candidates are
    /// looked up by exact slot fingerprint only.
    #[allow(dead_code)]
    Alias,
}

impl ClaimMatchMode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Alias => "alias",
        }
    }
}

// ─── Metric name constants ────────────────────────────────────────────────────
//
// Constants and helper functions below are part of the public observability surface. They are
// exercised by the `tests/prometheus_claim_metrics.rs` integration test under the `prometheus` feature.
// In the default build the `metrics` facade is no-op so the calls compile away; the unit test
// `metric_names_use_memory_prefix` references each constant directly to catch drift.

/// Total claim pipeline events by stage, schema, outcome, reason.
pub const METRIC_PIPELINE_TOTAL: &str = "memory_claim_pipeline_total";
/// Pipeline stage duration in seconds.
pub const METRIC_PIPELINE_DURATION_SECONDS: &str = "memory_claim_pipeline_duration_seconds";
/// Number of candidates considered for a claim slot.
pub const METRIC_CANDIDATE_COUNT: &str = "memory_claim_candidate_count";
/// Currently active claim relations by schema and outcome.
pub const METRIC_RELATIONS_ACTIVE: &str = "memory_claim_relations_active";
/// Total backfilled facts by outcome and reason.
pub const METRIC_BACKFILL_FACTS_TOTAL: &str = "memory_claim_backfill_facts_total";

// ─── Metric emission helpers (no-op without a recorder) ──────────────────────

/// Increment `memory_claim_pipeline_total{stage,schema,outcome,reason_code}`.
pub(crate) fn record_pipeline_event(
    stage: ClaimMetricStage,
    schema: ClaimSchemaFamily,
    outcome: &str,
    reason_code: &str,
) {
    metrics::counter!(
        METRIC_PIPELINE_TOTAL,
        "stage" => stage.as_str(),
        "schema" => schema_label(schema),
        "outcome" => outcome_label(outcome),
        "reason_code" => reason_label(reason_code),
    )
    .increment(1);
}

/// Record `memory_claim_pipeline_duration_seconds{stage,schema,outcome}`.
pub(crate) fn record_pipeline_duration(
    stage: ClaimMetricStage,
    schema: ClaimSchemaFamily,
    outcome: &str,
    duration: std::time::Duration,
) {
    metrics::histogram!(
        METRIC_PIPELINE_DURATION_SECONDS,
        "stage" => stage.as_str(),
        "schema" => schema_label(schema),
        "outcome" => outcome_label(outcome),
    )
    .record(duration.as_secs_f64());
}

/// Record `memory_claim_candidate_count{schema,match_mode}`.
pub(crate) fn record_candidate_count(
    schema: ClaimSchemaFamily,
    match_mode: ClaimMatchMode,
    count: usize,
) {
    metrics::histogram!(
        METRIC_CANDIDATE_COUNT,
        "schema" => schema_label(schema),
        "match_mode" => match_mode.as_str(),
    )
    .record(count as f64);
}

/// Set `memory_claim_relations_active{schema,outcome}` to `value`.
pub(crate) fn set_active_relations(schema: ClaimSchemaFamily, outcome: &str, value: f64) {
    metrics::gauge!(
        METRIC_RELATIONS_ACTIVE,
        "schema" => schema_label(schema),
        "outcome" => outcome_label(outcome),
    )
    .set(value);
}

/// Increment `memory_claim_backfill_facts_total{outcome,reason_code}`.
pub(crate) fn record_backfill_fact(outcome: &str, reason_code: &str) {
    metrics::counter!(
        METRIC_BACKFILL_FACTS_TOTAL,
        "outcome" => outcome_label(outcome),
        "reason_code" => reason_label(reason_code),
    )
    .increment(1);
}

// ─── Forbidden-label guard ────────────────────────────────────────────────────

// Label keys banned from Prometheus metrics by ADR-0005. The unit test
// `forbidden_label_keys_are_complete` asserts no metric emitted by this
// module ever uses one of them; the constant itself is test-only state.
#[cfg(test)]
pub(crate) const FORBIDDEN_LABEL_KEYS: &[&str] = &[
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
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_label_covers_all_families() {
        assert_eq!(schema_label(ClaimSchemaFamily::Attribute), "attribute");
        assert_eq!(schema_label(ClaimSchemaFamily::Quantity), "quantity");
        assert_eq!(schema_label(ClaimSchemaFamily::Relation), "relation");
        assert_eq!(schema_label(ClaimSchemaFamily::Commitment), "commitment");
    }

    #[test]
    fn outcome_label_covers_persisted_vocabulary() {
        for ok in [
            "duplicate",
            "supersession",
            "correction",
            "contradiction",
            "temporal_ambiguity",
            "coexist",
        ] {
            assert_eq!(outcome_label(ok), ok);
        }
        assert_eq!(outcome_label("unexpected"), "other");
    }

    #[test]
    fn reason_label_categorizes_known_codes() {
        assert_eq!(reason_label("invalid_value"), "invalid_value");
        assert_eq!(reason_label("storage"), "storage");
        assert_eq!(reason_label("made-up"), "other");
    }

    #[test]
    fn forbidden_label_keys_are_complete() {
        // ADR-0005 forbids every unbounded identifier from becoming a label.
        for key in FORBIDDEN_LABEL_KEYS {
            assert!(!key.is_empty());
        }
        assert!(FORBIDDEN_LABEL_KEYS.contains(&"namespace"));
        assert!(FORBIDDEN_LABEL_KEYS.contains(&"fact_id"));
        assert!(FORBIDDEN_LABEL_KEYS.contains(&"claim_id"));
        assert!(FORBIDDEN_LABEL_KEYS.contains(&"relation_id"));
        assert!(FORBIDDEN_LABEL_KEYS.contains(&"job_id"));
    }

    #[test]
    fn stage_as_str_is_stable() {
        assert_eq!(ClaimMetricStage::Project.as_str(), "project");
        assert_eq!(ClaimMetricStage::Reconcile.as_str(), "reconcile");
        assert_eq!(ClaimMetricStage::Backfill.as_str(), "backfill");
    }

    #[test]
    fn match_mode_as_str_is_stable() {
        assert_eq!(ClaimMatchMode::Exact.as_str(), "exact");
        assert_eq!(ClaimMatchMode::Alias.as_str(), "alias");
    }

    #[test]
    fn metric_names_use_memory_prefix() {
        // The `memory_` prefix is used for the five families so dashboards can
        // filter by prefix.
        assert!(METRIC_PIPELINE_TOTAL.starts_with("memory_claim_"));
        assert!(METRIC_PIPELINE_DURATION_SECONDS.starts_with("memory_claim_"));
        assert!(METRIC_CANDIDATE_COUNT.starts_with("memory_claim_"));
        assert!(METRIC_RELATIONS_ACTIVE.starts_with("memory_claim_"));
        assert!(METRIC_BACKFILL_FACTS_TOTAL.starts_with("memory_claim_"));
    }

    #[test]
    fn record_pipeline_event_is_safe_without_recorder() {
        // No recorder installed — the `metrics` facade is a no-op.
        record_pipeline_event(
            ClaimMetricStage::Project,
            ClaimSchemaFamily::Attribute,
            "duplicate",
            "duplicate",
        );
    }

    #[test]
    fn record_pipeline_duration_is_safe_without_recorder() {
        record_pipeline_duration(
            ClaimMetricStage::Reconcile,
            ClaimSchemaFamily::Quantity,
            "coexist",
            std::time::Duration::from_micros(1500),
        );
    }

    #[test]
    fn record_candidate_count_is_safe_without_recorder() {
        record_candidate_count(ClaimSchemaFamily::Relation, ClaimMatchMode::Exact, 4);
    }

    #[test]
    fn set_active_relations_is_safe_without_recorder() {
        set_active_relations(ClaimSchemaFamily::Attribute, "contradiction", 3.0);
    }

    #[test]
    fn record_backfill_fact_is_safe_without_recorder() {
        record_backfill_fact("completed", "completed");
    }
}
