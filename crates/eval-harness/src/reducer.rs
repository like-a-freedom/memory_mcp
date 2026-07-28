use std::collections::BTreeMap;

use crate::artifact::SuiteSummary;
use crate::domain::*;
use crate::error::EvalError;

/// Trait for computing mathematically correct suite summaries from case evidence.
pub trait SuiteReducer: Send + Sync {
    fn suite_id(&self) -> &SuiteId;
    fn reduce(&self, outcomes: &[EvalCaseOutcome]) -> Result<Vec<SuiteSummary>, EvalError>;
}

/// Reducer for retrieval suites. Computes aggregate recall@k, MRR, and top-1
/// from per-case `MetricEvidence::Retrieval` data.
pub struct RetrievalReducer {
    suite_id: SuiteId,
    #[allow(dead_code)]
    cutoff: u32,
}

impl RetrievalReducer {
    pub fn new(suite_id: impl Into<String>, cutoff: u32) -> Self {
        Self {
            suite_id: SuiteId::parse(suite_id).expect("suite_id must not be empty"),
            cutoff,
        }
    }
}

impl SuiteReducer for RetrievalReducer {
    fn suite_id(&self) -> &SuiteId {
        &self.suite_id
    }

    fn reduce(&self, outcomes: &[EvalCaseOutcome]) -> Result<Vec<SuiteSummary>, EvalError> {
        let mut total_relevant: u64 = 0;
        let mut total_hits: u64 = 0;
        let mut mrr_sum: f64 = 0.0;
        let mut top1_hits: u64 = 0;
        let mut valid_queries: u64 = 0;

        let mut passed = 0usize;
        let mut quality_failed = 0usize;
        let mut invalid = 0usize;

        for outcome in outcomes {
            match outcome.status {
                CaseStatus::Passed => passed += 1,
                CaseStatus::QualityFailed => quality_failed += 1,
                CaseStatus::Invalid => invalid += 1,
            }

            if let Some(MetricEvidence::Retrieval {
                relevant,
                hits_at_k,
                first_relevant_rank,
                ..
            }) = outcome.evidence.get("retrieval")
            {
                total_relevant += relevant;
                total_hits += hits_at_k;
                if *relevant > 0 {
                    valid_queries += 1;
                    if let Some(rank) = first_relevant_rank.filter(|r| *r > 0) {
                        mrr_sum += 1.0 / f64::from(rank);
                    }
                    if *hits_at_k > 0 && first_relevant_rank.is_some_and(|r| r == 1) {
                        top1_hits += 1;
                    }
                }
            }
        }

        let recall_at_k = if total_relevant > 0 {
            total_hits as f64 / total_relevant as f64
        } else {
            0.0
        };

        let mrr = if valid_queries > 0 {
            mrr_sum / valid_queries as f64
        } else {
            0.0
        };

        let top_1_hit_rate = if valid_queries > 0 {
            top1_hits as f64 / valid_queries as f64
        } else {
            0.0
        };

        let mut metrics = BTreeMap::new();
        metrics.insert("recall_at_5".to_string(), recall_at_k);
        metrics.insert("mrr".to_string(), mrr);
        metrics.insert("top_1_hit_rate".to_string(), top_1_hit_rate);

        Ok(vec![SuiteSummary {
            suite_id: self.suite_id.as_str().to_string(),
            mode: outcomes.first().map_or(EvalMode::RetrievalOnly, |o| o.mode),
            total: outcomes.len(),
            passed,
            quality_failed,
            invalid,
            metrics,
        }])
    }
}

/// Reducer for classification suites (extraction, claims). Sums confusion
/// counts across all cases before computing precision/recall/F1.
pub struct ClassificationReducer {
    suite_id: SuiteId,
    metric_prefix: String,
}

impl ClassificationReducer {
    pub fn new(suite_id: impl Into<String>, metric_prefix: impl Into<String>) -> Self {
        Self {
            suite_id: SuiteId::parse(suite_id).expect("suite_id must not be empty"),
            metric_prefix: metric_prefix.into(),
        }
    }
}

impl SuiteReducer for ClassificationReducer {
    fn suite_id(&self) -> &SuiteId {
        &self.suite_id
    }

    fn reduce(&self, outcomes: &[EvalCaseOutcome]) -> Result<Vec<SuiteSummary>, EvalError> {
        let mut total_tp: u64 = 0;
        let mut total_fp: u64 = 0;
        let mut total_fn: u64 = 0;
        let mut total_tn: u64 = 0;

        let mut passed = 0usize;
        let mut quality_failed = 0usize;
        let mut invalid = 0usize;

        for outcome in outcomes {
            match outcome.status {
                CaseStatus::Passed => passed += 1,
                CaseStatus::QualityFailed => quality_failed += 1,
                CaseStatus::Invalid => invalid += 1,
            }

            if let Some(MetricEvidence::Classification {
                true_positives,
                false_positives,
                false_negatives,
                true_negatives,
            }) = outcome.evidence.get("classification")
            {
                total_tp += true_positives;
                total_fp += false_positives;
                total_fn += false_negatives;
                total_tn += true_negatives;
            }
        }

        let mut metrics = BTreeMap::new();

        let precision = if total_tp + total_fp > 0 {
            total_tp as f64 / (total_tp + total_fp) as f64
        } else {
            return Err(EvalError::InvalidInput(
                "no classification predictions to evaluate".into(),
            ));
        };

        let recall = if total_tp + total_fn > 0 {
            total_tp as f64 / (total_tp + total_fn) as f64
        } else {
            return Err(EvalError::InvalidInput(
                "no classification ground truth to evaluate".into(),
            ));
        };

        let f1 = if precision + recall > 0.0 {
            2.0 * precision * recall / (precision + recall)
        } else {
            0.0
        };

        let prefix = &self.metric_prefix;
        metrics.insert(format!("{prefix}_precision"), precision);
        metrics.insert(format!("{prefix}_recall"), recall);
        metrics.insert(format!("{prefix}_f1"), f1);

        Ok(vec![SuiteSummary {
            suite_id: self.suite_id.as_str().to_string(),
            mode: outcomes.first().map_or(EvalMode::RetrievalOnly, |o| o.mode),
            total: outcomes.len(),
            passed,
            quality_failed,
            invalid,
            metrics,
        }])
    }
}

