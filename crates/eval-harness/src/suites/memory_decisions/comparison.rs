use crate::EvalError;

use super::receipts::{REPLAY_SCHEMA_V1, ReplayCase, ReplayIndex};

pub fn compare_replays(left: &ReplayIndex, right: &ReplayIndex) -> Result<(), EvalError> {
    for replay in [left, right] {
        if replay.schema_version != REPLAY_SCHEMA_V1 {
            return Err(EvalError::InvalidInput(format!(
                "unsupported decision replay schema {:?}",
                replay.schema_version
            )));
        }
    }
    if left.cases.is_empty() || right.cases.is_empty() {
        return Err(EvalError::InvalidInput(
            "decision replays require at least one case".into(),
        ));
    }
    if left.identity != right.identity {
        return Err(EvalError::InvalidInput(
            "replay identity differs (product, corpus, configuration, or input schema)".into(),
        ));
    }
    if left.cases.len() != right.cases.len() {
        return Err(EvalError::InvalidInput(format!(
            "replay case count differs: {} vs {}",
            left.cases.len(),
            right.cases.len()
        )));
    }

    reject_duplicate_case_ids(&left.cases)?;
    reject_duplicate_case_ids(&right.cases)?;

    for (left_case, right_case) in left.cases.iter().zip(&right.cases) {
        if left_case.case_id != right_case.case_id {
            return Err(EvalError::InvalidInput(format!(
                "replay case order differs: {} vs {}",
                left_case.case_id, right_case.case_id
            )));
        }
        if !same_case_semantics(left_case, right_case) {
            return Err(EvalError::InvalidInput(format!(
                "replay result differs for case {}",
                left_case.case_id
            )));
        }
    }

    Ok(())
}

fn reject_duplicate_case_ids(cases: &[ReplayCase]) -> Result<(), EvalError> {
    let mut ids = std::collections::BTreeSet::new();
    for case in cases {
        if !ids.insert(case.case_id.as_str()) {
            return Err(EvalError::InvalidInput(format!(
                "replay contains duplicate case id {}",
                case.case_id
            )));
        }
    }
    Ok(())
}

fn same_case_semantics(left: &ReplayCase, right: &ReplayCase) -> bool {
    left.source_order == right.source_order
        && left.roles == right.roles
        && left.disposition == right.disposition
        && left.evidence_success == right.evidence_success
        && left.decision_success == right.decision_success
        && left.probe_success == right.probe_success
        && left.score_observation_source == right.score_observation_source
        && left.score_observation == right.score_observation
        && left.temporal_observation == right.temporal_observation
        && left.temporal_score == right.temporal_score
        && left.evidence_enabled_temporal_observation == right.evidence_enabled_temporal_observation
        && left.evidence_enabled_temporal_score == right.evidence_enabled_temporal_score
        && left.reason_codes == right.reason_codes
}

#[cfg(test)]
mod tests {
    use super::super::receipts::{REPLAY_SCHEMA_V1, ReplayCase, ReplayIdentity, ReplayIndex};
    use super::compare_replays;

    fn replay() -> ReplayIndex {
        ReplayIndex {
            schema_version: REPLAY_SCHEMA_V1.into(),
            identity: ReplayIdentity {
                suite_id: "memory-decisions".into(),
                product_revision: "revision".into(),
                product_binary_sha256: "binary".into(),
                corpus_sha256: "corpus".into(),
                config_sha256: "config".into(),
                input_schema_sha256: Some("schema".into()),
            },
            cases: vec![ReplayCase {
                case_id: "case-a".into(),
                source_order: vec!["source-1".into()],
                roles: vec!["current".into()],
                disposition: "answerable".into(),
                evidence_success: Some(true),
                decision_success: Some(true),
                probe_success: Some(true),
                score_observation_source: None,
                score_observation: None,
                temporal_observation: None,
                temporal_score: None,
                evidence_enabled_temporal_observation: None,
                evidence_enabled_temporal_score: None,
                reason_codes: vec![],
                receipt_path: "/run-a/receipt.json".into(),
                receipt_sha256: "receipt-a".into(),
            }],
        }
    }

    #[test]
    fn equal_semantic_results_allow_different_receipt_locations_and_hashes() {
        let left = replay();
        let mut right = replay();
        right.cases[0].receipt_path = "/run-b/receipt.json".into();
        right.cases[0].receipt_sha256 = "receipt-b".into();

        assert!(compare_replays(&left, &right).is_ok());
    }

    #[test]
    fn changed_source_order_fails_replay_comparison() {
        let left = replay();
        let mut right = replay();
        right.cases[0].source_order = vec!["source-2".into()];

        assert!(compare_replays(&left, &right).is_err());
    }

    #[test]
    fn changed_disposition_fails_replay_comparison() {
        let left = replay();
        let mut right = replay();
        right.cases[0].disposition = "conflict".into();

        assert!(compare_replays(&left, &right).is_err());
    }

    #[test]
    fn changed_score_fails_replay_comparison() {
        let left = replay();
        let mut right = replay();
        right.cases[0].probe_success = Some(false);

        assert!(compare_replays(&left, &right).is_err());
    }

    #[test]
    fn changed_temporal_observation_fails_replay_comparison() {
        let left = replay();
        let mut right = replay();
        right.cases[0].temporal_observation = Some(
            serde_json::from_value(serde_json::json!({
                "sources":[{"event_id":"source-2","text":"South","role":"current"}],
                "disposition":"supported",
                "item_count":1,
                "budget_used":1,
                "relation_outcomes":[],
                "guidance":null
            }))
            .expect("valid temporal observation"),
        );

        assert!(compare_replays(&left, &right).is_err());
    }

    #[test]
    fn different_product_or_corpus_identity_fails_replay_comparison() {
        let left = replay();
        let mut right = replay();
        right.identity.product_binary_sha256 = "other-binary".into();

        assert!(compare_replays(&left, &right).is_err());
    }

    #[test]
    fn duplicate_case_ids_fail_replay_comparison() {
        let mut left = replay();
        left.cases.push(left.cases[0].clone());
        let right = left.clone();

        assert!(compare_replays(&left, &right).is_err());
    }
}
