use eval_harness::suites::load_verified_replay;
use std::path::PathBuf;
fn baseline() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/paired/deterministic-quality-2026-11-21/baseline-v4-1-valid")
}
#[test]
fn archived_raw_receipts_reconstruct_the_registered_replay() {
    load_verified_replay(&baseline().join("index.json")).expect("portable verified replay");
}
#[test]
fn detached_decision_score_is_rejected_even_when_repeated() {
    let dir = tempfile::tempdir().unwrap();
    let mut index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(baseline().join("index.json")).unwrap()).unwrap();
    for entry in std::fs::read_dir(baseline()).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name().to_string_lossy().starts_with("receipt-") {
            std::fs::copy(entry.path(), dir.path().join(entry.file_name())).unwrap();
        }
    }
    index["cases"][0]["evidence_enabled_temporal_score"]["decision_success"] =
        serde_json::json!(true);
    std::fs::write(
        dir.path().join("index.json"),
        serde_json::to_vec(&index).unwrap(),
    )
    .unwrap();
    assert!(load_verified_replay(&dir.path().join("index.json")).is_err());
}
#[test]
fn missing_raw_receipts_never_degrade_to_index_only_acceptance() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(baseline().join("index.json"), dir.path().join("index.json")).unwrap();
    assert!(load_verified_replay(&dir.path().join("index.json")).is_err());
}

#[test]
fn self_consistent_but_changed_probe_request_is_not_registered() {
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(baseline()).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name().to_string_lossy().ends_with(".json") {
            std::fs::copy(entry.path(), dir.path().join(entry.file_name())).unwrap();
        }
    }
    let path = dir.path().join("index.json");
    let mut index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let case = &mut index["cases"][0];
    let file = dir.path().join(case["receipt_path"].as_str().unwrap());
    let mut raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    raw["evidence_enabled_diagnostic"]["temporal_revision_request"]["query"] =
        serde_json::json!("");
    let bytes = serde_json::to_vec_pretty(&raw).unwrap();
    std::fs::write(&file, &bytes).unwrap();
    case["receipt_sha256"] = serde_json::json!(hex::encode(Sha256::digest(&bytes)));
    std::fs::write(&path, serde_json::to_vec(&index).unwrap()).unwrap();
    assert!(
        eval_harness::suites::load_registered_replay(
            &path,
            &baseline().join("../../../corpora/memory-decisions-v4")
        )
        .is_err()
    );
}

#[test]
fn archived_raw_receipts_match_the_actual_registered_corpus() {
    eval_harness::suites::load_registered_replay(
        &baseline().join("index.json"),
        &baseline().join("../../../corpora/memory-decisions-v4"),
    )
    .expect("registered baseline must remain portable");
}

fn reject_changed_registration(change: impl FnOnce(&mut serde_json::Value), expected: &str) {
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(baseline()).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name().to_string_lossy().ends_with(".json") {
            std::fs::copy(entry.path(), dir.path().join(entry.file_name())).unwrap();
        }
    }
    let path = dir.path().join("index.json");
    let mut index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let case = &mut index["cases"][0];
    let file = dir.path().join(case["receipt_path"].as_str().unwrap());
    let mut raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    change(&mut raw);
    let bytes = serde_json::to_vec_pretty(&raw).unwrap();
    std::fs::write(file, &bytes).unwrap();
    case["receipt_sha256"] = serde_json::json!(hex::encode(Sha256::digest(&bytes)));
    std::fs::write(&path, serde_json::to_vec(&index).unwrap()).unwrap();
    let error = eval_harness::suites::load_registered_replay(
        &path,
        &baseline().join("../../../corpora/memory-decisions-v4"),
    )
    .unwrap_err();
    assert!(error.to_string().contains(expected), "{error}");
}
#[test]
fn changed_private_oracle_cannot_redefine_registered_success() {
    reject_changed_registration(
        |raw| raw["desired_goal"]["oracle"]["probe_id"] = serde_json::json!("unregistered"),
        "oracle differs from registration",
    );
}
#[test]
fn changed_checkpoint_cannot_attest_the_registered_history() {
    reject_changed_registration(
        |raw| {
            raw["evidence_enabled_diagnostic"]["revision_checkpoint_receipts"][0]["known_revision"] =
                serde_json::json!(0)
        },
        "checkpoint or seed identity mismatch",
    );
}
#[test]
fn changed_seed_payload_cannot_attest_the_registered_history() {
    reject_changed_registration(
        |raw| {
            raw["evidence_enabled_diagnostic"]["ingest_receipts"][0]["request"]["content"] =
                serde_json::json!("unregistered")
        },
        "seed order or inputs differ",
    );
}

