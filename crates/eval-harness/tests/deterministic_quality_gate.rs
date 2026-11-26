//! Native regression gate: one primary capture against a reviewed immutable baseline.
use eval_harness::suites::{DeterministicAcceptancePlan, load_registered_replay};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    serde_json::from_slice(&std::fs::read(path).expect("read gate artifact"))
        .expect("parse gate artifact")
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[test]
fn historical_rollout_plan_preserves_frozen_rules_when_pairing_builds() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let path =
        root.join("evals/paired/deterministic-quality-2026-11-22/claim-stage-v9-acceptance.json");
    let frozen: DeterministicAcceptancePlan = read(&path);
    let plan = rollout_plan_for_current_binary(frozen.clone());

    assert!(frozen.product_binary_sha256.is_some());
    assert_eq!(plan.product_binary_sha256, None);
    assert_eq!(plan.corpus_sha256, frozen.corpus_sha256);
    assert_eq!(plan.config_sha256, frozen.config_sha256);
    assert_eq!(plan.candidate_config_sha256, frozen.candidate_config_sha256);
    assert_eq!(plan.families, frozen.families);
    assert_eq!(plan.ordinary_cases, frozen.ordinary_cases);
    assert_eq!(
        plan.minimum_gain_basis_points,
        frozen.minimum_gain_basis_points
    );
    assert_eq!(plan.maximum_items, frozen.maximum_items);
}

fn rollout_plan_for_current_binary(
    mut frozen: DeterministicAcceptancePlan,
) -> DeterministicAcceptancePlan {
    frozen.product_binary_sha256 = None;
    frozen
}

#[test]
fn historical_rollout_validates_frozen_corpus_and_acceptance_rules() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();

    let plan = verify_frozen_rollout(&root);

    assert_eq!(plan.families.len(), 8);
    assert_eq!(plan.minimum_gain_basis_points, 2500);
    assert_eq!(
        plan.score_channel,
        eval_harness::suites::AcceptanceScoreChannel::Primary
    );
    assert_eq!(plan.product_binary_sha256, None);
}

