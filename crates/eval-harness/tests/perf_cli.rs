//! `memory-perf` CLI contract (ADR-0082): run exits 0/1/2, writes the
//! artifact, and `--update-baseline` records a passing run as the baseline.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/perf-tiny.json")
}

fn run_memory_perf(args: &[&OsStr]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_memory-perf"))
        .args(args)
        // The profile-env assertion must not depend on the ambient shell:
        // with NER_EXTRACTOR removed, apply_env always takes the set-branch
        // and prints, regardless of what the test process inherited.
        .env_remove("NER_EXTRACTOR")
        .output()
        .expect("spawn memory-perf")
}

#[test]
fn run_with_tiny_profile_exits_zero_and_writes_artifact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let artifact = dir.path().join("perf.json");
    let output = run_memory_perf(&[
        OsStr::new("run"),
        OsStr::new("--profile"),
        fixture().as_os_str(),
        OsStr::new("--artifact"),
        artifact.as_os_str(),
        OsStr::new("--host-tag"),
        OsStr::new("cli-test"),
    ]);
    assert!(
        output.status.success(),
        "exit {:?}, stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let written = eval_harness::perf::artifact::read(&artifact).expect("artifact must be readable");
    assert_eq!(written.profile, "tiny");
    assert_eq!(written.host.host_tag, "cli-test");
    assert_eq!(written.scenarios.len(), 6, "all six scenarios recorded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("NER_EXTRACTOR"),
        "profile env must be applied: {stderr}"
    );
}

#[test]
fn update_baseline_copies_the_artifact_of_a_passing_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let artifact = dir.path().join("perf.json");
    let baseline = dir.path().join("baseline.json");
    let output = run_memory_perf(&[
        OsStr::new("run"),
        OsStr::new("--profile"),
        fixture().as_os_str(),
        OsStr::new("--artifact"),
        artifact.as_os_str(),
        OsStr::new("--update-baseline"),
        baseline.as_os_str(),
        OsStr::new("--host-tag"),
        OsStr::new("cli-test"),
    ]);
    assert!(
        output.status.success(),
        "exit {:?}, stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let recorded = eval_harness::perf::artifact::read(&baseline).expect("baseline must be written");
    assert_eq!(recorded.profile, "tiny");
}

#[test]
fn committed_profiles_load_under_schema_v1() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../evals/perf/profiles");
    let smoke = eval_harness::perf::profile::load(&root.join("perf-pr-smoke.json"))
        .expect("pr-smoke profile loads");
    assert_eq!(smoke.name, "pr-smoke");
    assert!(smoke.gates.is_empty(), "pr-smoke is integrity-only");
    let weekly = eval_harness::perf::profile::load(&root.join("perf-weekly.json"))
        .expect("weekly profile loads");
    assert_eq!(weekly.gates.len(), 2);
    assert_eq!(
        weekly
            .scenarios
            .iter()
            .find(|s| s.id.as_str() == "ingest-scale")
            .map(|s| s.scales.clone()),
        Some(vec![10, 100])
    );
}

/// A run may only claim the machine identity the profile was written for.
#[test]
fn host_tag_mismatch_is_invalid() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = run_memory_perf(&[
        OsStr::new("run"),
        OsStr::new("--profile"),
        fixture().as_os_str(),
        OsStr::new("--artifact"),
        dir.path().join("perf.json").as_os_str(),
        OsStr::new("--host-tag"),
        OsStr::new("wrong-host"),
    ]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "mismatched host tag must be invalid: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not match profile host_tag"));
}

/// Rewrites `src`'s artifact JSON through `f` into `dst`.
fn mutate_artifact(
    src: &std::path::Path,
    dst: &std::path::Path,
    f: impl FnOnce(&mut serde_json::Value),
) {
    let mut value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(src).expect("read source"))
            .expect("parse source");
    f(&mut value);
    std::fs::write(dst, serde_json::to_vec_pretty(&value).expect("serialize")).expect("write");
}

/// The baseline comparison path end to end: pass on a slower baseline,
/// regression (exit 1) on a faster one, invalid (exit 2) on an unreadable one.
#[test]
fn baseline_gates_drive_exit_codes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let artifact = dir.path().join("perf.json");
    let first = run_memory_perf(&[
        OsStr::new("run"),
        OsStr::new("--profile"),
        fixture().as_os_str(),
        OsStr::new("--artifact"),
        artifact.as_os_str(),
        OsStr::new("--host-tag"),
        OsStr::new("cli-test"),
    ]);
    assert!(
        first.status.success(),
        "first run: {}",
        String::from_utf8_lossy(&first.stderr)
    );

    let set_latency = |value: u64| {
        move |v: &mut serde_json::Value| {
            for record in v["scenarios"]
                .as_object_mut()
                .expect("scenarios")
                .values_mut()
            {
                for key in [
                    "p50_us", "p90_us", "p95_us", "p99_us", "mean_us", "total_us",
                ] {
                    record["latencies"][key] = serde_json::json!(value);
                }
            }
        }
    };

    // A baseline far slower than the current run must pass (ratio < 1.25).
    let slow = dir.path().join("slow.json");
    mutate_artifact(&artifact, &slow, set_latency(10_000_000));
    let vs_slow = run_memory_perf(&[
        OsStr::new("run"),
        OsStr::new("--profile"),
        fixture().as_os_str(),
        OsStr::new("--artifact"),
        dir.path().join("vs-slow.json").as_os_str(),
        OsStr::new("--baseline"),
        slow.as_os_str(),
        OsStr::new("--host-tag"),
        OsStr::new("cli-test"),
    ]);
    assert_eq!(
        vs_slow.status.code(),
        Some(0),
        "slower baseline must pass: {}",
        String::from_utf8_lossy(&vs_slow.stderr)
    );

    // A baseline far faster than the current run must regress (exit 1).
    let fast = dir.path().join("fast.json");
    mutate_artifact(&artifact, &fast, set_latency(1));
    let vs_fast = run_memory_perf(&[
        OsStr::new("run"),
        OsStr::new("--profile"),
        fixture().as_os_str(),
        OsStr::new("--artifact"),
        dir.path().join("vs-fast.json").as_os_str(),
        OsStr::new("--baseline"),
        fast.as_os_str(),
        OsStr::new("--host-tag"),
        OsStr::new("cli-test"),
    ]);
    assert_eq!(
        vs_fast.status.code(),
        Some(1),
        "faster baseline must regress: {}",
        String::from_utf8_lossy(&vs_fast.stderr)
    );
    assert!(
        String::from_utf8_lossy(&vs_fast.stderr).contains("ratio"),
        "regression reason must name the ratio"
    );

    // An unreadable baseline is invalid (exit 2), never a silent pass.
    let corrupt = dir.path().join("corrupt.json");
    mutate_artifact(&artifact, &corrupt, |v| {
        v["schema"] = serde_json::json!("memory-mcp-perf/v2");
    });
    let vs_corrupt = run_memory_perf(&[
        OsStr::new("run"),
        OsStr::new("--profile"),
        fixture().as_os_str(),
        OsStr::new("--artifact"),
        dir.path().join("vs-corrupt.json").as_os_str(),
        OsStr::new("--baseline"),
        corrupt.as_os_str(),
        OsStr::new("--host-tag"),
        OsStr::new("cli-test"),
    ]);
    assert_eq!(
        vs_corrupt.status.code(),
        Some(2),
        "corrupt baseline must be invalid: {}",
        String::from_utf8_lossy(&vs_corrupt.stderr)
    );
    assert!(String::from_utf8_lossy(&vs_corrupt.stderr).contains("baseline read failed"));
}
