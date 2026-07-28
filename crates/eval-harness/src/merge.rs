use std::collections::BTreeSet;

use crate::artifact::{RunArtifact, SuiteSummary};
use crate::domain::*;
use crate::error::EvalError;

fn compute_suite_summaries(outcomes: &[EvalCaseOutcome]) -> Vec<SuiteSummary> {
    let mut by_suite: std::collections::BTreeMap<String, Vec<&EvalCaseOutcome>> =
        std::collections::BTreeMap::new();
    for outcome in outcomes {
        by_suite
            .entry(outcome.suite_id.clone())
            .or_default()
            .push(outcome);
    }

    by_suite
        .into_iter()
        .map(|(suite_id, cases)| {
            let total = cases.len();
            let passed = cases
                .iter()
                .filter(|o| o.status == CaseStatus::Passed)
                .count();
            let quality_failed = cases
                .iter()
                .filter(|o| o.status == CaseStatus::QualityFailed)
                .count();
            let invalid = cases
                .iter()
                .filter(|o| o.status == CaseStatus::Invalid)
                .count();
            let mode = cases
                .first()
                .map(|o| o.mode)
                .unwrap_or(EvalMode::RetrievalOnly);

            let mut metrics = std::collections::BTreeMap::new();
            for case in &cases {
                for (key, value) in &case.metrics {
                    metrics.entry(key.clone()).or_insert(*value);
                }
            }

            SuiteSummary {
                suite_id,
                mode,
                total,
                passed,
                quality_failed,
                invalid,
                metrics,
            }
        })
        .collect()
}

