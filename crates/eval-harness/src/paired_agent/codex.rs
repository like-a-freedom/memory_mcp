//! Optional live Codex adapter for the paired evaluator.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::managed::{
    AgentManagedOperatorFileImport, AgentManagedOperatorImportReceipt,
    AgentManagedProbeObservation, AgentManagedProbeRequest, AgentManagedTaskInput,
    AgentManagedTrialBackend,
};
use super::{
    PairedTaskInput, PairedTrialBackend, PairedTrialExecution, PreparedPairedTask,
    resolve_known_revision_prompt, sha256,
};
use crate::EvalError;
use crate::codex_agent::{
    CodexAgentConfig, CodexMemoryToolProfile, CodexMemoryTransport, run_codex_agent,
};
use crate::memory_fixture::seed_memory;
use crate::paired::{PairedEvaluationPlan, RunArm};
use crate::paired_http::IsolatedHttpMemoryServer;
use crate::suites::memory_decisions::stdio::{StdioSession, configure_embedded_memory_command};

const MAX_CODEX_JSONL_BYTES: usize = 1_048_576;
const MAX_OPERATOR_CLI_OUTPUT_BYTES: usize = 1_048_576;
const OPERATOR_CLI_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub struct CodexArmRuntime {
    pub codex_binary: PathBuf,
    pub memory_server_binary: PathBuf,
    pub model: String,
    pub temp_root: Option<PathBuf>,
    pub claim_rollout_stage: String,
    pub transport: PairedAgentTransport,
    pub memory_tool_profile: CodexMemoryToolProfile,
    pub codex_api_key: Option<OsString>,
    /// When set, use an existing locally authenticated Codex home. The trial
    /// workspace and HOME remain ephemeral; Codex user config is ignored.
    pub local_codex_home: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairedAgentTransport {
    Stdio,
    StreamableHttp,
}

impl PairedAgentTransport {
    pub fn from_identity(value: &str) -> Result<Self, EvalError> {
        match value {
            "stdio" => Ok(Self::Stdio),
            "streamable_http" => Ok(Self::StreamableHttp),
            _ => Err(EvalError::InvalidConfig(format!(
                "unsupported paired agent transport `{value}`"
            ))),
        }
    }

    const fn identity(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::StreamableHttp => "streamable_http",
        }
    }
}

pub struct CodexPairedBackend {
    incumbent: CodexArmRuntime,
    candidate: CodexArmRuntime,
    shared: crate::paired::SharedRunIdentity,
    codex_binary_sha256: String,
    incumbent_product_sha256: String,
    candidate_product_sha256: String,
    incumbent_schema_sha256: String,
    candidate_schema_sha256: String,
    incumbent_schema_validated: bool,
    candidate_schema_validated: bool,
}

pub struct AgentManagedCodexState {
    _trial_directory: TrialDirectory,
    config: CodexAgentConfig,
    arm: RunArm,
    tool_schema_sha256: String,
    product_sha256: String,
    codex_sha256: String,
    operator_imports: Vec<AgentManagedOperatorImportReceipt>,
}

impl CodexPairedBackend {
    pub fn new(
        plan: &PairedEvaluationPlan,
        incumbent: CodexArmRuntime,
        candidate: CodexArmRuntime,
    ) -> Result<Self, EvalError> {
        plan.validate()?;
        if incumbent.memory_tool_profile != candidate.memory_tool_profile {
            return Err(EvalError::InvalidConfig(
                "paired Codex arms must use the same registered memory tool profile".into(),
            ));
        }
        validate_codex_prompt_identity(&plan.shared_identity, incumbent.memory_tool_profile)?;
        let evaluator_sha256 = match incumbent.memory_tool_profile {
            CodexMemoryToolProfile::RecallOnly => None,
            CodexMemoryToolProfile::AgentManagedMemory
            | CodexMemoryToolProfile::AgentSourceAuthority
            | CodexMemoryToolProfile::AgentSourceAuthorityLiteralInput
            | CodexMemoryToolProfile::AgentFormation
            | CodexMemoryToolProfile::AgentMaintenance
            | CodexMemoryToolProfile::AgentMaintenanceV2
            | CodexMemoryToolProfile::AgentMaintenanceV3
            | CodexMemoryToolProfile::AgentMaintenanceV4
            | CodexMemoryToolProfile::AgentMaintenanceV5
            | CodexMemoryToolProfile::AgentMaintenanceV6
            | CodexMemoryToolProfile::AgentMaintenanceV7 => {
                let executable = std::env::current_exe().map_err(|_| {
                    EvalError::InvalidConfig(
                        "could not resolve the active eval-harness executable".into(),
                    )
                })?;
                Some(hash_file(&executable)?)
            }
            CodexMemoryToolProfile::AgentOperatorPrepared
            | CodexMemoryToolProfile::RecallOnlyV2
            | CodexMemoryToolProfile::RecallOnlyV3
            | CodexMemoryToolProfile::RecallOnlyV4 => {
                return Err(EvalError::InvalidConfig(
                    "internal phase profile cannot be used for a paired run".into(),
                ));
            }
        };
        validate_evaluator_fingerprint(
            &plan.shared_identity,
            incumbent.memory_tool_profile,
            evaluator_sha256.as_deref(),
        )?;
        if incumbent.model != plan.shared_identity.model
            || candidate.model != plan.shared_identity.model
        {
            return Err(EvalError::InvalidConfig(
                "both Codex arms must use the model pinned in the paired plan".into(),
            ));
        }
        let same_single_auth_source = incumbent.codex_api_key == candidate.codex_api_key
            && incumbent.local_codex_home == candidate.local_codex_home
            && (incumbent.codex_api_key.is_some() ^ incumbent.local_codex_home.is_some());
        if !same_single_auth_source {
            return Err(EvalError::InvalidConfig(
                "both Codex arms must use the same single authentication source".into(),
            ));
        }
        if incumbent.transport != candidate.transport
            || incumbent.transport.identity() != plan.shared_identity.transport
        {
            return Err(EvalError::InvalidConfig(
                "paired runtime transport must match the transport pinned in the plan".into(),
            ));
        }
        if !matches!(
            incumbent.memory_tool_profile,
            CodexMemoryToolProfile::RecallOnly
                | CodexMemoryToolProfile::RecallOnlyV2
                | CodexMemoryToolProfile::RecallOnlyV3
                | CodexMemoryToolProfile::RecallOnlyV4
        ) {
            let expected_reader_config =
                crate::codex_agent::reader_config_sha256_for_profile(incumbent.memory_tool_profile)
                    .ok_or_else(|| {
                        EvalError::InvalidConfig(
                            "agent-managed memory profile has no pinned reader configuration"
                                .into(),
                        )
                    })?;
            if incumbent.transport != PairedAgentTransport::Stdio
                || plan.shared_identity.wall_time_budget_ms != 120_000
                || plan.shared_identity.reader_config_sha256 != expected_reader_config
            {
                return Err(EvalError::InvalidConfig(
                    "agent-managed memory profile requires its pinned stdio reader configuration"
                        .into(),
                ));
            }
        }
        validate_claim_rollout_stages(
            plan,
            &incumbent.claim_rollout_stage,
            &candidate.claim_rollout_stage,
        )?;
        let incumbent_codex_sha256 = hash_file(&incumbent.codex_binary)?;
        let candidate_codex_sha256 = hash_file(&candidate.codex_binary)?;
        if incumbent_codex_sha256 != candidate_codex_sha256
            || plan.shared_identity.provider_fingerprints.get("codex_cli")
                != Some(&incumbent_codex_sha256)
        {
            return Err(EvalError::InvalidConfig(
                "Codex executable hash must match the shared codex_cli provider fingerprint".into(),
            ));
        }
        validate_product_binary(
            &incumbent.memory_server_binary,
            &plan.incumbent_product.binary_sha256,
        )?;
        validate_product_binary(
            &candidate.memory_server_binary,
            &plan.candidate_product.binary_sha256,
        )?;
        validate_codex_version(&incumbent.codex_binary, &plan.shared_identity.agent_version)?;
        validate_codex_version(&candidate.codex_binary, &plan.shared_identity.agent_version)?;
        Ok(Self {
            incumbent,
            candidate,
            shared: plan.shared_identity.clone(),
            codex_binary_sha256: incumbent_codex_sha256,
            incumbent_product_sha256: plan.incumbent_product.binary_sha256.clone(),
            candidate_product_sha256: plan.candidate_product.binary_sha256.clone(),
            incumbent_schema_sha256: plan.incumbent_product.tool_schema_sha256.clone(),
            candidate_schema_sha256: plan.candidate_product.tool_schema_sha256.clone(),
            incumbent_schema_validated: false,
            candidate_schema_validated: false,
        })
    }
}

