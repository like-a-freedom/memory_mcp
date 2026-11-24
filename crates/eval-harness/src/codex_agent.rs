//! Optional Codex CLI adapter for paired memory-agent trials. Read-only is the default;
//! write-enabled profiles are explicit, scoped and identity-pinned.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::EvalError;
use crate::paired::{AgentTraceEvent, NormalizedErrorClass, TraceEventKind, TrialStatus};
use crate::paired_agent::ExtractFactEvidenceObservation;
use crate::paired_agent::tool_results::{
    collect_ingested_episode_id, collect_inline_extracted_episode_id, collect_source_episode_ids,
    has_complete_fact_evidence,
};

pub use crate::paired_agent::AgentObservation as CodexAgentObservation;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodexMemoryToolProfile {
    #[default]
    RecallOnly,
    AgentManagedMemory,
    AgentFormation,
    /// Paired workflow with legacy agent file ingestion and operator setup.
    AgentSourceAuthority,
    /// Paired workflow with literal source text for incumbent and operator import for candidate.
    AgentSourceAuthorityLiteralInput,
    /// Internal phase profile for read-only verification after operator setup.
    AgentOperatorPrepared,
    AgentMaintenance,
    AgentMaintenanceV2,
    AgentMaintenanceV3,
    AgentMaintenanceV4,
}

impl CodexMemoryToolProfile {
    pub fn from_name(value: &str) -> Result<Self, EvalError> {
        match value {
            "recall_only" => Ok(Self::RecallOnly),
            "agent_managed_memory" => Ok(Self::AgentManagedMemory),
            "agent_source_authority" => Ok(Self::AgentSourceAuthority),
            "agent_source_authority_literal_input" => Ok(Self::AgentSourceAuthorityLiteralInput),
            "agent_formation" => Ok(Self::AgentFormation),
            "agent_maintenance" => Ok(Self::AgentMaintenance),
            "agent_maintenance_v2" => Ok(Self::AgentMaintenanceV2),
            "agent_maintenance_v3" => Ok(Self::AgentMaintenanceV3),
            "agent_maintenance_v4" => Ok(Self::AgentMaintenanceV4),
            _ => Err(EvalError::InvalidConfig(
                "unsupported Codex memory tool profile".into(),
            )),
        }
    }

