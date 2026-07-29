use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Instant;

use async_trait::async_trait;

use crate::artifact::{GateStatus, derive_run_verdict};
use crate::domain::*;
use crate::error::EvalError;
use crate::profile::ProfileManifest;
use crate::reducer::SuiteReducer;

pub struct RunContext {
    pub profile: EvalProfile,
}

pub struct RunRequest {
    pub manifest: ProfileManifest,
    pub manifest_path: PathBuf,
    pub artifact_path: PathBuf,
    pub baseline: Option<crate::RunArtifact>,
    pub suite_filter: BTreeSet<SuiteId>,
    pub issues: Vec<RunIssue>,
}

#[async_trait]
pub trait EvalSuite: Send + Sync {
    fn id(&self) -> &str;
    fn mode(&self) -> EvalMode;
    fn expected_case_ids(&self) -> &[EvalCaseId];
    fn reducer(&self) -> &dyn SuiteReducer;
    async fn run(&self, context: &RunContext) -> Vec<EvalCaseOutcome>;
}

pub struct Runner {
    suites: Vec<Box<dyn EvalSuite>>,
}

impl Runner {
    pub fn new(suites: Vec<Box<dyn EvalSuite>>) -> Self {
        Self { suites }
    }

    pub async fn run(&self, request: &RunRequest) -> Result<crate::RunArtifact, EvalError> {
        let started = Instant::now();
        let profile = request.manifest.profile;
        let context = RunContext { profile };

        let mut all_outcomes = Vec::new();
        let mut expected_ids = Vec::new();
        let mut suite_summaries = Vec::new();
        let mut issues = request.issues.clone();

        for suite in &self.suites {
            if !request.suite_filter.is_empty()
                && !request.suite_filter.contains(&SuiteId::parse(suite.id())?)
            {
                continue;
            }

            expected_ids.extend(suite.expected_case_ids().iter().cloned());

            let outcomes = suite.run(&context).await;

            if let Some(decl) = request
                .manifest
                .suites
                .iter()
                .find(|decl| decl.id == suite.id())
                && let Some(expected) = decl.expected_coverage.as_ref()
                && outcomes.len() != expected.exact_cases
            {
                issues.push(RunIssue {
                    stage: RunStage::Coverage,
                    suite_id: Some(SuiteId::parse(suite.id())?),
                    message: format!(
                        "suite '{}' produced {} cases, expected exactly {}",
                        suite.id(),
                        outcomes.len(),
                        expected.exact_cases
                    ),
                });
            }

            let summaries = suite.reducer().reduce(&outcomes)?;
            suite_summaries.extend(summaries);

            all_outcomes.extend(outcomes);
        }

        all_outcomes.sort_by(|a, b| {
            a.suite_id()
                .cmp(b.suite_id())
                .then(a.case_id().cmp(b.case_id()))
        });

        expected_ids.sort();
        expected_ids.dedup();

        let duration_ms = started.elapsed().as_millis() as u64;

        let budget_status = if request.manifest.time_budget_seconds > 0 {
            let budget_ms = request.manifest.time_budget_seconds * 1000;
            if duration_ms > budget_ms {
                Some(GateStatus::Failed)
            } else {
                Some(GateStatus::Passed)
            }
        } else {
            None
        };

        let selected_gates: Vec<_> = request
            .manifest
            .gates
            .iter()
            .filter(|gate| {
                if request.suite_filter.is_empty() {
                    return true;
                }
                let Ok(suite_id) = SuiteId::parse(&gate.target.suite_id) else {
                    return false;
                };
                request.suite_filter.contains(&suite_id)
            })
            .cloned()
            .collect();
        let gates = crate::evaluate_gates(
            &selected_gates,
            &crate::RunArtifact {
                schema_version: crate::EVAL_ARTIFACT_SCHEMA_V1.to_string(),
                run_id: "pending".into(),
                profile,
                started_at: chrono::Utc::now(),
                duration_ms,
                expected_case_ids: expected_ids.clone(),
                expected_cases: vec![],
                outcomes: all_outcomes.clone(),
                suite_summaries: suite_summaries.clone(),
                gates: vec![],
                fingerprint: crate::RunFingerprint::capture(),
                budget_status: None,
                verdict: crate::domain::RunVerdict::default(),
                issues: vec![],
            },
            request.baseline.as_ref(),
        )?;

        let fingerprint = crate::RunFingerprint::capture();

        let budget = budget_status.unwrap_or(GateStatus::Invalid);
        let verdict = derive_run_verdict(&all_outcomes, &gates, budget.clone(), &issues);

        let artifact = crate::RunArtifact {
            schema_version: crate::EVAL_ARTIFACT_SCHEMA_V2.to_string(),
            run_id: format!("run-{}", chrono::Utc::now().timestamp()),
            profile,
            started_at: chrono::Utc::now(),
            duration_ms,
            expected_case_ids: expected_ids,
            expected_cases: vec![],
            outcomes: all_outcomes,
            suite_summaries,
            gates,
            fingerprint,
            budget_status: Some(budget),
            verdict,
            issues,
        };

        artifact.validate()?;

        Ok(artifact)
    }
}
