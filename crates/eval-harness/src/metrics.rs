use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use crate::error::EvalError;

#[derive(Debug, Clone)]
pub struct RetrievalObservation {
    pub relevant_ids: BTreeSet<String>,
    pub ranked_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalMetrics {
    pub recall_at_k: f64,
    pub mrr: f64,
    pub top_1_hit_rate: f64,
    pub total_cases: usize,
    pub cases_with_relevant: usize,
}

#[derive(Debug, Clone, Default)]
pub struct ClassificationCounts {
    pub true_positives: usize,
    pub false_positives: usize,
    pub false_negatives: usize,
    pub true_negatives: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClassificationMetrics {
    pub precision: Option<f64>,
    pub recall: Option<f64>,
    pub f1: Option<f64>,
}

pub fn retrieval_metrics(
    cases: &[RetrievalObservation],
    cutoff: NonZeroUsize,
) -> Result<RetrievalMetrics, EvalError> {
    if cases.is_empty() {
        return Err(EvalError::InvalidInput(
            "retrieval metrics require at least one case".into(),
        ));
    }

    let k = cutoff.get();
    let mut recall_sum = 0.0;
    let mut rr_sum = 0.0;
    let mut top_1_hits = 0;
    let mut cases_with_relevant = 0;

    for case in cases {
        if case.relevant_ids.is_empty() {
            return Err(EvalError::InvalidInput("case has no relevant IDs".into()));
        }

        let relevant_count = case.relevant_ids.len();
        cases_with_relevant += 1;

        let ranked_at_cutoff: BTreeSet<&str> =
            case.ranked_ids.iter().take(k).map(String::as_str).collect();

        let hits_in_cutoff = case
            .relevant_ids
            .iter()
            .filter(|id| ranked_at_cutoff.contains(id.as_str()))
            .count();

        let case_recall = hits_in_cutoff as f64 / relevant_count as f64;
        recall_sum += case_recall;

        if let Some(first_rank) = case
            .ranked_ids
            .iter()
            .position(|id| case.relevant_ids.contains(id))
        {
            rr_sum += 1.0 / (first_rank + 1) as f64;
        }

        if case
            .ranked_ids
            .first()
            .is_some_and(|id| case.relevant_ids.contains(id))
        {
            top_1_hits += 1;
        }
    }

    let total = cases.len();
    Ok(RetrievalMetrics {
        recall_at_k: recall_sum / total as f64,
        mrr: rr_sum / total as f64,
        top_1_hit_rate: top_1_hits as f64 / total as f64,
        total_cases: total,
        cases_with_relevant,
    })
}

pub fn classification_metrics(
    counts: ClassificationCounts,
) -> Result<ClassificationMetrics, EvalError> {
    let total = counts.true_positives
        + counts.false_positives
        + counts.false_negatives
        + counts.true_negatives;
    if total == 0 {
        return Err(EvalError::InvalidInput(
            "classification metrics require at least one observation".into(),
        ));
    }

    let predicted_positive = counts.true_positives + counts.false_positives;
    let actual_positive = counts.true_positives + counts.false_negatives;

    let precision = if predicted_positive == 0 {
        None
    } else {
        Some(counts.true_positives as f64 / predicted_positive as f64)
    };

    let recall = if actual_positive == 0 {
        None
    } else {
        Some(counts.true_positives as f64 / actual_positive as f64)
    };

    let f1 = match (precision, recall) {
        (Some(p), Some(r)) if p + r > 0.0 => Some(2.0 * p * r / (p + r)),
        _ => None,
    };

    Ok(ClassificationMetrics {
        precision,
        recall,
        f1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(relevant: &[&str], ranked: &[&str]) -> RetrievalObservation {
        RetrievalObservation {
            relevant_ids: relevant.iter().map(|s| s.to_string()).collect(),
            ranked_ids: ranked.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn recall_at_five_ignores_a_hit_at_rank_six() {
        let obs = observation(&["expected"], &["a", "b", "c", "d", "e", "expected"]);
        let metrics = retrieval_metrics(&[obs], NonZeroUsize::new(5).unwrap()).unwrap();
        assert_eq!(metrics.recall_at_k, 0.0);
    }

    #[test]
    fn recall_at_five_counts_hit_at_rank_one() {
        let obs = observation(&["expected"], &["expected", "a", "b", "c", "d"]);
        let metrics = retrieval_metrics(&[obs], NonZeroUsize::new(5).unwrap()).unwrap();
        assert!((metrics.recall_at_k - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn mrr_uses_first_relevant_rank_in_full_ranking() {
        let obs = observation(&["expected"], &["noise", "noise", "expected"]);
        let metrics = retrieval_metrics(&[obs], NonZeroUsize::new(5).unwrap()).unwrap();
        assert!((metrics.mrr - 1.0 / 3.0).abs() < 1e-10);
    }

    #[test]
    fn top_1_hit_rate_counts_first_position_only() {
        let obs1 = observation(&["a"], &["a", "b"]);
        let obs2 = observation(&["b"], &["x", "b"]);
        let metrics = retrieval_metrics(&[obs1, obs2], NonZeroUsize::new(5).unwrap()).unwrap();
        assert!((metrics.top_1_hit_rate - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn empty_metric_input_is_invalid() {
        assert!(retrieval_metrics(&[], NonZeroUsize::new(5).unwrap()).is_err());
    }

    #[test]
    fn case_with_no_relevant_ids_is_invalid() {
        let obs = observation(&[], &["a", "b"]);
        assert!(retrieval_metrics(&[obs], NonZeroUsize::new(5).unwrap()).is_err());
    }

    #[test]
    fn partial_recall_at_k() {
        let obs = observation(&["a", "b"], &["a", "x", "x", "x", "x"]);
        let metrics = retrieval_metrics(&[obs], NonZeroUsize::new(5).unwrap()).unwrap();
        assert!((metrics.recall_at_k - 0.5).abs() < 1e-10);
    }

    #[test]
    fn classification_precision_requires_predicted_positives() {
        let counts = ClassificationCounts {
            true_positives: 0,
            false_positives: 0,
            false_negatives: 5,
            true_negatives: 5,
        };
        let metrics = classification_metrics(counts).unwrap();
        assert_eq!(metrics.precision, None);
    }

    #[test]
    fn classification_recall_requires_actual_positives() {
        let counts = ClassificationCounts {
            true_positives: 0,
            false_positives: 5,
            false_negatives: 0,
            true_negatives: 5,
        };
        let metrics = classification_metrics(counts).unwrap();
        assert_eq!(metrics.recall, None);
    }

    #[test]
    fn classification_zero_population_is_invalid() {
        let counts = ClassificationCounts::default();
        assert!(classification_metrics(counts).is_err());
    }

    #[test]
    fn classification_perfect() {
        let counts = ClassificationCounts {
            true_positives: 10,
            false_positives: 0,
            false_negatives: 0,
            true_negatives: 10,
        };
        let metrics = classification_metrics(counts).unwrap();
        assert_eq!(metrics.precision, Some(1.0));
        assert_eq!(metrics.recall, Some(1.0));
        assert_eq!(metrics.f1, Some(1.0));
    }

    #[test]
    fn classification_with_all_three_counts() {
        let counts = ClassificationCounts {
            true_positives: 3,
            false_positives: 1,
            false_negatives: 2,
            true_negatives: 4,
        };
        let metrics = classification_metrics(counts).unwrap();
        let p = 3.0 / 4.0;
        let r = 3.0 / 5.0;
        let expected_f1 = 2.0 * p * r / (p + r);
        assert!((metrics.precision.unwrap() - p).abs() < 1e-10);
        assert!((metrics.recall.unwrap() - r).abs() < 1e-10);
        assert!((metrics.f1.unwrap() - expected_f1).abs() < 1e-10);
    }
}
