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
pub const OBSERVATIONS_SCHEMA_V4: &str = "memory-mcp-agent-observations/v4";
pub const OBSERVATIONS_SCHEMA_V3: &str = "memory-mcp-agent-observations/v3";
pub const OBSERVATIONS_SCHEMA_V5: &str = "memory-mcp-agent-observations/v5";
pub const OBSERVATIONS_SCHEMA_V6: &str = "memory-mcp-agent-observations/v6";
pub const OBSERVATIONS_SCHEMA_V7: &str = "memory-mcp-agent-observations/v7";
pub const OBSERVATIONS_SCHEMA_V8: &str = "memory-mcp-agent-observations/v8";

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
            OBSERVATIONS_SCHEMA_V1
                | OBSERVATIONS_SCHEMA_V2
                | OBSERVATIONS_SCHEMA_V3
                | OBSERVATIONS_SCHEMA_V4
                | OBSERVATIONS_SCHEMA_V5
                | OBSERVATIONS_SCHEMA_V6
                | OBSERVATIONS_SCHEMA_V7
                | OBSERVATIONS_SCHEMA_V8
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
        let has_request_cutoff_fingerprint = self
            .incumbent
            .trials
            .iter()
            .chain(&self.candidate.trials)
            .filter_map(|trial| trial.observation.memory_reads.as_ref())
            .flatten()
            .any(|read| read.requested_cutoff_sha256.is_some());
        let has_selector_presence = self
            .incumbent
            .trials
            .iter()
            .chain(&self.candidate.trials)
            .filter_map(|trial| trial.observation.memory_reads.as_ref())
            .flatten()
            .any(|read| read.selector_arguments_present.is_some());
        if !matches!(
            self.schema_version.as_str(),
            OBSERVATIONS_SCHEMA_V7 | OBSERVATIONS_SCHEMA_V8
        ) && (has_request_cutoff_fingerprint || has_selector_presence)
        {
            return Err(EvalError::InvalidInput(
                "cutoff fingerprints and selector presence require paired observations v7 or v8"
                    .into(),
            ));
        }
        let captured_reads = self
            .incumbent
            .trials
            .iter()
            .chain(&self.candidate.trials)
            .filter_map(|trial| trial.observation.memory_reads.as_ref())
            .flatten();
        if self.schema_version == OBSERVATIONS_SCHEMA_V8
            && captured_reads
                .clone()
                .any(|read| read.response_sequence.is_some() != read.elapsed_ms.is_some())
        {
            return Err(EvalError::InvalidInput(
                "paired observations v8 require elapsed timing for every completed read and forbid it for interrupted reads".into(),
            ));
        }
        if self.schema_version != OBSERVATIONS_SCHEMA_V8
            && captured_reads.clone().any(|read| read.elapsed_ms.is_some())
        {
            return Err(EvalError::InvalidInput(
                "per-read elapsed timing requires paired observations v8".into(),
            ));
        }
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
        let has_registered_snapshot_retry = !snapshot_retry_requirements.is_empty();
        if has_registered_snapshot_retry
            && !matches!(
                self.schema_version.as_str(),
                OBSERVATIONS_SCHEMA_V7 | OBSERVATIONS_SCHEMA_V8
            )
        {
            return Err(EvalError::InvalidInput(
                "snapshot-bound retry scoring requires paired observations v7 or v8".into(),
            ));
        }
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
            match (
                self.schema_version.as_str(),
                trial.observation.read_capture_status,
                &trial.observation.memory_reads,
            ) {
                (
                    OBSERVATIONS_SCHEMA_V4
                    | OBSERVATIONS_SCHEMA_V5
                    | OBSERVATIONS_SCHEMA_V6
                    | OBSERVATIONS_SCHEMA_V7
                    | OBSERVATIONS_SCHEMA_V8,
                    Some(_),
                    Some(_),
                ) => {}
                (version, None, None)
                    if version != OBSERVATIONS_SCHEMA_V4
                        && version != OBSERVATIONS_SCHEMA_V5
                        && version != OBSERVATIONS_SCHEMA_V6
                        && version != OBSERVATIONS_SCHEMA_V7
                        && version != OBSERVATIONS_SCHEMA_V8 => {}
                _ => {
                    return Err(EvalError::InvalidInput(
                        "read capture fields do not match observation version".into(),
                    ));
                }
            }
            if self.schema_version == OBSERVATIONS_SCHEMA_V4
                && trial
                    .observation
                    .memory_reads
                    .as_ref()
                    .is_some_and(|reads| {
                        reads.iter().any(|read| {
                            read.requested_valid_at_sha256.is_some()
                                || read.selector_arguments_sha256.is_some()
                                || read.evidence.as_ref().is_some_and(|evidence| {
                                    evidence.returned_valid_at_sha256.is_some()
                                })
                        })
                    })
            {
                return Err(EvalError::InvalidInput(
                    "observations v4 cannot contain valid-time read fingerprints or selector fingerprints"
                        .into(),
                ));
            }
            if self.schema_version == OBSERVATIONS_SCHEMA_V5
                && trial
                    .observation
                    .memory_reads
                    .as_ref()
                    .is_some_and(|reads| {
                        reads
                            .iter()
                            .any(|read| read.selector_arguments_sha256.is_some())
                    })
            {
                return Err(EvalError::InvalidInput(
                    "paired observations v5 cannot contain selector fingerprints".into(),
                ));
            }
            if self.schema_version == OBSERVATIONS_SCHEMA_V6
                && trial
                    .observation
                    .memory_reads
                    .as_ref()
                    .is_some_and(|reads| {
                        reads
                            .iter()
                            .any(|read| read.selector_arguments_sha256.is_none())
                    })
            {
                return Err(EvalError::InvalidInput(
                    "paired observations v6 require selector fingerprints for every read".into(),
                ));
            }
            if matches!(
                self.schema_version.as_str(),
                OBSERVATIONS_SCHEMA_V7 | OBSERVATIONS_SCHEMA_V8
            )
                && trial
                    .observation
                    .memory_reads
                    .as_ref()
                    .is_some_and(|reads| {
                        reads.iter().any(|read| {
                            (task
                                .oracle
                                .read_expectation
                                .as_ref()
                                .and_then(|expectation| expectation.snapshot_bound_retry.as_ref())
                                .is_none()
                                && read.selector_arguments_sha256.is_none())
                                || read.requested_cutoff_sha256.as_ref().is_some_and(|digest| {
                                    crate::paired::validate_sha256("requested cutoff", digest)
                                        .is_err()
                                })
                                || read.selector_arguments_present.as_ref().is_some_and(|presence| {
                                    presence.len()
                                        != crate::paired_agent::tool_results::ASSEMBLE_CONTEXT_SELECTOR_FIELDS.len()
                                        || crate::paired_agent::tool_results::ASSEMBLE_CONTEXT_SELECTOR_FIELDS
                                            .iter()
                                            .any(|field| !presence.contains_key(*field))
                                })
                                || read.evidence.as_ref().is_some_and(|evidence| {
                                    read.requested_cutoff_sha256.as_ref().is_some_and(|request| {
                                        evidence
                                            .requested_cutoff_sha256
                                            .as_ref()
                                            .is_some_and(|nested| nested != request)
                                    })
                                })
                        })
                    })
            {
                return Err(EvalError::InvalidInput(
                    "paired observations v7-v8 contains invalid cutoff or selector fingerprints".into(),
                ));
            }
            if task
                .oracle
                .read_expectation
                .as_ref()
                .is_some_and(|expectation| expectation.selector_arguments.is_some())
                && !matches!(
                    self.schema_version.as_str(),
                    OBSERVATIONS_SCHEMA_V6 | OBSERVATIONS_SCHEMA_V7 | OBSERVATIONS_SCHEMA_V8
                )
            {
                return Err(EvalError::InvalidInput(
                    "registered selector expectations require paired observations v6-v8".into(),
                ));
            }
            if task
                .oracle
                .read_expectation
                .as_ref()
                .is_some_and(|expectation| expectation.snapshot_bound_retry.is_some())
                && !matches!(
                    self.schema_version.as_str(),
                    OBSERVATIONS_SCHEMA_V7 | OBSERVATIONS_SCHEMA_V8
                )
            {
                return Err(EvalError::InvalidInput(
                    "snapshot-bound retry scoring requires paired observations v7 or v8".into(),
                ));
            }
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
        let request_fingerprints_complete = receipts
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
        let valid_time_registered = matches!(
            self.schema_version.as_str(),
            OBSERVATIONS_SCHEMA_V5
                | OBSERVATIONS_SCHEMA_V6
                | OBSERVATIONS_SCHEMA_V7
                | OBSERVATIONS_SCHEMA_V8
        ) && tasks.values().any(|task| {
            task.oracle
                .read_expectation
                .as_ref()
                .is_some_and(|expectation| expectation.valid_at.is_some())
        });
        let selector_expectation_registered = tasks.values().any(|task| {
            task.oracle
                .read_expectation
                .as_ref()
                .is_some_and(|expectation| expectation.selector_arguments.is_some())
        });
        let snapshot_retry_requirements = tasks
            .values()
            .filter_map(|task| {
                task.oracle
                    .read_expectation
                    .as_ref()
                    .and_then(|expectation| expectation.snapshot_bound_retry.as_ref())
                    .map(|retry| (task.key.clone(), retry.retry_required_when_eligible))
            })
            .collect::<BTreeMap<_, _>>();
        let snapshot_retry_registered = !snapshot_retry_requirements.is_empty();
        let artifact_schema = if matches!(
            self.schema_version.as_str(),
            OBSERVATIONS_SCHEMA_V4
                | OBSERVATIONS_SCHEMA_V5
                | OBSERVATIONS_SCHEMA_V6
                | OBSERVATIONS_SCHEMA_V7
                | OBSERVATIONS_SCHEMA_V8
        ) {
            crate::paired::paired_artifact_schema_for_reads(
                receipts
                    .iter()
                    .any(|receipt| receipt.read_capture_status.is_some()),
                request_fingerprints_complete,
                valid_time_registered,
                selector_expectation_registered,
                snapshot_retry_registered,
                self.schema_version == OBSERVATIONS_SCHEMA_V8,
            )?
            .into()
        } else {
            PAIRED_ARTIFACT_SCHEMA_V6.into()
        };
        let mut artifact = PairedRunArtifact {
            schema_version: artifact_schema,
            arm,
            run_id: Some(self.run_id.clone()),
            run_started_at: self.run_started_at,
            run_completed_at: self.run_completed_at,
            identity: recorded_arm.identity.clone(),
            expected_trials,
            receipts,
        };
        super::complete_selector_registration_diagnostics(&mut artifact);
        super::complete_snapshot_retry_registration_diagnostics(
            &mut artifact,
            &snapshot_retry_requirements,
        );
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

