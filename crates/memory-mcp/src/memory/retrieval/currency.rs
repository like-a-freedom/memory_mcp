use std::collections::{HashMap, HashSet};

use crate::models::FactId;

use super::admission::{AdmissionIntent, QueryEvidence};
use crate::knowledge::api::TemporalRelationsBatch;

pub(super) struct CurrentProjectionResult {
    pub(super) fact_ids: Vec<FactId>,
    pub(super) currency_unknown: bool,
}

/// Removes known predecessors from a current-intent result. Candidates have
/// already passed retrieval admission, so a relation can only demote a fact;
/// it cannot make an otherwise unsupported counterpart eligible.
pub(super) fn project_current_candidates(
    query: &QueryEvidence,
    candidate_fact_ids: &[FactId],
    evidence: &TemporalRelationsBatch,
) -> CurrentProjectionResult {
    if query.intent != AdmissionIntent::Current {
        return CurrentProjectionResult {
            fact_ids: candidate_fact_ids.to_vec(),
            currency_unknown: false,
        };
    }
    if !evidence.evidence_available {
        return CurrentProjectionResult {
            fact_ids: Vec::new(),
            currency_unknown: true,
        };
    }

    let candidate_ids = candidate_fact_ids.iter().cloned().collect::<HashSet<_>>();
    let mut successors_by_predecessor: HashMap<FactId, HashSet<FactId>> = HashMap::new();
    let mut unresolved_predecessors = HashSet::new();

    for relation in evidence
        .relations
        .iter()
        .filter(|relation| relation.outcome.has_direction())
    {
        match (
            relation.predecessor_fact_id.as_ref(),
            relation.successor_fact_id.as_ref(),
        ) {
            (Some(predecessor), Some(successor)) => {
                successors_by_predecessor
                    .entry(predecessor.clone())
                    .or_default()
                    .insert(successor.clone());
            }
            (Some(predecessor), None) => {
                unresolved_predecessors.insert(predecessor.clone());
            }
            (None, _) => {}
        }
    }

    let cycle_members = directed_cycle_members(&successors_by_predecessor);
    let truncated_frontiers = evidence
        .truncated_fact_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let mut currency_unknown = false;
    let fact_ids = candidate_fact_ids
        .iter()
        .filter(|fact_id| {
            if cycle_members.contains(*fact_id)
                || truncated_frontiers.contains(*fact_id)
                || unresolved_predecessors.contains(*fact_id)
            {
                currency_unknown = true;
                return false;
            }

            let Some(successors) = successors_by_predecessor.get(*fact_id) else {
                return true;
            };

            if successors.len() > 1
                || successors
                    .iter()
                    .any(|successor| !candidate_ids.contains(successor))
            {
                currency_unknown = true;
            }
            false
        })
        .cloned()
        .collect();

    CurrentProjectionResult {
        fact_ids,
        currency_unknown,
    }
}

