//! Optional loopback-only adapter for OpenAI-compatible Chat Completions APIs.
//!
//! Deterministic evaluation and observation scoring do not use this adapter.

use std::collections::BTreeSet;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use reqwest::{Client, Url, redirect::Policy};
use serde_json::{Value, json};

use super::{AgentObservation, ExtractFactEvidenceObservation, TerminalTransportStage, sha256};
use crate::EvalError;
use crate::paired::{AgentTraceEvent, NormalizedErrorClass, TraceEventKind, TrialStatus};

const CHAT_COMPLETIONS_PATH: &str = "chat/completions";
const MAX_MODEL_RESPONSE_BYTES: usize = 1_048_576;

/// The model runner may call only the memory tools enabled for this phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatCompletionsToolProfile {
    RecallOnly,
    SourceAuthorityFormation,
}

impl ChatCompletionsToolProfile {
    /// Resolve source-authority formation permissions from the paired workflow.
    /// The candidate writes memory only when no operator import was registered.
    pub fn source_authority_formation_for_arm(
        arm: crate::paired::RunArm,
        operator_import_registered: bool,
    ) -> Self {
        match (arm, operator_import_registered) {
            (crate::paired::RunArm::Candidate, true) => Self::RecallOnly,
            _ => Self::SourceAuthorityFormation,
        }
    }

    /// Exact public memory-tool allowlist used for the selected phase.
    pub fn enabled_tools(self) -> &'static [&'static str] {
        match self {
            Self::RecallOnly => &["assemble_context"],
            Self::SourceAuthorityFormation => &["ingest", "extract"],
        }
    }
}

struct RegisteredTool {
    memory_tool_name: String,
    function_name: String,
    definition: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompletionError {
    error_class: NormalizedErrorClass,
    http_status_code: Option<u16>,
    transport_stage: Option<TerminalTransportStage>,
}

/// MCP boundary used by the model loop. Implementations execute only public
/// `tools/call` requests; the model never receives direct storage access.
pub trait MemoryToolSession {
    fn tool_schemas(&self) -> &Value;
    fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, EvalError>;
}

/// Selects the public request to execute in a one-read diagnostic phase.
pub enum RecallReadMode {
    Live,
    FixedReply(Value),
}

/// Executes at most one public read in the trial's own memory session.
pub(crate) type RecallReplyValidator<'a> = dyn Fn(&Value) -> Result<(), EvalError> + 'a;

pub struct RecallMemorySession<'a, M> {
    memory: &'a mut M,
    mode: Option<RecallReadMode>,
    validate_reply: Option<&'a RecallReplyValidator<'a>>,
}

impl<'a, M: MemoryToolSession> RecallMemorySession<'a, M> {
    pub fn new(memory: &'a mut M, mode: RecallReadMode) -> Result<Self, EvalError> {
        if let RecallReadMode::FixedReply(arguments) = &mode
            && !arguments
                .get("query")
                .and_then(Value::as_str)
                .is_some_and(|q| !q.trim().is_empty())
        {
            return Err(EvalError::InvalidConfig(
                "fixed recall requires a public query".into(),
            ));
        }
        Ok(Self {
            memory,
            mode: Some(mode),
            validate_reply: None,
        })
    }

    pub(crate) fn with_reply_validator(
        mut self,
        validator: Option<&'a RecallReplyValidator<'a>>,
    ) -> Self {
        self.validate_reply = validator;
        self
    }
}

impl<M: MemoryToolSession> MemoryToolSession for RecallMemorySession<'_, M> {
    fn tool_schemas(&self) -> &Value {
        self.memory.tool_schemas()
    }

    fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, EvalError> {
        if name != "assemble_context" {
            return Err(EvalError::InvalidInput(
                "recall diagnostic only permits assemble_context".into(),
            ));
        }
        let arguments = match self.mode.take() {
            Some(RecallReadMode::Live) => arguments,
            Some(RecallReadMode::FixedReply(registered)) => registered,
            None => {
                return Err(EvalError::InvalidInput(
                    "recall diagnostic permits only one read".into(),
                ));
            }
        };
        let reply = self.memory.call_tool(name, arguments)?;
        if let Some(validate) = self.validate_reply {
            validate(&reply)?;
        }
        Ok(reply)
    }
}

impl MemoryToolSession for crate::suites::memory_decisions::stdio::StdioSession {
    fn tool_schemas(&self) -> &Value {
        crate::suites::memory_decisions::stdio::StdioSession::tool_schemas(self)
    }

    fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, EvalError> {
        crate::suites::memory_decisions::stdio::StdioSession::call_tool(self, name, arguments)
    }
}

#[derive(Clone)]
pub struct ChatCompletionsConfig {
    endpoint: String,
    model: String,
    bearer_token: Option<String>,
    sampling: ChatCompletionsSampling,
    response_format: Option<Value>,
    request_parameters: Option<Value>,
}

/// Optional, hashable generation controls sent on every Chat Completions turn.
/// `temperature=0` with no overrides preserves the legacy runner.
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatCompletionsSampling {
    #[serde(default)]
    pub temperature: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f64>,
}

