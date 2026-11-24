use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use eval_harness::codex_agent::{
    CodexMemoryToolProfile, agent_managed_retry_policy_sha256, reader_config_sha256_for_profile,
    system_instructions_sha256_for_profile, task_rules_sha256_for_profile,
};
use eval_harness::paired::{
    BootstrapConfig, CaseTrialKey, ModelIdentityProtocol, PairedEvaluationPlan,
    PairedEvaluationStage, ProductRunIdentity, RunArm, SharedRunIdentity,
};
use eval_harness::paired_agent::codex::{
    CodexArmRuntime, CodexPairedBackend, PairedAgentTransport,
};
use eval_harness::paired_agent::managed::{
    AgentManagedOperatorFileImport, AgentManagedProbeRequest, AgentManagedTaskInput,
    run_agent_managed_trial,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::tempdir;

const EPISODE_ID: &str = "episode:source-cedar";
const EVENT_ID: &str = "source-authority-source-cedar";
const VALID_AT: &str = "2026-11-23T00:00:00Z";

#[test]
fn incumbent_receives_literal_source_content_and_candidate_import_is_isolated_from_agent_workspaces()
 {
    let leaked_input = Path::new(env!("CARGO_MANIFEST_DIR")).join("codex-input.txt");
    assert!(
        !leaked_input.exists(),
        "Codex fixture must not leave prompt files in the crate"
    );

    let root = tempdir().expect("temporary integration-test root");
    let memory_binary = root.path().join("memory-mcp-fixture.sh");
    let codex_binary = root.path().join("codex-fixture.sh");
    write_executable(&memory_binary, memory_server_script());
    write_executable(&codex_binary, codex_script());

    let codex_home = root.path().join("codex-home");
    let incumbent_temp = root.path().join("incumbent-temp");
    let candidate_temp = root.path().join("candidate-temp");
    for directory in [&codex_home, &incumbent_temp, &candidate_temp] {
        fs::create_dir(directory).expect("create isolated runtime directory");
    }

    let profile = CodexMemoryToolProfile::AgentSourceAuthorityLiteralInput;
    let tool_schemas = fixture_tool_schemas();
    let schema_sha256 = digest(&serde_json::to_vec(&tool_schemas).unwrap());
    let product_sha256 = digest(&fs::read(&memory_binary).unwrap());
    let codex_sha256 = digest(&fs::read(&codex_binary).unwrap());
    let evaluator_sha256 = digest(&fs::read(std::env::current_exe().unwrap()).unwrap());
    let config_sha256 = digest(br#"{"EMBEDDINGS_ENABLED":"false","NER_EXTRACTOR":"regex","SURREALDB_DB_NAME":"test","SURREALDB_EMBEDDED":"true","SURREALDB_NAMESPACE":"main"}"#);
    let key_source = CaseTrialKey {
        stratum_id: "source_authority".into(),
        family_id: "cedar-source-family".into(),
        case_id: "cedar-source-recall".into(),
        repetition: 1,
        incumbent_first: true,
    };
    let key_control = CaseTrialKey {
        stratum_id: "source_authority".into(),
        family_id: "ordinary-control-family".into(),
        case_id: "ordinary-control-recall".into(),
        repetition: 1,
        incumbent_first: false,
    };
    let mut expected_trials = vec![key_source.clone(), key_control];
    expected_trials.sort();
    let plan = PairedEvaluationPlan {
        schema_version: "memory-mcp-agent-plan/v2".into(),
        plan_id: "source-authority-codex-workspace-test".into(),
        stage: PairedEvaluationStage::ExploratoryComparison,
        pilot_report_sha256: None,
        power_analysis_sha256: None,
        expected_trials,
        planned_independent_families: 2,
        shared_identity: SharedRunIdentity {
            agent_runner: "codex-cli".into(),
            agent_version: "0.171.0".into(),
            model: "test-model".into(),
            model_identity_protocol: ModelIdentityProtocol::SameAliasWithinInvocation {
                max_duration_seconds: 7200,
            },
            system_prompt_sha256: system_instructions_sha256_for_profile(profile),
            task_prompt_sha256: task_rules_sha256_for_profile(profile),
            host_policy_sha256:
                eval_harness::codex_agent::agent_source_authority_literal_input_host_policy_sha256(),
            transport: "stdio".into(),
            retry_policy_sha256: agent_managed_retry_policy_sha256(),
            reader_config_sha256: reader_config_sha256_for_profile(profile).unwrap(),
            judge_config_sha256: None,
            corpus_sha256: digest(b"test-corpus"),
            oracle_sha256: digest(b"test-oracle"),
            clock_id: "test-clock".into(),
            simulator_sha256: eval_harness::codex_agent::agent_source_authority_simulator_sha256(),
            provider_fingerprints: BTreeMap::from([
                ("codex_cli".into(), codex_sha256),
                ("memory_eval_cli".into(), evaluator_sha256),
            ]),
            context_budget_tokens: 4096,
            tool_call_budget: 8,
            turn_budget: 8,
            output_token_budget: 1024,
            wall_time_budget_ms: 120_000,
            hardware_class: "test".into(),
        },
        incumbent_product: ProductRunIdentity {
            revision: "test-memory".into(),
            binary_sha256: product_sha256.clone(),
            product_config_sha256: config_sha256.clone(),
            tool_schema_sha256: schema_sha256.clone(),
            enabled_features: BTreeSet::from(["fs-watch".into()]),
            claim_rollout_stage: Some("evidence".into()),
        },
        candidate_product: ProductRunIdentity {
            revision: "test-memory".into(),
            binary_sha256: product_sha256,
            product_config_sha256: config_sha256,
            tool_schema_sha256: schema_sha256,
            enabled_features: BTreeSet::from(["fs-watch".into()]),
            claim_rollout_stage: Some("evidence".into()),
        },
        declared_tool_schema_change: None,
        bootstrap: BootstrapConfig {
            seed: 20261123,
            draws: 1000,
            confidence_level_basis_points: 9500,
        },
        promotion_criteria: None,
    };
    let runtime = |temp_root: PathBuf| CodexArmRuntime {
        codex_binary: codex_binary.clone(),
        memory_server_binary: memory_binary.clone(),
        model: "test-model".into(),
        temp_root: Some(temp_root),
        claim_rollout_stage: "evidence".into(),
        transport: PairedAgentTransport::Stdio,
        memory_tool_profile: profile,
        codex_api_key: None,
        local_codex_home: Some(codex_home.clone()),
    };
    let mut backend =
        CodexPairedBackend::new(&plan, runtime(incumbent_temp), runtime(candidate_temp))
            .expect("registered test runner configuration");
    let valid_at: DateTime<Utc> = VALID_AT.parse().unwrap();
    let task = AgentManagedTaskInput {
        key: key_source,
        formation_prompt: "Please remember this source fact for a later session. The literal source text is: Fact: Cedar Observatory the calibration ledger is stored in Cabinet-28. Pass that exact text as the memory ingest content, then extract the returned episode. Do not pass a filename or invent details. Confirm briefly after both calls succeed.".into(),
        candidate_formation_prompt: Some(
            "Use memory evidence to check the Cedar Observatory ledger location; do not write."
                .into(),
        ),
        candidate_operator_imports: vec![AgentManagedOperatorFileImport {
            fixture_id: "source_cedar".into(),
            relative_path: "fixtures/source_cedar.txt".into(),
            content: "Fact: Cedar Observatory the calibration ledger is stored in Cabinet-28.\n"
                .into(),
            source_type: "doc".into(),
            valid_at,
            event_id: EVENT_ID.into(),
        }],
        recall_prompt: "Where is the Cedar Observatory calibration ledger stored?".into(),
        seed_episodes: vec![],
    };
    let probe = AgentManagedProbeRequest {
        query: "Where is the Cedar Observatory calibration ledger stored?".into(),
        valid_at,
        context_budget: 1024,
    };

    let incumbent = run_agent_managed_trial(&mut backend, RunArm::Incumbent, &task, &probe)
        .expect("legacy file ingestion must use the evaluator-owned source root");

    assert_eq!(
        incumbent.formation.status,
        eval_harness::paired::TrialStatus::Completed
    );
    assert!(incumbent.operator_imports.is_empty());
    assert_eq!(incumbent.formation.memory_write_episode_ids, [EPISODE_ID]);
    assert_eq!(incumbent.independent_probe.source_episode_ids, [EPISODE_ID]);

    let candidate = run_agent_managed_trial(&mut backend, RunArm::Candidate, &task, &probe)
        .expect("candidate import must not contaminate the Codex working directory");

    assert_eq!(
        candidate.formation.status,
        eval_harness::paired::TrialStatus::Completed
    );
    assert_eq!(candidate.operator_imports.len(), 1);
    assert!(candidate.operator_imports[0].import_succeeded);
    assert!(candidate.operator_imports[0].extraction_succeeded);
    assert_eq!(
        candidate
            .initial_episode_id_to_event_id
            .get(EPISODE_ID)
            .unwrap(),
        EVENT_ID
    );
    assert_eq!(candidate.independent_probe.source_episode_ids, [EPISODE_ID]);
    assert!(
        !leaked_input.exists(),
        "Codex fixture must not leave prompt files in the crate"
    );
}

fn fixture_tool_schemas() -> Value {
    json!([
        {"name":"ingest","inputSchema":{"type":"object","properties":{"content":{"type":"string"}}}},
        {"name":"extract","inputSchema":{"type":"object","properties":{"episode_id":{"type":"string"}}}},
        {"name":"assemble_context","inputSchema":{"type":"object","properties":{"query":{"type":"string"},"as_of":{"type":"string","deprecated":true},"valid_at":{"type":"string"},"known_revision":{"type":"integer"},"budget":{"type":"integer"},"compact":{"type":"boolean"}}}},
        {"name":"invalidate","inputSchema":{"type":"object","properties":{"fact_id":{"type":"string"}}}}
    ])
}

fn memory_server_script() -> &'static str {
    r##"#!/bin/sh
if [ "$1" = "import" ]; then
  root=""
  relative=""
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --root) root="$2"; shift 2 ;;
      --path) relative="$2"; shift 2 ;;
      *) shift ;;
    esac
  done
  [ -f "$root/$relative" ] || exit 80
  printf '%s\n' '{"status":"success","result":"episode:source-cedar"}'
  exit 0