fn validate_claim_rollout_stages(
    plan: &PairedEvaluationPlan,
    incumbent: &str,
    candidate: &str,
) -> Result<(), EvalError> {
    if !crate::paired::is_supported_claim_rollout_stage(incumbent)
        || !crate::paired::is_supported_claim_rollout_stage(candidate)
    {
        return Err(EvalError::InvalidConfig(
            "claim rollout stage is unsupported".into(),
        ));
    }
    match (
        plan.incumbent_product.claim_rollout_stage.as_deref(),
        plan.candidate_product.claim_rollout_stage.as_deref(),
    ) {
        (Some(expected_incumbent), Some(expected_candidate))
            if incumbent == expected_incumbent && candidate == expected_candidate =>
        {
            Ok(())
        }
        (None, None) if incumbent == candidate => Ok(()),
        _ => Err(EvalError::InvalidConfig(
            "paired claim rollout stages must match the per-arm stages pinned in the plan".into(),
        )),
    }
}

fn validate_codex_prompt_identity(
    shared: &crate::paired::SharedRunIdentity,
    profile: CodexMemoryToolProfile,
) -> Result<(), EvalError> {
    let (expected_system_prompt, expected_task_prompt) = match profile {
        CodexMemoryToolProfile::RecallOnly => (
            crate::codex_agent::system_instructions_sha256_for_profile(profile),
            crate::codex_agent::task_rules_sha256_for_profile(profile),
        ),
        CodexMemoryToolProfile::AgentManagedMemory => (
            crate::codex_agent::agent_managed_system_prompt_sha256(),
            crate::codex_agent::agent_managed_task_prompt_sha256(),
        ),
        CodexMemoryToolProfile::AgentSourceAuthority => (
            crate::codex_agent::system_instructions_sha256_for_profile(profile),
            crate::codex_agent::task_rules_sha256_for_profile(profile),
        ),
        CodexMemoryToolProfile::AgentSourceAuthorityLiteralInput => (
            crate::codex_agent::system_instructions_sha256_for_profile(profile),
            crate::codex_agent::task_rules_sha256_for_profile(profile),
        ),
        CodexMemoryToolProfile::AgentOperatorPrepared => {
            return Err(EvalError::InvalidConfig(
                "operator-prepared is an internal formation phase profile".into(),
            ));
        }
        CodexMemoryToolProfile::AgentFormation => (
            crate::codex_agent::system_instructions_sha256_for_profile(profile),
            crate::codex_agent::task_rules_sha256_for_profile(profile),
        ),
        CodexMemoryToolProfile::AgentMaintenance => (
            crate::codex_agent::agent_maintenance_system_prompt_sha256(),
            crate::codex_agent::agent_maintenance_task_prompt_sha256(),
        ),
        CodexMemoryToolProfile::AgentMaintenanceV2 => (
            crate::codex_agent::agent_maintenance_v2_system_prompt_sha256(),
            crate::codex_agent::agent_maintenance_v2_task_prompt_sha256(),
        ),
        CodexMemoryToolProfile::AgentMaintenanceV3 => (
            crate::codex_agent::agent_maintenance_v3_system_prompt_sha256(),
            crate::codex_agent::agent_maintenance_v3_task_prompt_sha256(),
        ),
        CodexMemoryToolProfile::AgentMaintenanceV4 => (
            crate::codex_agent::agent_maintenance_v4_system_prompt_sha256(),
            crate::codex_agent::agent_maintenance_v4_task_prompt_sha256(),
        ),
        CodexMemoryToolProfile::AgentMaintenanceV5 => (
            crate::codex_agent::agent_maintenance_v5_system_prompt_sha256(),
            crate::codex_agent::agent_maintenance_v5_task_prompt_sha256(),
        ),
        CodexMemoryToolProfile::AgentMaintenanceV6 => (
            crate::codex_agent::agent_maintenance_v6_system_prompt_sha256(),
            crate::codex_agent::agent_maintenance_v6_task_prompt_sha256(),
        ),
        CodexMemoryToolProfile::AgentMaintenanceV7 => (
            crate::codex_agent::agent_maintenance_v7_system_prompt_sha256(),
            crate::codex_agent::agent_maintenance_v7_task_prompt_sha256(),
        ),
        CodexMemoryToolProfile::RecallOnlyV2
        | CodexMemoryToolProfile::RecallOnlyV3
        | CodexMemoryToolProfile::RecallOnlyV4 => (
            crate::codex_agent::system_instructions_sha256_for_profile(profile),
            crate::codex_agent::task_rules_sha256_for_profile(profile),
        ),
    };
    if shared.system_prompt_sha256 != expected_system_prompt {
        return Err(EvalError::InvalidConfig(
            "paired system prompt hash does not match the Codex instructions embedded in this runner".into(),
        ));
    }
    if shared.task_prompt_sha256 != expected_task_prompt {
        return Err(EvalError::InvalidConfig(
            "paired task prompt hash does not match the task rules embedded in this runner".into(),
        ));
    }
    if !matches!(
        profile,
        CodexMemoryToolProfile::RecallOnly
            | CodexMemoryToolProfile::RecallOnlyV2
            | CodexMemoryToolProfile::RecallOnlyV3
            | CodexMemoryToolProfile::RecallOnlyV4
    ) {
        let expected_host_policy = match profile {
            CodexMemoryToolProfile::AgentMaintenance => {
                crate::codex_agent::agent_maintenance_host_policy_sha256()
            }
            CodexMemoryToolProfile::AgentMaintenanceV2 => {
                crate::codex_agent::agent_maintenance_v2_host_policy_sha256()
            }
            CodexMemoryToolProfile::AgentMaintenanceV3 => {
                crate::codex_agent::agent_maintenance_v3_host_policy_sha256()
            }
            CodexMemoryToolProfile::AgentMaintenanceV4 => {
                crate::codex_agent::agent_maintenance_v4_host_policy_sha256()
            }
            CodexMemoryToolProfile::AgentMaintenanceV5 => {
                crate::codex_agent::agent_maintenance_v4_host_policy_sha256()
            }
            CodexMemoryToolProfile::AgentMaintenanceV6 => {
                crate::codex_agent::agent_maintenance_v6_host_policy_sha256()
            }
            CodexMemoryToolProfile::AgentMaintenanceV7 => {
                crate::codex_agent::agent_maintenance_v7_host_policy_sha256()
            }
            CodexMemoryToolProfile::AgentSourceAuthority => {
                crate::codex_agent::agent_source_authority_host_policy_sha256()
            }
            CodexMemoryToolProfile::AgentSourceAuthorityLiteralInput => {
                crate::codex_agent::agent_source_authority_literal_input_host_policy_sha256()
            }
            CodexMemoryToolProfile::AgentManagedMemory | CodexMemoryToolProfile::AgentFormation => {
                crate::codex_agent::agent_managed_host_policy_sha256()
            }
            CodexMemoryToolProfile::RecallOnly
            | CodexMemoryToolProfile::RecallOnlyV2
            | CodexMemoryToolProfile::RecallOnlyV3
            | CodexMemoryToolProfile::RecallOnlyV4
            | CodexMemoryToolProfile::AgentOperatorPrepared => {
                return Err(EvalError::InvalidConfig(
                    "read-only phase profile cannot use a write host policy".into(),
                ));
            }
        };
        if shared.host_policy_sha256 != expected_host_policy {
            return Err(EvalError::InvalidConfig(
                "agent-managed host policy hash does not match the policy pinned in this runner"
                    .into(),
            ));
        }
        if shared.retry_policy_sha256 != crate::codex_agent::agent_managed_retry_policy_sha256() {
            return Err(EvalError::InvalidConfig(
                "agent-managed retry policy hash does not match the policy pinned in this runner"
                    .into(),
            ));
        }
    }
    Ok(())
}