impl Default for ChatCompletionsSampling {
    fn default() -> Self {
        Self {
            temperature: 0.0,
            top_p: None,
            top_k: None,
            min_p: None,
            presence_penalty: None,
            repeat_penalty: None,
        }
    }
}

impl ChatCompletionsSampling {
    pub fn validate(&self) -> Result<(), EvalError> {
        if !self.temperature.is_finite() || !(0.0..=2.0).contains(&self.temperature) {
            return Err(EvalError::InvalidConfig(
                "Chat Completions temperature must be between 0 and 2".into(),
            ));
        }
        if self
            .top_p
            .is_some_and(|top_p| !top_p.is_finite() || !(0.0..=1.0).contains(&top_p))
        {
            return Err(EvalError::InvalidConfig(
                "Chat Completions top_p must be between 0 and 1".into(),
            ));
        }
        if self.top_k == Some(0) {
            return Err(EvalError::InvalidConfig(
                "Chat Completions top_k must be positive when set".into(),
            ));
        }
        if self
            .min_p
            .is_some_and(|min_p| !min_p.is_finite() || !(0.0..=1.0).contains(&min_p))
        {
            return Err(EvalError::InvalidConfig(
                "Chat Completions min_p must be between 0 and 1".into(),
            ));
        }
        if self
            .presence_penalty
            .is_some_and(|penalty| !penalty.is_finite() || !(-2.0..=2.0).contains(&penalty))
        {
            return Err(EvalError::InvalidConfig(
                "Chat Completions presence_penalty must be between -2 and 2".into(),
            ));
        }
        if self
            .repeat_penalty
            .is_some_and(|penalty| !penalty.is_finite() || penalty <= 0.0)
        {
            return Err(EvalError::InvalidConfig(
                "Chat Completions repeat_penalty must be positive when set".into(),
            ));
        }
        Ok(())
    }

    /// Stable identity for registration and preflight; only effective sampler
    /// values are included, never endpoint or credential data.
    pub fn fingerprint_sha256(&self) -> Result<String, EvalError> {
        self.validate()?;
        Ok(sha256(&serde_json::to_vec(self)?))
    }
}