#[test]
fn valid_boundary_oracle_has_an_exact_cutoff_and_a_future_forbidden_event() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let corpus = root.join("evals/corpora/memory-decisions-v9");
    let events: Value = read(&corpus.join("events.json"));
    let probes: Value = read(&corpus.join("probes.json"));
    let oracles: Value = read(&corpus.join("oracle.json"));
    let probe = probes["probes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|probe| probe["id"] == "valid-boundary")
        .unwrap();
    let event_ids = probe["event_ids"].as_array().unwrap();
    let early_id = event_ids[0].as_str().unwrap();
    let future_id = event_ids[1].as_str().unwrap();
    let valid_at = probe["valid_at"].as_str().unwrap();
    let events = events["events"].as_array().unwrap();
    let event = |event_id: &str| {
        events.iter().find(|event| event["id"] == event_id).unwrap()["t_ref"]
            .as_str()
            .unwrap()
    };
    assert_eq!(
        event(early_id),
        valid_at,
        "the boundary event must be included at the exact cutoff"
    );
    assert!(
        event(future_id) > valid_at,
        "the prohibited event must be later than the requested valid time"
    );
    let oracle = oracles["oracles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|oracle| oracle["probe_id"] == "valid-boundary")
        .unwrap();
    assert_eq!(oracle["disposition"], "answerable");
    let expected_assertion = oracle["expected_assertions"][early_id].as_str().unwrap();
    let early_content = events.iter().find(|event| event["id"] == early_id).unwrap()["content"]
        .as_str()
        .unwrap();
    assert!(
        early_content.contains(expected_assertion),
        "the positive oracle must occur in its registered source"
    );
    assert!(
        oracle["forbidden_event_ids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| { id.as_str() == Some(future_id) })
    );
}

#[test]
fn absent_subject_oracle_shares_a_head_noun_without_matching_the_stored_subject() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let corpus = root.join("evals/corpora/memory-decisions-v9");
    let events: Value = read(&corpus.join("events.json"));
    let probes: Value = read(&corpus.join("probes.json"));
    let oracles: Value = read(&corpus.join("oracle.json"));
    let probe = probes["probes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|probe| probe["id"] == "absent-subject")
        .unwrap();
    let query = probe["query"].as_str().unwrap();
    let query_subject = query.split_once(" for ").unwrap().1.trim_end_matches('?');
    let event_id = probe["event_ids"].as_array().unwrap()[0].as_str().unwrap();
    let event = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["id"] == event_id)
        .unwrap();
    let stored_subject = event["content"]
        .as_str()
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("subject: "))
        .unwrap();
    assert_ne!(query_subject, stored_subject);
    assert_eq!(
        query_subject.split_whitespace().last(),
        stored_subject.split_whitespace().last(),
        "this adversarial absent-subject case must keep its shared head noun"
    );
    let oracle = oracles["oracles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|oracle| oracle["probe_id"] == "absent-subject")
        .unwrap();
    assert_eq!(oracle["disposition"], "unknown");
    assert!(
        oracle["forbidden_event_ids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| { id.as_str() == Some(event_id) })
    );
}

fn corpus_sha256(root: &Path) -> String {
    let mut digest = Sha256::new();
    for filename in ["events.json", "probes.json", "oracle.json"] {
        let bytes = std::fs::read(root.join(filename)).expect("read frozen corpus file");
        digest.update(filename.as_bytes());
        digest.update([0]);
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }
    hex::encode(digest.finalize())
}

fn verify_frozen_rollout(root: &Path) -> DeterministicAcceptancePlan {
    let frozen = root.join("evals/paired/deterministic-quality-2026-11-22");
    let plan_path = frozen.join("claim-stage-v9-acceptance.json");
    let plan: DeterministicAcceptancePlan = read(&plan_path);
    let historical_preregistration: Value =
        read(&frozen.join("claim-stage-v9-preregistration.json"));
    let plan_sha256 = sha256(&std::fs::read(&plan_path).expect("read frozen plan bytes"));
    let corpus_hash = corpus_sha256(&root.join("evals/corpora/memory-decisions-v9"));
    let corpus: Value = read(&root.join("evals/corpora/memory-decisions-v9/probes.json"));
    let probes = corpus["probes"].as_array().expect("registered probe list");
    assert_eq!(
        plan.families.len(),
        probes.len(),
        "plan must cover every v9 case"
    );
    for probe in probes {
        let case_id = probe["id"].as_str().expect("registered case id");
        let family_id = probe["family_id"].as_str().expect("registered family id");
        assert_eq!(
            plan.families.get(case_id).map(String::as_str),
            Some(family_id),
            "plan must pin the registered family for {case_id}"
        );
    }
    assert!(
        plan.ordinary_cases.iter().all(|case_id| {
            probes
                .iter()
                .any(|probe| probe["id"].as_str() == Some(case_id))
        }),
        "ordinary cases must be registered corpus cases"
    );

    assert_eq!(
        historical_preregistration["status"], "frozen-before-v9-capture",
        "rollout cohort must be preregistered before any capture"
    );
    assert_eq!(historical_preregistration["corpus_sha256"], corpus_hash);
    assert_eq!(
        historical_preregistration["acceptance_plan_sha256"],
        plan_sha256
    );
    assert_eq!(plan.corpus_sha256, corpus_hash);
    assert_eq!(
        plan.score_channel,
        eval_harness::suites::AcceptanceScoreChannel::Primary
    );
    // The historical plan pins its archived binary. CI instead proves that
    // both new stage arms use the same binary built from the current checkout.
    rollout_plan_for_current_binary(plan)
}

fn run_profile(root: &Path, binary: &Path, output: &Path, name: &str, profile: &str) -> PathBuf {
    let receipt_root = output.join(name);
    std::fs::create_dir(&receipt_root).expect("fresh rollout receipt directory");
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_memory-eval"));
    cmd.env_clear()
        .current_dir(root)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("TMPDIR", std::env::temp_dir())
        .env("MEMORY_DECISIONS_SERVER_BIN", binary)
        .env("MEMORY_DECISIONS_PRODUCT_REVISION", "ci-current")
        .env("MEMORY_DECISIONS_ARTIFACT_ROOT", &receipt_root)
        .args(["run", "--profile", profile, "--artifact"])
        .arg(receipt_root.join("run.json"));
    cmd.stdout(std::fs::File::create(receipt_root.join("stdout.log")).unwrap())
        .stderr(std::fs::File::create(receipt_root.join("stderr.log")).unwrap());
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = ChildGuard(cmd.spawn().expect("start deterministic rollout profile"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(900);
    let status = loop {
        if let Some(status) = child.0.try_wait().expect("poll profile child") {
            break status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "bounded rollout evaluation exceeded 900 seconds"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    };
    assert!(
        matches!(status.code(), Some(0 | 1)),
        "see {}",
        receipt_root.join("stderr.log").display()
    );
    receipt_root
}

#[test]
#[ignore = "requires MEMORY_DECISIONS_SERVER_BIN; CI explicitly executes this real-product gate"]
fn candidate_meets_frozen_deterministic_memory_quality_gate() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let binary = PathBuf::from(
        std::env::var_os("MEMORY_DECISIONS_SERVER_BIN").expect("compiled product executable"),
    );
    assert!(binary.is_absolute() && binary.is_file());
    let scratch = tempfile::tempdir().unwrap();
    let output = std::env::var_os("MEMORY_QUALITY_GATE_OUTPUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| scratch.path().join("quality"));
    std::fs::create_dir(&output).expect("fresh gate output directory");
    let candidate = run_profile(
        &root,
        &binary,
        &output,
        "current",
        "evals/profiles/memory_decisions_current.json",
    );
    let corpus = root.join("evals/corpora/memory-decisions-v9");
    let index_path = candidate.join("index.json");
    let replay =
        load_registered_replay(&index_path, &corpus).expect("complete registered current capture");
    assert_eq!(
        serde_json::to_value(&replay).unwrap()["identity"]["product_binary_sha256"],
        sha256(&std::fs::read(&binary).unwrap()),
        "capture must use the invoked binary"
    );
    let baseline =
        root.join("evals/paired/harness-simplification-baseline-2026-11-26/receipts/index.json");
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_memory-eval"));
    command
        .current_dir(&root)
        .args(["compare-current-memory", "--baseline"])
        .arg(baseline)
        .arg("--candidate")
        .arg(index_path)
        .arg("--corpus")
        .arg(corpus)
        .arg("--output")
        .arg(output.join("current-regression.json"));
    let status = command.status().expect("offline current regression grade");
    let report: Value = read(&output.join("current-regression.json"));
    assert!(
        status.success(),
        "registered regression gate rejected capture: {report}"
    );
    assert_eq!(report["counts"]["invalid"], 0);
    assert_eq!(report["counts"]["unobserved"], 0);
    assert_eq!(report["cases"].as_array().unwrap().len(), 8);
    assert_eq!(report["no_regressions"], true);
}