fn validate_evaluator_fingerprint(
    shared: &crate::paired::SharedRunIdentity,
    profile: CodexMemoryToolProfile,
    actual_sha256: Option<&str>,
) -> Result<(), EvalError> {
    if matches!(
        profile,
        CodexMemoryToolProfile::RecallOnly
            | CodexMemoryToolProfile::RecallOnlyV2
            | CodexMemoryToolProfile::RecallOnlyV3
            | CodexMemoryToolProfile::RecallOnlyV4
    ) {
        return Ok(());
    }
    let registered = shared
        .provider_fingerprints
        .get("memory_eval_cli")
        .map(String::as_str);
    if registered.is_none() || registered != actual_sha256 {
        return Err(EvalError::InvalidConfig(
            "agent-managed plan must pin the active memory-eval executable SHA-256".into(),
        ));
    }
    Ok(())
}

impl PairedTrialBackend for CodexPairedBackend {
    fn prepare(&mut self, task: &PairedTaskInput) -> Result<PreparedPairedTask, EvalError> {
        let Some(boundary_event_id) = task.knowledge_before_event_id.as_deref() else {
            return Ok(PreparedPairedTask {
                input: task.clone(),
                expected_known_revision: None,
            });
        };
        let boundary_index = task
            .seed_episodes
            .iter()
            .position(|episode| episode.event_id == boundary_event_id)
            .ok_or_else(|| {
                EvalError::InvalidInput(format!(
                    "paired task {} does not contain its knowledge boundary event",
                    task.key.case_id
                ))
            })?;
        let boundary_valid_at = task.seed_episodes[boundary_index].valid_at;
        let tempdir = TrialDirectory::create(self.candidate.temp_root.as_deref())?;
        let database_path = tempdir.path.join("database");
        let known_revision = match self.candidate.transport {
            PairedAgentTransport::Stdio => {
                let seeded = seed_memory(
                    &self.candidate.memory_server_binary,
                    &database_path,
                    &self.candidate.claim_rollout_stage,
                    &task.seed_episodes,
                    Some((boundary_index, boundary_valid_at)),
                )?;
                seeded.known_revision
            }
            PairedAgentTransport::StreamableHttp => {
                create_directory(database_path.clone())?;
                let mut server = IsolatedHttpMemoryServer::spawn(
                    &self.candidate.memory_server_binary,
                    &database_path,
                    &self.candidate.claim_rollout_stage,
                )?;
                let async_runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| {
                        EvalError::Suite(format!("create HTTP fixture runtime: {error}"))
                    })?;
                let seeded = async_runtime.block_on(async {
                    server.wait_ready().await?;
                    server
                        .seed_with_revision_checkpoint(
                            &task.seed_episodes,
                            boundary_index,
                            boundary_valid_at,
                        )
                        .await
                })?;
                seeded.known_revision
            }
        }
        .ok_or_else(|| {
            EvalError::InvalidInput(
                "candidate memory server did not return the exact knowledge revision".into(),
            )
        })?;
        let prompt = resolve_known_revision_prompt(&task.prompt, known_revision)?;
        Ok(PreparedPairedTask {
            input: PairedTaskInput {
                prompt,
                ..task.clone()
            },
            expected_known_revision: Some(known_revision),
        })
    }

    fn execute(
        &mut self,
        arm: RunArm,
        prepared_task: &PreparedPairedTask,
    ) -> Result<PairedTrialExecution, EvalError> {
        let task = &prepared_task.input;
        let (runtime, expected_schema, expected_product, schema_validated) = match arm {
            RunArm::Incumbent => (
                &self.incumbent,
                &self.incumbent_schema_sha256,
                &self.incumbent_product_sha256,
                &mut self.incumbent_schema_validated,
            ),
            RunArm::Candidate => (
                &self.candidate,
                &self.candidate_schema_sha256,
                &self.candidate_product_sha256,
                &mut self.candidate_schema_validated,
            ),
        };
        validate_product_binary(&runtime.memory_server_binary, expected_product)?;
        if hash_file(&runtime.codex_binary)? != self.codex_binary_sha256 {
            return Err(EvalError::InvalidConfig(
                "Codex executable changed after paired-run preflight".into(),
            ));
        }
        let tempdir = TrialDirectory::create(runtime.temp_root.as_deref())?;
        let database_path = tempdir.path.join("database");
        let working_directory = create_directory(tempdir.path.join("workspace"))?;
        let home_directory = create_directory(tempdir.path.join("home"))?;
        let codex_home = match &runtime.local_codex_home {
            Some(path) => path.clone(),
            None => create_directory(tempdir.path.join("codex-home"))?,
        };
        let mut http_server = None;
        let (bindings, tool_schemas, memory_transport) = match runtime.transport {
            PairedAgentTransport::Stdio => {
                let checkpoint = if arm == RunArm::Candidate {
                    prepared_task
                        .expected_known_revision
                        .map(|revision| (revision, task.knowledge_before_event_id.as_deref()))
                } else {
                    None
                };
                let boundary = checkpoint
                    .map(|(_, event_id)| {
                        let index = task
                            .seed_episodes
                            .iter()
                            .position(|episode| Some(episode.event_id.as_str()) == event_id)
                            .ok_or_else(|| {
                                EvalError::InvalidInput(
                                    "prepared checkpoint event is missing from task history".into(),
                                )
                            })?;
                        Ok::<_, EvalError>((index, task.seed_episodes[index].valid_at))
                    })
                    .transpose()?;
                let seeded = seed_memory(
                    &runtime.memory_server_binary,
                    &database_path,
                    &runtime.claim_rollout_stage,
                    &task.seed_episodes,
                    boundary,
                )?;
                validate_checkpoint(
                    arm,
                    prepared_task.expected_known_revision,
                    seeded.known_revision,
                )?;
                (
                    seeded.episode_id_to_event_id,
                    seeded.tool_schemas,
                    CodexMemoryTransport::Stdio,
                )
            }
            PairedAgentTransport::StreamableHttp => {
                create_directory(database_path.clone())?;
                let mut server = IsolatedHttpMemoryServer::spawn(
                    &runtime.memory_server_binary,
                    &database_path,
                    &runtime.claim_rollout_stage,
                )?;
                let async_runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| {
                        EvalError::Suite(format!("create HTTP fixture runtime: {error}"))
                    })?;
                let (bindings, schemas, observed_revision) = async_runtime.block_on(async {
                    server.wait_ready().await?;
                    if arm == RunArm::Candidate && prepared_task.expected_known_revision.is_some() {
                        let event_id =
                            task.knowledge_before_event_id.as_deref().ok_or_else(|| {
                                EvalError::InvalidInput(
                                    "prepared revision lacks its boundary event".into(),
                                )
                            })?;
                        let index = task
                            .seed_episodes
                            .iter()
                            .position(|episode| episode.event_id == event_id)
                            .ok_or_else(|| {
                                EvalError::InvalidInput(
                                    "prepared checkpoint event is missing from task history".into(),
                                )
                            })?;
                        let seeded = server
                            .seed_with_revision_checkpoint(
                                &task.seed_episodes,
                                index,
                                task.seed_episodes[index].valid_at,
                            )
                            .await?;
                        let revision = seeded.known_revision.ok_or_else(|| {
                            EvalError::InvalidInput(
                                "HTTP seed omitted its requested revision checkpoint".into(),
                            )
                        })?;
                        Ok::<_, EvalError>((
                            seeded.episode_id_to_event_id,
                            seeded.tool_schemas,
                            Some(revision),
                        ))
                    } else {
                        let (bindings, schemas) = server.seed(&task.seed_episodes).await?;
                        Ok::<_, EvalError>((bindings, schemas, None))
                    }
                })?;
                validate_checkpoint(
                    arm,
                    prepared_task.expected_known_revision,
                    observed_revision,
                )?;
                let transport = CodexMemoryTransport::StreamableHttp {
                    url: server.endpoint.clone(),
                    bearer_token: server.bearer_token.clone(),
                };
                http_server = Some(server);
                (bindings, schemas, transport)
            }
        };
        if !*schema_validated && sha256(&serde_json::to_vec(&tool_schemas)?) != *expected_schema {
            return Err(EvalError::InvalidConfig(
                "memory server tool schema does not match the paired product identity".into(),
            ));
        }
        *schema_validated = true;
        let config = CodexAgentConfig {
            codex_binary: runtime.codex_binary.clone(),
            codex_home,
            home_directory,
            model: runtime.model.clone(),
            claim_rollout_stage: runtime.claim_rollout_stage.clone(),
            memory_server_binary: runtime.memory_server_binary.clone(),
            database_path,
            working_directory,
            memory_server_working_directory: None,
            context_budget_tokens: self.shared.context_budget_tokens,
            tool_call_budget: self.shared.tool_call_budget,
            turn_budget: self.shared.turn_budget,
            output_token_budget: self.shared.output_token_budget,
            wall_time_budget: Duration::from_millis(self.shared.wall_time_budget_ms),
            max_jsonl_bytes: MAX_CODEX_JSONL_BYTES,
            memory_transport,
            memory_tool_profile: runtime.memory_tool_profile,
            codex_api_key: runtime.codex_api_key.clone(),
        };
        let observation = run_codex_agent(&config, &task.prompt)?;
        drop(http_server);
        Ok(PairedTrialExecution {
            observation,
            episode_id_to_event_id: bindings,
        })
    }
}