impl ChatCompletionsConfig {
    pub fn new(endpoint: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            model: model.into(),
            bearer_token: None,
            sampling: ChatCompletionsSampling::default(),
            response_format: None,
            request_parameters: None,
        }
    }

    /// Set an optional credential in memory; it is never serialized or logged.
    pub fn with_bearer_token(mut self, token: Option<String>) -> Self {
        self.bearer_token = token.filter(|value| !value.is_empty());
        self
    }

    pub fn with_sampling(mut self, sampling: ChatCompletionsSampling) -> Self {
        self.sampling = sampling;
        self
    }

    /// Set an optional OpenAI-compatible response format, such as JSON Schema.
    /// The format is sent unchanged on every turn and must be pinned by callers.
    pub fn with_response_format(mut self, response_format: Option<Value>) -> Self {
        self.response_format = response_format;
        self
    }

    /// Set pinned, provider-specific top-level request parameters.
    /// Reserved protocol and sampling fields cannot be overridden.
    pub fn with_request_parameters(mut self, request_parameters: Option<Value>) -> Self {
        self.request_parameters = request_parameters;
        self
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ChatCompletionBudgets {
    pub context_tokens: u64,
    pub tool_calls: u32,
    pub turns: u32,
    pub output_tokens: u64,
    pub wall_time: Duration,
}

pub struct ChatCompletionsAgent {
    endpoint: Url,
    model: String,
    bearer_token: Option<String>,
    sampling: ChatCompletionsSampling,
    response_format: Option<Value>,
    request_parameters: Option<Value>,
    client: Client,
    runtime: tokio::runtime::Runtime,
}

impl ChatCompletionsAgent {
    pub fn new(config: ChatCompletionsConfig) -> Result<Self, EvalError> {
        if config.model.trim().is_empty() {
            return Err(EvalError::InvalidConfig(
                "Chat Completions model identifier is empty".into(),
            ));
        }
        config.sampling.validate()?;
        if let Some(response_format) = &config.response_format {
            validate_response_format(response_format)?;
        }
        if let Some(request_parameters) = &config.request_parameters {
            validate_request_parameters(request_parameters)?;
        }
        let endpoint = chat_completions_url(&config.endpoint)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| EvalError::InvalidConfig("could not create model runtime".into()))?;
        let client = {
            let _entered = runtime.enter();
            Client::builder()
                .no_proxy()
                .redirect(Policy::none())
                .build()
                .map_err(|_| {
                    EvalError::InvalidConfig("could not create local model client".into())
                })?
        };
        Ok(Self {
            endpoint,
            model: config.model,
            bearer_token: config.bearer_token,
            sampling: config.sampling,
            response_format: config.response_format,
            request_parameters: config.request_parameters,
            client,
            runtime,
        })
    }

    /// Execute one isolated agent phase. Raw model/MCP content remains transient;
    /// the returned trace contains hashes, normalized tool names and usage only.
    pub fn run_phase<M: MemoryToolSession>(
        &self,
        memory: &mut M,
        prompt: &str,
        system_instructions: &str,
        task_rules: &str,
        profile: ChatCompletionsToolProfile,
        budgets: ChatCompletionBudgets,
    ) -> Result<AgentObservation, EvalError> {
        if prompt.trim().is_empty()
            || system_instructions.trim().is_empty()
            || task_rules.trim().is_empty()
            || budgets.context_tokens == 0
            || budgets.tool_calls == 0
            || budgets.turns == 0
            || budgets.output_tokens == 0
            || budgets.wall_time.is_zero()
        {
            return Err(EvalError::InvalidConfig(
                "Chat Completions phase prompt and budgets must be nonempty".into(),
            ));
        }
        let tools = registered_tools(memory.tool_schemas(), profile)?;
        let combined_system_instructions = format!("{system_instructions}\n\n{task_rules}");
        let mut messages = vec![
            json!({"role":"system","content":combined_system_instructions}),
            json!({"role":"user","content":prompt}),
        ];
        let started = Instant::now();
        let mut observation = AgentObservation {
            trace: Vec::new(),
            final_answer: None,
            memory_source_episode_ids: Vec::new(),
            memory_reads: Some(Vec::new()),
            read_capture_status: Some(super::ReadCaptureStatus::Observed),
            memory_write_episode_ids: Vec::new(),
            extract_fact_evidence: Vec::new(),
            extract_fact_evidence_telemetry_complete: matches!(
                profile,
                ChatCompletionsToolProfile::SourceAuthorityFormation
            ),
            terminal_http_status_code: None,
            terminal_transport_stage: None,
            turns_used: 0,
            tool_calls: 0,
            user_clarifications: 0,
            input_tokens: 0,
            output_tokens: 0,
            wall_time_ms: 0,
            status: TrialStatus::ModelFailure,
        };
        let mut sources = BTreeSet::new();
        let mut writes = BTreeSet::new();
        let mut seen_call_ids = BTreeSet::new();

        'turns: loop {
            if observation.turns_used >= budgets.turns {
                observation.status = TrialStatus::BudgetExceeded;
                push_event(
                    &mut observation.trace,
                    observation.turns_used,
                    TraceEventKind::BudgetLimitReached,
                    None,
                    None,
                    None,
                    None,
                    None,
                )?;
                break;
            }
            let remaining = budgets.wall_time.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                observation.status = TrialStatus::TimedOut;
                push_event(
                    &mut observation.trace,
                    observation.turns_used,
                    TraceEventKind::Timeout,
                    None,
                    None,
                    Some(NormalizedErrorClass::Timeout),
                    None,
                    None,
                )?;
                break;
            }
            observation.turns_used += 1;
            push_event(
                &mut observation.trace,
                observation.turns_used,
                TraceEventKind::AgentTurnStarted,
                None,
                None,
                None,
                None,
                None,
            )?;
            let remaining_output_tokens = budgets
                .output_tokens
                .saturating_sub(observation.output_tokens);
            if remaining_output_tokens == 0 {
                observation.status = TrialStatus::BudgetExceeded;
                push_event(
                    &mut observation.trace,
                    observation.turns_used,
                    TraceEventKind::BudgetLimitReached,
                    None,
                    None,
                    None,
                    None,
                    None,
                )?;
                break;
            }
            let mut request = json!({
                "model": self.model,
                "messages": messages,
                "tools": tools.iter().map(|tool| &tool.definition).collect::<Vec<_>>(),
                "tool_choice": "auto",
                "parallel_tool_calls": false,
                "temperature": self.sampling.temperature,
                "max_tokens": remaining_output_tokens.min(u64::from(u32::MAX)),
            });
            if let Some(response_format) = &self.response_format {
                request["response_format"] = response_format.clone();
            }
            if let Some(request_parameters) = &self.request_parameters
                && let (Some(request), Some(parameters)) =
                    (request.as_object_mut(), request_parameters.as_object())
            {
                request.extend(parameters.clone());
            }
            if let Some(top_p) = self.sampling.top_p {
                request["top_p"] = json!(top_p);
            }
            if let Some(top_k) = self.sampling.top_k {
                request["top_k"] = json!(top_k);
            }
            if let Some(min_p) = self.sampling.min_p {
                request["min_p"] = json!(min_p);
            }
            if let Some(presence_penalty) = self.sampling.presence_penalty {
                request["presence_penalty"] = json!(presence_penalty);
            }
            if let Some(repeat_penalty) = self.sampling.repeat_penalty {
                request["repeat_penalty"] = json!(repeat_penalty);
            }
            let response = match self.complete(&request, remaining) {
                Ok(response) => response,
                Err(error) => {
                    observation.terminal_http_status_code = error.http_status_code;
                    observation.terminal_transport_stage = error.transport_stage;
                    observation.status = if error.error_class == NormalizedErrorClass::Timeout {
                        TrialStatus::TimedOut
                    } else {
                        TrialStatus::ModelFailure
                    };
                    push_event(
                        &mut observation.trace,
                        observation.turns_used,
                        if observation.status == TrialStatus::TimedOut {
                            TraceEventKind::Timeout
                        } else {
                            TraceEventKind::ModelFailure
                        },
                        None,
                        None,
                        Some(error.error_class),
                        None,
                        None,
                    )?;
                    break;
                }
            };
            let Some((input_tokens, output_tokens)) = response_usage(&response) else {
                fail_model(&mut observation, NormalizedErrorClass::InvalidResponse)?;
                break;
            };
            let Some(input_total) = observation.input_tokens.checked_add(input_tokens) else {
                fail_model(&mut observation, NormalizedErrorClass::Internal)?;
                break;
            };
            let Some(output_total) = observation.output_tokens.checked_add(output_tokens) else {
                fail_model(&mut observation, NormalizedErrorClass::Internal)?;
                break;
            };
            observation.input_tokens = input_total;
            observation.output_tokens = output_total;
            push_event_with_usage(
                &mut observation.trace,
                observation.turns_used,
                input_tokens,
                output_tokens,
            )?;
            if input_tokens > budgets.context_tokens
                || observation.output_tokens > budgets.output_tokens
            {
                observation.status = TrialStatus::BudgetExceeded;
                push_event(
                    &mut observation.trace,
                    observation.turns_used,
                    TraceEventKind::BudgetLimitReached,
                    None,
                    None,
                    None,
                    None,
                    None,
                )?;
                break;
            }
            if response.get("model").and_then(Value::as_str) != Some(self.model.as_str()) {
                fail_model(&mut observation, NormalizedErrorClass::InvalidResponse)?;
                break;
            }
            let Some(message) = response
                .pointer("/choices/0/message")
                .filter(|value| value.is_object())
            else {
                fail_model(&mut observation, NormalizedErrorClass::InvalidResponse)?;
                break;
            };
            let calls = message
                .get("tool_calls")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if calls.is_empty() {
                let Some(content) = message.get("content").and_then(Value::as_str) else {
                    fail_model(&mut observation, NormalizedErrorClass::InvalidResponse)?;
                    break;
                };
                if let Err(error_class) = validate_final_answer(content, profile) {
                    fail_model(&mut observation, error_class)?;
                    break;
                }
                observation.final_answer = Some(content.to_owned());
                if profile == ChatCompletionsToolProfile::RecallOnly {
                    let final_value: Value = serde_json::from_str(content)
                        .map_err(|_| invalid_model_response("final answer was not JSON"))?;
                    observation.user_clarifications = u32::from(
                        final_value
                            .get("needs_clarification")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    );
                    if observation.user_clarifications > 0 {
                        push_event(
                            &mut observation.trace,
                            observation.turns_used,
                            TraceEventKind::Clarification,
                            None,
                            None,
                            None,
                            None,
                            None,
                        )?;
                    }
                }
                push_event(
                    &mut observation.trace,
                    observation.turns_used,
                    TraceEventKind::FinalAnswer,
                    None,
                    None,
                    None,
                    Some(super::sha256(content.as_bytes())),
                    None,
                )?;
                observation.status = TrialStatus::Completed;
                break;
            }
            let mut assistant_message = json!({
                "role":"assistant",
                "content":message.get("content").cloned().unwrap_or(Value::Null),
                "tool_calls":calls,
            });
            if let Some(reasoning_content) = message.get("reasoning_content") {
                if !reasoning_content.is_null() && !reasoning_content.is_string() {
                    fail_model(&mut observation, NormalizedErrorClass::InvalidResponse)?;
                    break 'turns;
                }
                if reasoning_content.is_string() {
                    assistant_message["reasoning_content"] = reasoning_content.clone();
                }
            }
            messages.push(assistant_message);
            if calls.len() != 1 {
                fail_model(&mut observation, NormalizedErrorClass::InvalidToolCall)?;
                break 'turns;
            }
            let (call_id, tool_name, arguments) =
                match parse_tool_call(&calls[0], &tools, &mut seen_call_ids) {
                    Ok(call) => call,
                    Err(error_class) => {
                        let error_class = if error_class == NormalizedErrorClass::Internal {
                            NormalizedErrorClass::InvalidToolCall
                        } else {
                            error_class
                        };
                        fail_model(&mut observation, error_class)?;
                        break 'turns;
                    }
                };
            if observation.tool_calls >= budgets.tool_calls {
                observation.status = TrialStatus::BudgetExceeded;
                push_event(
                    &mut observation.trace,
                    observation.turns_used,
                    TraceEventKind::BudgetLimitReached,
                    None,
                    None,
                    None,
                    None,
                    None,
                )?;
                break 'turns;
            }
            observation.tool_calls += 1;
            let request_kind = if tool_name == "assemble_context" {
                TraceEventKind::MemoryRead
            } else {
                TraceEventKind::MemoryWrite
            };
            let request_sequence = push_event(
                &mut observation.trace,
                observation.turns_used,
                request_kind,
                Some(call_id.clone()),
                Some(tool_name.to_owned()),
                None,
                Some(hash_json(&arguments)?),
                None,
            )?;
            if tool_name == "extract" {
                observation
                    .extract_fact_evidence
                    .push(ExtractFactEvidenceObservation {
                        request_sequence,
                        response_sequence: None,
                        requested: arguments
                            .get("include_fact_evidence")
                            .and_then(Value::as_bool)
                            .unwrap_or(true),
                        returned: false,
                        response_wire_bytes: None,
                    });
            }
            let tool_result = match memory.call_tool(tool_name, arguments.clone()) {
                Ok(result)
                    if result.pointer("/result/isError").and_then(Value::as_bool) != Some(true) =>
                {
                    result
                }
                Ok(_) | Err(_) => {
                    observation.status = TrialStatus::ToolFailure;
                    let response_sequence = push_event(
                        &mut observation.trace,
                        observation.turns_used,
                        TraceEventKind::ToolError,
                        Some(call_id.clone()),
                        Some(tool_name.to_owned()),
                        Some(NormalizedErrorClass::Unknown),
                        None,
                        None,
                    )?;
                    if tool_name == "assemble_context"
                        && let Some(reads) = observation.memory_reads.as_mut()
                    {
                        reads.push(super::MemoryReadObservation {
                            request_sequence,
                            response_sequence: Some(response_sequence),
                            outcome: super::MemoryReadOutcome::Failed,
                            item_count: None,
                            evidence: None,
                        });
                    }
                    messages.push(json!({
                        "role":"tool",
                        "tool_call_id":call_id,
                        "content":"{\"error\":\"memory tool failed\"}",
                    }));
                    continue 'turns;
                }
            };
            let response_sha256 = hash_json(&tool_result)?;
            let response_sequence = push_event(
                &mut observation.trace,
                observation.turns_used,
                TraceEventKind::ToolResponse,
                Some(call_id.clone()),
                Some(tool_name.to_owned()),
                None,
                Some(response_sha256),
                None,
            )?;
            match tool_name {
                "assemble_context" => {
                    super::tool_results::collect_source_episode_ids(&tool_result, &mut sources);
                    let (outcome, item_count) =
                        super::tool_results::read_result(&tool_result["result"]);
                    if let Some(reads) = observation.memory_reads.as_mut() {
                        reads.push(super::MemoryReadObservation {
                            request_sequence,
                            response_sequence: Some(response_sequence),
                            outcome,
                            item_count,
                            evidence: super::tool_results::read_evidence(
                                &tool_result["result"],
                                super::tool_results::requested_cutoff(&arguments),
                            ),
                        });
                    }
                }
                "ingest" => {
                    super::tool_results::collect_ingested_episode_id(&tool_result, &mut writes)
                }
                "extract" => {
                    super::tool_results::collect_inline_extracted_episode_id(
                        &tool_result,
                        &mut writes,
                    );
                    if let Some(observation) = observation.extract_fact_evidence.last_mut()
                        && observation.request_sequence == request_sequence
                    {
                        observation.response_sequence = Some(response_sequence);
                        observation.returned = observation.requested
                            && super::tool_results::has_complete_fact_evidence(&tool_result);
                        observation.response_wire_bytes = Some(
                            u64::try_from(serde_json::to_vec(&tool_result)?.len())
                                .unwrap_or(u64::MAX),
                        );
                    }
                }
                _ => unreachable!("registered tool profile contains only known tools"),
            }
            messages.push(json!({
                "role":"tool",
                "tool_call_id":call_id,
                "content":serde_json::to_string(&tool_result)
                    .map_err(|_| invalid_model_response("memory result serialization failed"))?,
            }));
        }

        Ok(finish_observation(observation, started, sources, writes))
    }

    fn complete(&self, request: &Value, timeout: Duration) -> Result<Value, CompletionError> {
        let mut builder = self
            .client
            .post(self.endpoint.clone())
            .timeout(timeout)
            .json(request);
        if let Some(token) = &self.bearer_token {
            builder = builder.bearer_auth(token);
        }
        let response = self
            .runtime
            .block_on(async { builder.send().await })
            .map_err(|error| CompletionError {
                error_class: if error.is_timeout() {
                    NormalizedErrorClass::Timeout
                } else {
                    NormalizedErrorClass::Transport
                },
                http_status_code: None,
                transport_stage: Some(TerminalTransportStage::RequestSend),
            })?;
        if !response.status().is_success() {
            return Err(CompletionError {
                error_class: NormalizedErrorClass::Transport,
                http_status_code: Some(response.status().as_u16()),
                transport_stage: None,
            });
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_MODEL_RESPONSE_BYTES as u64)
        {
            return Err(CompletionError {
                error_class: NormalizedErrorClass::InvalidResponse,
                http_status_code: None,
                transport_stage: None,
            });
        }
        let bytes = self.runtime.block_on(async {
            let mut response = response;
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|error| CompletionError {
                error_class: if error.is_timeout() {
                    NormalizedErrorClass::Timeout
                } else {
                    NormalizedErrorClass::Transport
                },
                http_status_code: None,
                transport_stage: Some(TerminalTransportStage::ResponseBodyRead),
            })? {
                if bytes.len().saturating_add(chunk.len()) > MAX_MODEL_RESPONSE_BYTES {
                    return Err(CompletionError {
                        error_class: NormalizedErrorClass::InvalidResponse,
                        http_status_code: None,
                        transport_stage: None,
                    });
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok::<_, CompletionError>(bytes)
        })?;
        serde_json::from_slice(&bytes).map_err(|_| CompletionError {
            error_class: NormalizedErrorClass::InvalidResponse,
            http_status_code: None,
            transport_stage: None,
        })
    }
}

