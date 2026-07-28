use std::path::Path;

use serde::Deserialize;

use crate::domain::EvalProfile;
use crate::error::EvalError;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileManifest {
    pub schema_version: String,
    pub profile: EvalProfile,
    pub time_budget_seconds: u64,
    pub suites: Vec<SuiteDecl>,
    pub gates: Vec<GateDecl>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteDecl {
    pub id: String,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub corpus_root: Option<String>,
    #[serde(default)]
    pub expected_coverage: Option<ExpectedCoverage>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedCoverage {
    pub min_cases: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateDecl {
    pub metric: String,
    pub hard_floor: Option<f64>,
    pub regression_budget: Option<f64>,
    #[serde(default)]
    pub split: Option<String>,
}

impl ProfileManifest {
    pub fn parse(raw: &str) -> Result<Self, EvalError> {
        let manifest: ProfileManifest =
            serde_json::from_str(raw).map_err(|e| EvalError::InvalidConfig(e.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn load(path: &Path) -> Result<Self, EvalError> {
        let raw = std::fs::read_to_string(path).map_err(|source| EvalError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&raw)
    }

    fn validate(&self) -> Result<(), EvalError> {
        if self.schema_version != "memory-mcp-eval-profile/v1" {
            return Err(EvalError::InvalidConfig(format!(
                "unsupported profile schema version: {}",
                self.schema_version
            )));
        }
        if self.suites.is_empty() {
            return Err(EvalError::InvalidConfig(
                "profile must declare at least one suite".into(),
            ));
        }
        if self.time_budget_seconds == 0 {
            return Err(EvalError::InvalidConfig(
                "time budget must be positive".into(),
            ));
        }

        let mut seen_ids = std::collections::HashSet::new();
        for suite in &self.suites {
            if !seen_ids.insert(suite.id.as_str()) {
                return Err(EvalError::InvalidConfig(format!(
                    "duplicate suite ID: {}",
                    suite.id
                )));
            }
            if suite.expected_coverage.is_none() && self.profile != EvalProfile::Nightly {
                return Err(EvalError::InvalidConfig(format!(
                    "suite {} must declare expected_coverage for non-nightly profiles",
                    suite.id
                )));
            }
        }

        for gate in &self.gates {
            if gate.metric.is_empty() {
                return Err(EvalError::InvalidConfig(
                    "gate metric must not be empty".into(),
                ));
            }
            if gate.hard_floor.is_none() && gate.regression_budget.is_none() {
                return Err(EvalError::InvalidConfig(format!(
                    "gate for {} must declare at least one of hard_floor or regression_budget",
                    gate.metric
                )));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_pr_profile_json() -> &'static str {
        r#"{
            "schema_version": "memory-mcp-eval-profile/v1",
            "profile": "pr",
            "time_budget_seconds": 600,
            "suites": [
                {
                    "id": "local-retrieval",
                    "expected_coverage": { "min_cases": 50 }
                }
            ],
            "gates": [
                { "metric": "recall_at_5", "hard_floor": 0.90 }
            ]
        }"#
    }

    #[test]
    fn pr_profile_rejects_missing_expected_coverage() {
        let raw = r#"{"schema_version":"memory-mcp-eval-profile/v1","profile":"pr",
            "time_budget_seconds":600,"suites":[{"id":"s1"}],"gates":[]}"#;
        assert!(ProfileManifest::parse(raw).is_err());
    }

    #[test]
    fn pr_profile_rejects_empty_suites() {
        let raw = r#"{"schema_version":"memory-mcp-eval-profile/v1","profile":"pr",
            "time_budget_seconds":600,"suites":[],"gates":[]}"#;
        assert!(ProfileManifest::parse(raw).is_err());
    }

    #[test]
    fn pr_profile_rejects_zero_budget() {
        let raw = r#"{"schema_version":"memory-mcp-eval-profile/v1","profile":"pr",
            "time_budget_seconds":0,"suites":[{"id":"s1","expected_coverage":{"min_cases":1}}],"gates":[]}"#;
        assert!(ProfileManifest::parse(raw).is_err());
    }

    #[test]
    fn pr_profile_rejects_duplicate_suite_ids() {
        let raw = r#"{"schema_version":"memory-mcp-eval-profile/v1","profile":"pr",
            "time_budget_seconds":600,"suites":[
                {"id":"s1","expected_coverage":{"min_cases":1}},
                {"id":"s1","expected_coverage":{"min_cases":1}}
            ],"gates":[]}"#;
        assert!(ProfileManifest::parse(raw).is_err());
    }

    #[test]
    fn pr_profile_rejects_unknown_fields() {
        let raw = r#"{"schema_version":"memory-mcp-eval-profile/v1","profile":"pr",
            "time_budget_seconds":600,"suites":[{"id":"s1","expected_coverage":{"min_cases":1}}],
            "gates":[],"unexpected":1}"#;
        assert!(ProfileManifest::parse(raw).is_err());
    }

    #[test]
    fn pr_profile_rejects_wrong_schema_version() {
        let raw = r#"{"schema_version":"wrong","profile":"pr",
            "time_budget_seconds":600,"suites":[{"id":"s1","expected_coverage":{"min_cases":1}}],"gates":[]}"#;
        assert!(ProfileManifest::parse(raw).is_err());
    }

    #[test]
    fn gate_requires_floor_or_budget() {
        let raw = r#"{"schema_version":"memory-mcp-eval-profile/v1","profile":"pr",
            "time_budget_seconds":600,
            "suites":[{"id":"s1","expected_coverage":{"min_cases":1}}],
            "gates":[{"metric":"recall"}]}"#;
        assert!(ProfileManifest::parse(raw).is_err());
    }

    #[test]
    fn valid_profile_parses() {
        let manifest = ProfileManifest::parse(valid_pr_profile_json()).unwrap();
        assert_eq!(manifest.time_budget_seconds, 600);
        assert_eq!(manifest.suites.len(), 1);
        assert_eq!(manifest.gates.len(), 1);
    }

    #[test]
    fn nightly_profile_allows_missing_coverage() {
        let raw = r#"{"schema_version":"memory-mcp-eval-profile/v1","profile":"nightly",
            "time_budget_seconds":3600,"suites":[{"id":"e2e"}],"gates":[]}"#;
        assert!(ProfileManifest::parse(raw).is_ok());
    }

    #[test]
    fn loads_real_pr_profile() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals/profiles/pr.json");
        if path.exists() {
            let manifest = ProfileManifest::load(&path).unwrap();
            assert_eq!(manifest.time_budget_seconds, 600);
            assert!(!manifest.suites.is_empty());
        }
    }
}
