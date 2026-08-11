use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::*;
use crate::error::EvalError;

pub use crate::domain::derive_run_verdict;

pub const EVAL_ARTIFACT_SCHEMA_V1: &str = "memory-mcp-eval/v1";
pub const EVAL_ARTIFACT_SCHEMA_V2: &str = "memory-mcp-eval/v2";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunFingerprint {
    pub rust_version: String,
    pub os_arch: String,
    pub package_version: String,
    pub build_profile: String,
    pub enabled_features: Vec<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub device: Option<String>,
    pub configuration_hash: String,
    pub git_commit: Option<String>,
    pub evaluator_versions: BTreeMap<String, String>,
    pub profile_digest: String,
}

impl RunFingerprint {
    pub fn capture() -> Self {
        Self {
            rust_version: option_env!("CARGO_PKG_RUST_VERSION")
                .unwrap_or("unknown")
                .to_string(),
            os_arch: std::env::consts::OS.to_string() + "/" + std::env::consts::ARCH,
            package_version: env!("CARGO_PKG_VERSION").to_string(),
            build_profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
            .to_string(),
            enabled_features: vec![],
            provider: None,
            model: None,
            device: None,
            configuration_hash: "uncomputed".to_string(),
            git_commit: option_env!("GIT_COMMIT").map(String::from),
            evaluator_versions: BTreeMap::new(),
            profile_digest: "uncomputed".to_string(),
        }
    }