fn validate_response_format(response_format: &Value) -> Result<(), EvalError> {
    let object = response_format.as_object().ok_or_else(|| {
        EvalError::InvalidConfig("Chat Completions response_format must be a JSON object".into())
    })?;
    match response_format.get("type").and_then(Value::as_str) {
        Some("json_object") if object.len() == 1 => Ok(()),
        Some("json_schema") => {
            let Some(json_schema) = response_format
                .get("json_schema")
                .and_then(Value::as_object)
            else {
                return Err(EvalError::InvalidConfig(
                    "JSON Schema response_format requires a json_schema object".into(),
                ));
            };
            let has_valid_name = json_schema
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| !name.trim().is_empty());
            let has_schema = json_schema.get("schema").is_some_and(Value::is_object);
            let strict_is_boolean = json_schema.get("strict").is_none_or(Value::is_boolean);
            if has_valid_name && has_schema && strict_is_boolean {
                Ok(())
            } else {
                Err(EvalError::InvalidConfig(
                    "JSON Schema response_format requires a nonempty name, object schema, and optional boolean strict flag".into(),
                ))
            }
        }
        _ => Err(EvalError::InvalidConfig(
            "Chat Completions response_format type must be json_object or json_schema".into(),
        )),
    }
}

fn chat_completions_url(endpoint: &str) -> Result<Url, EvalError> {
    let mut url = Url::parse(endpoint)
        .map_err(|_| EvalError::InvalidConfig("model endpoint URL is invalid".into()))?;
    let loopback = url
        .host_str()
        .and_then(|host| host.parse::<IpAddr>().ok())
        .is_some_and(|address| address.is_loopback());
    if url.scheme() != "http"
        || !loopback
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(EvalError::InvalidConfig(
            "model endpoint must be a plain HTTP loopback URL without credentials or query".into(),
        ));
    }
    let path = format!(
        "{}/{}",
        url.path().trim_end_matches('/'),
        CHAT_COMPLETIONS_PATH
    );
    url.set_path(&path);
    Ok(url)
}

