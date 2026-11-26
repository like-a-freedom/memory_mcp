//! Provider-neutral paired source-authority runner for loopback Chat Completions.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use super::chat_completions::{
    ChatCompletionBudgets, ChatCompletionsAgent, ChatCompletionsConfig, ChatCompletionsSampling,
    ChatCompletionsToolProfile, endpoint_config_sha256, request_parameters_config_sha256,
    response_format_config_sha256,
};
use super::codex::{
    TrialDirectory, directory_size, hash_file, run_operator_file_setup,
    stage_operator_import_fixtures, validate_product_binary,
};
use super::managed::{
    AgentManagedOperatorImportReceipt, AgentManagedProbeObservation, AgentManagedProbeRequest,
    AgentManagedTaskInput, AgentManagedTaskManifest, AgentManagedTrialBackend,
};
use super::prompts;
use super::{AgentObservation, SeedMemoryOutput, sha256};
use crate::EvalError;
use crate::paired::{ModelIdentityProtocol, PairedEvaluationPlan, RunArm};
use crate::suites::memory_decisions::stdio::StdioSession;

pub const CHAT_COMPLETIONS_AGENT_RUNNER: &str = "openai-compatible-chat-completions";

/// One arm's product binary and loopback model endpoint. Both arms must share
/// the plan-pinned model alias and endpoint so only the memory product differs.
pub struct ChatCompletionsArmRuntime {
    pub memory_server_binary: PathBuf,
    pub endpoint: String,
    pub bearer_token: Option<String>,
    pub temp_root: Option<PathBuf>,
    pub claim_rollout_stage: String,
}

pub struct ChatCompletionsPairedBackend {
    shared: crate::paired::SharedRunIdentity,
    plan_sha256: String,
    public_synthetic_traces: Option<PathBuf>,
    incumbent: ArmRuntime,
    candidate: ArmRuntime,
}

struct ArmRuntime {
    memory_server_binary: PathBuf,
    temp_root: Option<PathBuf>,
    claim_rollout_stage: String,
    expected_product_sha256: String,
    expected_tool_schema_sha256: String,
    agent: ChatCompletionsAgent,
}

pub struct ChatCompletionsTrialState {
    _trial_directory: TrialDirectory,
    key: crate::paired::CaseTrialKey,
    database_path: PathBuf,
    source_root: PathBuf,
    arm: RunArm,
    tool_schema_sha256: String,
    operator_imports: Vec<AgentManagedOperatorImportReceipt>,
}

impl ChatCompletionsPairedBackend {
    /// Explicit assertion by the caller that every task input/output is public synthetic data.
    pub fn with_public_synthetic_traces(mut self, directory: PathBuf) -> Self {
        self.public_synthetic_traces = Some(directory);
        self
    }

    pub fn new(
        plan: &PairedEvaluationPlan,
        manifest: &AgentManagedTaskManifest,
        incumbent: ChatCompletionsArmRuntime,
        candidate: ChatCompletionsArmRuntime,
        sampling: ChatCompletionsSampling,
    ) -> Result<Self, EvalError> {
        Self::new_with_response_format(plan, manifest, incumbent, candidate, sampling, None)
    }

    pub fn new_with_response_format(
        plan: &PairedEvaluationPlan,
        manifest: &AgentManagedTaskManifest,
        incumbent: ChatCompletionsArmRuntime,
        candidate: ChatCompletionsArmRuntime,
        sampling: ChatCompletionsSampling,
        response_format: Option<serde_json::Value>,
    ) -> Result<Self, EvalError> {
        Self::new_with_request_parameters(
            plan,
            manifest,
            incumbent,
            candidate,
            sampling,
            response_format,
            None,
        )
    }

    pub fn new_with_request_parameters(
        plan: &PairedEvaluationPlan,
        manifest: &AgentManagedTaskManifest,
        incumbent: ChatCompletionsArmRuntime,
        candidate: ChatCompletionsArmRuntime,
        sampling: ChatCompletionsSampling,
        response_format: Option<serde_json::Value>,
        request_parameters: Option<serde_json::Value>,
    ) -> Result<Self, EvalError> {
        plan.validate()?;
        manifest.validate(plan)?;
        let fingerprints = manifest.fingerprints_for_chat_completions()?;
        let shared = &plan.shared_identity;
        if shared.agent_runner != CHAT_COMPLETIONS_AGENT_RUNNER
            || shared.transport != "stdio"
            || shared.model.trim().is_empty()
        {
            return Err(EvalError::InvalidConfig(
                "paired plan must pin the loopback Chat Completions runner and stdio memory transport".into(),
            ));
        }
        if shared.system_prompt_sha256 != fingerprints.system_prompt_sha256
            || shared.task_prompt_sha256 != fingerprints.task_prompt_sha256
            || shared.host_policy_sha256 != fingerprints.host_policy_sha256
            || shared.retry_policy_sha256 != fingerprints.retry_policy_sha256
            || shared.reader_config_sha256 != fingerprints.protocol_config_sha256
            || shared.simulator_sha256 != fingerprints.simulator_sha256
        {
            return Err(EvalError::InvalidConfig(
                "paired plan prompt and runner-policy hashes do not match the registered Chat Completions protocol".into(),
            ));
        }
        Self::build(
            plan,
            incumbent,
            candidate,
            sampling,
            response_format,
            request_parameters,
        )
    }

