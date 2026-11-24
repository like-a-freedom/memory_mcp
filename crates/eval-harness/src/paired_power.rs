use crate::EvalError;
use crate::paired::ExploratoryPilotReport;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const EXACT_PAIRED_POWER_SCHEMA_V1: &str = "memory-mcp-exact-paired-power/v1";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExactMcNemarPowerConfig {
    pub target_risk_difference: f64,
    pub minimum_point_effect: f64,
    pub one_sided_alpha: f64,
    pub target_power: f64,
    pub confidence_level: f64,
    pub maximum_sample_size: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExactMcNemarPowerPlan {
    pub schema_version: String,
    pub independent_family_count: u32,
    pub discordant_family_count: u32,
    pub confidence_level: f64,
    pub discordance_probability_upper_bound: f64,
    pub target_risk_difference: f64,
    pub minimum_point_effect: f64,
    pub one_sided_alpha: f64,
    pub target_power: f64,
    pub maximum_sample_size: u32,
    pub required_sample_size: Option<u32>,
    pub power_at_required_sample_size: Option<f64>,
    pub power_at_cap: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedPilotPowerAnalysis {
    pub schema_version: String,
    pub plan_id: String,
    pub plan_sha256: String,
    pub stratum_id: String,
    pub first_run_id: Option<String>,
    pub second_run_id: Option<String>,
    pub independent_family_count: u32,
    pub discordant_family_count: u32,
    pub discordant_family_ids: Vec<String>,
    pub candidate_win_family_ids_by_run: BTreeMap<String, Vec<String>>,
    pub candidate_loss_family_ids_by_run: BTreeMap<String, Vec<String>>,
    pub power_plan: ExactMcNemarPowerPlan,
    /// This planner reports sample size only; it never accepts or emits a promotion decision.
    pub promotion_decision: String,
}

pub const PAIRED_PILOT_POWER_ANALYSIS_SCHEMA_V1: &str = "memory-mcp-pilot-power-analysis/v1";

pub fn plan_paired_pilot_power(
    pilot: &ExploratoryPilotReport,
    stratum_id: &str,
    config: ExactMcNemarPowerConfig,
) -> Result<PairedPilotPowerAnalysis, EvalError> {
    pilot.first_run.validate().map_err(|error| {
        EvalError::InvalidInput(format!(
            "first comparison in exploratory pilot is invalid: {error}"
        ))
    })?;
    pilot.second_run.validate().map_err(|error| {
        EvalError::InvalidInput(format!(
            "second comparison in exploratory pilot is invalid: {error}"
        ))
    })?;
    pilot.validate()?;
    if pilot.first_run.plan.stage != crate::paired::PairedEvaluationStage::ExploratoryComparison
        || pilot.second_run.plan.stage
            != crate::paired::PairedEvaluationStage::ExploratoryComparison
    {
        return Err(EvalError::InvalidInput(
            "paired power planning requires an exploratory pilot report".into(),
        ));
    }

    let mut expected_family_counts = BTreeMap::<String, usize>::new();
    for trial in &pilot.first_run.plan.expected_trials {
        if trial.stratum_id == stratum_id {
            *expected_family_counts
                .entry(trial.family_id.clone())
                .or_default() += 1;
        }
    }
    if expected_family_counts.is_empty() {
        return Err(EvalError::InvalidInput(format!(
            "paired power stratum {stratum_id:?} has no registered families"
        )));
    }
    if expected_family_counts.values().any(|count| *count != 1) {
        return Err(EvalError::InvalidInput(
            "exact McNemar planning requires exactly one paired task per family in the stratum per run".into(),
        ));
    }
    let expected_families = expected_family_counts
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let first_deltas = task_success_family_deltas(&pilot.first_run, stratum_id)?;
    let second_deltas = task_success_family_deltas(&pilot.second_run, stratum_id)?;
    if first_deltas.keys().cloned().collect::<BTreeSet<_>>() != expected_families
        || second_deltas.keys().cloned().collect::<BTreeSet<_>>() != expected_families
    {
        return Err(EvalError::InvalidInput(
            "pilot task-success family deltas do not match the registered stratum families".into(),
        ));
    }

    let first_label = run_label(&pilot.first_run);
    let second_label = run_label(&pilot.second_run);
    let (first_wins, first_losses) = classify_family_deltas(&first_deltas)?;
    let (second_wins, second_losses) = classify_family_deltas(&second_deltas)?;
    let wins_by_run = BTreeMap::from([
        (first_label, first_wins.clone()),
        (second_label, second_wins.clone()),
    ]);
    let losses_by_run = BTreeMap::from([
        (run_label(&pilot.first_run), first_losses.clone()),
        (run_label(&pilot.second_run), second_losses.clone()),
    ]);
    let mut discordant_family_ids = Vec::new();
    for family_id in &expected_families {
        let first_delta = first_deltas.get(family_id).copied().ok_or_else(|| {
            EvalError::InvalidInput("pilot first-run family delta is missing".into())
        })?;
        let second_delta = second_deltas.get(family_id).copied().ok_or_else(|| {
            EvalError::InvalidInput("pilot second-run family delta is missing".into())
        })?;
        if first_delta != 0.0 || second_delta != 0.0 {
            discordant_family_ids.push(family_id.clone());
        }
    }

    let independent_family_count = u32::try_from(expected_families.len()).map_err(|_| {
        EvalError::InvalidInput("pilot independent family count exceeds u32".into())
    })?;
    let discordant_family_count = u32::try_from(discordant_family_ids.len())
        .map_err(|_| EvalError::InvalidInput("pilot discordant family count exceeds u32".into()))?;
    let power_plan =
        plan_exact_mcnemar_power(independent_family_count, discordant_family_count, config)?;

    Ok(PairedPilotPowerAnalysis {
        schema_version: PAIRED_PILOT_POWER_ANALYSIS_SCHEMA_V1.into(),
        plan_id: pilot.first_run.plan.plan_id.clone(),
        plan_sha256: pilot.plan_sha256.clone(),
        stratum_id: stratum_id.into(),
        first_run_id: pilot.first_run.run_id.clone(),
        second_run_id: pilot.second_run.run_id.clone(),
        independent_family_count,
        discordant_family_count,
        discordant_family_ids,
        candidate_win_family_ids_by_run: wins_by_run,
        candidate_loss_family_ids_by_run: losses_by_run,
        power_plan,
        promotion_decision: "not_produced".into(),
    })
}

fn classify_family_deltas(
    deltas: &BTreeMap<String, f64>,
) -> Result<(Vec<String>, Vec<String>), EvalError> {
    let mut wins = Vec::new();
    let mut losses = Vec::new();
    for (family_id, delta) in deltas {
        if ![-1.0, 0.0, 1.0].contains(delta) {
            return Err(EvalError::InvalidInput(
                "exact McNemar planning requires one binary paired outcome per family and run"
                    .into(),
            ));
        }
        if *delta > 0.0 {
            wins.push(family_id.clone());
        } else if *delta < 0.0 {
            losses.push(family_id.clone());
        }
    }
    Ok((wins, losses))
}

fn task_success_family_deltas(
    comparison: &crate::paired::PairedRunComparison,
    stratum_id: &str,
) -> Result<BTreeMap<String, f64>, EvalError> {
    comparison
        .by_stratum
        .get(stratum_id)
        .map(|metrics| metrics.task_success.family_deltas.clone())
        .ok_or_else(|| {
            EvalError::InvalidInput(format!(
                "paired pilot has no task-success estimates for stratum {stratum_id:?}"
            ))
        })
}

fn run_label(comparison: &crate::paired::PairedRunComparison) -> String {
    comparison.run_id.clone().unwrap_or_else(|| {
        comparison
            .run_started_at
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
    })
}

pub fn plan_exact_mcnemar_power(
    independent_family_count: u32,
    discordant_family_count: u32,
    config: ExactMcNemarPowerConfig,
) -> Result<ExactMcNemarPowerPlan, EvalError> {
    validate_inputs(independent_family_count, discordant_family_count, config)?;
    let discordance_upper_bound = clopper_pearson_upper_bound(
        independent_family_count,
        discordant_family_count,
        config.confidence_level,
    );
    if discordance_upper_bound + f64::EPSILON < config.target_risk_difference {
        return Err(EvalError::InvalidInput(
            "pilot discordance upper bound is below the target risk difference".into(),
        ));
    }

    let mut required_sample_size = None;
    let mut power_at_required_sample_size = None;
    for sample_size in 1..=config.maximum_sample_size {
        let power = exact_mcnemar_power(
            sample_size as usize,
            discordance_upper_bound,
            config.target_risk_difference,
            config.minimum_point_effect,
            config.one_sided_alpha,
        );
        if power >= config.target_power {
            required_sample_size = Some(sample_size);
            power_at_required_sample_size = Some(power);
            break;
        }
    }
    let power_at_cap = exact_mcnemar_power(
        config.maximum_sample_size as usize,
        discordance_upper_bound,
        config.target_risk_difference,
        config.minimum_point_effect,
        config.one_sided_alpha,
    );

    Ok(ExactMcNemarPowerPlan {
        schema_version: EXACT_PAIRED_POWER_SCHEMA_V1.into(),
        independent_family_count,
        discordant_family_count,
        confidence_level: config.confidence_level,
        discordance_probability_upper_bound: discordance_upper_bound,
        target_risk_difference: config.target_risk_difference,
        minimum_point_effect: config.minimum_point_effect,
        one_sided_alpha: config.one_sided_alpha,
        target_power: config.target_power,
        maximum_sample_size: config.maximum_sample_size,
        required_sample_size,
        power_at_required_sample_size,
        power_at_cap,
    })
}

fn validate_inputs(
    independent_family_count: u32,
    discordant_family_count: u32,
    config: ExactMcNemarPowerConfig,
) -> Result<(), EvalError> {
    if independent_family_count == 0 {
        return Err(EvalError::InvalidInput(
            "independent family count must be positive".into(),
        ));
    }
    if discordant_family_count > independent_family_count {
        return Err(EvalError::InvalidInput(
            "discordant family count cannot exceed independent family count".into(),
        ));
    }
    if !config.target_risk_difference.is_finite()
        || config.target_risk_difference <= 0.0
        || config.target_risk_difference >= 1.0
    {
        return Err(EvalError::InvalidInput(
            "target risk difference must be finite and between zero and one".into(),
        ));
    }
    if !config.minimum_point_effect.is_finite()
        || config.minimum_point_effect < 0.0
        || config.minimum_point_effect >= config.target_risk_difference
    {
        return Err(EvalError::InvalidInput(
            "minimum point effect must be finite, non-negative, and below the target risk difference".into(),
        ));
    }
    if !config.one_sided_alpha.is_finite()
        || config.one_sided_alpha <= 0.0
        || config.one_sided_alpha >= 1.0
    {
        return Err(EvalError::InvalidInput(
            "one-sided alpha must be finite and between zero and one".into(),
        ));
    }
    if !config.target_power.is_finite() || config.target_power <= 0.0 || config.target_power >= 1.0
    {
        return Err(EvalError::InvalidInput(
            "target power must be finite and between zero and one".into(),
        ));
    }
    if !config.confidence_level.is_finite()
        || config.confidence_level <= 0.0
        || config.confidence_level >= 1.0
    {
        return Err(EvalError::InvalidInput(
            "confidence level must be finite and between zero and one".into(),
        ));
    }
    if config.maximum_sample_size == 0 || config.maximum_sample_size > 1_000 {
        return Err(EvalError::InvalidInput(
            "maximum sample size must be between 1 and 1000".into(),
        ));
    }
    Ok(())
}

fn clopper_pearson_upper_bound(
    independent_families: u32,
    discordant_families: u32,
    confidence_level: f64,
) -> f64 {
    if discordant_families == independent_families {
        return 1.0;
    }
    let target_tail = 1.0 - confidence_level;
    let mut lower = 0.0;
    let mut upper = 1.0;
    for _ in 0..80 {
        let candidate = (lower + upper) / 2.0;
        let cumulative = binomial_probabilities(independent_families as usize, candidate)
            .into_iter()
            .take(discordant_families as usize + 1)
            .sum::<f64>();
        if cumulative > target_tail {
            lower = candidate;
        } else {
            upper = candidate;
        }
    }
    (lower + upper) / 2.0
}

fn exact_mcnemar_power(
    sample_size: usize,
    discordance_probability: f64,
    target_risk_difference: f64,
    minimum_point_effect: f64,
    alpha: f64,
) -> f64 {
    let candidate_win_probability_given_discordance =
        (discordance_probability + target_risk_difference) / (2.0 * discordance_probability);
    let discordant_counts = binomial_probabilities(sample_size, discordance_probability);
    let mut power = 0.0;

    for (discordant_count, discordant_probability) in discordant_counts.into_iter().enumerate() {
        if discordant_count == 0 || discordant_probability == 0.0 {
            continue;
        }
        let alternative_wins = binomial_probabilities(
            discordant_count,
            candidate_win_probability_given_discordance,
        );
        let null_wins = binomial_probabilities(discordant_count, 0.5);
        let mut null_tail = vec![0.0; discordant_count + 1];
        let mut tail_probability = 0.0;
        for wins in (0..=discordant_count).rev() {
            tail_probability += null_wins[wins];
            null_tail[wins] = tail_probability;
        }

        for (wins, alternative_probability) in alternative_wins.into_iter().enumerate() {
            let observed_effect = (2 * wins) as f64 / sample_size as f64
                - discordant_count as f64 / sample_size as f64;
            if observed_effect + f64::EPSILON >= minimum_point_effect
                && null_tail[wins] <= alpha + f64::EPSILON
            {
                power += discordant_probability * alternative_probability;
            }
        }
    }
    power.clamp(0.0, 1.0)
}

fn binomial_probabilities(trials: usize, success_probability: f64) -> Vec<f64> {
    if success_probability <= 0.0 {
        let mut probabilities = vec![0.0; trials + 1];
        probabilities[0] = 1.0;
        return probabilities;
    }
    if success_probability >= 1.0 {
        let mut probabilities = vec![0.0; trials + 1];
        probabilities[trials] = 1.0;
        return probabilities;
    }

    let log_success = success_probability.ln();
    let log_failure = (1.0 - success_probability).ln();
    let log_weights = (0..=trials)
        .map(|successes| {
            log_binomial_coefficient(trials, successes)
                + successes as f64 * log_success
                + (trials - successes) as f64 * log_failure
        })
        .collect::<Vec<_>>();
    let maximum = log_weights
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let mut probabilities = log_weights
        .into_iter()
        .map(|weight| (weight - maximum).exp())
        .collect::<Vec<_>>();
    let normalizer = probabilities.iter().sum::<f64>();
    probabilities
        .iter_mut()
        .for_each(|probability| *probability /= normalizer);
    probabilities
}

fn log_binomial_coefficient(trials: usize, successes: usize) -> f64 {
    let smaller = successes.min(trials - successes);
    (1..=smaller)
        .map(|index| ((trials - smaller + index) as f64).ln() - (index as f64).ln())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::{ExactMcNemarPowerConfig, exact_mcnemar_power, plan_exact_mcnemar_power};

    #[test]
    fn plans_exact_sample_size_from_family_level_discordance_bound() {
        let plan = plan_exact_mcnemar_power(
            8,
            3,
            ExactMcNemarPowerConfig {
                target_risk_difference: 0.25,
                minimum_point_effect: 0.10,
                one_sided_alpha: 0.025,
                target_power: 0.90,
                confidence_level: 0.95,
                maximum_sample_size: 200,
            },
        )
        .expect("the registered pilot inputs are valid");

        assert_eq!(plan.required_sample_size, Some(123));
        assert_eq!(plan.discordant_family_count, 3);
        assert!((plan.discordance_probability_upper_bound - 0.710_759_183_498_190_6).abs() < 1e-12);
        let power_at_required = plan
            .power_at_required_sample_size
            .expect("the target power is reachable below the cap");
        assert!((power_at_required - 0.900_812_327_940_497).abs() < 1e-12);
        assert!((plan.power_at_cap - 0.987_797_512_144_490_1).abs() < 1e-12);
        assert!(
            exact_mcnemar_power(
                122,
                plan.discordance_probability_upper_bound,
                0.25,
                0.10,
                0.025,
            ) < 0.90
        );
    }

    #[test]
    fn reports_infeasible_power_when_the_preregistered_sample_cap_is_too_small() {
        let plan = plan_exact_mcnemar_power(
            8,
            3,
            ExactMcNemarPowerConfig {
                target_risk_difference: 0.25,
                minimum_point_effect: 0.10,
                one_sided_alpha: 0.025,
                target_power: 0.90,
                confidence_level: 0.95,
                maximum_sample_size: 122,
            },
        )
        .expect("the planning cap is a valid input");

        assert_eq!(plan.required_sample_size, None);
        assert!(plan.power_at_cap < 0.90);
    }

    #[test]
    fn rejects_more_discordant_families_than_independent_families() {
        let error = plan_exact_mcnemar_power(
            8,
            9,
            ExactMcNemarPowerConfig {
                target_risk_difference: 0.25,
                minimum_point_effect: 0.10,
                one_sided_alpha: 0.025,
                target_power: 0.90,
                confidence_level: 0.95,
                maximum_sample_size: 200,
            },
        )
        .expect_err("discordance count cannot exceed the family count");

        assert!(error.to_string().contains("discordant family count"));
    }
}