    pub const fn enabled_tools(self) -> &'static [&'static str] {
        match self {
            Self::RecallOnly => &["assemble_context"],
            Self::AgentManagedMemory => &["assemble_context", "ingest", "extract", "invalidate"],
            Self::AgentFormation => &["ingest", "extract"],
            Self::AgentSourceAuthority | Self::AgentSourceAuthorityLiteralInput => {
                &["assemble_context", "ingest", "extract", "invalidate"]
            }
            Self::AgentOperatorPrepared => &["assemble_context"],
            Self::AgentMaintenance => &["assemble_context", "ingest", "extract", "invalidate"],
            Self::AgentMaintenanceV2 => &["assemble_context", "ingest", "extract", "invalidate"],
            Self::AgentMaintenanceV3 => &["assemble_context", "extract", "invalidate"],
            Self::AgentMaintenanceV4 => &["assemble_context", "extract", "invalidate"],
        }
    }

    const fn enabled_tools_config(self) -> &'static str {
        match self {
            Self::RecallOnly => "[\"assemble_context\"]",
            Self::AgentManagedMemory => {
                "[\"assemble_context\",\"ingest\",\"extract\",\"invalidate\"]"
            }
            Self::AgentFormation => "[\"ingest\",\"extract\"]",
            Self::AgentSourceAuthority | Self::AgentSourceAuthorityLiteralInput => {
                "[\"assemble_context\",\"ingest\",\"extract\",\"invalidate\"]"
            }
            Self::AgentOperatorPrepared => "[\"assemble_context\"]",
            Self::AgentMaintenance => {
                "[\"assemble_context\",\"ingest\",\"extract\",\"invalidate\"]"
            }
            Self::AgentMaintenanceV2 => {
                "[\"assemble_context\",\"ingest\",\"extract\",\"invalidate\"]"
            }
            Self::AgentMaintenanceV3 => "[\"assemble_context\",\"extract\",\"invalidate\"]",
            Self::AgentMaintenanceV4 => "[\"assemble_context\",\"extract\",\"invalidate\"]",
        }
    }

    const fn system_instructions(self) -> &'static str {
        match self {
            Self::RecallOnly => SYSTEM_INSTRUCTIONS,
            Self::AgentManagedMemory => AGENT_MEMORY_SYSTEM_INSTRUCTIONS,
            Self::AgentFormation => AGENT_FORMATION_SYSTEM_INSTRUCTIONS,
            Self::AgentSourceAuthority | Self::AgentSourceAuthorityLiteralInput => {
                AGENT_MEMORY_SYSTEM_INSTRUCTIONS
            }
            Self::AgentOperatorPrepared => SYSTEM_INSTRUCTIONS,
            Self::AgentMaintenance => AGENT_MAINTENANCE_SYSTEM_INSTRUCTIONS,
            Self::AgentMaintenanceV2 => AGENT_MAINTENANCE_SYSTEM_INSTRUCTIONS,
            Self::AgentMaintenanceV3 => AGENT_MAINTENANCE_SYSTEM_INSTRUCTIONS,
            Self::AgentMaintenanceV4 => AGENT_MAINTENANCE_SYSTEM_INSTRUCTIONS,
        }
    }

    const fn task_rules(self) -> &'static str {
        match self {
            Self::RecallOnly => TASK_RULES,
            Self::AgentManagedMemory => AGENT_MEMORY_TASK_RULES,
            Self::AgentFormation => AGENT_FORMATION_TASK_RULES,
            Self::AgentSourceAuthority | Self::AgentSourceAuthorityLiteralInput => {
                AGENT_MEMORY_TASK_RULES
            }
            Self::AgentOperatorPrepared => TASK_RULES,
            Self::AgentMaintenance => AGENT_MAINTENANCE_TASK_RULES,
            Self::AgentMaintenanceV2 => AGENT_MAINTENANCE_TASK_RULES,
            Self::AgentMaintenanceV3 => AGENT_MAINTENANCE_V3_TASK_RULES,
            Self::AgentMaintenanceV4 => AGENT_MAINTENANCE_V4_TASK_RULES,
        }
    }

    const fn assemble_context_output_token_limit(self) -> Option<u64> {
        match self {
            Self::RecallOnly
            | Self::AgentManagedMemory
            | Self::AgentSourceAuthority
            | Self::AgentSourceAuthorityLiteralInput
            | Self::AgentOperatorPrepared
            | Self::AgentMaintenance => Some(MEMORY_READER_OUTPUT_TOKEN_LIMIT),
            Self::AgentMaintenanceV2 => Some(AGENT_MAINTENANCE_V2_OUTPUT_TOKEN_LIMIT),
            Self::AgentMaintenanceV3 => Some(AGENT_MAINTENANCE_V2_OUTPUT_TOKEN_LIMIT),
            Self::AgentMaintenanceV4 => Some(AGENT_MAINTENANCE_V4_OUTPUT_TOKEN_LIMIT),
            Self::AgentFormation => None,
        }
    }

    /// Resolve the formation-phase profile for a registered managed workflow.
    pub fn formation_profile_for_arm(
        self,
        arm: crate::paired::RunArm,
        operator_prepared: bool,
    ) -> Option<Self> {
        match (self, arm, operator_prepared) {
            (Self::AgentSourceAuthority, crate::paired::RunArm::Incumbent, _) => {
                Some(Self::AgentFormation)
            }
            (Self::AgentSourceAuthority, crate::paired::RunArm::Candidate, true) => {
                Some(Self::AgentOperatorPrepared)
            }
            (Self::AgentSourceAuthority, crate::paired::RunArm::Candidate, false) => {
                Some(Self::AgentFormation)
            }
            (Self::AgentSourceAuthorityLiteralInput, crate::paired::RunArm::Incumbent, _) => {
                Some(Self::AgentFormation)
            }
            (Self::AgentSourceAuthorityLiteralInput, crate::paired::RunArm::Candidate, true) => {
                Some(Self::AgentOperatorPrepared)
            }
            (Self::AgentSourceAuthorityLiteralInput, crate::paired::RunArm::Candidate, false) => {
                Some(Self::AgentFormation)
            }
            (Self::AgentManagedMemory, _, false) => Some(Self::AgentFormation),
            (Self::AgentManagedMemory, _, true) => None,
            (
                Self::AgentMaintenance
                | Self::AgentMaintenanceV2
                | Self::AgentMaintenanceV3
                | Self::AgentMaintenanceV4,
                _,
                false,
            ) => Some(self),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct CodexAgentConfig {
    pub codex_binary: PathBuf,
    pub codex_home: PathBuf,
    pub home_directory: PathBuf,
    pub model: String,
    pub claim_rollout_stage: String,
    pub memory_server_binary: PathBuf,
    pub database_path: PathBuf,
    pub working_directory: PathBuf,
    /// Optional working directory for the stdio MCP child. This can differ
    /// from Codex's empty workspace when a registered legacy source import
    /// must resolve a fixture path without exposing files to the agent.
    pub memory_server_working_directory: Option<PathBuf>,
    pub context_budget_tokens: u64,
    pub tool_call_budget: u32,
    pub turn_budget: u32,
    pub output_token_budget: u64,
    pub wall_time_budget: Duration,
    pub max_jsonl_bytes: usize,
    pub memory_transport: CodexMemoryTransport,
    /// Explicitly pinned tool surface; read-only remains the default.
    pub memory_tool_profile: CodexMemoryToolProfile,
    /// Optional non-interactive Codex credential. Passed only to the Codex
    /// child process and never included in arguments or evaluation artifacts.
    pub codex_api_key: Option<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexMemoryTransport {
    Stdio,
    StreamableHttp { url: String, bearer_token: String },
}

const MAX_STDERR_CHUNK_BYTES: usize = 8192;
const STDOUT_CHUNK_BYTES: usize = 8192;
const MAX_PROMPT_BYTES: usize = 1_048_576;
// This is the pinned reader-config limit. The full model input budget also
// includes Codex instructions, task text, tool schemas, and prior turns.
const MEMORY_READER_OUTPUT_TOKEN_LIMIT: u64 = 8192;
const AGENT_MAINTENANCE_V2_OUTPUT_TOKEN_LIMIT: u64 = 2048;
const SYSTEM_INSTRUCTIONS: &str = crate::paired_agent::prompts::RECALL_SYSTEM_INSTRUCTIONS;
const TASK_RULES: &str = crate::paired_agent::prompts::RECALL_TASK_RULES;
const AGENT_MEMORY_SYSTEM_INSTRUCTIONS: &str =
    include_str!("../../../evals/paired/prompts/agent-managed-memory-system-v1.txt");
const AGENT_MEMORY_TASK_RULES: &str =
    include_str!("../../../evals/paired/prompts/agent-managed-memory-task-rules-v1.txt");
const AGENT_FORMATION_SYSTEM_INSTRUCTIONS: &str =
    crate::paired_agent::prompts::FORMATION_SYSTEM_INSTRUCTIONS;
const AGENT_FORMATION_TASK_RULES: &str = crate::paired_agent::prompts::FORMATION_TASK_RULES;
const AGENT_SOURCE_AUTHORITY_HOST_POLICY: &[u8] =
    include_bytes!("../../../evals/paired/source-authority-agent-workflow-v1/host-policy.json");
const AGENT_SOURCE_AUTHORITY_LITERAL_INPUT_HOST_POLICY: &[u8] = include_bytes!(
    "../../../evals/paired/source-authority-agent-workflow-literal-input-v1/host-policy.json"
);
const AGENT_MAINTENANCE_SYSTEM_INSTRUCTIONS: &str =
    include_str!("../../../evals/paired/prompts/agent-memory-maintenance-system-v1.txt");
const AGENT_MAINTENANCE_TASK_RULES: &str =
    include_str!("../../../evals/paired/prompts/agent-memory-maintenance-task-rules-v1.txt");
const AGENT_MAINTENANCE_V3_TASK_RULES: &str =
    include_str!("../../../evals/paired/prompts/agent-memory-maintenance-task-rules-v2.txt");
const AGENT_MAINTENANCE_V4_TASK_RULES: &str =
    include_str!("../../../evals/paired/prompts/agent-memory-maintenance-task-rules-v3.txt");
const AGENT_MEMORY_PROTOCOL_CONFIG: &[u8] =
    include_bytes!("../../../evals/paired/agent-managed-memory-v1/protocol-config.json");
const AGENT_MEMORY_HOST_POLICY: &[u8] =
    include_bytes!("../../../evals/paired/agent-managed-memory-v1/host-policy.json");
const AGENT_MEMORY_RETRY_POLICY: &[u8] =
    include_bytes!("../../../evals/paired/agent-managed-memory-v1/retry-policy.json");
const AGENT_MAINTENANCE_PROTOCOL_CONFIG: &[u8] =
    include_bytes!("../../../evals/paired/agent-managed-maintenance-v1/protocol-config.json");
const AGENT_MAINTENANCE_HOST_POLICY: &[u8] =
    include_bytes!("../../../evals/paired/agent-managed-maintenance-v1/host-policy.json");
const AGENT_MAINTENANCE_SIMULATOR: &[u8] =
    include_bytes!("../../../evals/paired/agent-managed-maintenance-v1/simulator.json");
const AGENT_MAINTENANCE_V2_PROTOCOL_CONFIG: &[u8] =
    include_bytes!("../../../evals/paired/agent-managed-maintenance-v2/protocol-config.json");
const AGENT_MAINTENANCE_V2_HOST_POLICY: &[u8] =
    include_bytes!("../../../evals/paired/agent-managed-maintenance-v2/host-policy.json");
const AGENT_MAINTENANCE_V2_SIMULATOR: &[u8] =
    include_bytes!("../../../evals/paired/agent-managed-maintenance-v2/simulator.json");
const AGENT_MAINTENANCE_V3_PROTOCOL_CONFIG: &[u8] =
    include_bytes!("../../../evals/paired/agent-managed-maintenance-v3/protocol-config.json");
const AGENT_MAINTENANCE_V3_HOST_POLICY: &[u8] =
    include_bytes!("../../../evals/paired/agent-managed-maintenance-v3/host-policy.json");
const AGENT_MAINTENANCE_V3_SIMULATOR: &[u8] =
    include_bytes!("../../../evals/paired/agent-managed-maintenance-v3/simulator.json");
const AGENT_MAINTENANCE_V4_PROTOCOL_CONFIG: &[u8] =
    include_bytes!("../../../evals/paired/agent-managed-maintenance-v4/protocol-config.json");
const AGENT_MAINTENANCE_V4_HOST_POLICY: &[u8] =
    include_bytes!("../../../evals/paired/agent-managed-maintenance-v4/host-policy.json");
const AGENT_MAINTENANCE_V4_SIMULATOR: &[u8] =
    include_bytes!("../../../evals/paired/agent-managed-maintenance-v4/simulator.json");

const AGENT_MAINTENANCE_V4_OUTPUT_TOKEN_LIMIT: u64 = 1024;

pub fn agent_maintenance_simulator_sha256() -> String {
    sha256(AGENT_MAINTENANCE_SIMULATOR)
}

pub fn agent_maintenance_v2_host_policy_sha256() -> String {
    sha256(AGENT_MAINTENANCE_V2_HOST_POLICY)
}

pub fn agent_maintenance_v2_simulator_sha256() -> String {
    sha256(AGENT_MAINTENANCE_V2_SIMULATOR)
}

pub fn agent_maintenance_v3_host_policy_sha256() -> String {
    sha256(AGENT_MAINTENANCE_V3_HOST_POLICY)
}

pub fn agent_maintenance_v3_simulator_sha256() -> String {
    sha256(AGENT_MAINTENANCE_V3_SIMULATOR)
}

pub fn agent_maintenance_v4_host_policy_sha256() -> String {
    sha256(AGENT_MAINTENANCE_V4_HOST_POLICY)
}

pub fn agent_maintenance_v4_simulator_sha256() -> String {
    sha256(AGENT_MAINTENANCE_V4_SIMULATOR)
}

enum StdoutMessage {
    Chunk(Vec<u8>),
    Eof,
    Error,
}

#[derive(Default)]
struct BudgetMonitor {
    seen_turns: u32,
    seen_tool_calls: BTreeSet<String>,
    output_tokens: u64,
    exceeded: bool,
}

impl BudgetMonitor {
    fn observe_line(&mut self, line: &[u8], config: &CodexAgentConfig) {
        let Ok(event) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        match event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "turn.started" => {
                self.seen_turns = self.seen_turns.saturating_add(1);
                self.exceeded |= self.seen_turns > config.turn_budget;
            }
            "item.started" | "item.completed" => {
                if let Some(item) = event.get("item")
                    && item.get("type").and_then(Value::as_str) == Some("mcp_tool_call")
                    && let Some(id) = item.get("id").and_then(Value::as_str)
                {
                    self.seen_tool_calls.insert(id.to_string());
                    self.exceeded |= self.seen_tool_calls.len()
                        > usize::try_from(config.tool_call_budget).unwrap_or(usize::MAX);
                }
            }
            "turn.completed" | "turn.failed" => {
                if let Some(usage) = event.get("usage") {
                    let input = usage.get("input_tokens").and_then(Value::as_u64);
                    let output = usage.get("output_tokens").and_then(Value::as_u64);
                    if input.is_some_and(|tokens| tokens > config.context_budget_tokens) {
                        self.exceeded = true;
                    }
                    if let Some(output) = output {
                        self.output_tokens = self.output_tokens.saturating_add(output);
                        self.exceeded |= self.output_tokens > config.output_token_budget;
                    }
                }
            }
            _ => {}
        }
    }
}

pub fn run_codex_agent(
    config: &CodexAgentConfig,
    task_prompt: &str,
) -> Result<CodexAgentObservation, EvalError> {
    validate_invocation_config(config)?;
    let schema_path = config.working_directory.join("codex-output-schema.json");
    let instructions_path = config
        .working_directory
        .join("codex-system-instructions.txt");
    let input_files_guard =
        TemporaryInputFilesGuard(vec![schema_path.clone(), instructions_path.clone()]);
    write_output_schema(&schema_path)?;
    write_system_instructions(&instructions_path, config.memory_tool_profile)?;
    let mut command = build_codex_command(config, &schema_path, &instructions_path);
    let started = Instant::now();
    let deadline = started + config.wall_time_budget;
    let mut child = command.spawn().map_err(|source| EvalError::Io {
        path: config.codex_binary.clone(),
        source,
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| EvalError::Suite("Codex CLI stdout was not piped".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| EvalError::Suite("Codex CLI stderr was not piped".into()))?;
    let (stdout_sender, stdout_receiver) = mpsc::sync_channel(8);
    let stdout_reader = thread::spawn(move || stream_stdout(stdout, stdout_sender));
    let stderr_reader = thread::spawn(move || drain_stderr(stderr));

    let prompt = compose_task_prompt(task_prompt, config.memory_tool_profile);
    if prompt.len() > MAX_PROMPT_BYTES {
        terminate_child(&mut child);
        join_reader(stdout_reader);
        join_reader(stderr_reader);
        drop(input_files_guard);
        return Err(EvalError::InvalidInput(
            "Codex task prompt exceeds the configured byte limit".into(),
        ));
    }
    let write_result = child
        .stdin
        .take()
        .ok_or_else(|| EvalError::Suite("Codex CLI stdin was not piped".into()))?
        .write_all(prompt.as_bytes());
    if let Err(error) = write_result {
        terminate_child(&mut child);
        join_reader(stdout_reader);
        join_reader(stderr_reader);
        drop(input_files_guard);
        return Err(EvalError::Suite(format!(
            "failed to pass task prompt to Codex CLI: {error}"
        )));
    }

    let mut jsonl = Vec::new();
    let mut line = Vec::new();
    let mut output_bytes = 0_usize;
    let mut monitor = BudgetMonitor::default();
    let mut stdout_done = false;
    let mut exit_status = None;
    let mut terminal_status = None;
    let mut process_observation_failed = false;
    while !stdout_done || exit_status.is_none() {
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(10));
        match stdout_receiver.recv_timeout(wait) {
            Ok(StdoutMessage::Chunk(chunk)) if terminal_status.is_none() => {
                terminal_status = append_stdout_chunk(
                    &chunk,
                    &mut jsonl,
                    &mut line,
                    &mut output_bytes,
                    config,
                    &mut monitor,
                );
            }
            Ok(StdoutMessage::Chunk(_)) => {}
            Ok(StdoutMessage::Eof) => {
                stdout_done = true;
                if terminal_status.is_none() && !line.is_empty() {
                    jsonl.extend_from_slice(&line);
                    monitor.observe_line(&line, config);
                    if monitor.exceeded {
                        terminal_status = Some(TrialStatus::BudgetExceeded);
                    }
                }
                line.clear();
            }
            Ok(StdoutMessage::Error) => {
                stdout_done = true;
                process_observation_failed = true;
                if terminal_status.is_none() {
                    terminal_status = Some(TrialStatus::ModelFailure);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) if stdout_done => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                stdout_done = true;
                process_observation_failed = true;
                if terminal_status.is_none() {
                    terminal_status = Some(TrialStatus::ModelFailure);
                }
            }
        }

        if terminal_status.is_none() && Instant::now() >= deadline {
            terminal_status = Some(TrialStatus::TimedOut);
        }
        if terminal_status.is_some() && exit_status.is_none() {
            exit_status = kill_and_reap(&mut child);
        } else if exit_status.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => exit_status = Some(status),
                Ok(None) => {}
                Err(_) => {
                    process_observation_failed = true;
                    terminal_status = Some(TrialStatus::ModelFailure);
                    exit_status = kill_and_reap(&mut child);
                }
            }
        }
    }
    let elapsed_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    join_reader(stdout_reader);
    join_reader(stderr_reader);
    drop(input_files_guard);

    let terminal_status = terminal_status.or_else(|| {
        (!exit_status.is_some_and(|status| status.success())).then_some(TrialStatus::ModelFailure)
    });
    let jsonl = String::from_utf8(jsonl)
        .map_err(|_| EvalError::InvalidInput("Codex CLI JSONL output was not UTF-8".into()))?;
    let mut observation = match terminal_status {
        Some(status) => {
            parse_codex_jsonl_attempt_with_profile(&jsonl, status, config.memory_tool_profile)?
        }
        None => parse_codex_jsonl_with_profile(&jsonl, config.memory_tool_profile)?,
    };
    observation.wall_time_ms = elapsed_ms;
    if process_observation_failed && observation.status == TrialStatus::Completed {
        observation.status = TrialStatus::ModelFailure;
    }
    Ok(observation)
}

fn build_codex_command(
    config: &CodexAgentConfig,
    output_schema: &std::path::Path,
    system_instructions: &std::path::Path,
) -> Command {
    let mut command = Command::new(&config.codex_binary);
    command
        .arg("exec")
        .arg("--ignore-user-config")
        .arg("--ignore-rules")
        .arg("--strict-config")
        .arg("--ephemeral")
        .arg("--json")
        .arg("--sandbox")
        .arg("read-only")
        .arg("--skip-git-repo-check")
        .arg("--color")
        .arg("never")
        .arg("--model")
        .arg(&config.model)
        .arg("--cd")
        .arg(&config.working_directory)
        .arg("--output-schema")
        .arg(output_schema);
    for feature in [
        "shell_tool",
        "apps",
        "plugins",
        "memories",
        "request_permissions_tool",
    ] {
        command.arg("--disable").arg(feature);
    }
    let mut mcp_config = match &config.memory_transport {
        CodexMemoryTransport::Stdio => vec![
            (
                "mcp_servers.memory_decisions.command",
                toml_string(&config.memory_server_binary),
            ),
            ("mcp_servers.memory_decisions.args", "[\"serve\"]".into()),
            (
                "mcp_servers.memory_decisions.cwd",
                toml_string(
                    config
                        .memory_server_working_directory
                        .as_deref()
                        .unwrap_or(&config.working_directory),
                ),
            ),
            (
                "mcp_servers.memory_decisions.env",
                server_environment(config),
            ),
            ("mcp_servers.memory_decisions.env_vars", "[]".into()),
        ],
        CodexMemoryTransport::StreamableHttp { url, .. } => vec![
            ("mcp_servers.memory_decisions.url", toml_string_value(url)),
            (
                "mcp_servers.memory_decisions.bearer_token_env_var",
                toml_string_value("MEMORY_MCP_EVAL_HTTP_BEARER_TOKEN"),
            ),
        ],
    };
    mcp_config.extend([
        ("approval_policy", "\"on-request\"".into()),
        ("approvals_reviewer", "\"auto_review\"".into()),
        ("model_provider", "\"openai\"".into()),
        ("model_reasoning_effort", "\"medium\"".into()),
        ("model_instructions_file", toml_string(system_instructions)),
        ("mcp_servers.memory_decisions.enabled", "true".into()),
        ("mcp_servers.memory_decisions.required", "true".into()),
        (
            "mcp_servers.memory_decisions.enabled_tools",
            config.memory_tool_profile.enabled_tools_config().into(),
        ),
        (
            "mcp_servers.memory_decisions.default_tools_approval_mode",
            "\"auto\"".into(),
        ),
        (
            "mcp_servers.memory_decisions.startup_timeout_sec",
            "30".into(),
        ),
        (
            "mcp_servers.memory_decisions.tool_timeout_sec",
            config.wall_time_budget.as_secs().max(1).to_string(),
        ),
        ("web_search", "\"disabled\"".into()),
    ]);
    if let Some(output_token_limit) = config
        .memory_tool_profile
        .assemble_context_output_token_limit()
    {
        mcp_config.push((
            "mcp_servers.memory_decisions.tools.assemble_context.output_token_limit",
            output_token_limit.to_string(),
        ));
    }
    for (key, value) in mcp_config {
        command.arg("--config").arg(format!("{key}={value}"));
    }
    command
        .env_clear()
        .env("HOME", &config.home_directory)
        .env("CODEX_HOME", &config.codex_home)
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("TMPDIR", std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let CodexMemoryTransport::StreamableHttp { bearer_token, .. } = &config.memory_transport {
        command.env("MEMORY_MCP_EVAL_HTTP_BEARER_TOKEN", bearer_token);
    }
    if let Some(api_key) = &config.codex_api_key {
        command.env("CODEX_API_KEY", api_key);
    }
    command
}

fn validate_invocation_config(config: &CodexAgentConfig) -> Result<(), EvalError> {
    validate_enabled_memory_tools(config.memory_tool_profile.enabled_tools())?;
    for (label, path) in [
        ("Codex executable", &config.codex_binary),
        ("memory server executable", &config.memory_server_binary),
        ("Codex home", &config.codex_home),
        ("home directory", &config.home_directory),
        ("database directory", &config.database_path),
        ("isolated working directory", &config.working_directory),
    ] {
        if !path.is_absolute() {
            return Err(EvalError::InvalidConfig(format!(
                "{label} path must be absolute"
            )));
        }
    }
    if config
        .memory_server_working_directory
        .as_ref()
        .is_some_and(|path| !path.is_absolute())
    {
        return Err(EvalError::InvalidConfig(
            "memory server working directory path must be absolute".into(),
        ));
    }
    if !config.codex_binary.is_file() || !config.memory_server_binary.is_file() {
        return Err(EvalError::InvalidConfig(
            "Codex and memory server paths must point to executable files".into(),
        ));
    }
    if !config.codex_home.is_dir()
        || !config.home_directory.is_dir()
        || !config.database_path.is_dir()
        || !config.working_directory.is_dir()
        || config
            .memory_server_working_directory
            .as_ref()
            .is_some_and(|path| !path.is_dir())
    {
        return Err(EvalError::InvalidConfig(
            "Codex, home, database, and working paths must be existing directories".into(),
        ));
    }
    if config.model.trim().is_empty()
        || config.context_budget_tokens == 0
        || config.tool_call_budget == 0
        || config.turn_budget == 0
        || config.output_token_budget == 0
        || config.wall_time_budget.is_zero()
        || config.max_jsonl_bytes == 0
    {
        return Err(EvalError::InvalidConfig(
            "model and Codex execution budgets must be nonempty and positive".into(),
        ));
    }
    if !matches!(
        config.claim_rollout_stage.as_str(),
        "disabled" | "shadow" | "relations" | "evidence"
    ) {
        return Err(EvalError::InvalidConfig(
            "Codex memory server claim rollout stage is unsupported".into(),
        ));
    }
    if let CodexMemoryTransport::StreamableHttp { url, bearer_token } = &config.memory_transport {
        let parsed = reqwest::Url::parse(url).map_err(|_| {
            EvalError::InvalidConfig("HTTP MCP endpoint must be a valid absolute URL".into())
        })?;
        let loopback_host = parsed.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        });
        if parsed.scheme() != "http"
            || !loopback_host
            || parsed.path() != "/mcp"
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || bearer_token.trim().is_empty()
        {
            return Err(EvalError::InvalidConfig(
                "paired HTTP MCP must use a loopback /mcp URL and a separate bearer token".into(),
            ));
        }
    }
    if std::fs::read_dir(&config.working_directory)
        .map_err(|source| EvalError::Io {
            path: config.working_directory.clone(),
            source,
        })?
        .next()
        .is_some()
    {
        return Err(EvalError::InvalidConfig(
            "Codex working directory must be empty and isolated".into(),
        ));
    }
    let workspace = config
        .working_directory
        .canonicalize()
        .map_err(|source| EvalError::Io {
            path: config.working_directory.clone(),
            source,
        })?;
    let database = config
        .database_path
        .canonicalize()
        .map_err(|source| EvalError::Io {
            path: config.database_path.clone(),
            source,
        })?;
    if database.starts_with(&workspace) || workspace.starts_with(&database) {
        return Err(EvalError::InvalidConfig(
            "Codex workspace and memory database must be separate directories".into(),
        ));
    }
    if let Some(memory_server_working_directory) = config.memory_server_working_directory.as_deref()
    {
        let server_workspace =
            memory_server_working_directory
                .canonicalize()
                .map_err(|source| EvalError::Io {
                    path: memory_server_working_directory.to_path_buf(),
                    source,
                })?;
        if database.starts_with(&server_workspace) || server_workspace.starts_with(&database) {
            return Err(EvalError::InvalidConfig(
                "memory server working directory and database must be separate directories".into(),
            ));
        }
    }
    Ok(())
}

