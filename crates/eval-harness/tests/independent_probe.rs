#[cfg(unix)]
use eval_harness::independent_probe::capture_stdio_probe;
use eval_harness::independent_probe::{ProbeInput, ProbeObservation, ProbeOracle, score_probe};

fn input() -> ProbeInput {
    ProbeInput {
        schema_version: "memory-mcp-independent-probe/v1".into(),
        query: "Where is the depot?".into(),
        principal: "fixture-reader".into(),
        valid_at: "2026-05-01T10:00:00Z".parse().unwrap(),
        known_revision: 3,
        context_budget: 5,
        history_sha256: "a".repeat(64),
        claim_rollout_stage: "shadow".into(),
    }
}
fn oracle() -> ProbeOracle {
    ProbeOracle {
        sufficient_evidence_alternatives: vec![vec![
            vec!["source-a".into()],
            vec!["source-b".into()],
        ]],
        forbidden_event_ids: vec!["source-stale".into()],
        required_claim_annotations: vec![],
        required_claim_annotation_alternatives: vec![],
    }
}
#[test]
fn missing_independent_observation_is_invalid_instead_of_success() {
    assert!(score_probe(&input(), &oracle(), None, &"b".repeat(64)).is_err());
}

fn observation() -> ProbeObservation {
    ProbeObservation {
        input_sha256: input().sha256().unwrap(),
        product_sha256: "b".repeat(64),
        source_event_ids: vec!["source-a".into(), "source-b".into()],
        claim_annotations: vec![],
        budget_used: 2,
        completed: true,
    }
}
#[test]
fn one_side_of_a_required_evidence_set_fails() {
    let mut observed = observation();
    observed.source_event_ids.pop();
    assert!(
        !score_probe(&input(), &oracle(), Some(&observed), &"b".repeat(64))
            .unwrap()
            .evidence_success
    );
}
#[test]
fn every_required_group_supports_success() {
    assert!(
        score_probe(&input(), &oracle(), Some(&observation()), &"b".repeat(64))
            .unwrap()
            .evidence_success
    );
}

#[test]
fn required_typed_conflict_annotation_counts_when_only_one_source_fits_the_budget() {
    let oracle: ProbeOracle = serde_json::from_value(serde_json::json!({
        "sufficient_evidence_alternatives": [[["source-a"]]],
        "forbidden_event_ids": [],
        "required_claim_annotations": [{
            "source_event_id": "source-a",
            "evidence_status": "conflicting",
            "relations": [{"outcome": "contradiction", "role": "conflict"}]
        }]
    }))
    .unwrap();
    let observed: ProbeObservation = serde_json::from_value(serde_json::json!({
        "input_sha256": input().sha256().unwrap(),
        "product_sha256": "b".repeat(64),
        "source_event_ids": ["source-a"],
        "budget_used": 1,
        "completed": true,
        "claim_annotations": [{
            "source_event_id": "source-a",
            "evidence_status": "conflicting",
            "relations": [{"outcome": "contradiction", "role": "conflict"}]
        }]
    }))
    .unwrap();

    assert!(
        score_probe(&input(), &oracle, Some(&observed), &"b".repeat(64))
            .unwrap()
            .evidence_success
    );
}

#[test]
fn missing_typed_conflict_annotation_fails_even_when_source_coverage_passes() {
    let oracle: ProbeOracle = serde_json::from_value(serde_json::json!({
        "sufficient_evidence_alternatives": [[["source-a"]]],
        "forbidden_event_ids": [],
        "required_claim_annotations": [{
            "source_event_id": "source-a",
            "evidence_status": "conflicting",
            "relations": [{"outcome": "contradiction", "role": "conflict"}]
        }]
    }))
    .unwrap();
    let mut observed = observation();
    observed.source_event_ids = vec!["source-a".into()];
    observed.budget_used = 1;

    assert!(
        !score_probe(&input(), &oracle, Some(&observed), &"b".repeat(64))
            .unwrap()
            .evidence_success
    );
}