impl AgentManagedTrialBackend for CodexPairedBackend {
    type State = AgentManagedCodexState;

    fn start_trial(
        &mut self,
        arm: RunArm,
        task: &AgentManagedTaskInput,
    ) -> Result<(Self::State, std::collections::BTreeMap<String, String>), EvalError> {
        let (runtime, expected_schema, expected_product, schema_validated) = match arm {
            RunArm::Incumbent => (
                &self.incumbent,
                &self.incumbent_schema_sha256,
                &self.incumbent_product_sha256,
                &mut self.incumbent_schema_validated,
            ),
            RunArm::Candidate => (
                &self.candidate,
                &self.candidate_schema_sha256,
                &self.candidate_product_sha256,
                &mut self.candidate_schema_validated,
            ),
        };
        if !runtime.memory_tool_profile.supports_agent_managed_trial() {
            return Err(EvalError::InvalidConfig(
                "agent-managed paired trials require the registered writer tool profile".into(),
            ));
        }
        if runtime.transport != PairedAgentTransport::Stdio {
            return Err(EvalError::InvalidConfig(
                "agent-managed paired trials currently require stdio transport".into(),
            ));
        }
        validate_product_binary(&runtime.memory_server_binary, expected_product)?;
        let codex_sha256 = hash_file(&runtime.codex_binary)?;
        if codex_sha256 != self.codex_binary_sha256 {
            return Err(EvalError::InvalidConfig(
                "Codex executable changed after paired-run preflight".into(),
            ));
        }

        let trial_directory = TrialDirectory::create(runtime.temp_root.as_deref())?;
        let database_path = trial_directory.path.join("database");
        let working_directory = create_directory(trial_directory.path.join("formation-workspace"))?;
        let operator_import_workspace =
            create_directory(trial_directory.path.join("operator-import-workspace"))?;
        stage_operator_import_fixtures(
            &operator_import_workspace,
            &task.candidate_operator_imports,
        )?;
        let seeded = seed_memory(
            &runtime.memory_server_binary,
            &database_path,
            &runtime.claim_rollout_stage,
            &task.seed_episodes,
            None,
        )?;
        let observed_schema_sha256 = sha256(&serde_json::to_vec(&seeded.tool_schemas)?);
        if observed_schema_sha256 != *expected_schema {
            return Err(EvalError::InvalidConfig(
                "memory server tool schema does not match the paired product identity".into(),
            ));
        }
        *schema_validated = true;

        let mut episode_id_to_event_id = seeded.episode_id_to_event_id;
        let mut operator_imports = Vec::new();
        if arm == RunArm::Candidate {
            for fixture in &task.candidate_operator_imports {
                let (
                    import_succeeded,
                    extraction_attempted,
                    extraction_succeeded,
                    episode_id,
                    elapsed_ms,
                ) = run_operator_file_setup(
                    &runtime.memory_server_binary,
                    &database_path,
                    &runtime.claim_rollout_stage,
                    &operator_import_workspace,
                    fixture,
                );
                if let Some(episode_id) = episode_id
                    && episode_id_to_event_id
                        .insert(episode_id, fixture.event_id.clone())
                        .is_some()
                {
                    return Err(EvalError::InvalidInput(
                        "operator import reused an existing episode identity".into(),
                    ));
                }
                operator_imports.push(AgentManagedOperatorImportReceipt {
                    fixture_id: fixture.fixture_id.clone(),
                    import_succeeded,
                    extraction_attempted,
                    extraction_succeeded,
                    elapsed_ms,
                });
            }
        }

        let home_directory = create_directory(trial_directory.path.join("home"))?;
        let codex_home = match &runtime.local_codex_home {
            Some(path) => path.clone(),
            None => create_directory(trial_directory.path.join("codex-home"))?,
        };
        let write_profile = runtime
            .memory_tool_profile
            .formation_profile_for_arm(arm, !task.candidate_operator_imports.is_empty())
            .ok_or_else(|| {
                EvalError::InvalidConfig(
                    "operator-import tasks require the source-authority workflow profile".into(),
                )
            })?;
        let config = CodexAgentConfig {
            codex_binary: runtime.codex_binary.clone(),
            codex_home,
            home_directory,
            model: runtime.model.clone(),
            claim_rollout_stage: runtime.claim_rollout_stage.clone(),
            memory_server_binary: runtime.memory_server_binary.clone(),
            database_path,
            working_directory,
            memory_server_working_directory: (arm == RunArm::Incumbent
                && !task.candidate_operator_imports.is_empty())
            .then(|| operator_import_workspace.clone()),
            context_budget_tokens: self.shared.context_budget_tokens,
            tool_call_budget: self.shared.tool_call_budget,
            turn_budget: self.shared.turn_budget,
            output_token_budget: self.shared.output_token_budget,
            wall_time_budget: Duration::from_millis(self.shared.wall_time_budget_ms),
            max_jsonl_bytes: MAX_CODEX_JSONL_BYTES,
            memory_transport: CodexMemoryTransport::Stdio,
            memory_tool_profile: write_profile,
            codex_api_key: runtime.codex_api_key.clone(),
        };
        let state = AgentManagedCodexState {
            _trial_directory: trial_directory,
            config,
            arm,
            tool_schema_sha256: observed_schema_sha256,
            product_sha256: expected_product.clone(),
            codex_sha256,
            operator_imports,
        };
        Ok((state, episode_id_to_event_id))
    }