fn server_environment(config: &CodexAgentConfig) -> String {
    let values = [
        ("SURREALDB_EMBEDDED", "true".to_string()),
        (
            "SURREALDB_DATA_DIR",
            config.database_path.display().to_string(),
        ),
        ("SURREALDB_DB_NAME", "memory_decisions".to_string()),
        ("SURREALDB_NAMESPACE", "main".to_string()),
        ("SURREALDB_USERNAME", "root".to_string()),
        ("SURREALDB_PASSWORD", "root".to_string()),
        ("EMBEDDINGS_ENABLED", "false".to_string()),
        ("NER_EXTRACTOR", "regex".to_string()),
        (
            "MEMORY_CLAIM_ROLLOUT_STAGE",
            config.claim_rollout_stage.clone(),
        ),
        ("RUST_LOG", "warn".to_string()),
    ];
    format!(
        "{{ {} }}",
        values
            .into_iter()
            .map(|(key, value)| format!("{key} = {}", toml_string_value(&value)))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn toml_string(path: &std::path::Path) -> String {
    toml_string_value(&path.display().to_string())
}

fn toml_string_value(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
}

fn write_output_schema(path: &std::path::Path) -> Result<(), EvalError> {
    const SCHEMA: &str = r#"{"type":"object","properties":{"answer":{"type":"string"},"citations":{"type":"array","items":{"type":"string"}},"needs_clarification":{"type":"boolean"}},"required":["answer","citations","needs_clarification"],"additionalProperties":false}"#;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| EvalError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(SCHEMA.as_bytes())
        .map_err(|source| EvalError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(())
}

fn write_system_instructions(
    path: &std::path::Path,
    profile: CodexMemoryToolProfile,
) -> Result<(), EvalError> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| EvalError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(profile.system_instructions().as_bytes())
        .map_err(|source| EvalError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(())
}

fn compose_task_prompt(task_prompt: &str, profile: CodexMemoryToolProfile) -> String {
    format!("{task_prompt}\n\n{}", profile.task_rules())
}

/// SHA-256 of the exact Codex base instructions pinned by the paired harness.
pub fn system_instructions_sha256() -> String {
    sha256(SYSTEM_INSTRUCTIONS.as_bytes())
}

pub fn system_instructions_sha256_for_profile(profile: CodexMemoryToolProfile) -> String {
    if matches!(
        profile,
        CodexMemoryToolProfile::AgentSourceAuthority
            | CodexMemoryToolProfile::AgentSourceAuthorityLiteralInput
    ) {
        return crate::paired_agent::prompts::source_authority_system_prompt_sha256();
    }
    sha256(profile.system_instructions().as_bytes())
}

pub fn system_instructions_for_profile(profile: CodexMemoryToolProfile) -> &'static str {
    profile.system_instructions()
}

/// SHA-256 of the exact instruction suffix appended to each case prompt.
pub fn task_rules_sha256() -> String {
    sha256(TASK_RULES.as_bytes())
}

pub fn task_rules_sha256_for_profile(profile: CodexMemoryToolProfile) -> String {
    if matches!(
        profile,
        CodexMemoryToolProfile::AgentSourceAuthority
            | CodexMemoryToolProfile::AgentSourceAuthorityLiteralInput
    ) {
        return crate::paired_agent::prompts::source_authority_task_prompt_sha256();
    }
    sha256(profile.task_rules().as_bytes())
}

pub fn task_rules_for_profile(profile: CodexMemoryToolProfile) -> &'static str {
    profile.task_rules()
}

pub fn reader_config_sha256_for_profile(profile: CodexMemoryToolProfile) -> Option<String> {
    match profile {
        CodexMemoryToolProfile::RecallOnly => None,
        CodexMemoryToolProfile::AgentOperatorPrepared => None,
        CodexMemoryToolProfile::AgentSourceAuthority => {
            Some(crate::paired_agent::prompts::source_authority_protocol_config_sha256())
        }
        CodexMemoryToolProfile::AgentSourceAuthorityLiteralInput => Some(
            crate::paired_agent::prompts::source_authority_literal_input_protocol_config_sha256(),
        ),
        CodexMemoryToolProfile::AgentManagedMemory | CodexMemoryToolProfile::AgentFormation => {
            Some(sha256(AGENT_MEMORY_PROTOCOL_CONFIG))
        }
        CodexMemoryToolProfile::AgentMaintenance => Some(sha256(AGENT_MAINTENANCE_PROTOCOL_CONFIG)),
        CodexMemoryToolProfile::AgentMaintenanceV2 => {
            Some(sha256(AGENT_MAINTENANCE_V2_PROTOCOL_CONFIG))
        }
        CodexMemoryToolProfile::AgentMaintenanceV3 => {
            Some(sha256(AGENT_MAINTENANCE_V3_PROTOCOL_CONFIG))
        }
        CodexMemoryToolProfile::AgentMaintenanceV4 => {
            Some(sha256(AGENT_MAINTENANCE_V4_PROTOCOL_CONFIG))
        }
    }
}

pub fn agent_managed_host_policy_sha256() -> String {
    sha256(AGENT_MEMORY_HOST_POLICY)
}

pub fn agent_source_authority_host_policy_sha256() -> String {
    sha256(AGENT_SOURCE_AUTHORITY_HOST_POLICY)
}

pub fn agent_source_authority_literal_input_host_policy_sha256() -> String {
    sha256(AGENT_SOURCE_AUTHORITY_LITERAL_INPUT_HOST_POLICY)
}

pub fn agent_source_authority_simulator_sha256() -> String {
    crate::paired_agent::prompts::source_authority_simulator_sha256()
}

pub fn agent_source_authority_literal_input_simulator_sha256() -> String {
    crate::paired_agent::prompts::source_authority_literal_input_simulator_sha256()
}

pub fn agent_maintenance_host_policy_sha256() -> String {
    sha256(AGENT_MAINTENANCE_HOST_POLICY)
}

pub fn agent_managed_retry_policy_sha256() -> String {
    sha256(AGENT_MEMORY_RETRY_POLICY)
}

pub fn agent_managed_system_prompt_sha256() -> String {
    hash_phase_prompts(
        system_instructions_sha256_for_profile(CodexMemoryToolProfile::AgentFormation),
        system_instructions_sha256_for_profile(CodexMemoryToolProfile::RecallOnly),
    )
}

pub fn agent_managed_task_prompt_sha256() -> String {
    hash_phase_prompts(
        task_rules_sha256_for_profile(CodexMemoryToolProfile::AgentFormation),
        task_rules_sha256_for_profile(CodexMemoryToolProfile::RecallOnly),
    )
}

pub fn agent_maintenance_system_prompt_sha256() -> String {
    hash_phase_prompts(
        system_instructions_sha256_for_profile(CodexMemoryToolProfile::AgentMaintenance),
        system_instructions_sha256_for_profile(CodexMemoryToolProfile::RecallOnly),
    )
}

pub fn agent_maintenance_task_prompt_sha256() -> String {
    hash_phase_prompts(
        task_rules_sha256_for_profile(CodexMemoryToolProfile::AgentMaintenance),
        task_rules_sha256_for_profile(CodexMemoryToolProfile::RecallOnly),
    )
}

pub fn agent_maintenance_v2_system_prompt_sha256() -> String {
    hash_phase_prompts(
        system_instructions_sha256_for_profile(CodexMemoryToolProfile::AgentMaintenanceV2),
        system_instructions_sha256_for_profile(CodexMemoryToolProfile::RecallOnly),
    )
}

pub fn agent_maintenance_v2_task_prompt_sha256() -> String {
    hash_phase_prompts(
        task_rules_sha256_for_profile(CodexMemoryToolProfile::AgentMaintenanceV2),
        task_rules_sha256_for_profile(CodexMemoryToolProfile::RecallOnly),
    )
}

pub fn agent_maintenance_v3_system_prompt_sha256() -> String {
    hash_phase_prompts(
        system_instructions_sha256_for_profile(CodexMemoryToolProfile::AgentMaintenanceV3),
        system_instructions_sha256_for_profile(CodexMemoryToolProfile::RecallOnly),
    )
}

pub fn agent_maintenance_v3_task_prompt_sha256() -> String {
    hash_phase_prompts(
        task_rules_sha256_for_profile(CodexMemoryToolProfile::AgentMaintenanceV3),
        task_rules_sha256_for_profile(CodexMemoryToolProfile::RecallOnly),
    )
}

pub fn agent_maintenance_v4_system_prompt_sha256() -> String {
    hash_phase_prompts(
        system_instructions_sha256_for_profile(CodexMemoryToolProfile::AgentMaintenanceV4),
        system_instructions_sha256_for_profile(CodexMemoryToolProfile::RecallOnly),
    )
}

pub fn agent_maintenance_v4_task_prompt_sha256() -> String {
    hash_phase_prompts(
        task_rules_sha256_for_profile(CodexMemoryToolProfile::AgentMaintenanceV4),
        task_rules_sha256_for_profile(CodexMemoryToolProfile::RecallOnly),
    )
}

fn hash_phase_prompts(formation_sha256: String, recall_sha256: String) -> String {
    let mut digest = Sha256::new();
    digest.update(b"memory-mcp-agent-managed-prompt-phases/v1\0");
    digest.update(b"formation\0");
    digest.update(formation_sha256.as_bytes());
    digest.update(b"\0recall\0");
    digest.update(recall_sha256.as_bytes());
    hex::encode(digest.finalize())
}

struct TemporaryInputFilesGuard(Vec<PathBuf>);

impl Drop for TemporaryInputFilesGuard {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn stream_stdout(mut stdout: impl Read, sender: mpsc::SyncSender<StdoutMessage>) {
    let mut chunk = [0_u8; STDOUT_CHUNK_BYTES];
    loop {
        let count = match stdout.read(&mut chunk) {
            Ok(count) => count,
            Err(_) => {
                let _ = sender.send(StdoutMessage::Error);
                return;
            }
        };
        if count == 0 {
            let _ = sender.send(StdoutMessage::Eof);
            break;
        }
        if sender
            .send(StdoutMessage::Chunk(chunk[..count].to_vec()))
            .is_err()
        {
            break;
        }
    }
}

fn append_stdout_chunk(
    chunk: &[u8],
    jsonl: &mut Vec<u8>,
    line: &mut Vec<u8>,
    output_bytes: &mut usize,
    config: &CodexAgentConfig,
    monitor: &mut BudgetMonitor,
) -> Option<TrialStatus> {
    *output_bytes = output_bytes.saturating_add(chunk.len());
    if *output_bytes > config.max_jsonl_bytes {
        return Some(TrialStatus::BudgetExceeded);
    }
    for byte in chunk {
        line.push(*byte);
        if *byte == b'\n' {
            jsonl.extend_from_slice(line);
            monitor.observe_line(line, config);
            line.clear();
            if monitor.exceeded {
                return Some(TrialStatus::BudgetExceeded);
            }
        }
    }
    None
}

fn kill_and_reap(child: &mut Child) -> Option<ExitStatus> {
    let _ = child.kill();
    child.wait().ok()
}

fn drain_stderr(mut stderr: impl Read) {
    let mut buffer = [0_u8; MAX_STDERR_CHUNK_BYTES];
    while stderr.read(&mut buffer).is_ok_and(|count| count > 0) {}
}

fn terminate_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn join_reader(reader: thread::JoinHandle<()>) {
    let _ = reader.join();
}

/// Parses Codex `exec --json` JSONL while retaining only its final answer in memory
/// and metadata plus digests in the returned trace.
pub fn parse_codex_jsonl(jsonl: &str) -> Result<CodexAgentObservation, EvalError> {
    parse_codex_jsonl_with_profile(jsonl, CodexMemoryToolProfile::RecallOnly)
}

pub fn parse_codex_jsonl_attempt(
    jsonl: &str,
    terminal_status: TrialStatus,
) -> Result<CodexAgentObservation, EvalError> {
    if !matches!(
        terminal_status,
        TrialStatus::TimedOut | TrialStatus::ModelFailure | TrialStatus::BudgetExceeded
    ) {
        return Err(EvalError::InvalidInput(
            "partial Codex attempts require a timeout, model_failure, or budget_exceeded status"
                .into(),
        ));
    }
    parse_codex_jsonl_attempt_with_profile(
        jsonl,
        terminal_status,
        CodexMemoryToolProfile::RecallOnly,
    )
}

fn parse_codex_jsonl_with_profile(
    jsonl: &str,
    profile: CodexMemoryToolProfile,
) -> Result<CodexAgentObservation, EvalError> {
    parse_codex_jsonl_with_enabled_memory_tools(jsonl, profile.enabled_tools())
}

fn parse_codex_jsonl_attempt_with_profile(
    jsonl: &str,
    terminal_status: TrialStatus,
    profile: CodexMemoryToolProfile,
) -> Result<CodexAgentObservation, EvalError> {
    if !matches!(
        terminal_status,
        TrialStatus::TimedOut | TrialStatus::ModelFailure | TrialStatus::BudgetExceeded
    ) {
        return Err(EvalError::InvalidInput(
            "partial Codex attempts require a timeout, model_failure, or budget_exceeded status"
                .into(),
        ));
    }
    parse_codex_jsonl_with_terminal_status(jsonl, Some(terminal_status), profile.enabled_tools())
}

fn parse_codex_jsonl_with_enabled_memory_tools(
    jsonl: &str,
    enabled_tools: &[&str],
) -> Result<CodexAgentObservation, EvalError> {
    validate_enabled_memory_tools(enabled_tools)?;
    parse_codex_jsonl_with_terminal_status(jsonl, None, enabled_tools)
}

fn validate_enabled_memory_tools(enabled_tools: &[&str]) -> Result<(), EvalError> {
    const SUPPORTED_TOOLS: &[&str] = &["assemble_context", "ingest", "extract", "invalidate"];
    let unique = enabled_tools.iter().collect::<BTreeSet<_>>();
    let has_read = enabled_tools.contains(&"assemble_context");
    let has_ingest = enabled_tools.contains(&"ingest");
    let has_extract = enabled_tools.contains(&"extract");
    let has_invalidate = enabled_tools.contains(&"invalidate");
    if enabled_tools.is_empty()
        || (!has_read && !has_extract)
        || (has_ingest && !has_extract)
        || (has_invalidate && (!has_read || !has_extract))
        || unique.len() != enabled_tools.len()
        || enabled_tools
            .iter()
            .any(|tool| !SUPPORTED_TOOLS.contains(tool))
    {
        return Err(EvalError::InvalidConfig(
            "memory tool profile must enable retrieval or inline extraction; ingestion and invalidation require extraction, invalidation also requires retrieval, and tools must be unique and supported".into(),
        ));
    }
    Ok(())
}

fn parse_codex_jsonl_with_terminal_status(
    jsonl: &str,
    terminal_status: Option<TrialStatus>,
    enabled_tools: &[&str],
) -> Result<CodexAgentObservation, EvalError> {
    let mut trace = Vec::new();
    let mut pending = BTreeMap::<String, String>::new();
    let mut pending_extract_fact_evidence = BTreeMap::<String, usize>::new();
    let mut extract_fact_evidence = Vec::new();
    let mut completed = BTreeSet::<String>::new();
    let mut final_answer = None;
    use crate::paired_agent::{MemoryReadObservation, MemoryReadOutcome, ReadCaptureStatus};
    let mut memory_reads = Vec::<MemoryReadObservation>::new();
    let mut pending_reads = BTreeMap::new();
    let mut read_capture_status = ReadCaptureStatus::Observed;
    let mut memory_source_episode_ids = BTreeSet::new();
    let mut memory_write_episode_ids = BTreeSet::new();
    let mut turns_used = 0_u32;
    let mut tool_calls = 0_u32;
    let mut input_tokens = 0_u64;
    let mut output_tokens = 0_u64;
    let mut turn_failures = 0_u32;
    let mut clarification_count = 0_u32;
    let mut tool_failure = false;
    let mut saw_turn = false;
    let mut saw_terminal_failure = false;

    let lines = jsonl.lines().collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let event: Value = match serde_json::from_str(line) {
            Ok(event) => event,
            Err(_) if terminal_status.is_some() && index + 1 == lines.len() => break,
            Err(_) => {
                return Err(EvalError::InvalidInput(format!(
                    "Codex JSONL event on line {} is malformed",
                    index + 1
                )));
            }
        };
        let event_type = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match event_type {
            "turn.started" => {
                if saw_turn {
                    return Err(EvalError::InvalidInput(
                        "Codex started a new turn before the prior turn terminated".into(),
                    ));
                }
                turns_used = turns_used
                    .checked_add(1)
                    .ok_or_else(|| EvalError::InvalidInput("Codex turn count overflowed".into()))?;
                saw_turn = true;
                push_trace(
                    &mut trace,
                    turns_used,
                    TraceEventKind::AgentTurnStarted,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )?;
            }
            "item.started" | "item.completed" => {
                let Some(item) = event.get("item") else {
                    continue;
                };
                match item.get("type").and_then(Value::as_str).unwrap_or_default() {
                    "mcp_tool_call" => {
                        if turns_used == 0 || !saw_turn {
                            return Err(EvalError::InvalidInput(
                                "Codex emitted a tool event before a model turn".into(),
                            ));
                        }
                        let call_id = required_string(item, "id", "MCP item ID")?;
                        let server = required_string(item, "server", "MCP server name")?;
                        let tool = required_string(item, "tool", "MCP tool name")?;
                        if server != "memory_decisions" || !enabled_tools.contains(&tool.as_str()) {
                            return Err(EvalError::InvalidInput(
                                "Codex called a tool outside the registered memory tool profile"
                                    .into(),
                            ));
                        }
                        let trace_kind = if tool == "assemble_context" {
                            TraceEventKind::MemoryRead
                        } else {
                            TraceEventKind::MemoryWrite
                        };
                        let tool_name = format!("{server}/{tool}");
                        if !pending.contains_key(&call_id) {
                            if completed.contains(&call_id) {
                                return Err(EvalError::InvalidInput(
                                    "Codex reused a completed MCP item ID".into(),
                                ));
                            }
                            let arguments = item.get("arguments").ok_or_else(|| {
                                EvalError::InvalidInput(
                                    "Codex MCP tool event omitted its arguments".into(),
                                )
                            })?;
                            tool_calls = tool_calls.checked_add(1).ok_or_else(|| {
                                EvalError::InvalidInput("Codex tool-call count overflowed".into())
                            })?;
                            push_trace(
                                &mut trace,
                                turns_used,
                                trace_kind,
                                Some(call_id.clone()),
                                Some(tool_name.clone()),
                                None,
                                Some(hash_json(arguments)?),
                                None,
                                None,
                            )?;
                            if tool == "extract" {
                                let request_sequence =
                                    trace.last().map(|event| event.sequence).ok_or_else(|| {
                                        EvalError::InvalidInput(
                                            "Codex extract request was not recorded".into(),
                                        )
                                    })?;
                                let evidence_index = extract_fact_evidence.len();
                                extract_fact_evidence.push(ExtractFactEvidenceObservation {
                                    request_sequence,
                                    response_sequence: None,
                                    requested: arguments
                                        .get("include_fact_evidence")
                                        .and_then(Value::as_bool)
                                        .unwrap_or(false),
                                    returned: false,
                                    response_wire_bytes: None,
                                });
                                pending_extract_fact_evidence
                                    .insert(call_id.clone(), evidence_index);
                            }
                            if tool == "assemble_context" {
                                if event_type == "item.started" {
                                    let request_sequence = trace
                                        .last()
                                        .ok_or_else(|| {
                                            EvalError::InvalidInput(
                                                "Codex read request was not recorded".into(),
                                            )
                                        })?
                                        .sequence;
                                    pending_reads.insert(
                                        call_id.clone(),
                                        (
                                            memory_reads.len(),
                                            crate::paired_agent::tool_results::requested_cutoff(
                                                arguments,
                                            ),
                                        ),
                                    );
                                    memory_reads.push(MemoryReadObservation {
                                        request_sequence,
                                        response_sequence: None,
                                        outcome: MemoryReadOutcome::Unknown,
                                        item_count: None,
                                        evidence: None,
                                    });
                                } else {
                                    read_capture_status = ReadCaptureStatus::Partial;
                                }
                            }
                            pending.insert(call_id.clone(), tool_name.clone());
                        }
                        if event_type == "item.completed" {
                            let result = item.get("result");
                            let error = item.get("error").filter(|error| !error.is_null());
                            let result_is_error = result
                                .and_then(|result| result.get("isError"))
                                .and_then(Value::as_bool)
                                == Some(true);
                            let is_error = result_is_error
                                || error.is_some()
                                || item.get("status").and_then(Value::as_str) == Some("failed");
                            if result.is_some() || is_error {
                                let expected_tool = pending.remove(&call_id).ok_or_else(|| {
                                    EvalError::InvalidInput(
                                        "Codex completed an MCP call that was not pending".into(),
                                    )
                                })?;
                                if expected_tool != tool_name || !completed.insert(call_id.clone())
                                {
                                    return Err(EvalError::InvalidInput(
                                        "Codex MCP call completion does not match its request"
                                            .into(),
                                    ));
                                }
                                if is_error {
                                    tool_failure = true;
                                    let error_payload = result_is_error
                                        .then_some(result)
                                        .flatten()
                                        .or(error)
                                        .or(result)
                                        .ok_or_else(|| {
                                            EvalError::InvalidInput(
                                                "Codex MCP error event omitted its payload".into(),
                                            )
                                        })?;
                                    push_trace(
                                        &mut trace,
                                        turns_used,
                                        TraceEventKind::ToolError,
                                        Some(call_id.clone()),
                                        Some(tool_name),
                                        Some(classify_error(error_payload)),
                                        Some(hash_json(error_payload)?),
                                        None,
                                        None,
                                    )?;
                                    if let Some((index, _)) = pending_reads.remove(&call_id) {
                                        let read = &mut memory_reads[index];
                                        read.response_sequence =
                                            trace.last().map(|event| event.sequence);
                                        read.outcome = MemoryReadOutcome::Failed;
                                    }
                                    record_extract_fact_evidence_response(
                                        &mut extract_fact_evidence,
                                        &mut pending_extract_fact_evidence,
                                        &call_id,
                                        trace.last().map(|event| event.sequence),
                                        false,
                                        None,
                                    );
                                } else {
                                    let result = result.ok_or_else(|| {
                                        EvalError::InvalidInput(
                                            "Codex MCP success omitted its result".into(),
                                        )
                                    })?;
                                    push_trace(
                                        &mut trace,
                                        turns_used,
                                        TraceEventKind::ToolResponse,
                                        Some(call_id.clone()),
                                        Some(tool_name),
                                        None,
                                        Some(hash_json(result)?),
                                        None,
                                        None,
                                    )?;
                                    record_extract_fact_evidence_response(
                                        &mut extract_fact_evidence,
                                        &mut pending_extract_fact_evidence,
                                        &call_id,
                                        trace.last().map(|event| event.sequence),
                                        has_complete_fact_evidence(result),
                                        Some(serialized_response_bytes(result)?),
                                    );
                                    if tool == "assemble_context" {
                                        if let Some((index, requested_cutoff)) =
                                            pending_reads.remove(&call_id)
                                        {
                                            let read = &mut memory_reads[index];
                                            read.response_sequence =
                                                trace.last().map(|event| event.sequence);
                                            (read.outcome, read.item_count) =
                                                crate::paired_agent::tool_results::read_result(
                                                    result,
                                                );
                                            read.evidence =
                                                crate::paired_agent::tool_results::read_evidence(
                                                    result,
                                                    requested_cutoff,
                                                );
                                        }
                                        collect_source_episode_ids(
                                            result,
                                            &mut memory_source_episode_ids,
                                        );
                                    } else if tool == "ingest" {
                                        collect_ingested_episode_id(
                                            result,
                                            &mut memory_write_episode_ids,
                                        );
                                    } else if tool == "extract"
                                        && item.get("arguments").is_some_and(|arguments| {
                                            ["content", "text"].iter().any(|field| {
                                                arguments
                                                    .get(field)
                                                    .and_then(Value::as_str)
                                                    .is_some_and(|content| {
                                                        !content.trim().is_empty()
                                                    })
                                            })
                                        })
                                    {
                                        collect_inline_extracted_episode_id(
                                            result,
                                            &mut memory_write_episode_ids,
                                        );
                                    }
                                }
                            }
                        }
                    }
                    "agent_message" if event_type == "item.completed" => {
                        if !saw_turn {
                            return Err(EvalError::InvalidInput(
                                "Codex emitted a final message outside an active model turn".into(),
                            ));
                        }
                        if final_answer.is_some() {
                            return Err(EvalError::InvalidInput(
                                "Codex emitted more than one final agent message".into(),
                            ));
                        }
                        let text = required_string(item, "text", "final agent message")?;
                        let structured: Value = serde_json::from_str(&text).map_err(|_| {
                            EvalError::InvalidInput(
                                "Codex final message is not the required structured JSON".into(),
                            )
                        })?;
                        let needs_clarification = structured
                            .get("needs_clarification")
                            .and_then(Value::as_bool)
                            .ok_or_else(|| {
                                EvalError::InvalidInput(
                                    "Codex final message lacks needs_clarification".into(),
                                )
                            })?;
                        if structured.get("answer").and_then(Value::as_str).is_none() {
                            return Err(EvalError::InvalidInput(
                                "Codex final message lacks answer".into(),
                            ));
                        }
                        if !structured
                            .get("citations")
                            .and_then(Value::as_array)
                            .is_some_and(|citations| {
                                citations.iter().all(|citation| {
                                    citation.as_str().is_some_and(|id| !id.trim().is_empty())
                                })
                            })
                        {
                            return Err(EvalError::InvalidInput(
                                "Codex final message lacks citations".into(),
                            ));
                        }
                        if needs_clarification {
                            clarification_count = 1;
                            push_trace(
                                &mut trace,
                                turns_used.max(1),
                                TraceEventKind::Clarification,
                                None,
                                None,
                                None,
                                None,
                                None,
                                None,
                            )?;
                        }
                        push_trace(
                            &mut trace,
                            turns_used.max(1),
                            TraceEventKind::FinalAnswer,
                            None,
                            None,
                            None,
                            Some(sha256(text.as_bytes())),
                            None,
                            None,
                        )?;
                        final_answer = Some(text);
                    }
                    _ => {}
                }
            }
            "turn.completed" => {
                if !saw_turn || turns_used == 0 {
                    return Err(EvalError::InvalidInput(
                        "Codex completed a turn that was not started".into(),
                    ));
                }
                let usage = event.get("usage").ok_or_else(|| {
                    EvalError::InvalidInput("Codex completed turn omitted provider usage".into())
                })?;
                let turn_input = required_u64(usage, "input_tokens")?;
                let turn_output = required_u64(usage, "output_tokens")?;
                input_tokens = input_tokens.checked_add(turn_input).ok_or_else(|| {
                    EvalError::InvalidInput("Codex input token count overflowed".into())
                })?;
                output_tokens = output_tokens.checked_add(turn_output).ok_or_else(|| {
                    EvalError::InvalidInput("Codex output token count overflowed".into())
                })?;
                push_trace(
                    &mut trace,
                    turns_used,
                    TraceEventKind::AgentTurn,
                    None,
                    None,
                    None,
                    None,
                    Some(turn_input),
                    Some(turn_output),
                )?;
                saw_turn = false;
            }
            "turn.failed" => {
                if !saw_turn || turns_used == 0 {
                    return Err(EvalError::InvalidInput(
                        "Codex failed a turn that was not started".into(),
                    ));
                }
                let usage = event.get("usage").ok_or_else(|| {
                    EvalError::InvalidInput("Codex failed turn omitted provider usage".into())
                })?;
                let turn_input = required_u64(usage, "input_tokens")?;
                let turn_output = required_u64(usage, "output_tokens")?;
                input_tokens = input_tokens.checked_add(turn_input).ok_or_else(|| {
                    EvalError::InvalidInput("Codex input token count overflowed".into())
                })?;
                output_tokens = output_tokens.checked_add(turn_output).ok_or_else(|| {
                    EvalError::InvalidInput("Codex output token count overflowed".into())
                })?;
                push_trace(
                    &mut trace,
                    turns_used,
                    TraceEventKind::AgentTurn,
                    None,
                    None,
                    None,
                    None,
                    Some(turn_input),
                    Some(turn_output),
                )?;
                saw_terminal_failure = true;
                turn_failures = turn_failures.checked_add(1).ok_or_else(|| {
                    EvalError::InvalidInput("Codex failed-turn count overflowed".into())
                })?;
                saw_turn = false;
            }
            "error" => {
                saw_terminal_failure = true;
            }
            _ => {}
        }
    }

    if saw_turn && terminal_status.is_none() {
        return Err(EvalError::InvalidInput(
            "Codex JSONL ended before the model turn reported usage".into(),
        ));
    }
    if turns_used == 0 && terminal_status.is_none() {
        return Err(EvalError::InvalidInput(
            "Codex JSONL contains no model turn".into(),
        ));
    }
    if !pending.is_empty() && terminal_status.is_none() {
        return Err(EvalError::InvalidInput(
            "Codex JSONL ended with an unresolved MCP call".into(),
        ));
    }
    if terminal_status.is_none() && turn_failures > 0 && final_answer.is_some() {
        return Err(EvalError::InvalidInput(
            "Codex resumed after a failed turn; this retry sequence is not supported by the receipt schema".into(),
        ));
    }
    if terminal_status.is_none() && (turn_failures > 0 || saw_terminal_failure) {
        push_trace(
            &mut trace,
            turns_used.max(1),
            TraceEventKind::ModelFailure,
            None,
            None,
            None,
            None,
            None,
            None,
        )?;
    }
    let needs_final = final_answer.is_some();
    let status = if let Some(status) = terminal_status {
        let kind = match status {
            TrialStatus::TimedOut => TraceEventKind::Timeout,
            TrialStatus::ModelFailure => TraceEventKind::ModelFailure,
            TrialStatus::BudgetExceeded => TraceEventKind::BudgetLimitReached,
            _ => {
                return Err(EvalError::InvalidInput(
                    "unsupported Codex terminal status".into(),
                ));
            }
        };
        push_trace(
            &mut trace, turns_used, kind, None, None, None, None, None, None,
        )?;
        status
    } else if needs_final {
        TrialStatus::Completed
    } else if tool_failure && !saw_terminal_failure {
        TrialStatus::ToolFailure
    } else {
        TrialStatus::ModelFailure
    };
    if status == TrialStatus::ModelFailure
        && !trace.iter().any(|event| {
            matches!(
                event.kind,
                TraceEventKind::ModelFailure | TraceEventKind::Timeout
            )
        })
    {
        push_trace(
            &mut trace,
            turns_used.max(1),
            TraceEventKind::ModelFailure,
            None,
            None,
            None,
            None,
            None,
            None,
        )?;
    }
    Ok(CodexAgentObservation {
        trace,
        final_answer,
        memory_source_episode_ids: memory_source_episode_ids.into_iter().collect(),
        memory_reads: Some(memory_reads),
        read_capture_status: Some(read_capture_status),
        memory_write_episode_ids: memory_write_episode_ids.into_iter().collect(),
        extract_fact_evidence,
        extract_fact_evidence_telemetry_complete: true,
        terminal_http_status_code: None,
        terminal_transport_stage: None,
        turns_used,
        tool_calls,
        user_clarifications: clarification_count,
        input_tokens,
        output_tokens,
        wall_time_ms: 0,
        status,
    })
}

fn record_extract_fact_evidence_response(
    observations: &mut [ExtractFactEvidenceObservation],
    pending: &mut BTreeMap<String, usize>,
    call_id: &str,
    response_sequence: Option<u32>,
    returned: bool,
    response_wire_bytes: Option<u64>,
) {
    let Some(index) = pending.remove(call_id) else {
        return;
    };
    let Some(observation) = observations.get_mut(index) else {
        return;
    };
    observation.response_sequence = response_sequence;
    observation.returned = observation.requested && returned;
    observation.response_wire_bytes = response_wire_bytes;
}

fn serialized_response_bytes(value: &Value) -> Result<u64, EvalError> {
    serde_json::to_vec(value)
        .map(|bytes| u64::try_from(bytes.len()).unwrap_or(u64::MAX))
        .map_err(|_| EvalError::InvalidInput("extract response size could not be measured".into()))
}

fn required_string(object: &Value, key: &str, label: &str) -> Result<String, EvalError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| EvalError::InvalidInput(format!("Codex event omitted {label}")))
}

