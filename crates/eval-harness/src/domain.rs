use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::EvalError;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EvalCaseId(String);

impl EvalCaseId {
    pub fn parse(raw: impl Into<String>) -> Result<Self, EvalError> {
        let value = raw.into();
        if value.trim().is_empty() {
            return Err(EvalError::InvalidConfig("case id must not be empty".into()));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseStatus {
    Passed,
    QualityFailed,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvalProfile {
    Pr,
    Release,
    Nightly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvalMode {
    RetrievalOnly,
    EndToEnd,
    Lifecycle,
    Performance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorpusSplit {
    Development,
    Test,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelTrust {
    Official,
    Reviewed,
    Weak,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunCompleteness {
    Complete,
    Shard { index: u32, count: u32 },
}

impl RunCompleteness {
    pub fn validate(&self) -> Result<(), EvalError> {
        match self {
            RunCompleteness::Complete => Ok(()),
            RunCompleteness::Shard { index, count } => {
                if *count == 0 {
                    return Err(EvalError::InvalidConfig(
                        "shard count must be greater than zero".into(),
                    ));
                }
                if *index >= *count {
                    return Err(EvalError::InvalidConfig(format!(
                        "shard index {index} must be less than count {count}"
                    )));
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalCaseOutcome {
    pub case_id: EvalCaseId,
    pub suite_id: String,
    pub mode: EvalMode,
    pub split: CorpusSplit,
    pub label_trust: LabelTrust,
    pub status: CaseStatus,
    #[serde(default)]
    pub metrics: BTreeMap<String, f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invalid_reason: Option<String>,
    #[serde(default)]
    pub failures: Vec<String>,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub attempts: u32,
}

impl EvalCaseOutcome {
    pub fn validate(&self) -> Result<(), EvalError> {
        if self.status == CaseStatus::Invalid && self.invalid_reason.is_none() {
            return Err(EvalError::InvalidInput(format!(
                "case {} has Invalid status but no invalid_reason",
                self.case_id.as_str()
            )));
        }
        if self.status != CaseStatus::Invalid && self.invalid_reason.is_some() {
            return Err(EvalError::InvalidInput(format!(
                "case {} has non-Invalid status but carries an invalid_reason",
                self.case_id.as_str()
            )));
        }
        for (key, value) in &self.metrics {
            if !value.is_finite() {
                return Err(EvalError::InvalidInput(format!(
                    "case {} has non-finite metric {} = {value}",
                    self.case_id.as_str(),
                    key
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_status_serializes_with_the_truth_contract_names() {
        assert_eq!(
            serde_json::to_string(&CaseStatus::Passed).unwrap(),
            "\"passed\""
        );
        assert_eq!(
            serde_json::to_string(&CaseStatus::QualityFailed).unwrap(),
            "\"quality_failed\""
        );
        assert_eq!(
            serde_json::to_string(&CaseStatus::Invalid).unwrap(),
            "\"invalid\""
        );
    }

    #[test]
    fn eval_profile_serializes_with_snake_case() {
        assert_eq!(serde_json::to_string(&EvalProfile::Pr).unwrap(), "\"pr\"");
        assert_eq!(
            serde_json::to_string(&EvalProfile::Release).unwrap(),
            "\"release\""
        );
        assert_eq!(
            serde_json::to_string(&EvalProfile::Nightly).unwrap(),
            "\"nightly\""
        );
    }

    #[test]
    fn eval_mode_serializes_with_snake_case() {
        assert_eq!(
            serde_json::to_string(&EvalMode::RetrievalOnly).unwrap(),
            "\"retrieval_only\""
        );
        assert_eq!(
            serde_json::to_string(&EvalMode::EndToEnd).unwrap(),
            "\"end_to_end\""
        );
        assert_eq!(
            serde_json::to_string(&EvalMode::Lifecycle).unwrap(),
            "\"lifecycle\""
        );
        assert_eq!(
            serde_json::to_string(&EvalMode::Performance).unwrap(),
            "\"performance\""
        );
    }

    #[test]
    fn corpus_split_serializes_with_snake_case() {
        assert_eq!(
            serde_json::to_string(&CorpusSplit::Development).unwrap(),
            "\"development\""
        );
        assert_eq!(
            serde_json::to_string(&CorpusSplit::Test).unwrap(),
            "\"test\""
        );
    }

    #[test]
    fn label_trust_serializes_with_snake_case() {
        assert_eq!(
            serde_json::to_string(&LabelTrust::Official).unwrap(),
            "\"official\""
        );
        assert_eq!(
            serde_json::to_string(&LabelTrust::Reviewed).unwrap(),
            "\"reviewed\""
        );
        assert_eq!(
            serde_json::to_string(&LabelTrust::Weak).unwrap(),
            "\"weak\""
        );
    }

    #[test]
    fn case_id_rejects_empty_string() {
        assert!(EvalCaseId::parse("").is_err());
        assert!(EvalCaseId::parse("  ").is_err());
    }

    #[test]
    fn case_id_accepts_valid_string() {
        let id = EvalCaseId::parse("case-1").unwrap();
        assert_eq!(id.as_str(), "case-1");
    }

    #[test]
    fn run_completeness_shard_validates() {
        assert!(RunCompleteness::Complete.validate().is_ok());
        assert!(
            RunCompleteness::Shard { index: 0, count: 4 }
                .validate()
                .is_ok()
        );
        assert!(
            RunCompleteness::Shard { index: 0, count: 0 }
                .validate()
                .is_err()
        );
        assert!(
            RunCompleteness::Shard { index: 4, count: 4 }
                .validate()
                .is_err()
        );
    }

    #[test]
    fn case_outcome_invalid_requires_reason() {
        let outcome = EvalCaseOutcome {
            case_id: EvalCaseId::parse("case-1").unwrap(),
            suite_id: "test".into(),
            mode: EvalMode::RetrievalOnly,
            split: CorpusSplit::Development,
            label_trust: LabelTrust::Official,
            status: CaseStatus::Invalid,
            metrics: BTreeMap::new(),
            invalid_reason: None,
            failures: vec![],
            duration_ms: 0,
            attempts: 1,
        };
        assert!(outcome.validate().is_err());
    }

    #[test]
    fn case_outcome_passed_rejects_reason() {
        let outcome = EvalCaseOutcome {
            case_id: EvalCaseId::parse("case-1").unwrap(),
            suite_id: "test".into(),
            mode: EvalMode::RetrievalOnly,
            split: CorpusSplit::Development,
            label_trust: LabelTrust::Official,
            status: CaseStatus::Passed,
            metrics: BTreeMap::new(),
            invalid_reason: Some("should not be here".into()),
            failures: vec![],
            duration_ms: 0,
            attempts: 1,
        };
        assert!(outcome.validate().is_err());
    }

    #[test]
    fn case_outcome_rejects_non_finite_metrics() {
        let mut metrics = BTreeMap::new();
        metrics.insert("recall".to_string(), f64::NAN);
        let outcome = EvalCaseOutcome {
            case_id: EvalCaseId::parse("case-1").unwrap(),
            suite_id: "test".into(),
            mode: EvalMode::RetrievalOnly,
            split: CorpusSplit::Development,
            label_trust: LabelTrust::Official,
            status: CaseStatus::Passed,
            metrics,
            invalid_reason: None,
            failures: vec![],
            duration_ms: 0,
            attempts: 1,
        };
        assert!(outcome.validate().is_err());
    }
}