#[test]
fn one_returned_source_can_match_any_preregistered_conflict_annotation_alternative() {
    let oracle: ProbeOracle = serde_json::from_value(serde_json::json!({
        "sufficient_evidence_alternatives": [[ ["source-a", "source-b"] ]],
        "forbidden_event_ids": [],
        "required_claim_annotation_alternatives": [
            [{
                "source_event_id": "source-a",
                "evidence_status": "conflicting",
                "relations": [{"outcome": "contradiction", "role": "conflict"}]
            }],
            [{
                "source_event_id": "source-b",
                "evidence_status": "conflicting",
                "relations": [{"outcome": "contradiction", "role": "conflict"}]
            }]
        ]
    }))
    .unwrap();
    let observed: ProbeObservation = serde_json::from_value(serde_json::json!({
        "input_sha256": input().sha256().unwrap(),
        "product_sha256": "b".repeat(64),
        "source_event_ids": ["source-b"],
        "budget_used": 1,
        "completed": true,
        "claim_annotations": [{
            "source_event_id": "source-b",
            "evidence_status": "conflicting",
            "relations": [{"outcome": "contradiction", "role": "conflict"}]
        }]
    }))
    .unwrap();

    assert!(
        score_probe(&input(), &oracle, Some(&observed), &"b".repeat(64))
            .unwrap()
            .evidence_success
    );
}

#[test]
fn forbidden_source_defeats_otherwise_complete_evidence() {
    let mut observed = observation();
    observed.source_event_ids.push("source-stale".into());
    observed.budget_used = 3;
    assert!(
        !score_probe(&input(), &oracle(), Some(&observed), &"b".repeat(64))
            .unwrap()
            .evidence_success
    );
}

#[test]
fn observation_from_another_principal_is_invalid() {
    let mut requested = input();
    requested.principal = "other-reader".into();
    assert!(score_probe(&requested, &oracle(), Some(&observation()), &"b".repeat(64)).is_err());
}
#[test]
fn observation_from_another_product_is_invalid() {
    assert!(score_probe(&input(), &oracle(), Some(&observation()), &"c".repeat(64)).is_err());
}

#[test]
fn over_budget_evidence_fails() {
    let mut observed = observation();
    observed.budget_used = 6;
    assert!(
        !score_probe(&input(), &oracle(), Some(&observed), &"b".repeat(64))
            .unwrap()
            .evidence_success
    );
}
#[test]
fn failed_probe_cannot_score_success() {
    let mut observed = observation();
    observed.completed = false;
    assert!(
        !score_probe(&input(), &oracle(), Some(&observed), &"b".repeat(64))
            .unwrap()
            .evidence_success
    );
}

#[test]
fn offline_cli_scores_fixed_probe_without_an_agent_account() {
    let directory = tempfile::tempdir().unwrap();
    let input_path = directory.path().join("input.json");
    let oracle_path = directory.path().join("oracle.json");
    let observed_path = directory.path().join("observed.json");
    let output_path = directory.path().join("score.json");
    std::fs::write(&input_path, serde_json::to_vec(&input()).unwrap()).unwrap();
    std::fs::write(&oracle_path, serde_json::to_vec(&oracle()).unwrap()).unwrap();
    std::fs::write(&observed_path, serde_json::to_vec(&observation()).unwrap()).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .env_clear()
        .arg("score-independent-probe")
        .arg("--input")
        .arg(input_path)
        .arg("--oracle")
        .arg(oracle_path)
        .arg("--observation")
        .arg(observed_path)
        .arg("--product-sha256")
        .arg("b".repeat(64))
        .arg("--output")
        .arg(&output_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let score: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output_path).unwrap()).unwrap();
    assert_eq!(score["evidence_success"], true);
}

#[test]
fn unknown_probe_schema_is_rejected() {
    let mut requested = input();
    requested.schema_version = "future-schema".into();
    let mut observed = observation();
    observed.input_sha256 = requested.sha256().unwrap();
    assert!(score_probe(&requested, &oracle(), Some(&observed), &"b".repeat(64)).is_err());
}
#[test]
fn mutually_impossible_oracle_is_invalid() {
    let mut expected = oracle();
    expected.forbidden_event_ids.push("source-a".into());
    assert!(score_probe(&input(), &expected, Some(&observation()), &"b".repeat(64)).is_err());
}