pub fn merge_shards(shards: &[RunArtifact]) -> Result<RunArtifact, EvalError> {
    if shards.is_empty() {
        return Err(EvalError::InvalidInput("no shards to merge".into()));
    }

    let first = &shards[0];
    let schema = first.schema_version.clone();
    let profile = first.profile;
    let fingerprint = first.fingerprint.clone();

    for (i, shard) in shards.iter().enumerate() {
        if shard.schema_version != schema {
            return Err(EvalError::InvalidInput(format!(
                "shard {i} has schema {} but expected {schema}",
                shard.schema_version
            )));
        }
        if shard.profile != profile {
            return Err(EvalError::InvalidInput(format!(
                "shard {i} has profile {:?} but expected {profile:?}",
                shard.profile
            )));
        }
        if shard.fingerprint.configuration_hash != fingerprint.configuration_hash {
            return Err(EvalError::InvalidInput(format!(
                "shard {i} has different configuration hash"
            )));
        }
    }

    let mut all_outcomes = Vec::new();
    let mut all_expected = BTreeSet::new();

    for shard in shards {
        all_outcomes.extend(shard.outcomes.iter().cloned());
        for id in &shard.expected_case_ids {
            all_expected.insert(id.as_str());
        }
    }

    let mut seen_ids = BTreeSet::new();
    for outcome in &all_outcomes {
        if !seen_ids.insert(outcome.case_id.as_str()) {
            return Err(EvalError::InvalidInput(format!(
                "duplicate case ID in merged shards: {}",
                outcome.case_id.as_str()
            )));
        }
    }

    let outcome_ids: BTreeSet<&str> = all_outcomes.iter().map(|o| o.case_id.as_str()).collect();

    for id in &all_expected {
        if !outcome_ids.contains(*id) {
            return Err(EvalError::InvalidInput(format!(
                "missing outcome for expected case: {id}"
            )));
        }
    }

    all_outcomes.sort_by(|a, b| a.suite_id.cmp(&b.suite_id).then(a.case_id.cmp(&b.case_id)));

    let mut expected_ids: Vec<EvalCaseId> = all_expected
        .into_iter()
        .map(|s| EvalCaseId::parse(s.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    expected_ids.sort();

    let mut metric_sums = std::collections::BTreeMap::<String, (f64, usize)>::new();
    for outcome in &all_outcomes {
        for (key, value) in &outcome.metrics {
            let entry = metric_sums.entry(key.clone()).or_insert((0.0, 0));
            entry.0 += value;
            entry.1 += 1;
        }
    }

    let suite_summaries = compute_suite_summaries(&all_outcomes);
    let gates = first.gates.clone();

    let artifact = RunArtifact {
        schema_version: schema,
        run_id: format!("merged-{}", chrono::Utc::now().timestamp()),
        profile,
        started_at: first.started_at,
        duration_ms: shards.iter().map(|s| s.duration_ms).sum(),
        expected_case_ids: expected_ids,
        outcomes: all_outcomes,
        suite_summaries,
        gates,
        fingerprint,
    };

    artifact.validate()?;

    Ok(artifact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::{RunFingerprint, SuiteSummary};

    fn make_shard(shard_idx: u32, case_ids: Vec<&str>) -> RunArtifact {
        let outcomes: Vec<EvalCaseOutcome> = case_ids
            .iter()
            .map(|id| EvalCaseOutcome {
                case_id: EvalCaseId::parse(*id).unwrap(),
                suite_id: "test-suite".into(),
                mode: EvalMode::RetrievalOnly,
                split: CorpusSplit::Development,
                label_trust: LabelTrust::Official,
                status: CaseStatus::Passed,
                metrics: std::collections::BTreeMap::new(),
                invalid_reason: None,
                failures: vec![],
                duration_ms: 100,
                attempts: 1,
            })
            .collect();

        let expected_ids: Vec<EvalCaseId> = case_ids
            .iter()
            .map(|id| EvalCaseId::parse(*id).unwrap())
            .collect();

        RunArtifact {
            schema_version: crate::EVAL_ARTIFACT_SCHEMA_V1.to_string(),
            run_id: format!("shard-{shard_idx}"),
            profile: EvalProfile::Pr,
            started_at: chrono::Utc::now(),
            duration_ms: 100,
            expected_case_ids: expected_ids,
            outcomes,
            suite_summaries: vec![SuiteSummary {
                suite_id: "test-suite".into(),
                mode: EvalMode::RetrievalOnly,
                total: case_ids.len(),
                passed: case_ids.len(),
                quality_failed: 0,
                invalid: 0,
                metrics: std::collections::BTreeMap::new(),
            }],
            gates: vec![],
            fingerprint: RunFingerprint::default_for_test(),
        }
    }

    #[test]
    fn four_shard_merge_covers_all_cases() {
        let shard0 = make_shard(0, vec!["c1", "c2"]);
        let shard1 = make_shard(1, vec!["c3", "c4"]);
        let shard2 = make_shard(2, vec!["c5", "c6"]);
        let shard3 = make_shard(3, vec!["c7", "c8"]);

        let merged = merge_shards(&[shard0, shard1, shard2, shard3]).unwrap();
        assert_eq!(merged.outcomes.len(), 8);
        assert_eq!(merged.expected_case_ids.len(), 8);
    }

    #[test]
    fn merge_rejects_empty_shards() {
        assert!(merge_shards(&[]).is_err());
    }

    #[test]
    fn merge_rejects_duplicate_case_ids() {
        let shard0 = make_shard(0, vec!["c1", "c2"]);
        let shard1 = make_shard(1, vec!["c2", "c3"]);
        assert!(merge_shards(&[shard0, shard1]).is_err());
    }

    #[test]
    fn merge_rejects_schema_mismatch() {
        let mut shard0 = make_shard(0, vec!["c1"]);
        shard0.schema_version = "wrong".into();
        let shard1 = make_shard(1, vec!["c2"]);
        assert!(merge_shards(&[shard0, shard1]).is_err());
    }

    #[test]
    fn merge_rejects_profile_mismatch() {
        let mut shard0 = make_shard(0, vec!["c1"]);
        shard0.profile = EvalProfile::Release;
        let shard1 = make_shard(1, vec!["c2"]);
        assert!(merge_shards(&[shard0, shard1]).is_err());
    }

    #[test]
    fn merge_single_shard_passthrough() {
        let shard = make_shard(0, vec!["c1", "c2"]);
        let merged = merge_shards(&[shard]).unwrap();
        assert_eq!(merged.outcomes.len(), 2);
    }
}
