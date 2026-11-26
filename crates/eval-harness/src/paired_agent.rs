//! Preregistered, oracle-isolated paired agent execution.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::EvalError;
use crate::paired::{
    AcceptedAnswerMatch, AgentTraceEvent, CaseTrialKey, HardViolation, PAIRED_ARTIFACT_SCHEMA_V6,
    PairedEvaluationPlan, PairedRunArtifact, RunArm, TaskOracleDiagnostics, TraceEventKind,
    TrialReceipt, TrialStatus,
};

pub mod chat_completions;
pub mod chat_completions_backend;
pub mod codex;
pub mod independent;
pub mod managed;
pub mod observations;
pub(crate) mod prompts;
pub mod public_trace;
pub(crate) mod tool_results;

pub const PAIRED_TASK_MANIFEST_SCHEMA_V1: &str = "memory-mcp-agent-tasks/v1";
pub const PAIRED_TASK_MANIFEST_SCHEMA_V2: &str = "memory-mcp-agent-tasks/v2";
pub const PAIRED_TASK_MANIFEST_SCHEMA_V3: &str = "memory-mcp-agent-tasks/v3";
pub const PAIRED_KNOWN_REVISION_PLACEHOLDER: &str = "{{KNOWN_REVISION}}";
const RECEIPT_JOURNAL_SCHEMA_V4: &str = "memory-mcp-agent-trial-receipt/v4";
const RECEIPT_JOURNAL_SCHEMA_V5: &str = "memory-mcp-agent-trial-receipt/v5";
const RECEIPT_JOURNAL_SCHEMA_V6: &str = "memory-mcp-agent-trial-receipt/v6";
const RECEIPT_JOURNAL_SCHEMA_V7: &str = "memory-mcp-agent-trial-receipt/v7";
const RECEIPT_JOURNAL_SCHEMA_V8: &str = "memory-mcp-agent-trial-receipt/v8";
const RECEIPT_JOURNAL_SCHEMA_V9: &str = "memory-mcp-agent-trial-receipt/v9";
const RECEIPT_JOURNAL_SCHEMA_V10: &str = "memory-mcp-agent-trial-receipt/v10";
pub(crate) const SNAPSHOT_RETRY_PRESERVED_SELECTOR_FIELDS: [&str; 6] = [
    "fact_types",
    "budget",
    "view_mode",
    "window_start",
    "window_end",
    "compact",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedTaskManifest {
    pub schema_version: String,
    pub tasks: Vec<PairedTaskDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedTaskDefinition {
    pub key: CaseTrialKey,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge_before_event_id: Option<String>,
    pub seed_episodes: Vec<SeedEpisode>,
    pub oracle: HiddenTaskOracle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedTaskInput {
    pub key: CaseTrialKey,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge_before_event_id: Option<String>,
    pub seed_episodes: Vec<SeedEpisode>,
}

impl PairedTaskDefinition {
    fn runner_input(&self) -> PairedTaskInput {
        PairedTaskInput {
            key: self.key.clone(),
            prompt: self.prompt.clone(),
            knowledge_before_event_id: self.knowledge_before_event_id.clone(),
            seed_episodes: self.seed_episodes.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedPairedTask {
    pub input: PairedTaskInput,
    /// Public snapshot token resolved from the candidate before either arm runs.
    pub expected_known_revision: Option<i64>,
}

pub(crate) struct SeedMemoryOutput {
    pub(crate) episode_id_to_event_id: BTreeMap<String, String>,
    pub(crate) tool_schemas: Value,
    pub(crate) known_revision: Option<i64>,
}

/// Public fixture input. This type is the only task data passed to the agent backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedEpisode {
    pub event_id: String,
    pub source_id: String,
    pub source_type: String,
    pub content: String,
    pub valid_at: DateTime<Utc>,
}

/// Evaluator-only expected outcomes. This value never crosses the backend interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HiddenTaskOracle {
    /// Each alternative contains required evidence groups; any event ID in a group suffices.
    pub sufficient_evidence_alternatives: Vec<Vec<Vec<String>>>,
    pub accepted_answers: Vec<String>,
    pub forbidden_event_ids: Vec<String>,
    pub needs_clarification: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_expectation: Option<ReadExpectation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadExpectation {
    pub valid_at: Option<DateTime<Utc>>,
    pub required_claim_annotation_alternatives:
        Vec<Vec<crate::independent_probe::ProbeClaimAnnotation>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector_arguments: Option<BTreeMap<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_bound_retry: Option<SnapshotBoundRetryExpectation>,
}

/// Private registration for a retry that must use the snapshot returned by
/// the immediately preceding authorized empty partial read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotBoundRetryExpectation {
    /// Hash of the exact guidance text that authorizes one query refinement.
    pub authorized_guidance_sha256: String,
    /// Require one retry when its trigger and complete snapshot are observed.
    #[serde(default)]
    pub retry_required_when_eligible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotRetryAssessment {
    NotRegistered,
    NotObserved,
    NotEligible,
    Compliant,
    Noncompliant,
}

/// Content-free state-transition result for the registered snapshot retry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotRetryDiagnostic {
    pub expectation_registered: bool,
    pub assessment: SnapshotRetryAssessment,
    pub retry_required_when_eligible: bool,
    pub initial_read_eligible: Option<bool>,
    pub retry_observed: Option<bool>,
    pub retry_count: Option<u32>,
    pub retry_limit_respected: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_changed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_cutoff_matches: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_temporal_selector_matches: Option<BTreeMap<String, bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preserved_selector_matches: Option<BTreeMap<String, bool>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedTrialExecution {
    pub observation: AgentObservation,
    /// Maps the product's generated episode IDs to manifest event IDs for private scoring.
    pub episode_id_to_event_id: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentObservation {
    pub trace: Vec<AgentTraceEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_reads: Option<Vec<MemoryReadObservation>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_capture_status: Option<ReadCaptureStatus>,
    /// Exact final output text for private scoring.
    pub final_answer: Option<String>,
    /// Source episode IDs observed in successful memory reads.
    pub memory_source_episode_ids: Vec<String>,
    /// Episode IDs returned by successful `ingest` calls. These opaque IDs
    /// support independent post-write probes without retaining memory content.
    #[serde(default)]
    pub memory_write_episode_ids: Vec<String>,
    /// Transient fact/source pairs and invalidation targets used only for
    /// private agent-managed scoring; raw identifiers are never serialized.
    #[serde(skip)]
    pub observed_fact_sources: Vec<(String, String, u32)>,
    #[serde(skip)]
    pub invalidation_fact_ids: Vec<(String, u32)>,
    /// Content-free request/response observations for inline fact evidence.
    /// External runners may provide the same normalized telemetry; raw
    /// statements and source quotes must never be stored here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extract_fact_evidence: Vec<ExtractFactEvidenceObservation>,
    /// True when the runner observed and normalized every extract request and
    /// response in this phase. False is distinct from a measured zero.
    #[serde(default)]
    pub extract_fact_evidence_telemetry_complete: bool,
    /// In-memory HTTP status passed to the managed phase sidecar. Kept out of
    /// the shared observation wire format so its schema version stays stable.
    /// Only the numeric status is retained; response headers and body are
    /// discarded.
    #[serde(default, skip_serializing)]
    pub terminal_http_status_code: Option<u16>,
    /// Content-free phase where the optional Chat Completions adapter failed.
    /// Kept out of the shared observation wire format; managed sidecars may
    /// persist this diagnostic without retaining provider response content.
    #[serde(default, skip_serializing)]
    pub terminal_transport_stage: Option<TerminalTransportStage>,
    pub turns_used: u32,
    pub tool_calls: u32,
    pub user_clarifications: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub wall_time_ms: u64,
    pub status: TrialStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryReadObservation {
    pub request_sequence: u32,
    pub response_sequence: Option<u32>,
    /// Monotonic elapsed time between the observed request and its terminal
    /// response. Interrupted reads have no duration; zero is a valid measured
    /// duration at millisecond precision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    pub outcome: MemoryReadOutcome,
    pub item_count: Option<u32>,
    /// SHA-256 of the exact JSON query argument; query text is not retained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_query_sha256: Option<String>,
    /// SHA-256 of all request arguments except `query`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub non_query_arguments_sha256: Option<String>,
    /// Per-field hashes of the effective `assemble_context` selector values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector_arguments_sha256: Option<BTreeMap<String, String>>,
    /// Per-field presence in the request, preserving omitted versus explicit null.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector_arguments_present: Option<BTreeMap<String, bool>>,
    /// SHA-256 of normalized requested valid time; the timestamp is not retained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_valid_at_sha256: Option<String>,
    /// Hash of the requested valid-time and revision pair, retained even when
    /// the corresponding tool call fails before returning evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_cutoff_sha256: Option<String>,
    /// SHA-256 of returned guidance text; guidance itself is not retained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_guidance_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<MemoryReadEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryReadEvidence {
    pub source_episode_ids: Vec<String>,
    pub claim_annotations: Vec<crate::independent_probe::ProbeClaimAnnotation>,
    pub completeness: Option<ReadCompleteness>,
    pub requested_cutoff_sha256: Option<String>,
    pub returned_cutoff_sha256: Option<String>,
    /// SHA-256 of normalized snapshot valid time; the timestamp is not retained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returned_valid_at_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadCompleteness {
    Complete,
    Partial,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryReadDiagnostic {
    pub request_sequence: u32,
    pub response_sequence: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    pub outcome: MemoryReadOutcome,
    pub item_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_query_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub non_query_arguments_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector_expectation_registered: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector_argument_matches: Option<BTreeMap<String, bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_guidance_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_valid_at_match: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returned_valid_at_match: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_cutoff_match: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returned_cutoff_match: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_coverage: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotation_match: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completeness: Option<ReadCompleteness>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cited_returned_source_count: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryReadOutcome {
    Failed,
    Empty,
    Nonempty,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadCaptureStatus {
    Observed,
    Partial,
    Unavailable,
}

pub(crate) fn validate_read_associations(
    status: ReadCaptureStatus,
    reads: &[MemoryReadObservation],
    trace: &[AgentTraceEvent],
) -> Result<(), EvalError> {
    let invalid = || EvalError::InvalidInput("invalid read/reply association or coverage".into());
    let mut requests = BTreeSet::new();
    if status == ReadCaptureStatus::Unavailable && !reads.is_empty() {
        return Err(invalid());
    }
    for read in reads {
        let request = trace
            .iter()
            .find(|event| event.sequence == read.request_sequence)
            .filter(|event| {
                event.kind == TraceEventKind::MemoryRead
                    && matches!(
                        event.tool_name.as_deref(),
                        Some("assemble_context" | "memory_decisions/assemble_context")
                    )
            })
            .ok_or_else(invalid)?;
        if !requests.insert(read.request_sequence) {
            return Err(invalid());
        }
        let mut terminals = trace.iter().filter(|event| {
            event.call_id == request.call_id
                && matches!(
                    event.kind,
                    TraceEventKind::ToolResponse | TraceEventKind::ToolError
                )
        });
        let terminal = terminals.next();
        if request.call_id.is_none()
            || terminals.next().is_some()
            || terminal.map(|event| event.sequence) != read.response_sequence
            || (read.response_sequence.is_none() && read.elapsed_ms.is_some())
            || terminal.is_some_and(|event| {
                event.sequence <= request.sequence || event.tool_name != request.tool_name
            })
        {
            return Err(invalid());
        }
        let valid = match (
            terminal.map(|event| event.kind),
            read.outcome,
            read.item_count,
        ) {
            (Some(TraceEventKind::ToolError), MemoryReadOutcome::Failed, None)
            | (Some(TraceEventKind::ToolResponse), MemoryReadOutcome::Empty, Some(0))
            | (Some(TraceEventKind::ToolResponse) | None, MemoryReadOutcome::Unknown, None) => true,
            (Some(TraceEventKind::ToolResponse), MemoryReadOutcome::Nonempty, Some(count)) => {
                count > 0
            }
            _ => false,
        };
        if !valid {
            return Err(invalid());
        }
    }
    if status == ReadCaptureStatus::Observed
        && trace.iter().any(|event| {
            event.kind == TraceEventKind::MemoryRead && !requests.contains(&event.sequence)
        })
    {
        return Err(invalid());
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalTransportStage {
    RequestSend,
    ResponseBodyRead,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractFactEvidenceObservation {
    /// Sequence of the `extract` request in the agent trace.
    pub request_sequence: u32,
    /// Sequence of its response, if the call completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_sequence: Option<u32>,
    /// Whether the request explicitly asked for persisted fact evidence.
    pub requested: bool,
    /// Whether the successful response contained at least one nonempty
    /// persisted statement and source quote.
    pub returned: bool,
    /// Serialized MCP response bytes, measured transiently and retained only
    /// as a number. Missing on failed calls and legacy receipts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_wire_bytes: Option<u64>,
}

pub trait PairedTrialBackend {
    /// Resolve public temporal markers once before either arm receives the task.
    fn prepare(&mut self, task: &PairedTaskInput) -> Result<PreparedPairedTask, EvalError> {
        if task.knowledge_before_event_id.is_some()
            || task.prompt.contains(PAIRED_KNOWN_REVISION_PLACEHOLDER)
        {
            return Err(EvalError::InvalidConfig(
                "paired backend does not support exact knowledge-revision checkpoints".into(),
            ));
        }
        Ok(PreparedPairedTask {
            input: task.clone(),
            expected_known_revision: None,
        })
    }

    /// Run the agent using only the resolved public trial input.
    fn execute(
        &mut self,
        arm: RunArm,
        task: &PreparedPairedTask,
    ) -> Result<PairedTrialExecution, EvalError>;
}

impl PairedTaskManifest {
    pub fn validate(&self, plan: &PairedEvaluationPlan) -> Result<(), EvalError> {
        plan.validate()?;
        self.validate_schema_contract()?;
        let manifest_keys = self
            .tasks
            .iter()
            .map(|task| task.key.clone())
            .collect::<Vec<_>>();
        crate::paired::validate_case_coverage(
            &plan.expected_trials,
            &manifest_keys,
            &manifest_keys,
        )?;
        for task in &self.tasks {
            validate_task(task)?;
        }
        if self.corpus_sha256()? != plan.shared_identity.corpus_sha256 {
            return Err(EvalError::InvalidInput(
                "paired task manifest does not match the corpus hash in the plan".into(),
            ));
        }
        if self.oracle_sha256()? != plan.shared_identity.oracle_sha256 {
            return Err(EvalError::InvalidInput(
                "paired task manifest does not match the oracle hash in the plan".into(),
            ));
        }
        Ok(())
    }

    fn validate_schema_contract(&self) -> Result<(), EvalError> {
        if !matches!(
            self.schema_version.as_str(),
            PAIRED_TASK_MANIFEST_SCHEMA_V1
                | PAIRED_TASK_MANIFEST_SCHEMA_V2
                | PAIRED_TASK_MANIFEST_SCHEMA_V3
        ) {
            return Err(EvalError::InvalidInput(format!(
                "unsupported paired task manifest schema {:?}",
                self.schema_version
            )));
        }
        for task in &self.tasks {
            if task.oracle.read_expectation.is_some()
                && self.schema_version == PAIRED_TASK_MANIFEST_SCHEMA_V1
            {
                return Err(EvalError::InvalidInput(
                    "read expectations require task manifest v2".into(),
                ));
            }
            if task
                .oracle
                .read_expectation
                .as_ref()
                .is_some_and(|expectation| expectation.snapshot_bound_retry.is_some())
                && self.schema_version != PAIRED_TASK_MANIFEST_SCHEMA_V3
            {
                return Err(EvalError::InvalidInput(
                    "snapshot-bound retry requires paired task manifest v3".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn corpus_sha256(&self) -> Result<String, EvalError> {
        Ok(sha256(&serde_json::to_vec(&self.runner_inputs())?))
    }

    fn runner_inputs(&self) -> Vec<PairedTaskInput> {
        let mut tasks = self
            .tasks
            .iter()
            .map(PairedTaskDefinition::runner_input)
            .collect::<Vec<_>>();
        tasks.sort_by(|left, right| left.key.cmp(&right.key));
        tasks
    }

    pub fn oracle_sha256(&self) -> Result<String, EvalError> {
        let mut oracles = self
            .tasks
            .iter()
            .map(|task| (&task.key, &task.oracle))
            .collect::<Vec<_>>();
        oracles.sort_by(|left, right| left.0.cmp(right.0));
        Ok(sha256(&serde_json::to_vec(&oracles)?))
    }
}

fn validate_task(task: &PairedTaskDefinition) -> Result<(), EvalError> {
    if task.prompt.trim().is_empty() || task.seed_episodes.is_empty() {
        return Err(EvalError::InvalidInput(format!(
            "paired task {} requires a prompt and at least one seed episode",
            task.key.case_id
        )));
    }
    let mut event_ids = BTreeSet::new();
    for episode in &task.seed_episodes {
        if episode.event_id.trim().is_empty()
            || episode.source_id.trim().is_empty()
            || episode.source_type.trim().is_empty()
            || episode.content.trim().is_empty()
            || !event_ids.insert(episode.event_id.as_str())
        {
            return Err(EvalError::InvalidInput(format!(
                "paired task {} contains empty or duplicate seed-episode fields",
                task.key.case_id
            )));
        }
    }
    let marker_count = task
        .prompt
        .matches(PAIRED_KNOWN_REVISION_PLACEHOLDER)
        .count();
    match task.knowledge_before_event_id.as_deref() {
        Some(event_id)
            if event_ids.contains(event_id) && marker_count == 1 && !event_id.trim().is_empty() => {
        }
        Some(_) => {
            return Err(EvalError::InvalidInput(format!(
                "paired task {} requires a known-revision placeholder and a valid boundary event",
                task.key.case_id
            )));
        }
        None if marker_count == 0 => {}
        None => {
            return Err(EvalError::InvalidInput(format!(
                "paired task {} has a known-revision placeholder without a boundary event",
                task.key.case_id
            )));
        }
    }
    if task.oracle.accepted_answers.is_empty()
        || task
            .oracle
            .accepted_answers
            .iter()
            .any(|answer| answer.trim().is_empty())
    {
        return Err(EvalError::InvalidInput(format!(
            "paired task {} has an empty accepted answer",
            task.key.case_id
        )));
    }
    for evidence_group in task
        .oracle
        .sufficient_evidence_alternatives
        .iter()
        .flatten()
    {
        if evidence_group.is_empty()
            || evidence_group
                .iter()
                .any(|event_id| !event_ids.contains(event_id.as_str()))
        {
            return Err(EvalError::InvalidInput(format!(
                "paired task {} has an empty or unknown sufficient-evidence group",
                task.key.case_id
            )));
        }
    }
    if let Some(expectation) = &task.oracle.read_expectation {
        if let Some(retry) = &expectation.snapshot_bound_retry {
            crate::paired::validate_sha256(
                "authorized retry guidance",
                &retry.authorized_guidance_sha256,
            )?;
            if expectation.selector_arguments.is_some() {
                return Err(EvalError::InvalidInput(
                    "snapshot-bound retry cannot use a static selector oracle".into(),
                ));
            }
        }
        if let Some(selector_arguments) = &expectation.selector_arguments {
            tool_results::validate_expected_selector_arguments(selector_arguments)?;
            if let Some(expected_valid_at) = expectation.valid_at {
                let selector_valid_at = selector_arguments
                    .get("valid_at")
                    .and_then(Value::as_str)
                    .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                    .map(|value| value.with_timezone(&Utc));
                if selector_valid_at != Some(expected_valid_at) {
                    return Err(EvalError::InvalidInput(
                        "registered read selector valid_at contradicts its read expectation".into(),
                    ));
                }
            }
        }
        for alternative in &expectation.required_claim_annotation_alternatives {
            if alternative.is_empty() {
                return Err(EvalError::InvalidInput(
                    "empty read annotation alternative".into(),
                ));
            }
            for annotation in alternative {
                crate::independent_probe::validate_claim_annotation(annotation)?;
                if !task
                    .oracle
                    .sufficient_evidence_alternatives
                    .iter()
                    .flatten()
                    .flatten()
                    .any(|id| id == &annotation.source_event_id)
                {
                    return Err(EvalError::InvalidInput(
                        "read annotation source is not registered as required evidence".into(),
                    ));
                }
            }
        }
    }
    if task.oracle.forbidden_event_ids.iter().any(|event_id| {
        !event_ids.contains(event_id.as_str())
            || task
                .oracle
                .sufficient_evidence_alternatives
                .iter()
                .flatten()
                .any(|group| group.iter().any(|allowed| allowed == event_id))
    }) {
        return Err(EvalError::InvalidInput(format!(
            "paired task {} has unknown or simultaneously forbidden required evidence",
            task.key.case_id
        )));
    }
    Ok(())
}

pub fn run_paired_agent_evaluation<B: PairedTrialBackend>(
    plan: &PairedEvaluationPlan,
    manifest: &PairedTaskManifest,
    backend: &mut B,
    output_directory: &Path,
) -> Result<(PairedRunArtifact, PairedRunArtifact), EvalError> {
    manifest.validate(plan)?;
    run_paired_agent_evaluation_validated(plan, manifest, backend, output_directory)
}

pub(crate) fn run_paired_agent_evaluation_validated<B: PairedTrialBackend>(
    plan: &PairedEvaluationPlan,
    manifest: &PairedTaskManifest,
    backend: &mut B,
    output_directory: &Path,
) -> Result<(PairedRunArtifact, PairedRunArtifact), EvalError> {
    manifest.validate_schema_contract()?;
    validate_output_directory(output_directory)?;
    let plan_sha256 = plan.sha256()?;
    let incumbent_journal = output_directory.join("incumbent-receipts.jsonl");
    let candidate_journal = output_directory.join("candidate-receipts.jsonl");
    let incumbent_artifact = output_directory.join("incumbent.json");
    let candidate_artifact = output_directory.join("candidate.json");
    for path in [
        &incumbent_journal,
        &candidate_journal,
        &incumbent_artifact,
        &candidate_artifact,
    ] {
        if path.exists() {
            return Err(EvalError::InvalidConfig(format!(
                "paired output path already exists: {}",
                path.display()
            )));
        }
    }
    create_empty_file(&incumbent_journal)?;
    create_empty_file(&candidate_journal)?;
    let run_id = new_run_id()?;
    let run_started_at = Utc::now();

    let mut incumbent = PairedRunArtifact {
        schema_version: PAIRED_ARTIFACT_SCHEMA_V6.into(),
        arm: RunArm::Incumbent,
        run_id: Some(run_id.clone()),
        run_started_at,
        run_completed_at: run_started_at,
        identity: crate::paired::PairedRunIdentity {
            shared: plan.shared_identity.clone(),
            product: plan.incumbent_product.clone(),
        },
        expected_trials: plan.expected_trials.clone(),
        receipts: Vec::with_capacity(plan.expected_trials.len()),
    };
    let mut candidate = PairedRunArtifact {
        schema_version: PAIRED_ARTIFACT_SCHEMA_V6.into(),
        arm: RunArm::Candidate,
        run_id: Some(run_id),
        run_started_at,
        run_completed_at: run_started_at,
        identity: crate::paired::PairedRunIdentity {
            shared: plan.shared_identity.clone(),
            product: plan.candidate_product.clone(),
        },
        expected_trials: plan.expected_trials.clone(),
        receipts: Vec::with_capacity(plan.expected_trials.len()),
    };
    let tasks = manifest
        .tasks
        .iter()
        .map(|task| (task.key.clone(), task))
        .collect::<BTreeMap<_, _>>();
    let snapshot_retry_requirements = manifest
        .tasks
        .iter()
        .filter_map(|task| {
            task.oracle
                .read_expectation
                .as_ref()
                .and_then(|expectation| expectation.snapshot_bound_retry.as_ref())
                .map(|retry| (task.key.clone(), retry.retry_required_when_eligible))
        })
        .collect::<BTreeMap<_, _>>();
    let snapshot_retry_registered = !snapshot_retry_requirements.is_empty();

    let mut expected = plan.expected_trials.clone();
    expected.sort();
    for key in expected {
        let task = tasks.get(&key).ok_or_else(|| {
            EvalError::InvalidInput(format!("task manifest lacks case {}", key.case_id))
        })?;
        let public_input = task.runner_input();
        let prepared_task = backend.prepare(&public_input)?;
        validate_prepared_task(&public_input, &prepared_task)?;
        let (first_arm, second_arm) = if key.incumbent_first {
            (RunArm::Incumbent, RunArm::Candidate)
        } else {
            (RunArm::Candidate, RunArm::Incumbent)
        };
        for arm in [first_arm, second_arm] {
            let execution = backend.execute(arm, &prepared_task)?;
            let product_identity = match arm {
                RunArm::Incumbent => &plan.incumbent_product,
                RunArm::Candidate => &plan.candidate_product,
            };
            let identity = crate::paired::PairedRunIdentity {
                shared: plan.shared_identity.clone(),
                product: product_identity.clone(),
            };
            let mut receipt = score_trial(
                task,
                arm_order_for(arm, key.incumbent_first),
                prepared_task.expected_known_revision,
                &execution,
            )?;
            if snapshot_retry_registered && receipt.snapshot_retry_diagnostic.is_none() {
                receipt.snapshot_retry_diagnostic = Some(
                    snapshot_retry_requirements
                        .get(&receipt.key)
                        .copied()
                        .map(not_observed_snapshot_retry_diagnostic)
                        .unwrap_or_else(unregistered_snapshot_retry_diagnostic),
                );
            }
            crate::paired::validate_trial_receipt(&receipt, &identity.shared)?;
            let journal_path = match arm {
                RunArm::Incumbent => &incumbent_journal,
                RunArm::Candidate => &candidate_journal,
            };
            let valid_time_registered = task
                .oracle
                .read_expectation
                .as_ref()
                .is_some_and(|expectation| expectation.valid_at.is_some());
            let selector_expectation_registered = task
                .oracle
                .read_expectation
                .as_ref()
                .is_some_and(|expectation| expectation.selector_arguments.is_some());
            append_receipt_journal(
                journal_path,
                &plan_sha256,
                arm,
                &receipt,
                valid_time_registered,
                selector_expectation_registered,
                snapshot_retry_registered,
            )?;
            let artifact = match arm {
                RunArm::Incumbent => &mut incumbent,
                RunArm::Candidate => &mut candidate,
            };
            if receipt.read_capture_status.is_some() {
                artifact.schema_version = crate::paired::PAIRED_ARTIFACT_SCHEMA_V7.into();
            }
            artifact.receipts.push(receipt);
        }
    }

    for artifact in [&mut incumbent, &mut candidate] {
        let has_read_capture = artifact
            .receipts
            .iter()
            .any(|receipt| receipt.read_capture_status.is_some());
        let has_complete_request_fingerprints = artifact
            .receipts
            .iter()
            .filter(|receipt| receipt.read_capture_status.is_some())
            .all(|receipt| {
                receipt.read_diagnostics.as_ref().is_some_and(|reads| {
                    reads.iter().all(|read| {
                        read.request_query_sha256.is_some()
                            && read.non_query_arguments_sha256.is_some()
                    })
                })
            });
        let has_complete_elapsed_read_timing = has_read_capture
            && artifact
                .receipts
                .iter()
                .filter(|receipt| receipt.read_capture_status.is_some())
                .all(|receipt| {
                    receipt.read_diagnostics.as_ref().is_some_and(|reads| {
                        reads.iter().all(|read| {
                            (read.response_sequence.is_some() && read.elapsed_ms.is_some())
                                || (read.response_sequence.is_none() && read.elapsed_ms.is_none())
                        })
                    })
                });
        let valid_time_registered = manifest.tasks.iter().any(|task| {
            task.oracle
                .read_expectation
                .as_ref()
                .is_some_and(|expectation| expectation.valid_at.is_some())
        });
        let selector_expectation_registered = manifest.tasks.iter().any(|task| {
            task.oracle
                .read_expectation
                .as_ref()
                .is_some_and(|expectation| expectation.selector_arguments.is_some())
        });
        let snapshot_retry_registered = !snapshot_retry_requirements.is_empty();
        if has_read_capture || snapshot_retry_registered {
            artifact.schema_version = crate::paired::paired_artifact_schema_for_reads(
                has_read_capture,
                has_complete_request_fingerprints,
                valid_time_registered,
                selector_expectation_registered,
                snapshot_retry_registered,
                has_complete_elapsed_read_timing,
            )?
            .into();
        }
        complete_selector_registration_diagnostics(artifact);
        complete_snapshot_retry_registration_diagnostics(artifact, &snapshot_retry_requirements);
    }

    let run_completed_at = Utc::now();
    incumbent.run_completed_at = run_completed_at;
    candidate.run_completed_at = run_completed_at;
    incumbent.validate()?;
    candidate.validate()?;
    write_json_new(&incumbent_artifact, &incumbent)?;
    write_json_new(&candidate_artifact, &candidate)?;
    Ok((incumbent, candidate))
}

fn new_run_id() -> Result<String, EvalError> {
    static NEXT_RUN: AtomicU64 = AtomicU64::new(0);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| EvalError::InvalidConfig(format!("system clock precedes epoch: {error}")))?
        .as_nanos();
    let sequence = NEXT_RUN.fetch_add(1, Ordering::Relaxed);
    Ok(format!("{}-{timestamp}-{sequence}", std::process::id()))
}

pub(crate) fn complete_selector_registration_diagnostics(
    artifact: &mut crate::paired::PairedRunArtifact,
) {
    if !matches!(
        artifact.schema_version.as_str(),
        crate::paired::PAIRED_ARTIFACT_SCHEMA_V10
            | crate::paired::PAIRED_ARTIFACT_SCHEMA_V11
            | crate::paired::PAIRED_ARTIFACT_SCHEMA_V12
    ) {
        return;
    }
    for read in artifact
        .receipts
        .iter_mut()
        .filter_map(|receipt| receipt.read_diagnostics.as_mut())
        .flatten()
    {
        read.selector_expectation_registered.get_or_insert(false);
    }
}

pub(crate) fn complete_snapshot_retry_registration_diagnostics(
    artifact: &mut crate::paired::PairedRunArtifact,
    registered_tasks: &BTreeMap<CaseTrialKey, bool>,
) {
    if !matches!(
        artifact.schema_version.as_str(),
        crate::paired::PAIRED_ARTIFACT_SCHEMA_V11 | crate::paired::PAIRED_ARTIFACT_SCHEMA_V12
    ) {
        return;
    }
    for receipt in &mut artifact.receipts {
        receipt.snapshot_retry_diagnostic.get_or_insert_with(|| {
            if let Some(retry_required) = registered_tasks.get(&receipt.key) {
                not_observed_snapshot_retry_diagnostic(*retry_required)
            } else {
                unregistered_snapshot_retry_diagnostic()
            }
        });
    }
}

fn unregistered_snapshot_retry_diagnostic() -> SnapshotRetryDiagnostic {
    SnapshotRetryDiagnostic {
        expectation_registered: false,
        assessment: SnapshotRetryAssessment::NotRegistered,
        retry_required_when_eligible: false,
        initial_read_eligible: None,
        retry_observed: None,
        retry_count: None,
        retry_limit_respected: None,
        query_changed: None,
        snapshot_cutoff_matches: None,
        legacy_temporal_selector_matches: None,
        preserved_selector_matches: None,
    }
}

fn not_observed_snapshot_retry_diagnostic(
    retry_required_when_eligible: bool,
) -> SnapshotRetryDiagnostic {
    SnapshotRetryDiagnostic {
        expectation_registered: true,
        assessment: SnapshotRetryAssessment::NotObserved,
        retry_required_when_eligible,
        initial_read_eligible: None,
        retry_observed: None,
        retry_count: None,
        retry_limit_respected: None,
        query_changed: None,
        snapshot_cutoff_matches: None,
        legacy_temporal_selector_matches: None,
        preserved_selector_matches: None,
    }
}

fn arm_order_for(arm: RunArm, incumbent_first: bool) -> u8 {
    if (arm == RunArm::Incumbent) == incumbent_first {
        1
    } else {
        2
    }
}

fn evidence_covers(oracle: &HiddenTaskOracle, sources: &BTreeSet<&str>) -> bool {
    if oracle.sufficient_evidence_alternatives.is_empty() {
        return sources.is_empty();
    }
    oracle
        .sufficient_evidence_alternatives
        .iter()
        .any(|alternative| {
            !alternative.is_empty()
                && alternative.iter().all(|group| {
                    group
                        .iter()
                        .any(|event_id| sources.contains(event_id.as_str()))
                })
        })
}

fn score_read(
    read: &MemoryReadObservation,
    oracle: &HiddenTaskOracle,
    checkpoint: Option<i64>,
    execution: &PairedTrialExecution,
    citations: Option<&[String]>,
    redact_guidance_fingerprint: bool,
) -> Result<MemoryReadDiagnostic, EvalError> {
    let selector_expectation = oracle
        .read_expectation
        .as_ref()
        .and_then(|expectation| expectation.selector_arguments.as_ref());
    let actual_selector_fingerprints = if selector_expectation.is_some() {
        Some(read.selector_arguments_sha256.as_ref().ok_or_else(|| {
            EvalError::InvalidInput(
                "registered selector expectations require fingerprints for every captured read"
                    .into(),
            )
        })?)
    } else {
        None
    };
    let mut diagnostic = MemoryReadDiagnostic {
        request_sequence: read.request_sequence,
        response_sequence: read.response_sequence,
        elapsed_ms: read.elapsed_ms,
        outcome: read.outcome,
        item_count: read.item_count,
        request_query_sha256: read.request_query_sha256.clone(),
        non_query_arguments_sha256: read.non_query_arguments_sha256.clone(),
        selector_expectation_registered: selector_expectation.map(|_| true),
        selector_argument_matches: None,
        response_guidance_sha256: (!redact_guidance_fingerprint)
            .then(|| read.response_guidance_sha256.clone())
            .flatten(),
        requested_valid_at_match: None,
        returned_valid_at_match: None,
        requested_cutoff_match: None,
        returned_cutoff_match: None,
        source_coverage: None,
        annotation_match: None,
        completeness: None,
        cited_returned_source_count: None,
    };
    if let Some(digest) = read.requested_valid_at_sha256.as_deref() {
        crate::paired::validate_sha256("requested valid_at", digest)?;
    }
    if let Some(digest) = read.requested_cutoff_sha256.as_deref() {
        crate::paired::validate_sha256("requested cutoff", digest)?;
    }
    if let Some(fingerprints) = &read.selector_arguments_sha256 {
        if fingerprints.len() != tool_results::ASSEMBLE_CONTEXT_SELECTOR_FIELDS.len()
            || tool_results::ASSEMBLE_CONTEXT_SELECTOR_FIELDS
                .iter()
                .any(|field| !fingerprints.contains_key(*field))
        {
            return Err(EvalError::InvalidInput(
                "read selector fingerprints do not match the fixed field set".into(),
            ));
        }
        for digest in fingerprints.values() {
            crate::paired::validate_sha256("read selector", digest)?;
        }
    }
    if let Some(presence) = &read.selector_arguments_present {
        validate_selector_presence(presence)?;
    }
    if let Some(valid_at) = oracle
        .read_expectation
        .as_ref()
        .and_then(|expectation| expectation.valid_at)
    {
        let expected = tool_results::valid_at_sha256(valid_at);
        diagnostic.requested_valid_at_match = read
            .requested_valid_at_sha256
            .as_ref()
            .map(|digest| digest == &expected);
    }
    if let (Some(expectation), Some(actual)) = (selector_expectation, actual_selector_fingerprints)
    {
        let expected = tool_results::expected_selector_argument_fingerprints(expectation)?;
        diagnostic.selector_argument_matches = Some(
            tool_results::ASSEMBLE_CONTEXT_SELECTOR_FIELDS
                .iter()
                .map(|field| {
                    let matches = actual.get(*field) == expected.get(*field);
                    ((*field).to_owned(), matches)
                })
                .collect(),
        );
    }
    let Some(evidence) = &read.evidence else {
        return Ok(diagnostic);
    };
    let invalid = || EvalError::InvalidInput("invalid private read evidence".into());
    if !matches!(
        read.outcome,
        MemoryReadOutcome::Empty | MemoryReadOutcome::Nonempty
    ) || read
        .item_count
        .and_then(|count| usize::try_from(count).ok())
        != Some(evidence.source_episode_ids.len())
    {
        return Err(invalid());
    }
    for digest in [
        &read.requested_cutoff_sha256,
        &evidence.requested_cutoff_sha256,
        &evidence.returned_cutoff_sha256,
        &evidence.returned_valid_at_sha256,
    ]
    .into_iter()
    .flatten()
    {
        crate::paired::validate_sha256("read cutoff", digest)?;
    }
    if read
        .requested_cutoff_sha256
        .as_ref()
        .zip(evidence.requested_cutoff_sha256.as_ref())
        .is_some_and(|(request, evidence_request)| request != evidence_request)
    {
        return Err(invalid());
    }
    let episode_ids: BTreeSet<_> = evidence
        .source_episode_ids
        .iter()
        .map(String::as_str)
        .collect();
    let event_ids = episode_ids
        .iter()
        .map(|id| {
            execution
                .episode_id_to_event_id
                .get(*id)
                .map(String::as_str)
                .ok_or_else(invalid)
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let mut annotations = evidence.claim_annotations.clone();
    for annotation in &mut annotations {
        crate::independent_probe::validate_claim_annotation(annotation)?;
        if !episode_ids.contains(annotation.source_event_id.as_str()) {
            return Err(invalid());
        }
        annotation.source_event_id = execution
            .episode_id_to_event_id
            .get(&annotation.source_event_id)
            .cloned()
            .ok_or_else(invalid)?;
    }
    diagnostic.source_coverage = Some(evidence_covers(oracle, &event_ids));
    diagnostic.completeness = Some(evidence.completeness.unwrap_or(ReadCompleteness::Unknown));
    diagnostic.cited_returned_source_count = citations.map(|citations| {
        citations
            .iter()
            .map(String::as_str)
            .filter(|id| episode_ids.contains(id))
            .collect::<BTreeSet<_>>()
            .len()
    });
    if let Some(expectation) = &oracle.read_expectation {
        if let Some(valid_at) = expectation.valid_at {
            let expected = tool_results::valid_at_sha256(valid_at);
            diagnostic.returned_valid_at_match = evidence
                .returned_valid_at_sha256
                .as_ref()
                .map(|digest| digest == &expected);
        }
        if !expectation
            .required_claim_annotation_alternatives
            .is_empty()
        {
            diagnostic.annotation_match = Some(
                expectation
                    .required_claim_annotation_alternatives
                    .iter()
                    .any(|alternative| {
                        alternative.iter().all(|required| {
                            annotations.iter().any(|actual| {
                                crate::independent_probe::claim_annotation_matches(required, actual)
                            })
                        })
                    }),
            );
        }
        if let (Some(valid_at), Some(revision)) = (expectation.valid_at, checkpoint) {
            // A full cutoff is only an oracle when both axes were registered.
            // Comparing against a synthetic `None` revision turns an
            // unregistered transaction boundary into a false mismatch.
            let expected = tool_results::cutoff_digest(valid_at, Some(revision))?;
            diagnostic.requested_cutoff_match = evidence
                .requested_cutoff_sha256
                .as_ref()
                .map(|digest| digest == &expected);
            diagnostic.returned_cutoff_match = evidence
                .returned_cutoff_sha256
                .as_ref()
                .map(|digest| digest == &expected);
        }
    }
    Ok(diagnostic)
}

fn score_snapshot_bound_retry(
    task: &PairedTaskDefinition,
    observation: &AgentObservation,
) -> Result<Option<SnapshotRetryDiagnostic>, EvalError> {
    use SnapshotRetryAssessment as Assessment;

    let Some(policy) = task
        .oracle
        .read_expectation
        .as_ref()
        .and_then(|expectation| expectation.snapshot_bound_retry.as_ref())
    else {
        return Ok(None);
    };

    let initial_diagnostic =
        || not_observed_snapshot_retry_diagnostic(policy.retry_required_when_eligible);

    let (Some(ReadCaptureStatus::Observed), Some(reads)) = (
        observation.read_capture_status,
        observation.memory_reads.as_ref(),
    ) else {
        return Ok(Some(initial_diagnostic()));
    };
    let mut ordered = reads.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|read| read.request_sequence);
    let Some(initial) = ordered.first().copied() else {
        let mut diagnostic = initial_diagnostic();
        diagnostic.retry_observed = Some(false);
        diagnostic.retry_count = Some(0);
        diagnostic.retry_limit_respected = Some(true);
        return Ok(Some(diagnostic));
    };
    let retry_count = u32::try_from(ordered.len().saturating_sub(1))
        .map_err(|_| EvalError::InvalidInput("snapshot retry count overflowed".into()))?;
    let retry_observed = retry_count > 0;
    let retry_limit_respected = retry_count <= 1;
    let mut diagnostic = initial_diagnostic();
    diagnostic.retry_observed = Some(retry_observed);
    diagnostic.retry_count = Some(retry_count);
    diagnostic.retry_limit_respected = Some(retry_limit_respected);

    let eligibility = match (initial.outcome, initial.evidence.as_ref()) {
        (MemoryReadOutcome::Failed | MemoryReadOutcome::Nonempty, _) => Some(false),
        (MemoryReadOutcome::Unknown, _) => None,
        (MemoryReadOutcome::Empty, Some(evidence)) => match evidence.completeness {
            Some(ReadCompleteness::Complete) => Some(false),
            Some(ReadCompleteness::Unknown) | None => None,
            Some(ReadCompleteness::Partial) => {
                let (Some(guidance), Some(returned_cutoff)) = (
                    initial.response_guidance_sha256.as_deref(),
                    evidence.returned_cutoff_sha256.as_deref(),
                ) else {
                    return Ok(Some(diagnostic));
                };
                crate::paired::validate_sha256("authorized retry guidance", guidance)?;
                crate::paired::validate_sha256("returned snapshot cutoff", returned_cutoff)?;
                Some(guidance == policy.authorized_guidance_sha256)
            }
        },
        (MemoryReadOutcome::Empty, None) => None,
    };

    let Some(eligible) = eligibility else {
        return Ok(Some(diagnostic));
    };
    if !eligible {
        diagnostic.assessment = if retry_observed {
            Assessment::Noncompliant
        } else {
            Assessment::NotEligible
        };
        diagnostic.initial_read_eligible = Some(false);
        return Ok(Some(diagnostic));
    }

    if !retry_observed {
        diagnostic.assessment = if policy.retry_required_when_eligible {
            Assessment::Noncompliant
        } else {
            Assessment::Compliant
        };
        diagnostic.initial_read_eligible = Some(true);
        return Ok(Some(diagnostic));
    }

    let Some(retry) = ordered.get(1).copied() else {
        return Err(EvalError::InvalidInput(
            "snapshot retry count and observations disagree".into(),
        ));
    };
    let Some(initial_response_sequence) = initial.response_sequence else {
        diagnostic.initial_read_eligible = Some(true);
        return Ok(Some(diagnostic));
    };
    if initial_response_sequence >= retry.request_sequence {
        diagnostic.assessment = Assessment::Noncompliant;
        diagnostic.initial_read_eligible = Some(true);
        return Ok(Some(diagnostic));
    }

    let (Some(initial_query), Some(retry_query)) = (
        initial.request_query_sha256.as_deref(),
        retry.request_query_sha256.as_deref(),
    ) else {
        diagnostic.initial_read_eligible = Some(true);
        return Ok(Some(diagnostic));
    };
    crate::paired::validate_sha256("initial retry query", initial_query)?;
    crate::paired::validate_sha256("retry query", retry_query)?;
    let query_changed = initial_query != retry_query;

    let (Some(returned_cutoff), Some(requested_cutoff)) = (
        initial
            .evidence
            .as_ref()
            .and_then(|evidence| evidence.returned_cutoff_sha256.as_deref()),
        retry.requested_cutoff_sha256.as_deref(),
    ) else {
        diagnostic.initial_read_eligible = Some(true);
        diagnostic.query_changed = Some(query_changed);
        return Ok(Some(diagnostic));
    };
    crate::paired::validate_sha256("returned snapshot cutoff", returned_cutoff)?;
    crate::paired::validate_sha256("requested retry cutoff", requested_cutoff)?;
    let snapshot_cutoff_matches = returned_cutoff == requested_cutoff;

    let Some(presence) = retry.selector_arguments_present.as_ref() else {
        diagnostic.initial_read_eligible = Some(true);
        diagnostic.query_changed = Some(query_changed);
        diagnostic.snapshot_cutoff_matches = Some(snapshot_cutoff_matches);
        return Ok(Some(diagnostic));
    };
    validate_selector_presence(presence)?;
    let legacy_temporal_selector_matches = BTreeMap::from([
        ("as_of".to_owned(), presence.get("as_of") == Some(&false)),
        (
            "known_at".to_owned(),
            presence.get("known_at") == Some(&false),
        ),
    ]);

    let (Some(initial_selectors), Some(retry_selectors)) = (
        initial.selector_arguments_sha256.as_ref(),
        retry.selector_arguments_sha256.as_ref(),
    ) else {
        diagnostic.initial_read_eligible = Some(true);
        diagnostic.query_changed = Some(query_changed);
        diagnostic.snapshot_cutoff_matches = Some(snapshot_cutoff_matches);
        diagnostic.legacy_temporal_selector_matches = Some(legacy_temporal_selector_matches);
        return Ok(Some(diagnostic));
    };
    validate_selector_fingerprints(initial_selectors)?;
    validate_selector_fingerprints(retry_selectors)?;
    let preserved_selector_matches = SNAPSHOT_RETRY_PRESERVED_SELECTOR_FIELDS
        .into_iter()
        .map(|field| {
            (
                field.to_owned(),
                initial_selectors.get(field) == retry_selectors.get(field),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let compliant = retry_limit_respected
        && query_changed
        && snapshot_cutoff_matches
        && legacy_temporal_selector_matches
            .values()
            .all(|matches| *matches)
        && preserved_selector_matches.values().all(|matches| *matches);
    diagnostic.assessment = if compliant {
        Assessment::Compliant
    } else {
        Assessment::Noncompliant
    };
    diagnostic.initial_read_eligible = Some(true);
    diagnostic.query_changed = Some(query_changed);
    diagnostic.snapshot_cutoff_matches = Some(snapshot_cutoff_matches);
    diagnostic.legacy_temporal_selector_matches = Some(legacy_temporal_selector_matches);
    diagnostic.preserved_selector_matches = Some(preserved_selector_matches);
    Ok(Some(diagnostic))
}

fn validate_selector_presence(presence: &BTreeMap<String, bool>) -> Result<(), EvalError> {
    if presence.len() != tool_results::ASSEMBLE_CONTEXT_SELECTOR_FIELDS.len()
        || tool_results::ASSEMBLE_CONTEXT_SELECTOR_FIELDS
            .iter()
            .any(|field| !presence.contains_key(*field))
    {
        return Err(EvalError::InvalidInput(
            "read selector presence does not match the fixed field set".into(),
        ));
    }
    Ok(())
}

fn validate_selector_fingerprints(
    fingerprints: &BTreeMap<String, String>,
) -> Result<(), EvalError> {
    if fingerprints.len() != tool_results::ASSEMBLE_CONTEXT_SELECTOR_FIELDS.len()
        || tool_results::ASSEMBLE_CONTEXT_SELECTOR_FIELDS
            .iter()
            .any(|field| !fingerprints.contains_key(*field))
    {
        return Err(EvalError::InvalidInput(
            "read selector fingerprints do not match the fixed field set".into(),
        ));
    }
    for digest in fingerprints.values() {
        crate::paired::validate_sha256("read selector", digest)?;
    }
    Ok(())
}

fn score_trial(
    task: &PairedTaskDefinition,
    arm_order: u8,
    known_revision_checkpoint: Option<i64>,
    execution: &PairedTrialExecution,
) -> Result<TrialReceipt, EvalError> {
    #[derive(Deserialize)]
    struct FinalAnswer {
        answer: String,
        citations: Vec<String>,
    }

    let observation = &execution.observation;
    let mut violations = BTreeSet::new();
    let retrieved = observation
        .memory_source_episode_ids
        .iter()
        .map(|episode_id| {
            execution
                .episode_id_to_event_id
                .get(episode_id)
                .cloned()
                .ok_or_else(|| {
                    violations.insert(HardViolation::UnsupportedEvidence);
                    EvalError::InvalidInput(
                        "memory output referenced an episode outside the seeded task history"
                            .into(),
                    )
                })
        })
        .collect::<Result<BTreeSet<_>, _>>();
    let retrieved_mapping_valid = retrieved.is_ok();
    let retrieved = retrieved.unwrap_or_default();
    let forbidden = task
        .oracle
        .forbidden_event_ids
        .iter()
        .any(|event_id| retrieved.contains(event_id));
    if forbidden {
        violations.insert(HardViolation::ProhibitedEvidence);
    }
    let evidence_passed = evidence_covers(
        &task.oracle,
        &retrieved.iter().map(String::as_str).collect(),
    );
    let final_answer = observation
        .final_answer
        .as_deref()
        .and_then(|answer| serde_json::from_str::<FinalAnswer>(answer).ok());
    let accepted_answer_match = final_answer
        .as_ref()
        .map(|answer| classify_accepted_answer(&answer.answer, &task.oracle.accepted_answers));
    let answer_passed = accepted_answer_match == Some(AcceptedAnswerMatch::Exact);
    let cited_events = final_answer
        .iter()
        .flat_map(|answer| &answer.citations)
        .map(|episode_id| {
            if !observation
                .memory_source_episode_ids
                .iter()
                .any(|retrieved_id| retrieved_id == episode_id)
            {
                violations.insert(HardViolation::UnsupportedEvidence);
            }
            match execution.episode_id_to_event_id.get(episode_id) {
                Some(event_id) => Some(event_id.as_str()),
                None => {
                    violations.insert(HardViolation::UnsupportedEvidence);
                    None
                }
            }
        })
        .collect::<Vec<_>>();
    if cited_events.iter().any(|event_id| {
        event_id.is_some_and(|event_id| {
            task.oracle
                .forbidden_event_ids
                .iter()
                .any(|forbidden| forbidden == event_id)
        })
    }) {
        violations.insert(HardViolation::ProhibitedEvidence);
    }
    let cited_set = cited_events.into_iter().flatten().collect::<BTreeSet<_>>();
    let cited_evidence_passed = evidence_covers(&task.oracle, &cited_set);
    if observation.user_clarifications > 1
        || observation
            .trace
            .iter()
            .any(|event| event.kind == TraceEventKind::MemoryWrite)
    {
        violations.insert(HardViolation::UnsafeAction);
    }
    let clarification_passed =
        (observation.user_clarifications > 0) == task.oracle.needs_clarification;
    let task_oracle_diagnostics = TaskOracleDiagnostics {
        final_answer_present: observation.final_answer.is_some(),
        final_answer_schema_valid: final_answer.is_some(),
        accepted_answer: final_answer.as_ref().map(|_| answer_passed),
        accepted_answer_match,
        required_evidence_cited: final_answer.as_ref().map(|_| cited_evidence_passed),
        clarification_matches: clarification_passed,
    };
    let task_oracle_passed = observation.status == TrialStatus::Completed
        && task_oracle_diagnostics.accepted_answer == Some(true)
        && task_oracle_diagnostics.required_evidence_cited == Some(true)
        && task_oracle_diagnostics.clarification_matches
        && violations.is_empty();
    let repair_turns = observation
        .trace
        .iter()
        .filter(|event| event.kind == TraceEventKind::ToolError)
        .map(|event| event.turn)
        .collect::<BTreeSet<_>>()
        .len();
    let repair_turns = u32::try_from(repair_turns)
        .map_err(|_| EvalError::InvalidInput("repair-turn count overflowed".into()))?;
    let mut trace = observation.trace.clone();
    if let (Some(status), Some(reads)) =
        (observation.read_capture_status, &observation.memory_reads)
    {
        validate_read_associations(status, reads, &trace)?;
        let mut identifiers = BTreeMap::new();
        for event in &mut trace {
            if let Some(id) = &mut event.call_id {
                let next = identifiers.len() + 1;
                *id = identifiers
                    .entry(std::mem::take(id))
                    .or_insert_with(|| format!("call-{next}"))
                    .clone();
            }
        }
    }
    Ok(TrialReceipt {
        read_capture_status: observation.read_capture_status,
        read_diagnostics: observation
            .memory_reads
            .as_ref()
            .filter(|_| observation.read_capture_status.is_some())
            .map(|reads| {
                reads
                    .iter()
                    .map(|read| {
                        score_read(
                            read,
                            &task.oracle,
                            known_revision_checkpoint,
                            execution,
                            final_answer
                                .as_ref()
                                .map(|answer| answer.citations.as_slice()),
                            task.oracle
                                .read_expectation
                                .as_ref()
                                .is_some_and(|expectation| {
                                    expectation.snapshot_bound_retry.is_some()
                                }),
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?,
        snapshot_retry_diagnostic: score_snapshot_bound_retry(task, observation)?,
        key: task.key.clone(),
        arm_order,
        known_revision_checkpoint,
        status: observation.status,
        direct_memory_success: observation.status != TrialStatus::Skipped
            && evidence_passed
            && !forbidden
            && retrieved_mapping_valid,
        task_oracle_passed,
        task_oracle_diagnostics: Some(task_oracle_diagnostics),
        turns_used: observation.turns_used,
        tool_calls: observation.tool_calls,
        repair_turns,
        user_clarifications: observation.user_clarifications,
        input_tokens: observation.input_tokens,
        output_tokens: observation.output_tokens,
        wall_time_ms: observation.wall_time_ms,
        hard_violations: violations.into_iter().collect(),
        trace,
    })
}

fn normalize_answer(answer: &str) -> String {
    answer
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn classify_accepted_answer(answer: &str, accepted_answers: &[String]) -> AcceptedAnswerMatch {
    if accepted_answers
        .iter()
        .any(|expected| normalize_answer(expected) == normalize_answer(answer))
    {
        return AcceptedAnswerMatch::Exact;
    }
    if accepted_answers
        .iter()
        .any(|expected| contains_literal_answer(answer, expected))
    {
        AcceptedAnswerMatch::MentionedWithExtraText
    } else {
        AcceptedAnswerMatch::NoAcceptedValueMentioned
    }
}

fn contains_literal_answer(answer: &str, accepted_answer: &str) -> bool {
    let answer = normalize_answer(answer);
    let accepted_answer = normalize_answer(accepted_answer);
    if accepted_answer.is_empty() {
        return false;
    }

    answer
        .match_indices(&accepted_answer)
        .any(|(start, matched)| {
            let end = start + matched.len();
            let before_is_boundary = answer[..start]
                .chars()
                .next_back()
                .is_none_or(|character| !is_answer_word_character(character));
            let after_is_boundary = answer[end..]
                .chars()
                .next()
                .is_none_or(|character| !is_answer_word_character(character));
            before_is_boundary && after_is_boundary
        })
}

fn is_answer_word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn resolve_known_revision_prompt(prompt: &str, revision: i64) -> Result<String, EvalError> {
    if prompt.matches(PAIRED_KNOWN_REVISION_PLACEHOLDER).count() != 1 {
        return Err(EvalError::InvalidInput(
            "paired prompt must contain exactly one known-revision placeholder".into(),
        ));
    }
    Ok(prompt.replace(PAIRED_KNOWN_REVISION_PLACEHOLDER, &revision.to_string()))
}

fn validate_prepared_task(
    original: &PairedTaskInput,
    prepared: &PreparedPairedTask,
) -> Result<(), EvalError> {
    if prepared.input.key != original.key
        || prepared.input.seed_episodes != original.seed_episodes
        || prepared.input.knowledge_before_event_id != original.knowledge_before_event_id
    {
        return Err(EvalError::InvalidInput(
            "paired preparation changed the preregistered task history or identity".into(),
        ));
    }
    match (
        original.knowledge_before_event_id.is_some(),
        prepared.expected_known_revision,
    ) {
        (true, Some(revision)) if revision >= 0 => {
            if prepared.input.prompt != resolve_known_revision_prompt(&original.prompt, revision)? {
                return Err(EvalError::InvalidInput(
                    "paired preparation changed more than the known-revision prompt marker".into(),
                ));
            }
        }
        (true, _) => {
            return Err(EvalError::InvalidInput(
                "temporal paired task preparation requires a non-negative exact revision".into(),
            ));
        }
        (false, None) if prepared.input.prompt == original.prompt => {}
        (false, _) => {
            return Err(EvalError::InvalidInput(
                "paired preparation changed a task without a temporal checkpoint".into(),
            ));
        }
    }
    Ok(())
}

fn validate_output_directory(path: &Path) -> Result<(), EvalError> {
    if !path.is_absolute() {
        return Err(EvalError::InvalidConfig(
            "paired output directory must be absolute".into(),
        ));
    }
    std::fs::create_dir_all(path).map_err(|source| EvalError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn create_empty_file(path: &Path) -> Result<(), EvalError> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map(|_| ())
        .map_err(|source| EvalError::Io {
            path: path.to_path_buf(),
            source,
        })
}

fn append_receipt_journal(
    path: &Path,
    plan_sha256: &str,
    arm: RunArm,
    receipt: &TrialReceipt,
    valid_time_registered: bool,
    selector_expectation_registered: bool,
    snapshot_retry_registered: bool,
) -> Result<(), EvalError> {
    #[derive(Serialize)]
    struct JournalRecord<'a> {
        schema_version: &'static str,
        plan_sha256: &'a str,
        arm: RunArm,
        receipt: &'a TrialReceipt,
        receipt_sha256: String,
    }
    let receipt_bytes = serde_json::to_vec(&(arm, receipt))?;
    let record = JournalRecord {
        schema_version: receipt_journal_schema(
            receipt,
            valid_time_registered,
            selector_expectation_registered,
            snapshot_retry_registered,
        ),
        plan_sha256,
        arm,
        receipt,
        receipt_sha256: sha256(&receipt_bytes),
    };
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|source| EvalError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    serde_json::to_writer(&mut file, &record)?;
    file.write_all(b"\n").map_err(|source| EvalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    file.sync_data().map_err(|source| EvalError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn receipt_journal_schema(
    receipt: &TrialReceipt,
    valid_time_registered: bool,
    selector_expectation_registered: bool,
    snapshot_retry_registered: bool,
) -> &'static str {
    let has_complete_elapsed_read_timing = receipt.read_capture_status.is_some()
        && receipt.read_diagnostics.as_ref().is_some_and(|reads| {
            reads.iter().all(|read| {
                (read.response_sequence.is_some() && read.elapsed_ms.is_some())
                    || (read.response_sequence.is_none() && read.elapsed_ms.is_none())
            })
        });
    if has_complete_elapsed_read_timing {
        return RECEIPT_JOURNAL_SCHEMA_V10;
    }
    let has_complete_request_fingerprints =
        receipt.read_diagnostics.as_ref().is_some_and(|reads| {
            reads.iter().all(|read| {
                read.request_query_sha256.is_some() && read.non_query_arguments_sha256.is_some()
            })
        });
    if snapshot_retry_registered {
        return RECEIPT_JOURNAL_SCHEMA_V9;
    }
    if selector_expectation_registered {
        return RECEIPT_JOURNAL_SCHEMA_V8;
    }
    match (
        receipt.read_capture_status.is_some(),
        has_complete_request_fingerprints,
        valid_time_registered,
    ) {
        (true, true, true) => RECEIPT_JOURNAL_SCHEMA_V7,
        (true, true, false) => RECEIPT_JOURNAL_SCHEMA_V6,
        (true, false, _) => RECEIPT_JOURNAL_SCHEMA_V5,
        (false, _, _) => RECEIPT_JOURNAL_SCHEMA_V4,
    }
}

fn write_json_new<T: Serialize>(path: &Path, value: &T) -> Result<(), EvalError> {
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| EvalError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(&bytes).map_err(|source| EvalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    file.write_all(b"\n").map_err(|source| EvalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    file.sync_all().map_err(|source| EvalError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paired::{
        AgentTraceEvent, BootstrapConfig, HardViolation, ModelIdentityProtocol,
        PAIRED_PLAN_SCHEMA_V2, ProductRunIdentity, PromotionCriteria, ResourceNonInferiority,
        SharedRunIdentity,
    };

    pub(super) struct FixedBackend {
        pub(super) calls: Vec<(RunArm, String)>,
        pub(super) answer: String,
    }

    struct CheckpointBackend {
        fixed: FixedBackend,
        prepared_count: usize,
        executed_prompts: Vec<(RunArm, String, Option<i64>)>,
    }

    impl PairedTrialBackend for CheckpointBackend {
        fn prepare(&mut self, task: &PairedTaskInput) -> Result<PreparedPairedTask, EvalError> {
            self.prepared_count += 1;
            Ok(PreparedPairedTask {
                input: PairedTaskInput {
                    prompt: resolve_known_revision_prompt(&task.prompt, 7)?,
                    ..task.clone()
                },
                expected_known_revision: Some(7),
            })
        }

        fn execute(
            &mut self,
            arm: RunArm,
            task: &PreparedPairedTask,
        ) -> Result<PairedTrialExecution, EvalError> {
            self.executed_prompts.push((
                arm,
                task.input.prompt.clone(),
                task.expected_known_revision,
            ));
            self.fixed.execute(arm, task)
        }
    }

    impl PairedTrialBackend for FixedBackend {
        fn execute(
            &mut self,
            arm: RunArm,
            prepared_task: &PreparedPairedTask,
        ) -> Result<PairedTrialExecution, EvalError> {
            let task = &prepared_task.input;
            self.calls.push((arm, task.key.case_id.clone()));
            let episode = task.seed_episodes.first().expect("test seed episode");
            let episode_id = format!("episode:{}", episode.event_id);
            let final_answer = serde_json::json!({
                "answer": self.answer,
                "citations": [episode_id],
                "needs_clarification": false
            })
            .to_string();
            Ok(PairedTrialExecution {
                observation: AgentObservation {
                    trace: vec![
                        trace_event(1, 1, TraceEventKind::AgentTurnStarted, None, None),
                        trace_event(
                            2,
                            1,
                            TraceEventKind::MemoryRead,
                            Some("call-1"),
                            Some("memory_decisions/assemble_context"),
                        ),
                        trace_event(
                            3,
                            1,
                            TraceEventKind::ToolResponse,
                            Some("call-1"),
                            Some("memory_decisions/assemble_context"),
                        ),
                        trace_event(4, 1, TraceEventKind::FinalAnswer, None, None),
                        AgentTraceEvent {
                            sequence: 5,
                            turn: 1,
                            kind: TraceEventKind::AgentTurn,
                            call_id: None,
                            tool_name: None,
                            error_class: None,
                            payload_sha256: None,
                            input_tokens: Some(12),
                            output_tokens: Some(6),
                        },
                    ],
                    final_answer: Some(final_answer),
                    memory_source_episode_ids: vec![episode_id.clone()],
                    memory_reads: None,
                    read_capture_status: None,
                    memory_write_episode_ids: vec![],
                    observed_fact_sources: Vec::new(),
                    invalidation_fact_ids: Vec::new(),
                    extract_fact_evidence: vec![],
                    extract_fact_evidence_telemetry_complete: false,
                    terminal_http_status_code: None,
                    terminal_transport_stage: None,
                    turns_used: 1,
                    tool_calls: 1,
                    user_clarifications: 0,
                    input_tokens: 12,
                    output_tokens: 6,
                    wall_time_ms: 100,
                    status: TrialStatus::Completed,
                },
                episode_id_to_event_id: BTreeMap::from([(episode_id, episode.event_id.clone())]),
            })
        }
    }

    struct ReadCaptureBackend {
        fixed: FixedBackend,
    }

    impl PairedTrialBackend for ReadCaptureBackend {
        fn execute(
            &mut self,
            arm: RunArm,
            task: &PreparedPairedTask,
        ) -> Result<PairedTrialExecution, EvalError> {
            let mut execution = self.fixed.execute(arm, task)?;
            execution.observation.memory_reads = Some(vec![MemoryReadObservation {
                request_sequence: 2,
                response_sequence: Some(3),
                elapsed_ms: Some(12),
                outcome: MemoryReadOutcome::Nonempty,
                item_count: Some(1),
                request_query_sha256: Some("b".repeat(64)),
                non_query_arguments_sha256: Some("c".repeat(64)),
                selector_arguments_sha256: None,
                selector_arguments_present: None,
                requested_valid_at_sha256: None,
                requested_cutoff_sha256: None,
                response_guidance_sha256: None,
                evidence: None,
            }]);
            execution.observation.read_capture_status = Some(ReadCaptureStatus::Observed);
            Ok(execution)
        }
    }

    fn trace_event(
        sequence: u32,
        turn: u32,
        kind: TraceEventKind,
        call_id: Option<&str>,
        tool_name: Option<&str>,
    ) -> AgentTraceEvent {
        AgentTraceEvent {
            sequence,
            turn,
            kind,
            call_id: call_id.map(str::to_owned),
            tool_name: tool_name.map(str::to_owned),
            error_class: None,
            payload_sha256: matches!(
                kind,
                TraceEventKind::MemoryRead
                    | TraceEventKind::ToolResponse
                    | TraceEventKind::FinalAnswer
            )
            .then(|| "a".repeat(64)),
            input_tokens: None,
            output_tokens: None,
        }
    }

    fn task(key: CaseTrialKey, event_id: &str) -> PairedTaskDefinition {
        PairedTaskDefinition {
            key,
            prompt: "Where is the North Depot?".into(),
            knowledge_before_event_id: None,
            seed_episodes: vec![SeedEpisode {
                event_id: event_id.into(),
                source_id: format!("source-{event_id}"),
                source_type: "owner-record".into(),
                content: "North Depot is located in North City".into(),
                valid_at: DateTime::parse_from_rfc3339("2026-05-01T10:00:00Z")
                    .expect("valid fixture date")
                    .with_timezone(&Utc),
            }],
            oracle: HiddenTaskOracle {
                sufficient_evidence_alternatives: vec![vec![vec![event_id.into()]]],
                accepted_answers: vec!["North City".into()],
                forbidden_event_ids: Vec::new(),
                needs_clarification: false,
                read_expectation: None,
            },
        }
    }

    pub(super) fn manifest() -> PairedTaskManifest {
        PairedTaskManifest {
            schema_version: PAIRED_TASK_MANIFEST_SCHEMA_V1.into(),
            tasks: vec![
                task(
                    CaseTrialKey {
                        stratum_id: "ordinary".into(),
                        family_id: "family-a".into(),
                        case_id: "case-a".into(),
                        repetition: 1,
                        incumbent_first: true,
                    },
                    "event-a",
                ),
                task(
                    CaseTrialKey {
                        stratum_id: "ordinary".into(),
                        family_id: "family-b".into(),
                        case_id: "case-b".into(),
                        repetition: 1,
                        incumbent_first: false,
                    },
                    "event-b",
                ),
            ],
        }
    }

    fn resource_margin() -> ResourceNonInferiority {
        ResourceNonInferiority {
            turns: 0.0,
            tool_calls: 0.0,
            user_clarifications: 0.0,
            input_tokens: 0.0,
            output_tokens: 0.0,
            wall_time_ms: 0.0,
        }
    }

    pub(super) fn plan(manifest: &PairedTaskManifest) -> PairedEvaluationPlan {
        let shared_identity = SharedRunIdentity {
            agent_runner: "fixed-backend".into(),
            agent_version: "0.169.0".into(),
            model: "fixed-model".into(),
            model_identity_protocol: ModelIdentityProtocol::ProviderRevision {
                revision: "fixed-model-revision".into(),
            },
            system_prompt_sha256: "a".repeat(64),
            task_prompt_sha256: "b".repeat(64),
            host_policy_sha256: "c".repeat(64),
            transport: "stdio".into(),
            retry_policy_sha256: "d".repeat(64),
            reader_config_sha256: "e".repeat(64),
            judge_config_sha256: None,
            corpus_sha256: manifest.corpus_sha256().expect("hash corpus"),
            oracle_sha256: manifest.oracle_sha256().expect("hash oracle"),
            clock_id: "fixed-clock".into(),
            simulator_sha256: "f".repeat(64),
            provider_fingerprints: BTreeMap::from([("agent".into(), "1".repeat(64))]),
            context_budget_tokens: 4096,
            tool_call_budget: 12,
            turn_budget: 20,
            output_token_budget: 1024,
            wall_time_budget_ms: 60_000,
            hardware_class: "local-arm64".into(),
        };
        let product = |revision: &str, binary: char, schema: char| ProductRunIdentity {
            revision: revision.into(),
            binary_sha256: binary.to_string().repeat(64),
            product_config_sha256: "2".repeat(64),
            tool_schema_sha256: schema.to_string().repeat(64),
            enabled_features: BTreeSet::new(),
            claim_rollout_stage: None,
        };
        PairedEvaluationPlan {
            schema_version: PAIRED_PLAN_SCHEMA_V2.into(),
            plan_id: "paired-test".into(),
            stage: crate::paired::PairedEvaluationStage::Confirmatory,
            pilot_report_sha256: Some("3".repeat(64)),
            power_analysis_sha256: Some("4".repeat(64)),
            expected_trials: manifest.tasks.iter().map(|task| task.key.clone()).collect(),
            planned_independent_families: 2,
            shared_identity,
            incumbent_product: product("incumbent", '5', '6'),
            candidate_product: product("candidate", '7', '8'),
            declared_tool_schema_change: Some(crate::paired::DeclaredToolSchemaChange {
                incumbent_sha256: "6".repeat(64),
                candidate_sha256: "8".repeat(64),
                diff_sha256: "9".repeat(64),
            }),
            bootstrap: BootstrapConfig {
                seed: 1,
                draws: 1_000,
                confidence_level_basis_points: 9_500,
            },
            promotion_criteria: Some(PromotionCriteria {
                direct_memory_margin: 0.05,
                task_success_margin: 0.05,
                repair_turn_gain_margin: 0.05,
                task_non_inferiority_by_stratum: BTreeMap::from([("ordinary".into(), 0.05)]),
                ordinary_workflow_strata: BTreeSet::from(["ordinary".into()]),
                resource_non_inferiority: resource_margin(),
                resource_non_inferiority_by_stratum: BTreeMap::from([(
                    "ordinary".into(),
                    resource_margin(),
                )]),
            }),
        }
    }

    #[test]
    fn paired_plan_executes_counterbalanced_arms_and_publishes_oracle_scored_receipts() {
        let directory = tempfile::tempdir().expect("paired run directory");
        let manifest = manifest();
        let plan = plan(&manifest);
        let mut backend = FixedBackend {
            calls: Vec::new(),
            answer: "North City".into(),
        };

        let (incumbent, candidate) =
            run_paired_agent_evaluation(&plan, &manifest, &mut backend, directory.path())
                .expect("paired artifacts validate and publish");

        assert_eq!(
            backend.calls,
            vec![
                (RunArm::Incumbent, "case-a".into()),
                (RunArm::Candidate, "case-a".into()),
                (RunArm::Candidate, "case-b".into()),
                (RunArm::Incumbent, "case-b".into()),
            ]
        );
        assert!(
            incumbent
                .receipts
                .iter()
                .all(|receipt| { receipt.direct_memory_success && receipt.task_oracle_passed })
        );
        assert!(
            candidate
                .receipts
                .iter()
                .all(|receipt| { receipt.direct_memory_success && receipt.task_oracle_passed })
        );
        assert!(candidate.receipts.iter().all(|receipt| {
            receipt
                .task_oracle_diagnostics
                .as_ref()
                .is_some_and(|diagnostics| {
                    diagnostics.accepted_answer_match == Some(AcceptedAnswerMatch::Exact)
                })
        }));
        assert!(directory.path().join("incumbent.json").is_file());
        assert!(directory.path().join("candidate.json").is_file());
        assert!(
            incumbent
                .run_id
                .as_ref()
                .is_some_and(|run_id| !run_id.is_empty())
        );
        assert_eq!(incumbent.run_id, candidate.run_id);
        assert_eq!(incumbent.schema_version, PAIRED_ARTIFACT_SCHEMA_V6);
        let journal = std::fs::read_to_string(directory.path().join("incumbent-receipts.jsonl"))
            .expect("durable receipt journal");
        assert_eq!(journal.lines().count(), 2);
        assert!(journal.lines().all(|line| {
            line.contains("\"schema_version\":\"memory-mcp-agent-trial-receipt/v4\"")
        }));
    }

    #[test]
    fn read_receipt_journal_stays_v5_without_request_fingerprints() {
        let directory = tempfile::tempdir().expect("journal directory");
        let path = directory.path().join("receipts.jsonl");
        std::fs::write(&path, "").expect("create journal");

        append_receipt_journal(
            &path,
            &"a".repeat(64),
            RunArm::Candidate,
            &journal_read_receipt(false),
            false,
            false,
            false,
        )
        .expect("write legacy read receipt");

        let line = std::fs::read_to_string(path).expect("read journal");
        assert!(line.contains("\"schema_version\":\"memory-mcp-agent-trial-receipt/v5\""));
        assert!(!line.contains("request_query_sha256"));
    }

    #[test]
    fn read_receipt_journal_uses_v6_for_request_fingerprints_without_raw_text() {
        let directory = tempfile::tempdir().expect("journal directory");
        let path = directory.path().join("receipts.jsonl");
        std::fs::write(&path, "").expect("create journal");

        append_receipt_journal(
            &path,
            &"a".repeat(64),
            RunArm::Candidate,
            &journal_read_receipt(true),
            false,
            false,
            false,
        )
        .expect("write v6 read receipt");

        let line = std::fs::read_to_string(path).expect("read journal");
        let record: serde_json::Value = serde_json::from_str(&line).expect("journal JSON");
        assert_eq!(
            record["schema_version"],
            "memory-mcp-agent-trial-receipt/v6"
        );
        assert_eq!(
            record["receipt"]["read_diagnostics"][0]["request_query_sha256"],
            "b".repeat(64)
        );
        assert!(!line.contains("PRIVATE QUERY"));
        assert!(!line.contains("PRIVATE GUIDANCE"));
    }

    #[test]
    fn registered_valid_time_publishes_timed_journal_with_unknown_match() {
        let directory = tempfile::tempdir().expect("paired output directory");
        let mut manifest = manifest();
        manifest.schema_version = PAIRED_TASK_MANIFEST_SCHEMA_V2.into();
        for task in &mut manifest.tasks {
            task.oracle.read_expectation = Some(ReadExpectation {
                valid_at: Some(
                    DateTime::parse_from_rfc3339("2026-05-01T10:00:00Z")
                        .expect("valid fixture date")
                        .with_timezone(&Utc),
                ),
                required_claim_annotation_alternatives: Vec::new(),
                selector_arguments: None,
                snapshot_bound_retry: None,
            });
        }
        let plan = plan(&manifest);
        let mut backend = ReadCaptureBackend {
            fixed: FixedBackend {
                calls: Vec::new(),
                answer: "North City".into(),
            },
        };

        run_paired_agent_evaluation(&plan, &manifest, &mut backend, directory.path())
            .expect("registered read publishes paired run artifacts");

        let journal = std::fs::read_to_string(directory.path().join("candidate-receipts.jsonl"))
            .expect("candidate receipt journal");
        assert_eq!(journal.lines().count(), 2);
        for line in journal.lines() {
            let record: serde_json::Value = serde_json::from_str(line).expect("journal JSON");
            assert_eq!(record["schema_version"], RECEIPT_JOURNAL_SCHEMA_V10);
            let read = &record["receipt"]["read_diagnostics"][0];
            assert!(read["elapsed_ms"].as_u64().is_some());
            assert!(read.get("requested_valid_at_match").is_none());
            assert!(read.get("returned_valid_at_match").is_none());
        }
    }

    fn journal_read_receipt(with_fingerprints: bool) -> TrialReceipt {
        TrialReceipt {
            read_capture_status: Some(ReadCaptureStatus::Observed),
            read_diagnostics: Some(vec![MemoryReadDiagnostic {
                request_sequence: 1,
                response_sequence: Some(2),
                elapsed_ms: None,
                outcome: MemoryReadOutcome::Empty,
                item_count: Some(0),
                request_query_sha256: with_fingerprints.then(|| "b".repeat(64)),
                non_query_arguments_sha256: with_fingerprints.then(|| "c".repeat(64)),
                selector_expectation_registered: None,
                selector_argument_matches: None,
                response_guidance_sha256: with_fingerprints.then(|| "d".repeat(64)),
                requested_valid_at_match: None,
                returned_valid_at_match: None,
                requested_cutoff_match: None,
                returned_cutoff_match: None,
                source_coverage: None,
                annotation_match: None,
                completeness: None,
                cited_returned_source_count: None,
            }]),
            snapshot_retry_diagnostic: None,
            key: CaseTrialKey {
                stratum_id: "test".into(),
                family_id: "family".into(),
                case_id: "case".into(),
                repetition: 1,
                incumbent_first: true,
            },
            arm_order: 1,
            known_revision_checkpoint: None,
            status: TrialStatus::Completed,
            direct_memory_success: false,
            task_oracle_passed: false,
            task_oracle_diagnostics: None,
            turns_used: 1,
            tool_calls: 1,
            repair_turns: 0,
            user_clarifications: 0,
            input_tokens: 1,
            output_tokens: 1,
            wall_time_ms: 1,
            hard_violations: Vec::new(),
            trace: Vec::new(),
        }
    }

    #[test]
    fn unrelated_forbidden_source_fails_without_claiming_temporal_leakage() {
        let directory = tempfile::tempdir().expect("paired output");
        let mut manifest = manifest();
        manifest.tasks[0]
            .oracle
            .sufficient_evidence_alternatives
            .clear();
        manifest.tasks[0].oracle.forbidden_event_ids = vec!["event-a".into()];
        let plan = plan(&manifest);
        let mut backend = FixedBackend {
            calls: Vec::new(),
            answer: "North City".into(),
        };

        let (_, candidate) =
            run_paired_agent_evaluation(&plan, &manifest, &mut backend, directory.path())
                .expect("valid paired run");
        let receipt = &candidate.receipts[0];
        assert!(!receipt.direct_memory_success);
        assert!(!receipt.task_oracle_passed);
        assert_eq!(
            receipt.hard_violations,
            vec![HardViolation::ProhibitedEvidence]
        );
        assert!(
            !receipt
                .hard_violations
                .contains(&HardViolation::TemporalMismatch)
        );
    }

    #[test]
    fn separate_paired_invocations_receive_distinct_runner_ids() {
        let first_directory = tempfile::tempdir().expect("first pilot output directory");
        let second_directory = tempfile::tempdir().expect("second pilot output directory");
        let manifest = manifest();
        let plan = plan(&manifest);
        let mut first_backend = FixedBackend {
            calls: Vec::new(),
            answer: "North City".into(),
        };
        let mut second_backend = FixedBackend {
            calls: Vec::new(),
            answer: "North City".into(),
        };

        let first = run_paired_agent_evaluation(
            &plan,
            &manifest,
            &mut first_backend,
            first_directory.path(),
        )
        .expect("first repeat run");
        let second = run_paired_agent_evaluation(
            &plan,
            &manifest,
            &mut second_backend,
            second_directory.path(),
        )
        .expect("second repeat run");

        assert_ne!(first.0.run_id, second.0.run_id);
        assert_eq!(first.0.run_id, first.1.run_id);
        assert_eq!(second.0.run_id, second.1.run_id);
    }

    #[test]
    fn exact_revision_prompt_is_prepared_once_and_shared_by_both_arms() {
        let directory = tempfile::tempdir().expect("paired run directory");
        let mut manifest = manifest();
        manifest.tasks[0].knowledge_before_event_id = Some("event-a".into());
        manifest.tasks[0].prompt = format!(
            "case-a uses memory snapshot {} to answer the question.",
            PAIRED_KNOWN_REVISION_PLACEHOLDER
        );
        manifest.tasks[1].knowledge_before_event_id = Some("event-b".into());
        manifest.tasks[1].prompt = format!(
            "case-b uses memory snapshot {} to answer the question.",
            PAIRED_KNOWN_REVISION_PLACEHOLDER
        );
        let plan = plan(&manifest);
        let expected_prompts = [
            "case-a uses memory snapshot 7 to answer the question.",
            "case-b uses memory snapshot 7 to answer the question.",
        ];
        let mut backend = CheckpointBackend {
            fixed: FixedBackend {
                calls: Vec::new(),
                answer: "North City".into(),
            },
            prepared_count: 0,
            executed_prompts: Vec::new(),
        };

        let (incumbent, candidate) =
            run_paired_agent_evaluation(&plan, &manifest, &mut backend, directory.path())
                .expect("both arms receive one shared resolved task");

        assert_eq!(backend.prepared_count, 2);
        assert_eq!(backend.executed_prompts.len(), 4);
        for expected_prompt in expected_prompts {
            let paired = backend
                .executed_prompts
                .iter()
                .filter(|(_, prompt, _)| prompt == expected_prompt)
                .collect::<Vec<_>>();
            assert_eq!(paired.len(), 2);
            assert!(paired.iter().all(|(_, _, revision)| *revision == Some(7)));
            assert_ne!(paired[0].0, paired[1].0);
        }
        assert!(
            incumbent
                .receipts
                .iter()
                .chain(&candidate.receipts)
                .all(|receipt| receipt.known_revision_checkpoint == Some(7))
        );
    }

    #[test]
    fn revision_prompt_replacement_requires_exactly_one_marker() {
        assert_eq!(
            resolve_known_revision_prompt("revision={{KNOWN_REVISION}}", 12).unwrap(),
            "revision=12"
        );
        assert!(resolve_known_revision_prompt("no marker", 12).is_err());
        assert!(
            resolve_known_revision_prompt("{{KNOWN_REVISION}} {{KNOWN_REVISION}}", 12).is_err()
        );
    }

    #[test]
    fn paired_preparation_cannot_change_the_preregistered_history() {
        let mut definition = task(
            CaseTrialKey {
                stratum_id: "ordinary".into(),
                family_id: "family-a".into(),
                case_id: "case-a".into(),
                repetition: 1,
                incumbent_first: true,
            },
            "event-a",
        );
        definition.knowledge_before_event_id = Some("event-a".into());
        definition.prompt = "snapshot={{KNOWN_REVISION}}".into();
        let input = PairedTaskInput {
            key: definition.key,
            prompt: definition.prompt,
            knowledge_before_event_id: definition.knowledge_before_event_id,
            seed_episodes: definition.seed_episodes,
        };
        let prepared = PreparedPairedTask {
            input: PairedTaskInput {
                prompt: "snapshot=9".into(),
                ..input.clone()
            },
            expected_known_revision: Some(9),
        };
        assert!(validate_prepared_task(&input, &prepared).is_ok());

        let changed_history = PreparedPairedTask {
            input: PairedTaskInput {
                seed_episodes: Vec::new(),
                ..prepared.input.clone()
            },
            ..prepared.clone()
        };
        assert!(validate_prepared_task(&input, &changed_history).is_err());

        let changed_prompt = PreparedPairedTask {
            input: PairedTaskInput {
                prompt: "snapshot=9 plus unrelated prompt edit".into(),
                ..prepared.input.clone()
            },
            ..prepared
        };
        assert!(validate_prepared_task(&input, &changed_prompt).is_err());
    }

    #[test]
    fn direct_memory_success_is_separate_from_a_wrong_agent_answer() {
        let manifest = manifest();
        let plan = plan(&manifest);
        let mut backend = FixedBackend {
            calls: Vec::new(),
            answer: "South City".into(),
        };
        let directory = tempfile::tempdir().expect("paired run directory");

        let (incumbent, _) =
            run_paired_agent_evaluation(&plan, &manifest, &mut backend, directory.path())
                .expect("wrong task answers remain valid failed receipts");

        assert!(
            incumbent
                .receipts
                .iter()
                .all(|receipt| receipt.direct_memory_success)
        );
        let receipt = &incumbent.receipts[0];
        let serialized = serde_json::to_value(receipt).expect("serialize scored receipt");
        assert_eq!(
            serialized.get("task_oracle_diagnostics"),
            Some(&serde_json::json!({
                "final_answer_present": true,
                "final_answer_schema_valid": true,
                "accepted_answer": false,
                "accepted_answer_match": "no_accepted_value_mentioned",
                "required_evidence_cited": true,
                "clarification_matches": true
            }))
        );
        assert!(
            incumbent
                .receipts
                .iter()
                .all(|receipt| !receipt.task_oracle_passed)
        );
        assert!(
            incumbent
                .receipts
                .iter()
                .all(|receipt| receipt.hard_violations.is_empty())
        );
    }

    #[test]
    fn accepted_value_in_an_explanatory_answer_is_diagnosed_without_passing_the_oracle() {
        let directory = tempfile::tempdir().expect("paired run directory");
        let manifest = manifest();
        let plan = plan(&manifest);
        let mut backend = FixedBackend {
            calls: Vec::new(),
            answer: "The North Depot is in North City.".into(),
        };

        let (_, candidate) =
            run_paired_agent_evaluation(&plan, &manifest, &mut backend, directory.path())
                .expect("explanatory answer remains a valid failed receipt");

        let receipt = &candidate.receipts[0];
        assert!(!receipt.task_oracle_passed);
        assert_eq!(
            serde_json::to_value(
                receipt
                    .task_oracle_diagnostics
                    .as_ref()
                    .expect("diagnostics")
            )
            .expect("serialize content-free diagnostics"),
            serde_json::json!({
                "final_answer_present": true,
                "final_answer_schema_valid": true,
                "accepted_answer": false,
                "accepted_answer_match": "mentioned_with_extra_text",
                "required_evidence_cited": true,
                "clarification_matches": true
            })
        );
    }

    #[test]
    fn accepted_value_substring_inside_a_different_word_is_not_a_mention() {
        let directory = tempfile::tempdir().expect("paired run directory");
        let manifest = manifest();
        let plan = plan(&manifest);
        let mut backend = FixedBackend {
            calls: Vec::new(),
            answer: "North Citywide".into(),
        };

        let (_, candidate) =
            run_paired_agent_evaluation(&plan, &manifest, &mut backend, directory.path())
                .expect("wrong answer remains a valid failed receipt");

        let diagnostics = candidate.receipts[0]
            .task_oracle_diagnostics
            .as_ref()
            .expect("content-free diagnostics");
        assert_eq!(diagnostics.accepted_answer, Some(false));
        assert_eq!(
            diagnostics.accepted_answer_match,
            Some(AcceptedAnswerMatch::NoAcceptedValueMentioned)
        );
    }

    #[test]
    fn run_refuses_existing_output_before_invoking_the_agent() {
        let directory = tempfile::tempdir().expect("paired run directory");
        std::fs::write(directory.path().join("incumbent.json"), b"preserve")
            .expect("pre-existing artifact");
        let manifest = manifest();
        let plan = plan(&manifest);
        let mut backend = FixedBackend {
            calls: Vec::new(),
            answer: "North City".into(),
        };

        let result = run_paired_agent_evaluation(&plan, &manifest, &mut backend, directory.path());

        assert!(result.is_err());
        assert!(backend.calls.is_empty());
        assert_eq!(
            std::fs::read(directory.path().join("incumbent.json")).unwrap(),
            b"preserve"
        );
    }

    #[test]
    fn manifest_hashes_bind_public_corpus_and_private_oracle_separately() {
        let mut original = manifest();
        let original_corpus = original.corpus_sha256().unwrap();
        let original_oracle = original.oracle_sha256().unwrap();
        original.tasks[0].oracle.accepted_answers = vec!["Different answer".into()];

        assert_eq!(original.corpus_sha256().unwrap(), original_corpus);
        assert_ne!(original.oracle_sha256().unwrap(), original_oracle);
        let public_task = serde_json::to_value(PairedTaskInput {
            key: original.tasks[0].key.clone(),
            prompt: original.tasks[0].prompt.clone(),
            knowledge_before_event_id: None,
            seed_episodes: original.tasks[0].seed_episodes.clone(),
        })
        .unwrap();
        assert!(public_task.get("knowledge_before_event_id").is_none());
    }

    #[test]
    fn manifest_rejects_oracle_without_an_accepted_answer() {
        let mut manifest = manifest();
        manifest.tasks[0].oracle.accepted_answers.clear();
        let plan = plan(&manifest);

        let result = manifest.validate(&plan);

        assert!(
            matches!(result, Err(EvalError::InvalidInput(message)) if message.contains("accepted answer"))
        );
    }

    #[test]
    fn manifest_rejects_snapshot_retry_expectation_before_schema_v3() {
        let mut manifest = manifest();
        manifest.schema_version = PAIRED_TASK_MANIFEST_SCHEMA_V2.into();
        for task in &mut manifest.tasks {
            task.oracle.read_expectation = Some(ReadExpectation {
                valid_at: None,
                required_claim_annotation_alternatives: Vec::new(),
                selector_arguments: None,
                snapshot_bound_retry: Some(SnapshotBoundRetryExpectation {
                    authorized_guidance_sha256: "a".repeat(64),
                    retry_required_when_eligible: true,
                }),
            });
        }
        let plan = plan(&manifest);

        let result = manifest.validate(&plan);

        assert!(matches!(
            result,
            Err(EvalError::InvalidInput(message))
                if message.contains("snapshot-bound retry requires paired task manifest v3")
        ));
    }

    #[test]
    fn manifest_requires_one_placeholder_and_a_seeded_knowledge_boundary() {
        let mut manifest = manifest();
        manifest.tasks[0].knowledge_before_event_id = Some("event-a".into());
        manifest.tasks[0].prompt = format!(
            "Use the exact memory revision {}",
            PAIRED_KNOWN_REVISION_PLACEHOLDER
        );
        let paired_plan = plan(&manifest);

        assert!(manifest.validate(&paired_plan).is_ok());

        manifest.tasks[0].prompt = "Where was the depot?".into();
        let paired_plan = plan(&manifest);
        let result = manifest.validate(&paired_plan);
        assert!(
            matches!(result, Err(EvalError::InvalidInput(message)) if message.contains("placeholder"))
        );

        manifest.tasks[0].prompt = format!(
            "Use the exact memory revision {} twice: {}",
            PAIRED_KNOWN_REVISION_PLACEHOLDER, PAIRED_KNOWN_REVISION_PLACEHOLDER
        );
        let paired_plan = plan(&manifest);
        let result = manifest.validate(&paired_plan);
        assert!(
            matches!(result, Err(EvalError::InvalidInput(message)) if message.contains("placeholder"))
        );

        manifest.tasks[0].prompt = format!(
            "Use the exact memory revision {}",
            PAIRED_KNOWN_REVISION_PLACEHOLDER
        );
        manifest.tasks[0].knowledge_before_event_id = Some("missing-event".into());
        let paired_plan = plan(&manifest);
        let result = manifest.validate(&paired_plan);
        assert!(
            matches!(result, Err(EvalError::InvalidInput(message)) if message.contains("boundary event"))
        );

        manifest.tasks[0].knowledge_before_event_id = None;
        let paired_plan = plan(&manifest);
        let result = manifest.validate(&paired_plan);
        assert!(
            matches!(result, Err(EvalError::InvalidInput(message)) if message.contains("without a boundary event"))
        );
    }

    #[test]
    fn corpus_hash_binds_the_temporal_boundary_event() {
        let mut manifest = manifest();
        manifest.tasks[0].knowledge_before_event_id = Some("event-a".into());
        manifest.tasks[0].prompt = format!(
            "Use the exact memory revision {}",
            PAIRED_KNOWN_REVISION_PLACEHOLDER
        );
        let original_hash = manifest.corpus_sha256().unwrap();
        manifest.tasks[0].knowledge_before_event_id = Some("other-event".into());

        assert_ne!(manifest.corpus_sha256().unwrap(), original_hash);
    }

    #[test]
    fn unsupported_citations_fail_task_score_and_record_a_hard_violation() {
        let mut task = task(
            CaseTrialKey {
                stratum_id: "ordinary".into(),
                family_id: "family-a".into(),
                case_id: "case-a".into(),
                repetition: 1,
                incumbent_first: true,
            },
            "event-a",
        );
        let episode_id = "episode:event-a".to_string();
        let observation = AgentObservation {
            trace: vec![
                trace_event(1, 1, TraceEventKind::AgentTurnStarted, None, None),
                trace_event(2, 1, TraceEventKind::FinalAnswer, None, None),
                AgentTraceEvent {
                    sequence: 3,
                    turn: 1,
                    kind: TraceEventKind::AgentTurn,
                    call_id: None,
                    tool_name: None,
                    error_class: None,
                    payload_sha256: None,
                    input_tokens: Some(1),
                    output_tokens: Some(1),
                },
            ],
            final_answer: Some(
                serde_json::json!({
                    "answer":"North City",
                    "citations":["invented-episode"],
                    "needs_clarification":false
                })
                .to_string(),
            ),
            memory_source_episode_ids: vec![episode_id.clone()],
            memory_reads: None,
            read_capture_status: None,
            memory_write_episode_ids: vec![],
            observed_fact_sources: Vec::new(),
            invalidation_fact_ids: Vec::new(),
            extract_fact_evidence: vec![],
            extract_fact_evidence_telemetry_complete: false,
            terminal_http_status_code: None,
            terminal_transport_stage: None,
            turns_used: 1,
            tool_calls: 0,
            user_clarifications: 0,
            input_tokens: 1,
            output_tokens: 1,
            wall_time_ms: 1,
            status: TrialStatus::Completed,
        };
        let execution = PairedTrialExecution {
            observation,
            episode_id_to_event_id: BTreeMap::from([(episode_id, "event-a".into())]),
        };
        task.oracle.accepted_answers = vec!["North City".into()];

        let receipt = score_trial(&task, 1, None, &execution).unwrap();

        assert!(!receipt.task_oracle_passed);
        assert!(
            receipt
                .hard_violations
                .contains(&HardViolation::UnsupportedEvidence)
        );
    }
}