/// Hashes the effective Chat Completions URL with a versioned domain prefix.
/// Endpoint credentials and query strings are rejected by the URL validator.
pub fn endpoint_config_sha256(endpoint: &str) -> Result<String, EvalError> {
    let effective_url = chat_completions_url(endpoint)?;
    let canonical = format!("memory-mcp-chat-completions-endpoint/v1\n{effective_url}");
    Ok(super::sha256(canonical.as_bytes()))
}

/// Validates and hashes the effective, provider-neutral response format.
/// The versioned prefix separates this identity from unrelated JSON hashes.
pub fn response_format_config_sha256(response_format: &Value) -> Result<String, EvalError> {
    validate_response_format(response_format)?;
    let canonical = serde_json::to_vec(response_format)?;
    let mut bytes = b"memory-mcp-chat-completions-response-format/v1\n".to_vec();
    bytes.extend_from_slice(&canonical);
    Ok(super::sha256(&bytes))
}

/// Validates and hashes pinned provider-specific request parameters. The opaque
/// values are part of the run identity and may not replace protocol controls.
pub fn request_parameters_config_sha256(parameters: &Value) -> Result<String, EvalError> {
    validate_request_parameters(parameters)?;
    let canonical = serde_json::to_vec(parameters)?;
    let mut bytes = b"memory-mcp-chat-completions-request-parameters/v1\n".to_vec();
    bytes.extend_from_slice(&canonical);
    Ok(super::sha256(&bytes))
}