    fn build(
        plan: &PairedEvaluationPlan,
        incumbent: ChatCompletionsArmRuntime,
        candidate: ChatCompletionsArmRuntime,
        sampling: ChatCompletionsSampling,
        response_format: Option<serde_json::Value>,
        request_parameters: Option<serde_json::Value>,
    ) -> Result<Self, EvalError> {
        let shared = &plan.shared_identity;
        let runner_binary = std::env::current_exe().map_err(|_| {
            EvalError::InvalidConfig("could not resolve the active memory-eval executable".into())
        })?;
        if shared.provider_fingerprints.get("memory_eval_cli") != Some(&hash_file(&runner_binary)?)
        {
            return Err(EvalError::InvalidConfig(
                "paired plan must pin the active memory-eval executable SHA-256".into(),
            ));
        }
        let endpoint_config_sha256 = endpoint_config_sha256(&incumbent.endpoint)?;
        if shared
            .provider_fingerprints
            .get("chat_completions_endpoint_config")
            != Some(&endpoint_config_sha256)
        {
            return Err(EvalError::InvalidConfig(
                "paired plan must pin the active Chat Completions endpoint configuration SHA-256"
                    .into(),
            ));
        }
        let sampling_fingerprint = sampling.fingerprint_sha256()?;
        match shared
            .provider_fingerprints
            .get("chat_completions_sampling_config")
        {
            Some(expected) if expected == &sampling_fingerprint => {}
            Some(_) => {
                return Err(EvalError::InvalidConfig(
                    "paired plan sampling-configuration fingerprint does not match the active sampler".into(),
                ));
            }
            None if sampling == ChatCompletionsSampling::default() => {}
            None => {
                return Err(EvalError::InvalidConfig(
                    "non-default Chat Completions sampling must be hash-pinned in the paired plan"
                        .into(),
                ));
            }
        }
        let active_response_format_fingerprint = response_format
            .as_ref()
            .map(response_format_config_sha256)
            .transpose()?;
        match (
            active_response_format_fingerprint.as_ref(),
            shared
                .provider_fingerprints
                .get("chat_completions_response_format_config"),
        ) {
            (None, None) => {}
            (Some(active), Some(expected)) if active == expected => {}
            (None, Some(_)) => {
                return Err(EvalError::InvalidConfig(
                    "paired plan pins a Chat Completions response format but none is active".into(),
                ));
            }
            (Some(_), None) => {
                return Err(EvalError::InvalidConfig(
                    "Chat Completions response format must be hash-pinned in the paired plan"
                        .into(),
                ));
            }
            (Some(_), Some(_)) => {
                return Err(EvalError::InvalidConfig(
                    "paired plan response-format fingerprint does not match the active response format".into(),
                ));
            }
        }
        let active_request_parameters_fingerprint = request_parameters
            .as_ref()
            .map(request_parameters_config_sha256)
            .transpose()?;
        match (
            active_request_parameters_fingerprint.as_ref(),
            shared
                .provider_fingerprints
                .get("chat_completions_request_parameters_config"),
        ) {
            (None, None) => {}
            (Some(active), Some(expected)) if active == expected => {}
            (None, Some(_)) => {
                return Err(EvalError::InvalidConfig(
                    "paired plan pins Chat Completions request parameters but none are active"
                        .into(),
                ));
            }
            (Some(_), None) => {
                return Err(EvalError::InvalidConfig(
                    "Chat Completions request parameters must be hash-pinned in the paired plan"
                        .into(),
                ));
            }
            (Some(_), Some(_)) => {
                return Err(EvalError::InvalidConfig(
                    "paired plan request-parameters fingerprint does not match the active configuration"
                        .into(),
                ));
            }
        }
        if incumbent.endpoint != candidate.endpoint
            || incumbent.bearer_token != candidate.bearer_token
            || incumbent.claim_rollout_stage != plan.claim_rollout_stages(None)?.0
            || candidate.claim_rollout_stage != plan.claim_rollout_stages(None)?.1
        {
            return Err(EvalError::InvalidConfig(
                "paired Chat Completions arms must share one model endpoint and use their pinned claim stages".into(),
            ));
        }
        if let ModelIdentityProtocol::ProviderRevision { revision } =
            &shared.model_identity_protocol
            && revision != &shared.model
        {
            return Err(EvalError::InvalidConfig(
                "Chat Completions model identity must pin the exact provider model identifier"
                    .into(),
            ));
        }

        let expected_model = shared.model.clone();
        let make_arm = |runtime: ChatCompletionsArmRuntime,
                        product: &crate::paired::ProductRunIdentity|
         -> Result<ArmRuntime, EvalError> {
            validate_product_binary(&runtime.memory_server_binary, &product.binary_sha256)?;
            let agent = ChatCompletionsAgent::new(
                ChatCompletionsConfig::new(&runtime.endpoint, expected_model.clone())
                    .with_bearer_token(runtime.bearer_token.clone())
                    .with_sampling(sampling)
                    .with_response_format(response_format.clone())
                    .with_request_parameters(request_parameters.clone()),
            )?;
            Ok(ArmRuntime {
                memory_server_binary: runtime.memory_server_binary,
                temp_root: runtime.temp_root,
                claim_rollout_stage: runtime.claim_rollout_stage,
                expected_product_sha256: product.binary_sha256.clone(),
                expected_tool_schema_sha256: product.tool_schema_sha256.clone(),
                agent,
            })
        };
        Ok(Self {
            shared: shared.clone(),
            plan_sha256: plan.sha256()?,
            public_synthetic_traces: None,
            incumbent: make_arm(incumbent, &plan.incumbent_product)?,
            candidate: make_arm(candidate, &plan.candidate_product)?,
        })
    }

    fn arm(&self, arm: RunArm) -> &ArmRuntime {
        match arm {
            RunArm::Incumbent => &self.incumbent,
            RunArm::Candidate => &self.candidate,
        }
    }

    fn open_session(&self, state: &ChatCompletionsTrialState) -> Result<StdioSession, EvalError> {
        let runtime = self.arm(state.arm);
        let session = StdioSession::spawn_with_claim_stage_and_cwd(
            &runtime.memory_server_binary,
            &state.database_path,
            &runtime.claim_rollout_stage,
            &state.source_root,
        )?;
        self.validate_session(state, &session)?;
        Ok(session)
    }

    fn validate_session(
        &self,
        state: &ChatCompletionsTrialState,
        session: &StdioSession,
    ) -> Result<(), EvalError> {
        let runtime = self.arm(state.arm);
        let schema_sha256 = sha256(&serde_json::to_vec(session.tool_schemas())?);
        if schema_sha256 != state.tool_schema_sha256
            || schema_sha256 != runtime.expected_tool_schema_sha256
        {
            return Err(EvalError::InvalidConfig(
                "memory MCP tool schema changed during the local paired trial".into(),
            ));
        }
        if hash_file(&runtime.memory_server_binary)? != runtime.expected_product_sha256 {
            return Err(EvalError::InvalidConfig(
                "memory product binary changed during the local paired trial".into(),
            ));
        }
        Ok(())
    }

