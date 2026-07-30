use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::EvalError;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SuiteId(String);

impl SuiteId {
    pub fn parse(raw: impl Into<String>) -> Result<Self, EvalError> {
        let value = raw.into();
        if value.trim().is_empty() {
            return Err(EvalError::InvalidConfig(
                "suite id must not be empty".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CaseKey {
    pub suite_id: SuiteId,
    pub case_id: EvalCaseId,
}

impl CaseKey {
    pub fn parse(
        suite_id: impl Into<String>,
        case_id: impl Into<String>,
    ) -> Result<Self, EvalError> {
        Ok(Self {
            suite_id: SuiteId::parse(suite_id)?,
            case_id: EvalCaseId::parse(case_id)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MetricEvidence {
    Retrieval {
        relevant: u64,
        hits_at_k: u64,
        first_relevant_rank: Option<u32>,
        cutoff: u32,
    },
    Classification {
        true_positives: u64,
        false_positives: u64,
        false_negatives: u64,
        true_negatives: u64,
    },
    Count {
        value: u64,
    },
    Ratio {
        numerator: u64,
        denominator: u64,
    },
    Duration {
        nanoseconds: u64,
    },
}

impl MetricEvidence {
    pub fn retrieval(
        relevant: u64,
        hits_at_k: u64,
        first_relevant_rank: Option<u32>,
        cutoff: u32,
    ) -> Self {
        Self::Retrieval {
            relevant,
            hits_at_k,
            first_relevant_rank,
            cutoff,
        }
    }

    pub fn classification(tp: u64, fp: u64, fn_: u64, tn: u64) -> Self {
        Self::Classification {
            true_positives: tp,
            false_positives: fp,
            false_negatives: fn_,
            true_negatives: tn,
        }
    }

    pub fn count(value: u64) -> Self {
        Self::Count { value }
    }

    pub fn ratio(numerator: u64, denominator: u64) -> Self {
        Self::Ratio {
            numerator,
            denominator,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseStatus {
    Passed,
    QualityFailed,
    Invalid,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunVerdict {
    #[default]
    Passed,
    QualityFailed,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStage {
    SuiteLoad,
    SuiteRun,
    Coverage,
    Gate,
    Budget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunIssue {
    pub stage: RunStage,
    pub suite_id: Option<SuiteId>,
    pub message: String,
}

impl RunIssue {
    pub fn empty_suite(suite_id: &str) -> Self {
        Self {
            stage: RunStage::SuiteLoad,
            suite_id: Some(SuiteId::parse(suite_id).expect("suite_id must not be empty")),
            message: format!("suite '{suite_id}' failed to load"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateDirection {
    AtLeast,
    AtMost,
}

pub fn derive_run_verdict(
    outcomes: &[EvalCaseOutcome],
    gate_decisions: &[crate::artifact::GateDecision],
    budget_status: crate::artifact::GateStatus,
    issues: &[RunIssue],
) -> RunVerdict {
    if !issues.is_empty() {
        return RunVerdict::Invalid;
    }
    if budget_status == crate::artifact::GateStatus::Invalid {
        return RunVerdict::Invalid;
    }
    if outcomes.iter().any(|o| o.status == CaseStatus::Invalid) {
        return RunVerdict::Invalid;
    }
    if gate_decisions
        .iter()
        .any(|g| g.status == crate::artifact::GateStatus::Invalid)
    {
        return RunVerdict::Invalid;
    }

    if budget_status == crate::artifact::GateStatus::Failed {
        return RunVerdict::QualityFailed;
    }
    if outcomes
        .iter()
        .any(|o| o.status == CaseStatus::QualityFailed)
    {
        return RunVerdict::QualityFailed;
    }
    if gate_decisions
        .iter()
        .any(|g| g.status == crate::artifact::GateStatus::Failed)
    {
        return RunVerdict::QualityFailed;
    }

    RunVerdict::Passed
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvalProfile {
    Pr,
    Release,
    Nightly,
    ResponseSize,
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
    pub case_key: CaseKey,
    pub mode: EvalMode,
    pub split: CorpusSplit,
    pub label_trust: LabelTrust,
    pub status: CaseStatus,
    #[serde(default)]
    pub metrics: BTreeMap<String, f64>,
    #[serde(default)]
    pub evidence: BTreeMap<String, MetricEvidence>,
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
    /// Construct an outcome with explicit case key.
    pub fn new(
        suite_id: impl Into<String>,
        case_id: impl Into<String>,
        mode: EvalMode,
        split: CorpusSplit,
        label_trust: LabelTrust,
        status: CaseStatus,
    ) -> Self {
        Self {
            case_key: CaseKey::parse(suite_id, case_id).expect("valid case key"),
            mode,
            split,
            label_trust,
            status,
            metrics: BTreeMap::new(),
            evidence: BTreeMap::new(),
            invalid_reason: None,
            failures: vec![],
            duration_ms: 0,
            attempts: 1,
        }
    }

    /// Backward-compatible accessor for suite_id.
    pub fn suite_id(&self) -> &str {
        self.case_key.suite_id.as_str()
    }

    /// Backward-compatible accessor for case_id.
    pub fn case_id(&self) -> &EvalCaseId {
        &self.case_key.case_id
    }

    pub fn validate(&self) -> Result<(), EvalError> {
        if self.status == CaseStatus::Invalid && self.invalid_reason.is_none() {
            return Err(EvalError::InvalidInput(format!(
                "case {} has Invalid status but no invalid_reason",
                self.case_key.case_id.as_str()
            )));
        }
        if self.status != CaseStatus::Invalid && self.invalid_reason.is_some() {
            return Err(EvalError::InvalidInput(format!(
                "case {} has non-Invalid status but carries an invalid_reason",
                self.case_key.case_id.as_str()
            )));
        }
        for (key, value) in &self.metrics {
            if !value.is_finite() {
                return Err(EvalError::InvalidInput(format!(
                    "case {} has non-finite metric {} = {value}",
                    self.case_key.case_id.as_str(),
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
            case_key: CaseKey::parse("test", "case-1").unwrap(),
            mode: EvalMode::RetrievalOnly,
            split: CorpusSplit::Development,
            label_trust: LabelTrust::Official,
            status: CaseStatus::Invalid,
            metrics: BTreeMap::new(),
            evidence: BTreeMap::new(),
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
            case_key: CaseKey::parse("test", "case-1").unwrap(),
            mode: EvalMode::RetrievalOnly,
            split: CorpusSplit::Development,
            label_trust: LabelTrust::Official,
            status: CaseStatus::Passed,
            metrics: BTreeMap::new(),
            evidence: BTreeMap::new(),
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
            case_key: CaseKey::parse("test", "case-1").unwrap(),
            mode: EvalMode::RetrievalOnly,
            split: CorpusSplit::Development,
            label_trust: LabelTrust::Official,
            status: CaseStatus::Passed,
            metrics,
            evidence: BTreeMap::new(),
            invalid_reason: None,
            failures: vec![],
            duration_ms: 0,
            attempts: 1,
        };
        assert!(outcome.validate().is_err());
    }

    #[test]
    fn same_local_id_in_two_suites_is_not_a_duplicate() {
        let first = CaseKey::parse("retrieval", "case-1").unwrap();
        let second = CaseKey::parse("claims", "case-1").unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn empty_suite_or_case_id_is_rejected() {
        assert!(CaseKey::parse("", "case-1").is_err());
        assert!(CaseKey::parse("retrieval", "").is_err());
    }

    #[test]
    fn suite_id_rejects_empty_string() {
        assert!(SuiteId::parse("").is_err());
        assert!(SuiteId::parse("  ").is_err());
    }

    #[test]
    fn suite_id_accepts_valid_string() {
        let id = SuiteId::parse("local-retrieval").unwrap();
        assert_eq!(id.as_str(), "local-retrieval");
    }

    #[test]
    fn case_key_same_in_both_suites_is_duplicate() {
        let first = CaseKey::parse("retrieval", "case-1").unwrap();
        let second = CaseKey::parse("retrieval", "case-1").unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn metric_evidence_retrieval_serializes() {
        let evidence = MetricEvidence::retrieval(5, 3, Some(2), 5);
        let json = serde_json::to_string(&evidence).unwrap();
        assert!(json.contains("\"kind\":\"retrieval\""));
        assert!(json.contains("\"relevant\":5"));
        assert!(json.contains("\"hits_at_k\":3"));
    }

    #[test]
    fn metric_evidence_classification_serializes() {
        let evidence = MetricEvidence::classification(10, 2, 1, 87);
        let json = serde_json::to_string(&evidence).unwrap();
        assert!(json.contains("\"kind\":\"classification\""));
        assert!(json.contains("\"true_positives\":10"));
    }
}