fn directed_cycle_members(
    successors_by_predecessor: &HashMap<FactId, HashSet<FactId>>,
) -> HashSet<FactId> {
    fn visit(
        current: &FactId,
        successors_by_predecessor: &HashMap<FactId, HashSet<FactId>>,
        stack: &mut Vec<FactId>,
        visited: &mut HashSet<FactId>,
        cycle_members: &mut HashSet<FactId>,
    ) {
        if let Some(cycle_start) = stack.iter().position(|fact_id| fact_id == current) {
            cycle_members.extend(stack[cycle_start..].iter().cloned());
            return;
        }
        if !visited.insert(current.clone()) {
            return;
        }

        stack.push(current.clone());
        if let Some(successors) = successors_by_predecessor.get(current) {
            for successor in successors {
                visit(
                    successor,
                    successors_by_predecessor,
                    stack,
                    visited,
                    cycle_members,
                );
            }
        }
        stack.pop();
    }

    let mut visited = HashSet::new();
    let mut cycle_members = HashSet::new();
    for predecessor in successors_by_predecessor.keys() {
        visit(
            predecessor,
            successors_by_predecessor,
            &mut Vec::new(),
            &mut visited,
            &mut cycle_members,
        );
    }
    cycle_members
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::claims::RelationForFact;
    use crate::models::claim::ClaimRelationOutcome;

    fn query(intent: AdmissionIntent) -> QueryEvidence {
        QueryEvidence {
            content_terms: Vec::new(),
            qualified_references: Vec::new(),
            resolved_entity_ids: Vec::new(),
            repeated_topics: Vec::new(),
            mandatory_reference_alternatives: Vec::new(),
            scope_reference: None,
            intent,
            empty_query: false,
        }
    }

    fn relation(
        relation_id: &str,
        outcome: ClaimRelationOutcome,
        predecessor: Option<&str>,
        successor: Option<&str>,
    ) -> RelationForFact {
        RelationForFact {
            relation_id: relation_id.to_string(),
            outcome,
            reason_code: "test".to_string(),
            evaluator_version: "test".to_string(),
            predecessor_fact_id: predecessor.map(FactId::from),
            successor_fact_id: successor.map(FactId::from),
            participants: Vec::new(),
        }
    }

    fn batch(relations: Vec<RelationForFact>) -> TemporalRelationsBatch {
        TemporalRelationsBatch {
            relations,
            evidence_available: true,
            ..TemporalRelationsBatch::default()
        }
    }

    fn ids(result: &CurrentProjectionResult) -> Vec<String> {
        result.fact_ids.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn current_projection_keeps_an_admitted_successor_and_removes_its_predecessor() {
        let result = project_current_candidates(
            &query(AdmissionIntent::Current),
            &[
                FactId::from("fact:predecessor"),
                FactId::from("fact:successor"),
            ],
            &batch(vec![relation(
                "relation:1",
                ClaimRelationOutcome::Supersession,
                Some("fact:predecessor"),
                Some("fact:successor"),
            )]),
        );

        assert_eq!(ids(&result), vec!["fact:successor"]);
        assert!(!result.currency_unknown);
    }

    #[test]
    fn current_projection_withholds_a_predecessor_when_successor_is_not_admitted() {
        let result = project_current_candidates(
            &query(AdmissionIntent::Current),
            &[FactId::from("fact:predecessor")],
            &batch(vec![relation(
                "relation:2",
                ClaimRelationOutcome::Supersession,
                Some("fact:predecessor"),
                Some("fact:successor"),
            )]),
        );

        assert!(result.fact_ids.is_empty());
        assert!(result.currency_unknown);
    }

    #[test]
    fn current_projection_keeps_both_sides_of_an_unresolved_contradiction() {
        let result = project_current_candidates(
            &query(AdmissionIntent::Current),
            &[FactId::from("fact:a"), FactId::from("fact:b")],
            &batch(vec![relation(
                "relation:3",
                ClaimRelationOutcome::Contradiction,
                Some("fact:a"),
                Some("fact:b"),
            )]),
        );

        assert_eq!(ids(&result), vec!["fact:a", "fact:b"]);
        assert!(!result.currency_unknown);
    }

    #[test]
    fn current_projection_withholds_cycle_members_as_unknown() {
        let result = project_current_candidates(
            &query(AdmissionIntent::Current),
            &[FactId::from("fact:a"), FactId::from("fact:b")],
            &batch(vec![
                relation(
                    "relation:4",
                    ClaimRelationOutcome::Supersession,
                    Some("fact:a"),
                    Some("fact:b"),
                ),
                relation(
                    "relation:5",
                    ClaimRelationOutcome::Correction,
                    Some("fact:b"),
                    Some("fact:a"),
                ),
            ]),
        );

        assert!(result.fact_ids.is_empty());
        assert!(result.currency_unknown);
    }

    #[test]
    fn current_projection_withholds_a_truncated_relation_frontier() {
        let mut evidence = batch(vec![relation(
            "relation:6",
            ClaimRelationOutcome::Supersession,
            Some("fact:predecessor"),
            Some("fact:successor"),
        )]);
        evidence.truncated = true;
        evidence
            .truncated_fact_ids
            .push(FactId::from("fact:successor"));
        let result = project_current_candidates(
            &query(AdmissionIntent::Current),
            &[
                FactId::from("fact:predecessor"),
                FactId::from("fact:successor"),
            ],
            &evidence,
        );

        assert!(result.fact_ids.is_empty());
        assert!(result.currency_unknown);
    }

    #[test]
    fn current_projection_does_not_change_non_current_results() {
        let result = project_current_candidates(
            &query(AdmissionIntent::Content),
            &[
                FactId::from("fact:predecessor"),
                FactId::from("fact:successor"),
            ],
            &batch(vec![relation(
                "relation:7",
                ClaimRelationOutcome::Supersession,
                Some("fact:predecessor"),
                Some("fact:successor"),
            )]),
        );

        assert_eq!(ids(&result), vec!["fact:predecessor", "fact:successor"]);
        assert!(!result.currency_unknown);
    }

    #[test]
    fn current_projection_abstains_when_rollout_withholds_relation_evidence() {
        let result = project_current_candidates(
            &query(AdmissionIntent::Current),
            &[FactId::from("fact:predecessor")],
            &TemporalRelationsBatch::default(),
        );

        assert!(result.fact_ids.is_empty());
        assert!(result.currency_unknown);
    }
}
