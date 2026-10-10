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
