//! Repeated-task reliability summaries for paired agent evaluations.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    EvalError,
    paired::{
        CaseTrialKey, ConfidenceInterval, PairedEvaluationPlan, PairedRunArtifact, TrialReceipt,
        cluster_bootstrap, compare_paired_runs,
    },
};

pub const PAIRED_RELIABILITY_SCHEMA_V1: &str = "memory-mcp-agent-reliability/v1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReliabilityCaseResult {
    pub stratum_id: String,
    pub family_id: String,
    pub case_id: String,
    pub repetitions: u32,
    pub incumbent_direct_memory_pass_k: bool,
    pub candidate_direct_memory_pass_k: bool,
    /// `true` only if every preregistered attempt completed successfully without a hard violation.
    pub incumbent_task_success_pass_k: bool,
    /// `true` only if every preregistered attempt completed successfully without a hard violation.
    pub candidate_task_success_pass_k: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArmPassKSummary {
    pub passed_cases: u64,
    pub case_count: u64,
    /// Unweighted case-level proportion; descriptive when families contain different case counts.
    pub case_pass_rate: f64,
    /// Mean of per-family case rates; the statistical unit remains the independent family.
    pub equal_family_pass_rate: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedPassKSummary {
    pub independent_family_count: u32,
    pub incumbent: ArmPassKSummary,
    pub candidate: ArmPassKSummary,
    /// Candidate minus incumbent, with each independent family weighted equally.
    pub pass_k_gain: f64,
    pub family_deltas: BTreeMap<String, f64>,
    pub interval: Option<ConfidenceInterval>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedReliabilityMetrics {
    /// Agent-observed memory coverage; descriptive and not independent evidence.
    pub direct_memory_success: PairedPassKSummary,
    /// Oracle-scored task success across all preregistered repetitions.
    pub task_success: PairedPassKSummary,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedReliabilityReport {
    pub schema_version: String,
    pub plan_sha256: String,
    pub incumbent_artifact_sha256: String,
    pub candidate_artifact_sha256: String,
    pub bootstrap: crate::paired::BootstrapConfig,
    pub repetitions_per_case: u32,
    pub overall: PairedReliabilityMetrics,
    pub by_stratum: BTreeMap<String, PairedReliabilityMetrics>,
    pub cases: Vec<ReliabilityCaseResult>,
}

impl PairedReliabilityReport {
    pub fn validate(&self) -> Result<(), EvalError> {
        if self.schema_version != PAIRED_RELIABILITY_SCHEMA_V1
            || self.repetitions_per_case == 0
            || self.cases.is_empty()
        {
            return Err(EvalError::InvalidInput(
                "paired reliability report requires its schema, repeated cases, and positive k"
                    .into(),
            ));
        }
        for digest in [
            &self.plan_sha256,
            &self.incumbent_artifact_sha256,
            &self.candidate_artifact_sha256,
        ] {
            if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(EvalError::InvalidInput(
                    "paired reliability report contains an invalid SHA-256 identity".into(),
                ));
            }
        }
        if self.cases.iter().any(|case| {
            case.repetitions != self.repetitions_per_case
                || case.stratum_id.trim().is_empty()
                || case.family_id.trim().is_empty()
                || case.case_id.trim().is_empty()
        }) {
            return Err(EvalError::InvalidInput(
                "paired reliability cases require non-empty identities and the report k".into(),
            ));
        }
        let case_ids = self
            .cases
            .iter()
            .map(|case| {
                (
                    case.stratum_id.as_str(),
                    case.family_id.as_str(),
                    case.case_id.as_str(),
                )
            })
            .collect::<BTreeSet<_>>();
        if case_ids.len() != self.cases.len() {
            return Err(EvalError::InvalidInput(
                "paired reliability report contains duplicate cases".into(),
            ));
        }
        let expected_overall = summarize_metrics(&self.cases, &self.bootstrap)?;
        if self.overall != expected_overall {
            return Err(EvalError::InvalidInput(
                "paired reliability overall metrics do not match the case outcomes".into(),
            ));
        }
        let strata = self
            .cases
            .iter()
            .map(|case| case.stratum_id.clone())
            .collect::<BTreeSet<_>>();
        let expected_by_stratum = strata
            .into_iter()
            .map(|stratum| {
                let selected = self
                    .cases
                    .iter()
                    .filter(|case| case.stratum_id == stratum)
                    .cloned()
                    .collect::<Vec<_>>();
                Ok((stratum, summarize_metrics(&selected, &self.bootstrap)?))
            })
            .collect::<Result<BTreeMap<_, _>, EvalError>>()?;
        if self.by_stratum != expected_by_stratum {
            return Err(EvalError::InvalidInput(
                "paired reliability stratum metrics do not match the case outcomes".into(),
            ));
        }
        Ok(())
    }
}

/// Summarize pass^k from the exact case and repetition manifest already frozen in the plan.
pub fn summarize_paired_reliability(
    plan: &PairedEvaluationPlan,
    incumbent: &PairedRunArtifact,
    candidate: &PairedRunArtifact,
) -> Result<PairedReliabilityReport, EvalError> {
    compare_paired_runs(incumbent, candidate, plan)?;
    let repetitions_per_case = uniform_repetitions(&plan.expected_trials)?;
    let incumbent_receipts = receipts_by_case(incumbent);
    let candidate_receipts = receipts_by_case(candidate);
    let mut cases = Vec::new();
    for ((stratum_id, family_id, case_id), incumbent_trials) in incumbent_receipts {
        let candidate_trials = candidate_receipts
            .get(&(stratum_id.clone(), family_id.clone(), case_id.clone()))
            .ok_or_else(|| EvalError::InvalidInput("candidate is missing a paired case".into()))?;
        let incumbent_direct_memory_pass_k =
            pass_all_repetitions(&incumbent_trials, repetitions_per_case, |receipt| {
                receipt.direct_memory_success
            })?;
        let candidate_direct_memory_pass_k =
            pass_all_repetitions(candidate_trials, repetitions_per_case, |receipt| {
                receipt.direct_memory_success
            })?;
        let incumbent_task_success_pass_k = pass_all_repetitions(
            &incumbent_trials,
            repetitions_per_case,
            TrialReceipt::task_success,
        )?;
        let candidate_task_success_pass_k = pass_all_repetitions(
            candidate_trials,
            repetitions_per_case,
            TrialReceipt::task_success,
        )?;
        cases.push(ReliabilityCaseResult {
            stratum_id,
            family_id,
            case_id,
            repetitions: repetitions_per_case,
            incumbent_direct_memory_pass_k,
            candidate_direct_memory_pass_k,
            incumbent_task_success_pass_k,
            candidate_task_success_pass_k,
        });
    }
    let overall = summarize_metrics(&cases, &plan.bootstrap)?;
    let strata = cases
        .iter()
        .map(|case| case.stratum_id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let by_stratum = strata
        .into_iter()
        .map(|stratum| {
            let selected = cases
                .iter()
                .filter(|case| case.stratum_id == stratum)
                .cloned()
                .collect::<Vec<_>>();
            Ok((stratum, summarize_metrics(&selected, &plan.bootstrap)?))
        })
        .collect::<Result<BTreeMap<_, _>, EvalError>>()?;
    let report = PairedReliabilityReport {
        schema_version: PAIRED_RELIABILITY_SCHEMA_V1.into(),
        plan_sha256: plan.sha256()?,
        incumbent_artifact_sha256: sha256(&serde_json::to_vec(incumbent)?),
        candidate_artifact_sha256: sha256(&serde_json::to_vec(candidate)?),
        bootstrap: plan.bootstrap.clone(),
        repetitions_per_case,
        overall,
        by_stratum,
        cases,
    };
    report.validate()?;
    Ok(report)
}

fn uniform_repetitions(keys: &[CaseTrialKey]) -> Result<u32, EvalError> {
    let mut repetitions = BTreeMap::<(String, String, String), Vec<u32>>::new();
    for key in keys {
        repetitions
            .entry((
                key.stratum_id.clone(),
                key.family_id.clone(),
                key.case_id.clone(),
            ))
            .or_default()
            .push(key.repetition);
    }
    let mut expected = None;
    for observed in repetitions.values_mut() {
        observed.sort_unstable();
        let k = u32::try_from(observed.len())
            .map_err(|_| EvalError::InvalidInput("too many task repetitions".into()))?;
        if observed.iter().copied().ne(1..=k) || expected.is_some_and(|prior| prior != k) {
            return Err(EvalError::InvalidInput(
                "pass^k requires every case to have the same contiguous repetitions 1..=k".into(),
            ));
        }
        expected = Some(k);
    }
    expected.ok_or_else(|| EvalError::InvalidInput("pass^k requires at least one case".into()))
}

type CaseIdentity = (String, String, String);

fn receipts_by_case(artifact: &PairedRunArtifact) -> BTreeMap<CaseIdentity, Vec<&TrialReceipt>> {
    let mut cases = BTreeMap::<CaseIdentity, Vec<&TrialReceipt>>::new();
    for receipt in &artifact.receipts {
        cases
            .entry((
                receipt.key.stratum_id.clone(),
                receipt.key.family_id.clone(),
                receipt.key.case_id.clone(),
            ))
            .or_default()
            .push(receipt);
    }
    cases
}

fn pass_all_repetitions(
    receipts: &[&TrialReceipt],
    k: u32,
    passed: impl Fn(&TrialReceipt) -> bool,
) -> Result<bool, EvalError> {
    let mut repetitions = receipts
        .iter()
        .map(|receipt| receipt.key.repetition)
        .collect::<Vec<_>>();
    repetitions.sort_unstable();
    if repetitions.iter().copied().ne(1..=k) {
        return Err(EvalError::InvalidInput(
            "run artifact does not contain every pass^k repetition".into(),
        ));
    }
    Ok(receipts.iter().all(|receipt| passed(receipt)))
}

fn summarize_metrics(
    cases: &[ReliabilityCaseResult],
    bootstrap: &crate::paired::BootstrapConfig,
) -> Result<PairedReliabilityMetrics, EvalError> {
    Ok(PairedReliabilityMetrics {
        direct_memory_success: summarize_cases(
            cases,
            bootstrap,
            |case| case.incumbent_direct_memory_pass_k,
            |case| case.candidate_direct_memory_pass_k,
        )?,
        task_success: summarize_cases(
            cases,
            bootstrap,
            |case| case.incumbent_task_success_pass_k,
            |case| case.candidate_task_success_pass_k,
        )?,
    })
}

fn summarize_cases(
    cases: &[ReliabilityCaseResult],
    bootstrap: &crate::paired::BootstrapConfig,
    incumbent_passed: impl Fn(&ReliabilityCaseResult) -> bool,
    candidate_passed: impl Fn(&ReliabilityCaseResult) -> bool,
) -> Result<PairedPassKSummary, EvalError> {
    if cases.is_empty() {
        return Err(EvalError::InvalidInput(
            "pass^k summary requires at least one case".into(),
        ));
    }
    let case_count = u64::try_from(cases.len())
        .map_err(|_| EvalError::InvalidInput("too many reliability cases".into()))?;
    let incumbent_passed_cases = cases.iter().filter(|case| incumbent_passed(case)).count() as u64;
    let candidate_passed_cases = cases.iter().filter(|case| candidate_passed(case)).count() as u64;
    let mut family_case_rates = BTreeMap::<String, (Vec<f64>, Vec<f64>)>::new();
    for case in cases {
        let family_id = serde_json::to_string(&(&case.stratum_id, &case.family_id))?;
        let rates = family_case_rates.entry(family_id).or_default();
        rates.0.push(f64::from(u8::from(incumbent_passed(case))));
        rates.1.push(f64::from(u8::from(candidate_passed(case))));
    }
    let family_deltas = family_case_rates
        .iter()
        .map(|(family, (incumbent, candidate))| {
            let incumbent_rate = mean(incumbent);
            let candidate_rate = mean(candidate);
            (family.clone(), candidate_rate - incumbent_rate)
        })
        .collect::<BTreeMap<_, _>>();
    let incumbent_family_rate = mean(
        &family_case_rates
            .values()
            .map(|(incumbent, _)| mean(incumbent))
            .collect::<Vec<_>>(),
    );
    let candidate_family_rate = mean(
        &family_case_rates
            .values()
            .map(|(_, candidate)| mean(candidate))
            .collect::<Vec<_>>(),
    );
    let independent_family_count = u32::try_from(family_case_rates.len())
        .map_err(|_| EvalError::InvalidInput("too many reliability families".into()))?;
    let interval = if family_deltas.len() >= 2 {
        Some(cluster_bootstrap(&family_deltas, bootstrap)?)
    } else {
        None
    };
    Ok(PairedPassKSummary {
        independent_family_count,
        incumbent: ArmPassKSummary {
            passed_cases: incumbent_passed_cases,
            case_count,
            case_pass_rate: incumbent_passed_cases as f64 / case_count as f64,
            equal_family_pass_rate: incumbent_family_rate,
        },
        candidate: ArmPassKSummary {
            passed_cases: candidate_passed_cases,
            case_count,
            case_pass_rate: candidate_passed_cases as f64 / case_count as f64,
            equal_family_pass_rate: candidate_family_rate,
        },
        pass_k_gain: candidate_family_rate - incumbent_family_rate,
        family_deltas,
        interval,
    })
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