#[test]
fn score_receipt_binds_query_and_private_oracle() {
    let score = score_probe(&input(), &oracle(), Some(&observation()), &"b".repeat(64)).unwrap();
    let encoded = serde_json::to_value(score).unwrap();
    assert_eq!(encoded["input_sha256"], input().sha256().unwrap());
    assert_eq!(encoded["product_sha256"], "b".repeat(64));
    let mut revised = oracle();
    revised
        .forbidden_event_ids
        .push("other-stale-source".into());
    let other = serde_json::to_value(
        score_probe(&input(), &revised, Some(&observation()), &"b".repeat(64)).unwrap(),
    )
    .unwrap();
    assert_ne!(encoded["oracle_sha256"], other["oracle_sha256"]);
}

#[test]
fn malformed_product_fingerprint_is_invalid_even_when_it_matches() {
    let mut observed = observation();
    observed.product_sha256 = "not-a-hash".into();
    assert!(score_probe(&input(), &oracle(), Some(&observed), "not-a-hash").is_err());
}

#[test]
fn observation_with_more_sources_than_returned_items_is_invalid() {
    let mut observed = observation();
    observed.budget_used = 1;
    assert!(score_probe(&input(), &oracle(), Some(&observed), &"b".repeat(64)).is_err());
}

#[cfg(unix)]
fn stdio_fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    Vec<eval_harness::paired_agent::SeedEpisode>,
    ProbeInput,
    String,
) {
    use eval_harness::paired_agent::SeedEpisode;
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let binary = directory.path().join("memory-fixture");
    std::fs::write(&binary, r#"#!/bin/sh
n=0
while IFS= read -r line; do
 id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
 [ -n "$id" ] || continue
 case "$line" in
  *'"method":"initialize"'*) result='{}' ;;
  *'"method":"tools/list"'*) result='{"tools":[{"name":"ingest","inputSchema":{}},{"name":"extract","inputSchema":{}},{"name":"assemble_context","inputSchema":{"type":"object","properties":{"query":{"type":"string"},"as_of":{"type":"string","deprecated":true},"valid_at":{"type":"string"},"known_revision":{"type":"integer"},"budget":{"type":"integer"},"compact":{"type":"boolean"}}}}]}' ;;
  *'"name":"ingest"'*) n=$((n+1)); result="{\"structuredContent\":{\"result\":\"episode:$n\"}}" ;;
  *'"name":"extract"'*) result='{"structuredContent":{"result":{}}}' ;;
  *'"name":"assemble_context"'*) result='{"structuredContent":{"result":[{"source_episode":"episode:1"},{"source_episode":"episode:2"}],"context":{"snapshot":{"valid_at":"2026-05-01T10:00:00Z","known_revision":3}}}}' ;;
  *) exit 2 ;;
 esac
 printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$id" "$result"
done
"#).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let history = vec![
        SeedEpisode {
            event_id: "source-a".into(),
            source_id: "a".into(),
            source_type: "conversation".into(),
            content: "The depot is in Oslo.".into(),
            valid_at: input().valid_at,
        },
        SeedEpisode {
            event_id: "source-b".into(),
            source_id: "b".into(),
            source_type: "conversation".into(),
            content: "The depot is in Bergen.".into(),
            valid_at: input().valid_at,
        },
    ];
    let mut requested = input();
    requested.history_sha256 = hex::encode(Sha256::digest(serde_json::to_vec(&history).unwrap()));
    let product = hex::encode(Sha256::digest(std::fs::read(&binary).unwrap()));
    (directory, binary, history, requested, product)
}

