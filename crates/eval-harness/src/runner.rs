use async_trait::async_trait;

use crate::domain::*;
use crate::error::EvalError;

pub struct RunContext {
    pub profile: EvalProfile,
}

#[async_trait]
pub trait EvalSuite: Send + Sync {
    fn id(&self) -> &str;
    fn mode(&self) -> EvalMode;
    fn expected_case_ids(&self) -> &[EvalCaseId];
    async fn run(&self, context: &RunContext) -> Vec<EvalCaseOutcome>;
}

pub struct Runner {
    suites: Vec<Box<dyn EvalSuite>>,
}

impl Runner {
    pub fn new(suites: Vec<Box<dyn EvalSuite>>) -> Self {
        Self { suites }
    }

    pub async fn run(
        &self,
        profile: EvalProfile,
        baseline: Option<&crate::RunArtifact>,
    ) -> Result<crate::RunArtifact, EvalError> {
        let context = RunContext { profile };

        let mut all_outcomes = Vec::new();
        let mut expected_ids = Vec::new();
        let mut suite_summaries = Vec::new();

        for suite in &self.suites {
            expected_ids.extend(suite.expected_case_ids().iter().cloned());

            let outcomes = suite.run(&context).await;

            let mut passed = 0;
            let mut quality_failed = 0;
            let mut invalid = 0;
            for outcome in &outcomes {
                match outcome.status {
                    CaseStatus::Passed => passed += 1,
                    CaseStatus::QualityFailed => quality_failed += 1,
                    CaseStatus::Invalid => invalid += 1,
                }
            }

            let mut metrics = std::collections::BTreeMap::new();
            for outcome in &outcomes {
                for (key, value) in &outcome.metrics {
                    metrics.entry(key.clone()).or_insert(*value);
                }
            }

            suite_summaries.push(crate::artifact::SuiteSummary {
                suite_id: suite.id().to_string(),
                mode: suite.mode(),
                total: outcomes.len(),
                passed,
                quality_failed,
                invalid,
                metrics,
            });

            all_outcomes.extend(outcomes);
        }

        all_outcomes.sort_by(|a, b| {
            a.suite_id()
                .cmp(b.suite_id())
                .then(a.case_id().cmp(b.case_id()))
        });

        expected_ids.sort();
        expected_ids.dedup();

        let profile_path = match profile {
            EvalProfile::Pr => "evals/profiles/pr.json",
            EvalProfile::Release => "evals/profiles/release.json",
            EvalProfile::Nightly => "evals/profiles/nightly.json",
        };

        let profile_manifest =
            crate::ProfileManifest::load(std::path::Path::new(profile_path)).ok();

        let gates = if let Some(ref manifest) = profile_manifest {
            let artifact_so_far = crate::RunArtifact {
                schema_version: crate::EVAL_ARTIFACT_SCHEMA_V1.to_string(),
                run_id: "pending".into(),
                profile,
                started_at: chrono::Utc::now(),
                duration_ms: 0,
                expected_case_ids: expected_ids.clone(),
                outcomes: all_outcomes.clone(),
                suite_summaries: suite_summaries.clone(),
                gates: vec![],
                fingerprint: crate::RunFingerprint::capture(),
            };
            crate::evaluate_gates(&manifest.gates, &artifact_so_far, baseline)?
        } else {
            vec![]
        };

        let fingerprint = crate::RunFingerprint::capture();

        let artifact = crate::RunArtifact {
            schema_version: crate::EVAL_ARTIFACT_SCHEMA_V1.to_string(),
            run_id: format!("run-{}", chrono::Utc::now().timestamp()),
            profile,
            started_at: chrono::Utc::now(),
            duration_ms: 0,
            expected_case_ids: expected_ids,
            outcomes: all_outcomes,
            suite_summaries,
            gates,
            fingerprint,
        };

        artifact.validate()?;

        Ok(artifact)
    }
}
