//! Filesystem integration tests for the published evaluation artifact contract.

use eval_harness::{
    CaseStatus, CorpusSplit, EVAL_ARTIFACT_SCHEMA_V1, EvalCaseId, EvalCaseOutcome, EvalMode,
    EvalProfile, GateStatus, LabelTrust, RunArtifact, RunFingerprint, artifact,
    artifact::write_artifact,
};

fn artifact(case_id: &str, run_id: &str) -> RunArtifact {
    let outcomes = vec![EvalCaseOutcome::new(
        "test-suite",
        case_id,
        EvalMode::RetrievalOnly,
        CorpusSplit::Development,
        LabelTrust::Official,
        CaseStatus::Passed,
    )];
    let budget_status = GateStatus::Passed;
    RunArtifact {
        schema_version: EVAL_ARTIFACT_SCHEMA_V1.to_owned(),
        run_id: run_id.to_owned(),
        profile: EvalProfile::Pr,
        started_at: chrono::DateTime::from_timestamp(0, 0).expect("epoch"),
        duration_ms: 0,
        expected_case_ids: vec![EvalCaseId::parse(case_id).expect("case id")],
        expected_cases: vec![],
        verdict: artifact::derive_run_verdict(&outcomes, &[], budget_status.clone(), &[]),
        outcomes,
        suite_summaries: vec![],
        gates: vec![],
        fingerprint: RunFingerprint {
            rust_version: "test".into(),
            os_arch: "test".into(),
            package_version: "0.0.0".into(),
            build_profile: "test".into(),
            enabled_features: vec![],
            provider: None,
            model: None,
            device: None,
            configuration_hash: "test".into(),
            git_commit: None,
            evaluator_versions: std::collections::BTreeMap::new(),
            profile_digest: "test".into(),
        },
        budget_status: Some(budget_status),
        issues: vec![],
    }
}

#[test]
fn a_published_artifact_round_trips_and_leaves_no_temporary_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("run.json");
    let expected = artifact("case-1", "published-run");

    write_artifact(&path, &expected).expect("write artifact");

    let raw = std::fs::read_to_string(&path).expect("read artifact");
    let written: RunArtifact = serde_json::from_str(&raw).expect("deserialize artifact");
    written.validate().expect("published artifact validates");
    assert_eq!(written.run_id, "published-run");
    assert_eq!(written.outcomes[0].case_id().as_str(), "case-1");
    assert!(
        !path.with_extension("json.tmp").exists(),
        "the successful write must not leave its temporary file"
    );
}

#[test]
fn a_pre_rename_write_failure_preserves_the_existing_artifact() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("run.json");
    write_artifact(&path, &artifact("case-old", "existing-run")).expect("write old artifact");

    let temporary_path = path.with_extension("json.tmp");
    std::fs::create_dir(&temporary_path).expect("block temporary-file creation");
    let failure = write_artifact(&path, &artifact("case-new", "replacement-run"));

    assert!(
        matches!(
            failure.as_ref(),
            Err(eval_harness::EvalError::Io { path, .. }) if path == &temporary_path
        ),
        "the controlled pre-rename write must fail at the temporary path: {failure:?}"
    );
    let raw = std::fs::read_to_string(&path).expect("read preserved artifact");
    let preserved: RunArtifact =
        serde_json::from_str(&raw).expect("deserialize preserved artifact");
    preserved
        .validate()
        .expect("preserved artifact still validates");
    assert_eq!(preserved.run_id, "existing-run");
    assert_eq!(preserved.outcomes[0].case_id().as_str(), "case-old");
}