#[cfg(unix)]
#[test]
fn fixed_stdio_probe_captures_only_its_fresh_history_without_an_agent() {
    let (directory, binary, history, requested, product) = stdio_fixture();
    let store = directory.path().join("fresh-probe-store");
    let capture =
        capture_stdio_probe(&requested, &history, &binary, &product, &store, "shadow").unwrap();
    assert_eq!(
        capture.observation.source_event_ids,
        ["source-a", "source-b"]
    );
    assert_eq!(capture.request["query"], "Where is the depot?");
    assert_eq!(capture.request["known_revision"], 3);
    assert_eq!(capture.observation.budget_used, 2);
    assert!(
        score_probe(&requested, &oracle(), Some(&capture.observation), &product)
            .unwrap()
            .evidence_success
    );
    assert!(
        capture_stdio_probe(&requested, &history, &binary, &product, &store, "shadow").is_err(),
        "a second probe cannot reuse a store primed by a previous read"
    );
}

#[cfg(unix)]
#[test]
fn fixed_stdio_probe_scores_typed_conflict_under_a_one_item_budget() {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;

    let (directory, binary, history, mut requested, _) = stdio_fixture();
    std::fs::write(
        &binary,
        r#"#!/bin/sh
n=0
while IFS= read -r line; do
 id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
 [ -n "$id" ] || continue
 case "$line" in
  *'"method":"initialize"'*) result='{}' ;;
  *'"method":"tools/list"'*) result='{"tools":[{"name":"ingest","inputSchema":{}},{"name":"extract","inputSchema":{}},{"name":"assemble_context","inputSchema":{"type":"object","properties":{"query":{"type":"string"},"as_of":{"type":"string","deprecated":true},"valid_at":{"type":"string"},"known_revision":{"type":"integer"},"budget":{"type":"integer"},"compact":{"type":"boolean"}}}}]}' ;;
  *'"name":"ingest"'*) n=$((n+1)); result="{\"structuredContent\":{\"result\":\"episode:$n\"}}" ;;
  *'"name":"extract"'*) result='{"structuredContent":{"result":{}}}' ;;
  *'"name":"assemble_context"'*)
    case "$MEMORY_CLAIM_ROLLOUT_STAGE" in
      evidence) result='{"structuredContent":{"result":[{"source_episode":"episode:1","evidence_status":"conflicting","reconciliation":{"relations":[{"outcome":"contradiction","role":"conflict"}]}}],"context":{"snapshot":{"valid_at":"2026-05-01T10:00:00Z","known_revision":3}}}}' ;;
      shadow) result='{"structuredContent":{"result":[{"source_episode":"episode:1","evidence_status":"unknown"}],"context":{"snapshot":{"valid_at":"2026-05-01T10:00:00Z","known_revision":3}}}}' ;;
      *) exit 2 ;;
    esac ;;
  *) exit 2 ;;
 esac
 printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$id" "$result"
done
"#,
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let product = hex::encode(Sha256::digest(std::fs::read(&binary).unwrap()));
    requested.context_budget = 1;
    requested.claim_rollout_stage = "evidence".into();
    let oracle = ProbeOracle {
        sufficient_evidence_alternatives: vec![vec![vec!["source-a".into()]]],
        forbidden_event_ids: vec![],
        required_claim_annotations: vec![
            serde_json::from_value(serde_json::json!({
                "source_event_id": "source-a",
                "evidence_status": "conflicting",
                "relations": [{"outcome": "contradiction", "role": "conflict"}]
            }))
            .unwrap(),
        ],
        required_claim_annotation_alternatives: vec![],
    };
    let evidence = capture_stdio_probe(
        &requested,
        &history,
        &binary,
        &product,
        &directory.path().join("evidence-store"),
        "evidence",
    )
    .unwrap();
    assert_eq!(
        evidence.schema_version,
        "memory-mcp-independent-probe-capture/v2"
    );
    assert_eq!(evidence.observation.budget_used, 1);
    assert!(
        score_probe(&requested, &oracle, Some(&evidence.observation), &product)
            .unwrap()
            .evidence_success
    );

    requested.claim_rollout_stage = "shadow".into();
    let shadow = capture_stdio_probe(
        &requested,
        &history,
        &binary,
        &product,
        &directory.path().join("shadow-store"),
        "shadow",
    )
    .unwrap();
    assert!(
        !score_probe(&requested, &oracle, Some(&shadow.observation), &product)
            .unwrap()
            .evidence_success
    );

    let mut legacy = shadow.clone();
    legacy.schema_version = "memory-mcp-independent-probe-capture/v1".into();
    legacy.observation.claim_annotations.clear();
    legacy
        .validate(
            &requested,
            &history,
            &product,
            "stdio",
            &legacy.tool_schema_sha256,
        )
        .unwrap();
}