fn required_u64(object: &Value, key: &str) -> Result<u64, EvalError> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| EvalError::InvalidInput(format!("Codex usage omitted `{key}`")))
}

#[allow(clippy::too_many_arguments)]
fn push_trace(
    trace: &mut Vec<AgentTraceEvent>,
    turn: u32,
    kind: TraceEventKind,
    call_id: Option<String>,
    tool_name: Option<String>,
    error_class: Option<NormalizedErrorClass>,
    payload_sha256: Option<String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
) -> Result<(), EvalError> {
    let sequence = u32::try_from(trace.len() + 1)
        .map_err(|_| EvalError::InvalidInput("Codex trace exceeded sequence limit".into()))?;
    trace.push(AgentTraceEvent {
        sequence,
        turn,
        kind,
        call_id,
        tool_name,
        error_class,
        payload_sha256,
        input_tokens,
        output_tokens,
    });
    Ok(())
}

fn hash_json(value: &Value) -> Result<String, EvalError> {
    Ok(sha256(&serde_json::to_vec(value)?))
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn classify_error(value: &Value) -> NormalizedErrorClass {
    let mut text = Vec::new();
    collect_error_text(value, &mut text);
    let contains = |needle: &str| text.iter().any(|value| value.contains(needle));
    if contains("requires approval")
        || contains("approval policy is never")
        || contains("approval request")
    {
        NormalizedErrorClass::ApprovalRequired
    } else if contains("invalid argument")
        || contains("invalid_argument")
        || contains("invalid params")
        || contains("invalid_params")
        || contains("unknown field")
        || contains("missing field")
        || contains("cannot be combined")
        || contains("mutually exclusive")
    {
        NormalizedErrorClass::InvalidArguments
    } else if contains("not found") || contains("not_found") {
        NormalizedErrorClass::NotFound
    } else if contains("unauthorized") || contains("forbidden") {
        NormalizedErrorClass::Unauthorized
    } else if contains("timed out") || contains("timeout") {
        NormalizedErrorClass::Timeout
    } else if contains("rate limit") || contains("rate_limit") {
        NormalizedErrorClass::RateLimited
    } else if contains("retry") {
        NormalizedErrorClass::Retryable
    } else if contains("internal") {
        NormalizedErrorClass::Internal
    } else {
        NormalizedErrorClass::Unknown
    }
}

fn collect_error_text(value: &Value, text: &mut Vec<String>) {
    match value {
        Value::String(value) => text.push(value.to_ascii_lowercase()),
        Value::Array(values) => {
            for value in values {
                collect_error_text(value, text);
            }
        }
        Value::Object(values) => {
            for key in ["message", "text", "content", "data", "error"] {
                if let Some(value) = values.get(key) {
                    collect_error_text(value, text);
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paired::{NormalizedErrorClass, TraceEventKind};

    #[test]
    fn parses_tool_trace_usage_and_final_answer_without_persisting_payloads() {
        let jsonl = concat!(
            r#"{"type":"turn.started"}"#,
            "\n",
            r#"{"type":"item.started","item":{"id":"call-1","type":"mcp_tool_call","server":"memory_decisions","tool":"assemble_context","arguments":{"query":"Which code?"}}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"call-1","type":"mcp_tool_call","server":"memory_decisions","tool":"assemble_context","arguments":{"query":"Which code?"},"result":{"content":[{"type":"text","text":"SECRET MEMORY PAYLOAD"}]},"error":null,"status":"completed"}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"answer-1","type":"agent_message","text":"{\"answer\":\"FJ-38\",\"citations\":[\"episode-1\"],\"needs_clarification\":false}"}}"#,
            "\n",
            r#"{"type":"turn.completed","usage":{"input_tokens":120,"output_tokens":24}}"#,
            "\n"
        );

        let actual = parse_codex_jsonl(jsonl).expect("well-formed Codex event stream");

        assert_eq!(actual.status, crate::paired::TrialStatus::Completed);
        assert_eq!(actual.turns_used, 1);
        assert_eq!(actual.tool_calls, 1);
        assert_eq!(actual.input_tokens, 120);
        assert_eq!(actual.output_tokens, 24);
        assert_eq!(actual.user_clarifications, 0);
        assert_eq!(
            actual.final_answer.as_deref(),
            Some(r#"{"answer":"FJ-38","citations":["episode-1"],"needs_clarification":false}"#)
        );
        assert_eq!(actual.trace.len(), 5);
        assert_eq!(actual.trace[0].kind, TraceEventKind::AgentTurnStarted);
        assert_eq!(actual.trace[1].kind, TraceEventKind::MemoryRead);
        assert_eq!(actual.trace[2].kind, TraceEventKind::ToolResponse);
        assert_eq!(actual.trace[3].kind, TraceEventKind::FinalAnswer);
        assert_eq!(actual.trace[4].kind, TraceEventKind::AgentTurn);
        assert!(
            actual
                .trace
                .iter()
                .all(|event| { event.payload_sha256.as_deref() != Some("SECRET MEMORY PAYLOAD") })
        );
        assert!(actual.trace.iter().all(|event| event.error_class.is_none()));
    }

    #[test]
    fn records_requested_and_returned_fact_evidence_without_persisting_content() {
        let jsonl = concat!(
            r#"{"type":"turn.started"}"#,
            "\n",
            r#"{"type":"item.started","item":{"id":"extract-1","type":"mcp_tool_call","server":"memory_decisions","tool":"extract","arguments":{"content":"Correction: Cobalt uses Harbor-91.","include_fact_evidence":true}}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"extract-1","type":"mcp_tool_call","server":"memory_decisions","tool":"extract","arguments":{"content":"Correction: Cobalt uses Harbor-91.","include_fact_evidence":true},"result":{"structuredContent":{"result":{"episode_id":"episode:synthetic","facts":[{"fact_id":"fact:synthetic","statement":"Cobalt uses Harbor-91.","source_quote":"PRIVATE SOURCE QUOTE"}]}}},"error":null,"status":"completed"}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"answer-1","type":"agent_message","text":"{\"answer\":\"ok\",\"citations\":[],\"needs_clarification\":false}"}}"#,
            "\n",
            r#"{"type":"turn.completed","usage":{"input_tokens":120,"output_tokens":24}}"#,
            "\n"
        );

        let actual =
            parse_codex_jsonl_with_profile(jsonl, CodexMemoryToolProfile::AgentMaintenanceV4)
                .expect("well-formed maintenance trace");
        let serialized = serde_json::to_string(&actual).expect("observation serializes");
        let observation_json = serde_json::to_value(&actual).expect("observation serializes");
        let audit = observation_json
            .get("extract_fact_evidence")
            .and_then(serde_json::Value::as_array)
            .expect("evidence audit is present");

        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0]["requested"], true);
        assert_eq!(audit[0]["returned"], true);
        assert_eq!(audit[0]["request_sequence"], 2);
        assert_eq!(audit[0]["response_sequence"], 3);
        assert!(audit[0]["response_wire_bytes"].as_u64().unwrap_or_default() > 0);
        assert_eq!(
            observation_json["extract_fact_evidence_telemetry_complete"],
            true
        );
        assert!(!serialized.contains("PRIVATE SOURCE QUOTE"));
        assert!(!serialized.contains("Cobalt uses Harbor-91."));
    }

    #[test]
    fn does_not_count_fact_evidence_when_the_returned_quote_is_empty() {
        let jsonl = concat!(
            r#"{"type":"turn.started"}"#,
            "\n",
            r#"{"type":"item.started","item":{"id":"extract-1","type":"mcp_tool_call","server":"memory_decisions","tool":"extract","arguments":{"content":"Correction: Cobalt uses Harbor-91.","include_fact_evidence":true}}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"extract-1","type":"mcp_tool_call","server":"memory_decisions","tool":"extract","arguments":{"content":"Correction: Cobalt uses Harbor-91.","include_fact_evidence":true},"result":{"structuredContent":{"result":{"episode_id":"episode:synthetic","facts":[{"fact_id":"fact:synthetic","statement":"Cobalt uses Harbor-91.","source_quote":" "}]}}},"error":null,"status":"completed"}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"answer-1","type":"agent_message","text":"{\"answer\":\"ok\",\"citations\":[],\"needs_clarification\":false}"}}"#,
            "\n",
            r#"{"type":"turn.completed","usage":{"input_tokens":120,"output_tokens":24}}"#,
            "\n"
        );

        let actual =
            parse_codex_jsonl_with_profile(jsonl, CodexMemoryToolProfile::AgentMaintenanceV4)
                .expect("well-formed maintenance trace");
        let observation_json = serde_json::to_value(&actual).expect("observation serializes");
        let audit = observation_json
            .get("extract_fact_evidence")
            .and_then(serde_json::Value::as_array)
            .expect("evidence audit is present");

        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0]["requested"], true);
        assert_eq!(audit[0]["returned"], false);
        assert_eq!(
            observation_json["extract_fact_evidence_telemetry_complete"],
            true
        );
    }

    #[test]
    fn does_not_count_fact_evidence_without_explicit_request() {
        let jsonl = concat!(
            r#"{"type":"turn.started"}"#,
            "\n",
            r#"{"type":"item.started","item":{"id":"extract-1","type":"mcp_tool_call","server":"memory_decisions","tool":"extract","arguments":{"content":"Correction: Cobalt uses Harbor-91."}}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"extract-1","type":"mcp_tool_call","server":"memory_decisions","tool":"extract","arguments":{"content":"Correction: Cobalt uses Harbor-91."},"result":{"structuredContent":{"result":{"episode_id":"episode:synthetic","facts":[{"fact_id":"fact:synthetic","statement":"Cobalt uses Harbor-91.","source_quote":"PRIVATE SOURCE QUOTE"}]}}},"error":null,"status":"completed"}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"answer-1","type":"agent_message","text":"{\"answer\":\"ok\",\"citations\":[],\"needs_clarification\":false}"}}"#,
            "\n",
            r#"{"type":"turn.completed","usage":{"input_tokens":120,"output_tokens":24}}"#,
            "\n"
        );

        let actual =
            parse_codex_jsonl_with_profile(jsonl, CodexMemoryToolProfile::AgentMaintenanceV4)
                .expect("well-formed maintenance trace");
        let observation_json = serde_json::to_value(&actual).expect("observation serializes");
        let audit = observation_json
            .get("extract_fact_evidence")
            .and_then(serde_json::Value::as_array)
            .expect("evidence audit is present");

        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0]["requested"], false);
        assert_eq!(audit[0]["returned"], false);
        assert_eq!(
            observation_json["extract_fact_evidence_telemetry_complete"],
            true
        );
    }

    #[test]
    fn rejects_fact_evidence_trace_when_extract_response_is_missing() {
        let jsonl = concat!(
            r#"{"type":"turn.started"}"#,
            "\n",
            r#"{"type":"item.started","item":{"id":"extract-1","type":"mcp_tool_call","server":"memory_decisions","tool":"extract","arguments":{"content":"Correction: Cobalt uses Harbor-91.","include_fact_evidence":true}}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"answer-1","type":"agent_message","text":"{\"answer\":\"ok\",\"citations\":[],\"needs_clarification\":false}"}}"#,
            "\n",
            r#"{"type":"turn.completed","usage":{"input_tokens":120,"output_tokens":24}}"#,
            "\n"
        );

        let error =
            parse_codex_jsonl_with_profile(jsonl, CodexMemoryToolProfile::AgentMaintenanceV4)
                .expect_err("an extract request without a response is an invalid trace");

        assert!(error.to_string().contains("unresolved MCP call"));
    }

    #[test]
    fn classifies_tool_errors_from_mcp_text_content() {
        let error = serde_json::json!({
            "isError": true,
            "content": [{
                "type": "text",
                "text": "failed to deserialize parameters: unknown field valid_at"
            }]
        });

        assert_eq!(
            classify_error(&error),
            NormalizedErrorClass::InvalidArguments
        );
    }

    #[test]
    fn classifies_codex_mcp_approval_policy_errors() {
        let error = serde_json::json!({
            "message": "MCP tool call requires approval, but approval policy is never"
        });

        assert_eq!(
            classify_error(&error),
            NormalizedErrorClass::ApprovalRequired
        );
    }

    #[test]
    fn classifies_mcp_result_when_codex_also_emits_a_wrapper_error() {
        let jsonl = concat!(
            r#"{"type":"turn.started"}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"call-1","type":"mcp_tool_call","server":"memory_decisions","tool":"assemble_context","arguments":{"query":"A bounded synthetic task"},"result":{"isError":true,"content":[{"type":"text","text":"failed to deserialize parameters: unknown field valid_at"}]},"error":{"message":"MCP tool call failed"},"status":"failed"}}"#,
            "\n",
            r#"{"type":"turn.completed","usage":{"input_tokens":9,"output_tokens":4}}"#
        );

        let actual = parse_codex_jsonl(jsonl).expect("valid Codex MCP error event");
        let error_event = actual
            .trace
            .iter()
            .find(|event| event.kind == TraceEventKind::ToolError)
            .expect("the MCP error should be traced");

        assert_eq!(
            error_event.error_class,
            Some(NormalizedErrorClass::InvalidArguments)
        );
    }

    #[test]
    fn records_only_episode_identifiers_from_memory_results_for_hidden_scoring() {
        let jsonl = concat!(
            r#"{"type":"turn.started"}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"call-1","type":"mcp_tool_call","server":"memory_decisions","tool":"assemble_context","arguments":{"query":"Where?"},"result":{"structuredContent":{"result":[{"source_episode":"episode:one"},{"provenance":{"source_episode":"episode:two"}}]}}}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"answer-1","type":"agent_message","text":"{\"answer\":\"North City\",\"citations\":[\"episode:one\"],\"needs_clarification\":false}"}}"#,
            "\n",
            r#"{"type":"turn.completed","usage":{"input_tokens":9,"output_tokens":4}}"#
        );

        let actual = parse_codex_jsonl(jsonl).expect("valid cited Codex trace");

        assert_eq!(
            actual.memory_source_episode_ids,
            vec!["episode:one", "episode:two"]
        );
    }

    #[test]
    fn records_episode_created_by_inline_extract_for_hidden_scoring() {
        let jsonl = concat!(
            r#"{"type":"turn.started"}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"extract-1","type":"mcp_tool_call","server":"memory_decisions","tool":"extract","arguments":{"content":"Decision: use Quartz-31.","source_id":"conversation://test/correction"},"result":{"structuredContent":{"result":{"episode_id":"episode:inline-created"}}}}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"answer-1","type":"agent_message","text":"{\"answer\":\"updated\",\"citations\":[\"episode:inline-created\"],\"needs_clarification\":false}"}}"#,
            "\n",
            r#"{"type":"turn.completed","usage":{"input_tokens":9,"output_tokens":4}}"#
        );

        let actual = parse_codex_jsonl_with_enabled_memory_tools(
            jsonl,
            &["assemble_context", "extract", "invalidate"],
        )
        .expect("valid inline extraction trace");

        assert_eq!(actual.memory_write_episode_ids, ["episode:inline-created"]);
    }

    #[test]
    fn does_not_count_reextracting_an_existing_episode_as_a_new_ingest() {
        let jsonl = concat!(
            r#"{"type":"turn.started"}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"extract-1","type":"mcp_tool_call","server":"memory_decisions","tool":"extract","arguments":{"episode_id":"episode:existing"},"result":{"structuredContent":{"result":{"episode_id":"episode:existing"}}}}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"answer-1","type":"agent_message","text":"{\"answer\":\"done\",\"citations\":[],\"needs_clarification\":false}"}}"#,
            "\n",
            r#"{"type":"turn.completed","usage":{"input_tokens":9,"output_tokens":4}}"#
        );

        let actual = parse_codex_jsonl_with_enabled_memory_tools(
            jsonl,
            &["assemble_context", "extract", "invalidate"],
        )
        .expect("valid existing-episode extraction trace");

        assert!(actual.memory_write_episode_ids.is_empty());
    }

    #[test]
    fn rejects_codex_tool_calls_outside_the_read_only_memory_allow_list() {
        let jsonl = concat!(
            r#"{"type":"turn.started"}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"call-1","type":"mcp_tool_call","server":"memory_decisions","tool":"ingest","arguments":{},"result":{"content":[]},"status":"completed"}}"#,
            "\n"
        );

        let error = parse_codex_jsonl(jsonl).expect_err("write tools must be denied");

        assert!(
            error
                .to_string()
                .contains("outside the registered memory tool profile")
        );
    }

    #[test]
    fn memory_tool_profiles_are_narrow_and_reject_unknown_or_duplicate_tools() {
        assert_eq!(
            CodexMemoryToolProfile::RecallOnly.enabled_tools(),
            &["assemble_context"]
        );
        assert_eq!(
            CodexMemoryToolProfile::AgentManagedMemory.enabled_tools(),
            &["assemble_context", "ingest", "extract", "invalidate"]
        );
        assert_eq!(
            CodexMemoryToolProfile::AgentFormation.enabled_tools(),
            &["ingest", "extract"]
        );
        assert_eq!(
            CodexMemoryToolProfile::from_name("agent_maintenance")
                .expect("registered maintenance profile"),
            CodexMemoryToolProfile::AgentMaintenance
        );
        assert_eq!(
            CodexMemoryToolProfile::AgentMaintenance.enabled_tools(),
            &["assemble_context", "ingest", "extract", "invalidate"]
        );
        assert_eq!(
            CodexMemoryToolProfile::from_name("agent_maintenance_v2")
                .expect("registered v2 maintenance profile"),
            CodexMemoryToolProfile::AgentMaintenanceV2
        );
        assert_eq!(
            CodexMemoryToolProfile::AgentMaintenanceV2.enabled_tools(),
            &["assemble_context", "ingest", "extract", "invalidate"]
        );
        assert_eq!(
            CodexMemoryToolProfile::from_name("agent_maintenance_v3")
                .expect("registered v3 maintenance profile"),
            CodexMemoryToolProfile::AgentMaintenanceV3
        );
        assert_eq!(
            CodexMemoryToolProfile::AgentMaintenanceV3.enabled_tools(),
            &["assemble_context", "extract", "invalidate"]
        );
        assert_eq!(
            CodexMemoryToolProfile::from_name("agent_maintenance_v4")
                .expect("registered v4 maintenance profile"),
            CodexMemoryToolProfile::AgentMaintenanceV4
        );
        assert_eq!(
            CodexMemoryToolProfile::from_name("agent_source_authority_literal_input")
                .expect("registered literal-source workflow"),
            CodexMemoryToolProfile::AgentSourceAuthorityLiteralInput
        );
        assert_eq!(
            CodexMemoryToolProfile::AgentSourceAuthorityLiteralInput.enabled_tools(),
            &["assemble_context", "ingest", "extract", "invalidate"]
        );
        assert_eq!(
            CodexMemoryToolProfile::AgentMaintenanceV4.enabled_tools(),
            &["assemble_context", "extract", "invalidate"]
        );
        assert_ne!(
            reader_config_sha256_for_profile(CodexMemoryToolProfile::AgentMaintenance),
            reader_config_sha256_for_profile(CodexMemoryToolProfile::AgentMaintenanceV2)
        );
        assert_ne!(
            reader_config_sha256_for_profile(CodexMemoryToolProfile::AgentMaintenance),
            reader_config_sha256_for_profile(CodexMemoryToolProfile::AgentFormation)
        );
        assert_ne!(
            reader_config_sha256_for_profile(CodexMemoryToolProfile::AgentMaintenanceV2),
            reader_config_sha256_for_profile(CodexMemoryToolProfile::AgentMaintenanceV3)
        );
        assert_ne!(
            reader_config_sha256_for_profile(CodexMemoryToolProfile::AgentMaintenanceV3),
            reader_config_sha256_for_profile(CodexMemoryToolProfile::AgentMaintenanceV4)
        );
        assert!(validate_enabled_memory_tools(&["assemble_context", "ingest"]).is_err());
        assert!(validate_enabled_memory_tools(&["ingest", "extract"]).is_ok());
        assert!(validate_enabled_memory_tools(&["extract"]).is_ok());
        assert!(validate_enabled_memory_tools(&["ingest"]).is_err());
        assert!(validate_enabled_memory_tools(&["ingest", "invalidate"]).is_err());
        assert!(validate_enabled_memory_tools(&["assemble_context", "invalidate"]).is_err());
        assert!(validate_enabled_memory_tools(&["assemble_context", "ingest", "ingest"]).is_err());
        assert!(validate_enabled_memory_tools(&["assemble_context", "shell"]).is_err());
    }

    #[test]
    fn source_authority_workflow_uses_read_only_candidate_formation_after_import() {
        let profile = CodexMemoryToolProfile::from_name("agent_source_authority")
            .expect("source-authority paired workflow is registered");

        assert_eq!(
            profile.formation_profile_for_arm(crate::paired::RunArm::Incumbent, true),
            Some(CodexMemoryToolProfile::AgentFormation)
        );
        assert_eq!(
            profile.formation_profile_for_arm(crate::paired::RunArm::Candidate, true),
            Some(CodexMemoryToolProfile::AgentOperatorPrepared)
        );
        assert_eq!(
            CodexMemoryToolProfile::AgentOperatorPrepared.enabled_tools(),
            &["assemble_context"]
        );
        assert_eq!(
            profile.formation_profile_for_arm(crate::paired::RunArm::Candidate, false),
            Some(CodexMemoryToolProfile::AgentFormation)
        );
        assert_eq!(
            CodexMemoryToolProfile::AgentManagedMemory
                .formation_profile_for_arm(crate::paired::RunArm::Candidate, true),
            None
        );
    }

    #[test]
    fn literal_source_authority_workflow_uses_read_only_candidate_formation_after_import() {
        let profile = CodexMemoryToolProfile::from_name("agent_source_authority_literal_input")
            .expect("literal-source paired workflow is registered");

        assert_eq!(
            profile.formation_profile_for_arm(crate::paired::RunArm::Incumbent, true),
            Some(CodexMemoryToolProfile::AgentFormation)
        );
        assert_eq!(
            profile.formation_profile_for_arm(crate::paired::RunArm::Candidate, true),
            Some(CodexMemoryToolProfile::AgentOperatorPrepared)
        );
        assert_ne!(
            reader_config_sha256_for_profile(profile),
            reader_config_sha256_for_profile(CodexMemoryToolProfile::AgentSourceAuthority)
        );
    }

    #[test]
    fn extracts_episode_id_from_json_text_mcp_result_without_retaining_memory_text() {
        let tool_text = serde_json::json!({
            "status": "success",
            "result": "episode:writer-1",
            "guidance": "PRIVATE GUIDANCE"
        })
        .to_string();
        let jsonl = [
            serde_json::json!({"type": "turn.started"}).to_string(),
            serde_json::json!({
                "type": "item.completed",
                "item": {
                    "id": "write-1",
                    "type": "mcp_tool_call",
                    "server": "memory_decisions",
                    "tool": "ingest",
                    "arguments": {"content": "PRIVATE SOURCE CONTENT"},
                    "result": {"content": [{"type": "text", "text": tool_text} ]}
                }
            })
            .to_string(),
            serde_json::json!({
                "type": "item.completed",
                "item": {
                    "id": "answer-1",
                    "type": "agent_message",
                    "text": serde_json::json!({
                        "answer": "stored",
                        "citations": [],
                        "needs_clarification": false
                    })
                    .to_string()
                }
            })
            .to_string(),
            serde_json::json!({
                "type": "turn.completed",
                "usage": {"input_tokens": 21, "output_tokens": 7}
            })
            .to_string(),
        ]
        .join("\n");

        let actual = parse_codex_jsonl_with_enabled_memory_tools(
            &jsonl,
            &["assemble_context", "ingest", "extract"],
        )
        .expect("valid Codex MCP content-text result");

        assert_eq!(actual.memory_write_episode_ids, vec!["episode:writer-1"]);
        let serialized = serde_json::to_string(&actual).expect("serialize safe observation");
        assert!(!serialized.contains("PRIVATE SOURCE CONTENT"));
        assert!(!serialized.contains("PRIVATE GUIDANCE"));
    }

    #[test]
    fn registered_memory_write_tools_are_traced_as_writes_without_retaining_payloads() {
        let jsonl = concat!(
            r#"{"type":"turn.started"}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"write-1","type":"mcp_tool_call","server":"memory_decisions","tool":"ingest","arguments":{"content":"PRIVATE MEMORY TEXT"},"result":{"structuredContent":{"result":"episode:agent-created"}}}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"write-2","type":"mcp_tool_call","server":"memory_decisions","tool":"extract","arguments":{"episode_id":"episode:agent-created"},"result":{"structuredContent":{"result":"extracted"}}}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"read-1","type":"mcp_tool_call","server":"memory_decisions","tool":"assemble_context","arguments":{"query":"PRIVATE MEMORY TEXT"},"result":{"structuredContent":{"result":[{"source_episode":"episode:agent-created"}]}}}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"answer-1","type":"agent_message","text":"{\"answer\":\"stored\",\"citations\":[\"episode:agent-created\"],\"needs_clarification\":false}"}}"#,
            "\n",
            r#"{"type":"turn.completed","usage":{"input_tokens":21,"output_tokens":7}}"#
        );

        let actual = parse_codex_jsonl_with_enabled_memory_tools(
            jsonl,
            &["assemble_context", "ingest", "extract"],
        )
        .expect("explicitly enabled memory writes are valid");

        let writes = actual
            .trace
            .iter()
            .filter(|event| event.kind == TraceEventKind::MemoryWrite)
            .collect::<Vec<_>>();
        assert_eq!(writes.len(), 2);
        assert!(writes.iter().all(|event| event.payload_sha256.is_some()));
        assert!(
            actual
                .trace
                .iter()
                .any(|event| event.kind == TraceEventKind::MemoryRead)
        );
        assert_eq!(
            actual.memory_source_episode_ids,
            vec!["episode:agent-created"]
        );
        assert_eq!(
            actual.memory_write_episode_ids,
            vec!["episode:agent-created"]
        );
        let serialized = serde_json::to_string(&actual.trace).expect("serialize metadata trace");
        assert!(!serialized.contains("PRIVATE MEMORY TEXT"));
    }

    #[test]
    fn missing_turn_usage_is_invalid_instead_of_becoming_zero_cost() {
        let jsonl = concat!(
            r#"{"type":"turn.started"}"#,
            "\n",
            r#"{"type":"turn.completed"}"#
        );

        let error =
            parse_codex_jsonl(jsonl).expect_err("missing provider usage is not a valid receipt");

        assert!(error.to_string().contains("omitted provider usage"));
    }

    #[test]
    fn preserves_partial_trace_when_wall_time_ends_during_a_turn() {
        let jsonl = concat!(
            r#"{"type":"turn.started"}"#,
            "\n",
            r#"{"type":"item.started","item":{"id":"call-1","type":"mcp_tool_call","server":"memory_decisions","tool":"assemble_context","arguments":{"query":"Which code?"}}}"#,
            "\n"
        );

        let actual = parse_codex_jsonl_attempt(jsonl, TrialStatus::TimedOut)
            .expect("an explicitly timed-out partial attempt remains scoreable as a failure");

        assert_eq!(actual.status, TrialStatus::TimedOut);
        assert_eq!(actual.turns_used, 1);
        assert_eq!(actual.tool_calls, 1);
        assert_eq!(actual.input_tokens, 0);
        assert_eq!(actual.output_tokens, 0);
        assert_eq!(actual.trace[0].kind, TraceEventKind::AgentTurnStarted);
        assert_eq!(actual.trace[1].kind, TraceEventKind::MemoryRead);
        assert_eq!(actual.trace[2].kind, TraceEventKind::Timeout);
    }

    #[test]
    fn classifies_tool_errors_without_retaining_provider_error_text() {
        let jsonl = concat!(
            r#"{"type":"turn.started"}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"call-1","type":"mcp_tool_call","server":"memory_decisions","tool":"assemble_context","arguments":{"query":"private"},"error":{"message":"request timed out for SECRET"},"status":"failed"}}"#,
            "\n",
            r#"{"type":"turn.completed","usage":{"input_tokens":12,"output_tokens":3}}"#,
            "\n"
        );

        let actual = parse_codex_jsonl(jsonl).expect("tool error is retained as a closed class");

        assert_eq!(actual.status, crate::paired::TrialStatus::ToolFailure);
        assert_eq!(actual.tool_calls, 1);
        assert_eq!(actual.trace[2].kind, TraceEventKind::ToolError);
        assert_eq!(
            actual.trace[2].error_class,
            Some(NormalizedErrorClass::Timeout)
        );
        assert!(actual.trace[2].payload_sha256.is_some());
    }

    #[test]
    fn records_usage_for_a_failed_model_turn() {
        let jsonl = concat!(
            r#"{"type":"turn.started"}"#,
            "\n",
            r#"{"type":"turn.failed","usage":{"input_tokens":40,"output_tokens":7},"error":{"message":"provider failed"}}"#,
            "\n"
        );

        let actual = parse_codex_jsonl(jsonl).expect("failed turn still reports its resource use");

        assert_eq!(actual.status, crate::paired::TrialStatus::ModelFailure);
        assert_eq!(actual.turns_used, 1);
        assert_eq!(actual.input_tokens, 40);
        assert_eq!(actual.output_tokens, 7);
        assert_eq!(actual.trace[0].kind, TraceEventKind::AgentTurnStarted);
        assert_eq!(actual.trace[1].kind, TraceEventKind::AgentTurn);
        assert_eq!(actual.trace[2].kind, TraceEventKind::ModelFailure);
    }

    #[test]
    fn rejects_process_output_that_never_started_a_model_turn() {
        let error = parse_codex_jsonl("{}").expect_err("no attempted turn cannot form a trial");

        assert!(error.to_string().contains("no model turn"));
    }

    #[test]
    fn requires_citations_in_the_structured_final_answer() {
        let jsonl = concat!(
            r#"{"type":"turn.started"}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"answer-1","type":"agent_message","text":"{\"answer\":\"FJ-38\",\"needs_clarification\":false}"}}"#,
            "\n",
            r#"{"type":"turn.completed","usage":{"input_tokens":9,"output_tokens":4}}"#
        );

        let error = parse_codex_jsonl(jsonl).expect_err("final answer must carry citation IDs");

        assert!(error.to_string().contains("lacks citations"));
    }

    #[test]
    fn codex_invocation_isolated_to_one_read_tool_without_shell_or_web() {
        let config = CodexAgentConfig {
            codex_binary: "/tmp/eval/codex".into(),
            codex_home: "/tmp/eval/codex-home".into(),
            home_directory: "/tmp/eval/home".into(),
            model: "pinned-model".into(),
            claim_rollout_stage: "shadow".into(),
            memory_server_binary: "/tmp/eval/memory-mcp".into(),
            database_path: "/tmp/eval/database".into(),
            working_directory: "/tmp/eval/agent-workspace".into(),
            memory_server_working_directory: None,
            context_budget_tokens: 4096,
            tool_call_budget: 12,
            turn_budget: 20,
            output_token_budget: 1024,
            wall_time_budget: Duration::from_secs(30),
            max_jsonl_bytes: 1_048_576,
            memory_transport: CodexMemoryTransport::Stdio,
            memory_tool_profile: CodexMemoryToolProfile::RecallOnly,
            codex_api_key: None,
        };

        let command = build_codex_command(
            &config,
            &config.working_directory.join("schema.json"),
            &config.working_directory.join("system-instructions.txt"),
        );
        let arguments: Vec<_> = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        let joined = arguments.join(" ");

        assert!(arguments.iter().any(|arg| arg == "--ignore-user-config"));
        assert!(joined.contains("approval_policy=\"on-request\""));
        assert!(joined.contains("approvals_reviewer=\"auto_review\""));
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--sandbox", "read-only"])
        );
        assert!(!arguments.iter().any(|arg| arg == "--approve-for-me"));
        assert!(joined.contains(
            "model_instructions_file=\"/tmp/eval/agent-workspace/system-instructions.txt\""
        ));
        assert!(joined.contains("model_provider=\"openai\""));
        assert!(joined.contains("model_reasoning_effort=\"medium\""));
        assert!(joined.contains("output_token_limit=8192"));
        assert!(arguments.iter().any(|arg| arg == "--ephemeral"));
        assert!(arguments.iter().any(|arg| arg == "--json"));
        assert!(joined.contains("--disable shell_tool"));
        assert!(joined.contains("--disable apps"));
        assert!(joined.contains("--disable plugins"));
        assert!(joined.contains("--disable memories"));
        assert!(joined.contains("--disable request_permissions_tool"));
        assert!(joined.contains("web_search=\"disabled\""));
        assert!(joined.contains("enabled_tools=[\"assemble_context\"]"));
        assert!(joined.contains("env_vars=[]"));
    }

    #[test]
    fn formation_invocation_exposes_only_ingest_and_extract() {
        let config = CodexAgentConfig {
            codex_binary: "/tmp/eval/codex".into(),
            codex_home: "/tmp/eval/codex-home".into(),
            home_directory: "/tmp/eval/home".into(),
            model: "pinned-model".into(),
            claim_rollout_stage: "shadow".into(),
            memory_server_binary: "/tmp/eval/memory-mcp".into(),
            database_path: "/tmp/eval/database".into(),
            working_directory: "/tmp/eval/agent-workspace".into(),
            memory_server_working_directory: None,
            context_budget_tokens: 4096,
            tool_call_budget: 8,
            turn_budget: 8,
            output_token_budget: 1024,
            wall_time_budget: Duration::from_secs(30),
            max_jsonl_bytes: 1_048_576,
            memory_transport: CodexMemoryTransport::Stdio,
            memory_tool_profile: CodexMemoryToolProfile::AgentFormation,
            codex_api_key: None,
        };

        let command = build_codex_command(
            &config,
            &config.working_directory.join("schema.json"),
            &config.working_directory.join("system-instructions.txt"),
        );
        let arguments: Vec<_> = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        let joined = arguments.join(" ");

        assert!(joined.contains("enabled_tools=[\"ingest\",\"extract\"]"));
        assert!(!joined.contains("assemble_context"));
        assert!(!joined.contains("invalidate"));
        assert!(joined.contains("--disable shell_tool"));
        assert!(!joined.contains("--enable shell_tool"));
        assert!(!joined.contains("output_token_limit"));
    }

    #[test]
    fn maintenance_invocation_exposes_only_registered_memory_tools() {
        let config = CodexAgentConfig {
            codex_binary: "/tmp/eval/codex".into(),
            codex_home: "/tmp/eval/codex-home".into(),
            home_directory: "/tmp/eval/home".into(),
            model: "pinned-model".into(),
            claim_rollout_stage: "evidence".into(),
            memory_server_binary: "/tmp/eval/memory-mcp".into(),
            database_path: "/tmp/eval/database".into(),
            working_directory: "/tmp/eval/agent-workspace".into(),
            memory_server_working_directory: None,
            context_budget_tokens: 4096,
            tool_call_budget: 8,
            turn_budget: 8,
            output_token_budget: 1024,
            wall_time_budget: Duration::from_secs(30),
            max_jsonl_bytes: 1_048_576,
            memory_transport: CodexMemoryTransport::Stdio,
            memory_tool_profile: CodexMemoryToolProfile::AgentMaintenance,
            codex_api_key: None,
        };

        let command = build_codex_command(
            &config,
            &config.working_directory.join("schema.json"),
            &config.working_directory.join("system-instructions.txt"),
        );
        let arguments: Vec<_> = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        let joined = arguments.join(" ");

        assert!(joined.contains(
            "enabled_tools=[\"assemble_context\",\"ingest\",\"extract\",\"invalidate\"]"
        ));
        assert!(joined.contains("output_token_limit=8192"));
        assert!(joined.contains("--disable shell_tool"));
        assert!(!joined.contains("--enable shell_tool"));
        assert!(!joined.contains("file_import"));
    }

    #[test]
    fn maintenance_v2_invocation_uses_the_pinned_2048_token_reader_limit() {
        let config = CodexAgentConfig {
            codex_binary: "/tmp/eval/codex".into(),
            codex_home: "/tmp/eval/codex-home".into(),
            home_directory: "/tmp/eval/home".into(),
            model: "pinned-model".into(),
            claim_rollout_stage: "evidence".into(),
            memory_server_binary: "/tmp/eval/memory-mcp".into(),
            database_path: "/tmp/eval/database".into(),
            working_directory: "/tmp/eval/agent-workspace".into(),
            memory_server_working_directory: None,
            context_budget_tokens: 4096,
            tool_call_budget: 8,
            turn_budget: 8,
            output_token_budget: 1024,
            wall_time_budget: Duration::from_secs(30),
            max_jsonl_bytes: 1_048_576,
            memory_transport: CodexMemoryTransport::Stdio,
            memory_tool_profile: CodexMemoryToolProfile::AgentMaintenanceV2,
            codex_api_key: None,
        };

        let command = build_codex_command(
            &config,
            &config.working_directory.join("schema.json"),
            &config.working_directory.join("system-instructions.txt"),
        );
        let arguments: Vec<_> = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        let joined = arguments.join(" ");

        assert!(joined.contains("output_token_limit=2048"));
        assert!(!joined.contains("output_token_limit=8192"));
        assert!(joined.contains(
            "enabled_tools=[\"assemble_context\",\"ingest\",\"extract\",\"invalidate\"]"
        ));
        assert!(joined.contains("--disable shell_tool"));
    }

    #[test]
    fn maintenance_v3_invocation_exposes_only_tools_needed_by_inline_extract_workflow() {
        let config = CodexAgentConfig {
            codex_binary: "/tmp/eval/codex".into(),
            codex_home: "/tmp/eval/codex-home".into(),
            home_directory: "/tmp/eval/home".into(),
            model: "pinned-model".into(),
            claim_rollout_stage: "evidence".into(),
            memory_server_binary: "/tmp/eval/memory-mcp".into(),
            database_path: "/tmp/eval/database".into(),
            working_directory: "/tmp/eval/agent-workspace".into(),
            memory_server_working_directory: None,
            context_budget_tokens: 4096,
            tool_call_budget: 8,
            turn_budget: 8,
            output_token_budget: 1024,
            wall_time_budget: Duration::from_secs(30),
            max_jsonl_bytes: 1_048_576,
            memory_transport: CodexMemoryTransport::Stdio,
            memory_tool_profile: CodexMemoryToolProfile::AgentMaintenanceV3,
            codex_api_key: None,
        };

        let command = build_codex_command(
            &config,
            &config.working_directory.join("schema.json"),
            &config.working_directory.join("system-instructions.txt"),
        );
        let arguments: Vec<_> = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        let joined = arguments.join(" ");

        assert!(joined.contains("output_token_limit=2048"));
        assert!(joined.contains("enabled_tools=[\"assemble_context\",\"extract\",\"invalidate\"]"));
        assert!(!joined.contains("enabled_tools=[\"assemble_context\",\"ingest\""));
        assert!(joined.contains("--disable shell_tool"));
    }

    #[test]
    fn system_prompt_file_matches_the_registered_digest_and_is_removed_by_guard() {
        let directory = tempfile::tempdir().expect("temporary prompt directory");
        let path = directory.path().join("system-instructions.txt");

        write_system_instructions(&path, CodexMemoryToolProfile::RecallOnly)
            .expect("write pinned system instructions");
        let contents = std::fs::read(&path).expect("read pinned system instructions");
        let guard = TemporaryInputFilesGuard(vec![path.clone()]);

        assert_eq!(sha256(&contents), system_instructions_sha256());
        assert_eq!(contents, SYSTEM_INSTRUCTIONS.as_bytes());
        assert!(
            compose_task_prompt("Question", CodexMemoryToolProfile::RecallOnly)
                .ends_with(&format!("\n\n{TASK_RULES}"))
        );
        assert_eq!(task_rules_sha256(), sha256(TASK_RULES.as_bytes()));

        drop(guard);
        assert!(!path.exists());
    }

    #[test]
    fn codex_http_mcp_config_uses_loopback_url_and_keeps_bearer_out_of_arguments() {
        let token = "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_testsecret0123456789abcdef";
        let config = CodexAgentConfig {
            codex_binary: "/tmp/eval/codex".into(),
            codex_home: "/tmp/eval/codex-home".into(),
            home_directory: "/tmp/eval/home".into(),
            model: "pinned-model".into(),
            claim_rollout_stage: "shadow".into(),
            memory_server_binary: "/tmp/eval/memory-mcp-http".into(),
            database_path: "/tmp/eval/database".into(),
            working_directory: "/tmp/eval/agent-workspace".into(),
            memory_server_working_directory: None,
            context_budget_tokens: 4096,
            tool_call_budget: 12,
            turn_budget: 20,
            output_token_budget: 1024,
            wall_time_budget: Duration::from_secs(30),
            max_jsonl_bytes: 1_048_576,
            memory_transport: CodexMemoryTransport::StreamableHttp {
                url: "http://127.0.0.1:4141/mcp".into(),
                bearer_token: token.into(),
            },
            memory_tool_profile: CodexMemoryToolProfile::RecallOnly,
            codex_api_key: None,
        };

        let command = build_codex_command(
            &config,
            &config.working_directory.join("schema.json"),
            &config.working_directory.join("system-instructions.txt"),
        );
        let arguments: Vec<_> = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        let joined = arguments.join(" ");

        assert!(joined.contains("mcp_servers.memory_decisions.url=\"http://127.0.0.1:4141/mcp\""));
        assert!(joined.contains("bearer_token_env_var=\"MEMORY_MCP_EVAL_HTTP_BEARER_TOKEN\""));
        assert!(joined.contains("enabled_tools=[\"assemble_context\"]"));
        assert!(!joined.contains(token));
        assert_eq!(
            command
                .get_envs()
                .find(|(name, _)| *name == "MEMORY_MCP_EVAL_HTTP_BEARER_TOKEN")
                .and_then(|(_, value)| value),
            Some(std::ffi::OsStr::new(token))
        );
    }

    #[test]
    fn codex_api_key_is_forwarded_only_to_the_child_environment() {
        let secret = "codex-eval-secret";
        let config = CodexAgentConfig {
            codex_binary: "/tmp/eval/codex".into(),
            codex_home: "/tmp/eval/codex-home".into(),
            home_directory: "/tmp/eval/home".into(),
            model: "pinned-model".into(),
            claim_rollout_stage: "shadow".into(),
            memory_server_binary: "/tmp/eval/memory-mcp".into(),
            database_path: "/tmp/eval/database".into(),
            working_directory: "/tmp/eval/agent-workspace".into(),
            memory_server_working_directory: None,
            context_budget_tokens: 4096,
            tool_call_budget: 12,
            turn_budget: 20,
            output_token_budget: 1024,
            wall_time_budget: Duration::from_secs(30),
            max_jsonl_bytes: 1_048_576,
            memory_transport: CodexMemoryTransport::Stdio,
            memory_tool_profile: CodexMemoryToolProfile::RecallOnly,
            codex_api_key: Some(std::ffi::OsString::from(secret)),
        };

        let command = build_codex_command(
            &config,
            &config.working_directory.join("schema.json"),
            &config.working_directory.join("system-instructions.txt"),
        );
        let arguments: Vec<_> = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        let passed_key = command
            .get_envs()
            .find(|(name, _)| *name == "CODEX_API_KEY")
            .and_then(|(_, value)| value);

        assert!(!arguments.join(" ").contains(secret));
        assert_eq!(passed_key, Some(std::ffi::OsStr::new(secret)));
    }

    #[test]
    fn local_codex_home_is_used_for_auth_while_trial_home_stays_separate() {
        let config = CodexAgentConfig {
            codex_binary: "/tmp/eval/codex".into(),
            codex_home: "/Users/test/.codex".into(),
            home_directory: "/tmp/eval/home".into(),
            model: "pinned-model".into(),
            claim_rollout_stage: "shadow".into(),
            memory_server_binary: "/tmp/eval/memory-mcp".into(),
            database_path: "/tmp/eval/database".into(),
            working_directory: "/tmp/eval/agent-workspace".into(),
            memory_server_working_directory: None,
            context_budget_tokens: 4096,
            tool_call_budget: 12,
            turn_budget: 20,
            output_token_budget: 1024,
            wall_time_budget: Duration::from_secs(30),
            max_jsonl_bytes: 1_048_576,
            memory_transport: CodexMemoryTransport::Stdio,
            memory_tool_profile: CodexMemoryToolProfile::RecallOnly,
            codex_api_key: None,
        };

        let command = build_codex_command(
            &config,
            &config.working_directory.join("schema.json"),
            &config.working_directory.join("system-instructions.txt"),
        );
        assert_eq!(
            command
                .get_envs()
                .find(|(name, _)| *name == "CODEX_HOME")
                .and_then(|(_, value)| value),
            Some(std::ffi::OsStr::new("/Users/test/.codex"))
        );
        assert_eq!(
            command
                .get_envs()
                .find(|(name, _)| *name == "HOME")
                .and_then(|(_, value)| value),
            Some(std::ffi::OsStr::new("/tmp/eval/home"))
        );
        assert!(
            command
                .get_envs()
                .all(|(name, _)| name != std::ffi::OsStr::new("CODEX_API_KEY"))
        );
    }
}
