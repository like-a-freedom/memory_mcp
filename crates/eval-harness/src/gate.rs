use crate::artifact::{GateDecision, GateFailureReason, GateStatus, RunArtifact};
use crate::error::EvalError;
use crate::profile::GateDecl;

fn find_observed_metric(
    artifact: &RunArtifact,
    target: &crate::profile::GateTarget,
) -> Option<f64> {
    artifact
        .suite_summaries
        .iter()
        .filter(|s| s.suite_id == target.suite_id)
        .flat_map(|s| s.metrics.get(&target.metric))
        .copied()
        .next()
}

fn has_non_finite_gate_value(
    observed: f64,
    hard_floor: Option<f64>,
    baseline: Option<f64>,
    regression_budget: Option<f64>,
) -> bool {
    !observed.is_finite()
        || hard_floor.is_some_and(|value| !value.is_finite())
        || baseline.is_some_and(|value| !value.is_finite())
        || regression_budget.is_some_and(|value| !value.is_finite())
}

pub fn evaluate_gates(
    gate_decls: &[GateDecl],
    artifact: &RunArtifact,
    baseline: Option<&RunArtifact>,
) -> Result<Vec<GateDecision>, EvalError> {
    let mut decisions = Vec::new();

    for decl in gate_decls {
        let observed = find_observed_metric(artifact, &decl.target);

        let Some(observed) = observed else {
            let mut decision = evaluate_missing_metric(&decl.target.suite_id, &decl.target.metric);
            decision.hard_floor = decl.hard_floor;
            decision.regression_budget = decl.regression_budget;
            decisions.push(decision);
            continue;
        };

        let baseline_value = baseline.and_then(|b| find_observed_metric(b, &decl.target));

        if has_non_finite_gate_value(
            observed,
            decl.hard_floor,
            baseline_value,
            decl.regression_budget,
        ) {
            decisions.push(GateDecision {
                suite_id: decl.target.suite_id.clone(),
                metric: decl.target.metric.clone(),
                observed,
                hard_floor: decl.hard_floor,
                baseline: baseline_value,
                regression_budget: decl.regression_budget,
                status: GateStatus::Invalid,
                reason: GateFailureReason::NonFiniteMetric,
            });
            continue;
        }

        if decl.baseline_required && baseline_value.is_none() {
            decisions.push(GateDecision {
                suite_id: decl.target.suite_id.clone(),
                metric: decl.target.metric.clone(),
                observed,
                hard_floor: decl.hard_floor,
                baseline: None,
                regression_budget: decl.regression_budget,
                status: GateStatus::Invalid,
                reason: GateFailureReason::MissingBaseline,
            });
            continue;
        }

        if let Some(floor) = decl.hard_floor
            && observed < floor
        {
            decisions.push(GateDecision {
                suite_id: decl.target.suite_id.clone(),
                metric: decl.target.metric.clone(),
                observed,
                hard_floor: Some(floor),
                baseline: baseline_value,
                regression_budget: decl.regression_budget,
                status: GateStatus::Failed,
                reason: GateFailureReason::HardFloorNotMet,
            });
            continue;
        }

        if let (Some(baseline_val), Some(budget)) = (baseline_value, decl.regression_budget)
            && baseline_val - observed > budget
        {
            decisions.push(GateDecision {
                suite_id: decl.target.suite_id.clone(),
                metric: decl.target.metric.clone(),
                observed,
                hard_floor: decl.hard_floor,
                baseline: Some(baseline_val),
                regression_budget: Some(budget),
                status: GateStatus::Failed,
                reason: GateFailureReason::RegressionBudgetExceeded,
            });
            continue;
        }

        decisions.push(GateDecision {
            suite_id: decl.target.suite_id.clone(),
            metric: decl.target.metric.clone(),
            observed,
            hard_floor: decl.hard_floor,
            baseline: baseline_value,
            regression_budget: decl.regression_budget,
            status: GateStatus::Passed,
            reason: GateFailureReason::None,
        });
    }

    Ok(decisions)
}

pub fn evaluate_missing_metric(suite_id: &str, metric: &str) -> GateDecision {
    GateDecision {
        suite_id: suite_id.to_string(),
        metric: metric.to_string(),
        observed: 0.0,
        hard_floor: None,
        baseline: None,
        regression_budget: None,
        status: GateStatus::Invalid,
        reason: GateFailureReason::MissingMetric,
    }
}

