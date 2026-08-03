use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;

use crate::domain::MetricEvidence;
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

/// Canonical metric names for the values produced by [`render_case_metrics`].
///
/// `None` fields produce no entry for that arm. Suites that need a
/// diagnostic-only alias (none at the time of writing) may pass an override;
/// gate-consumed names are the defaults and suites should not rename them.
#[derive(Debug, Clone, Default)]
pub struct CaseMetricNames {
    /// Override for `recall_at_<cutoff>` (retrieval evidence).
    pub recall_at_k: Option<String>,
    /// Override for `mrr` (retrieval evidence).
    pub mrr: Option<String>,
    /// Override for `top_1_hit_rate` (retrieval evidence).
    pub top_1_hit_rate: Option<String>,
    /// Prefix for `<prefix>_precision` / `<prefix>_recall` / `<prefix>_f1`
    /// (classification evidence). Defaults to `"metric"` and is always
    /// overridden in practice; see suite call sites.
    pub classification_prefix: Option<String>,
    /// Explicit name for ratio and count evidence.
    pub name: Option<String>,
    /// Restricts which keys the renderer emits. `None` (default) emits all
    /// keys for the arm. Suites use this to keep a subset of a classification
    /// triplet (e.g. claims keeps only precision/recall per case; F1 stays
    /// an aggregate-only reducer output there).
    pub only_keys: Option<&'static [&'static str]>,
}

impl CaseMetricNames {
    /// Names for a classification arm: prefix-only constructor.
    pub fn classification(prefix: impl Into<String>) -> Self {
        Self {
            classification_prefix: Some(prefix.into()),
            ..Self::default()
        }
    }

    /// Names for a ratio or count arm: single-name constructor.
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            ..Self::default()
        }
    }
}