fn validate_request_parameters(parameters: &Value) -> Result<(), EvalError> {
    let Some(parameters) = parameters.as_object() else {
        return Err(EvalError::InvalidConfig(
            "Chat Completions request parameters must be a JSON object".into(),
        ));
    };
    const RESERVED: &[&str] = &[
        "model",
        "messages",
        "tools",
        "tool_choice",
        "parallel_tool_calls",
        "temperature",
        "max_tokens",
        "max_completion_tokens",
        "top_p",
        "top_k",
        "min_p",
        "presence_penalty",
        "repeat_penalty",
        "response_format",
        "stream",
        "stream_options",
        "functions",
        "function_call",
    ];
    if parameters
        .keys()
        .any(|key| RESERVED.contains(&key.as_str()))
    {
        return Err(EvalError::InvalidConfig(
            "Chat Completions request parameters cannot override protocol or sampling fields"
                .into(),
        ));
    }
    Ok(())
}

fn registered_tools(
    schemas: &Value,
    profile: ChatCompletionsToolProfile,
) -> Result<Vec<RegisteredTool>, EvalError> {
    let schemas = schemas
        .as_array()
        .ok_or_else(|| EvalError::InvalidInput("MCP tool schemas are not an array".into()))?;
    let mut tools = Vec::new();
    for name in profile.enabled_tools() {
        let schema = schemas
            .iter()
            .find(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
            .ok_or_else(|| {
                EvalError::InvalidInput(format!("MCP server does not advertise `{name}`"))
            })?;
        let function_name = function_name(name);
        let parameters = schema.get("inputSchema").cloned().ok_or_else(|| {
            EvalError::InvalidInput(format!("MCP server omits `{name}` input schema"))
        })?;
        let description = schema
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("Registered memory operation.");
        let definition = json!({
            "type":"function",
            "function": {
                "name":function_name,
                "description":description,
                "parameters":parameters
            }
        });
        if tools
            .iter()
            .any(|tool: &RegisteredTool| tool.function_name == function_name)
        {
            return Err(EvalError::InvalidInput(
                "MCP tool names collide after model function-name normalization".into(),
            ));
        }
        tools.push(RegisteredTool {
            memory_tool_name: (*name).to_owned(),
            function_name,
            definition,
        });
    }
    Ok(tools)
}

fn function_name(tool_name: &str) -> String {
    let safe = tool_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if safe
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_alphanumeric())
    {
        safe
    } else {
        format!("memory_{safe}")
    }
}

