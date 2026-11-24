//! Offline scoring of paired observations captured by an external runner.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{
    AgentObservation, PairedTaskDefinition, PairedTaskInput, PairedTaskManifest,
    PairedTrialExecution, score_trial, sha256, write_json_new,
};
use crate::EvalError;
use crate::paired::{
    CaseTrialKey, NormalizedErrorClass, PAIRED_ARTIFACT_SCHEMA_V6, PairedEvaluationPlan,
    PairedRunArtifact, PairedRunIdentity, RunArm, TraceEventKind,
};

pub const OBSERVATIONS_SCHEMA_V1: &str = "memory-mcp-agent-observations/v1";
pub const OBSERVATIONS_SCHEMA_V2: &str = "memory-mcp-agent-observations/v2";
pub const OBSERVATIONS_SCHEMA_V3: &str = "memory-mcp-agent-observations/v3";

/// Export the plan-bound inputs a runner needs without exposing expected outcomes.
pub fn export_paired_inputs(
    plan: &PairedEvaluationPlan,
    manifest: &PairedTaskManifest,
    output: &Path,
) -> Result<(), EvalError> {
    #[derive(Serialize)]
    struct RunnerInputs {
        schema_version: &'static str,
        plan_sha256: String,
        incumbent_identity: PairedRunIdentity,
        candidate_identity: PairedRunIdentity,
        tasks: Vec<PairedTaskInput>,
    }

    manifest.validate(plan)?;
    let inputs = RunnerInputs {
        schema_version: "memory-mcp-agent-inputs/v1",
        plan_sha256: plan.sha256()?,
        incumbent_identity: PairedRunIdentity {
            shared: plan.shared_identity.clone(),
            product: plan.incumbent_product.clone(),
        },
        candidate_identity: PairedRunIdentity {
            shared: plan.shared_identity.clone(),
            product: plan.candidate_product.clone(),
        },
        tasks: manifest.runner_inputs(),
    };
    write_json_new(output, &inputs)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedPair {
    pub schema_version: String,
    pub plan_sha256: String,
    pub run_id: String,
    pub run_started_at: DateTime<Utc>,
    pub run_completed_at: DateTime<Utc>,
    pub incumbent: RecordedArm,
    pub candidate: RecordedArm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedArm {
    pub identity: PairedRunIdentity,
    pub trials: Vec<RecordedTrial>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedTrial {
    pub key: CaseTrialKey,
    pub arm_order: u8,
    pub known_revision_checkpoint: Option<i64>,
    pub observation: AgentObservation,
    pub episode_id_to_event_id: BTreeMap<String, String>,
}

impl RecordedPair {
    /// Score captured observations without invoking the runner or using the current clock.
    pub fn score(
        &self,
        plan: &PairedEvaluationPlan,
        manifest: &PairedTaskManifest,
    ) -> Result<(PairedRunArtifact, PairedRunArtifact), EvalError> {
        if !matches!(
            self.schema_version.as_str(),
            OBSERVATIONS_SCHEMA_V1 | OBSERVATIONS_SCHEMA_V2 | OBSERVATIONS_SCHEMA_V3
        ) {
            return Err(EvalError::InvalidInput(format!(
                "unsupported paired observations schema {:?}",
                self.schema_version
            )));
        }
        if self.schema_version == OBSERVATIONS_SCHEMA_V1
            && self
                .incumbent
                .trials
                .iter()
                .chain(&self.candidate.trials)
                .flat_map(|trial| &trial.observation.trace)
                .any(|event| {
                    event
                        .error_class
                        .is_some_and(NormalizedErrorClass::requires_v3)
                })
        {
            return Err(EvalError::InvalidInput(
                "paired observations v1 cannot contain model-response diagnostics from newer schemas".into(),
            ));
        }
        if self.schema_version == OBSERVATIONS_SCHEMA_V2
            && self
                .incumbent
                .trials
                .iter()
                .chain(&self.candidate.trials)
                .flat_map(|trial| &trial.observation.trace)
                .any(|event| {
                    event
                        .error_class
                        .is_some_and(NormalizedErrorClass::requires_v4)
                })
        {
            return Err(EvalError::InvalidInput(
                "paired observations v2 cannot contain v3 final-answer diagnostics".into(),
            ));
        }
        manifest.validate(plan)?;
        if self.plan_sha256 != plan.sha256()? {
            return Err(EvalError::InvalidInput(
                "recorded observations do not match the paired plan hash".into(),
            ));
        }
        let tasks = manifest
            .tasks
            .iter()
            .map(|task| (&task.key, task))
            .collect::<BTreeMap<_, _>>();
        let incumbent = self.score_arm(plan, &tasks, RunArm::Incumbent)?;
        let candidate = self.score_arm(plan, &tasks, RunArm::Candidate)?;
        for (left, right) in incumbent.receipts.iter().zip(&candidate.receipts) {
            if left.known_revision_checkpoint != right.known_revision_checkpoint {
                return Err(EvalError::InvalidInput(format!(
                    "paired trial {} used different knowledge-revision checkpoints",
                    left.key.case_id
                )));
            }
        }
        Ok((incumbent, candidate))
    }

    fn score_arm(
        &self,
        plan: &PairedEvaluationPlan,
        tasks: &BTreeMap<&CaseTrialKey, &PairedTaskDefinition>,
        arm: RunArm,
    ) -> Result<PairedRunArtifact, EvalError> {
        let (recorded_arm, product) = match arm {
            RunArm::Incumbent => (&self.incumbent, &plan.incumbent_product),
            RunArm::Candidate => (&self.candidate, &plan.candidate_product),
        };
        if recorded_arm.identity.shared != plan.shared_identity
            || recorded_arm.identity.product != *product
        {
            return Err(EvalError::InvalidInput(format!(
                "recorded {arm:?} identity differs from the paired plan"
            )));
        }
        let mut receipts = Vec::with_capacity(recorded_arm.trials.len());
        for trial in &recorded_arm.trials {
            let task = tasks.get(&trial.key).ok_or_else(|| {
                EvalError::InvalidInput(format!(
                    "recorded {arm:?} trial {} is absent from the task manifest",
                    trial.key.case_id
                ))
            })?;
            trial.validate(task)?;
            receipts.push(score_trial(
                task,
                trial.arm_order,
                trial.known_revision_checkpoint,
                &PairedTrialExecution {
                    observation: trial.observation.clone(),
                    episode_id_to_event_id: trial.episode_id_to_event_id.clone(),
                },
            )?);
        }
        receipts.sort_by(|left, right| left.key.cmp(&right.key));
        let mut expected_trials = plan.expected_trials.clone();
        expected_trials.sort();
        let artifact = PairedRunArtifact {
            schema_version: PAIRED_ARTIFACT_SCHEMA_V6.into(),
            arm,
            run_id: Some(self.run_id.clone()),
            run_started_at: self.run_started_at,
            run_completed_at: self.run_completed_at,
            identity: recorded_arm.identity.clone(),
            expected_trials,
            receipts,
        };
        artifact.validate()?;
        Ok(artifact)
    }
}

impl RecordedTrial {
    fn validate(&self, task: &PairedTaskDefinition) -> Result<(), EvalError> {
        let invalid = |reason| {
            EvalError::InvalidInput(format!("recorded trial {} {reason}", self.key.case_id))
        };
        if task.knowledge_before_event_id.is_some() != self.known_revision_checkpoint.is_some() {
            return Err(invalid(
                "has the wrong knowledge-revision checkpoint presence",
            ));
        }

        let expected_digest = self
            .observation
            .final_answer
            .as_ref()
            .map(|answer| sha256(answer.as_bytes()));
        let answer_digests = self
            .observation
            .trace
            .iter()
            .filter(|event| event.kind == TraceEventKind::FinalAnswer)
            .map(|event| event.payload_sha256.as_deref());
        if !answer_digests.eq(expected_digest.as_deref().map(Some)) {
            return Err(invalid("has an inconsistent final answer fingerprint"));
        }
        if !self.observation.memory_source_episode_ids.is_empty()
            && !self.observation.trace.iter().any(|event| {
                event.kind == TraceEventKind::ToolResponse
                    && event.tool_name.as_deref() == Some("assemble_context")
            })
        {
            return Err(invalid(
                "reports sources without a successful assemble_context call",
            ));
        }
        let seed_events = task
            .seed_episodes
            .iter()
            .map(|episode| episode.event_id.as_str())
            .collect::<BTreeSet<_>>();
        let mut mapped = BTreeSet::new();
        if self
            .episode_id_to_event_id
            .iter()
            .any(|(episode_id, event_id)| {
                episode_id.trim().is_empty()
                    || !seed_events.contains(event_id.as_str())
                    || !mapped.insert(event_id.as_str())
            })
        {
            return Err(invalid(
                "has an invalid or ambiguous mapping to seeded events",
            ));
        }
        Ok(())
    }
}

/// Validate both arms before reserving a fresh destination for the receipts.
pub fn score_recorded_pair(
    plan: &PairedEvaluationPlan,
    manifest: &PairedTaskManifest,
    observations: &RecordedPair,
    output_directory: &Path,
) -> Result<(), EvalError> {
    let (incumbent, candidate) = observations.score(plan, manifest)?;
    std::fs::create_dir(output_directory).map_err(|source| EvalError::Io {
        path: output_directory.to_path_buf(),
        source,
    })?;
    let incumbent_path = output_directory.join("incumbent.json");
    let candidate_path = output_directory.join("candidate.json");
    let result = write_json_new(&incumbent_path, &incumbent)
        .and_then(|()| write_json_new(&candidate_path, &candidate));
    if result.is_err() {
        let _ = std::fs::remove_file(incumbent_path);
        let _ = std::fs::remove_file(candidate_path);
        let _ = std::fs::remove_dir(output_directory);
    }
    result
}
