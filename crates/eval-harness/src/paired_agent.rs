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
pub(crate) mod tool_results;

pub const PAIRED_TASK_MANIFEST_SCHEMA_V1: &str = "memory-mcp-agent-tasks/v1";
pub const PAIRED_KNOWN_REVISION_PLACEHOLDER: &str = "{{KNOWN_REVISION}}";
const RECEIPT_JOURNAL_SCHEMA_V4: &str = "memory-mcp-agent-trial-receipt/v4";

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
    /// Exact final output text for private scoring.
    pub final_answer: Option<String>,
    /// Source episode IDs observed in successful memory reads.
    pub memory_source_episode_ids: Vec<String>,
    /// Episode IDs returned by successful `ingest` calls. These opaque IDs
    /// support independent post-write probes without retaining memory content.
    #[serde(default)]
    pub memory_write_episode_ids: Vec<String>,
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
        if self.schema_version != PAIRED_TASK_MANIFEST_SCHEMA_V1 {
            return Err(EvalError::InvalidInput(format!(
                "unsupported paired task manifest schema {:?}",
                self.schema_version
            )));
        }
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
            let receipt = score_trial(
                task,
                arm_order_for(arm, key.incumbent_first),
                prepared_task.expected_known_revision,
                &execution,
            )?;
            crate::paired::validate_trial_receipt(&receipt, &identity.shared)?;
            let journal_path = match arm {
                RunArm::Incumbent => &incumbent_journal,
                RunArm::Candidate => &candidate_journal,
            };
            append_receipt_journal(journal_path, &plan_sha256, arm, &receipt)?;
            match arm {
                RunArm::Incumbent => incumbent.receipts.push(receipt),
                RunArm::Candidate => candidate.receipts.push(receipt),
            }
        }
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

fn arm_order_for(arm: RunArm, incumbent_first: bool) -> u8 {
    if (arm == RunArm::Incumbent) == incumbent_first {
        1
    } else {
        2
    }
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
    let evidence_passed = if task.oracle.sufficient_evidence_alternatives.is_empty() {
        retrieved.is_empty()
    } else {
        task.oracle
            .sufficient_evidence_alternatives
            .iter()
            .any(|alternative| {
                !alternative.is_empty()
                    && alternative.iter().all(|group| {
                        group
                            .iter()
                            .any(|event_id| retrieved.contains(event_id.as_str()))
                    })
            })
    };
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
    let cited_evidence_passed = if task.oracle.sufficient_evidence_alternatives.is_empty() {
        cited_set.is_empty()
    } else {
        task.oracle
            .sufficient_evidence_alternatives
            .iter()
            .any(|alternative| {
                !alternative.is_empty()
                    && alternative.iter().all(|group| {
                        group
                            .iter()
                            .any(|event_id| cited_set.contains(event_id.as_str()))
                    })
            })
    };
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
    Ok(TrialReceipt {
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
        trace: observation.trace.clone(),
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
        schema_version: RECEIPT_JOURNAL_SCHEMA_V4,
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
                    memory_write_episode_ids: vec![],
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
            memory_write_episode_ids: vec![],
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
