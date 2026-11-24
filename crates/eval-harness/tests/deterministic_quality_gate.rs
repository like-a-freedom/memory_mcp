//! Native CI gate: public MCP replay, two distinct captures and frozen acceptance.
use eval_harness::suites::{
    DeterministicAcceptancePlan, accept_candidate, compare_replays, load_registered_replay,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    serde_json::from_slice(&std::fs::read(path).expect("read gate artifact"))
        .expect("parse gate artifact")
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[test]
fn ci_rollout_plan_keeps_frozen_rules_and_pairs_the_current_binary() {
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
fn current_rollout_gate_validates_frozen_corpus_and_acceptance_rules() {
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

fn primary_scores(receipt_root: &Path) -> BTreeMap<String, (bool, bool)> {
    let index: Value = read(&receipt_root.join("index.json"));
    index["cases"]
        .as_array()
        .expect("case list")
        .iter()
        .map(|case| {
            let id = case["case_id"].as_str().expect("case id").to_string();
            let receipt: Value =
                read(&receipt_root.join(case["receipt_path"].as_str().expect("receipt path")));
            (
                id,
                (
                    receipt["score"]["evidence_success"]
                        .as_bool()
                        .expect("primary evidence score"),
                    receipt["score"]["probe_success"]
                        .as_bool()
                        .expect("primary decision score"),
                ),
            )
        })
        .collect()
}

fn primary_rollout_stages(receipt_root: &Path) -> Vec<String> {
    let index: Value = read(&receipt_root.join("index.json"));
    index["cases"]
        .as_array()
        .expect("case list")
        .iter()
        .map(|case| {
            let receipt: Value =
                read(&receipt_root.join(case["receipt_path"].as_str().expect("receipt path")));
            receipt["primary_claim_rollout_stage"]
                .as_str()
                .expect("primary rollout stage")
                .to_string()
        })
        .collect()
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
    let rollout_plan = verify_frozen_rollout(&root);
    let scratch = tempfile::tempdir().unwrap();
    let output = std::env::var_os("MEMORY_QUALITY_GATE_OUTPUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| scratch.path().join("quality"));
    std::fs::create_dir(&output).expect("fresh gate output directory");
    // The historical v4 fixture predates the reviewed additive
    // `extract.include_fact_evidence` schema change and is intentionally pinned
    // to the old public-schema fingerprint. `accept_candidate` must reject
    // those cross-schema artifacts. Legacy request/compact-response behavior
    // is covered at the public extract boundary; memory quality is gated below
    // by current-schema, same-binary v9 default/shadow captures.
    let default_first = run_profile(
        &root,
        &binary,
        &output,
        "rollout-v9-default-first",
        "evals/profiles/memory_decisions_v9_default.json",
    );
    let default_second = run_profile(
        &root,
        &binary,
        &output,
        "rollout-v9-default-second",
        "evals/profiles/memory_decisions_v9_default.json",
    );
    let shadow_first = run_profile(
        &root,
        &binary,
        &output,
        "rollout-v9-shadow-first",
        "evals/profiles/memory_decisions_v9_shadow.json",
    );
    let shadow_second = run_profile(
        &root,
        &binary,
        &output,
        "rollout-v9-shadow-second",
        "evals/profiles/memory_decisions_v9_shadow.json",
    );
    let corpus = root.join("evals/corpora/memory-decisions-v9");
    let default_a = load_registered_replay(&default_first.join("index.json"), &corpus).unwrap();
    let default_b = load_registered_replay(&default_second.join("index.json"), &corpus).unwrap();
    let shadow_a = load_registered_replay(&shadow_first.join("index.json"), &corpus).unwrap();
    let shadow_b = load_registered_replay(&shadow_second.join("index.json"), &corpus).unwrap();
    compare_replays(&default_a, &default_b).expect("default captures must replay identically");
    compare_replays(&shadow_a, &shadow_b).expect("shadow controls must replay identically");

    let default_index: Value = read(&default_first.join("index.json"));
    let shadow_index: Value = read(&shadow_first.join("index.json"));
    for key in [
        "product_revision",
        "product_binary_sha256",
        "corpus_sha256",
        "input_schema_sha256",
    ] {
        assert_eq!(
            default_index["identity"][key], shadow_index["identity"][key],
            "default and shadow controls must use the same product and public contract: {key}"
        );
    }
    assert_ne!(
        default_index["identity"]["config_sha256"], shadow_index["identity"]["config_sha256"],
        "the rollout stages must have distinct configuration fingerprints"
    );

    let default_scores = primary_scores(&default_first);
    let shadow_scores = primary_scores(&shadow_first);
    assert!(
        primary_rollout_stages(&default_first)
            .iter()
            .all(|stage| stage == "default"),
        "default receipts must name the product-default stage"
    );
    assert!(
        primary_rollout_stages(&shadow_first)
            .iter()
            .all(|stage| stage == "shadow"),
        "rollback receipts must name the explicit shadow stage"
    );
    assert_eq!(default_scores.len(), 8, "all default cases must be scored");
    assert_eq!(shadow_scores.len(), 8, "all rollback cases must be scored");
    let default_evidence = default_scores
        .values()
        .filter(|(evidence, _)| *evidence)
        .count();
    let default_decisions = default_scores
        .values()
        .filter(|(_, decision)| *decision)
        .count();
    let shadow_evidence = shadow_scores
        .values()
        .filter(|(evidence, _)| *evidence)
        .count();
    let shadow_decisions = shadow_scores
        .values()
        .filter(|(_, decision)| *decision)
        .count();
    let acceptance = accept_candidate(
        &rollout_plan,
        [&shadow_a, &shadow_b],
        [&default_a, &default_b],
    )
    .expect("frozen primary-channel rollout plan accepts compatible repeated captures");
    let gains = default_scores
        .iter()
        .filter(|(case, (_, passed))| {
            *passed && shadow_scores.get(*case).is_some_and(|(_, old)| !old)
        })
        .map(|(case, _)| case.clone())
        .collect::<Vec<_>>();
    let losses = shadow_scores
        .iter()
        .filter(|(case, (_, passed))| {
            *passed && default_scores.get(*case).is_some_and(|(_, new)| !new)
        })
        .map(|(case, _)| case.clone())
        .collect::<Vec<_>>();
    let evidence_regressions = shadow_scores
        .iter()
        .filter(|(case, (evidence, _))| {
            *evidence && default_scores.get(*case).is_some_and(|(new, _)| !new)
        })
        .map(|(case, _)| case.clone())
        .collect::<Vec<_>>();
    let accepted = acceptance.accepted;
    let report = serde_json::json!({
        "schema_version": "memory-mcp-claim-rollout-acceptance/v1",
        "product_revision": default_index["identity"]["product_revision"],
        "product_binary_sha256": default_index["identity"]["product_binary_sha256"],
        "corpus_sha256": default_index["identity"]["corpus_sha256"],
        "input_schema_sha256": default_index["identity"]["input_schema_sha256"],
        "default_config_sha256": default_index["identity"]["config_sha256"],
        "shadow_config_sha256": shadow_index["identity"]["config_sha256"],
        "default_replays": ["rollout-v9-default-first/index.json", "rollout-v9-default-second/index.json"],
        "shadow_replays": ["rollout-v9-shadow-first/index.json", "rollout-v9-shadow-second/index.json"],
        "default_metrics": {"evidence_success": default_evidence, "decision_success": default_decisions, "cases": default_scores.len()},
        "shadow_metrics": {"evidence_success": shadow_evidence, "decision_success": shadow_decisions, "cases": shadow_scores.len()},
        "decision_gains": gains,
        "decision_losses": losses,
        "evidence_regressions": evidence_regressions,
        "acceptance": acceptance,
        "repeat_captures_match": true,
        "accepted": accepted
    });
    std::fs::write(
        output.join("claim-rollout-default-acceptance.json"),
        serde_json::to_vec_pretty(&report).expect("serialize rollout report"),
    )
    .expect("write rollout report");
    assert!(accepted, "default rollout rejected: {report}");
}