    fn operator_import_receipts(
        &self,
        state: &Self::State,
    ) -> Vec<AgentManagedOperatorImportReceipt> {
        state.operator_imports.clone()
    }

    fn form_memory(
        &mut self,
        state: &mut Self::State,
        prompt: &str,
    ) -> Result<crate::paired_agent::AgentObservation, EvalError> {
        validate_managed_runtime(&state.config, &state.product_sha256, &state.codex_sha256)?;
        if !state
            .config
            .memory_tool_profile
            .supports_agent_memory_formation()
        {
            return Err(EvalError::InvalidConfig(
                "formation session lost its pinned memory writer profile".into(),
            ));
        }
        if state.config.memory_tool_profile == CodexMemoryToolProfile::AgentOperatorPrepared
            && (state.arm != RunArm::Candidate || state.operator_imports.is_empty())
        {
            return Err(EvalError::InvalidConfig(
                "read-only formation requires a registered candidate operator setup".into(),
            ));
        }
        run_codex_agent(&state.config, prompt)
    }

    fn recall_in_fresh_session(
        &mut self,
        state: &mut Self::State,
        prompt: &str,
    ) -> Result<crate::paired_agent::AgentObservation, EvalError> {
        validate_managed_runtime(&state.config, &state.product_sha256, &state.codex_sha256)?;
        let mut config = state.config.clone();
        config.memory_tool_profile = state.config.memory_tool_profile.recall_profile();
        config.working_directory =
            create_directory(state._trial_directory.path.join("recall-workspace"))?;
        run_codex_agent(&config, prompt)
    }

    fn independent_probe(
        &mut self,
        state: &mut Self::State,
        probe: &AgentManagedProbeRequest,
    ) -> Result<AgentManagedProbeObservation, EvalError> {
        validate_managed_runtime(&state.config, &state.product_sha256, &state.codex_sha256)?;
        let mut session = StdioSession::spawn_with_claim_stage(
            &state.config.memory_server_binary,
            &state.config.database_path,
            &state.config.claim_rollout_stage,
        )?;
        let observed_schema = sha256(&serde_json::to_vec(session.tool_schemas())?);
        if observed_schema != state.tool_schema_sha256 {
            return Err(EvalError::InvalidConfig(
                "post-write probe tool schema changed within the paired trial".into(),
            ));
        }
        let read = crate::independent_probe::read_public_probe(
            &mut session,
            &probe.query,
            probe.valid_at,
            probe.context_budget,
            None,
        )?;
        session.shutdown()?;
        let retained_store_bytes = directory_size(&state.config.database_path)?;
        Ok(AgentManagedProbeObservation {
            source_episode_ids: read.source_episode_ids,
            known_revision: read.known_revision,
            retained_store_bytes,
            request_sha256: sha256(&serde_json::to_vec(&read.request)?),
            response_sha256: sha256(&serde_json::to_vec(&read.response)?),
        })
    }
}

pub(crate) fn stage_operator_import_fixtures(
    working_directory: &Path,
    fixtures: &[AgentManagedOperatorFileImport],
) -> Result<(), EvalError> {
    for fixture in fixtures {
        let relative = Path::new(&fixture.relative_path);
        if relative.is_absolute()
            || fixture.relative_path.contains('\\')
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(EvalError::InvalidInput(
                "registered operator fixture path is not confined".into(),
            ));
        }
        let path = working_directory.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|_| {
                EvalError::InvalidInput("registered operator fixture could not be staged".into())
            })?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|_| {
                EvalError::InvalidInput("registered operator fixture could not be staged".into())
            })?;
        file.write_all(fixture.content.as_bytes()).map_err(|_| {
            EvalError::InvalidInput("registered operator fixture could not be staged".into())
        })?;
    }
    Ok(())
}

pub(crate) fn run_operator_file_setup(
    binary: &Path,
    database_path: &Path,
    claim_stage: &str,
    working_directory: &Path,
    fixture: &AgentManagedOperatorFileImport,
) -> (bool, bool, bool, Option<String>, u64) {
    let started = Instant::now();
    let import_args = [
        "import".to_string(),
        "file".to_string(),
        "--root".to_string(),
        working_directory.display().to_string(),
        "--path".to_string(),
        fixture.relative_path.clone(),
        "--source-type".to_string(),
        fixture.source_type.clone(),
        "--t-ref".to_string(),
        fixture.valid_at.to_rfc3339(),
    ];
    let Ok(import_response) = run_memory_cli_json(
        binary,
        database_path,
        claim_stage,
        working_directory,
        &import_args,
    ) else {
        return (false, false, false, None, elapsed_ms(started));
    };
    let Some(episode_id) = import_response
        .get("result")
        .and_then(serde_json::Value::as_str)
        .filter(|episode_id| episode_id.starts_with("episode:") && episode_id.len() > 8)
        .map(str::to_owned)
    else {
        return (false, false, false, None, elapsed_ms(started));
    };
    if import_response
        .get("status")
        .and_then(serde_json::Value::as_str)
        != Some("success")
    {
        return (false, false, false, None, elapsed_ms(started));
    }

    let extract_args = [
        "extract".to_string(),
        "--episode-id".to_string(),
        episode_id.clone(),
    ];
    let Ok(extract_response) = run_memory_cli_json(
        binary,
        database_path,
        claim_stage,
        working_directory,
        &extract_args,
    ) else {
        return (true, true, false, Some(episode_id), elapsed_ms(started));
    };
    let extraction_succeeded = extract_response
        .get("status")
        .and_then(serde_json::Value::as_str)
        == Some("success");
    (
        true,
        true,
        extraction_succeeded,
        Some(episode_id),
        elapsed_ms(started),
    )
}

fn run_memory_cli_json(
    binary: &Path,
    database_path: &Path,
    claim_stage: &str,
    working_directory: &Path,
    args: &[String],
) -> Result<serde_json::Value, ()> {
    let mut command = Command::new(binary);
    command.args(args).current_dir(working_directory);
    configure_embedded_memory_command(&mut command, database_path, Some(claim_stage));
    let (status, output) = run_bounded_command(command)?;
    if !status.success() {
        return Err(());
    }
    let response: serde_json::Value = serde_json::from_slice(&output).map_err(|_| ())?;
    if response.get("status").and_then(serde_json::Value::as_str) == Some("error") {
        return Err(());
    }
    Ok(response)
}

