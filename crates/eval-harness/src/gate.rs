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

pub fn evaluate_gates(
    gate_decls: &[GateDecl],
    artifact: &RunArtifact,
    baseline: Option<&RunArtifact>,
) -> Result<Vec<GateDecision>, EvalError> {
    let mut decisions = Vec::new();

    for decl in gate_decls {
        let observed = find_observed_metric(artifact, &decl.target);

        let Some(observed) = observed else {
            decisions.push(GateDecision {
                metric: decl.target.metric.clone(),
                observed: 0.0,
                hard_floor: decl.hard_floor,
                baseline: None,
                regression_budget: decl.regression_budget,
                status: GateStatus::Invalid,
                reason: GateFailureReason::None,
            });
            continue;
        };

        let baseline_value = baseline.and_then(|b| find_observed_metric(b, &decl.target));

        if decl.baseline_required && baseline_value.is_none() {
            decisions.push(GateDecision {
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

pub fn evaluate_metric_gate(
    observed: f64,
    hard_floor: Option<f64>,
    baseline: Option<f64>,
    regression_budget: Option<f64>,
) -> GateDecision {
    if let Some(floor) = hard_floor
        && observed < floor
    {
        return GateDecision {
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
}
