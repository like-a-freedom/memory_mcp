use eval_harness::suites::{compare_current_replays, load_registered_replay};
use std::path::Path;

#[test]
fn historical_capture_preserves_all_three_oracle_scope_failures() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let baseline = load_registered_replay(
        &root.join("evals/paired/harness-simplification-baseline-2026-11-26/receipts/index.json"),
        &root.join("evals/corpora/memory-decisions-v9"),
    )
    .unwrap();
    let report = compare_current_replays(&baseline, &baseline).unwrap();
    assert!(report.no_regressions);
    assert!(!report.all_cases_pass);
    assert_eq!(report.current_failures.len(), 3);
    assert!(report.regressions.is_empty());
}

fn frozen() -> eval_harness::suites::ReplayIndex {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    load_registered_replay(
        &root.join("evals/paired/harness-simplification-baseline-2026-11-26/receipts/index.json"),
        &root.join("evals/corpora/memory-decisions-v9"),
    )
    .unwrap()
}

#[test]
fn new_safety_failure_in_already_failing_case_is_a_regression() {
    let baseline = frozen();
    let mut changed = serde_json::to_value(&baseline).unwrap();
    changed["cases"][1]["reason_codes"] =
        serde_json::json!(["decision_unobservable", "forbidden_source"]);
    let candidate = serde_json::from_value(changed).unwrap();
    let report = compare_current_replays(&baseline, &candidate).unwrap();
    assert!(!report.no_regressions);
    assert!(
        report
            .regressions
            .iter()
            .any(|reason| reason.ends_with(":reason.forbidden_source"))
    );
}

#[test]
fn a_new_temporal_failure_cannot_hide_behind_an_unchanged_primary_score() {
    let baseline = frozen();
    let mut changed = serde_json::to_value(&baseline).unwrap();
    changed["cases"][1]["temporal_score"]["decision_success"] = false.into();
    let candidate = serde_json::from_value(changed).unwrap();
    let report = compare_current_replays(&baseline, &candidate).unwrap();
    assert!(!report.no_regressions);
    assert!(
        report
            .regressions
            .iter()
            .any(|reason| reason.ends_with(":temporal.decision_success"))
    );
}

#[test]
fn cli_regrades_raw_receipts_and_reports_scoped_acceptance_without_product_execution() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let index = root.join("evals/paired/current-contract-baseline-2026-11-26/receipts/index.json");
    let dir = tempfile::tempdir().unwrap();
    let report = dir.path().join("comparison.json");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .args(["compare-current-memory", "--baseline"])
        .arg(&index)
        .arg("--candidate")
        .arg(&index)
        .arg("--corpus")
        .arg(root.join("evals/corpora/memory-decisions-v10"))
        .arg("--output")
        .arg(&report)
        .env("MEMORY_DECISIONS_SERVER_BIN", "/no/product/may/be/executed")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(report).unwrap()).unwrap();
    assert_eq!(
        result["inputs"]["baseline_index_sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert_eq!(
        result["inputs"]["baseline_index_sha256"],
        result["inputs"]["candidate_index_sha256"]
    );
    assert_eq!(
        result["grader_identity"]["scorer_sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert_eq!(
        result["grader_identity"]["normalizer_sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert_eq!(result["counts"]["passed"], 8);
    assert_eq!(result["counts"]["failed"], 0);
    assert_eq!(result["counts"]["invalid"], 0);
    assert_eq!(result["counts"]["unobserved"], 0);
    assert_eq!(result["cases"].as_array().unwrap().len(), 8);
    assert_eq!(result["no_regressions"], true);
    assert_eq!(result["all_cases_pass"], true);
    assert_eq!(result["current_failures"].as_array().unwrap().len(), 0);
}

#[test]
fn failed_offline_grading_is_preserved_when_the_stage_is_retried() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let baseline =
        root.join("evals/paired/current-contract-baseline-2026-11-26/receipts/index.json");
    let dir = tempfile::tempdir().unwrap();
    let failed = dir.path().join("attempt-01.json");
    let candidate = dir.path().join("candidate.json");
    std::fs::write(&candidate, b"{}").unwrap();
    let run = |output: &Path, candidate: &Path| {
        std::process::Command::new(env!("CARGO_BIN_EXE_memory-eval"))
            .args(["compare-current-memory", "--baseline"])
            .arg(&baseline)
            .arg("--candidate")
            .arg(candidate)
            .arg("--corpus")
            .arg(root.join("evals/corpora/memory-decisions-v10"))
            .arg("--output")
            .arg(output)
            .output()
            .unwrap()
    };
    assert_eq!(run(&failed, &candidate).status.code(), Some(2));
    let failure_bytes = std::fs::read(&failed).expect("failed grading attempt must survive");
    let receipt: serde_json::Value = serde_json::from_slice(&failure_bytes).unwrap();
    assert_eq!(receipt["execution_status"], "invalid");
    let successful = dir.path().join("attempt-02.json");
    assert!(run(&successful, &baseline).status.success());
    assert_eq!(std::fs::read(&failed).unwrap(), failure_bytes);
    assert_eq!(run(&failed, &baseline).status.code(), Some(2));
    assert_eq!(std::fs::read(&failed).unwrap(), failure_bytes);
}

#[test]
fn current_gate_rejects_legacy_roles_without_snapshot_cutoffs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let index =
        root.join("evals/paired/harness-simplification-baseline-2026-11-26/receipts/index.json");
    let dir = tempfile::tempdir().unwrap();
    let report = dir.path().join("invalid.json");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .args(["compare-current-memory", "--baseline"])
        .arg(&index)
        .arg("--candidate")
        .arg(&index)
        .arg("--corpus")
        .arg(root.join("evals/corpora/memory-decisions-v9"))
        .arg("--output")
        .arg(&report)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(report).unwrap()).unwrap();
    assert_eq!(result["execution_status"], "invalid");
    assert!(result.to_string().contains("separate time cutoffs"));
}

macro_rules! archived_scope_control {
    ($test:ident, $case:literal) => {
        #[test]
        fn $test() {
            let replay = serde_json::to_value(frozen()).unwrap();
            let case = replay["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|case| case["case_id"] == $case)
                .unwrap();
            assert_eq!(case["evidence_success"], true);
            assert_eq!(case["decision_success"], false);
            assert_eq!(case["temporal_score"]["probe_success"], true);
            assert_eq!(
                case["reason_codes"],
                serde_json::json!(["decision_unobservable"])
            );
        }
    };
}
archived_scope_control!(
    office_entry_preserves_correct_evidence_and_explicit_snapshot,
    "office-entry"
);
archived_scope_control!(
    similar_name_preserves_correct_evidence_and_explicit_snapshot,
    "similar-name"
);
archived_scope_control!(
    single_current_preserves_correct_evidence_and_explicit_snapshot,
    "single-current"
);