/// Render per-case diagnostic metrics from typed evidence.
///
/// This is the single home for case-level metric naming and arithmetic when a
/// suite needs to expose a number in `EvalCaseOutcome.metrics`. It reuses the
/// exact formulas used by the aggregate path (`retrieval_metrics`,
/// `classification_metrics`, ratio = n/d) so case diagnostics and reducer
/// aggregates cannot drift apart.
///
/// Suites must call this rather than hand-building `BTreeMap<String, f64>`
/// with hardcoded metric-key strings.
///
/// Evidence arm behavior:
/// - `Retrieval` → `recall_at_<cutoff>`, `mrr`, `top_1_hit_rate`, using the
///   same per-case rules as [`retrieval_metrics`] (recall = hits/relevant when
///   relevant > 0 else 0; mrr = 1/rank when `Some(rank)` and rank > 0 else 0;
///   top-1 = 1 iff `first_relevant_rank == Some(1)`).
/// - `Classification` → `<prefix>_precision`, `<prefix>_recall`,
///   `<prefix>_f1` via [`classification_metrics`]. Zero-population arms
///   (precision with no predicted positives, recall with no actual positives)
///   render as 1.0 — the "vacuous success" convention the suites already use
///   for per-case diagnostics. The reducer retains its own error-on-empty rule
///   for aggregates; this render choice is case-display only.
/// - `Ratio` → `name = numerator / denominator`; renders 0.0 when the
///   denominator is 0.
/// - `Count` → `name = value as f64`.
/// - `Duration` → `name = nanoseconds as f64 / 1_000_000.0` (rendered as ms).
///
/// Unknown/unnamed arms return an empty map rather than guessing a key.
pub fn render_case_metrics(
    evidence: &MetricEvidence,
    names: &CaseMetricNames,
) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    match evidence {
        MetricEvidence::Retrieval {
            relevant,
            hits_at_k,
            first_relevant_rank,
            cutoff,
        } => {
            let recall = if *relevant > 0 {
                *hits_at_k as f64 / *relevant as f64
            } else {
                0.0
            };
            let rank = first_relevant_rank.filter(|r| *r > 0);
            let mrr = rank.map_or(0.0, |r| 1.0 / f64::from(r));
            let top_1 = if *first_relevant_rank == Some(1) {
                1.0
            } else {
                0.0
            };
            let recall_key = names
                .recall_at_k
                .clone()
                .unwrap_or_else(|| format!("recall_at_{cutoff}"));
            out.insert(recall_key, recall);
            out.insert(names.mrr.clone().unwrap_or_else(|| "mrr".into()), mrr);
            out.insert(
                names
                    .top_1_hit_rate
                    .clone()
                    .unwrap_or_else(|| "top_1_hit_rate".into()),
                top_1,
            );
        }
        MetricEvidence::Classification {
            true_positives,
            false_positives,
            false_negatives,
            true_negatives,
        } => {
            let prefix = names
                .classification_prefix
                .clone()
                .unwrap_or_else(|| "metric".into());
            let counts = ClassificationCounts {
                true_positives: *true_positives as usize,
                false_positives: *false_positives as usize,
                false_negatives: *false_negatives as usize,
                true_negatives: *true_negatives as usize,
            };
            match classification_metrics(counts) {
                Ok(m) => {
                    // Per-case vacuity convention shared by the extraction
                    // and claims suites (pre-ADR-0025 behavior, preserved
                    // verbatim so case diagnostics do not drift):
                    // - precision undefined (nothing predicted): 1.0 when
                    //   nothing was expected either (vacuous success), 0.0
                    //   when positives were missed entirely (fn > 0).
                    // - recall undefined (no actual positives): 1.0.
                    // - f1 recomputed from the resolved precision/recall.
                    let precision =
                        m.precision
                            .unwrap_or(if *false_negatives > 0 { 0.0 } else { 1.0 });
                    let recall = m.recall.unwrap_or(1.0);
                    let f1 = if precision + recall > 0.0 {
                        2.0 * precision * recall / (precision + recall)
                    } else {
                        0.0
                    };
                    out.insert(format!("{prefix}_precision"), precision);
                    out.insert(format!("{prefix}_recall"), recall);
                    out.insert(format!("{prefix}_f1"), f1);
                }
                Err(_) => {
                    // classification_metrics rejects the all-zero confusion
                    // matrix, but per-case convention renders it as a
                    // vacuous success (1.0 / 1.0 / 1.0) — both classification
                    // suites did exactly this before ADR-0025.
                    out.insert(format!("{prefix}_precision"), 1.0);
                    out.insert(format!("{prefix}_recall"), 1.0);
                    out.insert(format!("{prefix}_f1"), 1.0);
                }
            }
        }
        MetricEvidence::Ratio {
            numerator,
            denominator,
        } => {
            if let Some(name) = &names.name {
                let value = if *denominator > 0 {
                    *numerator as f64 / *denominator as f64
                } else {
                    0.0
                };
                out.insert(name.clone(), value);
            }
        }
        MetricEvidence::Count { value } => {
            if let Some(name) = &names.name {
                out.insert(name.clone(), *value as f64);
            }
        }
        MetricEvidence::Duration { nanoseconds } => {
            if let Some(name) = &names.name {
                out.insert(name.clone(), *nanoseconds as f64 / 1_000_000.0);
            }
        }
    }
    out
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

    #[test]
    fn render_case_metrics_retrieval_matches_retrieval_metrics() {
        use crate::domain::MetricEvidence;

        // Each sample: (relevant_ids, ranked_ids, cutoff, expected evidence).
        // The evidence is what the suite would record for that observation.
        let samples: Vec<(&[&str], &[&str], u32)> = vec![
            (&["a", "b"], &["a", "x", "y", "z", "w"], 5),
            (&["a", "b"], &["x", "y", "a", "b", "z"], 5),
            (&["a", "b", "c"], &["x", "y", "z", "w", "v"], 5),
            (&["a"], &["x", "y", "z", "w", "a"], 5),
            (
                &["a", "b", "c", "d"],
                &["a", "x", "b", "y", "c", "z", "z2"],
                10,
            ),
            (&["a"], &["a"], 5),
        ];

        for (relevant, ranked, cutoff) in samples {
            let relevant_ids: std::collections::BTreeSet<String> =
                relevant.iter().map(|s| s.to_string()).collect();
            let ranked_ids: Vec<String> = ranked.iter().map(|s| s.to_string()).collect();

            let hits_at_k = ranked_ids
                .iter()
                .take(cutoff as usize)
                .filter(|id| relevant_ids.contains(*id))
                .count() as u64;
            let first_relevant_rank = ranked_ids
                .iter()
                .position(|id| relevant_ids.contains(id))
                .map(|r| (r + 1) as u32);

            let evidence = MetricEvidence::retrieval(
                relevant_ids.len() as u64,
                hits_at_k,
                first_relevant_rank,
                cutoff,
            );
            let rendered = render_case_metrics(&evidence, &CaseMetricNames::default());

            let obs = RetrievalObservation {
                relevant_ids,
                ranked_ids,
            };
            let aggregate =
                retrieval_metrics(&[obs], NonZeroUsize::new(cutoff as usize).unwrap()).unwrap();

            let recall_key = format!("recall_at_{cutoff}");
            assert!(
                (rendered[&recall_key] - aggregate.recall_at_k).abs() < 1e-12,
                "recall drift for {relevant:?} / {ranked:?} / cutoff={cutoff}"
            );
            assert!(
                (rendered["mrr"] - aggregate.mrr).abs() < 1e-12,
                "mrr drift for {relevant:?} / {ranked:?} / cutoff={cutoff}"
            );
            assert!(
                (rendered["top_1_hit_rate"] - aggregate.top_1_hit_rate).abs() < 1e-12,
                "top_1 drift for {relevant:?} / {ranked:?} / cutoff={cutoff}"
            );
        }
    }

    #[test]
    fn render_case_metrics_classification_matches_classification_metrics() {
        use crate::domain::MetricEvidence;

        let cases = [
            (10_u64, 0_u64, 0_u64, 5_u64),
            (3, 1, 2, 4),
            (0, 4, 3, 5),
            (4, 0, 0, 0),
            (0, 0, 5, 5),
        ];
        for (tp, fp, fn_, tn) in cases {
            let evidence = MetricEvidence::classification(tp, fp, fn_, tn);
            let rendered =
                render_case_metrics(&evidence, &CaseMetricNames::classification("entity"));
            let aggregate = classification_metrics(ClassificationCounts {
                true_positives: tp as usize,
                false_positives: fp as usize,
                false_negatives: fn_ as usize,
                true_negatives: tn as usize,
            })
            .unwrap();

            // Vacuity convention (matches the pre-ADR-0025 suite formulas):
            // precision undefined ⇒ 0.0 when fn > 0 (missed all positives),
            // 1.0 otherwise; recall undefined ⇒ 1.0; f1 recomputed from the
            // resolved precision/recall.
            let expected_p = aggregate
                .precision
                .unwrap_or(if fn_ > 0 { 0.0 } else { 1.0 });
            let expected_r = aggregate.recall.unwrap_or(1.0);
            let expected_f1 = if expected_p + expected_r > 0.0 {
                2.0 * expected_p * expected_r / (expected_p + expected_r)
            } else {
                0.0
            };

            assert!(
                (rendered["entity_precision"] - expected_p).abs() < 1e-12,
                "precision drift for tp={tp} fp={fp} fn={fn_} tn={tn}"
            );
            assert!(
                (rendered["entity_recall"] - expected_r).abs() < 1e-12,
                "recall drift for tp={tp} fp={fp} fn={fn_} tn={tn}"
            );
            assert!(
                (rendered["entity_f1"] - expected_f1).abs() < 1e-12,
                "f1 drift for tp={tp} fp={fp} fn={fn_} tn={tn}"
            );
        }
    }

    #[test]
    fn render_case_metrics_all_zero_classification_renders_vacuous_success() {
        // Pre-ADR-0025, both classification suites rendered an all-zero
        // confusion matrix as 1.0 / 1.0 / 1.0. The renderer preserves that
        // convention despite classification_metrics rejecting it.
        use crate::domain::MetricEvidence;
        let evidence = MetricEvidence::classification(0, 0, 0, 0);
        let rendered = render_case_metrics(&evidence, &CaseMetricNames::classification("x"));
        assert_eq!(rendered["x_precision"], 1.0);
        assert_eq!(rendered["x_recall"], 1.0);
        assert_eq!(rendered["x_f1"], 1.0);
    }

    #[test]
    fn render_case_metrics_missed_all_positives_renders_zero_precision() {
        // tp = fp = 0 with fn > 0: both suites penalize this as precision
        // 0.0 (not vacuous success) — the extraction suite did exactly this.
        use crate::domain::MetricEvidence;
        let evidence = MetricEvidence::classification(0, 0, 5, 5);
        let rendered = render_case_metrics(&evidence, &CaseMetricNames::classification("x"));
        assert_eq!(rendered["x_precision"], 0.0);
        assert_eq!(rendered["x_recall"], 0.0);
        assert_eq!(rendered["x_f1"], 0.0);
    }

    #[test]
    fn render_case_metrics_ratio_and_count_use_explicit_name() {
        use crate::domain::MetricEvidence;
        let ratio = MetricEvidence::ratio(3, 4);
        let rendered = render_case_metrics(&ratio, &CaseMetricNames::named("pass_rate"));
        assert!((rendered["pass_rate"] - 0.75).abs() < 1e-12);

        let ratio_zero = MetricEvidence::ratio(0, 0);
        let rendered_zero = render_case_metrics(&ratio_zero, &CaseMetricNames::named("pass_rate"));
        assert_eq!(rendered_zero["pass_rate"], 0.0);

        let count = MetricEvidence::count(7);
        let rendered_count = render_case_metrics(&count, &CaseMetricNames::named("items"));
        assert_eq!(rendered_count["items"], 7.0);
    }

    #[test]
    fn render_case_metrics_retrieval_uses_cutoff_in_key() {
        use crate::domain::MetricEvidence;
        let evidence = MetricEvidence::retrieval(4, 2, Some(1), 10);
        let rendered = render_case_metrics(&evidence, &CaseMetricNames::default());
        assert!(rendered.contains_key("recall_at_10"));
        assert!(!rendered.contains_key("recall_at_5"));
    }
}
