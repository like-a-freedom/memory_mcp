use std::collections::BTreeSet;

use crate::artifact::{GateStatus, RunArtifact, SuiteSummary};
use crate::domain::*;
use crate::error::EvalError;
use crate::profile::ProfileManifest;

/// Merges per-shard artifacts into one truthful Evaluation Artifact.
///
/// Summary math runs through the same `SuiteReducer` seam a direct run uses
/// (ADR-0025), and gates, budget status, and verdict are re-derived from the
/// merged outcome set instead of being copied from one shard.
pub fn merge_shards(
    shards: &[RunArtifact],
    manifest: &ProfileManifest,
) -> Result<RunArtifact, EvalError> {
    if shards.is_empty() {
        return Err(EvalError::InvalidInput("no shards to merge".into()));
    }

    let first = &shards[0];
    let schema = first.schema_version.clone();
    let fingerprint = first.fingerprint.clone();

    for (i, shard) in shards.iter().enumerate() {
        if shard.schema_version != schema {
            return Err(EvalError::InvalidInput(format!(
                "shard {i} has schema {} but expected {schema}",
                shard.schema_version
            )));
        }
        if shard.profile != manifest.profile {
            return Err(EvalError::InvalidInput(format!(
                "shard {i} has profile {:?} but manifest declares {:?}",
                shard.profile, manifest.profile
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
        let key = (
            outcome.case_key.suite_id.as_str(),
            outcome.case_key.case_id.as_str(),
        );
        if !seen_ids.insert(key) {
            return Err(EvalError::InvalidInput(format!(
                "duplicate case in merged shards: suite `{}` case `{}`",
                outcome.case_key.suite_id.as_str(),
                outcome.case_id().as_str()
            )));
        }
    }

    // Expected ids are bare corpus case ids; outcomes are suite-scoped, so a
    // bare id is covered when any suite produced a case with that id.
    let outcome_case_ids: BTreeSet<&str> =
        all_outcomes.iter().map(|o| o.case_id().as_str()).collect();

    for id in &all_expected {
        if !outcome_case_ids.contains(*id) {
            return Err(EvalError::InvalidInput(format!(
                "missing outcome for expected case: {id}"
            )));
        }
    }

    all_outcomes.sort_by(|a, b| {
        a.suite_id()
            .cmp(b.suite_id())
            .then(a.case_id().cmp(b.case_id()))
    });

    let mut expected_ids: Vec<EvalCaseId> = all_expected
        .into_iter()
        .map(|s| EvalCaseId::parse(s.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    expected_ids.sort();

    // Reduce through the suite reducers so a merged artifact cannot disagree
    // with a direct run of the same suites. `all_outcomes` is consumed by the
    // grouping below; one copy (`pending_outcomes`) backs the gate-evaluation
    // artifact and is then partially moved into the final artifact.
    let pending_outcomes = all_outcomes.clone();
    let mut by_suite: std::collections::BTreeMap<String, Vec<EvalCaseOutcome>> =
        std::collections::BTreeMap::new();
    for outcome in all_outcomes {
        by_suite
            .entry(outcome.suite_id().to_string())
            .or_default()
            .push(outcome);
    }
    let suite_summaries = by_suite
        .into_iter()
        .map(|(suite_id, outcomes)| {
            crate::suites::registry::reducer_for(&suite_id).reduce(&outcomes)
        })
        .collect::<Result<Vec<Vec<SuiteSummary>>, EvalError>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

    // Budget semantics match a direct run: the budget is checked against the
    // merged wall-clock-equivalent duration. Shards are usually run in
    // parallel, so the summed duration is conservative (total compute, not
    // wall clock) — a merged artifact can fail the budget gate when every
    // shard passed it, which is deliberate.
    let duration_ms = shards.iter().map(|s| s.duration_ms).sum::<u64>();
    let budget_status = if manifest.time_budget_seconds > 0 {
        let budget_ms = manifest.time_budget_seconds * 1000;
        if duration_ms > budget_ms {
            Some(GateStatus::Failed)
        } else {
            Some(GateStatus::Passed)
        }
    } else {
        None
    };

    let pending = RunArtifact {
        schema_version: crate::EVAL_ARTIFACT_SCHEMA_V1.to_string(),
        run_id: "pending".into(),
        profile: manifest.profile,
        started_at: chrono::Utc::now(),
        duration_ms,
        expected_case_ids: expected_ids.clone(),
        expected_cases: vec![],
        outcomes: pending_outcomes,
        suite_summaries: suite_summaries.clone(),
        gates: vec![],
        fingerprint: fingerprint.clone(),
        budget_status: None,
        verdict: RunVerdict::default(),
        issues: vec![],
    };
    let gates = crate::evaluate_gates(&manifest.gates, &pending, None)?;
    let budget = budget_status.unwrap_or(GateStatus::Invalid);
    // Shard issues are intentionally not carried over: coverage and validity
    // are re-derived from the merged outcome set (a shard that under/overran
    // coverage fails the expected-case checks below), so stale per-shard
    // issues would mislabel an otherwise complete merge.
    let verdict = derive_run_verdict(&pending.outcomes, &gates, budget.clone(), &[]);

    let artifact = RunArtifact {
        schema_version: crate::EVAL_ARTIFACT_SCHEMA_V2.to_string(),
        run_id: format!("merged-{}", chrono::Utc::now().timestamp()),
        profile: manifest.profile,
        started_at: first.started_at,
        duration_ms,
        expected_case_ids: expected_ids,
        expected_cases: vec![],
        outcomes: pending.outcomes,
        suite_summaries: pending.suite_summaries,
        gates,
        fingerprint,
        budget_status: Some(budget),
        verdict,
        issues: vec![],
    };

    artifact.validate()?;

    Ok(artifact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::{RunFingerprint, SuiteSummary};

    fn test_manifest(profile: EvalProfile) -> ProfileManifest {
        ProfileManifest {
            schema_version: "memory-mcp-eval-profile/v1".into(),
            profile,
            time_budget_seconds: 600,
            suites: vec![],
            gates: vec![],
        }
    }

    fn make_shard(shard_idx: u32, case_ids: Vec<&str>) -> RunArtifact {
        make_shard_with_suite(shard_idx, "test-suite", case_ids)
    }

    fn make_shard_with_suite(shard_idx: u32, suite_id: &str, case_ids: Vec<&str>) -> RunArtifact {
        let outcomes: Vec<EvalCaseOutcome> = case_ids
            .iter()
            .map(|id| {
                EvalCaseOutcome::new(
                    suite_id,
                    *id,
                    EvalMode::RetrievalOnly,
                    CorpusSplit::Development,
                    LabelTrust::Official,
                    CaseStatus::Passed,
                )
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
            expected_cases: vec![],
            outcomes,
            suite_summaries: vec![SuiteSummary {
                suite_id: suite_id.into(),
                mode: EvalMode::RetrievalOnly,
                total: case_ids.len(),
                passed: case_ids.len(),
                quality_failed: 0,
                invalid: 0,
                metrics: std::collections::BTreeMap::new(),
            }],
            gates: vec![],
            fingerprint: RunFingerprint::default_for_test(),
            budget_status: None,
            verdict: crate::domain::RunVerdict::default(),
            issues: vec![],
        }
    }

    fn make_shard_with_outcomes(outcomes: Vec<EvalCaseOutcome>) -> RunArtifact {
        let expected_ids: Vec<EvalCaseId> = outcomes.iter().map(|o| o.case_id().clone()).collect();
        RunArtifact {
            schema_version: crate::EVAL_ARTIFACT_SCHEMA_V1.to_string(),
            run_id: "shard-outcomes".into(),
            profile: EvalProfile::Pr,
            started_at: chrono::Utc::now(),
            duration_ms: 100,
            expected_case_ids: expected_ids,
            expected_cases: vec![],
            outcomes,
            suite_summaries: vec![],
            gates: vec![],
            fingerprint: RunFingerprint::default_for_test(),
            budget_status: None,
            verdict: crate::domain::RunVerdict::default(),
            issues: vec![],
        }
    }

    fn classification_shard_outcome(
        case_id: &str,
        suite: &str,
        tp: u64,
        fp: u64,
        fn_: u64,
    ) -> EvalCaseOutcome {
        let mut outcome = EvalCaseOutcome::new(
            suite,
            case_id,
            EvalMode::EndToEnd,
            CorpusSplit::Test,
            LabelTrust::Official,
            CaseStatus::QualityFailed,
        );
        let evidence = MetricEvidence::classification(tp, fp, fn_, 0);
        outcome
            .evidence
            .insert("classification".to_string(), evidence.clone());
        outcome.metrics = crate::metrics::render_case_metrics(
            &evidence,
            &crate::metrics::CaseMetricNames::classification("entity"),
        );
        outcome
    }

    #[test]
    fn four_shard_merge_covers_all_cases() {
        let manifest = test_manifest(EvalProfile::Pr);
        let shard0 = make_shard(0, vec!["c1", "c2"]);
        let shard1 = make_shard(1, vec!["c3", "c4"]);
        let shard2 = make_shard(2, vec!["c5", "c6"]);
        let shard3 = make_shard(3, vec!["c7", "c8"]);

        let merged = merge_shards(&[shard0, shard1, shard2, shard3], &manifest).unwrap();
        assert_eq!(merged.outcomes.len(), 8);
        assert_eq!(merged.expected_case_ids.len(), 8);
        assert_eq!(merged.verdict, RunVerdict::Passed);
    }

    #[test]
    fn merge_rejects_empty_shards() {
        let manifest = test_manifest(EvalProfile::Pr);
        assert!(merge_shards(&[], &manifest).is_err());
    }

    #[test]
    fn merge_accepts_same_case_ids_across_suites() {
        // Two shards from different suites sharing the same corpus case ids
        // (the NER quality profile shape) must merge without a duplicate.
        let manifest = test_manifest(EvalProfile::Pr);
        let shard0 = make_shard_with_suite(0, "ner-quality-anno", vec!["q-en-1", "q-en-2"]);
        let shard1 = make_shard_with_suite(1, "ner-quality-regex", vec!["q-en-1", "q-en-2"]);
        let merged = merge_shards(&[shard0, shard1], &manifest).unwrap();
        assert_eq!(merged.outcomes.len(), 4);
        assert_eq!(merged.expected_case_ids.len(), 2);
        assert_eq!(merged.suite_summaries.len(), 2);
    }

    #[test]
    fn merge_rejects_duplicate_case_ids() {
        let manifest = test_manifest(EvalProfile::Pr);
        let shard0 = make_shard(0, vec!["c1", "c2"]);
        let shard1 = make_shard(1, vec!["c2", "c3"]);
        assert!(merge_shards(&[shard0, shard1], &manifest).is_err());
    }

    #[test]
    fn merge_rejects_schema_mismatch() {
        let manifest = test_manifest(EvalProfile::Pr);
        let mut shard0 = make_shard(0, vec!["c1"]);
        shard0.schema_version = "wrong".into();
        let shard1 = make_shard(1, vec!["c2"]);
        assert!(merge_shards(&[shard0, shard1], &manifest).is_err());
    }

    #[test]
    fn merge_rejects_profile_mismatch() {
        let manifest = test_manifest(EvalProfile::Pr);
        let mut shard0 = make_shard(0, vec!["c1"]);
        shard0.profile = EvalProfile::Release;
        let shard1 = make_shard(1, vec!["c2"]);
        assert!(merge_shards(&[shard0, shard1], &manifest).is_err());
    }

    #[test]
    fn merge_single_shard_passthrough() {
        let manifest = test_manifest(EvalProfile::Pr);
        let shard = make_shard(0, vec!["c1", "c2"]);
        let merged = merge_shards(&[shard], &manifest).unwrap();
        assert_eq!(merged.outcomes.len(), 2);
    }

    #[test]
    fn merged_classification_metrics_aggregate_instead_of_first_wins() {
        let manifest = test_manifest(EvalProfile::Pr);
        // Shard A: tp=2 fp=0 fn=1 (per-case f1 0.8); shard B: tp=1 fp=0 fn=2.
        let shard_a = make_shard_with_outcomes(vec![classification_shard_outcome(
            "c1",
            "extraction",
            2,
            0,
            1,
        )]);
        let shard_b = make_shard_with_outcomes(vec![classification_shard_outcome(
            "c2",
            "extraction",
            1,
            0,
            2,
        )]);
        let merged = merge_shards(&[shard_a, shard_b], &manifest).unwrap();
        let summary = merged
            .suite_summaries
            .iter()
            .find(|s| s.suite_id == "extraction")
            .unwrap();
        // Aggregate tp=3 fp=0 fn=3 -> precision 1.0, recall 0.5, f1 2/3.
        assert!(
            (summary.metrics["entity_f1"] - 2.0 / 3.0).abs() < 1e-9,
            "got {}",
            summary.metrics["entity_f1"]
        );
        assert_eq!(summary.total, 2);
        assert_eq!(summary.quality_failed, 2);
    }

    #[test]
    fn merged_verdict_reflects_quality_failures() {
        let manifest = test_manifest(EvalProfile::Pr);
        let shard = make_shard_with_outcomes(vec![classification_shard_outcome(
            "c1",
            "extraction",
            1,
            0,
            2,
        )]);
        let merged = merge_shards(&[shard], &manifest).unwrap();
        assert_eq!(merged.verdict, RunVerdict::QualityFailed);
    }
}