#[cfg(unix)]
#[test]
fn fixed_stdio_probe_supports_the_registered_legacy_as_of_contract() {
    use eval_harness::paired_agent::SeedEpisode;
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let binary = directory.path().join("legacy-memory-fixture");
    std::fs::write(
        &binary,
        r##"#!/bin/sh
n=0
while IFS= read -r line; do
 id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
 [ -n "$id" ] || continue
 case "$line" in
  *'"method":"initialize"'*) result='{}' ;;
  *'"method":"tools/list"'*) result='{"tools":[{"name":"ingest","inputSchema":{"type":"object","properties":{"content":{"type":"string"}}}},{"name":"extract","inputSchema":{"type":"object","properties":{"episode_id":{"type":"string"}}}},{"name":"assemble_context","inputSchema":{"type":"object","properties":{"query":{"type":"string"},"as_of":{"type":"string"},"budget":{"type":"integer"},"compact":{"type":"boolean"}}}}]}' ;;
  *'"name":"ingest"'*) n=$((n+1)); result="{\"structuredContent\":{\"result\":\"episode:$n\"}}" ;;
  *'"name":"extract"'*) result='{"structuredContent":{"result":{}}}' ;;
  *as_of*) result='{"structuredContent":{"result":[{"source_episode":"episode:1"}],"status":"success"}}' ;;
  *'"name":"assemble_context"'*) result='{"isError":true,"structuredContent":{"status":"error"}}' ;;
  *) exit 2 ;;
 esac
 printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$id" "$result"
done
"##,
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();

    let history = vec![SeedEpisode {
        event_id: "source-a".into(),
        source_id: "fixture".into(),
        source_type: "doc".into(),
        content: "Fact: Cedar Observatory ledger is in Cabinet-28.".into(),
        valid_at: input().valid_at,
    }];
    let mut requested = input();
    requested.known_revision = 0;
    requested.history_sha256 = hex::encode(Sha256::digest(serde_json::to_vec(&history).unwrap()));
    let product = hex::encode(Sha256::digest(std::fs::read(&binary).unwrap()));
    let capture = capture_stdio_probe(
        &requested,
        &history,
        &binary,
        &product,
        &directory.path().join("legacy-probe-store"),
        "shadow",
    )
    .expect("registered legacy products use as_of without claiming a revision snapshot");

    assert_eq!(capture.request["as_of"], requested.valid_at.to_rfc3339());
    assert!(capture.request.get("valid_at").is_none());
    assert!(capture.request.get("known_revision").is_none());
    assert_eq!(capture.observation.source_event_ids, ["source-a"]);
    assert_eq!(
        capture.api_profile,
        eval_harness::independent_probe::ProbeApiProfile::LegacyAsOfV0
    );
    capture
        .validate(
            &requested,
            &history,
            &product,
            "stdio",
            &capture.tool_schema_sha256,
        )
        .unwrap();
    let legacy_oracle = ProbeOracle {
        sufficient_evidence_alternatives: vec![vec![vec!["source-a".into()]]],
        forbidden_event_ids: vec![],
        required_claim_annotations: vec![],
        required_claim_annotation_alternatives: vec![],
    };
    assert!(
        score_probe(
            &requested,
            &legacy_oracle,
            Some(&capture.observation),
            &product
        )
        .unwrap()
        .evidence_success
    );

    requested.known_revision = 1;
    assert!(
        capture_stdio_probe(
            &requested,
            &history,
            &binary,
            &product,
            &directory.path().join("revision-bound-probe-store"),
            "shadow",
        )
        .is_err()
    );
}

