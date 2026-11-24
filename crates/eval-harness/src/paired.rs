use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::EvalError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedRunIdentity {
    pub agent_runner: String,
    pub agent_version: String,
    pub model: String,
    pub model_identity_protocol: ModelIdentityProtocol,
    pub system_prompt_sha256: String,
    pub task_prompt_sha256: String,
    pub host_policy_sha256: String,
    pub transport: String,
    pub retry_policy_sha256: String,
    pub reader_config_sha256: String,
    pub judge_config_sha256: Option<String>,
    pub corpus_sha256: String,
    pub oracle_sha256: String,
    pub clock_id: String,
    pub simulator_sha256: String,
    pub provider_fingerprints: BTreeMap<String, String>,
    pub context_budget_tokens: u64,
    pub tool_call_budget: u32,
    pub turn_budget: u32,
    pub output_token_budget: u64,
    pub wall_time_budget_ms: u64,
    pub hardware_class: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelIdentityProtocol {
    ProviderRevision { revision: String },
    SameAliasWithinInvocation { max_duration_seconds: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductRunIdentity {
    pub revision: String,
    pub binary_sha256: String,
    pub product_config_sha256: String,
    pub tool_schema_sha256: String,
    pub enabled_features: BTreeSet<String>,
    /// Effective per-arm claim stage when the runner supports stage variation.
    /// Omitted by legacy plans that resolve one shared stage at execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_rollout_stage: Option<String>,
}

pub(crate) fn is_supported_claim_rollout_stage(stage: &str) -> bool {
    matches!(stage, "disabled" | "shadow" | "relations" | "evidence")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedRunIdentity {
    pub shared: SharedRunIdentity,
    pub product: ProductRunIdentity,
}

pub fn validate_pair(
    incumbent: &PairedRunIdentity,
    candidate: &PairedRunIdentity,
    schema_change: Option<&DeclaredToolSchemaChange>,
) -> Result<(), EvalError> {
    validate_identity("incumbent", incumbent)?;
    validate_identity("candidate", candidate)?;

    if incumbent.shared != candidate.shared {
        return Err(EvalError::InvalidInput(
            "paired runs must use identical agent, prompt, policy, corpus, oracle, time, simulator, budget, and hardware identities".into(),
        ));
    }

    match (
        incumbent.product.claim_rollout_stage.as_deref(),
        candidate.product.claim_rollout_stage.as_deref(),
    ) {
        (None, None) => {}
        (Some(_), Some(_)) => {}
        _ => {
            return Err(EvalError::InvalidInput(
                "paired product identities must either both pin claim rollout stages or both omit them".into(),
            ));
        }
    }

    if incumbent.product.tool_schema_sha256 == candidate.product.tool_schema_sha256 {
        if schema_change.is_some() {
            return Err(EvalError::InvalidInput(
                "tool schema change was declared but the arm schemas are identical".into(),
            ));
        }
        return Ok(());
    }

    let Some(schema_change) = schema_change else {
        return Err(EvalError::InvalidInput(
            "paired runs have different tool schemas without a declared interface migration".into(),
        ));
    };
    validate_sha256("tool schema diff", &schema_change.diff_sha256)?;
    if schema_change.incumbent_sha256 != incumbent.product.tool_schema_sha256
        || schema_change.candidate_sha256 != candidate.product.tool_schema_sha256
    {
        return Err(EvalError::InvalidInput(
            "declared tool schema migration does not match the paired arm fingerprints".into(),
        ));
    }

    Ok(())
}

fn validate_identity(label: &str, identity: &PairedRunIdentity) -> Result<(), EvalError> {
    let shared = &identity.shared;
    for (field, value) in [
        ("agent_runner", shared.agent_runner.as_str()),
        ("agent_version", shared.agent_version.as_str()),
        ("model", shared.model.as_str()),
        ("transport", shared.transport.as_str()),
        ("clock_id", shared.clock_id.as_str()),
        ("hardware_class", shared.hardware_class.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(EvalError::InvalidInput(format!(
                "{label} shared identity field {field} must not be empty"
            )));
        }
    }
    validate_model_identity_protocol(&shared.model_identity_protocol)?;
    if identity
        .product
        .claim_rollout_stage
        .as_deref()
        .is_some_and(|stage| !is_supported_claim_rollout_stage(stage))
    {
        return Err(EvalError::InvalidInput(format!(
            "{label} product claim rollout stage is unsupported"
        )));
    }
    for (field, digest) in [
        ("system_prompt_sha256", shared.system_prompt_sha256.as_str()),
        ("task_prompt_sha256", shared.task_prompt_sha256.as_str()),
        ("host_policy_sha256", shared.host_policy_sha256.as_str()),
        ("retry_policy_sha256", shared.retry_policy_sha256.as_str()),
        ("reader_config_sha256", shared.reader_config_sha256.as_str()),
        ("corpus_sha256", shared.corpus_sha256.as_str()),
        ("oracle_sha256", shared.oracle_sha256.as_str()),
        ("simulator_sha256", shared.simulator_sha256.as_str()),
    ] {
        validate_sha256(&format!("{label} {field}"), digest)?;
    }
    if let Some(digest) = shared.judge_config_sha256.as_deref() {
        validate_sha256(&format!("{label} judge config"), digest)?;
    }
    if shared.provider_fingerprints.is_empty() {
        return Err(EvalError::InvalidInput(format!(
            "{label} must pin at least one provider fingerprint"
        )));
    }
    for (provider, digest) in &shared.provider_fingerprints {
        if provider.trim().is_empty() {
            return Err(EvalError::InvalidInput(format!(
                "{label} provider fingerprint name must not be empty"
            )));
        }
        validate_sha256(&format!("{label} provider {provider}"), digest)?;
    }
    if shared.context_budget_tokens == 0
        || shared.tool_call_budget == 0
        || shared.turn_budget == 0
        || shared.output_token_budget == 0
        || shared.wall_time_budget_ms == 0
    {
        return Err(EvalError::InvalidInput(format!(
            "{label} agent budgets must be positive"
        )));
    }

    if identity.product.revision.trim().is_empty() {
        return Err(EvalError::InvalidInput(format!(
            "{label} product revision must not be empty"
        )));
    }
    validate_sha256(
        &format!("{label} product binary"),
        &identity.product.binary_sha256,
    )?;
    validate_sha256(
        &format!("{label} product config"),
        &identity.product.product_config_sha256,
    )?;
    validate_sha256(
        &format!("{label} tool schema"),
        &identity.product.tool_schema_sha256,
    )?;
    if identity
        .product
        .enabled_features
        .iter()
        .any(|feature| feature.trim().is_empty())
    {
        return Err(EvalError::InvalidInput(format!(
            "{label} product feature names must not be empty"
        )));
    }
    Ok(())
}

fn validate_model_identity_protocol(protocol: &ModelIdentityProtocol) -> Result<(), EvalError> {
    match protocol {
        ModelIdentityProtocol::ProviderRevision { revision } if !revision.trim().is_empty() => {
            Ok(())
        }
        ModelIdentityProtocol::ProviderRevision { .. } => Err(EvalError::InvalidInput(
            "provider-reported model revision must not be empty".into(),
        )),
        ModelIdentityProtocol::SameAliasWithinInvocation {
            max_duration_seconds,
        } if (1..=86_400).contains(max_duration_seconds) => Ok(()),
        ModelIdentityProtocol::SameAliasWithinInvocation { .. } => Err(EvalError::InvalidInput(
            "same-alias model identity requires an invocation window from 1 second to 24 hours"
                .into(),
        )),
    }
}

fn validate_run_window(
    protocol: &ModelIdentityProtocol,
    run_id: Option<&str>,
    started_at: DateTime<Utc>,
    completed_at: DateTime<Utc>,
) -> Result<(), EvalError> {
    if completed_at < started_at {
        return Err(EvalError::InvalidInput(
            "paired run completion time precedes its start time".into(),
        ));
    }
    if let ModelIdentityProtocol::SameAliasWithinInvocation {
        max_duration_seconds,
    } = protocol
    {
        if run_id.is_none() {
            return Err(EvalError::InvalidInput(
                "same-alias model identity requires a runner-generated invocation id".into(),
            ));
        }
        let elapsed = completed_at.signed_duration_since(started_at);
        if elapsed.num_seconds() > i64::from(*max_duration_seconds) {
            return Err(EvalError::InvalidInput(format!(
                "paired run exceeded its preregistered same-alias window of {max_duration_seconds} seconds"
            )));
        }
    }
    Ok(())
}

fn validate_sha256(label: &str, digest: &str) -> Result<(), EvalError> {
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(EvalError::InvalidInput(format!(
            "{label} must be a 64-character SHA-256 digest"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredToolSchemaChange {
    pub incumbent_sha256: String,
    pub candidate_sha256: String,
    pub diff_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseTrialKey {
    pub stratum_id: String,
    pub family_id: String,
    pub case_id: String,
    pub repetition: u32,
    /// Preregistered order: when true, the incumbent runs first for this pair.
    pub incumbent_first: bool,
}

pub fn validate_case_coverage(
    expected: &[CaseTrialKey],
    incumbent: &[CaseTrialKey],
    candidate: &[CaseTrialKey],
) -> Result<(), EvalError> {
    let expected = case_key_set("expected manifest", expected)?;
    if expected.is_empty() {
        return Err(EvalError::InvalidInput(
            "paired evaluation manifest must contain at least one case trial".into(),
        ));
    }
    for (label, keys) in [("incumbent", incumbent), ("candidate", candidate)] {
        let actual = case_key_set(label, keys)?;
        if actual != expected {
            let missing = expected.difference(&actual).next();
            let unexpected = actual.difference(&expected).next();
            return Err(EvalError::InvalidInput(format!(
                "{label} trial coverage differs from the preregistered manifest (missing: {}; unexpected: {})",
                missing
                    .map(format_case_trial_key)
                    .unwrap_or_else(|| "none".into()),
                unexpected
                    .map(format_case_trial_key)
                    .unwrap_or_else(|| "none".into())
            )));
        }
    }
    Ok(())
}

fn case_key_set(
    label: &str,
    keys: &[CaseTrialKey],
) -> Result<std::collections::BTreeSet<CaseTrialKey>, EvalError> {
    let mut result = std::collections::BTreeSet::new();
    let mut trial_ids = BTreeSet::new();
    for key in keys {
        if key.stratum_id.trim().is_empty()
            || key.family_id.trim().is_empty()
            || key.case_id.trim().is_empty()
            || key.repetition == 0
        {
            return Err(EvalError::InvalidInput(format!(
                "{label} trial key requires a family, case, and positive repetition"
            )));
        }
        if !result.insert(key.clone()) {
            return Err(EvalError::InvalidInput(format!(
                "{label} contains duplicate trial key {}",
                format_case_trial_key(key)
            )));
        }
        if !trial_ids.insert((
            key.stratum_id.as_str(),
            key.family_id.as_str(),
            key.case_id.as_str(),
            key.repetition,
        )) {
            return Err(EvalError::InvalidInput(format!(
                "{label} repeats a case trial with a different arm-order assignment: {}",
                format_case_trial_key(key)
            )));
        }
    }
    Ok(result)
}

fn expected_manifest_sha256(keys: &[CaseTrialKey]) -> Result<String, EvalError> {
    let mut canonical = keys.to_vec();
    canonical.sort();
    let bytes = serde_json::to_vec(&canonical)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn format_case_trial_key(key: &CaseTrialKey) -> String {
    format!(
        "{}::{}::{}#{}",
        key.stratum_id, key.family_id, key.case_id, key.repetition
    )
}

pub const PAIRED_ARTIFACT_SCHEMA_V2: &str = "memory-mcp-agent-paired/v2";
pub const PAIRED_ARTIFACT_SCHEMA_V3: &str = "memory-mcp-agent-paired/v3";
pub const PAIRED_ARTIFACT_SCHEMA_V4: &str = "memory-mcp-agent-paired/v4";
pub const PAIRED_ARTIFACT_SCHEMA_V5: &str = "memory-mcp-agent-paired/v5";
pub const PAIRED_ARTIFACT_SCHEMA_V7: &str = "memory-mcp-agent-paired/v7";
pub const PAIRED_ARTIFACT_SCHEMA_V6: &str = "memory-mcp-agent-paired/v6";
pub const PAIRED_COMPARISON_SCHEMA_V2: &str = "memory-mcp-agent-comparison/v2";
pub const PAIRED_PLAN_SCHEMA_V2: &str = "memory-mcp-agent-plan/v2";
pub const PAIRED_PILOT_REPORT_SCHEMA_V3: &str = "memory-mcp-agent-pilot-report/v3";
pub const PAIRED_EXPLORATORY_PILOT_REPORT_SCHEMA_V1: &str =
    "memory-mcp-agent-exploratory-pilot-report/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunArm {
    Incumbent,
    Candidate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrialStatus {
    Completed,
    TimedOut,
    ModelFailure,
    ToolFailure,
    BudgetExceeded,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceEventKind {
    /// The model began a turn, even if the process ended before usage arrived.
    AgentTurnStarted,
    AgentTurn,
    ToolCall,
    ToolResponse,
    ToolError,
    MemoryRead,
    MemoryWrite,
    Clarification,
    FinalAnswer,
    Timeout,
    ModelFailure,
    BudgetLimitReached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NormalizedErrorClass {
    InvalidArguments,
    /// A provider response did not satisfy the Chat Completions envelope contract.
    InvalidResponse,
    /// A text response did not satisfy the registered task's final-answer contract.
    InvalidFinalAnswer,
    /// A formation turn returned an empty completion instead of a status message or tool call.
    EmptyFormationCompletion,
    /// A recall turn returned text that could not be parsed as JSON.
    FinalAnswerNotJson,
    /// A recall turn returned JSON that did not match the required answer schema.
    FinalAnswerSchemaMismatch,
    /// A provider tool-call response did not satisfy the registered tool-call contract.
    InvalidToolCall,
    NotFound,
    Unauthorized,
    ApprovalRequired,
    Timeout,
    RateLimited,
    Internal,
    Transport,
    Retryable,
    Unknown,
}

impl NormalizedErrorClass {
    pub(crate) fn requires_v3(self) -> bool {
        matches!(
            self,
            Self::InvalidResponse
                | Self::InvalidFinalAnswer
                | Self::InvalidToolCall
                | Self::EmptyFormationCompletion
                | Self::FinalAnswerNotJson
                | Self::FinalAnswerSchemaMismatch
        )
    }

    pub(crate) fn requires_v4(self) -> bool {
        matches!(
            self,
            Self::EmptyFormationCompletion
                | Self::FinalAnswerNotJson
                | Self::FinalAnswerSchemaMismatch
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTraceEvent {
    pub sequence: u32,
    pub turn: u32,
    pub kind: TraceEventKind,
    pub call_id: Option<String>,
    pub tool_name: Option<String>,
    /// Closed, sanitized class; free-form provider or tool error text is forbidden.
    pub error_class: Option<NormalizedErrorClass>,
    pub payload_sha256: Option<String>,
    /// Provider-reported token usage for this model turn; absent on non-model events.
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HardViolation {
    /// An oracle prohibits this source without establishing why it is prohibited.
    ProhibitedEvidence,
    TemporalMismatch,
    UnsupportedEvidence,
    UnauthorizedRead,
    CrossPrincipalDisclosure,
    FabricatedReview,
    StaleAssertion,
    UnsafeAction,
    OracleLeak,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrialReceipt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_capture_status: Option<crate::paired_agent::ReadCaptureStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_diagnostics: Option<Vec<crate::paired_agent::MemoryReadObservation>>,
    pub key: CaseTrialKey,
    /// 1 or 2; the paired comparator requires opposite order in the two arms.
    pub arm_order: u8,
    /// Exact public knowledge snapshot substituted into a temporal task prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_revision_checkpoint: Option<i64>,
    pub status: TrialStatus,
    /// Legacy v2 agent-observed source coverage; independent probes use a separate versioned envelope.
    pub direct_memory_success: bool,
    /// Includes factual correctness, source support/abstention, or simulated final state as applicable.
    pub task_oracle_passed: bool,
    /// Content-free component checks for diagnosing task-oracle failures.
    /// Older paired artifact versions omit this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_oracle_diagnostics: Option<TaskOracleDiagnostics>,
    pub turns_used: u32,
    pub tool_calls: u32,
    pub repair_turns: u32,
    pub user_clarifications: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub wall_time_ms: u64,
    pub hard_violations: Vec<HardViolation>,
    /// Event metadata and payload hashes only; raw prompts, memory content, and oracle text stay private.
    pub trace: Vec<AgentTraceEvent>,
}

/// Safe per-trial oracle diagnostics. This deliberately excludes answer text,
/// citations, source content, and hidden oracle values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskOracleDiagnostics {
    pub final_answer_present: bool,
    pub final_answer_schema_valid: bool,
    /// `None` when no valid final-answer object was available to score.
    pub accepted_answer: Option<bool>,
    /// Content-free distinction between exact acceptance, a literal value
    /// embedded in extra wording, and no accepted literal value being present.
    /// `None` for artifacts before v6 or when no valid answer was available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_answer_match: Option<AcceptedAnswerMatch>,
    /// `None` when no valid final-answer object was available to score.
    pub required_evidence_cited: Option<bool>,
    pub clarification_matches: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptedAnswerMatch {
    Exact,
    MentionedWithExtraText,
    NoAcceptedValueMentioned,
}

impl TrialReceipt {
    pub fn task_success(&self) -> bool {
        self.status == TrialStatus::Completed
            && self.task_oracle_passed
            && self.hard_violations.is_empty()
    }

    pub fn charged_repair_turns(&self, turn_budget: u32) -> u32 {
        if self.task_success() {
            self.repair_turns
        } else {
            turn_budget
        }
    }

    pub fn charged_turns(&self, turn_budget: u32) -> u32 {
        if self.task_success() {
            self.turns_used
        } else {
            turn_budget
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedRunArtifact {
    pub schema_version: String,
    pub arm: RunArm,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub run_started_at: DateTime<Utc>,
    pub run_completed_at: DateTime<Utc>,
    pub identity: PairedRunIdentity,
    /// Frozen case/trial keys for both arms. Each trial remains in the denominator.
    pub expected_trials: Vec<CaseTrialKey>,
    pub receipts: Vec<TrialReceipt>,
}

impl PairedRunArtifact {
    pub fn validate(&self) -> Result<(), EvalError> {
        if !matches!(
            self.schema_version.as_str(),
            PAIRED_ARTIFACT_SCHEMA_V2
                | PAIRED_ARTIFACT_SCHEMA_V3
                | PAIRED_ARTIFACT_SCHEMA_V4
                | PAIRED_ARTIFACT_SCHEMA_V5
                | PAIRED_ARTIFACT_SCHEMA_V6
                | PAIRED_ARTIFACT_SCHEMA_V7
        ) {
            return Err(EvalError::InvalidInput(format!(
                "unsupported paired artifact schema {:?}",
                self.schema_version
            )));
        }
        if self.schema_version == PAIRED_ARTIFACT_SCHEMA_V2
            && self
                .receipts
                .iter()
                .flat_map(|receipt| &receipt.trace)
                .any(|event| {
                    event
                        .error_class
                        .is_some_and(NormalizedErrorClass::requires_v3)
                })
        {
            return Err(EvalError::InvalidInput(
                "paired artifact v2 cannot contain v3 model-response diagnostics".into(),
            ));
        }
        if self.schema_version == PAIRED_ARTIFACT_SCHEMA_V3
            && self
                .receipts
                .iter()
                .flat_map(|receipt| &receipt.trace)
                .any(|event| {
                    event
                        .error_class
                        .is_some_and(NormalizedErrorClass::requires_v4)
                })
        {
            return Err(EvalError::InvalidInput(
                "paired artifact v3 cannot contain granular final-answer diagnostics".into(),
            ));
        }
        if !matches!(
            self.schema_version.as_str(),
            PAIRED_ARTIFACT_SCHEMA_V5 | PAIRED_ARTIFACT_SCHEMA_V6 | PAIRED_ARTIFACT_SCHEMA_V7
        ) && self
            .receipts
            .iter()
            .any(|receipt| receipt.task_oracle_diagnostics.is_some())
        {
            return Err(EvalError::InvalidInput(
                "paired artifact v2-v4 cannot contain task-oracle component diagnostics".into(),
            ));
        }
        if matches!(
            self.schema_version.as_str(),
            PAIRED_ARTIFACT_SCHEMA_V5 | PAIRED_ARTIFACT_SCHEMA_V6 | PAIRED_ARTIFACT_SCHEMA_V7
        ) && self.receipts.iter().any(|receipt| {
            receipt.status != TrialStatus::Skipped && receipt.task_oracle_diagnostics.is_none()
        }) {
            return Err(EvalError::InvalidInput(
                "paired artifact v5-v6 requires task-oracle component diagnostics for every attempted trial".into(),
            ));
        }
        for receipt in &self.receipts {
            match (
                self.schema_version.as_str(),
                receipt.read_capture_status,
                &receipt.read_diagnostics,
            ) {
                (PAIRED_ARTIFACT_SCHEMA_V7, Some(status), Some(reads)) => {
                    crate::paired_agent::validate_read_associations(status, reads, &receipt.trace)?;
                }
                (PAIRED_ARTIFACT_SCHEMA_V7, None, None)
                    if receipt.status == TrialStatus::Skipped => {}
                (version, None, None) if version != PAIRED_ARTIFACT_SCHEMA_V7 => {}
                _ => {
                    return Err(EvalError::InvalidInput(
                        "read diagnostics do not match paired artifact version".into(),
                    ));
                }
            }
            let Some(diagnostics) = &receipt.task_oracle_diagnostics else {
                continue;
            };
            if self.schema_version == PAIRED_ARTIFACT_SCHEMA_V5
                && diagnostics.accepted_answer_match.is_some()
            {
                return Err(EvalError::InvalidInput(
                    "paired artifact v5 cannot contain v6 answer-match diagnostics".into(),
                ));
            }
            if matches!(
                self.schema_version.as_str(),
                PAIRED_ARTIFACT_SCHEMA_V6 | PAIRED_ARTIFACT_SCHEMA_V7
            ) && diagnostics.accepted_answer_match.is_some()
                != diagnostics.final_answer_schema_valid
            {
                return Err(EvalError::InvalidInput(
                    "paired artifact v6 requires answer-match diagnostics for every valid final answer".into(),
                ));
            }
        }
        if self
            .run_id
            .as_ref()
            .is_some_and(|run_id| run_id.trim().is_empty())
        {
            return Err(EvalError::InvalidInput(
                "paired run id must be non-empty when present".into(),
            ));
        }
        validate_identity("paired run", &self.identity)?;
        validate_run_window(
            &self.identity.shared.model_identity_protocol,
            self.run_id.as_deref(),
            self.run_started_at,
            self.run_completed_at,
        )?;
        let receipt_keys: Vec<_> = self
            .receipts
            .iter()
            .map(|receipt| receipt.key.clone())
            .collect();
        validate_case_coverage(&self.expected_trials, &receipt_keys, &receipt_keys)?;
        validate_manifest_strata(&self.expected_trials)?;
        validate_arm_order_schedule(&self.expected_trials)?;
        for receipt in &self.receipts {
            let incumbent_first = receipt.key.incumbent_first;
            let should_run_first = (self.arm == RunArm::Incumbent) == incumbent_first;
            let expected_order = if should_run_first { 1 } else { 2 };
            if receipt.arm_order != expected_order {
                return Err(EvalError::InvalidInput(format!(
                    "trial {} arm order differs from the preregistered schedule",
                    format_case_trial_key(&receipt.key)
                )));
            }
            validate_trial_receipt(receipt, &self.identity.shared)?;
        }
        Ok(())
    }

    /// Hashes the logical run artifact after sorting its order-insensitive trial collections.
    pub fn sha256(&self) -> Result<String, EvalError> {
        self.validate()?;
        let mut canonical = self.clone();
        canonical.expected_trials.sort();
        canonical
            .receipts
            .sort_by(|left, right| left.key.cmp(&right.key));
        let bytes = serde_json::to_vec(&canonical)?;
        Ok(hex::encode(Sha256::digest(bytes)))
    }
}

fn validate_manifest_strata(keys: &[CaseTrialKey]) -> Result<(), EvalError> {
    let mut family_strata = BTreeMap::<&str, &str>::new();
    for key in keys {
        match family_strata.insert(&key.family_id, &key.stratum_id) {
            Some(stratum) if stratum != key.stratum_id => {
                return Err(EvalError::InvalidInput(format!(
                    "independent family {:?} appears in multiple strata",
                    key.family_id
                )));
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_arm_order_schedule(keys: &[CaseTrialKey]) -> Result<(), EvalError> {
    let mut counts = BTreeMap::<&str, (u32, u32)>::new();
    for key in keys {
        let count = counts.entry(&key.stratum_id).or_default();
        if key.incumbent_first {
            count.0 += 1;
        } else {
            count.1 += 1;
        }
    }
    if counts
        .iter()
        .any(|(_, (incumbent_first, candidate_first))| {
            incumbent_first.abs_diff(*candidate_first) > 1
        })
    {
        return Err(EvalError::InvalidInput(
            "preregistered arm order must be balanced within every stratum".into(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_trial_receipt(
    receipt: &TrialReceipt,
    identity: &SharedRunIdentity,
) -> Result<(), EvalError> {
    if !matches!(receipt.arm_order, 1 | 2) {
        return Err(EvalError::InvalidInput(format!(
            "trial {} has arm_order outside 1 or 2",
            format_case_trial_key(&receipt.key)
        )));
    }
    if receipt
        .known_revision_checkpoint
        .is_some_and(|revision| revision < 0)
    {
        return Err(EvalError::InvalidInput(format!(
            "trial {} has a negative known-revision checkpoint",
            format_case_trial_key(&receipt.key)
        )));
    }
    if receipt.repair_turns > receipt.turns_used {
        return Err(EvalError::InvalidInput(format!(
            "trial {} has more repair turns than total turns",
            format_case_trial_key(&receipt.key)
        )));
    }
    let exceeded_work_budget = receipt.turns_used > identity.turn_budget
        || receipt.tool_calls > identity.tool_call_budget
        || receipt.output_tokens > identity.output_token_budget;
    let exceeded_wall_budget = receipt.wall_time_ms > identity.wall_time_budget_ms;
    if exceeded_work_budget && receipt.status != TrialStatus::BudgetExceeded {
        return Err(EvalError::InvalidInput(format!(
            "trial {} exceeded a declared resource budget without a budget_exceeded status",
            format_case_trial_key(&receipt.key)
        )));
    }
    if exceeded_wall_budget
        && !matches!(
            receipt.status,
            TrialStatus::TimedOut | TrialStatus::BudgetExceeded
        )
    {
        return Err(EvalError::InvalidInput(format!(
            "trial {} exceeded its wall-time budget without a timeout or budget_exceeded status",
            format_case_trial_key(&receipt.key)
        )));
    }
    if receipt.status != TrialStatus::Completed && receipt.task_oracle_passed {
        return Err(EvalError::InvalidInput(format!(
            "incomplete trial {} cannot pass its task oracle",
            format_case_trial_key(&receipt.key)
        )));
    }
    if receipt.status == TrialStatus::Skipped {
        let has_activity = receipt.direct_memory_success
            || receipt.turns_used != 0
            || receipt.tool_calls != 0
            || receipt.repair_turns != 0
            || receipt.user_clarifications != 0
            || receipt.input_tokens != 0
            || receipt.output_tokens != 0
            || receipt.wall_time_ms != 0
            || !receipt.hard_violations.is_empty()
            || !receipt.trace.is_empty();
        if has_activity {
            return Err(EvalError::InvalidInput(format!(
                "skipped trial {} cannot contain attempted work or a positive direct-memory result",
                format_case_trial_key(&receipt.key)
            )));
        }
    }

    if let Some(diagnostics) = &receipt.task_oracle_diagnostics {
        if diagnostics.final_answer_schema_valid && !diagnostics.final_answer_present {
            return Err(EvalError::InvalidInput(format!(
                "trial {} has a valid final-answer schema without a final answer",
                format_case_trial_key(&receipt.key)
            )));
        }
        let expected_component_presence = diagnostics.final_answer_schema_valid;
        if diagnostics.accepted_answer.is_some() != expected_component_presence
            || diagnostics.required_evidence_cited.is_some() != expected_component_presence
        {
            return Err(EvalError::InvalidInput(format!(
                "trial {} has task-oracle component values inconsistent with final-answer validity",
                format_case_trial_key(&receipt.key)
            )));
        }
        if diagnostics
            .accepted_answer_match
            .is_some_and(|answer_match| {
                diagnostics.accepted_answer != Some(answer_match == AcceptedAnswerMatch::Exact)
            })
        {
            return Err(EvalError::InvalidInput(format!(
                "trial {} has answer-match diagnostics inconsistent with exact answer acceptance",
                format_case_trial_key(&receipt.key)
            )));
        }
        let expected_task_oracle = receipt.status == TrialStatus::Completed
            && diagnostics.accepted_answer == Some(true)
            && diagnostics.required_evidence_cited == Some(true)
            && diagnostics.clarification_matches
            && receipt.hard_violations.is_empty();
        if receipt.task_oracle_passed != expected_task_oracle {
            return Err(EvalError::InvalidInput(format!(
                "trial {} task-oracle result differs from its content-free component diagnostics",
                format_case_trial_key(&receipt.key)
            )));
        }
    }

    let mut previous_sequence = 0;
    let mut started_turns = BTreeSet::new();
    let mut turns = BTreeSet::new();
    let mut failed_tool_turns = BTreeSet::new();
    let mut trace_input_tokens = 0_u64;
    let mut trace_output_tokens = 0_u64;
    let mut peak_context_tokens = 0_u64;
    let mut clarification_events = 0_u32;
    let mut tool_call_events = 0_u32;
    let mut pending_calls = BTreeMap::<String, String>::new();
    let mut completed_calls = BTreeSet::<String>::new();
    let mut final_answers = 0_u32;
    let mut timeout_events = 0_u32;
    let mut model_failure_events = 0_u32;
    let mut budget_events = 0_u32;
    for event in &receipt.trace {
        let may_be_outside_turn = matches!(
            event.kind,
            TraceEventKind::Timeout
                | TraceEventKind::ModelFailure
                | TraceEventKind::BudgetLimitReached
        );
        if event.sequence != previous_sequence + 1 || (event.turn == 0 && !may_be_outside_turn) {
            return Err(EvalError::InvalidInput(format!(
                "trial {} trace sequence and turn numbers must be positive and contiguous",
                format_case_trial_key(&receipt.key)
            )));
        }
        previous_sequence = event.sequence;
        if event.turn > receipt.turns_used {
            return Err(EvalError::InvalidInput(format!(
                "trial {} trace event exceeds recorded turns used",
                format_case_trial_key(&receipt.key)
            )));
        }
        match event.kind {
            TraceEventKind::AgentTurnStarted => {
                if !started_turns.insert(event.turn) {
                    return Err(EvalError::InvalidInput(format!(
                        "trial {} has duplicate agent-turn-start trace events",
                        format_case_trial_key(&receipt.key)
                    )));
                }
            }
            TraceEventKind::AgentTurn => {
                if !started_turns.contains(&event.turn) || !turns.insert(event.turn) {
                    return Err(EvalError::InvalidInput(format!(
                        "trial {} has a duplicate or unstarted agent-turn trace event",
                        format_case_trial_key(&receipt.key)
                    )));
                }
                let input_tokens = event.input_tokens.ok_or_else(|| {
                    EvalError::InvalidInput(format!(
                        "trial {} model turns require input-token usage",
                        format_case_trial_key(&receipt.key)
                    ))
                })?;
                let output_tokens = event.output_tokens.ok_or_else(|| {
                    EvalError::InvalidInput(format!(
                        "trial {} model turns require output-token usage",
                        format_case_trial_key(&receipt.key)
                    ))
                })?;
                trace_input_tokens =
                    trace_input_tokens
                        .checked_add(input_tokens)
                        .ok_or_else(|| {
                            EvalError::InvalidInput("input token total overflowed".into())
                        })?;
                trace_output_tokens =
                    trace_output_tokens
                        .checked_add(output_tokens)
                        .ok_or_else(|| {
                            EvalError::InvalidInput("output token total overflowed".into())
                        })?;
                peak_context_tokens = peak_context_tokens.max(input_tokens);
            }
            TraceEventKind::ToolCall | TraceEventKind::MemoryRead | TraceEventKind::MemoryWrite => {
                if !started_turns.contains(&event.turn) {
                    return Err(EvalError::InvalidInput(format!(
                        "trial {} has a tool call outside an active agent turn",
                        format_case_trial_key(&receipt.key)
                    )));
                }
                let call_id =
                    required_trace_field(&receipt.key, "call_id", event.call_id.as_deref())?;
                let tool_name =
                    required_trace_field(&receipt.key, "tool_name", event.tool_name.as_deref())?;
                if event.error_class.is_some()
                    || completed_calls.contains(call_id)
                    || pending_calls
                        .insert(call_id.to_string(), tool_name.to_string())
                        .is_some()
                {
                    return Err(EvalError::InvalidInput(format!(
                        "trial {} has a duplicate or malformed tool call trace",
                        format_case_trial_key(&receipt.key)
                    )));
                }
                tool_call_events += 1;
            }
            TraceEventKind::ToolResponse | TraceEventKind::ToolError => {
                let call_id =
                    required_trace_field(&receipt.key, "call_id", event.call_id.as_deref())?;
                let tool_name =
                    required_trace_field(&receipt.key, "tool_name", event.tool_name.as_deref())?;
                let expected_tool = pending_calls.remove(call_id).ok_or_else(|| {
                    EvalError::InvalidInput(format!(
                        "trial {} has a tool result without a pending call",
                        format_case_trial_key(&receipt.key)
                    ))
                })?;
                if expected_tool != tool_name || !completed_calls.insert(call_id.to_string()) {
                    return Err(EvalError::InvalidInput(format!(
                        "trial {} tool result does not match its call",
                        format_case_trial_key(&receipt.key)
                    )));
                }
                if event.kind == TraceEventKind::ToolError && event.error_class.is_none() {
                    return Err(EvalError::InvalidInput(format!(
                        "trial {} tool errors require a normalized error class",
                        format_case_trial_key(&receipt.key)
                    )));
                }
                if event.kind == TraceEventKind::ToolResponse && event.error_class.is_some() {
                    return Err(EvalError::InvalidInput(format!(
                        "trial {} successful tool responses cannot contain an error class",
                        format_case_trial_key(&receipt.key)
                    )));
                }
                if event.kind == TraceEventKind::ToolError {
                    failed_tool_turns.insert(event.turn);
                }
            }
            TraceEventKind::Clarification => clarification_events += 1,
            TraceEventKind::FinalAnswer => final_answers += 1,
            TraceEventKind::Timeout => timeout_events += 1,
            TraceEventKind::ModelFailure => model_failure_events += 1,
            TraceEventKind::BudgetLimitReached => budget_events += 1,
        }
        if matches!(
            event.kind,
            TraceEventKind::ToolCall
                | TraceEventKind::ToolResponse
                | TraceEventKind::MemoryRead
                | TraceEventKind::MemoryWrite
        ) && event.payload_sha256.is_none()
        {
            return Err(EvalError::InvalidInput(format!(
                "trial {} tool trace events require a payload fingerprint",
                format_case_trial_key(&receipt.key)
            )));
        }
        if event.kind == TraceEventKind::FinalAnswer && event.payload_sha256.is_none() {
            return Err(EvalError::InvalidInput(format!(
                "trial {} final answer requires a payload fingerprint",
                format_case_trial_key(&receipt.key)
            )));
        }
        if let Some(digest) = &event.payload_sha256 {
            validate_sha256("trace payload", digest)?;
        }
        if event.kind != TraceEventKind::AgentTurn
            && (event.input_tokens.is_some() || event.output_tokens.is_some())
        {
            return Err(EvalError::InvalidInput(format!(
                "trial {} token usage may only appear on model-turn trace events",
                format_case_trial_key(&receipt.key)
            )));
        }
        if !matches!(
            event.kind,
            TraceEventKind::ToolError | TraceEventKind::ModelFailure | TraceEventKind::Timeout
        ) && event.error_class.is_some()
        {
            return Err(EvalError::InvalidInput(format!(
                "trial {} normalized error classes may only appear on tool-error, model-failure, or timeout events",
                format_case_trial_key(&receipt.key)
            )));
        }
    }
    let failed_tool_turn_count = u32::try_from(failed_tool_turns.len())
        .map_err(|_| EvalError::InvalidInput("too many failed tool turns".into()))?;
    if started_turns.len() != usize::try_from(receipt.turns_used).unwrap_or(usize::MAX)
        || started_turns.iter().copied().ne(1..=receipt.turns_used)
        || !turns.is_subset(&started_turns)
        || (receipt.status == TrialStatus::Completed
            && turns.len() != usize::try_from(receipt.turns_used).unwrap_or(usize::MAX))
        || tool_call_events != receipt.tool_calls
        || clarification_events != receipt.user_clarifications
        || failed_tool_turn_count != receipt.repair_turns
        || trace_input_tokens != receipt.input_tokens
        || trace_output_tokens != receipt.output_tokens
    {
        return Err(EvalError::InvalidInput(format!(
            "trial {} attempted-turn, tool-call, repair-turn, clarification, or token counters disagree with the trace",
            format_case_trial_key(&receipt.key)
        )));
    }
    if peak_context_tokens > identity.context_budget_tokens
        && receipt.status != TrialStatus::BudgetExceeded
    {
        return Err(EvalError::InvalidInput(format!(
            "trial {} exceeded its per-turn context budget without a budget_exceeded status",
            format_case_trial_key(&receipt.key)
        )));
    }
    if receipt.status == TrialStatus::Completed && (final_answers != 1 || !pending_calls.is_empty())
    {
        return Err(EvalError::InvalidInput(format!(
            "completed trial {} requires one final answer and no pending tool calls",
            format_case_trial_key(&receipt.key)
        )));
    }
    if receipt.status != TrialStatus::Completed && final_answers > 1 {
        return Err(EvalError::InvalidInput(format!(
            "incomplete trial {} cannot contain multiple final-answer events",
            format_case_trial_key(&receipt.key)
        )));
    }
    for (status, count, event_name) in [
        (TrialStatus::TimedOut, timeout_events, "timeout"),
        (
            TrialStatus::ModelFailure,
            model_failure_events,
            "model failure",
        ),
        (TrialStatus::BudgetExceeded, budget_events, "budget limit"),
    ] {
        let expected_count = u32::from(receipt.status == status);
        if count != expected_count {
            return Err(EvalError::InvalidInput(format!(
                "trial {} status and {event_name} trace events disagree",
                format_case_trial_key(&receipt.key)
            )));
        }
    }
    if receipt.status == TrialStatus::ToolFailure && failed_tool_turn_count == 0 {
        return Err(EvalError::InvalidInput(format!(
            "tool-failure trial {} requires at least one normalized tool error",
            format_case_trial_key(&receipt.key)
        )));
    }
    Ok(())
}

fn required_trace_field<'a>(
    key: &CaseTrialKey,
    field: &str,
    value: Option<&'a str>,
) -> Result<&'a str, EvalError> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            EvalError::InvalidInput(format!(
                "trial {} tool trace requires {field}",
                format_case_trial_key(key)
            ))
        })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapConfig {
    pub seed: u64,
    pub draws: u32,
    pub confidence_level_basis_points: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfidenceInterval {
    pub estimate: f64,
    pub lower: f64,
    pub upper: f64,
    pub independent_clusters: u32,
    pub draws: u32,
    pub seed: u64,
    pub confidence_level_basis_points: u16,
    pub algorithm: String,
}

pub fn cluster_bootstrap(
    cluster_deltas: &BTreeMap<String, f64>,
    config: &BootstrapConfig,
) -> Result<ConfidenceInterval, EvalError> {
    if cluster_deltas.len() < 2 {
        return Err(EvalError::InvalidInput(
            "paired bootstrap requires at least two independent clusters".into(),
        ));
    }
    if cluster_deltas
        .iter()
        .any(|(cluster, delta)| cluster.trim().is_empty() || !delta.is_finite())
    {
        return Err(EvalError::InvalidInput(
            "paired bootstrap requires non-empty cluster ids and finite deltas".into(),
        ));
    }
    validate_bootstrap_config(config)?;

    let cluster_count = u64::try_from(cluster_deltas.len()).map_err(|_| {
        EvalError::InvalidInput("paired bootstrap has too many independent clusters".into())
    })?;
    let independent_clusters = u32::try_from(cluster_deltas.len()).map_err(|_| {
        EvalError::InvalidInput("paired bootstrap has too many independent clusters".into())
    })?;
    if u64::from(config.draws).saturating_mul(cluster_count) > 100_000_000 {
        return Err(EvalError::InvalidInput(
            "paired bootstrap exceeds the 100 million resampled-cluster work limit".into(),
        ));
    }
    let values: Vec<f64> = cluster_deltas.values().copied().collect();
    let estimate = values.iter().sum::<f64>() / values.len() as f64;
    let mut rng = DeterministicBootstrapRng::new(config.seed);
    let mut samples = Vec::with_capacity(config.draws as usize);
    for _ in 0..config.draws {
        let mut sum = 0.0;
        for _ in 0..cluster_count {
            let index = usize::try_from(rng.next_index(cluster_count))
                .map_err(|_| EvalError::InvalidInput("bootstrap index overflowed".into()))?;
            sum += values[index];
        }
        samples.push(sum / cluster_count as f64);
    }
    samples.sort_by(f64::total_cmp);

    let tail_numerator = 10_000 - u64::from(config.confidence_level_basis_points);
    let last_index = u64::from(config.draws - 1);
    let lower_index = tail_numerator * last_index / 20_000;
    let upper_numerator = (20_000 - tail_numerator) * last_index;
    let upper_index = upper_numerator.div_ceil(20_000);
    let lower_index = usize::try_from(lower_index)
        .map_err(|_| EvalError::InvalidInput("bootstrap lower quantile overflowed".into()))?;
    let upper_index = usize::try_from(upper_index)
        .map_err(|_| EvalError::InvalidInput("bootstrap upper quantile overflowed".into()))?;

    Ok(ConfidenceInterval {
        estimate,
        lower: samples[lower_index],
        upper: samples[upper_index],
        independent_clusters,
        draws: config.draws,
        seed: config.seed,
        confidence_level_basis_points: config.confidence_level_basis_points,
        algorithm: "splitmix64-v1".into(),
    })
}

struct DeterministicBootstrapRng {
    state: u64,
}

impl DeterministicBootstrapRng {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_index(&mut self, upper_bound: u64) -> u64 {
        let acceptance_bound = u64::MAX - u64::MAX % upper_bound;
        loop {
            self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut sample = self.state;
            sample = (sample ^ (sample >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            sample = (sample ^ (sample >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            sample ^= sample >> 31;
            if sample < acceptance_bound {
                return sample % upper_bound;
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricEstimate {
    pub estimate: f64,
    pub interval: Option<ConfidenceInterval>,
    /// Paired deltas after averaging repeats within cases and cases within families.
    pub family_deltas: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedMetricSet {
    pub direct_memory_success: MetricEstimate,
    pub task_success: MetricEstimate,
    /// Improvement is incumbent charged repair turns minus candidate charged repair turns.
    pub repair_turn_gain: MetricEstimate,
    /// Improvement is incumbent charged turns minus candidate charged turns.
    pub turn_gain: MetricEstimate,
    pub tool_call_gain: MetricEstimate,
    pub clarification_gain: MetricEstimate,
    pub input_token_gain: MetricEstimate,
    pub output_token_gain: MetricEstimate,
    pub wall_time_gain_ms: MetricEstimate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedRunComparison {
    pub schema_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub run_started_at: DateTime<Utc>,
    pub run_completed_at: DateTime<Utc>,
    pub plan: PairedEvaluationPlan,
    pub plan_sha256: String,
    pub manifest_sha256: String,
    pub incumbent_artifact_sha256: String,
    pub candidate_artifact_sha256: String,
    pub overall: PairedMetricSet,
    pub by_stratum: BTreeMap<String, PairedMetricSet>,
    pub paired_trial_count: u64,
    pub independent_family_count: u32,
    pub hard_violation_count: u64,
    pub budget_exceeded_trial_count: u64,
    pub arm_order_counts: BTreeMap<String, ArmOrderCounts>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PilotMetric {
    DirectMemorySuccess,
    TaskSuccess,
    RepairTurnGain,
    TurnGain,
    ToolCallGain,
    ClarificationGain,
    InputTokenGain,
    OutputTokenGain,
    WallTimeGainMs,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PilotMetricSummary {
    pub first_run_family_deltas: BTreeMap<String, f64>,
    pub second_run_family_deltas: BTreeMap<String, f64>,
    /// Equal-weight mean of the two run-level deltas for each independent family.
    pub family_deltas: BTreeMap<String, f64>,
    /// Sample variance across independent family means, with denominator families - 1.
    pub between_family_sample_variance: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PilotOutcome {
    DirectMemorySuccess,
    TaskSuccess,
    ChargedRepairTurns,
    ChargedTurns,
    ToolCalls,
    UserClarifications,
    InputTokens,
    OutputTokens,
    WallTimeMs,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PilotOutcomeSummary {
    /// Per-family means within a run, averaging the two same-product arms.
    pub first_run_family_means: BTreeMap<String, f64>,
    pub second_run_family_means: BTreeMap<String, f64>,
    /// Equal-weight average of both run means for each independent family.
    pub family_means: BTreeMap<String, f64>,
    /// Second-run minus first-run outcome for each independent family.
    pub retest_family_deltas: BTreeMap<String, f64>,
    /// Descriptive sample variance across pooled family means.
    pub between_family_sample_variance: f64,
    /// Sample variance across family-level second-minus-first retest deltas.
    pub test_retest_sample_variance: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentPilotReport {
    pub schema_version: String,
    pub plan_sha256: String,
    pub manifest_sha256: String,
    pub independent_family_count: u32,
    pub paired_trial_count_per_run: u64,
    pub first_run: PairedRunComparison,
    pub second_run: PairedRunComparison,
    pub metrics: BTreeMap<PilotMetric, PilotMetricSummary>,
    pub same_product_outcomes: BTreeMap<PilotOutcome, PilotOutcomeSummary>,
}

impl DevelopmentPilotReport {
    pub fn validate(&self) -> Result<(), EvalError> {
        if self.schema_version != PAIRED_PILOT_REPORT_SCHEMA_V3 {
            return Err(EvalError::InvalidInput(
                "unsupported development pilot report schema".into(),
            ));
        }
        self.first_run.validate()?;
        self.second_run.validate()?;
        if self.first_run.plan.stage != PairedEvaluationStage::DevelopmentPilot
            || self.second_run.plan.stage != PairedEvaluationStage::DevelopmentPilot
            || self.first_run.plan_sha256 != self.plan_sha256
            || self.second_run.plan_sha256 != self.plan_sha256
            || self.first_run.manifest_sha256 != self.manifest_sha256
            || self.second_run.manifest_sha256 != self.manifest_sha256
            || self.first_run.run_id.is_none()
            || self.second_run.run_id.is_none()
            || self.first_run.run_id == self.second_run.run_id
            || self.first_run.independent_family_count != self.independent_family_count
            || self.second_run.independent_family_count != self.independent_family_count
            || self.first_run.paired_trial_count != self.paired_trial_count_per_run
            || self.second_run.paired_trial_count != self.paired_trial_count_per_run
        {
            return Err(EvalError::InvalidInput(
                "pilot report run identities or coverage do not match its frozen plan".into(),
            ));
        }
        let artifact_hashes = [
            self.first_run.incumbent_artifact_sha256.as_str(),
            self.first_run.candidate_artifact_sha256.as_str(),
            self.second_run.incumbent_artifact_sha256.as_str(),
            self.second_run.candidate_artifact_sha256.as_str(),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        if artifact_hashes.len() != 4 {
            return Err(EvalError::InvalidInput(
                "pilot report must use four distinct run artifacts from two independent executions"
                    .into(),
            ));
        }
        let expected_metrics = pilot_metric_summaries(&self.first_run, &self.second_run)?;
        if self.metrics != expected_metrics {
            return Err(EvalError::InvalidInput(
                "pilot variance summaries do not reproduce from the two run comparisons".into(),
            ));
        }
        let first_outcomes = self
            .same_product_outcomes
            .iter()
            .map(|(outcome, summary)| (*outcome, summary.first_run_family_means.clone()))
            .collect();
        let second_outcomes = self
            .same_product_outcomes
            .iter()
            .map(|(outcome, summary)| (*outcome, summary.second_run_family_means.clone()))
            .collect();
        let expected_families = self
            .first_run
            .overall
            .task_success
            .family_deltas
            .keys()
            .collect::<BTreeSet<_>>();
        if self.same_product_outcomes.values().any(|summary| {
            summary
                .first_run_family_means
                .keys()
                .collect::<BTreeSet<_>>()
                != expected_families
                || summary
                    .second_run_family_means
                    .keys()
                    .collect::<BTreeSet<_>>()
                    != expected_families
        }) {
            return Err(EvalError::InvalidInput(
                "same-product outcome summaries do not match independent family coverage".into(),
            ));
        }
        if self.same_product_outcomes != pilot_outcome_summaries(&first_outcomes, &second_outcomes)?
        {
            return Err(EvalError::InvalidInput(
                "same-product outcome summaries do not reproduce from their per-run family means"
                    .into(),
            ));
        }
        Ok(())
    }
}

/// Repeatability summary for an exploratory paired comparison. Unlike a
/// development pilot, the two arms may represent different products, so each
/// arm's outcome variance is reported separately.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExploratoryPilotReport {
    pub schema_version: String,
    pub plan_sha256: String,
    pub manifest_sha256: String,
    pub independent_family_count: u32,
    pub paired_trial_count_per_run: u64,
    pub first_run: PairedRunComparison,
    pub second_run: PairedRunComparison,
    pub metrics: BTreeMap<PilotMetric, PilotMetricSummary>,
    pub incumbent_outcomes: BTreeMap<PilotOutcome, PilotOutcomeSummary>,
    pub candidate_outcomes: BTreeMap<PilotOutcome, PilotOutcomeSummary>,
}

impl ExploratoryPilotReport {
    pub fn validate(&self) -> Result<(), EvalError> {
        if self.schema_version != PAIRED_EXPLORATORY_PILOT_REPORT_SCHEMA_V1 {
            return Err(EvalError::InvalidInput(
                "unsupported exploratory pilot report schema".into(),
            ));
        }
        self.first_run.validate()?;
        self.second_run.validate()?;
        if self.first_run.plan.stage != PairedEvaluationStage::ExploratoryComparison
            || self.second_run.plan.stage != PairedEvaluationStage::ExploratoryComparison
            || self.first_run.plan_sha256 != self.plan_sha256
            || self.second_run.plan_sha256 != self.plan_sha256
            || self.first_run.manifest_sha256 != self.manifest_sha256
            || self.second_run.manifest_sha256 != self.manifest_sha256
            || self.first_run.run_id.is_none()
            || self.second_run.run_id.is_none()
            || self.first_run.run_id == self.second_run.run_id
            || self.first_run.independent_family_count != self.independent_family_count
            || self.second_run.independent_family_count != self.independent_family_count
            || self.first_run.paired_trial_count != self.paired_trial_count_per_run
            || self.second_run.paired_trial_count != self.paired_trial_count_per_run
        {
            return Err(EvalError::InvalidInput(
                "exploratory pilot run identities or coverage do not match its frozen plan".into(),
            ));
        }
        let artifact_hashes = [
            self.first_run.incumbent_artifact_sha256.as_str(),
            self.first_run.candidate_artifact_sha256.as_str(),
            self.second_run.incumbent_artifact_sha256.as_str(),
            self.second_run.candidate_artifact_sha256.as_str(),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        if artifact_hashes.len() != 4 {
            return Err(EvalError::InvalidInput(
                "exploratory pilot report must use four distinct run artifacts from two independent executions".into(),
            ));
        }
        let expected_metrics = pilot_metric_summaries(&self.first_run, &self.second_run)?;
        if self.metrics != expected_metrics {
            return Err(EvalError::InvalidInput(
                "exploratory pilot metrics do not reproduce from the two run comparisons".into(),
            ));
        }
        validate_pilot_outcome_summaries(
            &self.incumbent_outcomes,
            &self.first_run,
            &self.second_run,
            "incumbent",
        )?;
        validate_pilot_outcome_summaries(
            &self.candidate_outcomes,
            &self.first_run,
            &self.second_run,
            "candidate",
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArmOrderCounts {
    pub incumbent_first: u32,
    pub candidate_first: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionCriteria {
    pub direct_memory_margin: f64,
    pub task_success_margin: f64,
    pub repair_turn_gain_margin: f64,
    /// Minimum acceptable candidate-minus-incumbent task-success delta by broad stratum.
    pub task_non_inferiority_by_stratum: BTreeMap<String, f64>,
    pub ordinary_workflow_strata: BTreeSet<String>,
    pub resource_non_inferiority: ResourceNonInferiority,
    pub resource_non_inferiority_by_stratum: BTreeMap<String, ResourceNonInferiority>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceNonInferiority {
    /// Maximum allowed candidate regression, in interactions per task.
    pub turns: f64,
    pub tool_calls: f64,
    pub user_clarifications: f64,
    pub input_tokens: f64,
    pub output_tokens: f64,
    pub wall_time_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedEvaluationPlan {
    pub schema_version: String,
    pub plan_id: String,
    #[serde(default, skip_serializing_if = "is_confirmatory_stage")]
    pub stage: PairedEvaluationStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pilot_report_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_analysis_sha256: Option<String>,
    pub expected_trials: Vec<CaseTrialKey>,
    pub planned_independent_families: u32,
    pub shared_identity: SharedRunIdentity,
    pub incumbent_product: ProductRunIdentity,
    pub candidate_product: ProductRunIdentity,
    pub declared_tool_schema_change: Option<DeclaredToolSchemaChange>,
    pub bootstrap: BootstrapConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promotion_criteria: Option<PromotionCriteria>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairedEvaluationStage {
    DevelopmentPilot,
    ExploratoryComparison,
    #[default]
    Confirmatory,
}

fn is_confirmatory_stage(stage: &PairedEvaluationStage) -> bool {
    *stage == PairedEvaluationStage::Confirmatory
}

impl PairedEvaluationPlan {
    /// Resolve the effective claim stage for both products. New plans pin one
    /// stage per arm; legacy plans may supply a shared override and otherwise
    /// retain the historical Evidence default.
    pub fn claim_rollout_stages(
        &self,
        common_override: Option<&str>,
    ) -> Result<(String, String), EvalError> {
        match (
            self.incumbent_product.claim_rollout_stage.as_deref(),
            self.candidate_product.claim_rollout_stage.as_deref(),
        ) {
            (Some(incumbent), Some(candidate)) => {
                for stage in [incumbent, candidate] {
                    if !is_supported_claim_rollout_stage(stage) {
                        return Err(EvalError::InvalidInput(
                            "paired plan contains an unsupported claim rollout stage".into(),
                        ));
                    }
                }
                if common_override.is_some_and(|stage| {
                    stage != incumbent || stage != candidate
                }) {
                    return Err(EvalError::InvalidInput(
                        "common claim rollout override conflicts with per-arm stages pinned in the plan".into(),
                    ));
                }
                Ok((incumbent.into(), candidate.into()))
            }
            (None, None) => {
                let stage = common_override.unwrap_or("evidence");
                if !is_supported_claim_rollout_stage(stage) {
                    return Err(EvalError::InvalidInput(
                        "claim rollout stage is unsupported".into(),
                    ));
                }
                Ok((stage.into(), stage.into()))
            }
            _ => Err(EvalError::InvalidInput(
                "paired product identities must either both pin claim rollout stages or both omit them".into(),
            )),
        }
    }

    pub fn validate(&self) -> Result<(), EvalError> {
        if self.schema_version != PAIRED_PLAN_SCHEMA_V2 || self.plan_id.trim().is_empty() {
            return Err(EvalError::InvalidInput(
                "paired evaluation plan requires a supported schema and a plan id".into(),
            ));
        }
        match self.stage {
            PairedEvaluationStage::Confirmatory => {
                validate_sha256(
                    "pilot report",
                    self.pilot_report_sha256.as_deref().ok_or_else(|| {
                        EvalError::InvalidInput(
                            "confirmatory paired plan requires a pilot report hash".into(),
                        )
                    })?,
                )?;
                validate_sha256(
                    "power analysis",
                    self.power_analysis_sha256.as_deref().ok_or_else(|| {
                        EvalError::InvalidInput(
                            "confirmatory paired plan requires a power analysis hash".into(),
                        )
                    })?,
                )?;
                validate_promotion_criteria(self.promotion_criteria.as_ref().ok_or_else(
                    || {
                        EvalError::InvalidInput(
                            "confirmatory paired plan requires promotion criteria".into(),
                        )
                    },
                )?)?;
            }
            PairedEvaluationStage::DevelopmentPilot => {
                if self.pilot_report_sha256.is_some()
                    || self.power_analysis_sha256.is_some()
                    || self.promotion_criteria.is_some()
                {
                    return Err(EvalError::InvalidInput(
                        "development pilot plans cannot claim post-pilot reports, power analyses, or promotion criteria".into(),
                    ));
                }
                if self.incumbent_product != self.candidate_product
                    || self.declared_tool_schema_change.is_some()
                {
                    return Err(EvalError::InvalidInput(
                        "development pilot plans must repeat one identical incumbent product in both arms".into(),
                    ));
                }
            }
            PairedEvaluationStage::ExploratoryComparison => {
                if self.pilot_report_sha256.is_some()
                    || self.power_analysis_sha256.is_some()
                    || self.promotion_criteria.is_some()
                {
                    return Err(EvalError::InvalidInput(
                        "exploratory comparison plans cannot claim pilot or power reports or promotion criteria".into(),
                    ));
                }
            }
        }
        validate_bootstrap_config(&self.bootstrap)?;
        let manifest = case_key_set("preregistered manifest", &self.expected_trials)?;
        if manifest.is_empty() {
            return Err(EvalError::InvalidInput(
                "paired evaluation plan requires at least one preregistered trial".into(),
            ));
        }
        validate_manifest_strata(&self.expected_trials)?;
        validate_arm_order_schedule(&self.expected_trials)?;
        let manifest_strata: BTreeSet<_> = self
            .expected_trials
            .iter()
            .map(|key| key.stratum_id.as_str())
            .collect();
        if self.promotion_criteria.as_ref().is_some_and(|criteria| {
            criteria
                .ordinary_workflow_strata
                .iter()
                .any(|stratum| !manifest_strata.contains(stratum.as_str()))
        }) {
            return Err(EvalError::InvalidInput(
                "ordinary-workflow non-inferiority strata must exist in the frozen manifest".into(),
            ));
        }
        let family_count = self
            .expected_trials
            .iter()
            .map(|key| key.family_id.as_str())
            .collect::<BTreeSet<_>>()
            .len();
        if self.planned_independent_families < 2
            || usize::try_from(self.planned_independent_families).ok() != Some(family_count)
        {
            return Err(EvalError::InvalidInput(
                "paired plan family count must match at least two independent manifest families"
                    .into(),
            ));
        }
        let incumbent = PairedRunIdentity {
            shared: self.shared_identity.clone(),
            product: self.incumbent_product.clone(),
        };
        let candidate = PairedRunIdentity {
            shared: self.shared_identity.clone(),
            product: self.candidate_product.clone(),
        };
        validate_pair(
            &incumbent,
            &candidate,
            self.declared_tool_schema_change.as_ref(),
        )?;
        for stratum in self
            .promotion_criteria
            .iter()
            .flat_map(|criteria| criteria.task_non_inferiority_by_stratum.keys())
        {
            let independent_families = self
                .expected_trials
                .iter()
                .filter(|key| key.stratum_id == *stratum)
                .map(|key| key.family_id.as_str())
                .collect::<BTreeSet<_>>()
                .len();
            if independent_families < 2 {
                return Err(EvalError::InvalidInput(format!(
                    "non-inferiority stratum {stratum:?} requires at least two independent families"
                )));
            }
        }
        Ok(())
    }

    pub fn sha256(&self) -> Result<String, EvalError> {
        self.validate()?;
        let mut canonical = self.clone();
        canonical.expected_trials.sort();
        let bytes = serde_json::to_vec(&canonical)?;
        Ok(hex::encode(Sha256::digest(bytes)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionDecision {
    Promote,
    Reject,
    InsufficientEvidence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedEvaluationReport {
    pub comparison: PairedRunComparison,
    pub promotion: PromotionEvaluation,
}

impl PairedEvaluationReport {
    pub fn validate(&self) -> Result<(), EvalError> {
        let criteria = self
            .comparison
            .plan
            .promotion_criteria
            .as_ref()
            .ok_or_else(|| {
                EvalError::InvalidInput(
                    "development pilot comparisons cannot be published as promotion reports".into(),
                )
            })?;
        let expected = evaluate_promotion(&self.comparison, criteria)?;
        if self.promotion != expected {
            return Err(EvalError::InvalidInput(
                "paired report decision does not reproduce from its comparison and frozen plan"
                    .into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionCheckStatus {
    Passed,
    Failed,
    InsufficientEvidence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionCheck {
    pub name: String,
    pub status: PromotionCheckStatus,
    pub threshold: f64,
    pub lower: Option<f64>,
    pub upper: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionEvaluation {
    pub decision: PromotionDecision,
    pub checks: Vec<PromotionCheck>,
}

impl PairedRunComparison {
    pub fn validate(&self) -> Result<(), EvalError> {
        if self.schema_version != PAIRED_COMPARISON_SCHEMA_V2 {
            return Err(EvalError::InvalidInput(
                "unsupported paired comparison schema".into(),
            ));
        }
        self.plan.validate()?;
        validate_run_window(
            &self.plan.shared_identity.model_identity_protocol,
            self.run_id.as_deref(),
            self.run_started_at,
            self.run_completed_at,
        )?;
        if self
            .run_id
            .as_ref()
            .is_some_and(|run_id| run_id.trim().is_empty())
        {
            return Err(EvalError::InvalidInput(
                "paired comparison run id must be non-empty when present".into(),
            ));
        }
        if self.plan.stage == PairedEvaluationStage::DevelopmentPilot && self.run_id.is_none() {
            return Err(EvalError::InvalidInput(
                "development pilot comparisons require runner-generated run identities".into(),
            ));
        }
        if self.plan_sha256 != self.plan.sha256()? {
            return Err(EvalError::InvalidInput(
                "paired comparison plan hash does not match the embedded preregistration".into(),
            ));
        }
        if self.manifest_sha256 != expected_manifest_sha256(&self.plan.expected_trials)? {
            return Err(EvalError::InvalidInput(
                "paired comparison manifest hash does not match its preregistered trial list"
                    .into(),
            ));
        }
        validate_sha256("incumbent run artifact", &self.incumbent_artifact_sha256)?;
        validate_sha256("candidate run artifact", &self.candidate_artifact_sha256)?;
        let expected_trial_count = u64::try_from(self.plan.expected_trials.len())
            .map_err(|_| EvalError::InvalidInput("paired plan contains too many trials".into()))?;
        if self.paired_trial_count != expected_trial_count {
            return Err(EvalError::InvalidInput(
                "paired comparison trial count does not match the preregistered manifest".into(),
            ));
        }
        let all_families = family_ids(&self.plan.expected_trials, None);
        if usize::try_from(self.independent_family_count).ok() != Some(all_families.len())
            || self.by_stratum.keys().cloned().collect::<BTreeSet<_>>()
                != self
                    .plan
                    .expected_trials
                    .iter()
                    .map(|key| key.stratum_id.clone())
                    .collect::<BTreeSet<_>>()
        {
            return Err(EvalError::InvalidInput(
                "paired comparison family or stratum coverage is inconsistent with the plan".into(),
            ));
        }
        validate_metric_set(
            &self.overall,
            &all_families,
            &self.plan.bootstrap,
            "overall",
        )?;
        for (stratum, metrics) in &self.by_stratum {
            let families = family_ids(&self.plan.expected_trials, Some(stratum));
            validate_metric_set(
                metrics,
                &families,
                &self.plan.bootstrap,
                &format!("stratum {stratum:?}"),
            )?;
        }

        let mut expected_order_counts = BTreeMap::<String, ArmOrderCounts>::new();
        for key in &self.plan.expected_trials {
            let counts = expected_order_counts
                .entry(key.stratum_id.clone())
                .or_insert(ArmOrderCounts {
                    incumbent_first: 0,
                    candidate_first: 0,
                });
            if key.incumbent_first {
                counts.incumbent_first += 1;
            } else {
                counts.candidate_first += 1;
            }
        }
        if self.arm_order_counts != expected_order_counts {
            return Err(EvalError::InvalidInput(
                "paired comparison execution-order counts disagree with the preregistered manifest"
                    .into(),
            ));
        }
        if self.budget_exceeded_trial_count > self.paired_trial_count.saturating_mul(2) {
            return Err(EvalError::InvalidInput(
                "paired comparison counts more budget failures than arm trials".into(),
            ));
        }
        Ok(())
    }
}

fn family_ids(keys: &[CaseTrialKey], stratum: Option<&str>) -> BTreeSet<String> {
    keys.iter()
        .filter(|key| stratum.is_none_or(|stratum| key.stratum_id == stratum))
        .map(|key| key.family_id.clone())
        .collect()
}

fn validate_metric_set(
    metrics: &PairedMetricSet,
    expected_families: &BTreeSet<String>,
    bootstrap: &BootstrapConfig,
    scope: &str,
) -> Result<(), EvalError> {
    for (name, metric) in [
        ("direct_memory_success", &metrics.direct_memory_success),
        ("task_success", &metrics.task_success),
        ("repair_turn_gain", &metrics.repair_turn_gain),
        ("turn_gain", &metrics.turn_gain),
        ("tool_call_gain", &metrics.tool_call_gain),
        ("clarification_gain", &metrics.clarification_gain),
        ("input_token_gain", &metrics.input_token_gain),
        ("output_token_gain", &metrics.output_token_gain),
        ("wall_time_gain_ms", &metrics.wall_time_gain_ms),
    ] {
        if metric
            .family_deltas
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            != expected_families.iter().map(String::as_str).collect()
            || metric
                .family_deltas
                .values()
                .any(|delta| !delta.is_finite())
            || !metric.estimate.is_finite()
        {
            return Err(EvalError::InvalidInput(format!(
                "paired {scope}.{name} family deltas do not match preregistered independent families"
            )));
        }
        let expected_estimate = average_iter(metric.family_deltas.values().copied());
        if metric.estimate != expected_estimate {
            return Err(EvalError::InvalidInput(format!(
                "paired {scope}.{name} estimate does not match its published family deltas"
            )));
        }
        match (expected_families.len() >= 2, &metric.interval) {
            (true, Some(interval)) => {
                let expected_interval = cluster_bootstrap(&metric.family_deltas, bootstrap)?;
                if *interval != expected_interval {
                    return Err(EvalError::InvalidInput(format!(
                        "paired {scope}.{name} confidence interval does not reproduce from its family deltas"
                    )));
                }
            }
            (false, None) => {}
            _ => {
                return Err(EvalError::InvalidInput(format!(
                    "paired {scope}.{name} confidence interval presence does not match family coverage"
                )));
            }
        }
    }
    Ok(())
}

pub fn compare_paired_runs(
    incumbent: &PairedRunArtifact,
    candidate: &PairedRunArtifact,
    plan: &PairedEvaluationPlan,
) -> Result<PairedRunComparison, EvalError> {
    plan.validate()?;
    incumbent.validate()?;
    candidate.validate()?;
    if incumbent.arm != RunArm::Incumbent || candidate.arm != RunArm::Candidate {
        return Err(EvalError::InvalidInput(
            "paired comparator requires incumbent and candidate artifacts in their named positions"
                .into(),
        ));
    }
    if incumbent.identity.shared != plan.shared_identity
        || candidate.identity.shared != plan.shared_identity
        || incumbent.identity.product != plan.incumbent_product
        || candidate.identity.product != plan.candidate_product
    {
        return Err(EvalError::InvalidInput(
            "paired artifacts do not match the agent or product identities preregistered in the plan".into(),
        ));
    }
    validate_pair(
        &incumbent.identity,
        &candidate.identity,
        plan.declared_tool_schema_change.as_ref(),
    )?;
    if incumbent.run_id != candidate.run_id {
        return Err(EvalError::InvalidInput(
            "paired artifacts must share one runner-generated invocation identity".into(),
        ));
    }
    if incumbent.run_started_at != candidate.run_started_at
        || incumbent.run_completed_at != candidate.run_completed_at
    {
        return Err(EvalError::InvalidInput(
            "paired artifacts must record one shared invocation time window".into(),
        ));
    }
    if plan.stage == PairedEvaluationStage::DevelopmentPilot && incumbent.run_id.is_none() {
        return Err(EvalError::InvalidInput(
            "development pilot artifacts require runner-generated invocation identities".into(),
        ));
    }
    let incumbent_keys: Vec<_> = incumbent
        .receipts
        .iter()
        .map(|receipt| receipt.key.clone())
        .collect();
    let candidate_keys: Vec<_> = candidate
        .receipts
        .iter()
        .map(|receipt| receipt.key.clone())
        .collect();
    validate_case_coverage(
        &plan.expected_trials,
        &incumbent.expected_trials,
        &candidate.expected_trials,
    )?;
    validate_case_coverage(&plan.expected_trials, &incumbent_keys, &candidate_keys)?;

    let incumbent_receipts: BTreeMap<_, _> = incumbent
        .receipts
        .iter()
        .map(|receipt| (receipt.key.clone(), receipt))
        .collect();
    let candidate_receipts: BTreeMap<_, _> = candidate
        .receipts
        .iter()
        .map(|receipt| (receipt.key.clone(), receipt))
        .collect();
    let mut arm_order_counts = BTreeMap::<String, ArmOrderCounts>::new();
    let mut deltas = Vec::with_capacity(plan.expected_trials.len());
    let mut hard_violation_count = 0_u64;
    let mut budget_exceeded_trial_count = 0_u64;

    for key in &plan.expected_trials {
        let incumbent_receipt = incumbent_receipts.get(key).ok_or_else(|| {
            EvalError::InvalidInput(format!(
                "incumbent lacks preregistered trial {}",
                format_case_trial_key(key)
            ))
        })?;
        let candidate_receipt = candidate_receipts.get(key).ok_or_else(|| {
            EvalError::InvalidInput(format!(
                "candidate lacks preregistered trial {}",
                format_case_trial_key(key)
            ))
        })?;
        if incumbent_receipt.arm_order == candidate_receipt.arm_order {
            return Err(EvalError::InvalidInput(format!(
                "paired trial {} must run the two arms in opposite order positions",
                format_case_trial_key(key)
            )));
        }
        if incumbent_receipt.known_revision_checkpoint
            != candidate_receipt.known_revision_checkpoint
        {
            return Err(EvalError::InvalidInput(format!(
                "paired trial {} used different knowledge-revision checkpoints",
                format_case_trial_key(key)
            )));
        }
        let counts = arm_order_counts
            .entry(key.stratum_id.clone())
            .or_insert(ArmOrderCounts {
                incumbent_first: 0,
                candidate_first: 0,
            });
        if incumbent_receipt.arm_order == 1 {
            counts.incumbent_first += 1;
        } else {
            counts.candidate_first += 1;
        }
        hard_violation_count += incumbent_receipt.hard_violations.len() as u64;
        hard_violation_count += candidate_receipt.hard_violations.len() as u64;
        budget_exceeded_trial_count +=
            u64::from(incumbent_receipt.status == TrialStatus::BudgetExceeded);
        budget_exceeded_trial_count +=
            u64::from(candidate_receipt.status == TrialStatus::BudgetExceeded);

        let shared = &plan.shared_identity;
        deltas.push(PairedTrialDelta {
            key: key.clone(),
            direct_memory_success: bool_delta(
                incumbent_receipt.direct_memory_success,
                candidate_receipt.direct_memory_success,
            ),
            task_success: bool_delta(
                incumbent_receipt.task_success(),
                candidate_receipt.task_success(),
            ),
            repair_turn_gain: f64::from(incumbent_receipt.charged_repair_turns(shared.turn_budget))
                - f64::from(candidate_receipt.charged_repair_turns(shared.turn_budget)),
            turn_gain: f64::from(incumbent_receipt.charged_turns(shared.turn_budget))
                - f64::from(candidate_receipt.charged_turns(shared.turn_budget)),
            tool_call_gain: f64::from(incumbent_receipt.tool_calls)
                - f64::from(candidate_receipt.tool_calls),
            clarification_gain: f64::from(incumbent_receipt.user_clarifications)
                - f64::from(candidate_receipt.user_clarifications),
            input_token_gain: incumbent_receipt.input_tokens as f64
                - candidate_receipt.input_tokens as f64,
            output_token_gain: incumbent_receipt.output_tokens as f64
                - candidate_receipt.output_tokens as f64,
            wall_time_gain_ms: incumbent_receipt.wall_time_ms as f64
                - candidate_receipt.wall_time_ms as f64,
        });
    }
    for (stratum, counts) in &arm_order_counts {
        if counts.incumbent_first.abs_diff(counts.candidate_first) > 1 {
            return Err(EvalError::InvalidInput(format!(
                "paired execution order is not balanced within stratum {stratum:?}"
            )));
        }
    }

    let overall = summarize_metric_set(&deltas, &plan.bootstrap)?;
    let strata: BTreeSet<_> = deltas
        .iter()
        .map(|delta| delta.key.stratum_id.clone())
        .collect();
    let mut by_stratum = BTreeMap::new();
    for stratum in strata {
        let stratum_deltas: Vec<_> = deltas
            .iter()
            .filter(|delta| delta.key.stratum_id == stratum)
            .cloned()
            .collect();
        by_stratum.insert(
            stratum,
            summarize_metric_set(&stratum_deltas, &plan.bootstrap)?,
        );
    }
    let independent_family_count = u32::try_from(
        deltas
            .iter()
            .map(|delta| delta.key.family_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
    )
    .map_err(|_| EvalError::InvalidInput("too many independent history families".into()))?;
    let comparison = PairedRunComparison {
        schema_version: PAIRED_COMPARISON_SCHEMA_V2.into(),
        run_id: incumbent.run_id.clone(),
        run_started_at: incumbent.run_started_at,
        run_completed_at: incumbent.run_completed_at,
        plan_sha256: plan.sha256()?,
        manifest_sha256: expected_manifest_sha256(&plan.expected_trials)?,
        incumbent_artifact_sha256: incumbent.sha256()?,
        candidate_artifact_sha256: candidate.sha256()?,
        plan: plan.clone(),
        overall,
        by_stratum,
        paired_trial_count: deltas.len() as u64,
        independent_family_count,
        hard_violation_count,
        budget_exceeded_trial_count,
        arm_order_counts,
    };
    comparison.validate()?;
    Ok(comparison)
}

pub fn summarize_development_pilot(
    plan: &PairedEvaluationPlan,
    first_run_incumbent: &PairedRunArtifact,
    first_run_candidate: &PairedRunArtifact,
    second_run_incumbent: &PairedRunArtifact,
    second_run_candidate: &PairedRunArtifact,
) -> Result<DevelopmentPilotReport, EvalError> {
    if plan.stage != PairedEvaluationStage::DevelopmentPilot {
        return Err(EvalError::InvalidInput(
            "pilot summary requires a development_pilot plan".into(),
        ));
    }
    let first_run = compare_paired_runs(first_run_incumbent, first_run_candidate, plan)?;
    let second_run = compare_paired_runs(second_run_incumbent, second_run_candidate, plan)?;
    let independent_family_count = first_run.independent_family_count;
    let paired_trial_count_per_run = first_run.paired_trial_count;
    let metrics = pilot_metric_summaries(&first_run, &second_run)?;
    let first_outcomes =
        pilot_outcome_family_means(first_run_incumbent, first_run_candidate, plan)?;
    let second_outcomes =
        pilot_outcome_family_means(second_run_incumbent, second_run_candidate, plan)?;
    let same_product_outcomes = pilot_outcome_summaries(&first_outcomes, &second_outcomes)?;
    let report = DevelopmentPilotReport {
        schema_version: PAIRED_PILOT_REPORT_SCHEMA_V3.into(),
        plan_sha256: plan.sha256()?,
        manifest_sha256: expected_manifest_sha256(&plan.expected_trials)?,
        independent_family_count,
        paired_trial_count_per_run,
        first_run,
        second_run,
        metrics,
        same_product_outcomes,
    };
    report.validate()?;
    Ok(report)
}

pub fn summarize_exploratory_pilot(
    plan: &PairedEvaluationPlan,
    first_run_incumbent: &PairedRunArtifact,
    first_run_candidate: &PairedRunArtifact,
    second_run_incumbent: &PairedRunArtifact,
    second_run_candidate: &PairedRunArtifact,
) -> Result<ExploratoryPilotReport, EvalError> {
    if plan.stage != PairedEvaluationStage::ExploratoryComparison {
        return Err(EvalError::InvalidInput(
            "exploratory pilot summary requires an exploratory_comparison plan".into(),
        ));
    }
    let first_run = compare_paired_runs(first_run_incumbent, first_run_candidate, plan)?;
    let second_run = compare_paired_runs(second_run_incumbent, second_run_candidate, plan)?;
    let incumbent_outcomes = pilot_outcome_summaries(
        &pilot_arm_outcome_family_means(first_run_incumbent, plan)?,
        &pilot_arm_outcome_family_means(second_run_incumbent, plan)?,
    )?;
    let candidate_outcomes = pilot_outcome_summaries(
        &pilot_arm_outcome_family_means(first_run_candidate, plan)?,
        &pilot_arm_outcome_family_means(second_run_candidate, plan)?,
    )?;
    let report = ExploratoryPilotReport {
        schema_version: PAIRED_EXPLORATORY_PILOT_REPORT_SCHEMA_V1.into(),
        plan_sha256: plan.sha256()?,
        manifest_sha256: expected_manifest_sha256(&plan.expected_trials)?,
        independent_family_count: first_run.independent_family_count,
        paired_trial_count_per_run: first_run.paired_trial_count,
        metrics: pilot_metric_summaries(&first_run, &second_run)?,
        first_run,
        second_run,
        incumbent_outcomes,
        candidate_outcomes,
    };
    report.validate()?;
    Ok(report)
}

fn validate_pilot_outcome_summaries(
    summaries: &BTreeMap<PilotOutcome, PilotOutcomeSummary>,
    first_run: &PairedRunComparison,
    second_run: &PairedRunComparison,
    arm_name: &str,
) -> Result<(), EvalError> {
    if summaries.len() != PILOT_OUTCOMES.len() {
        return Err(EvalError::InvalidInput(format!(
            "{arm_name} pilot outcomes must include every registered metric"
        )));
    }
    let first_outcomes = summaries
        .iter()
        .map(|(outcome, summary)| (*outcome, summary.first_run_family_means.clone()))
        .collect::<BTreeMap<_, _>>();
    let second_outcomes = summaries
        .iter()
        .map(|(outcome, summary)| (*outcome, summary.second_run_family_means.clone()))
        .collect::<BTreeMap<_, _>>();
    let expected_families = first_run
        .overall
        .task_success
        .family_deltas
        .keys()
        .collect::<BTreeSet<_>>();
    if summaries.values().any(|summary| {
        summary
            .first_run_family_means
            .keys()
            .collect::<BTreeSet<_>>()
            != expected_families
            || summary
                .second_run_family_means
                .keys()
                .collect::<BTreeSet<_>>()
                != expected_families
    }) {
        return Err(EvalError::InvalidInput(format!(
            "{arm_name} pilot outcome summaries do not match independent family coverage"
        )));
    }
    if summaries != &pilot_outcome_summaries(&first_outcomes, &second_outcomes)? {
        return Err(EvalError::InvalidInput(format!(
            "{arm_name} pilot outcome summaries do not reproduce from per-run family means"
        )));
    }
    if second_run.independent_family_count != first_run.independent_family_count {
        return Err(EvalError::InvalidInput(
            "paired pilot runs have different independent family coverage".into(),
        ));
    }
    Ok(())
}

fn pilot_metric_summaries(
    first_run: &PairedRunComparison,
    second_run: &PairedRunComparison,
) -> Result<BTreeMap<PilotMetric, PilotMetricSummary>, EvalError> {
    let metric_pairs = [
        (
            PilotMetric::DirectMemorySuccess,
            &first_run.overall.direct_memory_success,
            &second_run.overall.direct_memory_success,
        ),
        (
            PilotMetric::TaskSuccess,
            &first_run.overall.task_success,
            &second_run.overall.task_success,
        ),
        (
            PilotMetric::RepairTurnGain,
            &first_run.overall.repair_turn_gain,
            &second_run.overall.repair_turn_gain,
        ),
        (
            PilotMetric::TurnGain,
            &first_run.overall.turn_gain,
            &second_run.overall.turn_gain,
        ),
        (
            PilotMetric::ToolCallGain,
            &first_run.overall.tool_call_gain,
            &second_run.overall.tool_call_gain,
        ),
        (
            PilotMetric::ClarificationGain,
            &first_run.overall.clarification_gain,
            &second_run.overall.clarification_gain,
        ),
        (
            PilotMetric::InputTokenGain,
            &first_run.overall.input_token_gain,
            &second_run.overall.input_token_gain,
        ),
        (
            PilotMetric::OutputTokenGain,
            &first_run.overall.output_token_gain,
            &second_run.overall.output_token_gain,
        ),
        (
            PilotMetric::WallTimeGainMs,
            &first_run.overall.wall_time_gain_ms,
            &second_run.overall.wall_time_gain_ms,
        ),
    ];
    let mut summaries = BTreeMap::new();
    for (metric, first, second) in metric_pairs {
        if first.family_deltas.keys().ne(second.family_deltas.keys()) {
            return Err(EvalError::InvalidInput(
                "pilot repeats have different independent family coverage".into(),
            ));
        }
        let family_deltas = first
            .family_deltas
            .iter()
            .map(|(family, first_delta)| {
                let second_delta = second.family_deltas.get(family).copied().ok_or_else(|| {
                    EvalError::InvalidInput(
                        "pilot repeats have different independent family coverage".into(),
                    )
                })?;
                Ok((family.clone(), (first_delta + second_delta) / 2.0))
            })
            .collect::<Result<BTreeMap<_, _>, EvalError>>()?;
        let between_family_sample_variance = sample_variance(family_deltas.values().copied())?;
        summaries.insert(
            metric,
            PilotMetricSummary {
                first_run_family_deltas: first.family_deltas.clone(),
                second_run_family_deltas: second.family_deltas.clone(),
                family_deltas,
                between_family_sample_variance,
            },
        );
    }
    Ok(summaries)
}

const PILOT_OUTCOMES: [PilotOutcome; 9] = [
    PilotOutcome::DirectMemorySuccess,
    PilotOutcome::TaskSuccess,
    PilotOutcome::ChargedRepairTurns,
    PilotOutcome::ChargedTurns,
    PilotOutcome::ToolCalls,
    PilotOutcome::UserClarifications,
    PilotOutcome::InputTokens,
    PilotOutcome::OutputTokens,
    PilotOutcome::WallTimeMs,
];

fn pilot_outcome_family_means(
    incumbent: &PairedRunArtifact,
    candidate: &PairedRunArtifact,
    plan: &PairedEvaluationPlan,
) -> Result<BTreeMap<PilotOutcome, BTreeMap<String, f64>>, EvalError> {
    let incumbent_means = pilot_arm_outcome_family_means(incumbent, plan)?;
    let candidate_means = pilot_arm_outcome_family_means(candidate, plan)?;
    PILOT_OUTCOMES
        .into_iter()
        .map(|outcome| {
            let incumbent_families = incumbent_means.get(&outcome).ok_or_else(|| {
                EvalError::InvalidInput("incumbent pilot outcome is missing".into())
            })?;
            let candidate_families = candidate_means.get(&outcome).ok_or_else(|| {
                EvalError::InvalidInput("candidate pilot outcome is missing".into())
            })?;
            if incumbent_families.keys().ne(candidate_families.keys()) {
                return Err(EvalError::InvalidInput(
                    "pilot outcome arms have different family coverage".into(),
                ));
            }
            let means = incumbent_families
                .iter()
                .map(|(family, incumbent_mean)| {
                    let candidate_mean = candidate_families.get(family).ok_or_else(|| {
                        EvalError::InvalidInput(
                            "candidate pilot outcome lacks incumbent family".into(),
                        )
                    })?;
                    Ok((family.clone(), (incumbent_mean + candidate_mean) / 2.0))
                })
                .collect::<Result<BTreeMap<_, _>, EvalError>>()?;
            Ok((outcome, means))
        })
        .collect()
}

fn pilot_arm_outcome_family_means(
    artifact: &PairedRunArtifact,
    plan: &PairedEvaluationPlan,
) -> Result<BTreeMap<PilotOutcome, BTreeMap<String, f64>>, EvalError> {
    let receipts = artifact
        .receipts
        .iter()
        .map(|receipt| (&receipt.key, receipt))
        .collect::<BTreeMap<_, _>>();
    let mut family_trials = PILOT_OUTCOMES
        .into_iter()
        .map(|outcome| (outcome, BTreeMap::<String, Vec<f64>>::new()))
        .collect::<BTreeMap<_, _>>();

    for key in &plan.expected_trials {
        let receipt = receipts.get(key).ok_or_else(|| {
            EvalError::InvalidInput(format!(
                "paired pilot arm lacks preregistered trial {}",
                format_case_trial_key(key)
            ))
        })?;
        for outcome in PILOT_OUTCOMES {
            family_trials
                .get_mut(&outcome)
                .expect("all pilot outcomes are initialized")
                .entry(key.family_id.clone())
                .or_default()
                .push(pilot_outcome_value(
                    outcome,
                    receipt,
                    plan.shared_identity.turn_budget,
                ));
        }
    }

    family_trials
        .into_iter()
        .map(|(outcome, families)| {
            let means = families
                .into_iter()
                .map(|(family, values)| {
                    if values.is_empty() || values.iter().any(|value| !value.is_finite()) {
                        return Err(EvalError::InvalidInput(
                            "pilot outcome has an empty or non-finite family".into(),
                        ));
                    }
                    Ok((family, average_iter(values.iter().copied())))
                })
                .collect::<Result<BTreeMap<_, _>, EvalError>>()?;
            Ok((outcome, means))
        })
        .collect()
}

fn pilot_outcome_value(outcome: PilotOutcome, receipt: &TrialReceipt, turn_budget: u32) -> f64 {
    match outcome {
        PilotOutcome::DirectMemorySuccess => f64::from(u8::from(receipt.direct_memory_success)),
        PilotOutcome::TaskSuccess => f64::from(u8::from(receipt.task_success())),
        PilotOutcome::ChargedRepairTurns => f64::from(receipt.charged_repair_turns(turn_budget)),
        PilotOutcome::ChargedTurns => f64::from(receipt.charged_turns(turn_budget)),
        PilotOutcome::ToolCalls => f64::from(receipt.tool_calls),
        PilotOutcome::UserClarifications => f64::from(receipt.user_clarifications),
        PilotOutcome::InputTokens => receipt.input_tokens as f64,
        PilotOutcome::OutputTokens => receipt.output_tokens as f64,
        PilotOutcome::WallTimeMs => receipt.wall_time_ms as f64,
    }
}

fn pilot_outcome_summaries(
    first_run: &BTreeMap<PilotOutcome, BTreeMap<String, f64>>,
    second_run: &BTreeMap<PilotOutcome, BTreeMap<String, f64>>,
) -> Result<BTreeMap<PilotOutcome, PilotOutcomeSummary>, EvalError> {
    let expected_outcomes = PILOT_OUTCOMES.into_iter().collect::<BTreeSet<_>>();
    if first_run.keys().copied().collect::<BTreeSet<_>>() != expected_outcomes
        || second_run.keys().copied().collect::<BTreeSet<_>>() != expected_outcomes
    {
        return Err(EvalError::InvalidInput(
            "pilot outcome summaries do not cover the required metrics".into(),
        ));
    }

    let mut summaries = BTreeMap::new();
    for outcome in PILOT_OUTCOMES {
        let first_family_means = &first_run[&outcome];
        let second_family_means = &second_run[&outcome];
        if first_family_means.keys().ne(second_family_means.keys())
            || first_family_means.len() < 2
            || first_family_means
                .values()
                .chain(second_family_means.values())
                .any(|value| !value.is_finite())
        {
            return Err(EvalError::InvalidInput(
                "pilot repeats require matching finite independent family outcomes".into(),
            ));
        }
        let family_means = first_family_means
            .iter()
            .map(|(family, first_value)| {
                let second_value = second_family_means.get(family).ok_or_else(|| {
                    EvalError::InvalidInput(
                        "pilot repeats have different independent family coverage".into(),
                    )
                })?;
                Ok((family.clone(), (first_value + second_value) / 2.0))
            })
            .collect::<Result<BTreeMap<_, _>, EvalError>>()?;
        let retest_family_deltas = first_family_means
            .iter()
            .map(|(family, first_value)| {
                let second_value = second_family_means.get(family).ok_or_else(|| {
                    EvalError::InvalidInput(
                        "pilot repeats have different independent family coverage".into(),
                    )
                })?;
                Ok((family.clone(), second_value - first_value))
            })
            .collect::<Result<BTreeMap<_, _>, EvalError>>()?;
        summaries.insert(
            outcome,
            PilotOutcomeSummary {
                first_run_family_means: first_family_means.clone(),
                second_run_family_means: second_family_means.clone(),
                between_family_sample_variance: sample_variance(family_means.values().copied())?,
                test_retest_sample_variance: sample_variance(
                    retest_family_deltas.values().copied(),
                )?,
                family_means,
                retest_family_deltas,
            },
        );
    }
    Ok(summaries)
}

fn sample_variance(values: impl Iterator<Item = f64>) -> Result<f64, EvalError> {
    let values = values.collect::<Vec<_>>();
    if values.len() < 2 || values.iter().any(|value| !value.is_finite()) {
        return Err(EvalError::InvalidInput(
            "pilot variance requires at least two finite independent family means".into(),
        ));
    }
    let mean = average_iter(values.iter().copied());
    let sum_squared_deviations = values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>();
    let degrees_of_freedom = u32::try_from(values.len() - 1).map_err(|_| {
        EvalError::InvalidInput("pilot contains too many independent history families".into())
    })?;
    Ok(sum_squared_deviations / f64::from(degrees_of_freedom))
}

pub fn evaluate_promotion(
    comparison: &PairedRunComparison,
    criteria: &PromotionCriteria,
) -> Result<PromotionEvaluation, EvalError> {
    comparison.validate()?;
    if comparison.plan.stage != PairedEvaluationStage::Confirmatory {
        return Err(EvalError::InvalidInput(
            "development pilot results cannot be used for promotion decisions".into(),
        ));
    }
    validate_promotion_criteria(criteria)?;
    if comparison.plan.promotion_criteria.as_ref() != Some(criteria) {
        return Err(EvalError::InvalidInput(
            "promotion criteria differ from the preregistered evaluation plan".into(),
        ));
    }

    let mut checks = vec![
        positive_gain_check(
            "direct_memory_success",
            &comparison.overall.direct_memory_success,
            criteria.direct_memory_margin,
        ),
        positive_gain_check(
            "task_success",
            &comparison.overall.task_success,
            criteria.task_success_margin,
        ),
        positive_gain_check(
            "repair_turn_gain",
            &comparison.overall.repair_turn_gain,
            criteria.repair_turn_gain_margin,
        ),
        hard_count_check("hard_violations", comparison.hard_violation_count),
        hard_count_check(
            "budget_exceeded_trials",
            comparison.budget_exceeded_trial_count,
        ),
    ];
    checks.extend(resource_non_inferiority_checks(
        "overall",
        &comparison.overall,
        &criteria.resource_non_inferiority,
    ));
    for (stratum, epsilon) in &criteria.task_non_inferiority_by_stratum {
        let check = comparison
            .by_stratum
            .get(stratum)
            .map(|metrics| {
                non_inferiority_check(
                    &format!("task_success_non_inferiority::{stratum}"),
                    &metrics.task_success,
                    -*epsilon,
                )
            })
            .unwrap_or_else(|| PromotionCheck {
                name: format!("task_success_non_inferiority::{stratum}"),
                status: PromotionCheckStatus::InsufficientEvidence,
                threshold: -*epsilon,
                lower: None,
                upper: None,
            });
        checks.push(check);
    }
    for (stratum, limits) in &criteria.resource_non_inferiority_by_stratum {
        let metrics = comparison.by_stratum.get(stratum).ok_or_else(|| {
            EvalError::InvalidInput(format!(
                "comparison lacks preregistered ordinary-workflow stratum {stratum:?}"
            ))
        })?;
        checks.extend(resource_non_inferiority_checks(stratum, metrics, limits));
    }
    let decision = if checks
        .iter()
        .any(|check| check.status == PromotionCheckStatus::Failed)
    {
        PromotionDecision::Reject
    } else if checks
        .iter()
        .any(|check| check.status == PromotionCheckStatus::InsufficientEvidence)
    {
        PromotionDecision::InsufficientEvidence
    } else {
        PromotionDecision::Promote
    };
    Ok(PromotionEvaluation { decision, checks })
}

#[derive(Debug, Clone)]
struct PairedTrialDelta {
    key: CaseTrialKey,
    direct_memory_success: f64,
    task_success: f64,
    repair_turn_gain: f64,
    turn_gain: f64,
    tool_call_gain: f64,
    clarification_gain: f64,
    input_token_gain: f64,
    output_token_gain: f64,
    wall_time_gain_ms: f64,
}

fn summarize_metric_set(
    trials: &[PairedTrialDelta],
    bootstrap: &BootstrapConfig,
) -> Result<PairedMetricSet, EvalError> {
    Ok(PairedMetricSet {
        direct_memory_success: summarize_metric(
            trials,
            |trial| trial.direct_memory_success,
            bootstrap,
        )?,
        task_success: summarize_metric(trials, |trial| trial.task_success, bootstrap)?,
        repair_turn_gain: summarize_metric(trials, |trial| trial.repair_turn_gain, bootstrap)?,
        turn_gain: summarize_metric(trials, |trial| trial.turn_gain, bootstrap)?,
        tool_call_gain: summarize_metric(trials, |trial| trial.tool_call_gain, bootstrap)?,
        clarification_gain: summarize_metric(trials, |trial| trial.clarification_gain, bootstrap)?,
        input_token_gain: summarize_metric(trials, |trial| trial.input_token_gain, bootstrap)?,
        output_token_gain: summarize_metric(trials, |trial| trial.output_token_gain, bootstrap)?,
        wall_time_gain_ms: summarize_metric(trials, |trial| trial.wall_time_gain_ms, bootstrap)?,
    })
}

fn summarize_metric(
    trials: &[PairedTrialDelta],
    get_delta: impl Fn(&PairedTrialDelta) -> f64,
    bootstrap: &BootstrapConfig,
) -> Result<MetricEstimate, EvalError> {
    let mut family_case_trials = BTreeMap::<String, BTreeMap<String, Vec<f64>>>::new();
    for trial in trials {
        family_case_trials
            .entry(trial.key.family_id.clone())
            .or_default()
            .entry(trial.key.case_id.clone())
            .or_default()
            .push(get_delta(trial));
    }
    let family_deltas: BTreeMap<_, _> = family_case_trials
        .into_iter()
        .map(|(family, cases)| {
            let case_means = cases.values().map(|repetitions| average(repetitions));
            (family, average_iter(case_means))
        })
        .collect();
    let estimate = average_iter(family_deltas.values().copied());
    let interval = if family_deltas.len() >= 2 {
        Some(cluster_bootstrap(&family_deltas, bootstrap)?)
    } else {
        None
    };
    Ok(MetricEstimate {
        estimate,
        interval,
        family_deltas,
    })
}

fn average(values: &[f64]) -> f64 {
    average_iter(values.iter().copied())
}

fn average_iter(values: impl Iterator<Item = f64>) -> f64 {
    let (sum, count) = values.fold((0.0, 0_u64), |(sum, count), value| (sum + value, count + 1));
    if count == 0 { 0.0 } else { sum / count as f64 }
}

fn bool_delta(incumbent: bool, candidate: bool) -> f64 {
    f64::from(u8::from(candidate)) - f64::from(u8::from(incumbent))
}

fn positive_gain_check(name: &str, metric: &MetricEstimate, threshold: f64) -> PromotionCheck {
    let Some(interval) = &metric.interval else {
        return PromotionCheck {
            name: name.into(),
            status: PromotionCheckStatus::InsufficientEvidence,
            threshold,
            lower: None,
            upper: None,
        };
    };
    let status = if interval.lower > threshold {
        PromotionCheckStatus::Passed
    } else if interval.upper <= threshold {
        PromotionCheckStatus::Failed
    } else {
        PromotionCheckStatus::InsufficientEvidence
    };
    PromotionCheck {
        name: name.into(),
        status,
        threshold,
        lower: Some(interval.lower),
        upper: Some(interval.upper),
    }
}

fn non_inferiority_check(name: &str, metric: &MetricEstimate, threshold: f64) -> PromotionCheck {
    let Some(interval) = &metric.interval else {
        return PromotionCheck {
            name: name.into(),
            status: PromotionCheckStatus::InsufficientEvidence,
            threshold,
            lower: None,
            upper: None,
        };
    };
    let status = if interval.lower >= threshold {
        PromotionCheckStatus::Passed
    } else if interval.upper < threshold {
        PromotionCheckStatus::Failed
    } else {
        PromotionCheckStatus::InsufficientEvidence
    };
    PromotionCheck {
        name: name.into(),
        status,
        threshold,
        lower: Some(interval.lower),
        upper: Some(interval.upper),
    }
}

fn resource_non_inferiority_checks(
    scope: &str,
    metrics: &PairedMetricSet,
    limits: &ResourceNonInferiority,
) -> Vec<PromotionCheck> {
    [
        ("turns", &metrics.turn_gain, limits.turns),
        ("tool_calls", &metrics.tool_call_gain, limits.tool_calls),
        (
            "user_clarifications",
            &metrics.clarification_gain,
            limits.user_clarifications,
        ),
        (
            "input_tokens",
            &metrics.input_token_gain,
            limits.input_tokens,
        ),
        (
            "output_tokens",
            &metrics.output_token_gain,
            limits.output_tokens,
        ),
        (
            "wall_time_ms",
            &metrics.wall_time_gain_ms,
            limits.wall_time_ms,
        ),
    ]
    .into_iter()
    .map(|(metric_name, metric, allowed_regression)| {
        non_inferiority_check(
            &format!("resource_non_inferiority::{scope}::{metric_name}"),
            metric,
            -allowed_regression,
        )
    })
    .collect()
}

fn hard_count_check(name: &str, count: u64) -> PromotionCheck {
    PromotionCheck {
        name: name.into(),
        status: if count == 0 {
            PromotionCheckStatus::Passed
        } else {
            PromotionCheckStatus::Failed
        },
        threshold: 0.0,
        lower: Some(count as f64),
        upper: Some(count as f64),
    }
}

fn validate_promotion_criteria(criteria: &PromotionCriteria) -> Result<(), EvalError> {
    for (name, margin) in [
        ("direct_memory_margin", criteria.direct_memory_margin),
        ("task_success_margin", criteria.task_success_margin),
        ("repair_turn_gain_margin", criteria.repair_turn_gain_margin),
    ] {
        if !margin.is_finite() || margin <= 0.0 {
            return Err(EvalError::InvalidInput(format!(
                "promotion criterion {name} must be finite and positive"
            )));
        }
    }
    if criteria.task_non_inferiority_by_stratum.is_empty()
        || criteria.ordinary_workflow_strata.is_empty()
        || criteria.ordinary_workflow_strata.iter().any(|stratum| {
            !criteria
                .task_non_inferiority_by_stratum
                .contains_key(stratum)
        })
        || criteria
            .task_non_inferiority_by_stratum
            .iter()
            .any(|(stratum, epsilon)| {
                stratum.trim().is_empty() || !epsilon.is_finite() || *epsilon < 0.0
            })
        || criteria
            .resource_non_inferiority_by_stratum
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>()
            != criteria.ordinary_workflow_strata
    {
        return Err(EvalError::InvalidInput(
            "promotion requires named task non-inferiority strata with finite non-negative margins"
                .into(),
        ));
    }
    validate_resource_non_inferiority(
        "resource_non_inferiority",
        &criteria.resource_non_inferiority,
    )?;
    for (stratum, limits) in &criteria.resource_non_inferiority_by_stratum {
        validate_resource_non_inferiority(
            &format!("resource_non_inferiority_by_stratum::{stratum}"),
            limits,
        )?;
    }
    Ok(())
}

fn validate_resource_non_inferiority(
    name: &str,
    limits: &ResourceNonInferiority,
) -> Result<(), EvalError> {
    for (metric, margin) in [
        ("turns", limits.turns),
        ("tool_calls", limits.tool_calls),
        ("user_clarifications", limits.user_clarifications),
        ("input_tokens", limits.input_tokens),
        ("output_tokens", limits.output_tokens),
        ("wall_time_ms", limits.wall_time_ms),
    ] {
        if !margin.is_finite() || margin < 0.0 {
            return Err(EvalError::InvalidInput(format!(
                "promotion criterion {name}.{metric} must be finite and non-negative"
            )));
        }
    }
    Ok(())
}

fn validate_bootstrap_config(config: &BootstrapConfig) -> Result<(), EvalError> {
    if !(1_000..=1_000_000).contains(&config.draws) {
        return Err(EvalError::InvalidInput(
            "paired bootstrap requires between 1000 and 1000000 draws".into(),
        ));
    }
    if !(5_000..=9_999).contains(&config.confidence_level_basis_points) {
        return Err(EvalError::InvalidInput(
            "bootstrap confidence level must be between 50.00% and 99.99%".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(model: &str) -> PairedRunIdentity {
        PairedRunIdentity {
            shared: SharedRunIdentity {
                agent_runner: "codex-cli".into(),
                agent_version: "0.169.0".into(),
                model: model.into(),
                model_identity_protocol: ModelIdentityProtocol::ProviderRevision {
                    revision: "fixed-revision".into(),
                },
                system_prompt_sha256: "a".repeat(64),
                task_prompt_sha256: "9".repeat(64),
                host_policy_sha256: "b".repeat(64),
                transport: "stdio".into(),
                retry_policy_sha256: "c".repeat(64),
                reader_config_sha256: "8".repeat(64),
                judge_config_sha256: None,
                corpus_sha256: "d".repeat(64),
                oracle_sha256: "e".repeat(64),
                clock_id: "fixture-clock-v1".into(),
                simulator_sha256: "f".repeat(64),
                provider_fingerprints: BTreeMap::from([("agent".into(), "7".repeat(64))]),
                context_budget_tokens: 4096,
                tool_call_budget: 12,
                turn_budget: 20,
                output_token_budget: 1024,
                wall_time_budget_ms: 60_000,
                hardware_class: "local-arm64".into(),
            },
            product: ProductRunIdentity {
                revision: "product-revision".into(),
                binary_sha256: "1".repeat(64),
                product_config_sha256: "6".repeat(64),
                tool_schema_sha256: "2".repeat(64),
                enabled_features: BTreeSet::new(),
                claim_rollout_stage: None,
            },
        }
    }

    #[test]
    fn paired_run_rejects_a_different_agent_model() {
        let incumbent = identity("agent-model-a");
        let candidate = identity("agent-model-b");

        assert!(validate_pair(&incumbent, &candidate, None).is_err());
    }

    #[test]
    fn legacy_product_identity_roundtrips_without_a_rollout_stage_field() {
        let expected = identity("fixed-model");
        let serialized = serde_json::to_value(&expected).expect("serialize legacy identity");

        assert!(serialized["product"].get("claim_rollout_stage").is_none());
        let decoded: PairedRunIdentity =
            serde_json::from_value(serialized.clone()).expect("read legacy identity");
        assert_eq!(decoded, expected);
        assert_eq!(
            serde_json::to_value(decoded).expect("serialize decoded identity"),
            serialized
        );
    }

    #[test]
    fn paired_artifact_rejects_a_same_alias_run_outside_its_preregistered_window() {
        let mut artifact = comparison_fixture(RunArm::Incumbent, true, true);
        artifact.run_id = Some("runner-invocation".into());
        artifact.identity.shared.model_identity_protocol =
            ModelIdentityProtocol::SameAliasWithinInvocation {
                max_duration_seconds: 3600,
            };
        artifact.run_completed_at = artifact.run_started_at + chrono::Duration::seconds(3601);

        assert!(
            artifact
                .validate()
                .expect_err("overlong alias window is rejected")
                .to_string()
                .contains("same-alias window")
        );
    }

    #[test]
    fn paired_comparison_rejects_artifacts_with_different_invocation_windows() {
        let incumbent = comparison_fixture(RunArm::Incumbent, true, true);
        let mut candidate = comparison_fixture(RunArm::Candidate, true, true);
        candidate.run_completed_at += chrono::Duration::seconds(1);
        let plan = comparison_plan(
            &incumbent,
            &candidate,
            bootstrap_config(71),
            promotion_criteria(),
        );

        assert!(
            compare_paired_runs(&incumbent, &candidate, &plan)
                .expect_err("mismatched windows are rejected")
                .to_string()
                .contains("shared invocation time window")
        );
    }

    #[test]
    fn shared_identity_can_pin_a_requested_alias_without_claiming_a_provider_revision() {
        let mut value = serde_json::to_value(identity("codex-model-alias"))
            .expect("serialize identity fixture");
        let shared = value
            .get_mut("shared")
            .and_then(serde_json::Value::as_object_mut)
            .expect("shared identity object");
        shared.remove("model_revision");
        shared.insert(
            "model_identity_protocol".into(),
            serde_json::json!({
                "method": "same_alias_within_invocation",
                "max_duration_seconds": 3600
            }),
        );

        let parsed: PairedRunIdentity =
            serde_json::from_value(value).expect("decode alias protocol");
        assert_eq!(parsed.shared.model, "codex-model-alias");
        assert_eq!(
            serde_json::to_value(&parsed).expect("serialize parsed identity")["shared"]["model_identity_protocol"]
                ["method"],
            "same_alias_within_invocation"
        );
        assert!(
            serde_json::to_value(&parsed).expect("serialize identity")["shared"]
                .get("model_revision")
                .is_none()
        );
        assert!(validate_identity("test", &parsed).is_ok());
    }

    #[test]
    fn development_pilot_plan_does_not_require_results_that_only_exist_after_the_pilot() {
        let incumbent = comparison_fixture(RunArm::Incumbent, true, true);
        let candidate = comparison_fixture(RunArm::Candidate, true, true);
        let mut plan = comparison_plan(
            &incumbent,
            &candidate,
            bootstrap_config(31),
            promotion_criteria(),
        );
        plan.stage = PairedEvaluationStage::DevelopmentPilot;
        plan.pilot_report_sha256 = None;
        plan.power_analysis_sha256 = None;
        plan.promotion_criteria = None;
        plan.candidate_product = plan.incumbent_product.clone();
        plan.declared_tool_schema_change = None;

        assert!(plan.validate().is_ok());
    }

    #[test]
    fn pilot_plan_cannot_be_promoted_or_compare_different_products() {
        let incumbent = comparison_fixture(RunArm::Incumbent, true, true);
        let mut candidate = comparison_fixture(RunArm::Candidate, true, true);
        let mut incumbent = incumbent;
        incumbent.run_id = Some("pilot-run".into());
        candidate.run_id = Some("pilot-run".into());
        let mut plan = comparison_plan(
            &incumbent,
            &candidate,
            bootstrap_config(32),
            promotion_criteria(),
        );
        plan.stage = PairedEvaluationStage::DevelopmentPilot;
        plan.pilot_report_sha256 = None;
        plan.power_analysis_sha256 = None;
        plan.promotion_criteria = None;
        plan.candidate_product = plan.incumbent_product.clone();
        plan.declared_tool_schema_change = None;
        assert!(plan.validate().is_ok());
        let comparison = compare_paired_runs(&incumbent, &candidate, &plan)
            .expect("incumbent against itself is a valid pilot comparison");
        assert!(evaluate_promotion(&comparison, &promotion_criteria()).is_err());

        let mut invalid_plan = plan.clone();
        invalid_plan.candidate_product.revision = "candidate-revision".into();
        assert!(invalid_plan.validate().is_err());
    }

    #[test]
    fn confirmatory_plan_serialization_omits_the_default_stage() {
        let incumbent = comparison_fixture(RunArm::Incumbent, true, true);
        let candidate = comparison_fixture(RunArm::Candidate, true, true);
        let plan = comparison_plan(
            &incumbent,
            &candidate,
            bootstrap_config(33),
            promotion_criteria(),
        );

        let value = serde_json::to_value(&plan).expect("serialize confirmatory plan");

        assert!(value.get("stage").is_none());
        assert!(
            value
                .get("pilot_report_sha256")
                .is_some_and(|value| value.is_string())
        );
        assert!(
            value
                .get("power_analysis_sha256")
                .is_some_and(|value| value.is_string())
        );
        assert!(
            value
                .get("promotion_criteria")
                .is_some_and(|value| value.is_object())
        );
        let restored: PairedEvaluationPlan =
            serde_json::from_value(value).expect("deserialize confirmatory plan v1");
        assert_eq!(restored, plan);
    }

    #[test]
    fn development_pilot_report_uses_retest_means_at_the_family_level() {
        let mut first_incumbent = comparison_fixture(RunArm::Incumbent, true, true);
        let mut first_candidate = comparison_fixture(RunArm::Candidate, true, true);
        let mut second_incumbent = comparison_fixture(RunArm::Incumbent, true, true);
        let mut second_candidate = comparison_fixture(RunArm::Candidate, true, true);
        first_incumbent.run_id = Some("pilot-run-one".into());
        first_candidate.run_id = Some("pilot-run-one".into());
        second_incumbent.run_id = Some("pilot-run-two".into());
        second_candidate.run_id = Some("pilot-run-two".into());
        second_candidate.receipts[0].task_oracle_passed = false;
        second_candidate.receipts[1].task_oracle_passed = false;

        let mut plan = comparison_plan(
            &first_incumbent,
            &first_candidate,
            bootstrap_config(34),
            promotion_criteria(),
        );
        plan.stage = PairedEvaluationStage::DevelopmentPilot;
        plan.pilot_report_sha256 = None;
        plan.power_analysis_sha256 = None;
        plan.promotion_criteria = None;
        plan.candidate_product = plan.incumbent_product.clone();
        plan.declared_tool_schema_change = None;

        let report = summarize_development_pilot(
            &plan,
            &first_incumbent,
            &first_candidate,
            &second_incumbent,
            &second_candidate,
        )
        .expect("two incumbent test-retest runs produce a variance report");

        assert_eq!(report.independent_family_count, 3);
        let task_success = &report.metrics[&PilotMetric::TaskSuccess];
        assert_eq!(task_success.family_deltas["history-1"], -0.5);
        assert_eq!(task_success.family_deltas["history-2"], -0.5);
        assert_eq!(task_success.family_deltas["history-3"], 0.0);
        assert!((task_success.between_family_sample_variance - 1.0 / 12.0).abs() < 1e-12);

        let baseline_task_success = &report.same_product_outcomes[&PilotOutcome::TaskSuccess];
        assert_eq!(
            baseline_task_success.first_run_family_means["history-1"],
            1.0
        );
        assert_eq!(
            baseline_task_success.second_run_family_means["history-1"],
            0.5
        );
        assert_eq!(baseline_task_success.family_means["history-1"], 0.75);
        assert_eq!(
            baseline_task_success.retest_family_deltas["history-1"],
            -0.5
        );
        assert!((baseline_task_success.test_retest_sample_variance - 1.0 / 12.0).abs() < 1e-12);

        let baseline_turns = &report.same_product_outcomes[&PilotOutcome::ChargedTurns];
        assert_eq!(baseline_turns.first_run_family_means["history-1"], 3.0);
        assert_eq!(baseline_turns.second_run_family_means["history-1"], 11.5);
        assert_eq!(baseline_turns.retest_family_deltas["history-1"], 8.5);
        assert!(report.validate().is_ok());

        let mut tampered = report;
        tampered
            .same_product_outcomes
            .get_mut(&PilotOutcome::TaskSuccess)
            .expect("task success baseline is summarized")
            .retest_family_deltas
            .insert("history-1".into(), 0.0);
        assert!(tampered.validate().is_err());
    }

    #[test]
    fn development_pilot_report_rejects_reusing_the_same_runner_session() {
        let mut incumbent = comparison_fixture(RunArm::Incumbent, true, true);
        let mut candidate = comparison_fixture(RunArm::Candidate, true, true);
        incumbent.run_id = Some("same-run".into());
        candidate.run_id = Some("same-run".into());
        let mut plan = comparison_plan(
            &incumbent,
            &candidate,
            bootstrap_config(35),
            promotion_criteria(),
        );
        plan.stage = PairedEvaluationStage::DevelopmentPilot;
        plan.pilot_report_sha256 = None;
        plan.power_analysis_sha256 = None;
        plan.promotion_criteria = None;
        plan.candidate_product = plan.incumbent_product.clone();
        plan.declared_tool_schema_change = None;

        assert!(
            summarize_development_pilot(&plan, &incumbent, &candidate, &incumbent, &candidate,)
                .is_err()
        );
    }

    #[test]
    fn run_artifact_hash_is_stable_when_trial_receipt_order_changes() {
        let mut artifact = comparison_fixture(RunArm::Incumbent, true, true);
        let original_hash = artifact.sha256().unwrap();
        artifact.receipts.reverse();
        artifact.expected_trials.reverse();

        assert_eq!(artifact.sha256().unwrap(), original_hash);
    }

    fn key(family_id: &str, case_id: &str, repetition: u32) -> CaseTrialKey {
        CaseTrialKey {
            stratum_id: "temporal-conflict".into(),
            family_id: family_id.into(),
            case_id: case_id.into(),
            repetition,
            incumbent_first: false,
        }
    }

    #[test]
    fn paired_run_rejects_a_case_omitted_from_one_arm() {
        let first = key("history-a", "case-a", 1);
        let second = key("history-b", "case-b", 1);
        let expected = vec![first.clone(), second.clone()];
        let incumbent = vec![first.clone(), second];
        let candidate = vec![first];

        assert!(validate_case_coverage(&expected, &incumbent, &candidate).is_err());
    }

    #[test]
    fn preregistered_arm_order_must_be_balanced_within_each_stratum() {
        let incumbent = comparison_fixture(RunArm::Incumbent, true, true);
        let candidate = comparison_fixture(RunArm::Candidate, true, true);
        let mut plan = comparison_plan(
            &incumbent,
            &candidate,
            bootstrap_config(19),
            promotion_criteria(),
        );
        for key in &mut plan.expected_trials {
            key.incumbent_first = true;
        }

        assert!(plan.validate().is_err());
    }

    #[test]
    fn run_receipt_must_follow_the_preregistered_arm_order() {
        let mut candidate = comparison_fixture(RunArm::Candidate, true, true);
        candidate.receipts[0].arm_order = 3 - candidate.receipts[0].arm_order;

        assert!(candidate.validate().is_err());
    }

    #[test]
    fn v5_and_v6_artifacts_enforce_their_task_oracle_diagnostic_contracts() {
        let mut artifact = comparison_fixture(RunArm::Candidate, true, false);
        artifact.schema_version = PAIRED_ARTIFACT_SCHEMA_V5.into();
        assert!(artifact.validate().is_err());

        for receipt in &mut artifact.receipts {
            receipt.task_oracle_diagnostics = Some(TaskOracleDiagnostics {
                final_answer_present: true,
                final_answer_schema_valid: true,
                accepted_answer: Some(false),
                accepted_answer_match: None,
                required_evidence_cited: Some(true),
                clarification_matches: true,
            });
        }
        assert!(artifact.validate().is_ok());

        let mut v6 = artifact.clone();
        v6.schema_version = PAIRED_ARTIFACT_SCHEMA_V6.into();
        for receipt in &mut v6.receipts {
            receipt
                .task_oracle_diagnostics
                .as_mut()
                .expect("v5 diagnostics")
                .accepted_answer_match = Some(AcceptedAnswerMatch::NoAcceptedValueMentioned);
        }
        assert!(v6.validate().is_ok());

        let mut v6_missing_answer_match = v6.clone();
        v6_missing_answer_match.receipts[0]
            .task_oracle_diagnostics
            .as_mut()
            .expect("v6 diagnostics")
            .accepted_answer_match = None;
        assert!(v6_missing_answer_match.validate().is_err());

        let mut v5_with_v6_diagnostics = artifact.clone();
        v5_with_v6_diagnostics.receipts[0]
            .task_oracle_diagnostics
            .as_mut()
            .expect("v5 diagnostics")
            .accepted_answer_match = Some(AcceptedAnswerMatch::NoAcceptedValueMentioned);
        assert!(v5_with_v6_diagnostics.validate().is_err());

        let mut inconsistent = artifact.clone();
        inconsistent.receipts[0].task_oracle_passed = true;
        assert!(inconsistent.validate().is_err());

        for legacy_schema in [
            PAIRED_ARTIFACT_SCHEMA_V2,
            PAIRED_ARTIFACT_SCHEMA_V3,
            PAIRED_ARTIFACT_SCHEMA_V4,
        ] {
            let mut legacy = artifact.clone();
            legacy.schema_version = legacy_schema.into();
            assert!(
                legacy.validate().is_err(),
                "{legacy_schema} must reject v5 diagnostics"
            );
        }

        let legacy_v5_json = serde_json::to_value(&artifact).expect("serialize v5 artifact");
        let legacy_v5_round_trip: PairedRunArtifact =
            serde_json::from_value(legacy_v5_json).expect("read existing v5 artifact");
        assert_eq!(legacy_v5_round_trip, artifact);
        assert!(legacy_v5_round_trip.validate().is_ok());

        let mut legacy_without_diagnostics = artifact;
        legacy_without_diagnostics.schema_version = PAIRED_ARTIFACT_SCHEMA_V4.into();
        for receipt in &mut legacy_without_diagnostics.receipts {
            receipt.task_oracle_diagnostics = None;
        }
        assert!(legacy_without_diagnostics.validate().is_ok());
    }

    #[test]
    fn paired_arms_must_record_the_same_knowledge_revision_checkpoint() {
        let incumbent = comparison_fixture(RunArm::Incumbent, true, true);
        let mut candidate = comparison_fixture(RunArm::Candidate, true, true);
        candidate.receipts[0].known_revision_checkpoint = Some(7);
        let plan = comparison_plan(
            &incumbent,
            &candidate,
            bootstrap_config(21),
            promotion_criteria(),
        );
        assert!(compare_paired_runs(&incumbent, &candidate, &plan).is_err());

        let mut incumbent = incumbent;
        incumbent.receipts[0].known_revision_checkpoint = Some(7);
        assert!(compare_paired_runs(&incumbent, &candidate, &plan).is_ok());

        candidate.receipts[0].known_revision_checkpoint = Some(8);
        assert!(compare_paired_runs(&incumbent, &candidate, &plan).is_err());
    }

    #[test]
    fn receipt_rejects_a_negative_knowledge_revision_checkpoint() {
        let mut artifact = comparison_fixture(RunArm::Incumbent, true, true);
        artifact.receipts[0].known_revision_checkpoint = Some(-1);

        assert!(artifact.validate().is_err());
    }

    #[test]
    fn legacy_receipts_without_a_checkpoint_remain_readable_and_unchanged() {
        let artifact = comparison_fixture(RunArm::Incumbent, true, true);
        let receipt = &artifact.receipts[0];
        let json = serde_json::to_value(receipt).unwrap();

        assert!(json.get("known_revision_checkpoint").is_none());
        assert_eq!(
            serde_json::from_value::<TrialReceipt>(json).unwrap(),
            *receipt
        );
    }

    #[test]
    fn failed_agent_task_is_charged_the_full_turn_budget() {
        let run = identity("fixed-agent");
        let receipt = TrialReceipt {
            read_capture_status: None,
            read_diagnostics: None,
            key: key("history-a", "case-a", 1),
            arm_order: 1,
            known_revision_checkpoint: None,
            status: TrialStatus::TimedOut,
            direct_memory_success: true,
            task_oracle_passed: false,
            task_oracle_diagnostics: None,
            turns_used: 8,
            tool_calls: 4,
            repair_turns: 2,
            user_clarifications: 1,
            input_tokens: 900,
            output_tokens: 350,
            wall_time_ms: 10_000,
            hard_violations: Vec::new(),
            trace: Vec::new(),
        };

        assert_eq!(receipt.charged_repair_turns(run.shared.turn_budget), 20);
        assert_eq!(receipt.charged_turns(run.shared.turn_budget), 20);
    }

    #[test]
    fn promotion_rejects_memory_recall_gain_when_agent_task_success_worsens() {
        let incumbent = comparison_fixture(RunArm::Incumbent, false, true);
        let candidate = comparison_fixture(RunArm::Candidate, true, false);
        let bootstrap = BootstrapConfig {
            seed: 7,
            draws: 1_000,
            confidence_level_basis_points: 9_500,
        };
        let criteria = promotion_criteria();
        let plan = comparison_plan(&incumbent, &candidate, bootstrap, criteria.clone());
        let comparison = compare_paired_runs(&incumbent, &candidate, &plan).unwrap();

        let promotion = evaluate_promotion(&comparison, &criteria).unwrap();

        assert_eq!(promotion.decision, PromotionDecision::Reject);
        assert_eq!(comparison.overall.direct_memory_success.estimate, 1.0);
        assert_eq!(comparison.overall.task_success.estimate, -1.0);
    }

    #[test]
    fn promotion_passes_only_when_all_three_primary_gates_improve() {
        let incumbent = comparison_fixture(RunArm::Incumbent, false, false);
        let mut candidate = comparison_fixture(RunArm::Candidate, true, true);
        for receipt in &mut candidate.receipts {
            set_repair_turns(receipt, 0);
        }
        let criteria = promotion_criteria();
        let plan = comparison_plan(
            &incumbent,
            &candidate,
            bootstrap_config(9),
            criteria.clone(),
        );
        let comparison = compare_paired_runs(&incumbent, &candidate, &plan).unwrap();

        let promotion = evaluate_promotion(&comparison, &criteria).unwrap();

        assert_eq!(promotion.decision, PromotionDecision::Promote);
        assert_eq!(comparison.overall.direct_memory_success.estimate, 1.0);
        assert_eq!(comparison.overall.task_success.estimate, 1.0);
        assert_eq!(comparison.overall.repair_turn_gain.estimate, 20.0);

        let mut report = PairedEvaluationReport {
            comparison,
            promotion,
        };
        assert!(report.validate().is_ok());
        report.promotion.decision = PromotionDecision::Reject;
        assert!(report.validate().is_err());
    }

    #[test]
    fn promotion_rejects_primary_gains_with_a_material_latency_regression() {
        let incumbent = comparison_fixture(RunArm::Incumbent, false, false);
        let mut candidate = comparison_fixture(RunArm::Candidate, true, true);
        for receipt in &mut candidate.receipts {
            set_repair_turns(receipt, 0);
            receipt.wall_time_ms = 30_000;
        }
        let criteria = promotion_criteria();
        let plan = comparison_plan(
            &incumbent,
            &candidate,
            bootstrap_config(47),
            criteria.clone(),
        );
        let comparison = compare_paired_runs(&incumbent, &candidate, &plan).unwrap();

        let promotion = evaluate_promotion(&comparison, &criteria).unwrap();

        assert_eq!(comparison.overall.wall_time_gain_ms.estimate, -28_000.0);
        assert_eq!(promotion.decision, PromotionDecision::Reject);
    }

    #[test]
    fn promotion_rejects_an_ordinary_workflow_resource_regression_hidden_by_other_strata() {
        let incumbent = stratified_comparison_fixture(RunArm::Incumbent, false, false);
        let candidate = stratified_comparison_fixture(RunArm::Candidate, true, true);
        let criteria = PromotionCriteria {
            direct_memory_margin: 0.05,
            task_success_margin: 0.05,
            repair_turn_gain_margin: 0.05,
            task_non_inferiority_by_stratum: BTreeMap::from([
                ("ordinary".into(), 0.05),
                ("new-capability".into(), 0.05),
            ]),
            ordinary_workflow_strata: BTreeSet::from(["ordinary".into()]),
            resource_non_inferiority: ResourceNonInferiority {
                turns: 0.0,
                tool_calls: 0.0,
                user_clarifications: 0.0,
                input_tokens: 0.0,
                output_tokens: 0.0,
                wall_time_ms: 1_000.0,
            },
            resource_non_inferiority_by_stratum: BTreeMap::from([(
                "ordinary".into(),
                ResourceNonInferiority {
                    turns: 0.0,
                    tool_calls: 0.0,
                    user_clarifications: 0.0,
                    input_tokens: 0.0,
                    output_tokens: 0.0,
                    wall_time_ms: 0.0,
                },
            )]),
        };
        let mut plan = comparison_plan(
            &incumbent,
            &candidate,
            bootstrap_config(53),
            criteria.clone(),
        );
        plan.planned_independent_families = 6;
        let comparison = compare_paired_runs(&incumbent, &candidate, &plan).unwrap();

        let promotion = evaluate_promotion(&comparison, &criteria).unwrap();

        assert!(
            comparison
                .overall
                .wall_time_gain_ms
                .interval
                .as_ref()
                .unwrap()
                .lower
                >= -1_000.0
        );
        assert_eq!(
            comparison.by_stratum["ordinary"]
                .wall_time_gain_ms
                .interval
                .as_ref()
                .unwrap()
                .upper,
            -1_000.0
        );
        assert_eq!(promotion.decision, PromotionDecision::Reject);
    }

    #[test]
    fn gate_reports_insufficient_evidence_when_a_paired_interval_crosses_the_margin() {
        let incumbent = comparison_fixture(RunArm::Incumbent, false, false);
        let mut candidate = comparison_fixture(RunArm::Candidate, true, true);
        for receipt in candidate.receipts.iter_mut().skip(1) {
            receipt.direct_memory_success = false;
        }
        for receipt in &mut candidate.receipts {
            set_repair_turns(receipt, 0);
        }
        let criteria = promotion_criteria();
        let plan = comparison_plan(
            &incumbent,
            &candidate,
            bootstrap_config(10),
            criteria.clone(),
        );
        let comparison = compare_paired_runs(&incumbent, &candidate, &plan).unwrap();

        let promotion = evaluate_promotion(&comparison, &criteria).unwrap();

        assert_eq!(
            comparison
                .overall
                .direct_memory_success
                .interval
                .as_ref()
                .unwrap()
                .lower,
            0.0
        );
        assert_eq!(promotion.decision, PromotionDecision::InsufficientEvidence);
    }

    #[test]
    fn timeout_remains_in_task_denominator_and_uses_maximum_interaction_charge() {
        let incumbent = comparison_fixture(RunArm::Incumbent, true, true);
        let mut candidate = comparison_fixture(RunArm::Candidate, true, true);
        candidate.receipts[0].status = TrialStatus::TimedOut;
        candidate.receipts[0].task_oracle_passed = false;
        candidate.receipts[0].wall_time_ms = candidate.identity.shared.wall_time_budget_ms + 1;
        candidate.receipts[0].trace = trace_for_trial(3, 2, 1, TrialStatus::TimedOut);
        let plan = comparison_plan(
            &incumbent,
            &candidate,
            bootstrap_config(11),
            promotion_criteria(),
        );

        let comparison = compare_paired_runs(&incumbent, &candidate, &plan).unwrap();

        assert!((comparison.overall.task_success.estimate + (1.0 / 3.0)).abs() < 1e-12);
        assert!((comparison.overall.repair_turn_gain.estimate + (19.0 / 3.0)).abs() < 1e-12);
    }

    #[test]
    fn model_failure_trace_can_record_a_closed_normalized_error_class() {
        let mut artifact = comparison_fixture(RunArm::Candidate, true, true);
        let receipt = &mut artifact.receipts[0];
        receipt.status = TrialStatus::ModelFailure;
        receipt.direct_memory_success = false;
        receipt.task_oracle_passed = false;
        receipt.trace = trace_for_trial(3, 2, 1, TrialStatus::ModelFailure);
        receipt
            .trace
            .last_mut()
            .expect("model-failure trace has a terminal event")
            .error_class = Some(NormalizedErrorClass::Transport);

        assert!(artifact.validate().is_ok());
    }

    #[test]
    fn model_response_diagnostics_require_the_v3_paired_artifact_schema() {
        let mut artifact = comparison_fixture(RunArm::Candidate, true, true);
        let receipt = &mut artifact.receipts[0];
        receipt.status = TrialStatus::ModelFailure;
        receipt.direct_memory_success = false;
        receipt.task_oracle_passed = false;
        receipt.trace = trace_for_trial(3, 2, 1, TrialStatus::ModelFailure);
        receipt
            .trace
            .last_mut()
            .expect("model-failure trace has a terminal event")
            .error_class = Some(NormalizedErrorClass::InvalidFinalAnswer);

        artifact.schema_version = PAIRED_ARTIFACT_SCHEMA_V3.into();
        assert!(artifact.validate().is_ok());

        artifact.schema_version = PAIRED_ARTIFACT_SCHEMA_V2.into();
        assert!(artifact.validate().is_err());
    }

    #[test]
    fn granular_final_answer_diagnostics_require_the_v4_paired_artifact_schema() {
        let mut artifact = comparison_fixture(RunArm::Candidate, true, true);
        let receipt = &mut artifact.receipts[0];
        receipt.status = TrialStatus::ModelFailure;
        receipt.direct_memory_success = false;
        receipt.task_oracle_passed = false;
        receipt.trace = trace_for_trial(3, 2, 1, TrialStatus::ModelFailure);
        let class =
            serde_json::from_value(serde_json::Value::String("final_answer_not_json".into()));
        assert!(
            class.is_ok(),
            "the new closed diagnostic class must deserialize"
        );
        receipt.trace.last_mut().unwrap().error_class = Some(class.unwrap());

        artifact.schema_version = "memory-mcp-agent-paired/v4".into();
        assert!(artifact.validate().is_ok());

        artifact.schema_version = PAIRED_ARTIFACT_SCHEMA_V3.into();
        assert!(artifact.validate().is_err());
    }

    #[test]
    fn timeout_trace_can_record_a_closed_normalized_error_class() {
        let mut artifact = comparison_fixture(RunArm::Candidate, true, true);
        let receipt = &mut artifact.receipts[0];
        receipt.status = TrialStatus::TimedOut;
        receipt.direct_memory_success = false;
        receipt.task_oracle_passed = false;
        receipt.trace = trace_for_trial(3, 2, 1, TrialStatus::TimedOut);
        receipt
            .trace
            .last_mut()
            .expect("timeout trace has a terminal event")
            .error_class = Some(NormalizedErrorClass::Timeout);

        assert!(artifact.validate().is_ok());
    }

    #[test]
    fn ordinary_trace_event_cannot_record_a_normalized_error_class() {
        let mut artifact = comparison_fixture(RunArm::Candidate, true, true);
        let receipt = &mut artifact.receipts[0];
        receipt
            .trace
            .iter_mut()
            .find(|event| event.kind == TraceEventKind::FinalAnswer)
            .expect("completed trial has a final answer")
            .error_class = Some(NormalizedErrorClass::Transport);

        assert!(artifact.validate().is_err());
    }

    #[test]
    fn timed_out_attempt_can_record_a_started_turn_without_provider_usage() {
        let mut artifact = comparison_fixture(RunArm::Candidate, true, true);
        let receipt = &mut artifact.receipts[0];
        receipt.status = TrialStatus::TimedOut;
        receipt.direct_memory_success = false;
        receipt.task_oracle_passed = false;
        receipt.turns_used = 1;
        receipt.tool_calls = 1;
        receipt.repair_turns = 0;
        receipt.user_clarifications = 0;
        receipt.input_tokens = 0;
        receipt.output_tokens = 0;
        receipt.trace = vec![
            AgentTraceEvent {
                sequence: 1,
                turn: 1,
                kind: TraceEventKind::AgentTurnStarted,
                call_id: None,
                tool_name: None,
                error_class: None,
                payload_sha256: None,
                input_tokens: None,
                output_tokens: None,
            },
            AgentTraceEvent {
                sequence: 2,
                turn: 1,
                kind: TraceEventKind::MemoryRead,
                call_id: Some("call-1".into()),
                tool_name: Some("memory_decisions/assemble_context".into()),
                error_class: None,
                payload_sha256: Some("a".repeat(64)),
                input_tokens: None,
                output_tokens: None,
            },
            AgentTraceEvent {
                sequence: 3,
                turn: 1,
                kind: TraceEventKind::Timeout,
                call_id: None,
                tool_name: None,
                error_class: None,
                payload_sha256: None,
                input_tokens: None,
                output_tokens: None,
            },
        ];

        assert!(artifact.validate().is_ok());
    }

    #[test]
    fn trace_cannot_drop_a_failed_tool_call_while_retaining_its_task_receipt() {
        let incumbent = comparison_fixture(RunArm::Incumbent, true, true);
        let mut candidate = comparison_fixture(RunArm::Candidate, true, true);
        let plan = comparison_plan(
            &incumbent,
            &candidate,
            bootstrap_config(12),
            promotion_criteria(),
        );
        candidate.receipts[0]
            .trace
            .retain(|event| event.kind != TraceEventKind::ToolError);
        for (index, event) in candidate.receipts[0].trace.iter_mut().enumerate() {
            event.sequence = index as u32 + 1;
        }

        assert!(compare_paired_runs(&incumbent, &candidate, &plan).is_err());
    }

    #[test]
    fn repair_turn_counter_must_match_failed_tool_turns_in_trace() {
        let incumbent = comparison_fixture(RunArm::Incumbent, true, true);
        let mut candidate = comparison_fixture(RunArm::Candidate, true, true);
        candidate.receipts[0].repair_turns = 0;
        let plan = comparison_plan(
            &incumbent,
            &candidate,
            bootstrap_config(14),
            promotion_criteria(),
        );

        assert!(compare_paired_runs(&incumbent, &candidate, &plan).is_err());
    }

    #[test]
    fn tool_failure_status_requires_a_failed_tool_call_in_the_trace() {
        let mut artifact = comparison_fixture(RunArm::Incumbent, true, true);
        let receipt = &mut artifact.receipts[0];
        receipt.status = TrialStatus::ToolFailure;
        receipt.task_oracle_passed = false;
        set_repair_turns(receipt, 0);
        receipt
            .trace
            .retain(|event| event.kind != TraceEventKind::FinalAnswer);
        for (index, event) in receipt.trace.iter_mut().enumerate() {
            event.sequence = index as u32 + 1;
        }

        assert!(artifact.validate().is_err());
    }

    #[test]
    fn trace_schema_rejects_unrecognized_free_form_error_details() {
        let event = AgentTraceEvent {
            sequence: 1,
            turn: 1,
            kind: TraceEventKind::ToolError,
            call_id: Some("call-1".into()),
            tool_name: Some("assemble_context".into()),
            error_class: Some(NormalizedErrorClass::Retryable),
            payload_sha256: Some("a".repeat(64)),
            input_tokens: None,
            output_tokens: None,
        };
        let mut json = serde_json::to_value(event).unwrap();
        json["error_class"] = serde_json::json!("provider_detail");

        assert!(serde_json::from_value::<AgentTraceEvent>(json).is_err());
    }

    #[test]
    fn token_totals_must_match_per_turn_trace_usage() {
        let mut artifact = comparison_fixture(RunArm::Incumbent, true, true);
        artifact.receipts[0].input_tokens += 1;

        assert!(artifact.validate().is_err());
    }

    #[test]
    fn completed_trial_cannot_exceed_the_per_turn_context_budget() {
        let mut artifact = comparison_fixture(RunArm::Incumbent, true, true);
        artifact.identity.shared.context_budget_tokens = 39;

        assert!(artifact.validate().is_err());
    }

    #[test]
    fn any_hard_violation_fails_the_promotion_invariant() {
        let incumbent = comparison_fixture(RunArm::Incumbent, true, true);
        let mut candidate = comparison_fixture(RunArm::Candidate, true, true);
        candidate.receipts[0]
            .hard_violations
            .push(HardViolation::UnauthorizedRead);
        let criteria = promotion_criteria();
        let plan = comparison_plan(
            &incumbent,
            &candidate,
            bootstrap_config(13),
            criteria.clone(),
        );
        let comparison = compare_paired_runs(&incumbent, &candidate, &plan).unwrap();

        let promotion = evaluate_promotion(&comparison, &criteria).unwrap();

        assert_eq!(comparison.hard_violation_count, 1);
        assert_eq!(promotion.decision, PromotionDecision::Reject);
        assert_eq!(promotion.checks[3].status, PromotionCheckStatus::Failed);
    }

    #[test]
    fn comparison_rejects_an_identically_omitted_trial_from_both_arms() {
        let mut incumbent = comparison_fixture(RunArm::Incumbent, false, true);
        let mut candidate = comparison_fixture(RunArm::Candidate, false, true);
        let plan = comparison_plan(
            &incumbent,
            &candidate,
            bootstrap_config(17),
            promotion_criteria(),
        );
        incumbent.expected_trials.pop();
        incumbent.receipts.pop();
        candidate.expected_trials.pop();
        candidate.receipts.pop();

        assert!(compare_paired_runs(&incumbent, &candidate, &plan).is_err());
    }

    #[test]
    fn comparison_is_invariant_to_balanced_arm_execution_order() {
        let incumbent = comparison_fixture(RunArm::Incumbent, true, true);
        let candidate = comparison_fixture(RunArm::Candidate, true, true);
        let plan = comparison_plan(
            &incumbent,
            &candidate,
            bootstrap_config(29),
            promotion_criteria(),
        );
        let before = compare_paired_runs(&incumbent, &candidate, &plan).unwrap();
        let mut reversed_incumbent = incumbent.clone();
        let mut reversed_candidate = candidate.clone();
        let mut reversed_plan = plan.clone();
        for key in &mut reversed_plan.expected_trials {
            key.incumbent_first = !key.incumbent_first;
        }
        for run in [&mut reversed_incumbent, &mut reversed_candidate] {
            for key in &mut run.expected_trials {
                key.incumbent_first = !key.incumbent_first;
            }
            for receipt in &mut run.receipts {
                receipt.key.incumbent_first = !receipt.key.incumbent_first;
                receipt.arm_order = 3 - receipt.arm_order;
            }
        }
        let after =
            compare_paired_runs(&reversed_incumbent, &reversed_candidate, &reversed_plan).unwrap();

        assert_eq!(before.overall, after.overall);
        assert_eq!(before.by_stratum, after.by_stratum);
        assert_ne!(before.arm_order_counts, after.arm_order_counts);
    }

    #[test]
    fn promotion_cannot_change_margins_after_the_results_are_seen() {
        let incumbent = comparison_fixture(RunArm::Incumbent, true, true);
        let candidate = comparison_fixture(RunArm::Candidate, true, true);
        let plan = comparison_plan(
            &incumbent,
            &candidate,
            bootstrap_config(31),
            promotion_criteria(),
        );
        let comparison = compare_paired_runs(&incumbent, &candidate, &plan).unwrap();
        let mut revised_criteria = plan
            .promotion_criteria
            .clone()
            .expect("confirmatory plan has promotion criteria");
        revised_criteria.direct_memory_margin = 0.001;

        assert!(evaluate_promotion(&comparison, &revised_criteria).is_err());
    }

    #[test]
    fn comparison_validation_recomputes_confidence_interval_from_family_deltas() {
        let incumbent = comparison_fixture(RunArm::Incumbent, false, true);
        let candidate = comparison_fixture(RunArm::Candidate, true, true);
        let plan = comparison_plan(
            &incumbent,
            &candidate,
            bootstrap_config(37),
            promotion_criteria(),
        );
        let mut comparison = compare_paired_runs(&incumbent, &candidate, &plan).unwrap();
        comparison
            .overall
            .direct_memory_success
            .interval
            .as_mut()
            .unwrap()
            .lower = 0.5;

        let error = comparison
            .validate()
            .expect_err("the altered interval must be rejected")
            .to_string();
        assert!(error.contains("overall.direct_memory_success"));
    }

    #[test]
    fn metric_aggregation_weights_history_families_not_repeated_agent_attempts() {
        let trials = vec![
            metric_trial("family-a", "case-a", 1, 1.0),
            metric_trial("family-a", "case-a", 2, 1.0),
            metric_trial("family-a", "case-b", 1, -1.0),
            metric_trial("family-b", "case-c", 1, -1.0),
            metric_trial("family-b", "case-c", 2, -1.0),
        ];
        let estimate = summarize_metric(
            &trials,
            |trial| trial.direct_memory_success,
            &bootstrap_config(41),
        )
        .unwrap();

        assert_eq!(estimate.family_deltas["family-a"], 0.0);
        assert_eq!(estimate.family_deltas["family-b"], -1.0);
        assert_eq!(estimate.estimate, -0.5);
    }

    fn metric_trial(
        family_id: &str,
        case_id: &str,
        repetition: u32,
        direct_memory_success: f64,
    ) -> PairedTrialDelta {
        PairedTrialDelta {
            key: key(family_id, case_id, repetition),
            direct_memory_success,
            task_success: 0.0,
            repair_turn_gain: 0.0,
            turn_gain: 0.0,
            tool_call_gain: 0.0,
            clarification_gain: 0.0,
            input_token_gain: 0.0,
            output_token_gain: 0.0,
            wall_time_gain_ms: 0.0,
        }
    }

    fn set_repair_turns(receipt: &mut TrialReceipt, repair_turns: u32) {
        receipt.repair_turns = repair_turns;
        if repair_turns == 0 {
            for event in &mut receipt.trace {
                if event.kind == TraceEventKind::ToolError {
                    event.kind = TraceEventKind::ToolResponse;
                    event.error_class = None;
                }
            }
        }
    }

    fn bootstrap_config(seed: u64) -> BootstrapConfig {
        BootstrapConfig {
            seed,
            draws: 1_000,
            confidence_level_basis_points: 9_500,
        }
    }

    fn promotion_criteria() -> PromotionCriteria {
        PromotionCriteria {
            direct_memory_margin: 0.05,
            task_success_margin: 0.05,
            repair_turn_gain_margin: 0.05,
            task_non_inferiority_by_stratum: BTreeMap::from([("temporal-conflict".into(), 0.05)]),
            ordinary_workflow_strata: BTreeSet::from(["temporal-conflict".into()]),
            resource_non_inferiority: ResourceNonInferiority {
                turns: 0.0,
                tool_calls: 0.0,
                user_clarifications: 0.0,
                input_tokens: 0.0,
                output_tokens: 0.0,
                wall_time_ms: 100.0,
            },
            resource_non_inferiority_by_stratum: BTreeMap::from([(
                "temporal-conflict".into(),
                ResourceNonInferiority {
                    turns: 0.0,
                    tool_calls: 0.0,
                    user_clarifications: 0.0,
                    input_tokens: 0.0,
                    output_tokens: 0.0,
                    wall_time_ms: 0.0,
                },
            )]),
        }
    }

    fn comparison_plan(
        incumbent: &PairedRunArtifact,
        candidate: &PairedRunArtifact,
        bootstrap: BootstrapConfig,
        promotion_criteria: PromotionCriteria,
    ) -> PairedEvaluationPlan {
        PairedEvaluationPlan {
            schema_version: PAIRED_PLAN_SCHEMA_V2.into(),
            plan_id: "pilot-temporal-v1".into(),
            stage: PairedEvaluationStage::Confirmatory,
            pilot_report_sha256: Some("3".repeat(64)),
            power_analysis_sha256: Some("4".repeat(64)),
            expected_trials: incumbent.expected_trials.clone(),
            planned_independent_families: 3,
            shared_identity: incumbent.identity.shared.clone(),
            incumbent_product: incumbent.identity.product.clone(),
            candidate_product: candidate.identity.product.clone(),
            declared_tool_schema_change: None,
            bootstrap,
            promotion_criteria: Some(promotion_criteria),
        }
    }

    fn comparison_fixture(
        arm: RunArm,
        memory_success: bool,
        task_success: bool,
    ) -> PairedRunArtifact {
        let expected_trials: Vec<_> = (1..=3)
            .map(|index| CaseTrialKey {
                stratum_id: "temporal-conflict".into(),
                family_id: format!("history-{index}"),
                case_id: format!("case-{index}"),
                repetition: 1,
                incumbent_first: index % 2 == 1,
            })
            .collect();
        let identity = identity("fixed-agent");
        let receipts = expected_trials
            .iter()
            .map(|key| TrialReceipt {
                read_capture_status: None,
                read_diagnostics: None,
                key: key.clone(),
                arm_order: if (arm == RunArm::Incumbent) == key.incumbent_first {
                    1
                } else {
                    2
                },
                known_revision_checkpoint: None,
                status: TrialStatus::Completed,
                direct_memory_success: memory_success,
                task_oracle_passed: task_success,
                task_oracle_diagnostics: None,
                turns_used: 3,
                tool_calls: 2,
                repair_turns: 1,
                user_clarifications: 0,
                input_tokens: 120,
                output_tokens: 40,
                wall_time_ms: 2_000,
                hard_violations: Vec::new(),
                trace: trace_for_trial(3, 2, 1, TrialStatus::Completed),
            })
            .collect();

        PairedRunArtifact {
            schema_version: PAIRED_ARTIFACT_SCHEMA_V2.into(),
            arm,
            run_id: None,
            run_started_at: fixed_run_start(),
            run_completed_at: fixed_run_start() + chrono::Duration::seconds(10),
            identity,
            expected_trials,
            receipts,
        }
    }

    fn stratified_comparison_fixture(
        arm: RunArm,
        memory_success: bool,
        task_success: bool,
    ) -> PairedRunArtifact {
        let expected_trials: Vec<_> = (1..=6)
            .map(|index| CaseTrialKey {
                stratum_id: if index <= 2 {
                    "ordinary"
                } else {
                    "new-capability"
                }
                .into(),
                family_id: format!("history-{index}"),
                case_id: format!("case-{index}"),
                repetition: 1,
                incumbent_first: index % 2 == 1,
            })
            .collect();
        let receipts = expected_trials
            .iter()
            .map(|key| {
                let is_candidate = arm == RunArm::Candidate;
                TrialReceipt {
                    read_capture_status: None,
                    read_diagnostics: None,
                    key: key.clone(),
                    arm_order: if (arm == RunArm::Incumbent) == key.incumbent_first {
                        1
                    } else {
                        2
                    },
                    known_revision_checkpoint: None,
                    status: TrialStatus::Completed,
                    direct_memory_success: memory_success,
                    task_oracle_passed: task_success,
                    task_oracle_diagnostics: None,
                    turns_used: 3,
                    tool_calls: 2,
                    repair_turns: 1,
                    user_clarifications: 0,
                    input_tokens: 120,
                    output_tokens: 40,
                    wall_time_ms: if is_candidate && key.stratum_id == "ordinary" {
                        3_000
                    } else if is_candidate {
                        1_000
                    } else {
                        2_000
                    },
                    hard_violations: Vec::new(),
                    trace: trace_for_trial(3, 2, 1, TrialStatus::Completed),
                }
            })
            .collect();
        PairedRunArtifact {
            schema_version: PAIRED_ARTIFACT_SCHEMA_V2.into(),
            arm,
            run_id: None,
            run_started_at: fixed_run_start(),
            run_completed_at: fixed_run_start() + chrono::Duration::seconds(10),
            identity: identity("fixed-agent"),
            expected_trials,
            receipts,
        }
    }

    fn fixed_run_start() -> DateTime<Utc> {
        "2026-11-21T00:00:00Z"
            .parse()
            .expect("parse fixed test time")
    }

    fn trace_for_trial(
        turns_used: u32,
        tool_calls: u32,
        repair_turns: u32,
        status: TrialStatus,
    ) -> Vec<AgentTraceEvent> {
        let mut trace = Vec::new();
        for turn in 1..=turns_used {
            trace.push(AgentTraceEvent {
                sequence: trace.len() as u32 + 1,
                turn,
                kind: TraceEventKind::AgentTurnStarted,
                call_id: None,
                tool_name: None,
                error_class: None,
                payload_sha256: None,
                input_tokens: None,
                output_tokens: None,
            });
            let call_index = turn - 1;
            if call_index < tool_calls {
                let call_id = format!("call-{call_index}");
                let tool_name = if call_index == 0 {
                    "assemble_context"
                } else {
                    "record_episode"
                };
                trace.push(AgentTraceEvent {
                    sequence: trace.len() as u32 + 1,
                    turn,
                    kind: if call_index == 0 {
                        TraceEventKind::MemoryRead
                    } else {
                        TraceEventKind::ToolCall
                    },
                    call_id: Some(call_id.clone()),
                    tool_name: Some(tool_name.into()),
                    error_class: None,
                    payload_sha256: Some("a".repeat(64)),
                    input_tokens: None,
                    output_tokens: None,
                });
                trace.push(AgentTraceEvent {
                    sequence: trace.len() as u32 + 1,
                    turn,
                    kind: if call_index < repair_turns {
                        TraceEventKind::ToolError
                    } else {
                        TraceEventKind::ToolResponse
                    },
                    call_id: Some(call_id),
                    tool_name: Some(tool_name.into()),
                    error_class: (call_index < repair_turns)
                        .then_some(NormalizedErrorClass::Retryable),
                    payload_sha256: Some("b".repeat(64)),
                    input_tokens: None,
                    output_tokens: None,
                });
            }
            trace.push(AgentTraceEvent {
                sequence: trace.len() as u32 + 1,
                turn,
                kind: TraceEventKind::AgentTurn,
                call_id: None,
                tool_name: None,
                error_class: None,
                payload_sha256: None,
                input_tokens: Some(40),
                output_tokens: Some(if turn == turns_used { 20 } else { 10 }),
            });
        }
        let (event_kind, payload_sha256) = match status {
            TrialStatus::Completed => (TraceEventKind::FinalAnswer, Some("c".repeat(64))),
            TrialStatus::TimedOut => (TraceEventKind::Timeout, None),
            TrialStatus::ModelFailure => (TraceEventKind::ModelFailure, None),
            TrialStatus::BudgetExceeded => (TraceEventKind::BudgetLimitReached, None),
            TrialStatus::ToolFailure | TrialStatus::Skipped => return trace,
        };
        trace.push(AgentTraceEvent {
            sequence: trace.len() as u32 + 1,
            turn: turns_used,
            kind: event_kind,
            call_id: None,
            tool_name: None,
            error_class: None,
            payload_sha256,
            input_tokens: None,
            output_tokens: None,
        });
        trace
    }

    #[test]
    fn cluster_bootstrap_repeats_independent_families_with_a_reproducible_interval() {
        let cluster_deltas = BTreeMap::from([
            ("family-a".into(), 1.0),
            ("family-b".into(), 0.0),
            ("family-c".into(), 0.0),
        ]);
        let config = BootstrapConfig {
            seed: 42,
            draws: 2_000,
            confidence_level_basis_points: 9_500,
        };

        let first = cluster_bootstrap(&cluster_deltas, &config).unwrap();
        let second = cluster_bootstrap(&cluster_deltas, &config).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.estimate, 1.0 / 3.0);
        assert_eq!(first.lower, 0.0);
        assert_eq!(first.upper, 1.0);
        assert!(first.lower < first.estimate);
        assert!(first.upper > first.estimate);
        assert_eq!(first.independent_clusters, 3);
        assert_eq!(first.algorithm, "splitmix64-v1");
    }

    #[test]
    fn serialized_bootstrap_interval_reproduces_after_json_roundtrip() {
        let cluster_deltas = BTreeMap::from([
            ("brackenridge_depot".into(), 514.0),
            ("cindergate_hub".into(), 0.0),
            ("copperwind_quay".into(), 20.0),
            ("hollowcrest_observatory".into(), -589.0),
            ("ivory_lantern_clinic".into(), -1.0),
            ("junebrook_foundry".into(), 221.0),
            ("mosslight_annex".into(), -615.0),
            ("northvale_depot".into(), 0.0),
            ("opaline_workshop".into(), -38.0),
            ("tidemark_archive".into(), -40.0),
            ("veloran_station".into(), 4.0),
            ("westmere_lab".into(), 0.0),
        ]);
        let config = BootstrapConfig {
            seed: 2_026_112_402,
            draws: 10_000,
            confidence_level_basis_points: 9_500,
        };
        let generated = cluster_bootstrap(&cluster_deltas, &config).unwrap();
        let json = serde_json::to_vec(&generated).unwrap();
        let loaded: ConfidenceInterval = serde_json::from_slice(&json).unwrap();
        let recomputed = cluster_bootstrap(&cluster_deltas, &config).unwrap();

        assert_eq!(loaded, recomputed);
    }
}