fn tool_name_for_function<'a>(tools: &'a [RegisteredTool], name: &str) -> Option<&'a str> {
    tools
        .iter()
        .find(|tool| tool.function_name == name)
        .map(|tool| tool.memory_tool_name.as_str())
}

fn validate_final_answer(
    content: &str,
    profile: ChatCompletionsToolProfile,
) -> Result<(), NormalizedErrorClass> {
    if profile == ChatCompletionsToolProfile::SourceAuthorityFormation {
        return if content.trim().is_empty() {
            Err(NormalizedErrorClass::EmptyFormationCompletion)
        } else {
            Ok(())
        };
    }
    let value: Value =
        serde_json::from_str(content).map_err(|_| NormalizedErrorClass::FinalAnswerNotJson)?;
    let valid = value.get("answer").and_then(Value::as_str).is_some()
        && value
            .get("needs_clarification")
            .and_then(Value::as_bool)
            .is_some()
        && value
            .get("citations")
            .and_then(Value::as_array)
            .is_some_and(|citations| {
                citations
                    .iter()
                    .all(|citation| citation.as_str().is_some_and(|id| !id.trim().is_empty()))
            });
    if !valid {
        return Err(NormalizedErrorClass::FinalAnswerSchemaMismatch);
    }
    Ok(())
}

fn response_usage(response: &Value) -> Option<(u64, u64)> {
    let usage = response.get("usage")?;
    Some((
        usage.get("prompt_tokens")?.as_u64()?,
        usage.get("completion_tokens")?.as_u64()?,
    ))
}

fn parse_tool_call<'tools>(
    call: &Value,
    tools: &'tools [RegisteredTool],
    seen_call_ids: &mut BTreeSet<String>,
) -> Result<(String, &'tools str, Value), NormalizedErrorClass> {
    let call_id = call
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or(NormalizedErrorClass::InvalidToolCall)?
        .to_owned();
    if !seen_call_ids.insert(call_id.clone()) {
        return Err(NormalizedErrorClass::InvalidToolCall);
    }
    let function = call
        .get("function")
        .filter(|value| value.is_object())
        .ok_or(NormalizedErrorClass::InvalidToolCall)?;
    let function_name = function
        .get("name")
        .and_then(Value::as_str)
        .ok_or(NormalizedErrorClass::InvalidToolCall)?;
    let tool_name =
        tool_name_for_function(tools, function_name).ok_or(NormalizedErrorClass::Unauthorized)?;
    let arguments = function
        .get("arguments")
        .and_then(Value::as_str)
        .ok_or(NormalizedErrorClass::InvalidArguments)
        .and_then(|text| {
            serde_json::from_str(text).map_err(|_| NormalizedErrorClass::InvalidArguments)
        })?;
    Ok((call_id, tool_name, arguments))
}