pub fn evaluate_metric_gate(
    observed: f64,
    hard_floor: Option<f64>,
    baseline: Option<f64>,
    regression_budget: Option<f64>,
) -> GateDecision {
    if has_non_finite_gate_value(observed, hard_floor, baseline, regression_budget) {
        return GateDecision {
            suite_id: String::new(),
            metric: String::new(),
            observed,
            hard_floor,
            baseline,
            regression_budget,
            status: GateStatus::Invalid,
            reason: GateFailureReason::NonFiniteMetric,
        };
    }

    if let Some(floor) = hard_floor
        && observed < floor
    {
        return GateDecision {
            suite_id: String::new(),
            metric: String::new(),
            observed,
            hard_floor: Some(floor),
            baseline,
            regression_budget,
            status: GateStatus::Failed,
            reason: GateFailureReason::HardFloorNotMet,
        };
    }

    if let (Some(base), Some(budget)) = (baseline, regression_budget)
        && base - observed > budget
    {
        return GateDecision {
            suite_id: String::new(),
            metric: String::new(),
            observed,
            hard_floor,
            baseline: Some(base),
            regression_budget: Some(budget),
            status: GateStatus::Failed,
            reason: GateFailureReason::RegressionBudgetExceeded,
        };
    }

    GateDecision {
        suite_id: String::new(),
        metric: String::new(),
        observed,
        hard_floor,
        baseline,
        regression_budget,
        status: GateStatus::Passed,
        reason: GateFailureReason::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_floor_passes_above() {
        let decision = evaluate_metric_gate(0.95, Some(0.90), None, None);
        assert_eq!(decision.status, GateStatus::Passed);
    }

    #[test]
    fn hard_floor_fails_below() {
        let decision = evaluate_metric_gate(0.85, Some(0.90), None, None);
        assert_eq!(decision.status, GateStatus::Failed);
        assert_eq!(decision.reason, GateFailureReason::HardFloorNotMet);
    }

    #[test]
    fn regression_fails_even_above_the_hard_floor() {
        let decision = evaluate_metric_gate(0.94, Some(0.90), Some(0.98), Some(0.02));
        assert_eq!(decision.status, GateStatus::Failed);
        assert_eq!(decision.reason, GateFailureReason::RegressionBudgetExceeded);
    }

    #[test]
    fn regression_within_budget_passes() {
        let decision = evaluate_metric_gate(0.97, Some(0.90), Some(0.98), Some(0.02));
        assert_eq!(decision.status, GateStatus::Passed);
    }

    #[test]
    fn no_floor_no_budget_always_passes() {
        let decision = evaluate_metric_gate(0.50, None, None, None);
        assert_eq!(decision.status, GateStatus::Passed);
    }

    #[test]
    fn baseline_without_budget_ignores_regression() {
        let decision = evaluate_metric_gate(0.50, Some(0.40), Some(0.98), None);
        assert_eq!(decision.status, GateStatus::Passed);
    }

    #[test]
    fn missing_metric_is_explicitly_invalid() {
        let decision = evaluate_missing_metric("end-to-end", "context_match_rate");
        assert_eq!(decision.status, GateStatus::Invalid);
        assert_eq!(decision.reason, GateFailureReason::MissingMetric);
    }

    #[test]
    fn nan_gate_observation_is_invalid() {
        let decision = evaluate_metric_gate(f64::NAN, Some(0.95), None, None);

        assert_eq!(decision.status, GateStatus::Invalid);
        assert_eq!(decision.reason, GateFailureReason::NonFiniteMetric);
    }

    #[test]
    fn positive_infinity_gate_observation_is_invalid() {
        let decision = evaluate_metric_gate(f64::INFINITY, Some(0.95), None, None);

        assert_eq!(decision.status, GateStatus::Invalid);
        assert_eq!(decision.reason, GateFailureReason::NonFiniteMetric);
    }

    #[test]
    fn negative_infinity_gate_observation_is_invalid() {
        let decision = evaluate_metric_gate(f64::NEG_INFINITY, Some(0.95), None, None);

        assert_eq!(decision.status, GateStatus::Invalid);
        assert_eq!(decision.reason, GateFailureReason::NonFiniteMetric);
    }

    #[test]
    fn non_finite_suite_summary_metric_is_invalid_at_release_gate() {
        let outcome = crate::domain::EvalCaseOutcome::new(
            "retrieval-quality",
            "case-1",
            crate::domain::EvalMode::RetrievalOnly,
            crate::domain::CorpusSplit::Test,
            crate::domain::LabelTrust::Reviewed,
            crate::domain::CaseStatus::Passed,
        );
        let mut artifact = RunArtifact::fixture(
            vec![outcome],
            vec![crate::domain::EvalCaseId::parse("case-1").expect("valid case id")],
        );
        artifact
            .suite_summaries
            .push(crate::artifact::SuiteSummary {
                suite_id: "retrieval-quality".into(),
                mode: crate::domain::EvalMode::RetrievalOnly,
                total: 1,
                passed: 1,
                quality_failed: 0,
                invalid: 0,
                metrics: std::collections::BTreeMap::from([("precision_macro".into(), f64::NAN)]),
            });
        let gate = GateDecl {
            target: crate::profile::GateTarget {
                suite_id: "retrieval-quality".into(),
                metric: "precision_macro".into(),
                mode: None,
                split: None,
                label_trust: None,
            },
            hard_floor: Some(0.95),
            regression_budget: None,
            baseline_required: false,
        };

        let decision = evaluate_gates(&[gate], &artifact, None)
            .expect("gate evaluation succeeds")
            .remove(0);

        assert_eq!(decision.status, GateStatus::Invalid);
        assert_eq!(decision.reason, GateFailureReason::NonFiniteMetric);
    }
}
