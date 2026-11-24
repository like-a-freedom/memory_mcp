//! Two-stage agent-managed memory trials with a fixed post-write probe.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{AgentObservation, HiddenTaskOracle, SeedEpisode};
use crate::EvalError;
use crate::paired::{CaseTrialKey, PairedEvaluationPlan, RunArm, validate_case_coverage};

pub const AGENT_MANAGED_TASK_SCHEMA_V1: &str = "memory-mcp-agent-managed-tasks/v1";
pub const AGENT_MANAGED_TASK_SCHEMA_V2: &str = "memory-mcp-agent-managed-tasks/v2";
pub const AGENT_MANAGED_TASK_SCHEMA_V3: &str = "memory-mcp-agent-managed-tasks/v3";
pub const AGENT_MANAGED_ARTIFACT_SCHEMA_V1: &str = "memory-mcp-agent-managed-run/v1";
pub const AGENT_MANAGED_ARTIFACT_SCHEMA_V2: &str = "memory-mcp-agent-managed-run/v2";
pub const AGENT_MANAGED_ARTIFACT_SCHEMA_V3: &str = "memory-mcp-agent-managed-run/v3";
pub const AGENT_MANAGED_ARTIFACT_SCHEMA_V4: &str = "memory-mcp-agent-managed-run/v4";
pub const AGENT_MANAGED_ARTIFACT_SCHEMA_V5: &str = "memory-mcp-agent-managed-run/v5";
pub const AGENT_MANAGED_COMPARISON_SCHEMA_V1: &str = "memory-mcp-agent-managed-comparison/v1";
pub const AGENT_MANAGED_COMPARISON_SCHEMA_V2: &str = "memory-mcp-agent-managed-comparison/v2";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagedTaskInput {
    pub key: CaseTrialKey,
    pub formation_prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_formation_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidate_operator_imports: Vec<AgentManagedOperatorFileImport>,
    pub recall_prompt: String,
    pub seed_episodes: Vec<SeedEpisode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagedTaskDefinition {
    pub key: CaseTrialKey,
    pub formation_prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_formation_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidate_operator_imports: Vec<AgentManagedOperatorFileImport>,
    pub recall_prompt: String,
    pub independent_probe: AgentManagedProbeRequest,
    pub seed_episodes: Vec<SeedEpisode>,
    /// Evaluator-only source label for exactly one successful formation ingest,
    /// whether performed with `ingest` or inline `extract(content=...)`.
    pub target_write_event_id: String,
    pub oracle: HiddenTaskOracle,
}

/// Synthetic source file made available to the evaluator, never to the agent host.
/// The candidate operator adapter imports it before the agent session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagedOperatorFileImport {
    pub fixture_id: String,
    /// Portable path relative to the per-trial isolated workspace.
    pub relative_path: String,
    pub content: String,
    pub source_type: String,
    pub valid_at: DateTime<Utc>,
    pub event_id: String,
}