#[cfg(unix)]
#[test]
fn fixed_probe_rejects_a_response_from_another_snapshot() {
    let (directory, binary, history, mut requested, product) = stdio_fixture();
    requested.known_revision = 4;
    let store = directory.path().join("fresh-probe-store");
    assert!(
        capture_stdio_probe(&requested, &history, &binary, &product, &store, "shadow").is_err()
    );
}

#[cfg(unix)]
#[test]
fn local_fixture_cannot_claim_to_execute_as_an_arbitrary_principal() {
    let (directory, binary, history, mut requested, product) = stdio_fixture();
    requested.principal = "another-tenant".into();
    let store = directory.path().join("fresh-probe-store");
    assert!(
        capture_stdio_probe(&requested, &history, &binary, &product, &store, "shadow").is_err()
    );
    assert!(
        !store.exists(),
        "unsupported identity must fail before formation"
    );
}

#[cfg(unix)]
#[test]
fn fixed_probe_cli_captures_public_stdio_without_agent_credentials() {
    let (directory, binary, history, requested, product) = stdio_fixture();
    let input_path = directory.path().join("input.json");
    let history_path = directory.path().join("history.json");
    let output_path = directory.path().join("capture.json");
    std::fs::write(&input_path, serde_json::to_vec(&requested).unwrap()).unwrap();
    std::fs::write(&history_path, serde_json::to_vec(&history).unwrap()).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .env_clear()
        .arg("capture-independent-probe")
        .arg("--transport")
        .arg("stdio")
        .arg("--input")
        .arg(input_path)
        .arg("--history")
        .arg(history_path)
        .arg("--server-binary")
        .arg(binary)
        .arg("--product-sha256")
        .arg(product)
        .arg("--store")
        .arg(directory.path().join("store"))
        .arg("--claim-rollout-stage")
        .arg("shadow")
        .arg("--output")
        .arg(&output_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let capture: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output_path).unwrap()).unwrap();
    assert_eq!(
        capture["observation"]["source_event_ids"],
        serde_json::json!(["source-a", "source-b"])
    );
}

#[cfg(unix)]
#[test]
fn fixed_probe_requires_the_preregistered_retrieval_stage() {
    let (directory, binary, history, requested, product) = stdio_fixture();
    let store = directory.path().join("fresh-probe-store");
    assert!(
        capture_stdio_probe(&requested, &history, &binary, &product, &store, "evidence").is_err()
    );
    assert!(!store.exists(), "stage mismatch must fail before formation");
}

#[test]
fn negative_knowledge_revision_cannot_be_a_valid_probe() {
    let mut requested = input();
    requested.known_revision = -1;
    let mut observed = observation();
    observed.input_sha256 = requested.sha256().unwrap();
    assert!(score_probe(&requested, &oracle(), Some(&observed), &"b".repeat(64)).is_err());
}

#[test]
fn uncited_returned_items_cannot_pass_an_empty_evidence_oracle() {
    let expected = ProbeOracle {
        sufficient_evidence_alternatives: vec![],
        forbidden_event_ids: vec![],
        required_claim_annotations: vec![],
        required_claim_annotation_alternatives: vec![],
    };
    let mut observed = observation();
    observed.source_event_ids.clear();
    observed.budget_used = 1;
    assert!(
        !score_probe(&input(), &expected, Some(&observed), &"b".repeat(64))
            .unwrap()
            .evidence_success
    );
}

#[test]
fn score_receipt_binds_the_observation_that_was_scored() {
    let first = serde_json::to_value(
        score_probe(&input(), &oracle(), Some(&observation()), &"b".repeat(64)).unwrap(),
    )
    .unwrap();
    let mut changed = observation();
    changed.completed = false;
    let second = serde_json::to_value(
        score_probe(&input(), &oracle(), Some(&changed), &"b".repeat(64)).unwrap(),
    )
    .unwrap();
    assert_ne!(first["observation_sha256"], second["observation_sha256"]);
}

