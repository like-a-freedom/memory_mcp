use serde::{Deserialize, Serialize};

use super::cases::{DecisionOracle, ExpectedDisposition};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum EvidenceRole {
    Current,
    Historical,
    Conflicting,
    Unknown,
}

impl EvidenceRole {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Historical => "historical",
            Self::Conflicting => "conflicting",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ObservedDisposition {
    Supported,
    Conflict,
    Correction,
    Unknown,
    Unobservable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SourceObservation {
    pub(super) event_id: Option<String>,
    pub(super) text: String,
    pub(super) role: EvidenceRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Observed {
    pub(super) sources: Vec<SourceObservation>,
    pub(super) disposition: ObservedDisposition,
    pub(super) item_count: usize,
    pub(super) budget_used: usize,
    pub(super) relation_outcomes: Vec<String>,
    pub(super) guidance: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ScoreReason {
    MissingEvidence,
    WrongAssertion,
    ForbiddenSource,
    UnboundSource,
    IncorrectDisposition,
    IncorrectRole,
    DecisionUnobservable,
    UnsupportedTemporal,
    BudgetExceeded,
    Nonrepeatable,
}

impl ScoreReason {
    pub(super) fn as_str(&self) -> &'static str {
        match self {
            Self::MissingEvidence => "missing_evidence",
            Self::WrongAssertion => "wrong_assertion",
            Self::ForbiddenSource => "forbidden_source",
            Self::UnboundSource => "unbound_source",
            Self::IncorrectDisposition => "incorrect_disposition",
            Self::IncorrectRole => "incorrect_role",
            Self::DecisionUnobservable => "decision_unobservable",
            Self::UnsupportedTemporal => "unsupported_temporal",
            Self::BudgetExceeded => "budget_exceeded",
            Self::Nonrepeatable => "nonrepeatable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ProbeScore {
    pub(super) evidence_success: bool,
    pub(super) decision_success: bool,
    pub(super) probe_success: bool,
    pub(super) reason_codes: Vec<ScoreReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScoringObservationSource {
    Primary,
    TemporalRevision,
}

impl ScoringObservationSource {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::TemporalRevision => "temporal_revision",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ScoringObservation<'a> {
    pub(super) source: ScoringObservationSource,
    pub(super) observed: &'a Observed,
    pub(super) time_contract_supported: bool,
}

pub(super) fn select_scoring_observation<'a>(
    requires_separate_time_cutoffs: bool,
    primary: Option<&'a Observed>,
    temporal_revision: Option<&'a Observed>,
    temporal_snapshot_valid: bool,
) -> Option<ScoringObservation<'a>> {
    if requires_separate_time_cutoffs {
        return temporal_revision
            .map(|observed| ScoringObservation {
                source: ScoringObservationSource::TemporalRevision,
                observed,
                time_contract_supported: temporal_snapshot_valid,
            })
            .or_else(|| {
                primary.map(|observed| ScoringObservation {
                    source: ScoringObservationSource::Primary,
                    observed,
                    time_contract_supported: false,
                })
            });
    }

    primary.map(|observed| ScoringObservation {
        source: ScoringObservationSource::Primary,
        observed,
        time_contract_supported: true,
    })
}

pub(super) fn score_probe(
    expected: &DecisionOracle,
    observed: &Observed,
    budget: usize,
    time_contract_supported: bool,
) -> ProbeScore {
    let mut evidence_success = true;
    let mut decision_success = true;
    let mut reasons = Vec::new();

    if observed.budget_used > budget || observed.item_count > budget {
        evidence_success = false;
        reasons.push(ScoreReason::BudgetExceeded);
    }

    let mut supported_sources = std::collections::BTreeSet::new();
    for source in &observed.sources {
        let Some(event_id) = source.event_id.as_deref() else {
            evidence_success = false;
            reasons.push(ScoreReason::UnboundSource);
            continue;
        };
        if expected.forbidden_event_ids.iter().any(|id| id == event_id) {
            evidence_success = false;
            reasons.push(ScoreReason::ForbiddenSource);
            continue;
        }
        if let Some(assertion) = expected.expected_assertions.get(event_id) {
            if contains_assertion(&source.text, assertion) {
                supported_sources.insert(event_id);
            } else {
                reasons.push(ScoreReason::WrongAssertion);
            }
        }
    }

    if expected.evidence_alternatives.is_empty() {
        if !observed.sources.is_empty() {
            evidence_success = false;
            reasons.push(ScoreReason::ForbiddenSource);
        }
    } else {
        let complete_alternative = expected.evidence_alternatives.iter().any(|alternative| {
            !alternative.is_empty()
                && alternative.iter().all(|group| {
                    group
                        .iter()
                        .any(|event_id| supported_sources.contains(event_id.as_str()))
                })
        });
        if !complete_alternative {
            evidence_success = false;
            reasons.push(ScoreReason::MissingEvidence);
        }
    }

    let expected_observation = match expected.disposition {
        ExpectedDisposition::Answerable => Some(ObservedDisposition::Supported),
        ExpectedDisposition::Conflict => Some(ObservedDisposition::Conflict),
        ExpectedDisposition::Correction => Some(ObservedDisposition::Correction),
        ExpectedDisposition::Unknown => Some(ObservedDisposition::Unknown),
        ExpectedDisposition::UnsupportedTemporal => {
            decision_success = false;
            reasons.push(ScoreReason::UnsupportedTemporal);
            None
        }
    };
    if let Some(expected_observation) = expected_observation
        && observed.disposition != expected_observation
    {
        decision_success = false;
        if observed.disposition == ObservedDisposition::Unobservable
            || (matches!(
                &expected.disposition,
                ExpectedDisposition::Conflict | ExpectedDisposition::Correction
            ) && observed.disposition == ObservedDisposition::Supported
                && observed.relation_outcomes.is_empty())
        {
            reasons.push(ScoreReason::DecisionUnobservable);
        } else {
            reasons.push(ScoreReason::IncorrectDisposition);
        }
    }
    if let Some(expected_relation) = expected.relation_outcome.as_deref()
        && !observed
            .relation_outcomes
            .iter()
            .any(|actual| actual == expected_relation)
    {
        decision_success = false;
        reasons.push(ScoreReason::DecisionUnobservable);
    }
    if !time_contract_supported {
        decision_success = false;
        reasons.push(ScoreReason::UnsupportedTemporal);
    }

    for (event_id, role) in &expected.expected_roles {
        let Some(source) = observed
            .sources
            .iter()
            .find(|source| source.event_id.as_deref() == Some(event_id.as_str()))
        else {
            decision_success = false;
            reasons.push(ScoreReason::DecisionUnobservable);
            continue;
        };
        let expected_role = match role.as_str() {
            "current" => EvidenceRole::Current,
            "historical" => EvidenceRole::Historical,
            "conflicting" => EvidenceRole::Conflicting,
            _ => EvidenceRole::Unknown,
        };
        if source.role == EvidenceRole::Unknown {
            decision_success = false;
            reasons.push(ScoreReason::DecisionUnobservable);
        } else if source.role != expected_role {
            decision_success = false;
            reasons.push(ScoreReason::IncorrectRole);
        }
    }

    reasons.sort_by_key(|reason| serde_json::to_string(reason).unwrap_or_default());
    reasons.dedup();
    ProbeScore {
        evidence_success,
        decision_success,
        probe_success: evidence_success && decision_success,
        reason_codes: reasons,
    }
}

fn contains_assertion(content: &str, assertion: &str) -> bool {
    content.to_lowercase().contains(&assertion.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::{
        EvidenceRole, Observed, ObservedDisposition, ScoringObservationSource, SourceObservation,
        score_probe as score_probe_impl, select_scoring_observation,
    };
    use crate::suites::memory_decisions::cases::{DecisionOracle, ExpectedDisposition};

    fn oracle(
        disposition: ExpectedDisposition,
        required: &[(&str, &str)],
        alternatives: Vec<Vec<Vec<String>>>,
        forbidden: &[&str],
        roles: &[(&str, EvidenceRole)],
    ) -> DecisionOracle {
        DecisionOracle {
            probe_id: "p".into(),
            disposition,
            evidence_alternatives: alternatives,
            expected_assertions: required
                .iter()
                .map(|(id, assertion)| ((*id).to_string(), (*assertion).to_string()))
                .collect(),
            expected_roles: roles
                .iter()
                .map(|(id, role)| ((*id).to_string(), role.as_str().to_string()))
                .collect(),
            forbidden_event_ids: forbidden.iter().map(|id| (*id).to_string()).collect(),
            relation_outcome: None,
        }
    }

    fn source(id: &str, text: &str, role: EvidenceRole) -> SourceObservation {
        SourceObservation {
            event_id: Some(id.into()),
            text: text.into(),
            role,
        }
    }

    fn observed(
        sources: Vec<SourceObservation>,
        disposition: ObservedDisposition,
        budget: usize,
    ) -> Observed {
        Observed {
            sources,
            disposition,
            item_count: budget,
            budget_used: budget,
            relation_outcomes: Vec::new(),
            guidance: None,
        }
    }

    fn score_probe(
        expected: &DecisionOracle,
        observed: &Observed,
        budget: usize,
    ) -> super::ProbeScore {
        score_probe_impl(expected, observed, budget, true)
    }

    #[test]
    fn open_conflict_requires_both_sides() {
        let expected = oracle(
            ExpectedDisposition::Conflict,
            &[("a", "North City"), ("b", "South City")],
            vec![vec![vec!["a".into()], vec!["b".into()]]],
            &[],
            &[
                ("a", EvidenceRole::Conflicting),
                ("b", EvidenceRole::Conflicting),
            ],
        );
        let actual = observed(
            vec![source("a", "North City", EvidenceRole::Conflicting)],
            ObservedDisposition::Conflict,
            1,
        );

        let score = score_probe(&expected, &actual, 5);

        assert!(!score.evidence_success);
        assert!(!score.probe_success);
    }

    #[test]
    fn separate_cutoff_probe_scores_the_exact_revision_observation() {
        let expected = oracle(
            ExpectedDisposition::Answerable,
            &[("prior", "15 crates")],
            vec![vec![vec!["prior".into()]]],
            &["later"],
            &[("prior", EvidenceRole::Current)],
        );
        let current = observed(
            vec![
                source("later", "25 crates", EvidenceRole::Conflicting),
                source("prior", "15 crates", EvidenceRole::Conflicting),
            ],
            ObservedDisposition::Conflict,
            2,
        );
        let historical = observed(
            vec![source("prior", "15 crates", EvidenceRole::Current)],
            ObservedDisposition::Supported,
            1,
        );

        let selected = select_scoring_observation(true, Some(&current), Some(&historical), true)
            .expect("one scored observation");
        let score = score_probe(&expected, selected.observed, 5);

        assert_eq!(selected.source, ScoringObservationSource::TemporalRevision);
        assert!(selected.time_contract_supported);
        assert!(score.probe_success);
    }

    #[test]
    fn ordinary_probe_scores_primary_observation_even_when_temporal_diagnostic_differs() {
        let current = observed(
            vec![source("current", "15 crates", EvidenceRole::Current)],
            ObservedDisposition::Supported,
            1,
        );
        let diagnostic = observed(Vec::new(), ObservedDisposition::Unknown, 0);

        let selected = select_scoring_observation(false, Some(&current), Some(&diagnostic), false)
            .expect("one scored observation");

        assert_eq!(selected.source, ScoringObservationSource::Primary);
        assert_eq!(selected.observed, &current);
        assert!(selected.time_contract_supported);
    }

    #[test]
    fn cutoff_probe_without_exact_revision_keeps_failure_visible() {
        let current = observed(
            vec![source("current", "15 crates", EvidenceRole::Current)],
            ObservedDisposition::Supported,
            1,
        );

        let selected = select_scoring_observation(true, Some(&current), None, false)
            .expect("primary response remains diagnosable");
        let score = score_probe_impl(
            &oracle(
                ExpectedDisposition::Answerable,
                &[("current", "15 crates")],
                vec![vec![vec!["current".into()]]],
                &[],
                &[("current", EvidenceRole::Current)],
            ),
            selected.observed,
            5,
            selected.time_contract_supported,
        );

        assert_eq!(selected.source, ScoringObservationSource::Primary);
        assert!(!selected.time_contract_supported);
        assert!(!score.probe_success);
        assert!(
            score
                .reason_codes
                .contains(&super::ScoreReason::UnsupportedTemporal)
        );
    }

    #[test]
    fn correct_disposition_without_source_support_scores_zero() {
        let expected = oracle(
            ExpectedDisposition::Correction,
            &[("b", "South City")],
            vec![vec![vec!["b".into()]]],
            &[],
            &[("b", EvidenceRole::Current)],
        );
        let actual = observed(vec![], ObservedDisposition::Correction, 0);

        let score = score_probe(&expected, &actual, 5);

        assert!(!score.evidence_success);
        assert!(!score.probe_success);
    }

    #[test]
    fn evidence_without_a_public_relation_does_not_claim_conflict_decision_is_wrong() {
        let expected = oracle(
            ExpectedDisposition::Conflict,
            &[("a", "North City"), ("b", "South City")],
            vec![vec![vec!["a".into()], vec!["b".into()]]],
            &[],
            &[],
        );
        let actual = observed(
            vec![
                source("a", "North City", EvidenceRole::Unknown),
                source("b", "South City", EvidenceRole::Unknown),
            ],
            ObservedDisposition::Supported,
            2,
        );

        let score = score_probe(&expected, &actual, 5);

        assert!(score.evidence_success);
        assert!(!score.decision_success);
        assert!(
            score
                .reason_codes
                .contains(&super::ScoreReason::DecisionUnobservable)
        );
        assert!(
            !score
                .reason_codes
                .contains(&super::ScoreReason::IncorrectDisposition)
        );
    }

    #[test]
    fn correct_claim_cited_to_the_wrong_source_fails() {
        let expected = oracle(
            ExpectedDisposition::Answerable,
            &[("a", "North City")],
            vec![vec![vec!["a".into()]]],
            &[],
            &[],
        );
        let actual = observed(
            vec![source("b", "North City", EvidenceRole::Unknown)],
            ObservedDisposition::Supported,
            1,
        );

        let score = score_probe(&expected, &actual, 5);

        assert!(!score.evidence_success);
        assert!(!score.probe_success);
    }

    #[test]
    fn future_source_leakage_fails_supported_evidence() {
        let expected = oracle(
            ExpectedDisposition::Answerable,
            &[("a", "North City")],
            vec![vec![vec!["a".into()]]],
            &["b"],
            &[],
        );
        let actual = observed(
            vec![
                source("a", "North City", EvidenceRole::Current),
                source("b", "South City", EvidenceRole::Unknown),
            ],
            ObservedDisposition::Supported,
            2,
        );

        assert!(!score_probe(&expected, &actual, 5).evidence_success);
    }

    #[test]
    fn old_value_marked_current_fails_decision() {
        let expected = oracle(
            ExpectedDisposition::Correction,
            &[("a", "North City"), ("b", "South City")],
            vec![vec![vec!["b".into()]]],
            &[],
            &[
                ("a", EvidenceRole::Historical),
                ("b", EvidenceRole::Current),
            ],
        );
        let actual = observed(
            vec![
                source("a", "North City", EvidenceRole::Current),
                source("b", "South City", EvidenceRole::Current),
            ],
            ObservedDisposition::Correction,
            2,
        );

        let score = score_probe(&expected, &actual, 5);

        assert!(score.evidence_success);
        assert!(!score.decision_success);
    }

    #[test]
    fn harmless_historical_mention_does_not_break_supported_correction() {
        let expected = oracle(
            ExpectedDisposition::Correction,
            &[("a", "North City"), ("b", "South City")],
            vec![vec![vec!["b".into()]]],
            &[],
            &[
                ("a", EvidenceRole::Historical),
                ("b", EvidenceRole::Current),
            ],
        );
        let actual = observed(
            vec![
                source("a", "North City", EvidenceRole::Historical),
                source("b", "South City", EvidenceRole::Current),
            ],
            ObservedDisposition::Correction,
            2,
        );

        let score = score_probe(&expected, &actual, 5);

        assert!(score.evidence_success);
        assert!(score.decision_success);
        assert!(score.probe_success);
    }

    #[test]
    fn supported_resolution_requires_its_authoritative_source() {
        let expected = oracle(
            ExpectedDisposition::Correction,
            &[("resolution", "South City")],
            vec![vec![vec!["resolution".into()]]],
            &[],
            &[("resolution", EvidenceRole::Current)],
        );
        let actual = observed(
            vec![source("peer", "South City", EvidenceRole::Current)],
            ObservedDisposition::Correction,
            1,
        );

        assert!(!score_probe(&expected, &actual, 5).evidence_success);
    }

    #[test]
    fn unknown_empty_result_is_a_safe_refusal() {
        let expected = oracle(ExpectedDisposition::Unknown, &[], vec![], &[], &[]);
        let actual = observed(vec![], ObservedDisposition::Unknown, 0);

        let score = score_probe(&expected, &actual, 5);

        assert!(score.probe_success);
    }

    #[test]
    fn unrelated_evidence_fails_unknown_case() {
        let expected = oracle(ExpectedDisposition::Unknown, &[], vec![], &["south"], &[]);
        let actual = observed(
            vec![source("south", "East City", EvidenceRole::Unknown)],
            ObservedDisposition::Supported,
            1,
        );

        assert!(!score_probe(&expected, &actual, 5).evidence_success);
    }

    #[test]
    fn unobservable_evidence_role_does_not_count_as_a_decision() {
        let expected = oracle(
            ExpectedDisposition::Conflict,
            &[("a", "North City"), ("b", "South City")],
            vec![vec![vec!["a".into()], vec!["b".into()]]],
            &[],
            &[
                ("a", EvidenceRole::Conflicting),
                ("b", EvidenceRole::Conflicting),
            ],
        );
        let actual = observed(
            vec![
                source("a", "North City", EvidenceRole::Unknown),
                source("b", "South City", EvidenceRole::Unknown),
            ],
            ObservedDisposition::Supported,
            2,
        );

        let score = score_probe(&expected, &actual, 5);

        assert!(score.evidence_success);
        assert!(!score.decision_success);
    }

    #[test]
    fn budget_overflow_fails_the_probe() {
        let expected = oracle(
            ExpectedDisposition::Answerable,
            &[("a", "North City")],
            vec![vec![vec!["a".into()]]],
            &[],
            &[],
        );
        let actual = observed(
            vec![source("a", "North City", EvidenceRole::Current)],
            ObservedDisposition::Supported,
            6,
        );

        let score = score_probe(&expected, &actual, 5);

        assert!(!score.evidence_success);
        assert!(
            score
                .reason_codes
                .contains(&super::ScoreReason::BudgetExceeded)
        );
    }

    #[test]
    fn any_complete_alternative_evidence_set_succeeds() {
        let expected = oracle(
            ExpectedDisposition::Answerable,
            &[("a", "North City"), ("b", "North City")],
            vec![vec![vec!["a".into()]], vec![vec!["b".into()]]],
            &[],
            &[],
        );
        let actual = observed(
            vec![source("b", "North City", EvidenceRole::Current)],
            ObservedDisposition::Supported,
            1,
        );

        assert!(score_probe(&expected, &actual, 5).evidence_success);
    }

    #[test]
    fn unsupported_time_contract_has_zero_probe_success_despite_visible_evidence() {
        let expected = oracle(
            ExpectedDisposition::Correction,
            &[("b", "South City")],
            vec![vec![vec!["b".into()]]],
            &[],
            &[],
        );
        let actual = observed(
            vec![source("b", "South City", EvidenceRole::Current)],
            ObservedDisposition::Correction,
            1,
        );

        let score = score_probe_impl(&expected, &actual, 5, false);

        assert!(score.evidence_success);
        assert!(!score.probe_success);
        assert!(
            score
                .reason_codes
                .contains(&super::ScoreReason::UnsupportedTemporal)
        );
    }
}