    fn run_phase(
        &self,
        state: &ChatCompletionsTrialState,
        session: &mut StdioSession,
        prompt: &str,
        profile: ChatCompletionsToolProfile,
        diagnostic: Option<(
            super::chat_completions::RecallReadMode,
            Option<&super::chat_completions::RecallReplyValidator<'_>>,
        )>,
    ) -> Result<AgentObservation, EvalError> {
        let budgets = ChatCompletionBudgets {
            context_tokens: self.shared.context_budget_tokens,
            tool_calls: self.shared.tool_call_budget,
            turns: self.shared.turn_budget,
            output_tokens: self.shared.output_token_budget,
            wall_time: Duration::from_millis(self.shared.wall_time_budget_ms),
        };
        let agent = &self.arm(state.arm).agent;
        let observation = if let Some(directory) = &self.public_synthetic_traces {
            let identity = super::public_trace::SyntheticTraceIdentity {
                plan_sha256: self.plan_sha256.clone(),
                product_binary_sha256: self.arm(state.arm).expected_product_sha256.clone(),
                tool_schema_sha256: state.tool_schema_sha256.clone(),
                key: state.key.clone(),
                arm: state.arm,
                phase: match profile {
                    ChatCompletionsToolProfile::RecallOnly => "recall",
                    ChatCompletionsToolProfile::SourceAuthorityFormation => "formation",
                }
                .into(),
            };
            fs::create_dir_all(directory).map_err(|source| EvalError::Io {
                path: directory.clone(),
                source,
            })?;
            let path = directory.join(format!("{}.json", sha256(&serde_json::to_vec(&identity)?)));
            let mut traced = super::public_trace::PublicSyntheticSession::new(session, identity)?;
            let result = run_agent_phase(agent, &mut traced, prompt, profile, budgets, diagnostic);
            super::write_json_new(&path, &traced.finish())?;
            result
        } else {
            run_agent_phase(agent, session, prompt, profile, budgets, diagnostic)
        }?;
        let shutdown = session.shutdown();
        // A deadline kills and reaps the owned server. Its expected exit status
        // must not erase the timed-out observation or its already recorded calls.
        if observation.status != crate::paired::TrialStatus::TimedOut {
            shutdown?;
        }
        Ok(observation)
    }
}

fn run_agent_phase<M: super::chat_completions::MemoryToolSession>(
    agent: &ChatCompletionsAgent,
    session: &mut M,
    prompt: &str,
    profile: ChatCompletionsToolProfile,
    budgets: ChatCompletionBudgets,
    diagnostic: Option<(
        super::chat_completions::RecallReadMode,
        Option<&super::chat_completions::RecallReplyValidator<'_>>,
    )>,
) -> Result<AgentObservation, EvalError> {
    let (system, rules) = match profile {
        ChatCompletionsToolProfile::SourceAuthorityFormation => (
            prompts::FORMATION_SYSTEM_INSTRUCTIONS,
            prompts::FORMATION_TASK_RULES,
        ),
        ChatCompletionsToolProfile::RecallOnly => (
            prompts::RECALL_SYSTEM_INSTRUCTIONS,
            prompts::RECALL_TASK_RULES,
        ),
    };
    if let Some((mode, validate)) = diagnostic {
        let mut memory = super::chat_completions::RecallMemorySession::new(session, mode)?
            .with_reply_validator(validate);
        agent.run_phase(&mut memory, prompt, system, rules, profile, budgets)
    } else {
        agent.run_phase(session, prompt, system, rules, profile, budgets)
    }
}

#[derive(Clone, Copy, clap::ValueEnum, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecallCaptureMode {
    Live,
    FixedReply,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct FixedReadInput {
    pub key: crate::paired::CaseTrialKey,
    pub arm: RunArm,
    pub arguments: serde_json::Value,
    pub expected_source_events: Vec<String>,
    pub expected_completeness: Option<super::ReadCompleteness>,
    #[serde(default, skip_serializing_if = "is_current_recall_presentation")]
    pub presentation: super::chat_completions::RecallPresentation,
}

fn is_current_recall_presentation(
    presentation: &super::chat_completions::RecallPresentation,
) -> bool {
    *presentation == super::chat_completions::RecallPresentation::Current
}

impl FixedReadInput {
    fn validate_reply(
        &self,
        reply: &serde_json::Value,
        bindings: &BTreeMap<String, String>,
    ) -> Result<(), EvalError> {
        let invalid = || {
            EvalError::InvalidInput("fixed reply differs from registered shape or snapshot".into())
        };
        let cutoff = super::tool_results::requested_cutoff(&self.arguments);
        let evidence =
            super::tool_results::read_evidence(&reply["result"], None).ok_or_else(invalid)?;
        let sources = evidence
            .source_episode_ids
            .iter()
            .map(|id| bindings.get(id).map(String::as_str).ok_or_else(invalid))
            .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
        if !sources
            .into_iter()
            .eq(self.expected_source_events.iter().map(String::as_str))
            || evidence.completeness != self.expected_completeness
            || cutoff.is_some() && evidence.returned_cutoff_sha256 != cutoff
        {
            return Err(invalid());
        }
        Ok(())
    }
}

pub struct RecallRegistration {
    mode: RecallCaptureMode,
    inputs: Vec<FixedReadInput>,
}

