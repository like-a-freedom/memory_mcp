use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

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

/// Receives operational progress updates while suites run.
///
/// Progress is deliberately separate from [`RunArtifact`]: it is user-facing
/// execution feedback, not evaluation data. Implementations must be cheap and
/// non-blocking enough not to distort suite timings.
pub trait ProgressReporter: Send + Sync {
    fn suite_started(&self, position: usize, total: usize, suite_id: &str, expected_cases: usize);

    fn suite_finished(
        &self,
        position: usize,
        total: usize,
        suite_id: &str,
        outcome_count: usize,
        elapsed: Duration,
    );
}

/// No-op progress reporter used by library callers that do not need output.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopProgressReporter;

impl ProgressReporter for NoopProgressReporter {
    fn suite_started(
        &self,
        _position: usize,
        _total: usize,
        _suite_id: &str,
        _expected_cases: usize,
    ) {
    }

    fn suite_finished(
        &self,
        _position: usize,
        _total: usize,
        _suite_id: &str,
        _outcome_count: usize,
        _elapsed: Duration,
    ) {
    }
}

pub struct Runner {
    suites: Vec<Box<dyn EvalSuite>>,
}

impl Runner {
    pub fn new(suites: Vec<Box<dyn EvalSuite>>) -> Self {
        Self { suites }
    }

    pub async fn run(&self, request: &RunRequest) -> Result<crate::RunArtifact, EvalError> {
        self.run_with_progress(request, &NoopProgressReporter).await
    }

    pub async fn run_with_progress(
        &self,
        request: &RunRequest,
        progress: &dyn ProgressReporter,
    ) -> Result<crate::RunArtifact, EvalError> {
        let started = Instant::now();
        let profile = request.manifest.profile;
        let context = RunContext { profile };

        let mut selected_suites: Vec<&dyn EvalSuite> = Vec::new();
        for suite in &self.suites {
            if !request.suite_filter.is_empty()
                && !request.suite_filter.contains(&SuiteId::parse(suite.id())?)
            {
                continue;
            }
            selected_suites.push(suite.as_ref());
        }
        let suite_total = selected_suites.len();

        let mut all_outcomes = Vec::new();
        let mut expected_ids = Vec::new();
        let mut suite_summaries = Vec::new();
        let mut issues = request.issues.clone();

        for (suite_index, suite) in selected_suites.into_iter().enumerate() {
            let position = suite_index + 1;
            progress.suite_started(
                position,
                suite_total,
                suite.id(),
                suite.expected_case_ids().len(),
            );
            expected_ids.extend(suite.expected_case_ids().iter().cloned());

            let suite_started = Instant::now();
            let outcomes = suite.run(&context).await;
            progress.suite_finished(
                position,
                suite_total,
                suite.id(),
                outcomes.len(),
                suite_started.elapsed(),
            );

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

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::profile::SuiteDecl;
    use crate::reducer::CountReducer;

    struct StaticSuite {
        id: String,
        expected_ids: Vec<EvalCaseId>,
        outcomes: Vec<EvalCaseOutcome>,
        reducer: CountReducer,
    }

    impl StaticSuite {
        fn new(id: &str, case_id: &str) -> Self {
            let outcome = EvalCaseOutcome::new(
                id,
                case_id,
                EvalMode::Lifecycle,
                CorpusSplit::Test,
                LabelTrust::Official,
                CaseStatus::Passed,
            );
            Self {
                id: id.to_string(),
                expected_ids: vec![EvalCaseId::parse(case_id).unwrap()],
                outcomes: vec![outcome],
                reducer: CountReducer::new(id),
            }
        }
    }

    #[async_trait]
    impl EvalSuite for StaticSuite {
        fn id(&self) -> &str {
            &self.id
        }

        fn mode(&self) -> EvalMode {
            EvalMode::Lifecycle
        }

        fn expected_case_ids(&self) -> &[EvalCaseId] {
            &self.expected_ids
        }

        fn reducer(&self) -> &dyn SuiteReducer {
            &self.reducer
        }

        async fn run(&self, _context: &RunContext) -> Vec<EvalCaseOutcome> {
            self.outcomes.clone()
        }
    }

    #[derive(Default)]
    struct RecordingProgress {
        events: Mutex<Vec<String>>,
    }

    impl ProgressReporter for RecordingProgress {
        fn suite_started(
            &self,
            position: usize,
            total: usize,
            suite_id: &str,
            expected_cases: usize,
        ) {
            self.events.lock().unwrap().push(format!(
                "started:{position}/{total}:{suite_id}:{expected_cases}"
            ));
        }

        fn suite_finished(
            &self,
            position: usize,
            total: usize,
            suite_id: &str,
            outcome_count: usize,
            _elapsed: Duration,
        ) {
            self.events.lock().unwrap().push(format!(
                "finished:{position}/{total}:{suite_id}:{outcome_count}"
            ));
        }
    }

    fn request(suite_ids: &[&str], suite_filter: BTreeSet<SuiteId>) -> RunRequest {
        RunRequest {
            manifest: ProfileManifest {
                schema_version: "memory-mcp-eval-profile/v1".into(),
                profile: EvalProfile::Nightly,
                time_budget_seconds: 60,
                suites: suite_ids
                    .iter()
                    .map(|id| SuiteDecl {
                        id: (*id).into(),
                        mode: None,
                        corpus_root: None,
                        expected_coverage: None,
                    })
                    .collect(),
                gates: vec![],
            },
            manifest_path: PathBuf::from("profile.json"),
            artifact_path: PathBuf::from("artifact.json"),
            baseline: None,
            suite_filter,
            issues: vec![],
        }
    }

    #[tokio::test]
    async fn progress_reports_only_selected_suites_in_order() {
        let runner = Runner::new(vec![
            Box::new(StaticSuite::new("first", "first-case")),
            Box::new(StaticSuite::new("second", "second-case")),
        ]);
        let filter = BTreeSet::from([SuiteId::parse("second").unwrap()]);
        let reporter = RecordingProgress::default();

        let artifact = runner
            .run_with_progress(&request(&["first", "second"], filter), &reporter)
            .await
            .unwrap();

        assert_eq!(artifact.outcomes.len(), 1);
        assert_eq!(
            *reporter.events.lock().unwrap(),
            vec!["started:1/1:second:1", "finished:1/1:second:1"]
        );
    }

    #[tokio::test]
    async fn run_uses_the_silent_default_reporter() {
        let runner = Runner::new(vec![Box::new(StaticSuite::new("suite", "case"))]);

        let artifact = runner
            .run(&request(&["suite"], BTreeSet::new()))
            .await
            .unwrap();

        assert_eq!(artifact.outcomes.len(), 1);
    }
}
