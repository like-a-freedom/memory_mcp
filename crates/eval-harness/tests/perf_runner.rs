use eval_harness::perf::artifact::{self, sha256_hex};
use eval_harness::perf::domain::ARTIFACT_SCHEMA;
use eval_harness::perf::profile::{DriverKind, PerfProfile, ScenarioId, ScenarioSpec, load};
use eval_harness::perf::runner::{RunOptions, run_profile};
use eval_harness::perf::verdict::PerfVerdict;

const PROFILE_JSON: &str = r#"{
  "schema": "memory-mcp-perf-profile/v1",
  "name": "runner-test",
  "budget_seconds": 60,
  "host_tag": "test",
  "scenarios": [
    {
      "id": "ingest-scale",
      "driver": "in_process",
      "scales": [2],
      "iterations": 1,
      "warmup": 1
    }
  ]
}"#;

fn options(dir: &std::path::Path, artifact_name: &str) -> RunOptions {
    RunOptions {
        artifact_path: dir.join(artifact_name),
        baseline_path: None,
        host_tag: "test".to_owned(),
        commit: "deadbeef".to_owned(),
        now_iso: "2026-10-10T00:00:00Z".to_owned(),
    }
}

#[tokio::test]
async fn runner_writes_artifact_with_recorded_latency_and_passes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let profile_path = dir.path().join("profile.json");
    std::fs::write(&profile_path, PROFILE_JSON).expect("write profile");
    let raw = std::fs::read(&profile_path).expect("read profile bytes");
    let digest = sha256_hex(&raw);
    let profile = load(&profile_path).expect("valid profile");

    let opts = options(dir.path(), "artifact.json");
    let artifact_path = opts.artifact_path.clone();
    let outcome = run_profile(&profile, &digest, opts).await;

    assert!(
        artifact_path.exists(),
        "artifact must be written even before the verdict"
    );
    let written = artifact::read(&artifact_path).expect("artifact is readable");
    assert_eq!(written.schema, ARTIFACT_SCHEMA);
    assert_eq!(written.profile_digest, digest);

    let record = written
        .scenarios
        .get("ingest-scale-2")
        .expect("scenario record keyed by scenario and scale");
    let stats = record.latencies.as_ref().expect("latencies present");
    assert_eq!(
        stats.count, 2,
        "warmup iteration discarded; one recorded iteration at scale 2 = 2 ingests"
    );
    assert!(written.duration_ms > 0, "run took measurable time");
    let classes: std::collections::BTreeSet<&str> =
        record.sql.iter().map(|t| t.class.as_str()).collect();
    for expected in ["EpisodeWrite", "ProvenanceRead"] {
        assert!(
            classes.contains(expected),
            "SQL catalog must record {expected}, got {classes:?}"
        );
    }
    assert!(
        !record.sql.iter().any(|t| t.template_hash == "(overflow)"),
        "scale-2 catalog must stay under the template cap"
    );
    assert!(
        matches!(outcome.verdict, PerfVerdict::Passed),
        "no baseline means gates are skipped: {:?}",
        outcome.verdict
    );
}

#[tokio::test]
async fn exhausted_budget_writes_artifact_and_invalid_verdict() {
    let dir = tempfile::tempdir().expect("tempdir");
    let profile = PerfProfile {
        name: "zero-budget".to_owned(),
        budget_seconds: 0,
        host_tag: "test".to_owned(),
        env: Default::default(),
        scenarios: vec![ScenarioSpec {
            id: ScenarioId::IngestScale,
            driver: DriverKind::InProcess,
            scales: vec![2],
            iterations: 1,
            warmup: 0,
            queries: None,
        }],
        gates: vec![],
    };

    let opts = options(dir.path(), "artifact.json");
    let artifact_path = opts.artifact_path.clone();
    let outcome = run_profile(&profile, "test-digest", opts).await;

    assert!(artifact_path.exists(), "partial artifact still written");
    match &outcome.verdict {
        PerfVerdict::Invalid(reasons) => assert!(
            reasons.iter().any(|r| r.contains("budget")),
            "reason must mention budget: {reasons:?}"
        ),
        other => panic!("expected Invalid, got {other:?}"),
    }
}