impl RecallRegistration {
    pub fn new(
        mode: RecallCaptureMode,
        inputs: Option<Vec<FixedReadInput>>,
        plan: &PairedEvaluationPlan,
    ) -> Result<Self, EvalError> {
        let invalid = || {
            EvalError::InvalidConfig("unregistered recall read mode or fixed-read registry".into())
        };
        let supported_stage = match mode {
            RecallCaptureMode::Live => matches!(
                plan.stage,
                crate::paired::PairedEvaluationStage::DevelopmentPilot
                    | crate::paired::PairedEvaluationStage::ExploratoryComparison
            ),
            RecallCaptureMode::FixedReply => {
                plan.stage == crate::paired::PairedEvaluationStage::ExploratoryComparison
            }
        };
        if !supported_stage
            || plan.shared_identity.tool_call_budget != 1
            || plan.shared_identity.agent_runner != CHAT_COMPLETIONS_AGENT_RUNNER
            || plan.shared_identity.transport != "stdio"
            || matches!(mode, RecallCaptureMode::FixedReply) != inputs.is_some()
        {
            return Err(invalid());
        }
        let mut inputs = inputs.unwrap_or_default();
        if inputs.len() > plan.expected_trials.len().saturating_mul(2) {
            return Err(invalid());
        }
        let arm_order = |arm| u8::from(arm == RunArm::Candidate);
        inputs.sort_by(|a, b| {
            a.key
                .cmp(&b.key)
                .then_with(|| arm_order(a.arm).cmp(&arm_order(b.arm)))
        });
        for input in &mut inputs {
            input.expected_source_events.sort();
        }
        let fingerprint = sha256(&serde_json::to_vec(&("recall-read/v1", mode, &inputs))?);
        if fingerprint != plan.shared_identity.reader_config_sha256 {
            return Err(invalid());
        }
        if matches!(mode, RecallCaptureMode::FixedReply)
            && (inputs.len() != plan.expected_trials.len() * 2
                || plan.expected_trials.iter().any(|key| {
                    [RunArm::Incumbent, RunArm::Candidate]
                        .into_iter()
                        .any(|arm| {
                            inputs
                                .iter()
                                .filter(|item| &item.key == key && item.arm == arm)
                                .count()
                                != 1
                        })
                }))
        {
            return Err(invalid());
        }
        for input in &inputs {
            let args = &input.arguments;
            if !args
                .get("query")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|s| !s.trim().is_empty())
                || !args
                    .get("budget")
                    .and_then(serde_json::Value::as_u64)
                    .is_some_and(|n| (1..=32).contains(&n))
                || serde_json::to_vec(args)?.len() > 4096
                || input
                    .expected_source_events
                    .windows(2)
                    .any(|pair| pair[0] == pair[1])
            {
                return Err(invalid());
            }
        }
        Ok(Self { mode, inputs })
    }
}

pub struct ChatCompletionsRecallBackend {
    runner: ChatCompletionsPairedBackend,
    registration: RecallRegistration,
}

impl ChatCompletionsRecallBackend {
    pub fn with_public_synthetic_traces(mut self, directory: PathBuf) -> Self {
        self.runner = self.runner.with_public_synthetic_traces(directory);
        self
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        plan: &PairedEvaluationPlan,
        manifest: &super::PairedTaskManifest,
        registration: RecallRegistration,
        incumbent: ChatCompletionsArmRuntime,
        candidate: ChatCompletionsArmRuntime,
        sampling: ChatCompletionsSampling,
        response_format: Option<serde_json::Value>,
        request_parameters: Option<serde_json::Value>,
    ) -> Result<Self, EvalError> {
        manifest.validate(plan)?;
        for input in &registration.inputs {
            if !input.expected_source_events.iter().all(|event| {
                manifest
                    .tasks
                    .iter()
                    .find(|task| task.key == input.key)
                    .is_some_and(|task| {
                        task.seed_episodes
                            .iter()
                            .any(|seed| &seed.event_id == event)
                    })
            }) {
                return Err(EvalError::InvalidConfig(
                    "fixed read expects a source outside seeded history".into(),
                ));
            }
        }
        let runner = ChatCompletionsPairedBackend::build(
            plan,
            incumbent,
            candidate,
            sampling,
            response_format,
            request_parameters,
        )?;
        Ok(Self {
            runner,
            registration,
        })
    }

    fn seed(
        &self,
        arm: RunArm,
        task: &super::PairedTaskInput,
    ) -> Result<(ChatCompletionsTrialState, SeedMemoryOutput, StdioSession), EvalError> {
        let runtime = self.runner.arm(arm);
        let directory = TrialDirectory::create(runtime.temp_root.as_deref())?;
        let database_path = directory.path.join("database");
        let boundary = task
            .knowledge_before_event_id
            .as_ref()
            .map(|id| {
                task.seed_episodes
                    .iter()
                    .enumerate()
                    .find(|(_, seed)| &seed.event_id == id)
                    .map(|(index, seed)| (index, seed.valid_at))
                    .ok_or_else(|| {
                        EvalError::InvalidInput("recall checkpoint event missing".into())
                    })
            })
            .transpose()?;
        let mut session = StdioSession::spawn_with_claim_stage_and_cwd(
            &runtime.memory_server_binary,
            &database_path,
            &runtime.claim_rollout_stage,
            &directory.path,
        )?;
        let seeded = crate::memory_fixture::seed_memory_in_session(
            &mut session,
            &task.seed_episodes,
            boundary,
        )?;
        let state = ChatCompletionsTrialState {
            source_root: directory.path.clone(),
            _trial_directory: directory,
            key: task.key.clone(),
            database_path,
            arm,
            tool_schema_sha256: sha256(&serde_json::to_vec(&seeded.tool_schemas)?),
            operator_imports: Vec::new(),
        };
        Ok((state, seeded, session))
    }
}

impl super::PairedTrialBackend for ChatCompletionsRecallBackend {
    fn prepare(
        &mut self,
        task: &super::PairedTaskInput,
    ) -> Result<super::PreparedPairedTask, EvalError> {
        let mut input = task.clone();
        let expected_known_revision = if task.knowledge_before_event_id.is_some() {
            let (_state, seeded, mut session) = self.seed(RunArm::Candidate, task)?;
            session.shutdown()?;
            let revision = seeded
                .known_revision
                .ok_or_else(|| EvalError::InvalidInput("recall checkpoint unavailable".into()))?;
            input.prompt = super::resolve_known_revision_prompt(&input.prompt, revision)?;
            Some(revision)
        } else {
            None
        };
        Ok(super::PreparedPairedTask {
            input,
            expected_known_revision,
        })
    }