fn run_bounded_command(mut command: Command) -> Result<(std::process::ExitStatus, Vec<u8>), ()> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| ())?;
    let stdout = child.stdout.take().ok_or(())?;
    let stderr = child.stderr.take().ok_or(())?;
    let stdout_reader = thread::spawn(move || read_bounded_output(stdout));
    let stderr_reader =
        thread::spawn(move || io::copy(&mut io::BufReader::new(stderr), &mut io::sink()));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(());
            }
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= OPERATOR_CLI_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(());
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
        }
    };
    let (output, overflowed) = stdout_reader.join().map_err(|_| ())?.map_err(|_| ())?;
    let _ = stderr_reader.join();
    if overflowed {
        return Err(());
    }
    Ok((status, output))
}

fn read_bounded_output(mut reader: impl Read) -> io::Result<(Vec<u8>, bool)> {
    let mut output = Vec::new();
    let mut overflowed = false;
    let mut buffer = [0_u8; 8192];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = MAX_OPERATOR_CLI_OUTPUT_BYTES.saturating_sub(output.len());
        let retained = count.min(remaining);
        output.extend_from_slice(&buffer[..retained]);
        overflowed |= retained < count;
    }
    Ok((output, overflowed))
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn validate_managed_runtime(
    config: &CodexAgentConfig,
    expected_product_sha256: &str,
    expected_codex_sha256: &str,
) -> Result<(), EvalError> {
    validate_product_binary(&config.memory_server_binary, expected_product_sha256)?;
    if hash_file(&config.codex_binary)? != expected_codex_sha256 {
        return Err(EvalError::InvalidConfig(
            "Codex executable changed during the agent-managed trial".into(),
        ));
    }
    Ok(())
}

pub(crate) fn directory_size(path: &Path) -> Result<u64, EvalError> {
    let mut pending = vec![path.to_path_buf()];
    let mut total = 0_u64;
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory).map_err(|source| EvalError::Io {
            path: directory.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| EvalError::Io {
                path: directory.clone(),
                source,
            })?;
            let entry_path = entry.path();
            let file_type = entry.file_type().map_err(|source| EvalError::Io {
                path: entry_path.clone(),
                source,
            })?;
            if file_type.is_dir() {
                pending.push(entry_path);
            } else if file_type.is_file() {
                let bytes = entry
                    .metadata()
                    .map_err(|source| EvalError::Io {
                        path: entry_path,
                        source,
                    })?
                    .len();
                total = total.checked_add(bytes).ok_or_else(|| {
                    EvalError::InvalidInput("agent-managed retained store size overflowed".into())
                })?;
            }
        }
    }
    Ok(total)
}

fn validate_checkpoint(
    arm: RunArm,
    expected: Option<i64>,
    observed: Option<i64>,
) -> Result<(), EvalError> {
    if arm == RunArm::Candidate && expected != observed {
        return Err(EvalError::InvalidInput(format!(
            "candidate knowledge checkpoint changed: expected {expected:?}, observed {observed:?}"
        )));
    }
    Ok(())
}

pub(crate) fn validate_product_binary(path: &Path, expected_sha256: &str) -> Result<(), EvalError> {
    if !path.is_absolute() || !path.is_file() {
        return Err(EvalError::InvalidConfig(
            "paired product binary path must name an existing absolute file".into(),
        ));
    }
    if hash_file(path)? != expected_sha256 {
        return Err(EvalError::InvalidConfig(
            "paired product binary does not match its preregistered SHA-256".into(),
        ));
    }
    Ok(())
}