    #[cfg(test)]
    pub fn default_for_test() -> Self {
        Self {
            rust_version: "test".into(),
            os_arch: "test".into(),
            package_version: "0.0.0".into(),
            build_profile: "test".into(),
            enabled_features: vec![],
            provider: None,
            model: None,
            device: None,
            configuration_hash: "test".into(),
            git_commit: None,
            evaluator_versions: BTreeMap::new(),
            profile_digest: "test".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuiteSummary {
    pub suite_id: String,
    pub mode: EvalMode,
    pub total: usize,
    pub passed: usize,
    pub quality_failed: usize,
    pub invalid: usize,
    pub metrics: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateDecision {
    pub suite_id: String,
    pub metric: String,
    pub observed: f64,
    pub hard_floor: Option<f64>,
    pub baseline: Option<f64>,
    pub regression_budget: Option<f64>,
    pub status: GateStatus,
    pub reason: GateFailureReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateStatus {
    Passed,
    Failed,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateFailureReason {
    HardFloorNotMet,
    RegressionBudgetExceeded,
    MissingBaseline,
    IncompatibleBaseline,
    MissingMetric,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunArtifact {
    pub schema_version: String,
    pub run_id: String,
    pub profile: EvalProfile,
    pub started_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub expected_case_ids: Vec<EvalCaseId>,
    #[serde(default)]
    pub expected_cases: Vec<CaseKey>,
    pub outcomes: Vec<EvalCaseOutcome>,
    pub suite_summaries: Vec<SuiteSummary>,
    pub gates: Vec<GateDecision>,
    pub fingerprint: RunFingerprint,
    #[serde(default)]
    pub budget_status: Option<GateStatus>,
    #[serde(default)]
    pub verdict: RunVerdict,
    #[serde(default)]
    pub issues: Vec<RunIssue>,
}

impl RunArtifact {
    pub fn validate(&self) -> Result<(), EvalError> {
        if self.schema_version != EVAL_ARTIFACT_SCHEMA_V1
            && self.schema_version != EVAL_ARTIFACT_SCHEMA_V2
        {
            return Err(EvalError::InvalidInput(format!(
                "unsupported schema version: {}",
                self.schema_version
            )));
        }
        if self.expected_case_ids.is_empty() && self.expected_cases.is_empty() {
            return Err(EvalError::InvalidInput(
                "expected case IDs must not be empty".into(),
            ));
        }
        if self.outcomes.is_empty() {
            return Err(EvalError::InvalidInput("outcomes must not be empty".into()));
        }

        let mut seen_expected = std::collections::HashSet::new();
        for id in &self.expected_case_ids {
            if !seen_expected.insert(id.as_str()) {
                return Err(EvalError::InvalidInput(format!(
                    "duplicate expected case ID: {}",
                    id.as_str()
                )));
            }
        }

        let mut seen_case_keys = std::collections::HashSet::new();
        for case_key in &self.expected_cases {
            let key_str = format!(
                "{}::{}",
                case_key.suite_id.as_str(),
                case_key.case_id.as_str()
            );
            if !seen_case_keys.insert(key_str.clone()) {
                return Err(EvalError::InvalidInput(format!(
                    "duplicate expected case key: {key_str}"
                )));
            }
        }

        let mut seen_outcomes = std::collections::HashSet::new();
        for outcome in &self.outcomes {
            outcome.validate()?;

            let outcome_key = (
                outcome.case_key.suite_id.as_str(),
                outcome.case_key.case_id.as_str(),
            );

            if !seen_expected.contains(outcome.case_id().as_str()) {
                let key_str = format!(
                    "{}::{}",
                    outcome.case_key.suite_id.as_str(),
                    outcome.case_key.case_id.as_str()
                );
                if !seen_expected.contains(key_str.as_str()) {
                    return Err(EvalError::InvalidInput(format!(
                        "outcome for unexpected case: {}",
                        outcome.case_id().as_str()
                    )));
                }
            }

            if !seen_outcomes.insert(outcome_key) {
                return Err(EvalError::InvalidInput(format!(
                    "duplicate outcome for suite `{}` case `{}`",
                    outcome.case_key.suite_id.as_str(),
                    outcome.case_id().as_str()
                )));
            }
        }

        let outcome_keys: std::collections::HashSet<(String, String)> = self
            .outcomes
            .iter()
            .map(|o| {
                (
                    o.case_key.suite_id.as_str().to_string(),
                    o.case_key.case_id.as_str().to_string(),
                )
            })
            .collect();
        for id in &self.expected_case_ids {
            // Expected ids are bare corpus case ids; outcomes are
            // suite-scoped, so a bare id is covered when any suite produced a
            // case with that id.
            let present = outcome_keys
                .iter()
                .any(|(_, case_id)| case_id == id.as_str());
            if !present {
                return Err(EvalError::InvalidInput(format!(
                    "missing outcome for expected case ID: {}",
                    id.as_str()
                )));
            }
        }

        if self.schema_version == EVAL_ARTIFACT_SCHEMA_V2 {
            let budget = self.budget_status.clone().unwrap_or(GateStatus::Invalid);
            let recomputed = derive_run_verdict(&self.outcomes, &self.gates, budget, &self.issues);
            if self.verdict != recomputed {
                return Err(EvalError::InvalidInput(format!(
                    "stored verdict {:?} differs from recomputed {:?}",
                    self.verdict, recomputed
                )));
            }
        }

        Ok(())
    }

    #[cfg(test)]
    pub fn fixture(outcomes: Vec<EvalCaseOutcome>, expected: Vec<EvalCaseId>) -> Self {
        use std::collections::BTreeMap;

        let budget_val = GateStatus::Passed;
        let verdict = derive_run_verdict(&outcomes, &[], budget_val.clone(), &[]);

        RunArtifact {
            schema_version: EVAL_ARTIFACT_SCHEMA_V1.to_string(),
            run_id: "test-run".into(),
            profile: EvalProfile::Pr,
            started_at: Utc::now(),
            duration_ms: 0,
            expected_case_ids: expected,
            expected_cases: vec![],
            outcomes,
            suite_summaries: vec![],
            gates: vec![],
            fingerprint: RunFingerprint {
                rust_version: "test".into(),
                os_arch: "test".into(),
                package_version: "0.0.0".into(),
                build_profile: "test".into(),
                enabled_features: vec![],
                provider: None,
                model: None,
                device: None,
                configuration_hash: "test".into(),
                git_commit: None,
                evaluator_versions: BTreeMap::new(),
                profile_digest: "test".into(),
            },
            budget_status: Some(budget_val),
            verdict,
            issues: vec![],
        }
    }
}

pub fn write_artifact(path: &Path, artifact: &RunArtifact) -> Result<(), EvalError> {
    let tmp_path = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(artifact)?;
    std::fs::write(&tmp_path, &json).map_err(|source| EvalError::Io {
        path: tmp_path.clone(),
        source,
    })?;
    std::fs::File::open(&tmp_path)
        .and_then(|f| f.sync_all())
        .map_err(|source| EvalError::Io {
            path: tmp_path.clone(),
            source,
        })?;
    std::fs::rename(&tmp_path, path).map_err(|source| EvalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passed_fixture(case_id: &str) -> EvalCaseOutcome {
        EvalCaseOutcome::new(
            "test-suite",
            case_id,
            EvalMode::RetrievalOnly,
            CorpusSplit::Development,
            LabelTrust::Official,
            CaseStatus::Passed,
        )
    }

    #[test]
    fn empty_run_is_invalid() {
        let artifact = RunArtifact::fixture(Vec::new(), vec![EvalCaseId::parse("case-1").unwrap()]);
        assert!(matches!(
            artifact.validate(),
            Err(EvalError::InvalidInput(_))
        ));
    }

    #[test]
    fn selected_case_must_appear_exactly_once() {
        let outcome = passed_fixture("case-1");
        let artifact = RunArtifact::fixture(
            vec![outcome.clone(), outcome],
            vec![EvalCaseId::parse("case-1").unwrap()],
        );
        assert!(artifact.validate().is_err());
    }

    #[test]
    fn missing_outcome_for_expected_case_is_invalid() {
        let artifact = RunArtifact::fixture(
            vec![passed_fixture("case-1")],
            vec![
                EvalCaseId::parse("case-1").unwrap(),
                EvalCaseId::parse("case-2").unwrap(),
            ],
        );
        assert!(artifact.validate().is_err());
    }

    #[test]
    fn outcome_for_unexpected_case_is_invalid() {
        let artifact = RunArtifact::fixture(
            vec![passed_fixture("case-1")],
            vec![EvalCaseId::parse("case-2").unwrap()],
        );
        assert!(artifact.validate().is_err());
    }

    #[test]
    fn valid_single_case_artifact_passes() {
        let artifact = RunArtifact::fixture(
            vec![passed_fixture("case-1")],
            vec![EvalCaseId::parse("case-1").unwrap()],
        );
        assert!(artifact.validate().is_ok());
    }

    #[test]
    fn duplicate_expected_ids_are_invalid() {
        let artifact = RunArtifact::fixture(
            vec![passed_fixture("case-1")],
            vec![
                EvalCaseId::parse("case-1").unwrap(),
                EvalCaseId::parse("case-1").unwrap(),
            ],
        );
        assert!(artifact.validate().is_err());
    }

    #[test]
    fn wrong_schema_version_is_invalid() {
        let mut artifact = RunArtifact::fixture(
            vec![passed_fixture("case-1")],
            vec![EvalCaseId::parse("case-1").unwrap()],
        );
        artifact.schema_version = "wrong-version".into();
        assert!(artifact.validate().is_err());
    }

    #[test]
    fn write_artifact_writes_valid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test-artifact.json");
        let artifact = RunArtifact::fixture(
            vec![passed_fixture("case-1")],
            vec![EvalCaseId::parse("case-1").unwrap()],
        );
        write_artifact(&path, &artifact).unwrap();
        assert!(path.exists());

        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            written["schema_version"].as_str().unwrap(),
            EVAL_ARTIFACT_SCHEMA_V1
        );
    }

    #[test]
    fn write_artifact_is_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test-artifact.json");
        let artifact = RunArtifact::fixture(
            vec![passed_fixture("case-1")],
            vec![EvalCaseId::parse("case-1").unwrap()],
        );
        write_artifact(&path, &artifact).unwrap();

        let tmp_path = path.with_extension("json.tmp");
        assert!(!tmp_path.exists(), "tmp file should not remain after write");
    }
}
