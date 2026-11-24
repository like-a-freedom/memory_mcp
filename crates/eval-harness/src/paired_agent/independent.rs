//! Versioned independent-probe envelope; legacy agent coverage retains its meaning.
use super::*;
use crate::independent_probe::{ProbeCapture, ProbeInput, ProbeOracle, ProbeScore};

pub trait IndependentProbeBackend {
    /// Form a fresh store and issue exactly the fixed read. No oracle or agent state.
    fn capture(
        &mut self,
        arm: RunArm,
        input: &ProbeInput,
        history: &[SeedEpisode],
    ) -> Result<ProbeCapture, EvalError>;
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixedTrialProbe {
    pub key: CaseTrialKey,
    pub input: ProbeInput,
    pub launch_profile: BTreeMap<String, String>,
    pub oracle: ProbeOracle,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndependentReceipt {
    pub key: CaseTrialKey,
    pub capture: ProbeCapture,
    pub score: ProbeScore,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndependentPairedArtifact {
    pub schema_version: String,
    /// v2 direct_memory_success is agent-observed coverage, never the independent metric.
    pub agent_observed: PairedRunArtifact,
    pub probes_sha256: String,
    pub independent_memory: Vec<IndependentReceipt>,
}

/// Frozen acceptance floors for an ordinary agent-workflow development screen.
/// Every ordinary task must have an independent probe, so the probe floor must
/// equal the registered ordinary denominator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentWorkflowQualificationCriteria {
    pub ordinary_stratum_id: String,
    pub expected_trial_count: u64,
    pub minimum_task_successes: u64,
    pub minimum_direct_memory_successes: u64,
    pub minimum_probe_successes: u64,
}

/// The incumbent and candidate receipts from one independent execution.
pub struct AgentWorkflowCapture<'a> {
    pub incumbent: &'a IndependentPairedArtifact,
    pub candidate: &'a IndependentPairedArtifact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentWorkflowCaptureQualification {
    pub run_id: String,
    pub incumbent_artifact_sha256: String,
    pub candidate_artifact_sha256: String,
    pub candidate_task_successes: u64,
    pub candidate_task_trials: u64,
    pub candidate_direct_memory_successes: u64,
    pub candidate_independent_probe_successes: u64,
    pub candidate_independent_probe_trials: u64,
    pub candidate_hard_violations: u64,
    pub candidate_budget_exit_trials: u64,
    pub candidate_max_turns: u64,
    pub candidate_max_tool_calls: u64,
    pub candidate_max_output_tokens: u64,
    pub candidate_max_wall_time_ms: u64,
    pub failed_gates: Vec<String>,
    pub eligible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentWorkflowQualificationReport {
    pub schema_version: String,
    pub plan_id: String,
    pub plan_sha256: String,
    pub manifest_sha256: String,
    pub probes_sha256: String,
    pub criteria_sha256: String,
    pub criteria: AgentWorkflowQualificationCriteria,
    pub captures: Vec<AgentWorkflowCaptureQualification>,
    pub eligible: bool,
}

/// Compose ordinary agent-task success, independent fixed-query evidence,
/// hard violations, and registered resource exits into one qualification gate.
/// The two captures must be separate executions of the same frozen plan.
pub fn qualify_agent_workflow(
    plan: &PairedEvaluationPlan,
    manifest: &PairedTaskManifest,
    probes: &[FixedTrialProbe],
    criteria: &AgentWorkflowQualificationCriteria,
    captures: &[AgentWorkflowCapture<'_>; 2],
) -> Result<AgentWorkflowQualificationReport, EvalError> {
    if plan.stage != crate::paired::PairedEvaluationStage::DevelopmentPilot {
        return Err(invalid(
            "agent workflow qualification requires a development-pilot plan",
        ));
    }
    manifest.validate(plan)?;
    validate_probes(plan, manifest, probes)?;

    let ordinary_trials = plan
        .expected_trials
        .iter()
        .filter(|trial| trial.stratum_id == criteria.ordinary_stratum_id)
        .count();
    let ordinary_families = plan
        .expected_trials
        .iter()
        .filter(|trial| trial.stratum_id == criteria.ordinary_stratum_id)
        .map(|trial| trial.family_id.as_str())
        .collect::<BTreeSet<_>>();
    if criteria.ordinary_stratum_id.trim().is_empty()
        || criteria.expected_trial_count == 0
        || u64::try_from(ordinary_trials).ok() != Some(criteria.expected_trial_count)
        || u64::try_from(ordinary_families.len()).ok() != Some(criteria.expected_trial_count)
        || criteria.minimum_task_successes > criteria.expected_trial_count
        || criteria.minimum_direct_memory_successes > criteria.expected_trial_count
        || criteria.minimum_probe_successes != criteria.expected_trial_count
    {
        return Err(invalid(
            "qualification criteria must match the ordinary task and independent-probe denominators",
        ));
    }

    let mut run_ids = BTreeSet::new();
    let mut artifact_hashes = BTreeSet::new();
    let mut reports = Vec::with_capacity(captures.len());
    for capture in captures {
        capture.incumbent.validate(plan, manifest, probes)?;
        capture.candidate.validate(plan, manifest, probes)?;
        let incumbent = &capture.incumbent.agent_observed;
        let candidate = &capture.candidate.agent_observed;
        let run_id = candidate
            .run_id
            .as_ref()
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| invalid("qualification capture is missing a run id"))?;
        if incumbent.arm != RunArm::Incumbent
            || candidate.arm != RunArm::Candidate
            || incumbent.run_id.as_ref() != Some(run_id)
            || !run_ids.insert(run_id.clone())
        {
            return Err(invalid(
                "qualification captures must contain paired arms from two distinct executions",
            ));
        }
        let incumbent_bytes = serde_json::to_vec(capture.incumbent)?;
        let candidate_bytes = serde_json::to_vec(capture.candidate)?;
        let incumbent_hash = sha256(&incumbent_bytes);
        let candidate_hash = sha256(&candidate_bytes);
        if !artifact_hashes.insert(incumbent_hash.clone())
            || !artifact_hashes.insert(candidate_hash.clone())
        {
            return Err(invalid("qualification captures reuse a run artifact"));
        }

        let ordinary_receipts = candidate
            .receipts
            .iter()
            .filter(|receipt| receipt.key.stratum_id == criteria.ordinary_stratum_id)
            .collect::<Vec<_>>();
        let ordinary_probes = capture
            .candidate
            .independent_memory
            .iter()
            .filter(|receipt| receipt.key.stratum_id == criteria.ordinary_stratum_id)
            .collect::<Vec<_>>();
        let task_trials = u64::try_from(ordinary_receipts.len())
            .map_err(|_| invalid("ordinary task count exceeds u64"))?;
        let probe_trials = u64::try_from(ordinary_probes.len())
            .map_err(|_| invalid("ordinary independent-probe count exceeds u64"))?;
        if task_trials != criteria.expected_trial_count
            || probe_trials != criteria.expected_trial_count
        {
            return Err(invalid(
                "qualification capture does not cover the ordinary task and probe denominators",
            ));
        }

        let task_successes = ordinary_receipts
            .iter()
            .filter(|receipt| receipt.task_success())
            .count() as u64;
        let direct_memory_successes = ordinary_receipts
            .iter()
            .filter(|receipt| receipt.direct_memory_success)
            .count() as u64;
        let probe_successes = ordinary_probes
            .iter()
            .filter(|receipt| receipt.score.evidence_success)
            .count() as u64;
        let hard_violations = ordinary_receipts
            .iter()
            .map(|receipt| receipt.hard_violations.len() as u64)
            .sum();
        let budget_exit_trials = ordinary_receipts
            .iter()
            .filter(|receipt| {
                matches!(
                    receipt.status,
                    crate::paired::TrialStatus::TimedOut
                        | crate::paired::TrialStatus::BudgetExceeded
                )
            })
            .count() as u64;
        let max = |select: fn(&crate::paired::TrialReceipt) -> u64| {
            ordinary_receipts
                .iter()
                .map(|receipt| select(receipt))
                .max()
                .unwrap_or(0)
        };
        let mut failed_gates = Vec::new();
        if task_successes < criteria.minimum_task_successes {
            failed_gates.push("ordinary_task_success_floor".into());
        }
        if direct_memory_successes < criteria.minimum_direct_memory_successes {
            failed_gates.push("ordinary_agent_memory_read_floor".into());
        }
        if probe_successes < criteria.minimum_probe_successes {
            failed_gates.push("ordinary_independent_probe_floor".into());
        }
        if hard_violations != 0 {
            failed_gates.push("hard_violations".into());
        }
        if budget_exit_trials != 0 {
            failed_gates.push("resource_budget_exits".into());
        }
        reports.push(AgentWorkflowCaptureQualification {
            run_id: run_id.clone(),
            incumbent_artifact_sha256: incumbent_hash,
            candidate_artifact_sha256: candidate_hash,
            candidate_task_successes: task_successes,
            candidate_task_trials: task_trials,
            candidate_direct_memory_successes: direct_memory_successes,
            candidate_independent_probe_successes: probe_successes,
            candidate_independent_probe_trials: probe_trials,
            candidate_hard_violations: hard_violations,
            candidate_budget_exit_trials: budget_exit_trials,
            candidate_max_turns: max(|r| u64::from(r.turns_used)),
            candidate_max_tool_calls: max(|r| u64::from(r.tool_calls)),
            candidate_max_output_tokens: max(|r| r.output_tokens),
            candidate_max_wall_time_ms: max(|r| r.wall_time_ms),
            eligible: failed_gates.is_empty(),
            failed_gates,
        });
    }
    let eligible = reports.iter().all(|report| report.eligible);
    Ok(AgentWorkflowQualificationReport {
        schema_version: "memory-mcp-agent-workflow-qualification/v1".into(),
        plan_id: plan.plan_id.clone(),
        plan_sha256: plan.sha256()?,
        manifest_sha256: sha256(&serde_json::to_vec(manifest)?),
        probes_sha256: probes_hash(probes)?,
        criteria_sha256: sha256(&serde_json::to_vec(criteria)?),
        criteria: criteria.clone(),
        captures: reports,
        eligible,
    })
}

pub fn run_with_independent_probes<A: PairedTrialBackend, P: IndependentProbeBackend>(
    plan: &PairedEvaluationPlan,
    manifest: &PairedTaskManifest,
    probes: &[FixedTrialProbe],
    agent: &mut A,
    probe: &mut P,
    output: &Path,
) -> Result<(IndependentPairedArtifact, IndependentPairedArtifact), EvalError> {
    manifest.validate(plan)?;
    validate_probes(plan, manifest, probes)?;
    for name in ["independent-incumbent.json", "independent-candidate.json"] {
        if output.join(name).exists() {
            return Err(invalid("independent output already exists"));
        }
    }
    struct Wrapped<'a, A, P> {
        plan: &'a PairedEvaluationPlan,
        probes: &'a [FixedTrialProbe],
        agent: &'a mut A,
        probe: &'a mut P,
        incumbent: Vec<IndependentReceipt>,
        candidate: Vec<IndependentReceipt>,
    }
    impl<A: PairedTrialBackend, P: IndependentProbeBackend> PairedTrialBackend for Wrapped<'_, A, P> {
        fn prepare(&mut self, task: &PairedTaskInput) -> Result<PreparedPairedTask, EvalError> {
            self.agent.prepare(task)
        }
        fn execute(
            &mut self,
            arm: RunArm,
            task: &PreparedPairedTask,
        ) -> Result<PairedTrialExecution, EvalError> {
            let fixed = self
                .probes
                .iter()
                .find(|p| p.key == task.input.key)
                .ok_or_else(|| invalid("missing independent probe"))?;
            let product = match arm {
                RunArm::Incumbent => &self.plan.incumbent_product,
                RunArm::Candidate => &self.plan.candidate_product,
            };
            let (probe_input, expected_launch_profile) =
                probe_settings_for_arm(self.plan, fixed, arm)?;
            let capture = self
                .probe
                .capture(arm, &probe_input, &task.input.seed_episodes)?;
            if capture.launch_profile != expected_launch_profile {
                return Err(invalid(
                    "independent launch profile differs from registration",
                ));
            }
            capture.validate(
                &probe_input,
                &task.input.seed_episodes,
                &product.binary_sha256,
                &self.plan.shared_identity.transport,
                &product.tool_schema_sha256,
            )?;
            let score = crate::independent_probe::score_probe(
                &probe_input,
                &fixed.oracle,
                Some(&capture.observation),
                &product.binary_sha256,
            )?;
            let receipt = IndependentReceipt {
                key: fixed.key.clone(),
                capture,
                score,
            };
            match arm {
                RunArm::Incumbent => self.incumbent.push(receipt),
                RunArm::Candidate => self.candidate.push(receipt),
            }
            self.agent.execute(arm, task)
        }
    }
    let mut wrapped = Wrapped {
        plan,
        probes,
        agent,
        probe,
        incumbent: vec![],
        candidate: vec![],
    };
    let (left, right) = run_paired_agent_evaluation(plan, manifest, &mut wrapped, output)?;
    let probe_hash = probes_hash(probes)?;
    let left = IndependentPairedArtifact {
        schema_version: INDEPENDENT_PAIRED_SCHEMA.into(),
        agent_observed: left,
        probes_sha256: probe_hash.clone(),
        independent_memory: wrapped.incumbent,
    };
    let right = IndependentPairedArtifact {
        schema_version: INDEPENDENT_PAIRED_SCHEMA.into(),
        agent_observed: right,
        probes_sha256: probe_hash,
        independent_memory: wrapped.candidate,
    };
    left.validate(plan, manifest, probes)?;
    right.validate(plan, manifest, probes)?;
    write_json_new(&output.join("independent-incumbent.json"), &left)?;
    write_json_new(&output.join("independent-candidate.json"), &right)?;
    Ok((left, right))
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndependentComparison {
    pub schema_version: String,
    pub probes_sha256: String,
    pub trials: usize,
    pub incumbent_evidence_successes: usize,
    pub candidate_evidence_successes: usize,
    pub wins: Vec<CaseTrialKey>,
    pub losses: Vec<CaseTrialKey>,
}
/// Source-coverage comparison only. Decision acceptance is a separate richer protocol.
pub fn compare_independent_runs(
    left: &IndependentPairedArtifact,
    right: &IndependentPairedArtifact,
    plan: &PairedEvaluationPlan,
    manifest: &PairedTaskManifest,
    probes: &[FixedTrialProbe],
) -> Result<IndependentComparison, EvalError> {
    left.validate(plan, manifest, probes)?;
    right.validate(plan, manifest, probes)?;
    if left.agent_observed.arm != RunArm::Incumbent
        || right.agent_observed.arm != RunArm::Candidate
        || left.agent_observed.run_id != right.agent_observed.run_id
        || left.agent_observed.run_id.is_none()
    {
        return Err(invalid("independent paired arms or run identity mismatch"));
    }
    let mut wins = vec![];
    let mut losses = vec![];
    for a in &left.independent_memory {
        let b = right
            .independent_memory
            .iter()
            .find(|b| b.key == a.key)
            .ok_or_else(|| invalid("missing independent paired case"))?;
        if common_probe_launch_profile(&a.capture.launch_profile)
            != common_probe_launch_profile(&b.capture.launch_profile)
        {
            return Err(invalid("independent paired launch profiles differ"));
        }
        if !a.score.evidence_success && b.score.evidence_success {
            wins.push(a.key.clone());
        }
        if a.score.evidence_success && !b.score.evidence_success {
            losses.push(a.key.clone());
        }
    }
    wins.sort();
    losses.sort();
    Ok(IndependentComparison {
        schema_version: "memory-mcp-independent-comparison/v1".into(),
        probes_sha256: left.probes_sha256.clone(),
        trials: left.independent_memory.len(),
        incumbent_evidence_successes: left
            .independent_memory
            .iter()
            .filter(|r| r.score.evidence_success)
            .count(),
        candidate_evidence_successes: right
            .independent_memory
            .iter()
            .filter(|r| r.score.evidence_success)
            .count(),
        wins,
        losses,
    })
}

/// Compose the generic paired runner with real isolated public probe transports.
pub fn run_with_public_probes<A: PairedTrialBackend>(
    plan: &PairedEvaluationPlan,
    manifest: &PairedTaskManifest,
    probes: &[FixedTrialProbe],
    agent: &mut A,
    binaries: (std::path::PathBuf, std::path::PathBuf),
    output: &Path,
) -> Result<(IndependentPairedArtifact, IndependentPairedArtifact), EvalError> {
    struct OwnedScratch(std::path::PathBuf);
    impl Drop for OwnedScratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let root = std::env::temp_dir().join(format!("memory-independent-paired-{}", new_run_id()?));
    std::fs::create_dir(&root).map_err(|source| EvalError::Io {
        path: root.clone(),
        source,
    })?;
    let scratch = OwnedScratch(root);
    let mut backend = PublicProbeBackend::new(
        binaries.0,
        binaries.1,
        plan.shared_identity.transport.clone(),
        scratch.0.clone(),
    )?;
    run_with_independent_probes(plan, manifest, probes, agent, &mut backend, output)
}

/// Real public transports; every call receives a separately created store.
/// This adapter has no agent executable, account, or model dependency.
pub struct PublicProbeBackend {
    pub incumbent_binary: std::path::PathBuf,
    pub candidate_binary: std::path::PathBuf,
    pub transport: String,
    pub store_root: std::path::PathBuf,
    next_store: u64,
}
impl PublicProbeBackend {
    pub fn new(
        incumbent_binary: std::path::PathBuf,
        candidate_binary: std::path::PathBuf,
        transport: String,
        store_root: std::path::PathBuf,
    ) -> Result<Self, EvalError> {
        if !matches!(transport.as_str(), "stdio" | "streamable_http") || !store_root.is_absolute() {
            return Err(invalid(
                "unsupported probe transport or relative store root",
            ));
        }
        std::fs::create_dir_all(&store_root).map_err(|source| EvalError::Io {
            path: store_root.clone(),
            source,
        })?;
        Ok(Self {
            incumbent_binary,
            candidate_binary,
            transport,
            store_root,
            next_store: 0,
        })
    }
}
impl IndependentProbeBackend for PublicProbeBackend {
    fn capture(
        &mut self,
        arm: RunArm,
        input: &ProbeInput,
        history: &[SeedEpisode],
    ) -> Result<ProbeCapture, EvalError> {
        let binary = match arm {
            RunArm::Incumbent => &self.incumbent_binary,
            RunArm::Candidate => &self.candidate_binary,
        };
        let bytes = std::fs::read(binary).map_err(|source| EvalError::Io {
            path: binary.clone(),
            source,
        })?;
        let product = sha256(&bytes);
        let store = self.store_root.join(format!("probe-{}", self.next_store));
        self.next_store += 1;
        if self.transport == "stdio" {
            return crate::independent_probe::capture_stdio_probe(
                input,
                history,
                binary,
                &product,
                &store,
                &input.claim_rollout_stage,
            );
        }
        // A separate thread permits use from both synchronous and async host runners.
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let rt = tokio::runtime::Runtime::new().map_err(|e| invalid(&e.to_string()))?;
                    rt.block_on(crate::independent_probe::capture_http_probe(
                        input,
                        history,
                        binary,
                        &product,
                        &store,
                        &input.claim_rollout_stage,
                    ))
                })
                .join()
                .map_err(|_| invalid("independent HTTP capture thread panicked"))?
        })
    }
}