fi
if [ "$1" = "extract" ]; then
  printf '%s\n' '{"status":"success","result":{}}'
  exit 0
fi
[ "$1" = "serve" ] || exit 81
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  [ -n "$id" ] || continue
  case "$line" in
    *'"method":"initialize"'*) result='{}' ;;
    *'"method":"tools/list"'*) result='{"tools":[{"name":"ingest","inputSchema":{"type":"object","properties":{"content":{"type":"string"}}}},{"name":"extract","inputSchema":{"type":"object","properties":{"episode_id":{"type":"string"}}}},{"name":"assemble_context","inputSchema":{"type":"object","properties":{"query":{"type":"string"},"as_of":{"type":"string","deprecated":true},"valid_at":{"type":"string"},"known_revision":{"type":"integer"},"budget":{"type":"integer"},"compact":{"type":"boolean"}}}},{"name":"invalidate","inputSchema":{"type":"object","properties":{"fact_id":{"type":"string"}}}}]}' ;;
    *'"name":"assemble_context"'*) result='{"structuredContent":{"status":"success","result":[{"source_episode":"episode:source-cedar"}],"context":{"snapshot":{"valid_at":"2026-11-23T00:00:00Z","known_revision":1}}}}' ;;
    *'"name":"ingest"'*) result='{"structuredContent":{"result":"episode:source-cedar"}}' ;;
    *'"name":"extract"'*) result='{"structuredContent":{"result":{}}}' ;;
    *) result='{}' ;;
  esac
  printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$id" "$result"