#[tokio::test]
#[ignore = "requires MEMORY_MCP_HTTP_PROBE_BINARY built with streamable-http,test-fixtures"]
async fn independent_http_probe_uses_a_fresh_tenant_and_the_fixed_snapshot() {
    use eval_harness::independent_probe::capture_http_probe;
    use eval_harness::paired_agent::SeedEpisode;
    use sha2::{Digest, Sha256};
    let binary = std::path::PathBuf::from(
        std::env::var_os("MEMORY_MCP_HTTP_PROBE_BINARY").expect("HTTP fixture binary"),
    );
    let directory = tempfile::tempdir().unwrap();
    let history = vec![SeedEpisode {
        event_id: "source-a".into(),
        source_id: "a".into(),
        source_type: "conversation".into(),
        content: "North Depot is located in Oslo.".into(),
        valid_at: input().valid_at,
    }];
    let mut requested = input();
    requested.known_revision = 0;
    requested.history_sha256 = hex::encode(Sha256::digest(serde_json::to_vec(&history).unwrap()));
    let product = hex::encode(Sha256::digest(std::fs::read(&binary).unwrap()));
    let capture = capture_http_probe(
        &requested,
        &history,
        &binary,
        &product,
        &directory.path().join("store"),
        "shadow",
    )
    .await
    .unwrap();
    assert!(capture.observation.completed);
    assert_eq!(capture.observation.budget_used, 0);
    assert!(
        capture.observation.source_event_ids.is_empty(),
        "revision zero precedes the fixture history"
    );
    assert_eq!(capture.request["known_revision"], 0);
}

#[cfg(unix)]
#[test]
fn duplicate_event_ids_cannot_form_an_independent_history() {
    use sha2::{Digest, Sha256};
    let (directory, binary, mut history, mut requested, product) = stdio_fixture();
    history[1].event_id = "source-a".into();
    requested.history_sha256 = hex::encode(Sha256::digest(serde_json::to_vec(&history).unwrap()));
    let store = directory.path().join("store");
    assert!(
        capture_stdio_probe(&requested, &history, &binary, &product, &store, "shadow").is_err()
    );
    assert!(
        !store.exists(),
        "ambiguous event identities must fail before formation"
    );
}

#[cfg(unix)]
#[test]
fn capture_records_the_public_configuration_passed_to_the_child() {
    let (directory, binary, history, requested, product) = stdio_fixture();
    let capture = capture_stdio_probe(
        &requested,
        &history,
        &binary,
        &product,
        &directory.path().join("store"),
        "shadow",
    )
    .unwrap();
    let encoded = serde_json::to_value(capture).unwrap();
    assert_eq!(encoded["launch_profile"]["NER_EXTRACTOR"], "regex");
    assert_eq!(encoded["launch_profile"]["EMBEDDINGS_ENABLED"], "false");
    assert_eq!(
        encoded["launch_profile"]["MEMORY_CLAIM_ROLLOUT_STAGE"],
        "shadow"
    );
    assert_eq!(
        encoded["launch_profile"]["SURREALDB_DB_NAME"],
        "memory_decisions"
    );
    assert!(
        encoded["launch_profile"]
            .get("SURREALDB_PASSWORD")
            .is_none()
    );
}

#[cfg(unix)]
#[test]
fn capture_preserves_the_binding_from_public_episode_ids_to_fixed_events() {
    let (directory, binary, history, requested, product) = stdio_fixture();
    let capture = capture_stdio_probe(
        &requested,
        &history,
        &binary,
        &product,
        &directory.path().join("store"),
        "shadow",
    )
    .unwrap();
    let encoded = serde_json::to_value(capture).unwrap();
    assert_eq!(encoded["episode_id_to_event_id"]["episode:1"], "source-a");
    assert_eq!(encoded["episode_id_to_event_id"]["episode:2"], "source-b");
}