pub const INDEPENDENT_PAIRED_SCHEMA: &str = "memory-mcp-independent-paired/v1";
fn invalid(message: &str) -> EvalError {
    EvalError::InvalidInput(message.into())
}
fn probes_hash(probes: &[FixedTrialProbe]) -> Result<String, EvalError> {
    let mut sorted = probes.to_vec();
    sorted.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(sha256(&serde_json::to_vec(&sorted)?))
}
fn validate_probes(
    plan: &PairedEvaluationPlan,
    manifest: &PairedTaskManifest,
    probes: &[FixedTrialProbe],
) -> Result<(), EvalError> {
    let keys = probes.iter().map(|p| p.key.clone()).collect::<Vec<_>>();
    crate::paired::validate_case_coverage(&plan.expected_trials, &keys, &keys)?;
    for probe in probes {
        probe.input.validate()?;
        if probe.launch_profile.get("MEMORY_CLAIM_ROLLOUT_STAGE")
            != Some(&probe.input.claim_rollout_stage)
        {
            return Err(invalid(
                "probe reference stage differs from its registered launch profile",
            ));
        }
        match (
            plan.incumbent_product.claim_rollout_stage.as_deref(),
            plan.candidate_product.claim_rollout_stage.as_deref(),
        ) {
            (Some(incumbent_stage), Some(_))
                if probe.input.claim_rollout_stage != incumbent_stage =>
            {
                return Err(invalid(
                    "stage-pinned probes must register the incumbent stage as their reference stage",
                ));
            }
            (Some(_), Some(_)) | (None, None) => {}
            _ => return Err(invalid("paired plan has incomplete stage identities")),
        }
        let task = manifest
            .tasks
            .iter()
            .find(|t| t.key == probe.key)
            .ok_or_else(|| invalid("probe has no task"))?;
        if probe.input.history_sha256 != sha256(&serde_json::to_vec(&task.seed_episodes)?) {
            return Err(invalid("probe history mismatch"));
        }
        let ids = task
            .seed_episodes
            .iter()
            .map(|e| e.event_id.as_str())
            .collect::<BTreeSet<_>>();
        if probe
            .oracle
            .forbidden_event_ids
            .iter()
            .chain(
                probe
                    .oracle
                    .sufficient_evidence_alternatives
                    .iter()
                    .flatten()
                    .flatten(),
            )
            .any(|id| !ids.contains(id.as_str()))
        {
            return Err(invalid("probe oracle refers to unseeded evidence"));
        }
    }
    Ok(())
}

