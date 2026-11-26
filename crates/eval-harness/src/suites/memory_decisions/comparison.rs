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

/// A scoped regression result never hides failures already present in the baseline.
#[derive(Debug, serde::Serialize)]
pub struct CurrentRegressionReport {
    pub schema_version: &'static str,
    pub baseline_identity: serde_json::Value,
    pub candidate_identity: serde_json::Value,
    pub no_regressions: bool,
    pub all_cases_pass: bool,
    pub current_failures: Vec<String>,
    pub counts: serde_json::Value,
    pub cases: Vec<serde_json::Value>,
    pub grader_identity: serde_json::Value,
    pub regressions: Vec<String>,
}

pub fn compare_current_replays(
    baseline: &ReplayIndex,
    candidate: &ReplayIndex,
) -> Result<CurrentRegressionReport, EvalError> {
    compare_replays(baseline, baseline)?;
    compare_replays(candidate, candidate)?;
    let a = &baseline.identity;
    let b = &candidate.identity;
    if a.suite_id != b.suite_id
        || a.corpus_sha256 != b.corpus_sha256
        || a.config_sha256 != b.config_sha256
        || a.input_schema_sha256 != b.input_schema_sha256
        || baseline.cases.len() != candidate.cases.len()
    {
        return Err(EvalError::InvalidInput(
            "current regression captures have incompatible identities or coverage".into(),
        ));
    }
    let mut current_failures = Vec::new();
    let mut cases = Vec::new();
    let mut regressions = Vec::new();
    for (old, new) in baseline.cases.iter().zip(&candidate.cases) {
        if old.case_id != new.case_id {
            return Err(EvalError::InvalidInput(
                "current regression case coverage differs".into(),
            ));
        }
        let mut passed = true;
        for (dimension, before, after) in [
            (
                "primary.evidence",
                old.evidence_success,
                new.evidence_success,
            ),
            (
                "primary.decision",
                old.decision_success,
                new.decision_success,
            ),
            ("primary.probe", old.probe_success, new.probe_success),
        ] {
            let (Some(before), Some(after)) = (before, after) else {
                return Err(EvalError::InvalidInput(
                    "current regression requires complete scored observations".into(),
                ));
            };
            passed &= after;
            if before && !after {
                regressions.push(format!("{}:{dimension}", new.case_id));
            }
        }
        let old_temporal = old
            .temporal_score
            .as_ref()
            .ok_or_else(|| EvalError::InvalidInput("missing baseline temporal score".into()))?;
        let new_temporal = new
            .temporal_score
            .as_ref()
            .ok_or_else(|| EvalError::InvalidInput("missing candidate temporal score".into()))?;
        for dimension in ["evidence_success", "decision_success", "probe_success"] {
            let before = old_temporal[dimension].as_bool().ok_or_else(|| {
                EvalError::InvalidInput("incomplete baseline temporal score".into())
            })?;
            let after = new_temporal[dimension].as_bool().ok_or_else(|| {
                EvalError::InvalidInput("incomplete candidate temporal score".into())
            })?;
            passed &= after;
            if before && !after {
                regressions.push(format!("{}:temporal.{dimension}", new.case_id));
            }
        }
        let old_reasons = old_temporal["reason_codes"]
            .as_array()
            .ok_or_else(|| EvalError::InvalidInput("missing baseline temporal reasons".into()))?;
        let new_reasons = new_temporal["reason_codes"]
            .as_array()
            .ok_or_else(|| EvalError::InvalidInput("missing candidate temporal reasons".into()))?;
        for reason in new_reasons {
            let reason_text = reason
                .as_str()
                .ok_or_else(|| EvalError::InvalidInput("invalid temporal reason".into()))?;
            if !old_reasons.contains(reason) {
                regressions.push(format!("{}:temporal.reason.{reason_text}", new.case_id));
            }
        }
        for reason in &new.reason_codes {
            if !old.reason_codes.contains(reason) {
                regressions.push(format!("{}:reason.{reason}", new.case_id));
            }
        }
        if !passed {
            current_failures.push(new.case_id.clone());
        }
        cases.push(serde_json::json!({
            "case_id":new.case_id, "status":if passed {"passed"} else {"quality_failed"},
            "primary":{"evidence_success":new.evidence_success,"decision_success":new.decision_success,"probe_success":new.probe_success},
            "temporal":new.temporal_score,"reason_codes":new.reason_codes
        }));
    }
    Ok(CurrentRegressionReport {
        schema_version: "memory-current-regression/v1",
        baseline_identity: serde_json::to_value(&baseline.identity)?,
        candidate_identity: serde_json::to_value(&candidate.identity)?,
        no_regressions: regressions.is_empty(),
        all_cases_pass: current_failures.is_empty(),
        counts: serde_json::json!({"passed":candidate.cases.len()-current_failures.len(),"failed":current_failures.len(),"invalid":0,"unobserved":0}),
        current_failures,
        cases,
        grader_identity: grader_identity(),
        regressions,
    })
}

/// Digests describe the source compiled into this grader, independent of the working tree.
pub fn grader_identity() -> serde_json::Value {
    use sha2::{Digest, Sha256};
    let hash = |source: &[u8]| hex::encode(Sha256::digest(source));
    serde_json::json!({
        "protocol":"memory-current-regression/v1",
        "scorer_sha256":hash(include_bytes!("scoring.rs")),
        "normalizer_sha256":hash(include_bytes!("normalization.rs")),
        "registration_sha256":hash(include_bytes!("cases.rs")),
        "verification_sha256":hash(include_bytes!("verification.rs")),
        "comparison_sha256":hash(include_bytes!("comparison.rs"))
    })
}