    fn execute(
        &mut self,
        arm: RunArm,
        task: &super::PreparedPairedTask,
    ) -> Result<super::PairedTrialExecution, EvalError> {
        let (state, seeded, mut session) = self.seed(arm, &task.input)?;
        if seeded.known_revision != task.expected_known_revision {
            return Err(EvalError::InvalidInput(
                "recall checkpoint differs across seeded trials".into(),
            ));
        }
        let registered = self
            .registration
            .inputs
            .iter()
            .find(|entry| entry.key == task.input.key && entry.arm == arm);
        let mode = match (self.registration.mode, registered) {
            (RecallCaptureMode::Live, None) => super::chat_completions::RecallReadMode::Live,
            (RecallCaptureMode::FixedReply, Some(entry)) => match entry.presentation {
                super::chat_completions::RecallPresentation::Current => {
                    super::chat_completions::RecallReadMode::FixedReply(entry.arguments.clone())
                }
                super::chat_completions::RecallPresentation::EvidenceActionHint => {
                    super::chat_completions::RecallReadMode::FixedReplyWithEvidenceActionHint(
                        entry.arguments.clone(),
                    )
                }
            },
            _ => {
                return Err(EvalError::InvalidConfig(
                    "no registered fixed read for this trial".into(),
                ));
            }
        };
        let validation = std::cell::Cell::new(false);
        let validate = |reply: &serde_json::Value| {
            let Some(entry) = registered else {
                return Ok(());
            };
            let result = entry.validate_reply(reply, &seeded.episode_id_to_event_id);
            validation.set(result.is_err());
            result
        };
        self.runner.validate_session(&state, &session)?;
        let observation = self.runner.run_phase(
            &state,
            &mut session,
            &task.input.prompt,
            ChatCompletionsToolProfile::RecallOnly,
            Some((mode, Some(&validate))),
        )?;
        if validation.get() {
            return Err(EvalError::InvalidInput(
                "fixed reply differs from registered shape or snapshot".into(),
            ));
        }
        Ok(super::PairedTrialExecution {
            observation,
            episode_id_to_event_id: seeded.episode_id_to_event_id,
        })
    }
}

impl AgentManagedTrialBackend for ChatCompletionsPairedBackend {
    type State = ChatCompletionsTrialState;