fn probe_settings_for_arm(
    plan: &PairedEvaluationPlan,
    probe: &FixedTrialProbe,
    arm: RunArm,
) -> Result<(ProbeInput, BTreeMap<String, String>), EvalError> {
    let (incumbent_stage, candidate_stage) = match (
        plan.incumbent_product.claim_rollout_stage.as_deref(),
        plan.candidate_product.claim_rollout_stage.as_deref(),
    ) {
        (Some(_), Some(_)) => plan.claim_rollout_stages(None)?,
        (None, None) => plan.claim_rollout_stages(Some(&probe.input.claim_rollout_stage))?,
        _ => return Err(invalid("paired plan has incomplete stage identities")),
    };
    let stage = match arm {
        RunArm::Incumbent => incumbent_stage,
        RunArm::Candidate => candidate_stage,
    };
    let mut input = probe.input.clone();
    input.claim_rollout_stage = stage.clone();
    let mut launch_profile = probe.launch_profile.clone();
    launch_profile.insert("MEMORY_CLAIM_ROLLOUT_STAGE".into(), stage);
    Ok((input, launch_profile))
}

fn common_probe_launch_profile(profile: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    profile
        .iter()
        .filter(|(key, _)| key.as_str() != "MEMORY_CLAIM_ROLLOUT_STAGE")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

impl IndependentPairedArtifact {
    /// Re-score externally captured observations without credentials or an agent process.
    /// v2 artifacts deliberately cannot deserialize into this envelope.
    pub fn validate(
        &self,
        plan: &PairedEvaluationPlan,
        manifest: &PairedTaskManifest,
        probes: &[FixedTrialProbe],
    ) -> Result<(), EvalError> {
        manifest.validate(plan)?;
        validate_probes(plan, manifest, probes)?;
        if self.schema_version != INDEPENDENT_PAIRED_SCHEMA
            || self.probes_sha256 != probes_hash(probes)?
        {
            return Err(invalid(
                "incompatible independent paired semantics or probe manifest",
            ));
        }
        self.agent_observed.validate()?;
        let product = match self.agent_observed.arm {
            RunArm::Incumbent => &plan.incumbent_product,
            RunArm::Candidate => &plan.candidate_product,
        };
        if self.agent_observed.identity.shared != plan.shared_identity
            || self.agent_observed.identity.product != *product
            || self.agent_observed.expected_trials != plan.expected_trials
        {
            return Err(invalid("independent paired plan identity mismatch"));
        }
        let keys = self
            .independent_memory
            .iter()
            .map(|p| p.key.clone())
            .collect::<Vec<_>>();
        crate::paired::validate_case_coverage(&plan.expected_trials, &keys, &keys)?;
        let mut profile = None;
        for receipt in &self.independent_memory {
            let fixed = probes
                .iter()
                .find(|p| p.key == receipt.key)
                .ok_or_else(|| invalid("unknown independent receipt"))?;
            let task = manifest
                .tasks
                .iter()
                .find(|t| t.key == receipt.key)
                .ok_or_else(|| invalid("unknown task"))?;
            let (probe_input, expected_launch_profile) =
                probe_settings_for_arm(plan, fixed, self.agent_observed.arm)?;
            receipt.capture.validate(
                &probe_input,
                &task.seed_episodes,
                &product.binary_sha256,
                &plan.shared_identity.transport,
                &product.tool_schema_sha256,
            )?;
            if receipt.capture.launch_profile != expected_launch_profile {
                return Err(invalid(
                    "independent launch profile differs from registration",
                ));
            }
            let actual = crate::independent_probe::score_probe(
                &probe_input,
                &fixed.oracle,
                Some(&receipt.capture.observation),
                &product.binary_sha256,
            )?;
            if receipt.score != actual {
                return Err(invalid(
                    "independent score differs from captured observation",
                ));
            }
            if profile
                .as_ref()
                .is_some_and(|p| p != &receipt.capture.launch_profile)
            {
                return Err(invalid("independent launch profile changed within arm"));
            }
            profile = Some(receipt.capture.launch_profile.clone());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::independent_probe::ProbeObservation;
    struct ControlledProbe;
    impl IndependentProbeBackend for ControlledProbe {
        fn capture(
            &mut self,
            _arm: RunArm,
            input: &ProbeInput,
            history: &[SeedEpisode],
        ) -> Result<ProbeCapture, EvalError> {
            let source = history[0].event_id.clone();
            let product = "5".repeat(64);
            let stage = input.claim_rollout_stage.clone();
            Ok(ProbeCapture {
                schema_version: "memory-mcp-independent-probe-capture/v1".into(),
                launch_profile: BTreeMap::from([
                    ("NER_EXTRACTOR".into(), "regex".into()),
                    ("EMBEDDINGS_ENABLED".into(), "false".into()),
                    ("MEMORY_CLAIM_ROLLOUT_STAGE".into(), stage.clone()),
                ]),
                episode_id_to_event_id: BTreeMap::from([("episode:fixed".into(), source.clone())]),
                transport: "stdio".into(),
                extractor: "regex".into(),
                embeddings_enabled: false,
                observation: ProbeObservation {
                    input_sha256: input.sha256()?,
                    product_sha256: product,
                    source_event_ids: vec![source],
                    claim_annotations: vec![],
                    budget_used: 1,
                    completed: true,
                },
                request: serde_json::json!({"query": input.query, "valid_at": input.valid_at.to_rfc3339(), "known_revision": input.known_revision, "budget": input.context_budget, "compact": true}),
                response: serde_json::json!({"result": {"structuredContent": {"result": [{"source_episode": "episode:fixed"}], "context": {"snapshot": {"valid_at": input.valid_at, "known_revision": input.known_revision}}}}}),
                tool_schema_sha256: "6".repeat(64),
                claim_rollout_stage: stage,
                api_profile: crate::independent_probe::ProbeApiProfile::RevisionBoundV1,
            })
        }
    }
    struct Agent {
        retrieve: bool,
        answer: &'static str,
    }
    impl PairedTrialBackend for Agent {
        fn execute(
            &mut self,
            arm: RunArm,
            task: &PreparedPairedTask,
        ) -> Result<PairedTrialExecution, EvalError> {
            let mut fixed = super::super::tests::FixedBackend {
                calls: vec![],
                answer: self.answer.into(),
            };
            let mut execution = fixed.execute(arm, task)?;
            if !self.retrieve {
                execution.observation.memory_source_episode_ids.clear();
                execution.observation.trace.retain(|e| {
                    !matches!(
                        e.kind,
                        TraceEventKind::MemoryRead | TraceEventKind::ToolResponse
                    )
                });
                for (i, e) in execution.observation.trace.iter_mut().enumerate() {
                    e.sequence = (i + 1) as u32;
                }
                execution.observation.tool_calls = 0;
                if let Some(answer) = execution.observation.final_answer.as_deref() {
                    let mut value: serde_json::Value =
                        serde_json::from_str(answer).expect("fixture answer is valid JSON");
                    value["citations"] = serde_json::json!([]);
                    execution.observation.final_answer =
                        Some(serde_json::to_string(&value).expect("fixture answer serializes"));
                }
            }
            Ok(execution)
        }
    }
    fn fixture() -> (
        PairedTaskManifest,
        PairedEvaluationPlan,
        Vec<FixedTrialProbe>,
    ) {
        let m = super::super::tests::manifest();
        let mut p = super::super::tests::plan(&m);
        // This suite tests deterministic plumbing, not a powered agent claim.
        p.candidate_product = p.incumbent_product.clone();
        p.declared_tool_schema_change = None;
        p.stage = crate::paired::PairedEvaluationStage::DevelopmentPilot;
        p.pilot_report_sha256 = None;
        p.power_analysis_sha256 = None;
        p.promotion_criteria = None;
        let probes = m
            .tasks
            .iter()
            .map(|t| FixedTrialProbe {
                key: t.key.clone(),
                input: ProbeInput {
                    schema_version: "memory-mcp-independent-probe/v1".into(),
                    query: "fixed question".into(),
                    principal: "fixture-reader".into(),
                    valid_at: t.seed_episodes[0].valid_at,
                    known_revision: 3,
                    context_budget: 2,
                    history_sha256: sha256(&serde_json::to_vec(&t.seed_episodes).unwrap()),
                    claim_rollout_stage: "shadow".into(),
                },
                launch_profile: BTreeMap::from([
                    ("NER_EXTRACTOR".into(), "regex".into()),
                    ("EMBEDDINGS_ENABLED".into(), "false".into()),
                    ("MEMORY_CLAIM_ROLLOUT_STAGE".into(), "shadow".into()),
                ]),
                oracle: ProbeOracle {
                    sufficient_evidence_alternatives: vec![vec![vec![
                        t.seed_episodes[0].event_id.clone(),
                    ]]],
                    forbidden_event_ids: vec![],
                    required_claim_annotations: vec![],
                    required_claim_annotation_alternatives: vec![],
                },
            })
            .collect();
        (m, p, probes)
    }
    #[test]
    fn paired_probe_is_independent_of_agent_selected_sources() {
        let (m, p, probes) = fixture();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let (left, _) = run_with_independent_probes(
            &p,
            &m,
            &probes,
            &mut Agent {
                retrieve: false,
                answer: "North City",
            },
            &mut ControlledProbe,
            a.path(),
        )
        .expect("independent runner");
        let (right, _) = run_with_independent_probes(
            &p,
            &m,
            &probes,
            &mut Agent {
                retrieve: true,
                answer: "North City",
            },
            &mut ControlledProbe,
            b.path(),
        )
        .expect("independent runner");
        assert_eq!(
            left.independent_memory[0].score,
            right.independent_memory[0].score
        );
        assert!(left.independent_memory[0].score.evidence_success);
        assert!(!left.agent_observed.receipts[0].direct_memory_success);
        assert!(right.agent_observed.receipts[0].direct_memory_success);
        assert_eq!(left.agent_observed.receipts[0].tool_calls, 0);
        assert_eq!(right.agent_observed.receipts[0].tool_calls, 1);
    }

    #[test]
    fn ordinary_workflow_failure_blocks_confirmation_despite_passing_probes() {
        let (manifest, plan, probes) = four_ordinary_case_fixture();
        let first_output = tempfile::tempdir().unwrap();
        let second_output = tempfile::tempdir().unwrap();
        let (first_incumbent, first_candidate) = run_with_independent_probes(
            &plan,
            &manifest,
            &probes,
            &mut Agent {
                retrieve: false,
                answer: "Incorrect Depot",
            },
            &mut ControlledProbe,
            first_output.path(),
        )
        .expect("first independent capture");
        let (second_incumbent, second_candidate) = run_with_independent_probes(
            &plan,
            &manifest,
            &probes,
            &mut Agent {
                retrieve: false,
                answer: "Incorrect Depot",
            },
            &mut ControlledProbe,
            second_output.path(),
        )
        .expect("second independent capture");
        let captures = [
            AgentWorkflowCapture {
                incumbent: &first_incumbent,
                candidate: &first_candidate,
            },
            AgentWorkflowCapture {
                incumbent: &second_incumbent,
                candidate: &second_candidate,
            },
        ];

        let report = qualify_agent_workflow(
            &plan,
            &manifest,
            &probes,
            &AgentWorkflowQualificationCriteria {
                ordinary_stratum_id: "ordinary".into(),
                expected_trial_count: 4,
                minimum_task_successes: 3,
                minimum_direct_memory_successes: 3,
                minimum_probe_successes: 4,
            },
            &captures,
        )
        .expect("valid captures produce a qualification report");

        assert!(!report.eligible);
        for capture in report.captures {
            assert_eq!(capture.candidate_task_successes, 0);
            assert_eq!(capture.candidate_task_trials, 4);
            assert_eq!(capture.candidate_direct_memory_successes, 0);
            assert_eq!(capture.candidate_independent_probe_successes, 4);
            assert_eq!(capture.candidate_independent_probe_trials, 4);
            assert_eq!(capture.candidate_hard_violations, 0);
            assert_eq!(capture.candidate_budget_exit_trials, 0);
            assert!(
                capture
                    .failed_gates
                    .contains(&"ordinary_task_success_floor".to_owned())
            );
            assert!(
                capture
                    .failed_gates
                    .contains(&"ordinary_agent_memory_read_floor".to_owned())
            );
        }
    }

    #[test]
    fn ordinary_workflow_qualifies_only_when_both_captures_pass_every_gate() {
        let (manifest, plan, probes) = four_ordinary_case_fixture();
        let first_output = tempfile::tempdir().unwrap();
        let second_output = tempfile::tempdir().unwrap();
        let (first_incumbent, first_candidate) = run_with_independent_probes(
            &plan,
            &manifest,
            &probes,
            &mut Agent {
                retrieve: true,
                answer: "North City",
            },
            &mut ControlledProbe,
            first_output.path(),
        )
        .expect("first independent capture");
        let (second_incumbent, second_candidate) = run_with_independent_probes(
            &plan,
            &manifest,
            &probes,
            &mut Agent {
                retrieve: true,
                answer: "North City",
            },
            &mut ControlledProbe,
            second_output.path(),
        )
        .expect("second independent capture");
        let captures = [
            AgentWorkflowCapture {
                incumbent: &first_incumbent,
                candidate: &first_candidate,
            },
            AgentWorkflowCapture {
                incumbent: &second_incumbent,
                candidate: &second_candidate,
            },
        ];

        let report = qualify_agent_workflow(
            &plan,
            &manifest,
            &probes,
            &AgentWorkflowQualificationCriteria {
                ordinary_stratum_id: "ordinary".into(),
                expected_trial_count: 4,
                minimum_task_successes: 3,
                minimum_direct_memory_successes: 3,
                minimum_probe_successes: 4,
            },
            &captures,
        )
        .expect("valid captures produce a qualification report");

        assert!(report.eligible);
        assert_eq!(report.manifest_sha256.len(), 64);
        assert_eq!(report.criteria_sha256.len(), 64);
        for capture in report.captures {
            assert!(capture.eligible);
            assert_eq!(capture.candidate_task_successes, 4);
            assert_eq!(capture.candidate_direct_memory_successes, 4);
            assert_eq!(capture.candidate_independent_probe_successes, 4);
            assert_eq!(capture.candidate_hard_violations, 0);
            assert_eq!(capture.candidate_budget_exit_trials, 0);
        }
    }

    fn four_ordinary_case_fixture() -> (
        PairedTaskManifest,
        PairedEvaluationPlan,
        Vec<FixedTrialProbe>,
    ) {
        let (mut manifest, mut plan, mut probes) = fixture();
        for index in 2..4 {
            let mut task = manifest.tasks[0].clone();
            let suffix = (b'a' + index as u8) as char;
            let event_id = format!("event-{suffix}");
            task.key.family_id = format!("family-{suffix}");
            task.key.case_id = format!("case-{suffix}");
            task.key.incumbent_first = index % 2 != 0;
            task.seed_episodes[0].event_id = event_id.clone();
            task.seed_episodes[0].source_id = format!("source-{event_id}");
            task.oracle.sufficient_evidence_alternatives = vec![vec![vec![event_id.clone()]]];
            manifest.tasks.push(task.clone());
            probes.push(FixedTrialProbe {
                key: task.key,
                input: ProbeInput {
                    schema_version: "memory-mcp-independent-probe/v1".into(),
                    query: "fixed question".into(),
                    principal: "fixture-reader".into(),
                    valid_at: task.seed_episodes[0].valid_at,
                    known_revision: 3,
                    context_budget: 2,
                    history_sha256: sha256(&serde_json::to_vec(&task.seed_episodes).unwrap()),
                    claim_rollout_stage: "shadow".into(),
                },
                launch_profile: BTreeMap::from([
                    ("NER_EXTRACTOR".into(), "regex".into()),
                    ("EMBEDDINGS_ENABLED".into(), "false".into()),
                    ("MEMORY_CLAIM_ROLLOUT_STAGE".into(), "shadow".into()),
                ]),
                oracle: ProbeOracle {
                    sufficient_evidence_alternatives: vec![vec![vec![event_id]]],
                    forbidden_event_ids: vec![],
                    required_claim_annotations: vec![],
                    required_claim_annotation_alternatives: vec![],
                },
            });
        }
        plan.expected_trials = manifest.tasks.iter().map(|task| task.key.clone()).collect();
        plan.planned_independent_families = 4;
        plan.shared_identity.corpus_sha256 = manifest.corpus_sha256().unwrap();
        plan.shared_identity.oracle_sha256 = manifest.oracle_sha256().unwrap();
        plan.validate().unwrap();
        (manifest, plan, probes)
    }

    #[test]
    fn paired_independent_probe_uses_each_plan_pinned_rollout_stage() {
        let (manifest, mut plan, probes) = fixture();
        plan.stage = crate::paired::PairedEvaluationStage::ExploratoryComparison;
        plan.incumbent_product.claim_rollout_stage = Some("shadow".into());
        plan.candidate_product.claim_rollout_stage = Some("evidence".into());
        plan.validate().expect("valid per-arm probe plan");
        let output = tempfile::tempdir().expect("independent paired output");

        let (incumbent, candidate) = run_with_independent_probes(
            &plan,
            &manifest,
            &probes,
            &mut Agent {
                retrieve: true,
                answer: "North City",
            },
            &mut ControlledProbe,
            output.path(),
        )
        .expect("capture both plan-pinned stages");

        assert!(
            incumbent
                .independent_memory
                .iter()
                .all(|receipt| receipt.capture.claim_rollout_stage == "shadow")
        );
        assert!(
            candidate
                .independent_memory
                .iter()
                .all(|receipt| receipt.capture.claim_rollout_stage == "evidence")
        );
        compare_independent_runs(&incumbent, &candidate, &plan, &manifest, &probes)
            .expect("paired comparison allows only the preregistered stage difference");
    }
    #[test]
    fn paired_probe_rejects_changed_launch_configuration() {
        struct Changed;
        impl IndependentProbeBackend for Changed {
            fn capture(
                &mut self,
                arm: RunArm,
                input: &ProbeInput,
                history: &[SeedEpisode],
            ) -> Result<ProbeCapture, EvalError> {
                let mut c = ControlledProbe.capture(arm, input, history)?;
                c.extractor = "anno".into();
                c.launch_profile
                    .insert("NER_EXTRACTOR".into(), "anno".into());
                Ok(c)
            }
        }
        let (m, p, probes) = fixture();
        let out = tempfile::tempdir().unwrap();
        assert!(
            run_with_independent_probes(
                &p,
                &m,
                &probes,
                &mut Agent {
                    retrieve: false,
                    answer: "North City",
                },
                &mut Changed,
                out.path()
            )
            .is_err(),
            "a self-consistent but unregistered profile must fail"
        );
    }

    #[test]
    fn missing_probe_never_falls_back_to_agent_coverage() {
        let (m, p, mut probes) = fixture();
        probes.pop();
        let out = tempfile::tempdir().unwrap();
        assert!(
            run_with_independent_probes(
                &p,
                &m,
                &probes,
                &mut Agent {
                    retrieve: true,
                    answer: "North City",
                },
                &mut ControlledProbe,
                out.path()
            )
            .is_err()
        );
        assert!(!out.path().join("incumbent.json").exists());
    }
    #[test]
    fn independent_envelope_roundtrips_offline_and_rejects_legacy_semantics() {
        let (m, p, probes) = fixture();
        let out = tempfile::tempdir().unwrap();
        let (a, _) = run_with_independent_probes(
            &p,
            &m,
            &probes,
            &mut Agent {
                retrieve: false,
                answer: "North City",
            },
            &mut ControlledProbe,
            out.path(),
        )
        .unwrap();
        let bytes = serde_json::to_vec(&a).unwrap();
        let external: IndependentPairedArtifact = serde_json::from_slice(&bytes).unwrap();
        external.validate(&p, &m, &probes).unwrap();
        assert!(
            serde_json::from_value::<IndependentPairedArtifact>(
                serde_json::to_value(&a.agent_observed).unwrap()
            )
            .is_err()
        );
    }
    #[test]
    fn offline_envelope_rejects_observation_detached_from_raw_response() {
        let (m, p, probes) = fixture();
        let out = tempfile::tempdir().unwrap();
        let (mut a, _) = run_with_independent_probes(
            &p,
            &m,
            &probes,
            &mut Agent {
                retrieve: false,
                answer: "North City",
            },
            &mut ControlledProbe,
            out.path(),
        )
        .unwrap();
        a.independent_memory[0]
            .capture
            .observation
            .source_event_ids
            .clear();
        assert!(a.validate(&p, &m, &probes).is_err());
    }
    #[test]
    fn comparison_rejects_wrong_arm_and_reports_independent_denominator() {
        let (m, p, probes) = fixture();
        let out = tempfile::tempdir().unwrap();
        let (a, b) = run_with_independent_probes(
            &p,
            &m,
            &probes,
            &mut Agent {
                retrieve: false,
                answer: "North City",
            },
            &mut ControlledProbe,
            out.path(),
        )
        .unwrap();
        let report = compare_independent_runs(&a, &b, &p, &m, &probes).unwrap();
        assert_eq!(report.trials, 2);
        assert_eq!(report.incumbent_evidence_successes, 2);
        assert!(compare_independent_runs(&b, &a, &p, &m, &probes).is_err());
    }
    #[test]
    #[ignore = "requires MEMORY_DECISIONS_SERVER_BIN; real isolated stdio composition"]
    fn real_public_probe_composes_with_paired_agent_runner_without_credentials() {
        let binary =
            std::path::PathBuf::from(std::env::var_os("MEMORY_DECISIONS_SERVER_BIN").unwrap());
        let (m, mut p, mut probes) = fixture();
        for fixed in &mut probes {
            fixed.input.known_revision = 0;
            fixed.oracle.sufficient_evidence_alternatives.clear();
            fixed.oracle.forbidden_event_ids = m
                .tasks
                .iter()
                .find(|t| t.key == fixed.key)
                .unwrap()
                .seed_episodes
                .iter()
                .map(|e| e.event_id.clone())
                .collect();
        }
        let product = sha256(&std::fs::read(&binary).unwrap());
        let scratch = tempfile::tempdir().unwrap();
        let preflight = crate::independent_probe::capture_stdio_probe(
            &probes[0].input,
            &m.tasks[0].seed_episodes,
            &binary,
            &product,
            &scratch.path().join("preflight"),
            "shadow",
        )
        .unwrap();
        for fixed in &mut probes {
            fixed.launch_profile = preflight.launch_profile.clone();
        }
        p.incumbent_product.binary_sha256 = product;
        p.incumbent_product.tool_schema_sha256 = preflight.tool_schema_sha256;
        p.candidate_product = p.incumbent_product.clone();
        let out = scratch.path().join("paired");
        let (a, b) = run_with_public_probes(
            &p,
            &m,
            &probes,
            &mut Agent {
                retrieve: false,
                answer: "North City",
            },
            (binary.clone(), binary),
            &out,
        )
        .unwrap();
        let report = compare_independent_runs(&a, &b, &p, &m, &probes).unwrap();
        assert_eq!(report.incumbent_evidence_successes, 2);
        assert_eq!(report.candidate_evidence_successes, 2);
        assert!(!a.agent_observed.receipts[0].direct_memory_success);
    }
    #[test]
    fn offline_capture_requires_the_complete_seeded_history_even_when_empty() {
        let (m, p, probes) = fixture();
        let input = &probes[0].input;
        let mut capture = ControlledProbe
            .capture(RunArm::Incumbent, input, &m.tasks[0].seed_episodes)
            .unwrap();
        capture.episode_id_to_event_id.clear();
        capture.observation.source_event_ids.clear();
        capture.observation.budget_used = 0;
        capture.response["result"]["structuredContent"]["result"] = serde_json::json!([]);
        assert!(
            capture
                .validate(
                    input,
                    &m.tasks[0].seed_episodes,
                    &p.incumbent_product.binary_sha256,
                    "stdio",
                    &p.incumbent_product.tool_schema_sha256
                )
                .is_err()
        );
    }
    #[test]
    fn offline_capture_cannot_score_a_tool_error_as_success() {
        let (m, p, probes) = fixture();
        let input = &probes[0].input;
        let mut capture = ControlledProbe
            .capture(RunArm::Incumbent, input, &m.tasks[0].seed_episodes)
            .unwrap();
        capture.response["result"]["isError"] = serde_json::json!(true);
        assert!(
            capture
                .validate(
                    input,
                    &m.tasks[0].seed_episodes,
                    &p.incumbent_product.binary_sha256,
                    "stdio",
                    &p.incumbent_product.tool_schema_sha256
                )
                .is_err()
        );
    }
}