fn fail_model(
    observation: &mut AgentObservation,
    error_class: NormalizedErrorClass,
) -> Result<(), EvalError> {
    observation.status = TrialStatus::ModelFailure;
    push_event(
        &mut observation.trace,
        observation.turns_used,
        TraceEventKind::ModelFailure,
        None,
        None,
        Some(error_class),
        None,
        None,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn push_event(
    trace: &mut Vec<AgentTraceEvent>,
    turn: u32,
    kind: TraceEventKind,
    call_id: Option<String>,
    tool_name: Option<String>,
    error_class: Option<NormalizedErrorClass>,
    payload_sha256: Option<String>,
    input_tokens: Option<u64>,
) -> Result<u32, EvalError> {
    let sequence = u32::try_from(trace.len() + 1)
        .map_err(|_| invalid_model_response("trace sequence overflowed"))?;
    trace.push(AgentTraceEvent {
        sequence,
        turn,
        kind,
        call_id,
        tool_name,
        error_class,
        payload_sha256,
        input_tokens,
        output_tokens: None,
    });
    Ok(sequence)
}

fn push_event_with_usage(
    trace: &mut Vec<AgentTraceEvent>,
    turn: u32,
    input_tokens: u64,
    output_tokens: u64,
) -> Result<(), EvalError> {
    let sequence = u32::try_from(trace.len() + 1)
        .map_err(|_| invalid_model_response("trace sequence overflowed"))?;
    trace.push(AgentTraceEvent {
        sequence,
        turn,
        kind: TraceEventKind::AgentTurn,
        call_id: None,
        tool_name: None,
        error_class: None,
        payload_sha256: None,
        input_tokens: Some(input_tokens),
        output_tokens: Some(output_tokens),
    });
    Ok(())
}

fn finish_observation(
    mut observation: AgentObservation,
    started: Instant,
    sources: BTreeSet<String>,
    writes: BTreeSet<String>,
) -> AgentObservation {
    observation.memory_source_episode_ids = sources.into_iter().collect();
    observation.memory_write_episode_ids = writes.into_iter().collect();
    observation.wall_time_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    observation
}

fn hash_json(value: &Value) -> Result<String, EvalError> {
    serde_json::to_vec(value)
        .map(|bytes| super::sha256(&bytes))
        .map_err(|_| invalid_model_response("trace payload could not be fingerprinted"))
}

fn invalid_model_response(message: &str) -> EvalError {
    EvalError::InvalidInput(format!("Chat Completions response {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_sampler_preserves_legacy_greedy_parameters() {
        let sampling = ChatCompletionsSampling::default();

        assert_eq!(sampling.temperature, 0.0);
        assert_eq!(sampling.top_p, None);
        assert_eq!(sampling.top_k, None);
        assert!(sampling.validate().is_ok());
    }

    #[test]
    fn sampler_fingerprint_is_stable_and_changes_with_effective_parameters() {
        let first = ChatCompletionsSampling {
            temperature: 1.0,
            top_p: Some(0.95),
            top_k: Some(20),
            ..ChatCompletionsSampling::default()
        };
        let same = first;
        let different = ChatCompletionsSampling {
            top_k: Some(21),
            ..first
        };

        assert_eq!(
            first.fingerprint_sha256().unwrap(),
            same.fingerprint_sha256().unwrap()
        );
        assert_ne!(
            first.fingerprint_sha256().unwrap(),
            different.fingerprint_sha256().unwrap()
        );
    }

    #[test]
    fn response_format_fingerprint_is_stable_and_changes_with_the_registered_schema() {
        let first = json!({
            "type": "json_schema",
            "json_schema": {
                "name": "memory_recall_answer",
                "strict": true,
                "schema": {
                    "type": "object",
                    "properties": {"answer": {"type": "string"}},
                    "required": ["answer"],
                    "additionalProperties": false
                }
            }
        });
        let same = json!({
            "json_schema": {
                "schema": {
                    "additionalProperties": false,
                    "required": ["answer"],
                    "properties": {"answer": {"type": "string"}},
                    "type": "object"
                },
                "strict": true,
                "name": "memory_recall_answer"
            },
            "type": "json_schema"
        });
        let different = json!({
            "type": "json_schema",
            "json_schema": {
                "name": "memory_recall_answer",
                "strict": true,
                "schema": {
                    "type": "object",
                    "properties": {"answer": {"type": "string"}},
                    "required": ["answer"],
                    "additionalProperties": true
                }
            }
        });

        assert_eq!(
            response_format_config_sha256(&first).unwrap(),
            response_format_config_sha256(&same).unwrap()
        );
        assert_ne!(
            response_format_config_sha256(&first).unwrap(),
            response_format_config_sha256(&different).unwrap()
        );
    }

    #[test]
    fn sampler_rejects_temperature_outside_supported_range() {
        let sampling = ChatCompletionsSampling {
            temperature: 2.1,
            ..ChatCompletionsSampling::default()
        };

        assert!(sampling.validate().is_err());
    }

    #[test]
    fn sampler_rejects_top_p_outside_unit_interval() {
        let sampling = ChatCompletionsSampling {
            top_p: Some(1.01),
            ..ChatCompletionsSampling::default()
        };

        assert!(sampling.validate().is_err());
    }

    #[test]
    fn sampler_rejects_zero_top_k() {
        let sampling = ChatCompletionsSampling {
            top_k: Some(0),
            ..ChatCompletionsSampling::default()
        };

        assert!(sampling.validate().is_err());
    }

    #[test]
    fn sampler_rejects_min_p_outside_unit_interval() {
        let sampling = ChatCompletionsSampling {
            min_p: Some(1.01),
            ..ChatCompletionsSampling::default()
        };

        assert!(sampling.validate().is_err());
    }

    #[test]
    fn sampler_rejects_presence_penalty_outside_supported_range() {
        let sampling = ChatCompletionsSampling {
            presence_penalty: Some(2.01),
            ..ChatCompletionsSampling::default()
        };

        assert!(sampling.validate().is_err());
    }

    #[test]
    fn sampler_rejects_nonpositive_repeat_penalty() {
        let sampling = ChatCompletionsSampling {
            repeat_penalty: Some(0.0),
            ..ChatCompletionsSampling::default()
        };

        assert!(sampling.validate().is_err());
    }

    #[test]
    fn endpoint_fingerprint_hashes_the_effective_url_and_normalizes_trailing_slashes() {
        let expected = "f7bc8c808719d3bafd7ade8b4a4636cc29db9357b7dd495b8231dfbe2ea9af08";

        assert_eq!(
            endpoint_config_sha256("http://127.0.0.1:1234/v1").unwrap(),
            expected
        );
        assert_eq!(
            endpoint_config_sha256("http://127.0.0.1:1234/v1/").unwrap(),
            expected
        );
    }

    #[test]
    fn rejects_endpoint_that_can_transmit_evaluation_data_off_loopback() {
        let error = chat_completions_url("https://api.example.test/v1").unwrap_err();

        assert!(error.to_string().contains("loopback"));
    }
}
