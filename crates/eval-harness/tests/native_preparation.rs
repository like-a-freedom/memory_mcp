use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn run_preparation(raw: serde_json::Value) -> (std::process::Output, bool) {
    let directory = tempfile::tempdir().expect("temporary corpus directory");
    let sources = directory.path().join("sources.json");
    let output = directory.path().join("preparation.json");
    std::fs::write(&sources, serde_json::to_vec(&raw).expect("source JSON"))
        .expect("write source bundle");
    let mut child = Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .args(["prepare-retrieval-quality", "--sources"])
        .arg(sources)
        .arg("--output")
        .arg(&output)
        .env("EMBEDDINGS_ENABLED", "false")
        .env_remove("EMBEDDINGS_PROVIDER")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start preparation CLI");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if child
            .try_wait()
            .expect("poll preparation process")
            .is_some()
        {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().expect("terminate timed out preparation");
            child.wait().expect("reap preparation");
            panic!("preparation process exceeded its bounded lifetime");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let result = child
        .wait_with_output()
        .expect("capture preparation result");
    (result, output.exists())
}

fn source() -> serde_json::Value {
    serde_json::json!({
        "case_id":"current-plan",
        "source":{
            "key":"release-note", "source_id":"release-note", "source_type":"session_summary",
            "content":"Verified: The release candidate requires a restore drill.",
            "t_ref":"2026-10-08T00:00:00Z"
        }
    })
}

#[test]
fn preparation_requires_a_real_provider_and_never_writes_a_fake_inventory() {
    let (result, output_exists) = run_preparation(serde_json::json!([source()]));
    assert_eq!(result.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("real embedding provider"),
        "provider unavailability must be explicit: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !output_exists,
        "a failed preparation must not publish an inventory"
    );
}

#[test]
fn preparation_rejects_duplicate_source_keys_before_loading_the_provider() {
    let (result, output_exists) = run_preparation(serde_json::json!([source(), source()]));
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("duplicate source key"));
    assert!(!output_exists);
}

#[test]
fn release_profile_reports_missing_native_provider_as_invalid_coverage() {
    let directory = tempfile::tempdir().expect("artifact directory");
    let artifact = directory.path().join("release.json");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates")
        .parent()
        .expect("workspace");
    let mut child = Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .current_dir(root)
        .args([
            "run",
            "--profile",
            "evals/profiles/release.json",
            "--suite",
            "retrieval-quality",
            "--artifact",
        ])
        .arg(&artifact)
        .env("EMBEDDINGS_ENABLED", "false")
        .env_remove("EMBEDDINGS_PROVIDER")
        .env_remove("RETRIEVAL_RERANKER_MODEL_DIR")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("native release process");
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.try_wait().expect("poll").is_none() {
        if Instant::now() >= deadline {
            child.kill().expect("kill");
            child.wait().expect("reap");
            panic!("release run exceeded deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!child.wait().expect("status").success());
    let report: serde_json::Value = serde_json::from_slice(&std::fs::read(&artifact).expect(
        "a mandatory native gate must publish unavailable coverage instead of omitting the suite",
    ))
    .expect("report JSON");
    assert_eq!(
        report["expected_cases"]
            .as_array()
            .expect("declared cases")
            .len(),
        37
    );
    let outcomes = report["outcomes"].as_array().expect("coverage");
    assert_eq!(outcomes.len(), 37);
    assert!(
        outcomes
            .iter()
            .all(|outcome| outcome["status"] == "invalid")
    );
    assert_ne!(report["verdict"], "passed");
}
