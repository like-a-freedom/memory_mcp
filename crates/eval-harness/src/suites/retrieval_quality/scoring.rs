use super::cases::{QualityExpectation, QualityObservation};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct QualityScores {
    pub(crate) precision: Option<f64>,
    pub(crate) recall: Option<f64>,
    pub(crate) abstention: Option<bool>,
    pub(crate) false_abstention: Option<bool>,
    pub(crate) duplicate_fraction: f64,
    pub(crate) prohibited_free: bool,
    pub(crate) budget_ok: bool,
    pub(crate) order_ok: Option<bool>,
    pub(crate) allowed_items: usize,
    pub(crate) unique_items: usize,
    pub(crate) covered_groups: usize,
    pub(crate) required_groups: usize,
    pub(crate) prohibited_items: usize,
    pub(crate) duplicate_items: usize,
}

pub(crate) fn score_quality(
    expected: &QualityExpectation,
    actual: &QualityObservation,
) -> Result<QualityScores, String> {
    expected.validate()?;
    if actual.budget == 0 {
        return Err("quality observation budget must be greater than zero".into());
    }

    let unique: BTreeSet<_> = actual.ranked_keys.iter().collect();
    let unique_allowed = unique
        .iter()
        .filter(|key| expected.allowed_keys.contains(key.as_str()))
        .count();
    let covered_groups = expected
        .required_evidence_groups
        .iter()
        .filter(|group| group.iter().any(|key| unique.contains(key)))
        .count();
    let prohibited_items = unique
        .iter()
        .filter(|key| expected.prohibited_keys.contains(key.as_str()))
        .count();
    let prohibited_free = unique
        .iter()
        .all(|key| !expected.prohibited_keys.contains(key.as_str()));
    let duplicate_fraction = if actual.ranked_keys.is_empty() {
        0.0
    } else {
        (actual.ranked_keys.len() - unique.len()) as f64 / actual.ranked_keys.len() as f64
    };

    Ok(QualityScores {
        precision: (!unique.is_empty()).then(|| unique_allowed as f64 / unique.len() as f64),
        recall: (!expected.required_evidence_groups.is_empty())
            .then(|| covered_groups as f64 / expected.required_evidence_groups.len() as f64),
        abstention: Some(actual.ranked_keys.is_empty()),
        false_abstention: (!expected.expect_empty).then_some(actual.ranked_keys.is_empty()),
        duplicate_fraction,
        prohibited_free,
        budget_ok: actual.ranked_keys.len() <= actual.budget,
        order_ok: expected
            .expected_order
            .as_ref()
            .map(|expected_order| expected_order == &actual.ranked_keys),
        allowed_items: unique_allowed,
        unique_items: unique.len(),
        covered_groups,
        required_groups: expected.required_evidence_groups.len(),
        prohibited_items,
        duplicate_items: actual.ranked_keys.len() - unique.len(),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn set(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn expectation(
        required: &[&[&str]],
        allowed: &[&str],
        prohibited: &[&str],
        expect_empty: bool,
        expected_order: Option<&[&str]>,
    ) -> QualityExpectation {
        QualityExpectation {
            required_evidence_groups: required.iter().map(|group| set(group)).collect(),
            allowed_keys: set(allowed),
            prohibited_keys: set(prohibited),
            expect_empty,
            expected_order: expected_order
                .map(|keys| keys.iter().map(|key| (*key).to_owned()).collect()),
        }
    }

    fn observation(keys: &[&str], budget: usize) -> QualityObservation {
        QualityObservation {
            ranked_keys: keys.iter().map(|key| (*key).to_owned()).collect(),
            budget,
        }
    }

    #[test]
    fn scoring_uses_unique_ids_and_counts_covered_required_groups() {
        let expected = expectation(&[&["a"], &["b"]], &["a", "b"], &[], false, None);
        let actual = observation(&["a", "a"], 5);

        let scores = score_quality(&expected, &actual).expect("valid score");

        assert_eq!(scores.precision, Some(1.0));
        assert_eq!(scores.recall, Some(0.5));
        assert_eq!(scores.duplicate_fraction, 0.5);
        assert!(scores.budget_ok);
    }

    #[test]
    fn answerable_empty_is_false_abstention_and_unknown_empty_is_valid_abstention() {
        let answerable = expectation(&[&["a"]], &["a"], &[], false, None);
        let unknown = expectation(&[], &[], &[], true, None);

        let answerable_scores = score_quality(&answerable, &observation(&[], 5)).expect("score");
        let unknown_scores = score_quality(&unknown, &observation(&[], 5)).expect("score");

        assert_eq!(answerable_scores.precision, None);
        assert_eq!(answerable_scores.recall, Some(0.0));
        assert_eq!(answerable_scores.abstention, Some(true));
        assert_eq!(answerable_scores.false_abstention, Some(true));
        assert_eq!(unknown_scores.recall, None);
        assert_eq!(unknown_scores.abstention, Some(true));
        assert_eq!(unknown_scores.false_abstention, None);
    }

    #[test]
    fn prohibited_unknown_and_over_budget_ids_are_not_hidden_by_the_oracle() {
        let expected = expectation(&[&["a"]], &["a"], &["secret"], false, None);
        let actual = observation(&["a", "secret", "unrecognized"], 2);

        let scores = score_quality(&expected, &actual).expect("valid score");

        assert_eq!(scores.precision, Some(1.0 / 3.0));
        assert!(!scores.prohibited_free);
        assert!(!scores.budget_ok);
    }

    #[test]
    fn expected_order_is_checked_only_when_the_case_declares_it() {
        let expected = expectation(&[&["a", "b"]], &["a", "b"], &[], false, Some(&["a", "b"]));
        let actual = observation(&["b", "a"], 2);
        let unspecified = expectation(&[&["a", "b"]], &["a", "b"], &[], false, None);

        let ordered = score_quality(&expected, &actual).expect("valid score");
        let unordered = score_quality(&unspecified, &actual).expect("valid score");

        assert_eq!(ordered.order_ok, Some(false));
        assert_eq!(unordered.order_ok, None);
    }

    #[test]
    fn invalid_gold_and_zero_budget_fail_closed() {
        let invalid = expectation(&[&["a"]], &["a"], &["a"], false, None);
        assert!(invalid.validate().is_err());
        assert!(score_quality(&invalid, &observation(&["a"], 1)).is_err());

        let valid = expectation(&[&["a"]], &["a"], &[], false, None);
        assert!(score_quality(&valid, &observation(&["a"], 0)).is_err());
    }

    #[test]
    fn gold_keys_cannot_use_the_reserved_unknown_result_prefix() {
        let invalid = expectation(
            &[&["__unknown:fact:unexpected"]],
            &["__unknown:fact:unexpected"],
            &[],
            false,
            None,
        );

        assert_eq!(
            invalid
                .validate()
                .expect_err("reserved runtime key prefix must be rejected"),
            "gold keys must not use the reserved __unknown: prefix"
        );
    }
}