/// Content-free operator setup telemetry. Paths and source bytes stay out of run artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagedOperatorImportReceipt {
    pub fixture_id: String,
    pub import_succeeded: bool,
    pub extraction_attempted: bool,
    pub extraction_succeeded: bool,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagedTaskManifest {
    pub schema_version: String,
    pub tasks: Vec<AgentManagedTaskDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagedManifestFingerprints {
    pub schema_version: String,
    pub corpus_sha256: String,
    pub oracle_sha256: String,
    pub system_prompt_sha256: String,
    pub task_prompt_sha256: String,
    pub host_policy_sha256: String,
    pub retry_policy_sha256: String,
    pub simulator_sha256: String,
    pub protocol_config_sha256: String,
    pub task_count: u64,
    pub independent_family_count: u64,
}

impl AgentManagedTaskDefinition {
    fn runner_input(&self) -> AgentManagedTaskInput {
        AgentManagedTaskInput {
            key: self.key.clone(),
            formation_prompt: self.formation_prompt.clone(),
            candidate_formation_prompt: self.candidate_formation_prompt.clone(),
            candidate_operator_imports: self.candidate_operator_imports.clone(),
            recall_prompt: self.recall_prompt.clone(),
            seed_episodes: self.seed_episodes.clone(),
        }
    }
}

impl AgentManagedTaskManifest {
    pub fn validate(&self, plan: &PairedEvaluationPlan) -> Result<(), EvalError> {
        plan.validate()?;
        self.validate_definition()?;
        let mut keys = self
            .tasks
            .iter()
            .map(|task| task.key.clone())
            .collect::<Vec<_>>();
        keys.sort();
        validate_case_coverage(&plan.expected_trials, &keys, &keys)?;
        if self.corpus_sha256()? != plan.shared_identity.corpus_sha256
            || self.oracle_sha256()? != plan.shared_identity.oracle_sha256
        {
            return Err(EvalError::InvalidInput(
                "agent-managed manifest does not match the corpus and oracle hashes in the paired plan".into(),
            ));
        }
        Ok(())
    }

    pub fn validate_definition(&self) -> Result<(), EvalError> {
        let schema_v1 = self.schema_version == AGENT_MANAGED_TASK_SCHEMA_V1;
        let schema_v2 = self.schema_version == AGENT_MANAGED_TASK_SCHEMA_V2;
        let schema_v3 = self.schema_version == AGENT_MANAGED_TASK_SCHEMA_V3;
        if !schema_v1 && !schema_v2 && !schema_v3 {
            return Err(EvalError::InvalidInput(
                "unsupported agent-managed task manifest schema".into(),
            ));
        }
        if self.tasks.is_empty() {
            return Err(EvalError::InvalidInput(
                "agent-managed task manifest requires at least one task".into(),
            ));
        }
        let mut keys = self
            .tasks
            .iter()
            .map(|task| task.key.clone())
            .collect::<Vec<_>>();
        keys.sort();
        validate_case_coverage(&keys, &keys, &keys)?;
        for task in &self.tasks {
            if schema_v1
                && (task.candidate_formation_prompt.is_some()
                    || !task.candidate_operator_imports.is_empty())
            {
                return Err(EvalError::InvalidInput(
                    "agent-managed v1 manifest cannot define candidate operator setup".into(),
                ));
            }
            validate_task(task)?;
        }
        Ok(())
    }

    pub fn validate_for_profile(
        &self,
        profile: crate::codex_agent::CodexMemoryToolProfile,
    ) -> Result<(), EvalError> {
        self.validate_definition()?;
        let has_operator_imports = self
            .tasks
            .iter()
            .any(|task| !task.candidate_operator_imports.is_empty());
        match (profile, has_operator_imports) {
            (crate::codex_agent::CodexMemoryToolProfile::AgentSourceAuthority, true)
            | (
                crate::codex_agent::CodexMemoryToolProfile::AgentSourceAuthorityLiteralInput,
                true,
            )
            | (
                crate::codex_agent::CodexMemoryToolProfile::AgentManagedMemory
                | crate::codex_agent::CodexMemoryToolProfile::AgentMaintenance
                | crate::codex_agent::CodexMemoryToolProfile::AgentMaintenanceV2
                | crate::codex_agent::CodexMemoryToolProfile::AgentMaintenanceV3
                | crate::codex_agent::CodexMemoryToolProfile::AgentMaintenanceV4,
                false,
            ) => Ok(()),
            (
                crate::codex_agent::CodexMemoryToolProfile::AgentSourceAuthority
                | crate::codex_agent::CodexMemoryToolProfile::AgentSourceAuthorityLiteralInput,
                false,
            ) => Err(EvalError::InvalidInput(
                "source-authority workflow requires at least one operator-import task".into(),
            )),
            (_, true) => Err(EvalError::InvalidInput(
                "operator-import tasks require the source-authority workflow profile".into(),
            )),
            _ => Err(EvalError::InvalidInput(
                "task manifest profile is not a paired agent-managed workflow".into(),
            )),
        }
    }

    /// Validate the task shape for the source-authority workflow without
    /// selecting any particular agent provider or harness.
    pub fn validate_for_source_authority_workflow(&self) -> Result<(), EvalError> {
        self.validate_definition()?;
        if self
            .tasks
            .iter()
            .any(|task| !task.candidate_operator_imports.is_empty())
        {
            Ok(())
        } else {
            Err(EvalError::InvalidInput(
                "source-authority workflow requires at least one operator-import task".into(),
            ))
        }
    }

    pub fn fingerprints(&self) -> Result<AgentManagedManifestFingerprints, EvalError> {
        self.fingerprints_for_profile(
            crate::codex_agent::CodexMemoryToolProfile::AgentManagedMemory,
        )
    }

    pub fn fingerprints_for_profile(
        &self,
        profile: crate::codex_agent::CodexMemoryToolProfile,
    ) -> Result<AgentManagedManifestFingerprints, EvalError> {
        self.validate_for_profile(profile)?;
        let (system_prompt_sha256, task_prompt_sha256, host_policy_sha256, simulator_sha256) =
            match profile {
                crate::codex_agent::CodexMemoryToolProfile::AgentManagedMemory => (
                    crate::codex_agent::agent_managed_system_prompt_sha256(),
                    crate::codex_agent::agent_managed_task_prompt_sha256(),
                    crate::codex_agent::agent_managed_host_policy_sha256(),
                    sha256(include_bytes!(
                        "../../../../evals/paired/agent-managed-memory-v1/simulator.json"
                    )),
                ),
                crate::codex_agent::CodexMemoryToolProfile::AgentMaintenance => (
                    crate::codex_agent::agent_maintenance_system_prompt_sha256(),
                    crate::codex_agent::agent_maintenance_task_prompt_sha256(),
                    crate::codex_agent::agent_maintenance_host_policy_sha256(),
                    crate::codex_agent::agent_maintenance_simulator_sha256(),
                ),
                crate::codex_agent::CodexMemoryToolProfile::AgentMaintenanceV2 => (
                    crate::codex_agent::agent_maintenance_v2_system_prompt_sha256(),
                    crate::codex_agent::agent_maintenance_v2_task_prompt_sha256(),
                    crate::codex_agent::agent_maintenance_v2_host_policy_sha256(),
                    crate::codex_agent::agent_maintenance_v2_simulator_sha256(),
                ),
                crate::codex_agent::CodexMemoryToolProfile::AgentMaintenanceV3 => (
                    crate::codex_agent::agent_maintenance_v3_system_prompt_sha256(),
                    crate::codex_agent::agent_maintenance_v3_task_prompt_sha256(),
                    crate::codex_agent::agent_maintenance_v3_host_policy_sha256(),
                    crate::codex_agent::agent_maintenance_v3_simulator_sha256(),
                ),
                crate::codex_agent::CodexMemoryToolProfile::AgentMaintenanceV4 => (
                    crate::codex_agent::agent_maintenance_v4_system_prompt_sha256(),
                    crate::codex_agent::agent_maintenance_v4_task_prompt_sha256(),
                    crate::codex_agent::agent_maintenance_v4_host_policy_sha256(),
                    crate::codex_agent::agent_maintenance_v4_simulator_sha256(),
                ),
                crate::codex_agent::CodexMemoryToolProfile::AgentSourceAuthority => (
                    crate::codex_agent::system_instructions_sha256_for_profile(profile),
                    crate::codex_agent::task_rules_sha256_for_profile(profile),
                    crate::codex_agent::agent_source_authority_host_policy_sha256(),
                    crate::codex_agent::agent_source_authority_simulator_sha256(),
                ),
                crate::codex_agent::CodexMemoryToolProfile::AgentSourceAuthorityLiteralInput => (
                    crate::codex_agent::system_instructions_sha256_for_profile(profile),
                    crate::codex_agent::task_rules_sha256_for_profile(profile),
                    crate::codex_agent::agent_source_authority_literal_input_host_policy_sha256(),
                    crate::codex_agent::agent_source_authority_literal_input_simulator_sha256(),
                ),
                _ => {
                    return Err(EvalError::InvalidConfig(
                        "agent-managed task fingerprints require a managed memory profile".into(),
                    ));
                }
            };
        let families = self
            .tasks
            .iter()
            .map(|task| task.key.family_id.as_str())
            .collect::<BTreeSet<_>>();
        Ok(AgentManagedManifestFingerprints {
            schema_version: "memory-mcp-agent-managed-fingerprints/v1".into(),
            corpus_sha256: self.corpus_sha256()?,
            oracle_sha256: self.oracle_sha256()?,
            system_prompt_sha256,
            task_prompt_sha256,
            host_policy_sha256,
            retry_policy_sha256: crate::codex_agent::agent_managed_retry_policy_sha256(),
            simulator_sha256,
            protocol_config_sha256: crate::codex_agent::reader_config_sha256_for_profile(profile)
                .ok_or_else(|| {
                EvalError::InvalidConfig(
                    "agent-managed profile lacks its protocol configuration fingerprint".into(),
                )
            })?,
            task_count: usize_to_u64(self.tasks.len(), "task count")?,
            independent_family_count: usize_to_u64(families.len(), "family count")?,
        })
    }

    /// Fingerprints the provider-neutral literal-input source-authority
    /// protocol used by the loopback-only Chat Completions adapter.
    pub fn fingerprints_for_chat_completions(
        &self,
    ) -> Result<AgentManagedManifestFingerprints, EvalError> {
        self.validate_for_source_authority_workflow()?;
        let families = self
            .tasks
            .iter()
            .map(|task| task.key.family_id.as_str())
            .collect::<BTreeSet<_>>();
        Ok(AgentManagedManifestFingerprints {
            schema_version: "memory-mcp-agent-managed-fingerprints/v1".into(),
            corpus_sha256: self.corpus_sha256()?,
            oracle_sha256: self.oracle_sha256()?,
            system_prompt_sha256: super::prompts::source_authority_system_prompt_sha256(),
            task_prompt_sha256: super::prompts::source_authority_task_prompt_sha256(),
            host_policy_sha256: sha256(include_bytes!(
                "../../../../evals/paired/prompts/chat-completions-host-policy-v1.json"
            )),
            retry_policy_sha256: sha256(include_bytes!(
                "../../../../evals/paired/prompts/chat-completions-retry-policy-v1.json"
            )),
            simulator_sha256: if self.schema_version == AGENT_MANAGED_TASK_SCHEMA_V3 {
                super::prompts::source_authority_literal_input_simulator_sha256()
            } else {
                super::prompts::source_authority_simulator_sha256()
            },
            protocol_config_sha256: if self.schema_version == AGENT_MANAGED_TASK_SCHEMA_V3 {
                super::prompts::source_authority_literal_input_protocol_config_sha256()
            } else {
                super::prompts::source_authority_protocol_config_sha256()
            },
            task_count: usize_to_u64(self.tasks.len(), "task count")?,
            independent_family_count: usize_to_u64(families.len(), "family count")?,
        })
    }

    pub fn corpus_sha256(&self) -> Result<String, EvalError> {
        if self.schema_version == AGENT_MANAGED_TASK_SCHEMA_V1 {
            let mut inputs = self
                .tasks
                .iter()
                .map(|task| {
                    (
                        &task.key,
                        &task.formation_prompt,
                        &task.recall_prompt,
                        &task.independent_probe,
                        &task.seed_episodes,
                    )
                })
                .collect::<Vec<_>>();
            inputs.sort_by(|left, right| left.0.cmp(right.0));
            return Ok(sha256(&serde_json::to_vec(&inputs)?));
        }
        let mut inputs = self
            .tasks
            .iter()
            .map(|task| {
                (
                    &task.key,
                    &task.formation_prompt,
                    &task.candidate_formation_prompt,
                    &task.candidate_operator_imports,
                    &task.recall_prompt,
                    &task.independent_probe,
                    &task.seed_episodes,
                )
            })
            .collect::<Vec<_>>();
        inputs.sort_by(|left, right| left.0.cmp(right.0));
        Ok(sha256(&serde_json::to_vec(&inputs)?))
    }

    pub fn oracle_sha256(&self) -> Result<String, EvalError> {
        let mut oracles = self
            .tasks
            .iter()
            .map(|task| (&task.key, &task.target_write_event_id, &task.oracle))
            .collect::<Vec<_>>();
        oracles.sort_by(|left, right| left.0.cmp(right.0));
        Ok(sha256(&serde_json::to_vec(&oracles)?))
    }
}

fn validate_task(task: &AgentManagedTaskDefinition) -> Result<(), EvalError> {
    let input = task.runner_input();
    validate_input(&input, &task.independent_probe)?;
    let mut event_ids = task
        .seed_episodes
        .iter()
        .map(|episode| episode.event_id.as_str())
        .collect::<BTreeSet<_>>();
    let import_event_ids = task
        .candidate_operator_imports
        .iter()
        .map(|fixture| fixture.event_id.as_str())
        .collect::<BTreeSet<_>>();
    if task.target_write_event_id.trim().is_empty()
        || import_event_ids.len() != task.candidate_operator_imports.len()
        || (import_event_ids.is_empty() && !event_ids.insert(task.target_write_event_id.as_str()))
        || (!import_event_ids.is_empty()
            && (!import_event_ids.contains(task.target_write_event_id.as_str())
                || import_event_ids
                    .iter()
                    .any(|event_id| !event_ids.insert(event_id))))
        || task.oracle.accepted_answers.is_empty()
        || task.oracle.sufficient_evidence_alternatives.is_empty()
        || task
            .oracle
            .sufficient_evidence_alternatives
            .iter()
            .any(|alternative| {
                alternative.is_empty()
                    || alternative.iter().any(|group| {
                        group.is_empty()
                            || group.iter().any(|event_id| {
                                !event_ids.contains(event_id.as_str())
                                    || task.oracle.forbidden_event_ids.contains(event_id)
                            })
                    })
            })
        || task
            .oracle
            .forbidden_event_ids
            .iter()
            .any(|event_id| !event_ids.contains(event_id.as_str()))
        || !task
            .oracle
            .sufficient_evidence_alternatives
            .iter()
            .flatten()
            .any(|group| {
                group
                    .iter()
                    .any(|event_id| event_id == &task.target_write_event_id)
            })
    {
        return Err(EvalError::InvalidInput(
            "agent-managed task oracle must use known, non-forbidden evidence and include the target write".into(),
        ));
    }
    Ok(())
}

/// The fixed query is passed only to the independent probe after both agent sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagedProbeRequest {
    pub query: String,
    pub valid_at: DateTime<Utc>,
    pub context_budget: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagedProbeObservation {
    pub source_episode_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_revision: Option<i64>,
    pub retained_store_bytes: u64,
    pub request_sha256: String,
    pub response_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentManagedTrialResult {
    pub formation: AgentObservation,
    pub recall: AgentObservation,
    pub independent_probe: AgentManagedProbeObservation,
    pub initial_episode_id_to_event_id: BTreeMap<String, String>,
    pub operator_imports: Vec<AgentManagedOperatorImportReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagedRunSupplement {
    pub schema_version: String,
    pub arm: RunArm,
    pub run_id: String,
    pub identity_sha256: String,
    pub expected_trials: Vec<CaseTrialKey>,
    pub stages: Vec<AgentManagedStageReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagedWriteTotals {
    pub attempts: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub unresolved: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagedArmSummary {
    pub expected_trials: u64,
    pub formation_status_counts: BTreeMap<String, u64>,
    pub formation_turns_total: u64,
    pub formation_tool_calls_total: u64,
    pub formation_user_clarifications_total: u64,
    pub formation_input_tokens_total: u64,
    pub formation_output_tokens_total: u64,
    pub formation_wall_time_ms_total: u64,
    pub retained_store_bytes_total: u64,
    pub write_operations: BTreeMap<String, AgentManagedWriteTotals>,
    pub independent_probe_successes: u64,
    pub independent_probe_trials: u64,
    pub unsupported_source_total: u64,
    pub prohibited_source_total: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub operator_import_fixtures_total: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub operator_setup_actions_total: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub operator_setup_failures_total: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub operator_setup_wall_time_ms_total: u64,
    /// Missing on legacy artifacts that predate fact-evidence telemetry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact_evidence_audit: Option<AgentFactEvidenceAudit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagedProbePairCounts {
    pub paired_trials: u64,
    pub both_succeeded: u64,
    pub incumbent_only_succeeded: u64,
    pub candidate_only_succeeded: u64,
    pub neither_succeeded: u64,
}

/// Descriptive, reproducible summary. It deliberately contains no promotion decision:
/// a live agent pilot is not a substitute for preregistered product criteria.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagedPairedComparison {
    pub schema_version: String,
    pub plan_id: String,
    pub plan_sha256: String,
    pub run_id: String,
    pub incumbent_artifact_sha256: String,
    pub candidate_artifact_sha256: String,
    pub incumbent_stage_sha256: String,
    pub candidate_stage_sha256: String,
    pub paired_agent_comparison: crate::paired::PairedRunComparison,
    pub incumbent: AgentManagedArmSummary,
    pub candidate: AgentManagedArmSummary,
    pub independent_probe_pairs: AgentManagedProbePairCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagedStageReceipt {
    pub key: CaseTrialKey,
    pub formation: AgentManagedPhaseSummary,
    pub write_operations: Vec<AgentManagedWriteOperation>,
    pub successful_ingest_count: usize,
    pub identified_ingest_episode_count: usize,
    pub independent_probe: AgentManagedProbeReceipt,
    /// Candidate-side setup actions, kept separate from agent tool calls.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operator_imports: Vec<AgentManagedOperatorImportReceipt>,
    /// Content-free proof of whether extract evidence was requested and
    /// returned before any invalidation. Missing on legacy artifacts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact_evidence_audit: Option<AgentFactEvidenceAudit>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentFactEvidenceAudit {
    /// One for a current stage receipt; aggregates report the number of
    /// receipts with telemetry, allowing legacy captures to remain distinct.
    pub telemetry_trials: u64,
    pub extract_calls: u64,
    pub evidence_requests: u64,
    pub evidence_responses: u64,
    pub evidence_before_invalidation: u64,
    /// Missing from legacy receipts, preserving their replay serialization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_sizes: Option<AgentFactEvidenceSizeAudit>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentFactEvidenceSizeAudit {
    /// Count, sum and maximum of serialized extract responses. Values are
    /// content-free even when the response carries sensitive evidence.
    pub measured_responses: u64,
    pub total_wire_bytes: u64,
    pub maximum_wire_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagedPhaseSummary {
    pub status: crate::paired::TrialStatus,
    /// Content-free normalized reason for a terminal phase failure. Missing on
    /// legacy stage artifacts and when a phase completed without a failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_error_class: Option<crate::paired::NormalizedErrorClass>,
    /// HTTP status from a non-success model-provider response. The response
    /// body and headers are discarded; missing on legacy stage artifacts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_http_status_code: Option<u16>,
    /// Content-free location where the provider request failed. New in v5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_transport_stage: Option<super::TerminalTransportStage>,
    pub turns_used: u32,
    pub tool_calls: u32,
    pub user_clarifications: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub wall_time_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagedWriteOperation {
    pub tool_name: String,
    pub attempts: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub unresolved: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagedProbeReceipt {
    pub request_sha256: String,
    pub response_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_revision: Option<i64>,
    pub retained_store_bytes: u64,
    pub source_event_ids: Vec<String>,
    pub unsupported_source_count: usize,
    pub prohibited_source_count: usize,
    pub evidence_success: bool,
}

/// A backend owns one isolated per-trial store for the entire protocol.
/// `recall_in_fresh_session` must start a new agent process against that store.
pub trait AgentManagedTrialBackend {
    type State;

    fn start_trial(
        &mut self,
        arm: RunArm,
        task: &AgentManagedTaskInput,
    ) -> Result<(Self::State, BTreeMap<String, String>), EvalError>;

    /// Reports evaluator-owned operator actions completed before agent access.
    fn operator_import_receipts(
        &self,
        _state: &Self::State,
    ) -> Vec<AgentManagedOperatorImportReceipt> {
        Vec::new()
    }

    fn form_memory(
        &mut self,
        state: &mut Self::State,
        prompt: &str,
    ) -> Result<AgentObservation, EvalError>;

    fn recall_in_fresh_session(
        &mut self,
        state: &mut Self::State,
        prompt: &str,
    ) -> Result<AgentObservation, EvalError>;

    fn independent_probe(
        &mut self,
        state: &mut Self::State,
        probe: &AgentManagedProbeRequest,
    ) -> Result<AgentManagedProbeObservation, EvalError>;
}

pub fn run_agent_managed_trial<B: AgentManagedTrialBackend>(
    backend: &mut B,
    arm: RunArm,
    task: &AgentManagedTaskInput,
    probe: &AgentManagedProbeRequest,
) -> Result<AgentManagedTrialResult, EvalError> {
    validate_input(task, probe)?;
    let (mut state, initial_episode_id_to_event_id) = backend.start_trial(arm, task)?;
    let operator_imports = backend.operator_import_receipts(&state);
    validate_operator_import_receipts(task, arm, &operator_imports)?;
    validate_episode_bindings(
        task,
        arm,
        &operator_imports,
        &initial_episode_id_to_event_id,
    )?;

    let formation_prompt = if arm == RunArm::Candidate {
        task.candidate_formation_prompt
            .as_deref()
            .unwrap_or(&task.formation_prompt)
    } else {
        &task.formation_prompt
    };
    let formation = backend.form_memory(&mut state, formation_prompt)?;
    let recall = backend.recall_in_fresh_session(&mut state, &task.recall_prompt)?;
    let independent_probe = backend.independent_probe(&mut state, probe)?;
    validate_probe_observation(&independent_probe)?;

    Ok(AgentManagedTrialResult {
        formation,
        recall,
        independent_probe,
        initial_episode_id_to_event_id,
        operator_imports,
    })
}

pub fn run_agent_managed_paired_evaluation<B: AgentManagedTrialBackend>(
    plan: &PairedEvaluationPlan,
    manifest: &AgentManagedTaskManifest,
    backend: &mut B,
    output_directory: &std::path::Path,
) -> Result<
    (
        crate::paired::PairedRunArtifact,
        crate::paired::PairedRunArtifact,
        AgentManagedRunSupplement,
        AgentManagedRunSupplement,
    ),
    EvalError,
> {
    manifest.validate(plan)?;
    for name in [
        "agent-managed-incumbent.json",
        "agent-managed-candidate.json",
    ] {
        if output_directory.join(name).exists() {
            return Err(EvalError::InvalidConfig(format!(
                "agent-managed output path already exists: {}",
                output_directory.join(name).display()
            )));
        }
    }

    let paired_manifest = super::PairedTaskManifest {
        schema_version: super::PAIRED_TASK_MANIFEST_SCHEMA_V1.into(),
        tasks: manifest
            .tasks
            .iter()
            .map(|task| super::PairedTaskDefinition {
                key: task.key.clone(),
                prompt: task.recall_prompt.clone(),
                knowledge_before_event_id: None,
                seed_episodes: task.seed_episodes.clone(),
                oracle: task.oracle.clone(),
            })
            .collect(),
    };
    let mut managed_backend = ManagedPairedBackend {
        manifest,
        backend,
        incumbent_stages: Vec::with_capacity(manifest.tasks.len()),
        candidate_stages: Vec::with_capacity(manifest.tasks.len()),
    };
    let (incumbent, candidate) = super::run_paired_agent_evaluation_validated(
        plan,
        &paired_manifest,
        &mut managed_backend,
        output_directory,
    )?;
    let incumbent_supplement = create_supplement(&incumbent, managed_backend.incumbent_stages)?;
    let candidate_supplement = create_supplement(&candidate, managed_backend.candidate_stages)?;
    validate_supplement(&incumbent_supplement, &incumbent)?;
    validate_supplement(&candidate_supplement, &candidate)?;
    super::write_json_new(
        &output_directory.join("agent-managed-incumbent.json"),
        &incumbent_supplement,
    )?;
    super::write_json_new(
        &output_directory.join("agent-managed-candidate.json"),
        &candidate_supplement,
    )?;
    Ok((
        incumbent,
        candidate,
        incumbent_supplement,
        candidate_supplement,
    ))
}

impl AgentManagedRunSupplement {
    /// Hashes the logical sidecar after sorting its order-insensitive collections.
    pub fn sha256(&self) -> Result<String, EvalError> {
        let mut canonical = self.clone();
        canonical.expected_trials.sort();
        canonical
            .stages
            .sort_by(|left, right| left.key.cmp(&right.key));
        Ok(sha256(&serde_json::to_vec(&canonical)?))
    }
}

pub fn compare_agent_managed_runs(
    plan: &PairedEvaluationPlan,
    incumbent: &crate::paired::PairedRunArtifact,
    candidate: &crate::paired::PairedRunArtifact,
    incumbent_stages: &AgentManagedRunSupplement,
    candidate_stages: &AgentManagedRunSupplement,
) -> Result<AgentManagedPairedComparison, EvalError> {
    let paired_agent_comparison = crate::paired::compare_paired_runs(incumbent, candidate, plan)?;
    validate_supplement(incumbent_stages, incumbent)?;
    validate_supplement(candidate_stages, candidate)?;
    if incumbent_stages.arm != RunArm::Incumbent || candidate_stages.arm != RunArm::Candidate {
        return Err(EvalError::InvalidInput(
            "agent-managed sidecars are attached to the wrong paired arm".into(),
        ));
    }
    let incumbent_by_key = stages_by_key(incumbent_stages)?;
    let candidate_by_key = stages_by_key(candidate_stages)?;
    let mut probe_pairs = AgentManagedProbePairCounts {
        paired_trials: 0,
        both_succeeded: 0,
        incumbent_only_succeeded: 0,
        candidate_only_succeeded: 0,
        neither_succeeded: 0,
    };
    for key in &plan.expected_trials {
        let left = incumbent_by_key.get(key).ok_or_else(|| {
            EvalError::InvalidInput("incumbent stage sidecar lacks a paired trial".into())
        })?;
        let right = candidate_by_key.get(key).ok_or_else(|| {
            EvalError::InvalidInput("candidate stage sidecar lacks a paired trial".into())
        })?;
        probe_pairs.paired_trials = checked_add(probe_pairs.paired_trials, 1, "probe trial count")?;
        match (
            left.independent_probe.evidence_success,
            right.independent_probe.evidence_success,
        ) {
            (true, true) => {
                probe_pairs.both_succeeded =
                    checked_add(probe_pairs.both_succeeded, 1, "paired probe successes")?;
            }
            (true, false) => {
                probe_pairs.incumbent_only_succeeded = checked_add(
                    probe_pairs.incumbent_only_succeeded,
                    1,
                    "incumbent-only probe successes",
                )?;
            }
            (false, true) => {
                probe_pairs.candidate_only_succeeded = checked_add(
                    probe_pairs.candidate_only_succeeded,
                    1,
                    "candidate-only probe successes",
                )?;
            }
            (false, false) => {
                probe_pairs.neither_succeeded =
                    checked_add(probe_pairs.neither_succeeded, 1, "paired probe failures")?;
            }
        }
    }
    let classified_pairs = checked_add(
        checked_add(
            checked_add(
                probe_pairs.both_succeeded,
                probe_pairs.incumbent_only_succeeded,
                "paired probe outcomes",
            )?,
            probe_pairs.candidate_only_succeeded,
            "paired probe outcomes",
        )?,
        probe_pairs.neither_succeeded,
        "paired probe outcomes",
    )?;
    if classified_pairs != probe_pairs.paired_trials {
        return Err(EvalError::InvalidInput(
            "agent-managed post-write probe pair counts do not cover the paired trials".into(),
        ));
    }
    let incumbent_summary = summarize_stages(incumbent_stages)?;
    let candidate_summary = summarize_stages(candidate_stages)?;
    let schema_version = if candidate_summary.operator_import_fixtures_total > 0
        || incumbent_summary.operator_import_fixtures_total > 0
    {
        AGENT_MANAGED_COMPARISON_SCHEMA_V2
    } else {
        AGENT_MANAGED_COMPARISON_SCHEMA_V1
    };
    Ok(AgentManagedPairedComparison {
        schema_version: schema_version.into(),
        plan_id: plan.plan_id.clone(),
        plan_sha256: plan.sha256()?,
        run_id: paired_agent_comparison
            .run_id
            .clone()
            .ok_or_else(|| EvalError::InvalidInput("paired evaluation lacks a run ID".into()))?,
        incumbent_artifact_sha256: incumbent.sha256()?,
        candidate_artifact_sha256: candidate.sha256()?,
        incumbent_stage_sha256: incumbent_stages.sha256()?,
        candidate_stage_sha256: candidate_stages.sha256()?,
        paired_agent_comparison,
        incumbent: incumbent_summary,
        candidate: candidate_summary,
        independent_probe_pairs: probe_pairs,
    })
}

fn stages_by_key(
    supplement: &AgentManagedRunSupplement,
) -> Result<BTreeMap<&CaseTrialKey, &AgentManagedStageReceipt>, EvalError> {
    let mut stages = BTreeMap::new();
    for stage in &supplement.stages {
        if stages.insert(&stage.key, stage).is_some() {
            return Err(EvalError::InvalidInput(
                "agent-managed sidecar contains a duplicate trial key".into(),
            ));
        }
    }
    Ok(stages)
}

fn summarize_stages(
    supplement: &AgentManagedRunSupplement,
) -> Result<AgentManagedArmSummary, EvalError> {
    let mut summary = AgentManagedArmSummary {
        expected_trials: 0,
        formation_status_counts: BTreeMap::new(),
        formation_turns_total: 0,
        formation_tool_calls_total: 0,
        formation_user_clarifications_total: 0,
        formation_input_tokens_total: 0,
        formation_output_tokens_total: 0,
        formation_wall_time_ms_total: 0,
        retained_store_bytes_total: 0,
        write_operations: BTreeMap::new(),
        independent_probe_successes: 0,
        independent_probe_trials: 0,
        unsupported_source_total: 0,
        prohibited_source_total: 0,
        operator_import_fixtures_total: 0,
        operator_setup_actions_total: 0,
        operator_setup_failures_total: 0,
        operator_setup_wall_time_ms_total: 0,
        fact_evidence_audit: None,
    };
    for stage in &supplement.stages {
        summary.expected_trials = checked_add(summary.expected_trials, 1, "trial count")?;
        let status = format!("{:?}", stage.formation.status).to_ascii_lowercase();
        increment_count(&mut summary.formation_status_counts, status)?;
        summary.formation_turns_total = checked_add(
            summary.formation_turns_total,
            u64::from(stage.formation.turns_used),
            "formation turns",
        )?;
        summary.formation_tool_calls_total = checked_add(
            summary.formation_tool_calls_total,
            u64::from(stage.formation.tool_calls),
            "formation tool calls",
        )?;
        summary.formation_user_clarifications_total = checked_add(
            summary.formation_user_clarifications_total,
            u64::from(stage.formation.user_clarifications),
            "formation clarifications",
        )?;
        summary.formation_input_tokens_total = checked_add(
            summary.formation_input_tokens_total,
            stage.formation.input_tokens,
            "formation input tokens",
        )?;
        summary.formation_output_tokens_total = checked_add(
            summary.formation_output_tokens_total,
            stage.formation.output_tokens,
            "formation output tokens",
        )?;
        summary.formation_wall_time_ms_total = checked_add(
            summary.formation_wall_time_ms_total,
            stage.formation.wall_time_ms,
            "formation wall time",
        )?;
        summary.operator_import_fixtures_total = checked_add(
            summary.operator_import_fixtures_total,
            usize_to_u64(stage.operator_imports.len(), "operator import fixtures")?,
            "operator import fixtures",
        )?;
        summary.operator_setup_actions_total = checked_add(
            summary.operator_setup_actions_total,
            stage
                .operator_imports
                .iter()
                .try_fold(0_u64, |total, receipt| {
                    checked_add(
                        total,
                        1 + u64::from(receipt.extraction_attempted),
                        "operator setup actions",
                    )
                })?,
            "operator setup actions",
        )?;
        for receipt in &stage.operator_imports {
            let failed_actions = u64::from(!receipt.import_succeeded)
                + u64::from(receipt.extraction_attempted && !receipt.extraction_succeeded);
            summary.operator_setup_failures_total = checked_add(
                summary.operator_setup_failures_total,
                failed_actions,
                "operator setup failures",
            )?;
            summary.operator_setup_wall_time_ms_total = checked_add(
                summary.operator_setup_wall_time_ms_total,
                receipt.elapsed_ms,
                "operator setup wall time",
            )?;
        }
        summary.independent_probe_trials =
            checked_add(summary.independent_probe_trials, 1, "probe trials")?;
        if let Some(stage_audit) = stage.fact_evidence_audit {
            let audit = summary
                .fact_evidence_audit
                .get_or_insert_with(AgentFactEvidenceAudit::default);
            audit.telemetry_trials = checked_add(
                audit.telemetry_trials,
                stage_audit.telemetry_trials,
                "evidence telemetry trials",
            )?;
            audit.extract_calls = checked_add(
                audit.extract_calls,
                stage_audit.extract_calls,
                "evidence extract calls",
            )?;
            audit.evidence_requests = checked_add(
                audit.evidence_requests,
                stage_audit.evidence_requests,
                "evidence requests",
            )?;
            audit.evidence_responses = checked_add(
                audit.evidence_responses,
                stage_audit.evidence_responses,
                "evidence responses",
            )?;
            audit.evidence_before_invalidation = checked_add(
                audit.evidence_before_invalidation,
                stage_audit.evidence_before_invalidation,
                "evidence before invalidation",
            )?;
            if let Some(stage_sizes) = stage_audit.response_sizes {
                let sizes = audit
                    .response_sizes
                    .get_or_insert_with(AgentFactEvidenceSizeAudit::default);
                sizes.measured_responses = checked_add(
                    sizes.measured_responses,
                    stage_sizes.measured_responses,
                    "measured extract response sizes",
                )?;
                sizes.total_wire_bytes = checked_add(
                    sizes.total_wire_bytes,
                    stage_sizes.total_wire_bytes,
                    "extract response bytes",
                )?;
                sizes.maximum_wire_bytes =
                    sizes.maximum_wire_bytes.max(stage_sizes.maximum_wire_bytes);
            }
        }
        if stage.independent_probe.evidence_success {
            summary.independent_probe_successes =
                checked_add(summary.independent_probe_successes, 1, "probe successes")?;
        }
        summary.unsupported_source_total = checked_add(
            summary.unsupported_source_total,
            usize_to_u64(
                stage.independent_probe.unsupported_source_count,
                "unsupported sources",
            )?,
            "unsupported sources",
        )?;
        summary.retained_store_bytes_total = checked_add(
            summary.retained_store_bytes_total,
            stage.independent_probe.retained_store_bytes,
            "retained store bytes",
        )?;
        summary.prohibited_source_total = checked_add(
            summary.prohibited_source_total,
            usize_to_u64(
                stage.independent_probe.prohibited_source_count,
                "prohibited sources",
            )?,
            "prohibited sources",
        )?;
        for operation in &stage.write_operations {
            let totals = summary
                .write_operations
                .entry(operation.tool_name.clone())
                .or_insert(AgentManagedWriteTotals {
                    attempts: 0,
                    succeeded: 0,
                    failed: 0,
                    unresolved: 0,
                });
            totals.attempts = checked_add(
                totals.attempts,
                usize_to_u64(operation.attempts, "write attempts")?,
                "write attempts",
            )?;
            totals.succeeded = checked_add(
                totals.succeeded,
                usize_to_u64(operation.succeeded, "successful writes")?,
                "successful writes",
            )?;
            totals.failed = checked_add(
                totals.failed,
                usize_to_u64(operation.failed, "failed writes")?,
                "failed writes",
            )?;
            totals.unresolved = checked_add(
                totals.unresolved,
                usize_to_u64(operation.unresolved, "unresolved writes")?,
                "unresolved writes",
            )?;
        }
    }
    Ok(summary)
}

fn increment_count(counts: &mut BTreeMap<String, u64>, key: String) -> Result<(), EvalError> {
    let count = counts.entry(key).or_default();
    *count = checked_add(*count, 1, "status count")?;
    Ok(())
}

fn checked_add(left: u64, right: u64, label: &str) -> Result<u64, EvalError> {
    left.checked_add(right)
        .ok_or_else(|| EvalError::InvalidInput(format!("agent-managed {label} total overflowed")))
}

fn usize_to_u64(value: usize, label: &str) -> Result<u64, EvalError> {
    u64::try_from(value).map_err(|_| {
        EvalError::InvalidInput(format!("agent-managed {label} exceed the report range"))
    })
}

struct ManagedPairedBackend<'a, B> {
    manifest: &'a AgentManagedTaskManifest,
    backend: &'a mut B,
    incumbent_stages: Vec<AgentManagedStageReceipt>,
    candidate_stages: Vec<AgentManagedStageReceipt>,
}

impl<B: AgentManagedTrialBackend> super::PairedTrialBackend for ManagedPairedBackend<'_, B> {
    fn execute(
        &mut self,
        arm: RunArm,
        prepared_task: &super::PreparedPairedTask,
    ) -> Result<super::PairedTrialExecution, EvalError> {
        let definition = self
            .manifest
            .tasks
            .iter()
            .find(|task| task.key == prepared_task.input.key)
            .ok_or_else(|| {
                EvalError::InvalidInput("managed task disappeared after validation".into())
            })?;
        if prepared_task.input.prompt != definition.recall_prompt
            || prepared_task.input.seed_episodes != definition.seed_episodes
        {
            return Err(EvalError::InvalidInput(
                "paired recall input differs from the registered agent-managed task".into(),
            ));
        }
        let result = run_agent_managed_trial(
            self.backend,
            arm,
            &definition.runner_input(),
            &definition.independent_probe,
        )?;
        let write_operations = count_write_operations(&result.formation);
        let successful_ingest_count = count_successful_ingest_operations(
            &write_operations,
            result.formation.memory_write_episode_ids.len(),
        );
        let generated_episode_id = (successful_ingest_count == 1
            && result.formation.memory_write_episode_ids.len() == 1)
            .then(|| result.formation.memory_write_episode_ids[0].clone());
        let mut bindings = result.initial_episode_id_to_event_id;
        if let Some(episode_id) = generated_episode_id.as_ref() {
            if bindings.contains_key(episode_id) {
                return Err(EvalError::InvalidInput(
                    "agent write reused an existing episode ID".into(),
                ));
            }
            if bindings
                .values()
                .any(|event_id| event_id == &definition.target_write_event_id)
            {
                return Err(EvalError::InvalidInput(
                    "candidate agent repeated a target source already imported by operator setup"
                        .into(),
                ));
            }
            bindings.insert(episode_id.clone(), definition.target_write_event_id.clone());
        }
        let probe_receipt = score_independent_probe(
            &result.independent_probe,
            &bindings,
            &definition.target_write_event_id,
            &definition.oracle,
        );
        let stage_receipt = AgentManagedStageReceipt {
            key: definition.key.clone(),
            formation: phase_summary(&result.formation),
            write_operations,
            successful_ingest_count,
            identified_ingest_episode_count: result.formation.memory_write_episode_ids.len(),
            independent_probe: probe_receipt,
            operator_imports: result.operator_imports,
            fact_evidence_audit: summarize_fact_evidence(&result.formation)?,
        };
        match arm {
            RunArm::Incumbent => self.incumbent_stages.push(stage_receipt),
            RunArm::Candidate => self.candidate_stages.push(stage_receipt),
        }
        Ok(super::PairedTrialExecution {
            observation: result.recall,
            episode_id_to_event_id: bindings,
        })
    }
}

fn phase_summary(observation: &AgentObservation) -> AgentManagedPhaseSummary {
    AgentManagedPhaseSummary {
        status: observation.status,
        terminal_error_class: terminal_error_class(observation),
        terminal_http_status_code: observation.terminal_http_status_code,
        terminal_transport_stage: observation.terminal_transport_stage,
        turns_used: observation.turns_used,
        tool_calls: observation.tool_calls,
        user_clarifications: observation.user_clarifications,
        input_tokens: observation.input_tokens,
        output_tokens: observation.output_tokens,
        wall_time_ms: observation.wall_time_ms,
    }
}

fn terminal_error_class(
    observation: &AgentObservation,
) -> Option<crate::paired::NormalizedErrorClass> {
    let terminal_kind = match observation.status {
        crate::paired::TrialStatus::ModelFailure => crate::paired::TraceEventKind::ModelFailure,
        crate::paired::TrialStatus::TimedOut => crate::paired::TraceEventKind::Timeout,
        crate::paired::TrialStatus::ToolFailure => crate::paired::TraceEventKind::ToolError,
        crate::paired::TrialStatus::Completed
        | crate::paired::TrialStatus::BudgetExceeded
        | crate::paired::TrialStatus::Skipped => return None,
    };
    observation
        .trace
        .iter()
        .rev()
        .find(|event| event.kind == terminal_kind)
        .and_then(|event| event.error_class)
}

fn summarize_fact_evidence(
    observation: &AgentObservation,
) -> Result<Option<AgentFactEvidenceAudit>, EvalError> {
    if !observation.extract_fact_evidence_telemetry_complete {
        return Ok(None);
    }
    let extract_calls = observation
        .trace
        .iter()
        .filter(|event| {
            event.kind == crate::paired::TraceEventKind::MemoryWrite
                && event
                    .tool_name
                    .as_deref()
                    .is_some_and(|name| memory_tool_name_matches(name, "extract"))
        })
        .count();
    let invalidation_sequences = observation
        .trace
        .iter()
        .filter(|event| {
            event.kind == crate::paired::TraceEventKind::MemoryWrite
                && event
                    .tool_name
                    .as_deref()
                    .is_some_and(|name| memory_tool_name_matches(name, "invalidate"))
        })
        .map(|event| event.sequence)
        .collect::<Vec<_>>();
    let observations = &observation.extract_fact_evidence;
    let evidence_requests = observations.iter().filter(|item| item.requested).count();
    let evidence_responses = observations.iter().filter(|item| item.returned).count();
    let evidence_before_invalidation = observations
        .iter()
        .filter(|item| {
            item.returned
                && item.response_sequence.is_some_and(|response_sequence| {
                    !invalidation_sequences.is_empty()
                        && invalidation_sequences
                            .iter()
                            .all(|sequence| response_sequence < *sequence)
                })
        })
        .count();
    let response_wire_bytes = observations
        .iter()
        .filter_map(|item| item.response_wire_bytes)
        .collect::<Vec<_>>();
    let response_sizes = if response_wire_bytes.is_empty() {
        None
    } else {
        Some(AgentFactEvidenceSizeAudit {
            measured_responses: usize_to_u64(
                response_wire_bytes.len(),
                "measured extract response sizes",
            )?,
            total_wire_bytes: response_wire_bytes.iter().try_fold(0u64, |total, bytes| {
                checked_add(total, *bytes, "extract response bytes")
            })?,
            maximum_wire_bytes: response_wire_bytes.into_iter().max().unwrap_or_default(),
        })
    };
    Ok(Some(AgentFactEvidenceAudit {
        telemetry_trials: 1,
        extract_calls: usize_to_u64(extract_calls, "extract evidence calls")?,
        evidence_requests: usize_to_u64(evidence_requests, "evidence requests")?,
        evidence_responses: usize_to_u64(evidence_responses, "evidence responses")?,
        evidence_before_invalidation: usize_to_u64(
            evidence_before_invalidation,
            "evidence before invalidation",
        )?,
        response_sizes,
    }))
}

fn count_write_operations(observation: &AgentObservation) -> Vec<AgentManagedWriteOperation> {
    #[derive(Default)]
    struct Counts {
        attempts: usize,
        succeeded: usize,
        failed: usize,
        unresolved: usize,
    }
    let mut counts = BTreeMap::<String, Counts>::new();
    for write in observation
        .trace
        .iter()
        .filter(|event| event.kind == crate::paired::TraceEventKind::MemoryWrite)
    {
        let observed_tool_name = write.tool_name.as_deref().unwrap_or("unknown");
        let tool_name = canonical_write_tool_name(observed_tool_name);
        let entry = counts.entry(tool_name).or_default();
        entry.attempts = entry.attempts.saturating_add(1);
        let outcome = observation.trace.iter().find(|event| {
            event.call_id == write.call_id
                && event.tool_name.as_deref() == Some(observed_tool_name)
                && matches!(
                    event.kind,
                    crate::paired::TraceEventKind::ToolResponse
                        | crate::paired::TraceEventKind::ToolError
                )
        });
        match outcome.map(|event| event.kind) {
            Some(crate::paired::TraceEventKind::ToolResponse) => {
                entry.succeeded = entry.succeeded.saturating_add(1);
            }
            Some(crate::paired::TraceEventKind::ToolError) => {
                entry.failed = entry.failed.saturating_add(1);
            }
            _ => entry.unresolved = entry.unresolved.saturating_add(1),
        }
    }
    counts
        .into_iter()
        .map(|(tool_name, counts)| AgentManagedWriteOperation {
            tool_name,
            attempts: counts.attempts,
            succeeded: counts.succeeded,
            failed: counts.failed,
            unresolved: counts.unresolved,
        })
        .collect()
}

fn memory_tool_name_matches(observed: &str, expected: &str) -> bool {
    observed == expected || observed.strip_prefix("memory_decisions/") == Some(expected)
}

fn canonical_write_tool_name(observed: &str) -> String {
    let unqualified = observed
        .strip_prefix("memory_decisions/")
        .unwrap_or(observed);
    match unqualified {
        "ingest" | "extract" | "invalidate" => format!("memory_decisions/{unqualified}"),
        _ => observed.to_owned(),
    }
}

fn count_successful_ingest_operations(
    operations: &[AgentManagedWriteOperation],
    identified_episode_count: usize,
) -> usize {
    let successes = |name: &str| {
        operations
            .iter()
            .find(|operation| operation.tool_name == name)
            .map_or(0, |operation| operation.succeeded)
    };
    let ingest_successes = successes("memory_decisions/ingest");
    let inline_extract_ids = identified_episode_count.saturating_sub(ingest_successes);
    ingest_successes.saturating_add(inline_extract_ids.min(successes("memory_decisions/extract")))
}

fn score_independent_probe(
    observation: &AgentManagedProbeObservation,
    bindings: &BTreeMap<String, String>,
    target_event_id: &str,
    oracle: &HiddenTaskOracle,
) -> AgentManagedProbeReceipt {
    let mut source_event_ids = Vec::new();
    let mut unsupported_source_count = 0_usize;
    for episode_id in &observation.source_episode_ids {
        match bindings.get(episode_id) {
            Some(event_id) => source_event_ids.push(event_id.clone()),
            None => unsupported_source_count = unsupported_source_count.saturating_add(1),
        }
    }
    let prohibited_source_count = source_event_ids
        .iter()
        .filter(|event_id| oracle.forbidden_event_ids.contains(event_id))
        .count();
    let observed = source_event_ids.iter().collect::<BTreeSet<_>>();
    let evidence_success = unsupported_source_count == 0
        && prohibited_source_count == 0
        && source_event_ids
            .iter()
            .any(|event_id| event_id == target_event_id)
        && oracle
            .sufficient_evidence_alternatives
            .iter()
            .any(|alternative| {
                !alternative.is_empty()
                    && alternative
                        .iter()
                        .all(|group| group.iter().any(|event_id| observed.contains(event_id)))
            });
    AgentManagedProbeReceipt {
        request_sha256: observation.request_sha256.clone(),
        response_sha256: observation.response_sha256.clone(),
        known_revision: observation.known_revision,
        retained_store_bytes: observation.retained_store_bytes,
        source_event_ids,
        unsupported_source_count,
        prohibited_source_count,
        evidence_success,
    }
}

fn create_supplement(
    run: &crate::paired::PairedRunArtifact,
    mut stages: Vec<AgentManagedStageReceipt>,
) -> Result<AgentManagedRunSupplement, EvalError> {
    let mut expected_trials = run.expected_trials.clone();
    expected_trials.sort();
    stages.sort_by(|left, right| left.key.cmp(&right.key));
    Ok(AgentManagedRunSupplement {
        schema_version: AGENT_MANAGED_ARTIFACT_SCHEMA_V5.into(),
        arm: run.arm,
        run_id: run
            .run_id
            .clone()
            .ok_or_else(|| EvalError::InvalidInput("paired run lacks a run ID".into()))?,
        identity_sha256: sha256(&serde_json::to_vec(&run.identity)?),
        expected_trials,
        stages,
    })
}

fn validate_supplement(
    supplement: &AgentManagedRunSupplement,
    run: &crate::paired::PairedRunArtifact,
) -> Result<(), EvalError> {
    let mut expected = run.expected_trials.clone();
    expected.sort();
    let mut observed = supplement
        .stages
        .iter()
        .map(|stage| stage.key.clone())
        .collect::<Vec<_>>();
    observed.sort();
    let has_operator_imports = supplement
        .stages
        .iter()
        .any(|stage| !stage.operator_imports.is_empty());
    let has_terminal_error_classes = supplement
        .stages
        .iter()
        .any(|stage| stage.formation.terminal_error_class.is_some());
    let has_terminal_http_status_codes = supplement
        .stages
        .iter()
        .any(|stage| stage.formation.terminal_http_status_code.is_some());
    let has_terminal_transport_stages = supplement
        .stages
        .iter()
        .any(|stage| stage.formation.terminal_transport_stage.is_some());
    let schema_matches = match supplement.schema_version.as_str() {
        AGENT_MANAGED_ARTIFACT_SCHEMA_V1 => {
            !has_operator_imports
                && !has_terminal_error_classes
                && !has_terminal_http_status_codes
                && !has_terminal_transport_stages
        }
        AGENT_MANAGED_ARTIFACT_SCHEMA_V2 => {
            has_operator_imports
                && !has_terminal_error_classes
                && !has_terminal_http_status_codes
                && !has_terminal_transport_stages
        }
        AGENT_MANAGED_ARTIFACT_SCHEMA_V3 => {
            !has_terminal_http_status_codes && !has_terminal_transport_stages
        }
        AGENT_MANAGED_ARTIFACT_SCHEMA_V4 => !has_terminal_transport_stages,
        AGENT_MANAGED_ARTIFACT_SCHEMA_V5 => true,
        _ => false,
    };
    if !schema_matches
        || supplement.arm != run.arm
        || Some(&supplement.run_id) != run.run_id.as_ref()
        || supplement.identity_sha256 != sha256(&serde_json::to_vec(&run.identity)?)
        || supplement.expected_trials != expected
        || observed != expected
    {
        return Err(EvalError::InvalidInput(
            "agent-managed stage supplement does not match its paired run".into(),
        ));
    }
    for stage in &supplement.stages {
        validate_stage_receipt(stage, &run.identity.shared)?;
    }
    Ok(())
}

fn validate_stage_receipt(
    stage: &AgentManagedStageReceipt,
    identity: &crate::paired::SharedRunIdentity,
) -> Result<(), EvalError> {
    let mut tool_names = BTreeSet::new();
    let expected_ingest_successes = count_successful_ingest_operations(
        &stage.write_operations,
        stage.identified_ingest_episode_count,
    );
    let malformed_write = stage.successful_ingest_count != expected_ingest_successes
        || stage.identified_ingest_episode_count > stage.successful_ingest_count
        || stage.write_operations.iter().any(|operation| {
            !tool_names.insert(operation.tool_name.as_str())
                || operation.tool_name.trim().is_empty()
                || operation
                    .succeeded
                    .checked_add(operation.failed)
                    .and_then(|total| total.checked_add(operation.unresolved))
                    != Some(operation.attempts)
        });
    let extract_attempts = stage
        .write_operations
        .iter()
        .find(|operation| operation.tool_name == "memory_decisions/extract")
        .map_or(0, |operation| operation.attempts);
    let malformed_fact_evidence = stage.fact_evidence_audit.is_some_and(|audit| {
        audit.telemetry_trials != 1
            || usize::try_from(audit.extract_calls).ok() != Some(extract_attempts)
            || audit.evidence_requests > audit.extract_calls
            || audit.evidence_responses > audit.evidence_requests
            || audit.evidence_before_invalidation > audit.evidence_responses
            || audit.response_sizes.is_some_and(|sizes| {
                sizes.measured_responses > audit.extract_calls
                    || (sizes.measured_responses == 0
                        && (sizes.total_wire_bytes != 0 || sizes.maximum_wire_bytes != 0))
                    || sizes.maximum_wire_bytes > sizes.total_wire_bytes
            })
    });
    let malformed_terminal_error = stage.formation.terminal_error_class.is_some()
        && !matches!(
            stage.formation.status,
            crate::paired::TrialStatus::ModelFailure
                | crate::paired::TrialStatus::TimedOut
                | crate::paired::TrialStatus::ToolFailure
        );
    let malformed_terminal_http_status =
        stage
            .formation
            .terminal_http_status_code
            .is_some_and(|status| {
                !(100..=599).contains(&status)
                    || (200..300).contains(&status)
                    || stage.formation.terminal_error_class
                        != Some(crate::paired::NormalizedErrorClass::Transport)
                    || stage.formation.status != crate::paired::TrialStatus::ModelFailure
                    || stage.formation.terminal_transport_stage.is_some()
            });
    let malformed_terminal_transport_stage =
        stage.formation.terminal_transport_stage.is_some_and(|_| {
            stage.formation.terminal_http_status_code.is_some()
                || !matches!(
                    (stage.formation.terminal_error_class, stage.formation.status),
                    (
                        Some(crate::paired::NormalizedErrorClass::Transport),
                        crate::paired::TrialStatus::ModelFailure
                    ) | (
                        Some(crate::paired::NormalizedErrorClass::Timeout),
                        crate::paired::TrialStatus::TimedOut
                    )
                )
        });
    let mut operator_fixture_ids = BTreeSet::new();
    let malformed_operator_imports = stage.operator_imports.iter().any(|receipt| {
        receipt.fixture_id.trim().is_empty()
            || !operator_fixture_ids.insert(receipt.fixture_id.as_str())
            || (receipt.extraction_succeeded && !receipt.extraction_attempted)
            || (receipt.extraction_attempted && !receipt.import_succeeded)
    });
    let probe = &stage.independent_probe;
    let mut source_event_ids = BTreeSet::new();
    let malformed_probe = probe.known_revision.is_some_and(|revision| revision < 0)
        || !valid_sha256(&probe.request_sha256)
        || !valid_sha256(&probe.response_sha256)
        || probe.source_event_ids.iter().any(|event_id| {
            event_id.trim().is_empty() || !source_event_ids.insert(event_id.as_str())
        })
        || probe.prohibited_source_count > probe.source_event_ids.len()
        || (probe.evidence_success
            && (probe.unsupported_source_count != 0
                || probe.prohibited_source_count != 0
                || probe.source_event_ids.is_empty()));
    let exceeded_work_budget = stage.formation.turns_used > identity.turn_budget
        || stage.formation.tool_calls > identity.tool_call_budget
        || stage.formation.output_tokens > identity.output_token_budget;
    let exceeded_wall_budget = stage.formation.wall_time_ms > identity.wall_time_budget_ms;
    let wrong_budget_status = (exceeded_work_budget
        && stage.formation.status != crate::paired::TrialStatus::BudgetExceeded)
        || (exceeded_wall_budget
            && !matches!(
                stage.formation.status,
                crate::paired::TrialStatus::TimedOut | crate::paired::TrialStatus::BudgetExceeded
            ));
    if malformed_write {
        return Err(EvalError::InvalidInput(
            "agent-managed phase receipt contains inconsistent write-operation metadata".into(),
        ));
    }
    if malformed_fact_evidence {
        return Err(EvalError::InvalidInput(
            "agent-managed fact-evidence telemetry is inconsistent with extraction writes".into(),
        ));
    }
    if malformed_terminal_error {
        return Err(EvalError::InvalidInput(
            "agent-managed terminal error class requires a failed formation phase".into(),
        ));
    }
    if malformed_terminal_http_status {
        return Err(EvalError::InvalidInput(
            "agent-managed HTTP status requires a failed transport response".into(),
        ));
    }
    if malformed_terminal_transport_stage {
        return Err(EvalError::InvalidInput(
            "agent-managed transport stage requires a failed send or body-read phase".into(),
        ));
    }
    if malformed_operator_imports {
        return Err(EvalError::InvalidInput(
            "agent-managed operator import receipt is inconsistent".into(),
        ));
    }
    if malformed_probe {
        return Err(EvalError::InvalidInput(
            "agent-managed phase receipt contains invalid independent-probe metadata".into(),
        ));
    }
    if wrong_budget_status {
        return Err(EvalError::InvalidInput(
            "agent-managed phase receipt status does not match its recorded budget usage".into(),
        ));
    }
    Ok(())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

fn validate_input(
    task: &AgentManagedTaskInput,
    probe: &AgentManagedProbeRequest,
) -> Result<(), EvalError> {
    if task.formation_prompt.trim().is_empty()
        || task
            .candidate_formation_prompt
            .as_deref()
            .is_some_and(|prompt| prompt.trim().is_empty())
        || task.recall_prompt.trim().is_empty()
        || probe.query.trim().is_empty()
        || probe.context_budget == 0
    {
        return Err(EvalError::InvalidInput(
            "agent-managed trial prompts and probe budget must be nonempty".into(),
        ));
    }
    let mut event_ids = BTreeSet::new();
    if task.seed_episodes.iter().any(|episode| {
        episode.event_id.trim().is_empty() || !event_ids.insert(episode.event_id.as_str())
    }) {
        return Err(EvalError::InvalidInput(
            "agent-managed seed history requires unique nonempty event IDs".into(),
        ));
    }
    let mut fixture_ids = BTreeSet::new();
    let mut fixture_paths = BTreeSet::new();
    let mut import_event_ids = BTreeSet::new();
    for fixture in &task.candidate_operator_imports {
        let valid_identifier = |value: &str| {
            !value.is_empty()
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        };
        let safe_relative_path = !fixture.relative_path.is_empty()
            && !fixture.relative_path.starts_with('/')
            && !fixture.relative_path.contains('\\')
            && !fixture.relative_path.contains(':')
            && !fixture.relative_path.contains('\0')
            && fixture
                .relative_path
                .split('/')
                .all(|component| !component.is_empty() && component != "." && component != "..");
        if !valid_identifier(&fixture.fixture_id)
            || !fixture_ids.insert(fixture.fixture_id.as_str())
            || !fixture_paths.insert(fixture.relative_path.as_str())
            || !safe_relative_path
            || fixture.content.is_empty()
            || fixture.content.len() > 4 * 1024 * 1024
            || !valid_identifier(&fixture.source_type)
            || fixture.event_id.trim().is_empty()
            || !event_ids.insert(fixture.event_id.as_str())
            || !import_event_ids.insert(fixture.event_id.as_str())
        {
            return Err(EvalError::InvalidInput(
                "agent-managed operator file fixtures require unique IDs, confined paths, supported sizes, and known event IDs".into(),
            ));
        }
    }
    if !task.candidate_operator_imports.is_empty()
        && task
            .candidate_formation_prompt
            .as_deref()
            .is_none_or(|prompt| prompt.trim().is_empty())
    {
        return Err(EvalError::InvalidInput(
            "candidate operator file imports require a candidate-specific formation prompt".into(),
        ));
    }
    Ok(())
}

fn validate_episode_bindings(
    task: &AgentManagedTaskInput,
    arm: RunArm,
    operator_imports: &[AgentManagedOperatorImportReceipt],
    bindings: &BTreeMap<String, String>,
) -> Result<(), EvalError> {
    let mut event_ids = BTreeSet::new();
    if bindings.iter().any(|(episode_id, event_id)| {
        !episode_id.starts_with("episode:")
            || episode_id.len() == "episode:".len()
            || event_id.trim().is_empty()
            || !event_ids.insert(event_id.as_str())
    }) {
        return Err(EvalError::InvalidInput(
            "agent-managed seed episode bindings are malformed or duplicate".into(),
        ));
    }
    let mut expected_event_ids = task
        .seed_episodes
        .iter()
        .map(|episode| episode.event_id.as_str())
        .collect::<BTreeSet<_>>();
    if arm == RunArm::Candidate {
        let imported_fixture_ids = operator_imports
            .iter()
            .filter(|receipt| receipt.import_succeeded)
            .map(|receipt| receipt.fixture_id.as_str())
            .collect::<BTreeSet<_>>();
        expected_event_ids.extend(
            task.candidate_operator_imports
                .iter()
                .filter(|fixture| imported_fixture_ids.contains(fixture.fixture_id.as_str()))
                .map(|fixture| fixture.event_id.as_str()),
        );
    }
    let actual_event_ids = event_ids.iter().copied().collect::<BTreeSet<_>>();
    if bindings.len() != expected_event_ids.len() || actual_event_ids != expected_event_ids {
        return Err(EvalError::InvalidInput(
            "agent-managed setup bindings do not cover the registered history".into(),
        ));
    }
    Ok(())
}

fn validate_operator_import_receipts(
    task: &AgentManagedTaskInput,
    arm: RunArm,
    receipts: &[AgentManagedOperatorImportReceipt],
) -> Result<(), EvalError> {
    let expected = if arm == RunArm::Candidate {
        task.candidate_operator_imports
            .iter()
            .map(|fixture| fixture.fixture_id.as_str())
            .collect::<BTreeSet<_>>()
    } else {
        BTreeSet::new()
    };
    let actual = receipts
        .iter()
        .map(|receipt| receipt.fixture_id.as_str())
        .collect::<BTreeSet<_>>();
    if receipts.len() != actual.len() || actual != expected {
        return Err(EvalError::InvalidInput(
            "operator import receipts do not match the registered arm setup".into(),
        ));
    }
    if receipts.iter().any(|receipt| {
        receipt.extraction_succeeded && !receipt.extraction_attempted
            || receipt.extraction_attempted && !receipt.import_succeeded
    }) {
        return Err(EvalError::InvalidInput(
            "operator import receipt reports an impossible extraction outcome".into(),
        ));
    }
    Ok(())
}

fn validate_probe_observation(observation: &AgentManagedProbeObservation) -> Result<(), EvalError> {
    let valid_sha256 =
        |value: &str| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit());
    let mut episode_ids = BTreeSet::new();
    if observation
        .known_revision
        .is_some_and(|revision| revision < 0)
        || !valid_sha256(&observation.request_sha256)
        || !valid_sha256(&observation.response_sha256)
        || observation.source_episode_ids.iter().any(|id| {
            !id.starts_with("episode:")
                || id.len() == "episode:".len()
                || !episode_ids.insert(id.as_str())
        })
    {
        return Err(EvalError::InvalidInput(
            "agent-managed post-write probe observation is malformed".into(),
        ));
    }
    Ok(())
}

pub fn sha256(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paired::TrialStatus;

    struct State {
        formed_episode_id: String,
    }

    struct ControlledBackend {
        calls: Vec<&'static str>,
    }

    impl AgentManagedTrialBackend for ControlledBackend {
        type State = State;

        fn start_trial(
            &mut self,
            _arm: RunArm,
            _task: &AgentManagedTaskInput,
        ) -> Result<(Self::State, BTreeMap<String, String>), EvalError> {
            self.calls.push("start");
            Ok((
                State {
                    formed_episode_id: "episode:formed".into(),
                },
                BTreeMap::new(),
            ))
        }

        fn form_memory(
            &mut self,
            state: &mut Self::State,
            _prompt: &str,
        ) -> Result<AgentObservation, EvalError> {
            self.calls.push("form");
            Ok(observation(vec![state.formed_episode_id.clone()]))
        }

        fn recall_in_fresh_session(
            &mut self,
            state: &mut Self::State,
            _prompt: &str,
        ) -> Result<AgentObservation, EvalError> {
            self.calls.push("fresh_recall");
            Ok(observation(vec![state.formed_episode_id.clone()]))
        }

        fn independent_probe(
            &mut self,
            state: &mut Self::State,
            _probe: &AgentManagedProbeRequest,
        ) -> Result<AgentManagedProbeObservation, EvalError> {
            self.calls.push("probe");
            Ok(AgentManagedProbeObservation {
                source_episode_ids: vec![state.formed_episode_id.clone()],
                known_revision: Some(2),
                retained_store_bytes: 4096,
                request_sha256: "a".repeat(64),
                response_sha256: "b".repeat(64),
            })
        }
    }

    fn observation(memory_source_episode_ids: Vec<String>) -> AgentObservation {
        AgentObservation {
            trace: vec![],
            final_answer: None,
            memory_source_episode_ids,
            memory_reads: None,
            read_capture_status: None,
            memory_write_episode_ids: vec!["episode:formed".into()],
            extract_fact_evidence: vec![],
            extract_fact_evidence_telemetry_complete: false,
            terminal_http_status_code: None,
            terminal_transport_stage: None,
            turns_used: 1,
            tool_calls: 1,
            user_clarifications: 0,
            input_tokens: 1,
            output_tokens: 1,
            wall_time_ms: 1,
            status: TrialStatus::Completed,
        }
    }

    fn task() -> AgentManagedTaskInput {
        AgentManagedTaskInput {
            key: crate::paired::CaseTrialKey {
                stratum_id: "formation".into(),
                family_id: "family-1".into(),
                case_id: "case-1".into(),
                repetition: 1,
                incumbent_first: true,
            },
            formation_prompt: "Remember the Cedar Depot location.".into(),
            candidate_formation_prompt: None,
            candidate_operator_imports: vec![],
            recall_prompt: "Where is the Cedar Depot?".into(),
            seed_episodes: vec![],
        }
    }

    fn probe() -> AgentManagedProbeRequest {
        AgentManagedProbeRequest {
            query: "What city is recorded for the Cedar Depot?".into(),
            valid_at: DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
                .expect("valid fixed time")
                .with_timezone(&Utc),
            context_budget: 4,
        }
    }

    #[test]
    fn post_write_probe_uses_the_same_trial_state_after_fresh_recall() {
        let mut backend = ControlledBackend { calls: vec![] };

        let result = run_agent_managed_trial(&mut backend, RunArm::Incumbent, &task(), &probe())
            .expect("complete all stages");

        assert_eq!(backend.calls, ["start", "form", "fresh_recall", "probe"]);
        assert_eq!(
            result.formation.memory_write_episode_ids,
            ["episode:formed"]
        );
        assert_eq!(result.recall.memory_source_episode_ids, ["episode:formed"]);
        assert_eq!(
            result.independent_probe.source_episode_ids,
            ["episode:formed"]
        );
    }

    #[test]
    fn inline_extract_counts_as_one_ingest_only_when_it_returns_an_episode_id() {
        let operations = [AgentManagedWriteOperation {
            tool_name: "memory_decisions/extract".into(),
            attempts: 1,
            succeeded: 1,
            failed: 0,
            unresolved: 0,
        }];

        assert_eq!(count_successful_ingest_operations(&operations, 1), 1);
        assert_eq!(count_successful_ingest_operations(&operations, 0), 0);
    }

    #[test]
    fn inline_extract_episode_attribution_preserves_legacy_ingest_receipts() {
        let operations = [
            AgentManagedWriteOperation {
                tool_name: "memory_decisions/ingest".into(),
                attempts: 1,
                succeeded: 1,
                failed: 0,
                unresolved: 0,
            },
            AgentManagedWriteOperation {
                tool_name: "memory_decisions/extract".into(),
                attempts: 1,
                succeeded: 1,
                failed: 0,
                unresolved: 0,
            },
        ];

        assert_eq!(count_successful_ingest_operations(&operations, 1), 1);
    }
}