/// Reserve a fresh attempt before reading inputs; retain invalid grades and partial outputs.
pub fn score_recorded_pair(
    plan_path: &Path,
    manifest_path: &Path,
    observations_path: &Path,
    output_directory: &Path,
) -> Result<(), EvalError> {
    std::fs::create_dir(output_directory).map_err(|source| EvalError::Io {
        path: output_directory.to_path_buf(),
        source,
    })?;
    let read = |path: &Path| {
        std::fs::read(path).map_err(|source| EvalError::Io {
            path: path.to_path_buf(),
            source,
        })
    };
    // Reserve the attempt before parsing or validating inputs. Failed grades survive retries.
    let plan_bytes = read(plan_path);
    let manifest_bytes = read(manifest_path);
    let observation_bytes = read(observations_path);
    let digest = |bytes: &Result<Vec<u8>, EvalError>| bytes.as_ref().ok().map(|b| sha256(b));
    let inputs = serde_json::json!({
        "plan_sha256":digest(&plan_bytes), "tasks_sha256":digest(&manifest_bytes),
        "observations_sha256":digest(&observation_bytes)
    });
    let result = (|| -> Result<(), EvalError> {
        let plan: PairedEvaluationPlan = serde_json::from_slice(&plan_bytes?)?;
        let manifest: PairedTaskManifest = serde_json::from_slice(&manifest_bytes?)?;
        let observations: RecordedPair = serde_json::from_slice(&observation_bytes?)?;
        let (incumbent, candidate) = observations.score(&plan, &manifest)?;
        write_json_new(&output_directory.join("incumbent.json"), &incumbent)?;
        write_json_new(&output_directory.join("candidate.json"), &candidate)
    })();
    let binary_sha256 = std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::read(path).ok())
        .map(|b| sha256(&b));
    write_json_new(
        &output_directory.join("attempt.json"),
        &serde_json::json!({
            "schema_version":"memory-paired-grade-attempt/v1",
            "execution_status":if result.is_ok() {"scored"} else {"invalid"},
            "inputs":inputs, "grader_sha256":sha256(include_bytes!("observations.rs")),
            "grader_binary_sha256":binary_sha256,
            "error":result.as_ref().err().map(ToString::to_string)
        }),
    )?;
    result
}