pub(crate) fn hash_file(path: &Path) -> Result<String, EvalError> {
    if !path.is_absolute() || !path.is_file() {
        return Err(EvalError::InvalidConfig(
            "binary fingerprint path must name an existing absolute file".into(),
        ));
    }
    let bytes = std::fs::read(path).map_err(|source| EvalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(sha256(&bytes))
}

fn validate_codex_version(path: &Path, expected_version: &str) -> Result<(), EvalError> {
    if !path.is_absolute() || !path.is_file() {
        return Err(EvalError::InvalidConfig(
            "Codex executable path must name an existing absolute file".into(),
        ));
    }
    let output = Command::new(path)
        .arg("--version")
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .output()
        .map_err(|source| EvalError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let actual = String::from_utf8_lossy(&output.stdout);
    if !output.status.success()
        || !actual
            .split_whitespace()
            .any(|token| token == expected_version)
    {
        return Err(EvalError::InvalidConfig(
            "Codex executable version does not match the paired agent identity".into(),
        ));
    }
    Ok(())
}

fn create_directory(path: PathBuf) -> Result<PathBuf, EvalError> {
    std::fs::create_dir_all(&path).map_err(|source| EvalError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

pub(crate) struct TrialDirectory {
    pub(crate) path: PathBuf,
}

impl TrialDirectory {
    pub(crate) fn create(parent: Option<&Path>) -> Result<Self, EvalError> {
        static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
        let default_root = std::env::temp_dir();
        let parent = parent.unwrap_or(default_root.as_path());
        if !parent.is_absolute() || !parent.is_dir() {
            return Err(EvalError::InvalidConfig(
                "paired temporary root must be an existing absolute directory".into(),
            ));
        }
        for _ in 0..100 {
            let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(
                "memory-paired-trial-{}-{sequence}",
                std::process::id()
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(EvalError::Io { path, source });
                }
            }
        }
        Err(EvalError::InvalidConfig(
            "could not allocate a unique paired-trial temporary directory".into(),
        ))
    }
}

impl Drop for TrialDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::super::SeedEpisode;
    use super::super::tests::{manifest, plan};
    use super::*;
    use chrono::{DateTime, Utc};
    use serde_json::Value;

    fn codex_identity() -> crate::paired::SharedRunIdentity {
        let mut identity = plan(&manifest()).shared_identity;
        identity.system_prompt_sha256 = crate::codex_agent::system_instructions_sha256();
        identity.task_prompt_sha256 = crate::codex_agent::task_rules_sha256();
        identity.host_policy_sha256 = crate::codex_agent::agent_managed_host_policy_sha256();
        identity.retry_policy_sha256 = crate::codex_agent::agent_managed_retry_policy_sha256();
        identity
    }

    #[test]
    fn codex_prompt_identity_rejects_an_unregistered_system_prompt() {
        let mut identity = codex_identity();
        identity.system_prompt_sha256 = "0".repeat(64);

        let result = validate_codex_prompt_identity(&identity, CodexMemoryToolProfile::RecallOnly);

        assert!(
            matches!(result, Err(EvalError::InvalidConfig(message)) if message.contains("system prompt"))
        );
    }

    #[test]
    fn codex_prompt_identity_rejects_unregistered_task_rules() {
        let mut identity = codex_identity();
        identity.task_prompt_sha256 = "0".repeat(64);

        let result = validate_codex_prompt_identity(&identity, CodexMemoryToolProfile::RecallOnly);

        assert!(
            matches!(result, Err(EvalError::InvalidConfig(message)) if message.contains("task prompt"))
        );
    }

    #[test]
    fn agent_managed_prompt_identity_is_distinct_from_read_only_identity() {
        let mut identity = codex_identity();
        identity.system_prompt_sha256 = crate::codex_agent::agent_managed_system_prompt_sha256();
        identity.task_prompt_sha256 = crate::codex_agent::agent_managed_task_prompt_sha256();

        assert!(
            validate_codex_prompt_identity(&identity, CodexMemoryToolProfile::AgentManagedMemory)
                .is_ok()
        );
        assert!(
            validate_codex_prompt_identity(&identity, CodexMemoryToolProfile::RecallOnly).is_err()
        );

        identity.system_prompt_sha256 = crate::codex_agent::system_instructions_sha256_for_profile(
            CodexMemoryToolProfile::AgentManagedMemory,
        );
        identity.task_prompt_sha256 = crate::codex_agent::task_rules_sha256_for_profile(
            CodexMemoryToolProfile::AgentManagedMemory,
        );
        assert!(
            validate_codex_prompt_identity(&identity, CodexMemoryToolProfile::AgentManagedMemory)
                .is_err()
        );
    }

    #[test]
    fn source_authority_identity_pins_role_specific_prompts_and_host_policy() {
        let mut identity = codex_identity();
        identity.system_prompt_sha256 = crate::codex_agent::system_instructions_sha256_for_profile(
            CodexMemoryToolProfile::AgentSourceAuthority,
        );
        identity.task_prompt_sha256 = crate::codex_agent::task_rules_sha256_for_profile(
            CodexMemoryToolProfile::AgentSourceAuthority,
        );
        identity.host_policy_sha256 =
            crate::codex_agent::agent_source_authority_host_policy_sha256();

        assert!(
            validate_codex_prompt_identity(&identity, CodexMemoryToolProfile::AgentSourceAuthority)
                .is_ok()
        );
        assert!(
            crate::codex_agent::reader_config_sha256_for_profile(
                CodexMemoryToolProfile::AgentSourceAuthority
            )
            .is_some()
        );

        identity.host_policy_sha256 = crate::codex_agent::agent_managed_host_policy_sha256();
        assert!(
            validate_codex_prompt_identity(&identity, CodexMemoryToolProfile::AgentSourceAuthority)
                .is_err()
        );
    }

    #[test]
    fn maintenance_profile_has_its_own_prompt_and_host_policy_identity() {
        let mut identity = codex_identity();
        identity.system_prompt_sha256 =
            crate::codex_agent::agent_maintenance_system_prompt_sha256();
        identity.task_prompt_sha256 = crate::codex_agent::agent_maintenance_task_prompt_sha256();
        identity.host_policy_sha256 = crate::codex_agent::agent_maintenance_host_policy_sha256();

        assert!(
            validate_codex_prompt_identity(&identity, CodexMemoryToolProfile::AgentMaintenance)
                .is_ok()
        );

        identity.host_policy_sha256 = crate::codex_agent::agent_managed_host_policy_sha256();
        assert!(
            validate_codex_prompt_identity(&identity, CodexMemoryToolProfile::AgentMaintenance)
                .is_err()
        );
    }

    #[test]
    fn maintenance_v5_identity_pins_the_recovery_recall_phase() {
        let mut identity = codex_identity();
        identity.system_prompt_sha256 =
            crate::codex_agent::agent_maintenance_v5_system_prompt_sha256();
        identity.task_prompt_sha256 = crate::codex_agent::agent_maintenance_v5_task_prompt_sha256();
        identity.host_policy_sha256 = crate::codex_agent::agent_maintenance_v4_host_policy_sha256();

        assert!(
            validate_codex_prompt_identity(&identity, CodexMemoryToolProfile::AgentMaintenanceV5)
                .is_ok()
        );
        assert_eq!(
            crate::codex_agent::reader_config_sha256_for_profile(
                CodexMemoryToolProfile::AgentMaintenanceV5
            ),
            crate::codex_agent::reader_config_sha256_for_profile(
                CodexMemoryToolProfile::AgentMaintenanceV4
            )
        );

        identity.system_prompt_sha256 =
            crate::codex_agent::agent_maintenance_v4_system_prompt_sha256();
        identity.task_prompt_sha256 = crate::codex_agent::agent_maintenance_v4_task_prompt_sha256();
        assert!(
            validate_codex_prompt_identity(&identity, CodexMemoryToolProfile::AgentMaintenanceV5)
                .is_err()
        );
    }

    #[test]
    fn maintenance_v6_identity_pins_the_partial_read_retry_contract() {
        let mut identity = codex_identity();
        identity.system_prompt_sha256 =
            crate::codex_agent::agent_maintenance_v6_system_prompt_sha256();
        identity.task_prompt_sha256 = crate::codex_agent::agent_maintenance_v6_task_prompt_sha256();
        identity.host_policy_sha256 = crate::codex_agent::agent_maintenance_v6_host_policy_sha256();

        assert!(
            validate_codex_prompt_identity(&identity, CodexMemoryToolProfile::AgentMaintenanceV6)
                .is_ok()
        );
        assert_eq!(
            crate::codex_agent::reader_config_sha256_for_profile(
                CodexMemoryToolProfile::AgentMaintenanceV6
            ),
            crate::codex_agent::reader_config_sha256_for_profile(
                CodexMemoryToolProfile::AgentMaintenanceV5
            )
        );

        identity.system_prompt_sha256 =
            crate::codex_agent::agent_maintenance_v5_system_prompt_sha256();
        identity.task_prompt_sha256 = crate::codex_agent::agent_maintenance_v5_task_prompt_sha256();
        assert!(
            validate_codex_prompt_identity(&identity, CodexMemoryToolProfile::AgentMaintenanceV6)
                .is_err()
        );
    }

    #[test]
    fn maintenance_v7_identity_pins_the_snapshot_consistent_retry_contract() {
        let mut identity = codex_identity();
        identity.system_prompt_sha256 =
            crate::codex_agent::agent_maintenance_v7_system_prompt_sha256();
        identity.task_prompt_sha256 = crate::codex_agent::agent_maintenance_v7_task_prompt_sha256();
        identity.host_policy_sha256 = crate::codex_agent::agent_maintenance_v7_host_policy_sha256();

        assert!(
            validate_codex_prompt_identity(&identity, CodexMemoryToolProfile::AgentMaintenanceV7)
                .is_ok()
        );
        assert_eq!(
            crate::codex_agent::reader_config_sha256_for_profile(
                CodexMemoryToolProfile::AgentMaintenanceV7
            ),
            crate::codex_agent::reader_config_sha256_for_profile(
                CodexMemoryToolProfile::AgentMaintenanceV6
            )
        );

        identity.system_prompt_sha256 =
            crate::codex_agent::agent_maintenance_v6_system_prompt_sha256();
        identity.task_prompt_sha256 = crate::codex_agent::agent_maintenance_v6_task_prompt_sha256();
        assert!(
            validate_codex_prompt_identity(&identity, CodexMemoryToolProfile::AgentMaintenanceV7)
                .is_err()
        );
    }

    #[test]
    fn agent_managed_prompt_identity_rejects_an_unregistered_host_policy() {
        let mut identity = codex_identity();
        identity.system_prompt_sha256 = crate::codex_agent::agent_managed_system_prompt_sha256();
        identity.task_prompt_sha256 = crate::codex_agent::agent_managed_task_prompt_sha256();
        identity.host_policy_sha256 = "0".repeat(64);

        let result =
            validate_codex_prompt_identity(&identity, CodexMemoryToolProfile::AgentManagedMemory);

        assert!(
            matches!(result, Err(EvalError::InvalidConfig(message)) if message.contains("host policy"))
        );
    }

    #[test]
    fn agent_managed_prompt_identity_rejects_an_unregistered_retry_policy() {
        let mut identity = codex_identity();
        identity.system_prompt_sha256 = crate::codex_agent::agent_managed_system_prompt_sha256();
        identity.task_prompt_sha256 = crate::codex_agent::agent_managed_task_prompt_sha256();
        identity.retry_policy_sha256 = "0".repeat(64);

        let result =
            validate_codex_prompt_identity(&identity, CodexMemoryToolProfile::AgentManagedMemory);

        assert!(
            matches!(result, Err(EvalError::InvalidConfig(message)) if message.contains("retry policy"))
        );
    }

    #[test]
    fn agent_managed_run_requires_the_active_evaluator_binary_fingerprint() {
        let mut identity = codex_identity();
        identity
            .provider_fingerprints
            .insert("memory_eval_cli".into(), "e".repeat(64));

        assert!(
            validate_evaluator_fingerprint(
                &identity,
                CodexMemoryToolProfile::AgentManagedMemory,
                Some(&"e".repeat(64)),
            )
            .is_ok()
        );
        assert!(
            validate_evaluator_fingerprint(
                &identity,
                CodexMemoryToolProfile::AgentManagedMemory,
                Some(&"f".repeat(64)),
            )
            .is_err()
        );

        identity.provider_fingerprints.remove("memory_eval_cli");
        assert!(
            validate_evaluator_fingerprint(
                &identity,
                CodexMemoryToolProfile::AgentManagedMemory,
                Some(&"e".repeat(64)),
            )
            .is_err()
        );
    }

    #[test]
    fn candidate_seed_must_reproduce_the_prepared_revision() {
        assert!(validate_checkpoint(RunArm::Candidate, Some(7), Some(7)).is_ok());
        assert!(validate_checkpoint(RunArm::Candidate, Some(7), Some(8)).is_err());
        assert!(validate_checkpoint(RunArm::Candidate, Some(7), None).is_err());
        assert!(validate_checkpoint(RunArm::Incumbent, Some(7), None).is_ok());
    }

    #[test]
    fn rollout_stage_resolution_uses_each_registered_arm_stage() {
        let tasks = manifest();
        let mut plan = plan(&tasks);
        plan.incumbent_product.claim_rollout_stage = Some("shadow".into());
        plan.candidate_product.claim_rollout_stage = Some("evidence".into());

        plan.validate().expect("plan pins valid, different stages");
        assert_eq!(
            plan.claim_rollout_stages(None).expect("registered stages"),
            ("shadow".into(), "evidence".into())
        );
    }

    #[test]
    fn paired_plan_hash_binds_distinct_rollout_stages() {
        let tasks = manifest();
        let mut plan = plan(&tasks);
        let legacy_hash = plan.sha256().expect("hash legacy plan");
        plan.incumbent_product.claim_rollout_stage = Some("shadow".into());
        plan.candidate_product.claim_rollout_stage = Some("evidence".into());

        assert_ne!(plan.sha256().expect("hash stage-pinned plan"), legacy_hash);
    }

    #[test]
    fn paired_plan_rejects_a_stage_pinned_for_only_one_arm() {
        let tasks = manifest();
        let mut plan = plan(&tasks);
        plan.incumbent_product.claim_rollout_stage = Some("shadow".into());

        assert!(plan.validate().is_err());
        assert!(plan.claim_rollout_stages(None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn codex_backend_accepts_distinct_stages_pinned_by_the_plan() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("paired runtime directory");
        let codex_binary = directory.path().join("codex");
        std::fs::write(&codex_binary, "#!/bin/sh\nprintf 'codex-cli 0.169.0\\n'\n")
            .expect("write fake Codex executable");
        std::fs::set_permissions(&codex_binary, std::fs::Permissions::from_mode(0o755))
            .expect("make fake Codex executable runnable");
        let incumbent_binary = directory.path().join("incumbent-memory");
        let candidate_binary = directory.path().join("candidate-memory");
        std::fs::write(&incumbent_binary, b"incumbent memory server")
            .expect("write incumbent fixture binary");
        std::fs::write(&candidate_binary, b"candidate memory server")
            .expect("write candidate fixture binary");

        let tasks = manifest();
        let mut plan = plan(&tasks);
        plan.shared_identity.system_prompt_sha256 =
            crate::codex_agent::system_instructions_sha256();
        plan.shared_identity.task_prompt_sha256 = crate::codex_agent::task_rules_sha256();
        plan.shared_identity.provider_fingerprints.insert(
            "codex_cli".into(),
            hash_file(&codex_binary).expect("hash Codex executable"),
        );
        plan.incumbent_product.binary_sha256 =
            hash_file(&incumbent_binary).expect("hash incumbent");
        plan.candidate_product.binary_sha256 =
            hash_file(&candidate_binary).expect("hash candidate");
        plan.incumbent_product.claim_rollout_stage = Some("shadow".into());
        plan.candidate_product.claim_rollout_stage = Some("evidence".into());
        plan.validate().expect("valid paired plan");

        let runtime = |memory_server_binary, claim_rollout_stage| CodexArmRuntime {
            codex_binary: codex_binary.clone(),
            memory_server_binary,
            model: plan.shared_identity.model.clone(),
            temp_root: Some(directory.path().to_path_buf()),
            claim_rollout_stage,
            transport: PairedAgentTransport::Stdio,
            memory_tool_profile: CodexMemoryToolProfile::RecallOnly,
            codex_api_key: None,
            local_codex_home: Some(directory.path().to_path_buf()),
        };

        assert!(
            CodexPairedBackend::new(
                &plan,
                runtime(incumbent_binary, "shadow".into()),
                runtime(candidate_binary, "evidence".into()),
            )
            .is_ok()
        );
    }

    #[test]
    fn common_rollout_override_cannot_replace_registered_per_arm_stages() {
        let tasks = manifest();
        let mut plan = plan(&tasks);
        plan.incumbent_product.claim_rollout_stage = Some("shadow".into());
        plan.candidate_product.claim_rollout_stage = Some("evidence".into());

        assert!(plan.claim_rollout_stages(Some("evidence")).is_err());
    }

    #[test]
    fn legacy_plan_defaults_both_rollout_stages_to_evidence() {
        let tasks = manifest();
        let plan = plan(&tasks);

        assert_eq!(
            plan.claim_rollout_stages(None).expect("legacy default"),
            ("evidence".into(), "evidence".into())
        );
    }

    #[test]
    #[ignore = "requires MEMORY_MCP_PAIRED_STDIO_BINARY pointing to a built memory_mcp binary"]
    fn live_stdio_seed_captures_the_pre_correction_revision() {
        let binary = std::env::var_os("MEMORY_MCP_PAIRED_STDIO_BINARY")
            .map(PathBuf::from)
            .expect("set MEMORY_MCP_PAIRED_STDIO_BINARY to the candidate memory_mcp binary");
        let temporary = tempfile::tempdir().expect("paired stdio temp root");
        let boundary = DateTime::parse_from_rfc3339("2026-06-01T10:00:00Z")
            .expect("valid boundary time")
            .with_timezone(&Utc);
        let episodes = vec![
            SeedEpisode {
                event_id: "before-correction".into(),
                source_id: "source-before".into(),
                source_type: "conversation".into(),
                content: "The address is North City.".into(),
                valid_at: boundary,
            },
            SeedEpisode {
                event_id: "correction".into(),
                source_id: "source-correction".into(),
                source_type: "conversation".into(),
                content: "Correction: the address is South City.".into(),
                valid_at: boundary,
            },
        ];

        let seeded = seed_memory(
            &binary,
            &temporary.path().join("database"),
            "shadow",
            &episodes,
            Some((1, boundary)),
        )
        .expect("seed both events and capture the cutoff before correction");

        assert!(seeded.known_revision.is_some_and(|revision| revision > 0));
        assert!(seeded.tool_schemas.as_array().is_some_and(|tools| {
            tools
                .iter()
                .any(|tool| tool.get("name").and_then(Value::as_str) == Some("assemble_context"))
        }));
    }
}
