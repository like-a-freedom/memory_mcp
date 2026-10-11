use eval_harness::perf::artifact::{self, sha256_hex};
use eval_harness::perf::db_observer::MAX_TEMPLATES;
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

const EXTRACT_PROFILE_JSON: &str = r#"{
  "schema": "memory-mcp-perf-profile/v1",
  "name": "runner-catalog-test",
  "budget_seconds": 60,
  "host_tag": "test",
  "scenarios": [
    {
      "id": "latency-extract",
      "driver": "in_process",
      "scales": [2],
      "iterations": 1,
      "warmup": 1
    }
  ]
}"#;

/// Runner-level regression guard for record-id fragmentation: the extract
/// pipeline writes content-sha record ids inline, and before hex
/// normalization every statement shape went unique — 64 templates plus one
/// `(overflow)` row at scale 2. The normalizer unit test pins the formula;
/// this pins the observed catalog.
#[tokio::test]
async fn runner_catalog_does_not_fragment_record_ids() {
    let dir = tempfile::tempdir().expect("tempdir");
    let profile_path = dir.path().join("profile.json");
    std::fs::write(&profile_path, EXTRACT_PROFILE_JSON).expect("write profile");
    let profile = load(&profile_path).expect("valid profile");

    let opts = options(dir.path(), "artifact.json");
    let artifact_path = opts.artifact_path.clone();
    let outcome = run_profile(&profile, "catalog-digest", opts).await;
    assert!(
        matches!(outcome.verdict, PerfVerdict::Passed),
        "run must be healthy: {:?}",
        outcome.verdict
    );

    let written = artifact::read(&artifact_path).expect("artifact is readable");
    let record = written
        .scenarios
        .get("latency-extract-2")
        .expect("extract scenario record");
    assert!(
        record.sql.iter().any(|t| t.class == "FactWrite"),
        "extract must record FactWrite in the catalog"
    );
    assert!(
        !record.sql.iter().any(|t| t.template_hash == "(overflow)"),
        "record-id fragmentation would overflow the {MAX_TEMPLATES}-template cap: {:?}",
        record
            .sql
            .iter()
            .map(|t| &t.template_hash)
            .collect::<Vec<_>>()
    );
    assert!(
        record.sql.len() < MAX_TEMPLATES,
        "distinct templates stay under the cap: {}",
        record.sql.len()
    );
}

const ASSEMBLE_PROFILE_JSON: &str = r#"{
  "schema": "memory-mcp-perf-profile/v1",
  "name": "runner-phase-test",
  "budget_seconds": 60,
  "host_tag": "test",
  "scenarios": [
    {
      "id": "assemble-scale",
      "driver": "in_process",
      "scales": [2],
      "iterations": 1,
      "warmup": 0,
      "queries": 1
    }
  ]
}"#;

/// Phase split: catalog records carry a `phase` field so SQL from a
/// scenario's untimed setup (corpus ingest/extract) is distinguishable from
/// the timed measurement. `IngestScale` times its whole loop, so every
/// catalog record must be tagged `timed`.
#[tokio::test]
async fn runner_catalog_tags_ingest_scale_sql_as_timed_phase() {
    let dir = tempfile::tempdir().expect("tempdir");
    let profile_path = dir.path().join("profile.json");
    std::fs::write(&profile_path, PROFILE_JSON).expect("write profile");
    let profile = load(&profile_path).expect("valid profile");

    let opts = options(dir.path(), "artifact.json");
    let artifact_path = opts.artifact_path.clone();
    let outcome = run_profile(&profile, "phase-ingest-digest", opts).await;
    assert!(
        matches!(outcome.verdict, PerfVerdict::Passed),
        "run must be healthy: {:?}",
        outcome.verdict
    );

    let raw = std::fs::read_to_string(&artifact_path).expect("read artifact json");
    let json: serde_json::Value = serde_json::from_str(&raw).expect("artifact is valid json");
    let catalog = json["scenarios"]["ingest-scale-2"]["sql"]
        .as_array()
        .expect("sql catalog present");
    assert!(!catalog.is_empty(), "ingest-scale must record SQL");
    for entry in catalog {
        assert_eq!(
            entry["phase"].as_str(),
            Some("timed"),
            "every ingest-scale statement is part of the timed measurement: {entry}"
        );
    }
}

/// AssembleScale ingests and extracts the corpus as untimed setup, then times
/// the assemble calls: the catalog must contain both phases, extract writes
/// stay in `setup`, and the vector search serving assemble stays `timed`.
#[tokio::test]
async fn runner_catalog_splits_assemble_setup_from_timed_phase() {
    let dir = tempfile::tempdir().expect("tempdir");
    let profile_path = dir.path().join("profile.json");
    std::fs::write(&profile_path, ASSEMBLE_PROFILE_JSON).expect("write profile");
    let profile = load(&profile_path).expect("valid profile");

    let opts = options(dir.path(), "artifact.json");
    let artifact_path = opts.artifact_path.clone();
    let outcome = run_profile(&profile, "phase-assemble-digest", opts).await;
    assert!(
        matches!(outcome.verdict, PerfVerdict::Passed),
        "run must be healthy: {:?}",
        outcome.verdict
    );

    let raw = std::fs::read_to_string(&artifact_path).expect("read artifact json");
    let json: serde_json::Value = serde_json::from_str(&raw).expect("artifact is valid json");
    let catalog = json["scenarios"]["assemble-scale-2"]["sql"]
        .as_array()
        .expect("sql catalog present");

    let phases: std::collections::BTreeSet<&str> = catalog
        .iter()
        .filter_map(|entry| entry["phase"].as_str())
        .collect();
    assert_eq!(
        phases,
        ["setup", "timed"].into_iter().collect(),
        "assemble-scale must record both setup and timed SQL, got {phases:?}"
    );
    for entry in catalog {
        let class = entry["class"].as_str().unwrap_or_default();
        let phase = entry["phase"].as_str().unwrap_or_default();
        if class == "FactWrite" {
            assert_eq!(
                phase, "setup",
                "corpus writes belong to the untimed setup: {entry}"
            );
        }
        if class == "AnnSearch" {
            assert_eq!(
                phase, "timed",
                "assemble's vector search belongs to the timed measurement: {entry}"
            );
        }
    }
}