#[test]
fn primary_only_receipts_regrade_without_requiring_rollout_diagnostics() {
    use sha2::{Digest, Sha256};
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let baseline = root.join("evals/paired/harness-simplification-baseline-2026-11-26/receipts");
    let dir = tempfile::tempdir().unwrap();
    let mut index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(baseline.join("index.json")).unwrap()).unwrap();
    for case in index["cases"].as_array_mut().unwrap() {
        let mut raw: serde_json::Value = serde_json::from_slice(
            &std::fs::read(baseline.join(case["receipt_path"].as_str().unwrap())).unwrap(),
        )
        .unwrap();
        raw["schema_version"] = "memory-decisions-receipt/v5".into();
        raw["diagnostic_policy"] = "primary_only".into();
        raw["verbose_diagnostic"] = serde_json::Value::Null;
        raw["evidence_enabled_diagnostic"] = serde_json::Value::Null;
        raw["evidence_enabled_temporal_revision_score"] = serde_json::Value::Null;
        case["evidence_enabled_temporal_observation"] = serde_json::Value::Null;
        case["evidence_enabled_temporal_score"] = serde_json::Value::Null;
        let bytes = serde_json::to_vec_pretty(&raw).unwrap();
        case["receipt_sha256"] = hex::encode(Sha256::digest(&bytes)).into();
        std::fs::write(
            dir.path().join(case["receipt_path"].as_str().unwrap()),
            bytes,
        )
        .unwrap();
    }
    let path = dir.path().join("index.json");
    std::fs::write(&path, serde_json::to_vec(&index).unwrap()).unwrap();
    eval_harness::suites::load_registered_replay(&path, &root.join("evals/corpora/memory-decisions-v9"))
        .expect("primary scores retain independent request/history/oracle verification without diagnostic captures");
    assert_eq!(
        index["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|case| case["probe_success"] == true)
            .count(),
        5
    );
}

#[test]
fn detached_primary_temporal_score_is_rejected() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source = root.join("evals/paired/harness-simplification-baseline-2026-11-26/receipts");
    let dir = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(&source).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.path().join(entry.file_name())).unwrap();
    }
    let path = dir.path().join("index.json");
    let mut index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    index["cases"][1]["temporal_score"]["decision_success"] = false.into();
    std::fs::write(&path, serde_json::to_vec(&index).unwrap()).unwrap();
    assert!(load_verified_replay(&path).is_err());
}

fn reject_current_capture_mutation(
    primary_only: bool,
    change: impl FnOnce(&mut serde_json::Value),
) {
    use sha2::{Digest, Sha256};
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source = root.join(if primary_only {
        "evals/paired/harness-simplification-current-2026-11-26/receipts"
    } else {
        "evals/paired/harness-simplification-baseline-2026-11-26/receipts"
    });
    let dir = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.path().join(entry.file_name())).unwrap();
    }
    let path = dir.path().join("index.json");
    let mut index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let case = index["cases"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|case| case["case_id"] == "office-entry")
        .unwrap();
    let file = dir.path().join(case["receipt_path"].as_str().unwrap());
    let mut raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    change(&mut raw);
    let bytes = serde_json::to_vec_pretty(&raw).unwrap();
    std::fs::write(file, &bytes).unwrap();
    case["receipt_sha256"] = hex::encode(Sha256::digest(&bytes)).into();
    std::fs::write(&path, serde_json::to_vec(&index).unwrap()).unwrap();
    assert!(
        eval_harness::suites::load_registered_replay(
            &path,
            &root.join("evals/corpora/memory-decisions-v9")
        )
        .is_err(),
        "every registered repeat must be present and independently regraded"
    );
}

#[test]
fn archived_capture_cannot_omit_the_registered_second_read() {
    reject_current_capture_mutation(false, |raw| {
        raw["primary_compact_responses"]
            .as_array_mut()
            .unwrap()
            .pop();
    });
}
#[test]
fn primary_only_capture_cannot_omit_the_registered_second_read() {
    reject_current_capture_mutation(true, |raw| {
        raw["primary_compact_responses"]
            .as_array_mut()
            .unwrap()
            .pop();
    });
}
#[test]
fn archived_capture_cannot_hide_a_changed_second_read() {
    reject_current_capture_mutation(false, change_second_read);
}
#[test]
fn primary_only_capture_cannot_hide_a_changed_second_read() {
    reject_current_capture_mutation(true, change_second_read);
}

fn change_second_read(raw: &mut serde_json::Value) {
    raw["primary_compact_responses"][1] =
        serde_json::json!({"result":{"structuredContent":{"result":[]}}});
}

#[test]
fn diagnostic_formation_cannot_substitute_for_missing_primary_formation() {
    reject_current_capture_mutation(false, |raw| {
        raw["ingest_receipts"] = serde_json::json!([]);
    });
}

#[test]
fn extraction_without_a_successful_response_cannot_attest_formation() {
    reject_current_capture_mutation(true, |raw| {
        raw["extract_receipts"][0]
            .as_object_mut()
            .unwrap()
            .remove("response");
    });
}

#[test]
fn changed_extractor_cannot_reuse_the_registered_configuration() {
    reject_current_capture_mutation(true, |raw| {
        raw["primary_launch_profile"]["NER_EXTRACTOR"] = "unregistered".into();
    });
}

#[test]
fn diagnostic_extractor_cannot_differ_from_registered_configuration() {
    reject_current_capture_mutation(false, |raw| {
        raw["evidence_enabled_diagnostic"]["launch_profile"]["NER_EXTRACTOR"] = "other".into();
    });
}
