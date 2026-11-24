use std::process::Command;

#[test]
fn compares_normalized_replays_without_a_server_or_agent() {
    let directory = tempfile::tempdir().unwrap();
    let left = directory.path().join("left.json");
    let right = directory.path().join("right.json");
    let replay = serde_json::json!({
        "schema_version": "memory-decisions-replay/v1",
        "identity": {
            "suite_id": "memory-decisions",
            "product_revision": "frozen-revision",
            "product_binary_sha256": "1".repeat(64),
            "corpus_sha256": "2".repeat(64),
            "config_sha256": "3".repeat(64),
            "input_schema_sha256": "4".repeat(64)
        },
        "cases": [{
            "case_id": "late-correction",
            "source_order": ["correction"],
            "roles": ["current"],
            "disposition": "supported",
            "evidence_success": true,
            "decision_success": true,
            "probe_success": true,
            "reason_codes": [],
            "receipt_path": "/first/receipt.json",
            "receipt_sha256": "5".repeat(64)
        }]
    });
    let write = |path: &std::path::Path, replay: &serde_json::Value| {
        std::fs::write(path, serde_json::to_vec(replay).unwrap()).unwrap();
    };
    write(&left, &replay);
    let mut repeated = replay.clone();
    repeated["cases"][0]["receipt_path"] = "/second/receipt.json".into();
    repeated["cases"][0]["receipt_sha256"] = "6".repeat(64).into();
    write(&right, &repeated);
    let compare = || {
        Command::new(env!("CARGO_BIN_EXE_memory-eval"))
            .env_clear()
            .arg("compare-decision-replays")
            .arg("--left")
            .arg(&left)
            .arg("--right")
            .arg(&right)
            .output()
            .unwrap()
    };
    let result = compare();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );

    repeated["cases"][0]["source_order"][0] = "superseded".into();
    write(&right, &repeated);
    let result = compare();
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("late-correction"));
}