/// Reducer for suites that only need pass/fail/invalid counts (no custom metrics).
pub struct CountReducer {
    suite_id: SuiteId,
}

impl CountReducer {
    pub fn new(suite_id: impl Into<String>) -> Self {
        Self {
            suite_id: SuiteId::parse(suite_id).expect("suite_id must not be empty"),
        }
    }
}

impl SuiteReducer for CountReducer {
    fn suite_id(&self) -> &SuiteId {
        &self.suite_id
    }

    fn reduce(&self, outcomes: &[EvalCaseOutcome]) -> Result<Vec<SuiteSummary>, EvalError> {
        let mut passed = 0usize;
        let mut quality_failed = 0usize;
        let mut invalid = 0usize;

        for outcome in outcomes {
            match outcome.status {
                CaseStatus::Passed => passed += 1,
                CaseStatus::QualityFailed => quality_failed += 1,
                CaseStatus::Invalid => invalid += 1,
            }
        }

        Ok(vec![SuiteSummary {
            suite_id: self.suite_id.as_str().to_string(),
            mode: outcomes.first().map_or(EvalMode::RetrievalOnly, |o| o.mode),
            total: outcomes.len(),
            passed,
            quality_failed,
            invalid,
            metrics: std::collections::BTreeMap::new(),
        }])
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn retrieval_outcome(
        case_id: &str,
        relevant: u64,
        hits: u64,
        first_rank: Option<u32>,
        status: CaseStatus,
    ) -> EvalCaseOutcome {
        EvalCaseOutcome {
            case_key: CaseKey::parse("retrieval", case_id).unwrap(),
            mode: EvalMode::RetrievalOnly,
            split: CorpusSplit::Test,
            label_trust: LabelTrust::Official,
            status,
            metrics: BTreeMap::new(),
            evidence: [(
                "retrieval".to_string(),
                MetricEvidence::retrieval(relevant, hits, first_rank, 5),
            )]
            .into_iter()
            .collect(),
            invalid_reason: None,
            failures: vec![],
            duration_ms: 0,
            attempts: 1,
        }
    }

    fn classification_outcome(
        case_id: &str,
        tp: u64,
        fp: u64,
        fn_: u64,
        tn: u64,
    ) -> EvalCaseOutcome {
        EvalCaseOutcome {
            case_key: CaseKey::parse("extraction", case_id).unwrap(),
            mode: EvalMode::RetrievalOnly,
            split: CorpusSplit::Test,
            label_trust: LabelTrust::Official,
            status: CaseStatus::Passed,
            metrics: BTreeMap::new(),
            evidence: [(
                "classification".to_string(),
                MetricEvidence::classification(tp, fp, fn_, tn),
            )]
            .into_iter()
            .collect(),
            invalid_reason: None,
            failures: vec![],
            duration_ms: 0,
            attempts: 1,
        }
    }

    #[test]
    fn retrieval_summary_uses_all_cases() {
        let outcomes = vec![
            retrieval_outcome("a", 1, 1, Some(1), CaseStatus::Passed),
            retrieval_outcome("b", 1, 0, None, CaseStatus::QualityFailed),
        ];
        let summary = RetrievalReducer::new("local-retrieval", 5)
            .reduce(&outcomes)
            .unwrap()
            .remove(0);
        assert!((summary.metrics["recall_at_5"] - 0.5).abs() < 1e-12);
        assert!((summary.metrics["mrr"] - 0.5).abs() < 1e-12);
        assert!((summary.metrics["top_1_hit_rate"] - 0.5).abs() < 1e-12);
    }

    #[test]
    fn classification_summary_sums_confusion_counts_before_f1() {
        let outcomes = vec![
            classification_outcome("a", 1, 0, 0, 2),
            classification_outcome("b", 0, 1, 1, 0),
        ];
        let summary = ClassificationReducer::new("extraction", "entity")
            .reduce(&outcomes)
            .unwrap()
            .remove(0);
        assert!((summary.metrics["entity_precision"] - 0.5).abs() < 1e-12);
        assert!((summary.metrics["entity_recall"] - 0.5).abs() < 1e-12);
        assert!((summary.metrics["entity_f1"] - 0.5).abs() < 1e-12);
    }

    #[test]
    fn retrieval_empty_evidence_returns_zeroes() {
        let outcomes = vec![retrieval_outcome("a", 0, 0, None, CaseStatus::Passed)];
        let summary = RetrievalReducer::new("test", 5)
            .reduce(&outcomes)
            .unwrap()
            .remove(0);
        assert_eq!(summary.metrics["recall_at_5"], 0.0);
        assert_eq!(summary.metrics["mrr"], 0.0);
    }
}