    fn start_trial(
        &mut self,
        arm: RunArm,
        task: &AgentManagedTaskInput,
    ) -> Result<(Self::State, BTreeMap<String, String>), EvalError> {
        let runtime = self.arm(arm);
        let trial_directory = TrialDirectory::create(runtime.temp_root.as_deref())?;
        let database_path = trial_directory.path.join("database");
        fs::create_dir_all(&database_path).map_err(|source| EvalError::Io {
            path: database_path.clone(),
            source,
        })?;
        let source_root = trial_directory.path.join("source-root");
        fs::create_dir_all(&source_root).map_err(|source| EvalError::Io {
            path: source_root.clone(),
            source,
        })?;
        stage_operator_import_fixtures(&source_root, &task.candidate_operator_imports)?;

        let seeded = crate::memory_fixture::seed_memory(
            &runtime.memory_server_binary,
            &database_path,
            &runtime.claim_rollout_stage,
            &task.seed_episodes,
            None,
        )?;
        let SeedMemoryOutput {
            episode_id_to_event_id: mut bindings,
            tool_schemas,
            ..
        } = seeded;
        let tool_schema_sha256 = sha256(&serde_json::to_vec(&tool_schemas)?);
        if tool_schema_sha256 != runtime.expected_tool_schema_sha256 {
            return Err(EvalError::InvalidConfig(
                "memory MCP tool schema does not match the paired plan".into(),
            ));
        }

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
                    &source_root,
                    fixture,
                );
                if let Some(episode_id) = &episode_id
                    && bindings
                        .insert(episode_id.clone(), fixture.event_id.clone())
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
        Ok((
            ChatCompletionsTrialState {
                _trial_directory: trial_directory,
                key: task.key.clone(),
                database_path,
                source_root,
                arm,
                tool_schema_sha256,
                operator_imports,
            },
            bindings,
        ))
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
    ) -> Result<AgentObservation, EvalError> {
        let profile = ChatCompletionsToolProfile::source_authority_formation_for_arm(
            state.arm,
            !state.operator_imports.is_empty(),
        );
        let mut session = self.open_session(state)?;
        self.run_phase(state, &mut session, prompt, profile, None)
    }

    fn recall_in_fresh_session(
        &mut self,
        state: &mut Self::State,
        prompt: &str,
    ) -> Result<AgentObservation, EvalError> {
        let mut session = self.open_session(state)?;
        self.run_phase(
            state,
            &mut session,
            prompt,
            ChatCompletionsToolProfile::RecallOnly,
            None,
        )
    }

    fn independent_probe(
        &mut self,
        state: &mut Self::State,
        probe: &AgentManagedProbeRequest,
    ) -> Result<AgentManagedProbeObservation, EvalError> {
        let mut session = self.open_session(state)?;
        let read = crate::independent_probe::read_public_probe(
            &mut session,
            &probe.query,
            probe.valid_at,
            probe.context_budget,
            None,
        )?;
        session.shutdown()?;
        Ok(AgentManagedProbeObservation {
            source_episode_ids: read.source_episode_ids,
            known_revision: read.known_revision,
            retained_store_bytes: directory_size(&state.database_path)?,
            request_sha256: sha256(&serde_json::to_vec(&read.request)?),
            response_sha256: sha256(&serde_json::to_vec(&read.response)?),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::paired::PairedEvaluationPlan;
    use crate::paired_agent::chat_completions::RecallPresentation;

    fn preregistered_inputs() -> (PairedEvaluationPlan, AgentManagedTaskManifest, PathBuf) {
        let mut plan: PairedEvaluationPlan = serde_json::from_str(include_str!(
            "../../../../evals/paired/source-authority-agent-workflow-v2/plan.json"
        ))
        .expect("checked-in source-authority plan parses");
        let manifest: AgentManagedTaskManifest = serde_json::from_str(include_str!(
            "../../../../evals/paired/source-authority-agent-workflow-v2/tasks-v1.json"
        ))
        .expect("checked-in source-authority tasks parse");
        let fingerprint = manifest
            .fingerprints_for_chat_completions()
            .expect("checked-in source-authority tasks match the local runner protocol");
        let executable = std::env::current_exe().expect("resolve test executable");
        let executable_sha256 = hash_file(&executable).expect("hash test executable");
        plan.shared_identity.agent_runner = CHAT_COMPLETIONS_AGENT_RUNNER.into();
        plan.shared_identity.agent_version = "v1-test".into();
        plan.shared_identity.model = "test-model".into();
        plan.shared_identity.model_identity_protocol = ModelIdentityProtocol::ProviderRevision {
            revision: "test-model".into(),
        };
        plan.shared_identity.system_prompt_sha256 = fingerprint.system_prompt_sha256;
        plan.shared_identity.task_prompt_sha256 = fingerprint.task_prompt_sha256;
        plan.shared_identity.host_policy_sha256 = fingerprint.host_policy_sha256;
        plan.shared_identity.retry_policy_sha256 = fingerprint.retry_policy_sha256;
        plan.shared_identity.reader_config_sha256 = fingerprint.protocol_config_sha256;
        plan.shared_identity.simulator_sha256 = fingerprint.simulator_sha256;
        plan.shared_identity
            .provider_fingerprints
            .insert("memory_eval_cli".into(), executable_sha256.clone());
        plan.shared_identity.provider_fingerprints.insert(
            "chat_completions_endpoint_config".into(),
            "f7bc8c808719d3bafd7ade8b4a4636cc29db9357b7dd495b8231dfbe2ea9af08".into(),
        );
        plan.incumbent_product.binary_sha256 = executable_sha256.clone();
        plan.candidate_product.binary_sha256 = executable_sha256;
        (plan, manifest, executable)
    }

    #[test]
    fn live_recall_registration_accepts_a_development_pilot_plan() {
        let mut plan: PairedEvaluationPlan = serde_json::from_str(include_str!(
            "../../../../evals/paired/recall-read-local-screen-2026-11-24/live-plan.json"
        ))
        .expect("local recall plan parses");
        plan.stage = crate::paired::PairedEvaluationStage::DevelopmentPilot;
        plan.shared_identity.reader_config_sha256 = sha256(
            &serde_json::to_vec(&(
                "recall-read/v1",
                RecallCaptureMode::Live,
                &Vec::<FixedReadInput>::new(),
            ))
            .expect("serialize live recall registration"),
        );

        let registration = RecallRegistration::new(RecallCaptureMode::Live, None, &plan);

        assert!(registration.is_ok());
    }

    fn fixed_read_inputs(
        plan: &PairedEvaluationPlan,
        candidate_presentation: RecallPresentation,
    ) -> Vec<FixedReadInput> {
        plan.expected_trials
            .iter()
            .flat_map(|key| {
                [RunArm::Incumbent, RunArm::Candidate]
                    .into_iter()
                    .map(move |arm| FixedReadInput {
                        key: key.clone(),
                        arm,
                        arguments: serde_json::json!({
                            "query": format!("{} registered read", key.case_id),
                            "budget": 1
                        }),
                        expected_source_events: Vec::new(),
                        expected_completeness: None,
                        presentation: if arm == RunArm::Candidate {
                            candidate_presentation
                        } else {
                            RecallPresentation::Current
                        },
                    })
            })
            .collect()
    }

    fn fixed_read_plan() -> PairedEvaluationPlan {
        let mut plan: PairedEvaluationPlan = serde_json::from_str(include_str!(
            "../../../../evals/paired/recall-read-local-screen-2026-11-24/live-plan.json"
        ))
        .expect("local recall plan parses");
        plan.stage = crate::paired::PairedEvaluationStage::ExploratoryComparison;
        plan.shared_identity.tool_call_budget = 1;
        plan
    }

    #[test]
    fn current_fixed_read_presentation_is_omitted_from_legacy_registration_bytes() {
        let plan = fixed_read_plan();
        let inputs = fixed_read_inputs(&plan, RecallPresentation::Current);
        let serialized = serde_json::to_value(inputs).expect("serialize current inputs");

        assert!(
            serialized
                .as_array()
                .unwrap()
                .iter()
                .all(|input| input.get("presentation").is_none())
        );
    }

    #[test]
    fn evidence_action_hint_requires_a_matching_fixed_read_fingerprint() {
        let mut plan = fixed_read_plan();
        let current_inputs = fixed_read_inputs(&plan, RecallPresentation::Current);
        plan.shared_identity.reader_config_sha256 = sha256(
            &serde_json::to_vec(&(
                "recall-read/v1",
                RecallCaptureMode::FixedReply,
                &current_inputs,
            ))
            .expect("serialize current registration"),
        );
        assert!(
            RecallRegistration::new(RecallCaptureMode::FixedReply, Some(current_inputs), &plan,)
                .is_ok()
        );

        let candidate_inputs = fixed_read_inputs(&plan, RecallPresentation::EvidenceActionHint);
        let current_fingerprint = plan.shared_identity.reader_config_sha256.clone();
        plan.shared_identity.reader_config_sha256 = sha256(
            &serde_json::to_vec(&(
                "recall-read/v1",
                RecallCaptureMode::FixedReply,
                &candidate_inputs,
            ))
            .expect("serialize candidate registration"),
        );
        assert!(
            RecallRegistration::new(RecallCaptureMode::FixedReply, Some(candidate_inputs), &plan,)
                .is_ok()
        );

        plan.shared_identity.reader_config_sha256 = current_fingerprint;
        assert!(
            RecallRegistration::new(
                RecallCaptureMode::FixedReply,
                Some(fixed_read_inputs(
                    &plan,
                    RecallPresentation::EvidenceActionHint,
                )),
                &plan,
            )
            .is_err()
        );
    }

    fn runtime(
        binary: &Path,
        endpoint: &str,
        claim_rollout_stage: String,
    ) -> ChatCompletionsArmRuntime {
        ChatCompletionsArmRuntime {
            memory_server_binary: binary.to_path_buf(),
            endpoint: endpoint.into(),
            bearer_token: None,
            temp_root: None,
            claim_rollout_stage,
        }
    }

    #[test]
    fn runner_preflight_accepts_a_hash_pinned_provider_neutral_local_plan() {
        let (plan, manifest, binary) = preregistered_inputs();
        let (incumbent_stage, candidate_stage) = plan.claim_rollout_stages(None).unwrap();

        let backend = ChatCompletionsPairedBackend::new(
            &plan,
            &manifest,
            runtime(&binary, "http://127.0.0.1:1234/v1", incumbent_stage),
            runtime(&binary, "http://127.0.0.1:1234/v1", candidate_stage),
            ChatCompletionsSampling::default(),
        );

        assert!(backend.is_ok());
    }

    #[test]
    fn runner_preflight_rejects_an_endpoint_that_differs_from_the_frozen_plan() {
        let (mut plan, manifest, binary) = preregistered_inputs();
        plan.shared_identity
            .provider_fingerprints
            .insert("chat_completions_endpoint_config".into(), "0".repeat(64));
        let (incumbent_stage, candidate_stage) = plan.claim_rollout_stages(None).unwrap();

        let result = ChatCompletionsPairedBackend::new(
            &plan,
            &manifest,
            runtime(&binary, "http://127.0.0.1:1234/v1", incumbent_stage),
            runtime(&binary, "http://127.0.0.1:1234/v1", candidate_stage),
            ChatCompletionsSampling::default(),
        );

        assert!(result.is_err());
    }

    #[test]
    fn runner_preflight_rejects_sampler_settings_missing_from_the_frozen_plan() {
        let (plan, manifest, binary) = preregistered_inputs();
        let (incumbent_stage, candidate_stage) = plan.claim_rollout_stages(None).unwrap();

        let result = ChatCompletionsPairedBackend::new(
            &plan,
            &manifest,
            runtime(&binary, "http://127.0.0.1:1234/v1", incumbent_stage),
            runtime(&binary, "http://127.0.0.1:1234/v1", candidate_stage),
            crate::paired_agent::chat_completions::ChatCompletionsSampling {
                temperature: 1.0,
                top_p: Some(0.95),
                top_k: Some(20),
                ..ChatCompletionsSampling::default()
            },
        );

        assert!(result.is_err());
    }

    #[test]
    fn runner_preflight_accepts_sampler_settings_pinned_by_the_frozen_plan() {
        let (mut plan, manifest, binary) = preregistered_inputs();
        let sampling = ChatCompletionsSampling {
            temperature: 1.0,
            top_p: Some(0.95),
            top_k: Some(20),
            ..ChatCompletionsSampling::default()
        };
        plan.shared_identity.provider_fingerprints.insert(
            "chat_completions_sampling_config".into(),
            sampling.fingerprint_sha256().unwrap(),
        );
        let (incumbent_stage, candidate_stage) = plan.claim_rollout_stages(None).unwrap();

        let result = ChatCompletionsPairedBackend::new(
            &plan,
            &manifest,
            runtime(&binary, "http://127.0.0.1:1234/v1", incumbent_stage),
            runtime(&binary, "http://127.0.0.1:1234/v1", candidate_stage),
            sampling,
        );

        assert!(result.is_ok());
    }

    #[test]
    fn runner_preflight_rejects_a_response_format_missing_from_the_frozen_plan() {
        let (plan, manifest, binary) = preregistered_inputs();
        let (incumbent_stage, candidate_stage) = plan.claim_rollout_stages(None).unwrap();
        let response_format = serde_json::json!({"type":"json_object"});

        let result = ChatCompletionsPairedBackend::new_with_response_format(
            &plan,
            &manifest,
            runtime(&binary, "http://127.0.0.1:1234/v1", incumbent_stage),
            runtime(&binary, "http://127.0.0.1:1234/v1", candidate_stage),
            ChatCompletionsSampling::default(),
            Some(response_format),
        );

        assert!(result.is_err());
    }

    #[test]
    fn runner_preflight_accepts_only_the_response_format_registered_by_the_frozen_plan() {
        let (mut plan, manifest, binary) = preregistered_inputs();
        let response_format = serde_json::json!({"type":"json_object"});
        plan.shared_identity.provider_fingerprints.insert(
            "chat_completions_response_format_config".into(),
            response_format_config_sha256(&response_format).unwrap(),
        );
        let (incumbent_stage, candidate_stage) = plan.claim_rollout_stages(None).unwrap();

        let result = ChatCompletionsPairedBackend::new_with_response_format(
            &plan,
            &manifest,
            runtime(&binary, "http://127.0.0.1:1234/v1", incumbent_stage),
            runtime(&binary, "http://127.0.0.1:1234/v1", candidate_stage),
            ChatCompletionsSampling::default(),
            Some(response_format),
        );

        assert!(result.is_ok());
    }

    #[test]
    fn runner_preflight_accepts_only_request_parameters_registered_by_the_frozen_plan() {
        let (mut plan, manifest, binary) = preregistered_inputs();
        let request_parameters = serde_json::json!({
            "chat_template_kwargs": {"enable_thinking": false}
        });
        plan.shared_identity.provider_fingerprints.insert(
            "chat_completions_request_parameters_config".into(),
            request_parameters_config_sha256(&request_parameters).unwrap(),
        );
        let (incumbent_stage, candidate_stage) = plan.claim_rollout_stages(None).unwrap();

        let result = ChatCompletionsPairedBackend::new_with_request_parameters(
            &plan,
            &manifest,
            runtime(&binary, "http://127.0.0.1:1234/v1", incumbent_stage),
            runtime(&binary, "http://127.0.0.1:1234/v1", candidate_stage),
            ChatCompletionsSampling::default(),
            None,
            Some(request_parameters),
        );

        assert!(result.is_ok());
    }

    #[test]
    fn runner_preflight_rejects_request_parameters_that_differ_from_the_frozen_plan() {
        let (mut plan, manifest, binary) = preregistered_inputs();
        let registered = serde_json::json!({
            "chat_template_kwargs": {"enable_thinking": false}
        });
        plan.shared_identity.provider_fingerprints.insert(
            "chat_completions_request_parameters_config".into(),
            request_parameters_config_sha256(&registered).unwrap(),
        );
        let active = serde_json::json!({
            "chat_template_kwargs": {"enable_thinking": true}
        });
        let (incumbent_stage, candidate_stage) = plan.claim_rollout_stages(None).unwrap();

        let result = ChatCompletionsPairedBackend::new_with_request_parameters(
            &plan,
            &manifest,
            runtime(&binary, "http://127.0.0.1:1234/v1", incumbent_stage),
            runtime(&binary, "http://127.0.0.1:1234/v1", candidate_stage),
            ChatCompletionsSampling::default(),
            None,
            Some(active),
        );

        assert!(result.is_err());
    }

    #[test]
    fn runner_preflight_rejects_a_plan_response_format_when_the_active_config_is_missing() {
        let (mut plan, manifest, binary) = preregistered_inputs();
        let response_format = serde_json::json!({"type":"json_object"});
        plan.shared_identity.provider_fingerprints.insert(
            "chat_completions_response_format_config".into(),
            response_format_config_sha256(&response_format).unwrap(),
        );
        let (incumbent_stage, candidate_stage) = plan.claim_rollout_stages(None).unwrap();

        let result = ChatCompletionsPairedBackend::new_with_response_format(
            &plan,
            &manifest,
            runtime(&binary, "http://127.0.0.1:1234/v1", incumbent_stage),
            runtime(&binary, "http://127.0.0.1:1234/v1", candidate_stage),
            ChatCompletionsSampling::default(),
            None,
        );

        assert!(result.is_err());
    }

    #[test]
    fn runner_preflight_rejects_a_response_format_that_differs_from_the_frozen_plan() {
        let (mut plan, manifest, binary) = preregistered_inputs();
        let registered = serde_json::json!({"type":"json_object"});
        plan.shared_identity.provider_fingerprints.insert(
            "chat_completions_response_format_config".into(),
            response_format_config_sha256(&registered).unwrap(),
        );
        let active = serde_json::json!({
            "type":"json_schema",
            "json_schema":{"name":"answer","schema":{"type":"object"}}
        });
        let (incumbent_stage, candidate_stage) = plan.claim_rollout_stages(None).unwrap();

        let result = ChatCompletionsPairedBackend::new_with_response_format(
            &plan,
            &manifest,
            runtime(&binary, "http://127.0.0.1:1234/v1", incumbent_stage),
            runtime(&binary, "http://127.0.0.1:1234/v1", candidate_stage),
            ChatCompletionsSampling::default(),
            Some(active),
        );

        assert!(result.is_err());
    }

    #[test]
    fn runner_preflight_rejects_sampler_settings_that_differ_from_the_frozen_plan() {
        let (mut plan, manifest, binary) = preregistered_inputs();
        let registered = ChatCompletionsSampling {
            temperature: 1.0,
            top_p: Some(0.95),
            top_k: Some(20),
            ..ChatCompletionsSampling::default()
        };
        plan.shared_identity.provider_fingerprints.insert(
            "chat_completions_sampling_config".into(),
            registered.fingerprint_sha256().unwrap(),
        );
        let active = ChatCompletionsSampling {
            top_k: Some(21),
            ..registered
        };
        let (incumbent_stage, candidate_stage) = plan.claim_rollout_stages(None).unwrap();

        let result = ChatCompletionsPairedBackend::new(
            &plan,
            &manifest,
            runtime(&binary, "http://127.0.0.1:1234/v1", incumbent_stage),
            runtime(&binary, "http://127.0.0.1:1234/v1", candidate_stage),
            active,
        );

        assert!(result.is_err());
    }

    #[test]
    fn runner_preflight_rejects_a_plan_without_an_endpoint_fingerprint() {
        let (mut plan, manifest, binary) = preregistered_inputs();
        plan.shared_identity
            .provider_fingerprints
            .remove("chat_completions_endpoint_config");
        let (incumbent_stage, candidate_stage) = plan.claim_rollout_stages(None).unwrap();

        let result = ChatCompletionsPairedBackend::new(
            &plan,
            &manifest,
            runtime(&binary, "http://127.0.0.1:1234/v1", incumbent_stage),
            runtime(&binary, "http://127.0.0.1:1234/v1", candidate_stage),
            ChatCompletionsSampling::default(),
        );

        assert!(result.is_err());
    }

    #[test]
    fn runner_preflight_rejects_a_codex_pinned_plan() {
        let (mut plan, manifest, binary) = preregistered_inputs();
        plan.shared_identity.agent_runner = "codex-cli".into();
        let (incumbent_stage, candidate_stage) = plan.claim_rollout_stages(None).unwrap();

        let result = ChatCompletionsPairedBackend::new(
            &plan,
            &manifest,
            runtime(&binary, "http://127.0.0.1:1234/v1", incumbent_stage),
            runtime(&binary, "http://127.0.0.1:1234/v1", candidate_stage),
            ChatCompletionsSampling::default(),
        );

        assert!(result.is_err());
    }

    #[test]
    fn frozen_v1_local_pilot_is_rejected_after_the_recall_prompt_fingerprint_changes() {
        let mut plan: PairedEvaluationPlan = serde_json::from_str(include_str!(
            "../../../../evals/paired/source-authority-local-chat-completions-v1/plan.json"
        ))
        .expect("registered local pilot plan parses");
        let manifest: AgentManagedTaskManifest = serde_json::from_str(include_str!(
            "../../../../evals/paired/source-authority-local-chat-completions-v1/tasks-v1.json"
        ))
        .expect("registered local pilot tasks parse");
        let protocol = manifest
            .fingerprints_for_chat_completions()
            .expect("registered local pilot follows the provider-neutral protocol");
        assert_eq!(
            plan.shared_identity.system_prompt_sha256,
            protocol.system_prompt_sha256
        );
        assert_ne!(
            plan.shared_identity.task_prompt_sha256,
            protocol.task_prompt_sha256
        );
        assert_eq!(
            plan.shared_identity.host_policy_sha256,
            protocol.host_policy_sha256
        );
        assert_eq!(
            plan.shared_identity.retry_policy_sha256,
            protocol.retry_policy_sha256
        );
        assert_eq!(
            plan.shared_identity.reader_config_sha256,
            protocol.protocol_config_sha256
        );
        assert_eq!(
            plan.shared_identity.simulator_sha256,
            protocol.simulator_sha256
        );
        let binary = std::env::current_exe().expect("resolve test executable");
        let binary_sha256 = hash_file(&binary).expect("hash test executable");
        plan.shared_identity
            .provider_fingerprints
            .insert("memory_eval_cli".into(), binary_sha256.clone());
        plan.incumbent_product
            .binary_sha256
            .clone_from(&binary_sha256);
        plan.candidate_product
            .binary_sha256
            .clone_from(&binary_sha256);
        let (incumbent_stage, candidate_stage) = plan.claim_rollout_stages(None).unwrap();

        let result = ChatCompletionsPairedBackend::new(
            &plan,
            &manifest,
            runtime(&binary, "http://127.0.0.1:1234/v1", incumbent_stage),
            runtime(&binary, "http://127.0.0.1:1234/v1", candidate_stage),
            ChatCompletionsSampling::default(),
        );

        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("v1 plan must not run with the v2 prompt"),
        };
        assert!(
            error
                .to_string()
                .contains("prompt and runner-policy hashes")
        );
    }
}