done
"##
}

fn codex_script() -> &'static str {
    r##"#!/bin/sh
if [ "$1" = "--version" ]; then
  printf '%s\n' 'codex-cli 0.171.0'
  exit 0
fi
[ ! -e "$PWD/fixtures/source_cedar.txt" ] || exit 82
args="$*"
prompt="$(cat)"
if printf '%s' "$args" | grep -F 'enabled_tools=["ingest","extract"]' >/dev/null; then
  printf '%s' "$args" | grep -F '/operator-import-workspace"' >/dev/null || exit 83
  printf '%s' "$prompt" | grep -F 'Fact: Cedar Observatory the calibration ledger is stored in Cabinet-28.' >/dev/null || exit 84
  printf '%s\n' \
    '{"type":"turn.started"}' \
    '{"type":"item.completed","item":{"id":"write-1","type":"mcp_tool_call","server":"memory_decisions","tool":"ingest","arguments":{"content":"source fixture"},"result":{"structuredContent":{"result":"episode:source-cedar"}}}}' \
    '{"type":"item.completed","item":{"id":"extract-1","type":"mcp_tool_call","server":"memory_decisions","tool":"extract","arguments":{"episode_id":"episode:source-cedar"},"result":{"structuredContent":{"result":{}}}}}' \
    '{"type":"item.completed","item":{"id":"answer-1","type":"agent_message","text":"{\"answer\":\"Cabinet-28\",\"citations\":[\"episode:source-cedar\"],\"needs_clarification\":false}"}}' \
    '{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":5}}'
else
  printf '%s' "$args" | grep -F '/formation-workspace"' >/dev/null || exit 84
  printf '%s\n' \
    '{"type":"turn.started"}' \
    '{"type":"item.completed","item":{"id":"answer-1","type":"agent_message","text":"{\"answer\":\"Cabinet-28\",\"citations\":[\"episode:source-cedar\"],\"needs_clarification\":false}"}}' \
    '{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":5}}'
fi
"##
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).expect("write fixture executable");
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
