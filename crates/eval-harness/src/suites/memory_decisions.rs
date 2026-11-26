mod acceptance;
mod cases;
pub use acceptance::{
    AcceptanceScoreChannel, CrossBuildBinaryPins, DeterministicAcceptancePlan,
    DeterministicAcceptanceReport, accept_candidate,
};
mod comparison;
mod normalization;
mod receipts;
mod scoring;
pub(crate) mod stdio;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::domain::*;
use crate::error::EvalError;
use crate::profile::SuiteDecl;
use crate::reducer::SuiteReducer;
use crate::runner::{EvalSuite, RunContext};

use self::cases::{DecisionCorpus, DecisionOracle, DecisionProbe, MemoryEvent};
pub use self::comparison::{
    CurrentRegressionReport, compare_current_replays, compare_replays, grader_identity,
};
use self::normalization::normalize_observation;
pub use self::receipts::ReplayIndex;
use self::receipts::{
    REPLAY_SCHEMA_V1, ReplayCase, ReplayIdentity, hash_corpus, hash_file, sha256, write_index,
    write_receipt,
};
pub use self::verification::{
    load_current_regression_replay, load_registered_replay, load_verified_replay,
};
mod verification;
use self::scoring::{
    Observed, ProbeScore, ScoreReason, ScoringObservationSource, score_probe,
    select_scoring_observation,
};
use self::stdio::{MeasuredToolResponse, StdioSession};

pub struct MemoryDecisionsSuite {
    corpus: DecisionCorpus,
    expected_ids: Vec<EvalCaseId>,
    reducer: MemoryDecisionsReducer,
    binary: Option<PathBuf>,
    binary_sha256: Option<String>,
    product_revision: Option<String>,
    artifact_root: PathBuf,
    configuration_error: Option<String>,
    primary_claim_rollout_stage: String,
    diagnostics: bool,
    configuration: String,
    corpus_sha256: String,
    config_sha256: String,
    metadata: Arc<Mutex<BTreeMap<String, String>>>,
}

impl MemoryDecisionsSuite {
    pub fn from_decl(decl: &SuiteDecl) -> Result<Self, EvalError> {
        let root = decl.corpus_root.as_deref().ok_or_else(|| {
            EvalError::InvalidConfig("memory-decisions requires corpus_root".into())
        })?;
        let corpus_root = PathBuf::from(root);
        let mut corpus = DecisionCorpus::load(&corpus_root)?;
        if !decl.case_ids.is_empty() {
            let selected: std::collections::BTreeSet<_> = decl.case_ids.iter().collect();
            if selected.len() != decl.case_ids.len()
                || selected
                    .iter()
                    .any(|id| !corpus.probes.iter().any(|probe| &probe.id == *id))
            {
                return Err(EvalError::InvalidConfig(
                    "case_ids must name distinct registered memory-decision probes".into(),
                ));
            }
            corpus.probes.retain(|probe| selected.contains(&probe.id));
        }
        let primary_claim_rollout_stage = decl
            .effective_memory_decision_claim_rollout_stage()
            .to_owned();
        if !matches!(primary_claim_rollout_stage.as_str(), "default" | "shadow") {
            return Err(EvalError::InvalidConfig(format!(
                "memory-decisions supports claim_rollout_stage default or shadow, got `{primary_claim_rollout_stage}`"
            )));
        }
        let expected_ids = corpus
            .probes
            .iter()
            .map(|probe| EvalCaseId::parse(&probe.id))
            .collect::<Result<Vec<_>, _>>()?;
        let mut errors = Vec::new();

        let binary = match std::env::var_os("MEMORY_DECISIONS_SERVER_BIN") {
            Some(value) => {
                let path = PathBuf::from(value);
                if !path.is_absolute() || !path.is_file() {
                    errors.push(
                        "MEMORY_DECISIONS_SERVER_BIN must name an existing absolute executable"
                            .to_string(),
                    );
                    None
                } else {
                    Some(path)
                }
            }
            None => {
                errors.push("MEMORY_DECISIONS_SERVER_BIN is required".to_string());
                None
            }
        };
        let binary_sha256 = binary.as_deref().and_then(|path| hash_file(path).ok());
        if binary.is_some() && binary_sha256.is_none() {
            errors.push("could not hash the configured product executable".to_string());
        }

        let product_revision = std::env::var("MEMORY_DECISIONS_PRODUCT_REVISION").ok();
        if product_revision.as_deref().is_none_or(str::is_empty) {
            errors.push("MEMORY_DECISIONS_PRODUCT_REVISION is required".to_string());
        }

        let artifact_root = match std::env::var_os("MEMORY_DECISIONS_ARTIFACT_ROOT") {
            Some(value) => {
                let path = PathBuf::from(value);
                if !path.is_absolute() {
                    errors.push("MEMORY_DECISIONS_ARTIFACT_ROOT must be absolute".to_string());
                    fallback_artifact_root()
                } else if let Err(error) = std::fs::create_dir_all(&path) {
                    errors.push(format!("cannot create receipt root: {error}"));
                    fallback_artifact_root()
                } else if path.join("index.json").exists() {
                    errors.push(
                        "receipt root already contains index.json; use a fresh directory"
                            .to_string(),
                    );
                    fallback_artifact_root()
                } else {
                    path
                }
            }
            None => {
                errors.push("MEMORY_DECISIONS_ARTIFACT_ROOT is required".to_string());
                fallback_artifact_root()
            }
        };
        std::fs::create_dir_all(&artifact_root).map_err(|source| EvalError::Io {
            path: artifact_root.clone(),
            source,
        })?;
        let corpus_sha256 = hash_corpus(&corpus_root)?;
        let configuration = deterministic_configuration_for(&primary_claim_rollout_stage);
        let config_sha256 = sha256(configuration.as_bytes());

        let mut metadata = BTreeMap::from([
            ("corpus_sha256".into(), corpus_sha256.clone()),
            ("config_sha256".into(), config_sha256.clone()),
            ("provider_profile".into(), configuration.clone()),
        ]);
        if let Some(binary) = &binary {
            metadata.insert("product_binary".into(), binary.display().to_string());
        }
        if let Some(binary_hash) = &binary_sha256 {
            metadata.insert("product_binary_sha256".into(), binary_hash.clone());
        }
        if let Some(revision) = &product_revision {
            metadata.insert("product_revision".into(), revision.clone());
        }

        Ok(Self {
            corpus,
            expected_ids,
            reducer: MemoryDecisionsReducer::new("memory-decisions"),
            binary,
            binary_sha256,
            product_revision,
            artifact_root,
            configuration_error: (!errors.is_empty()).then(|| errors.join("; ")),
            primary_claim_rollout_stage,
            diagnostics: decl.diagnostics.unwrap_or(false),
            configuration,
            corpus_sha256,
            config_sha256,
            metadata: Arc::new(Mutex::new(metadata)),
        })
    }
}

#[async_trait]
impl EvalSuite for MemoryDecisionsSuite {
    fn id(&self) -> &str {
        "memory-decisions"
    }

    fn mode(&self) -> EvalMode {
        EvalMode::EndToEnd
    }

    fn expected_case_ids(&self) -> &[EvalCaseId] {
        &self.expected_ids
    }

    fn reducer(&self) -> &dyn SuiteReducer {
        &self.reducer
    }

    fn fingerprint_metadata(&self) -> BTreeMap<String, String> {
        self.metadata
            .lock()
            .map(|metadata| metadata.clone())
            .unwrap_or_default()
    }

    async fn run(&self, context: &RunContext) -> Vec<EvalCaseOutcome> {
        self.run_until(context, None).await
    }

    async fn run_with_deadline(
        &self,
        context: &RunContext,
        deadline: Instant,
    ) -> Vec<EvalCaseOutcome> {
        self.run_until(context, Some(deadline)).await
    }
}

impl MemoryDecisionsSuite {
    async fn run_until(
        &self,
        _context: &RunContext,
        deadline: Option<Instant>,
    ) -> Vec<EvalCaseOutcome> {
        let corpus = self.corpus.clone();
        let binary = self.binary.clone();
        let binary_sha256 = self.binary_sha256.clone();
        let product_revision = self.product_revision.clone();
        let artifact_root = self.artifact_root.clone();
        let corpus_sha256 = self.corpus_sha256.clone();
        let config_sha256 = self.config_sha256.clone();
        let primary_claim_rollout_stage = self.primary_claim_rollout_stage.clone();
        let diagnostics = self.diagnostics;
        let configuration = self.configuration.clone();
        let configuration_error = self.configuration_error.clone();
        let metadata = Arc::clone(&self.metadata);

        match tokio::task::spawn_blocking(move || {
            run_corpus(
                corpus,
                binary,
                binary_sha256,
                product_revision,
                artifact_root,
                corpus_sha256,
                config_sha256,
                primary_claim_rollout_stage,
                configuration,
                configuration_error,
                metadata,
                deadline,
                diagnostics,
            )
        })
        .await
        {
            Ok(outcomes) => outcomes,
            Err(error) => self
                .corpus
                .probes
                .iter()
                .map(|probe| {
                    invalid_outcome(&probe.id, format!("evaluation worker failed: {error}"), 0)
                })
                .collect(),
        }
    }
}

pub(super) struct MemoryDecisionsReducer {
    suite_id: SuiteId,
}

impl MemoryDecisionsReducer {
    pub(super) fn new(suite_id: impl Into<String>) -> Self {
        Self {
            suite_id: SuiteId::parse(suite_id).expect("static suite ID is nonempty"),
        }
    }
}

impl SuiteReducer for MemoryDecisionsReducer {
    fn suite_id(&self) -> &SuiteId {
        &self.suite_id
    }

    fn reduce(
        &self,
        outcomes: &[EvalCaseOutcome],
    ) -> Result<Vec<crate::artifact::SuiteSummary>, EvalError> {
        let mut passed = 0;
        let mut quality_failed = 0;
        let mut invalid = 0;
        let mut totals = [[0_u64; 2]; 9];
        for outcome in outcomes {
            match outcome.status {
                CaseStatus::Passed => passed += 1,
                CaseStatus::QualityFailed => quality_failed += 1,
                CaseStatus::Invalid => invalid += 1,
            }
            if outcome.status == CaseStatus::Invalid {
                continue;
            }
            for (index, key) in [
                "evidence_success",
                "decision_success",
                "probe_success",
                "temporal_evidence_success",
                "temporal_decision_success",
                "temporal_probe_success",
                "evidence_stage_temporal_evidence_success",
                "evidence_stage_temporal_decision_success",
                "evidence_stage_temporal_probe_success",
            ]
            .into_iter()
            .enumerate()
            {
                let value = outcome.evidence.get(key);
                if index >= 3 && value.is_none() {
                    continue;
                }
                let Some(MetricEvidence::Ratio {
                    numerator,
                    denominator,
                }) = value
                else {
                    return Err(EvalError::InvalidInput(format!(
                        "memory-decisions case {} lacks `{key}` ratio evidence",
                        outcome.case_id().as_str()
                    )));
                };
                totals[index][0] = totals[index][0]
                    .checked_add(*numerator)
                    .ok_or_else(|| EvalError::InvalidInput(format!("{key} numerator overflow")))?;
                totals[index][1] = totals[index][1].checked_add(*denominator).ok_or_else(|| {
                    EvalError::InvalidInput(format!("{key} denominator overflow"))
                })?;
            }
        }
        let mut metrics = BTreeMap::new();
        for (index, name) in [
            "evidence_success_rate",
            "decision_success_rate",
            "probe_success_rate",
            "temporal_evidence_success_rate",
            "temporal_decision_success_rate",
            "temporal_probe_success_rate",
            "evidence_stage_temporal_evidence_success_rate",
            "evidence_stage_temporal_decision_success_rate",
            "evidence_stage_temporal_probe_success_rate",
        ]
        .into_iter()
        .enumerate()
        {
            let [numerator, denominator] = totals[index];
            if index >= 3 && denominator == 0 {
                continue;
            }
            metrics.insert(
                name.to_string(),
                if denominator == 0 {
                    0.0
                } else {
                    numerator as f64 / denominator as f64
                },
            );
        }
        Ok(vec![crate::artifact::SuiteSummary {
            suite_id: self.suite_id.as_str().to_string(),
            mode: outcomes
                .first()
                .map_or(EvalMode::EndToEnd, |outcome| outcome.mode),
            total: outcomes.len(),
            passed,
            quality_failed,
            invalid,
            metrics,
        }])
    }
}

#[allow(clippy::too_many_arguments)]
fn run_corpus(
    corpus: DecisionCorpus,
    binary: Option<PathBuf>,
    binary_sha256: Option<String>,
    product_revision: Option<String>,
    artifact_root: PathBuf,
    corpus_sha256: String,
    config_sha256: String,
    primary_claim_rollout_stage: String,
    configuration: String,
    configuration_error: Option<String>,
    metadata: Arc<Mutex<BTreeMap<String, String>>>,
    deadline: Option<Instant>,
    diagnostics: bool,
) -> Vec<EvalCaseOutcome> {
    let run_started = Instant::now();
    let mut outcomes = Vec::with_capacity(corpus.probes.len());
    let mut replay_cases = Vec::with_capacity(corpus.probes.len());
    let mut preflight_failure = configuration_error;
    let mut schema_sha256 = None;
    let expected_product_revision = product_revision.unwrap_or_else(|| "unprovided".into());
    let expected_binary_hash = binary_sha256.unwrap_or_else(|| "unavailable".into());

    for probe in &corpus.probes {
        let case_started = Instant::now();
        if deadline.is_some_and(|limit| Instant::now() >= limit) {
            preflight_failure = Some("whole-run deadline exhausted before this case".into());
        }
        let attempted = preflight_failure.is_none() && binary.is_some();
        let oracle = corpus.oracles.get(&probe.id);
        let mut temporal_score_for_outcome = None;
        let mut evidence_stage_temporal_score_for_outcome = None;
        let mut temporal_observation_for_replay = None;
        let mut evidence_stage_temporal_observation_for_replay = None;
        let mut scored_read_metrics = None;
        let (mut status, mut score, receipt_value, mut observed_for_index, mut failure) =
            if let Some(reason) = &preflight_failure {
                (
                    CaseStatus::Invalid,
                    None,
                    failure_receipt(
                        probe,
                        oracle,
                        &expected_product_revision,
                        &expected_binary_hash,
                        reason,
                        "preflight",
                        &configuration,
                    ),
                    None,
                    Some(reason.clone()),
                )
            } else {
                match binary.as_deref() {
                    None => {
                        let reason = "product binary was not configured".to_string();
                        preflight_failure = Some(reason.clone());
                        (
                            CaseStatus::Invalid,
                            None,
                            failure_receipt(
                                probe,
                                oracle,
                                &expected_product_revision,
                                &expected_binary_hash,
                                &reason,
                                "preflight",
                                &configuration,
                            ),
                            None,
                            Some(reason),
                        )
                    }
                    Some(binary) => {
                        match execute_variant(
                            &corpus,
                            probe,
                            binary,
                            false,
                            probe.as_of,
                            &primary_claim_rollout_stage,
                            deadline,
                        ) {
                            Ok(primary) => {
                                if schema_sha256.is_none() {
                                    schema_sha256 =
                                        Some(sha256(primary.tool_schemas.to_string().as_bytes()));
                                }
                                let primary_observed = primary.observations.first().cloned();
                                let scoring_observation = select_scoring_observation(
                                    probe.requires_separate_time_cutoffs,
                                    primary_observed.as_ref(),
                                    primary.temporal_observation.as_ref(),
                                    primary.temporal_snapshot_valid,
                                );
                                let score_source =
                                    scoring_observation.map(|selected| selected.source);
                                scored_read_metrics = score_source.and_then(|source| {
                                    let measurement = match source {
                                        ScoringObservationSource::TemporalRevision => {
                                            primary.temporal_read_measurement.as_ref()
                                        }
                                        ScoringObservationSource::Primary => {
                                            primary.assemble_read_measurements.first()
                                        }
                                    }?;
                                    let observed = match source {
                                        ScoringObservationSource::TemporalRevision => {
                                            primary.temporal_observation.as_ref()
                                        }
                                        ScoringObservationSource::Primary => {
                                            primary.observations.first()
                                        }
                                    }?;
                                    Some(ScoredReadMetrics {
                                        elapsed_us: measurement.elapsed_us,
                                        compact_jsonrpc_bytes: measurement.compact_jsonrpc_bytes,
                                        item_count: observed.item_count,
                                        budget_used: observed.budget_used,
                                        requested_budget: probe.budget,
                                    })
                                });
                                let mut score = scoring_observation.and_then(|selected| {
                                    oracle.map(|oracle| {
                                        let mut score = score_probe(
                                            oracle,
                                            selected.observed,
                                            probe.budget,
                                            selected.time_contract_supported,
                                        );
                                        enforce_primary_reads(
                                            &mut score,
                                            &primary.observations,
                                            (selected.source == ScoringObservationSource::Primary)
                                                .then_some(oracle),
                                            probe.budget,
                                        );
                                        score
                                    })
                                });
                                let score = score.take();
                                let scoring_observed =
                                    scoring_observation.map(|selected| selected.observed.clone());
                                let temporal_score =
                                    primary.temporal_observation.as_ref().and_then(|observed| {
                                        oracle.map(|oracle| {
                                            score_probe(
                                                oracle,
                                                observed,
                                                probe.budget,
                                                primary.temporal_snapshot_valid,
                                            )
                                        })
                                    });
                                temporal_score_for_outcome = temporal_score.clone();
                                temporal_observation_for_replay =
                                    primary.temporal_observation.clone();
                                let primary_status =
                                    if score.as_ref().is_some_and(|score| score.probe_success) {
                                        CaseStatus::Passed
                                    } else {
                                        CaseStatus::QualityFailed
                                    };
                                let verbose = diagnostics.then(|| {
                                    execute_variant(
                                        &corpus, probe, binary, true, None, "shadow", deadline,
                                    )
                                });
                                let evidence_diagnostic = diagnostics.then(|| {
                                    execute_variant(
                                        &corpus, probe, binary, false, None, "evidence", deadline,
                                    )
                                });
                                let evidence_temporal_score = evidence_diagnostic
                                    .as_ref()
                                    .and_then(|run| run.as_ref().ok())
                                    .and_then(|trace| {
                                        trace.temporal_observation.as_ref().and_then(|observed| {
                                            oracle.map(|oracle| {
                                                score_probe(
                                                    oracle,
                                                    observed,
                                                    probe.budget,
                                                    trace.temporal_snapshot_valid,
                                                )
                                            })
                                        })
                                    });
                                evidence_stage_temporal_score_for_outcome =
                                    evidence_temporal_score.clone();
                                evidence_stage_temporal_observation_for_replay =
                                    evidence_diagnostic
                                        .as_ref()
                                        .and_then(|run| run.as_ref().ok())
                                        .and_then(|trace| trace.temporal_observation.clone());
                                let receipt = successful_receipt(SuccessfulReceipt {
                                    corpus: &corpus,
                                    probe,
                                    oracle,
                                    primary: &primary,
                                    verbose: verbose.as_ref().and_then(|run| run.as_ref().ok()),
                                    diagnostics_requested: diagnostics,
                                    evidence_diagnostic: evidence_diagnostic
                                        .as_ref()
                                        .and_then(|run| run.as_ref().ok()),
                                    score_source,
                                    score: score.as_ref(),
                                    temporal_score: temporal_score.as_ref(),
                                    evidence_temporal_score: evidence_temporal_score.as_ref(),
                                    product_revision: &expected_product_revision,
                                    binary_sha256: &expected_binary_hash,
                                    corpus_sha256: &corpus_sha256,
                                    config_sha256: &config_sha256,
                                    configuration: &configuration,
                                });
                                let mut failure = None;
                                let final_status = primary_status;
                                if let Some(Err(error)) = verbose {
                                    // The verbose arm is diagnostic and never changes the compact score.
                                    failure = Some(format!(
                                        "verbose diagnostic unavailable: {}",
                                        error.message
                                    ));
                                }
                                if let Some(Err(error)) = evidence_diagnostic {
                                    let detail = format!(
                                        "evidence-stage diagnostic unavailable: {}",
                                        error.message
                                    );
                                    failure = Some(failure.map_or(detail.clone(), |previous| {
                                        format!("{previous}; {detail}")
                                    }));
                                }
                                (final_status, score, receipt, scoring_observed, failure)
                            }
                            Err(error) => {
                                if !error.preflight_passed {
                                    preflight_failure = Some(error.message.clone());
                                }
                                let status = if error.invalid_evaluation || !error.preflight_passed
                                {
                                    CaseStatus::Invalid
                                } else {
                                    CaseStatus::QualityFailed
                                };
                                (
                                    status,
                                    None,
                                    failure_receipt(
                                        probe,
                                        oracle,
                                        &expected_product_revision,
                                        &expected_binary_hash,
                                        &error.message,
                                        &error.stage,
                                        &configuration,
                                    ),
                                    None,
                                    Some(error.message),
                                )
                            }
                        }
                    }
                }
            };

        let case_id = probe.id.as_str();
        let (receipt_path, receipt_hash) =
            match write_receipt(&artifact_root, case_id, &receipt_value) {
                Ok((path, hash)) => (path, hash),
                Err(error) => {
                    let reason = format!("failed to persist receipt: {error}");
                    preflight_failure = Some(reason.clone());
                    status = CaseStatus::Invalid;
                    score = None;
                    temporal_score_for_outcome = None;
                    evidence_stage_temporal_score_for_outcome = None;
                    temporal_observation_for_replay = None;
                    evidence_stage_temporal_observation_for_replay = None;
                    observed_for_index = None;
                    failure = Some(reason);
                    (
                        artifact_root.join(format!("receipt-{case_id}.json")),
                        String::new(),
                    )
                }
            };
        let mut outcome = make_outcome(
            case_id,
            status,
            score.as_ref(),
            temporal_score_for_outcome.as_ref(),
            evidence_stage_temporal_score_for_outcome.as_ref(),
            failure,
            case_started.elapsed().as_millis() as u64,
        );
        outcome.attempts = u32::from(attempted);
        if let Some(metrics) = scored_read_metrics {
            outcome
                .metrics
                .insert("scored_query_latency_us".into(), metrics.elapsed_us as f64);
            outcome.metrics.insert(
                "scored_query_compact_jsonrpc_bytes".into(),
                metrics.compact_jsonrpc_bytes as f64,
            );
            outcome
                .metrics
                .insert("scored_query_item_count".into(), metrics.item_count as f64);
            outcome.metrics.insert(
                "scored_query_budget_used".into(),
                metrics.budget_used as f64,
            );
            outcome.metrics.insert(
                "scored_query_requested_budget".into(),
                metrics.requested_budget as f64,
            );
        }
        if status == CaseStatus::Invalid {
            outcome.invalid_reason = outcome.failures.first().cloned();
        }
        outcomes.push(outcome);
        replay_cases.push(replay_case(ReplayCaseInput {
            case_id,
            observed: observed_for_index.as_ref(),
            score_source: if probe.requires_separate_time_cutoffs
                && temporal_observation_for_replay.is_some()
            {
                Some(ScoringObservationSource::TemporalRevision)
            } else {
                Some(ScoringObservationSource::Primary)
            },
            score: score.as_ref(),
            temporal_observation: temporal_observation_for_replay.as_ref(),
            temporal_score: temporal_score_for_outcome.as_ref(),
            evidence_enabled_temporal_observation: evidence_stage_temporal_observation_for_replay
                .as_ref(),
            evidence_enabled_temporal_score: evidence_stage_temporal_score_for_outcome.as_ref(),
            receipt_path: &receipt_path,
            receipt_hash: &receipt_hash,
        }));
    }

    let index = ReplayIndex {
        schema_version: REPLAY_SCHEMA_V1.into(),
        identity: ReplayIdentity {
            suite_id: "memory-decisions".into(),
            product_revision: expected_product_revision,
            product_binary_sha256: expected_binary_hash,
            corpus_sha256: corpus_sha256.clone(),
            config_sha256: config_sha256.clone(),
            input_schema_sha256: schema_sha256.clone(),
        },
        cases: replay_cases,
    };
    let index_result = write_index(&artifact_root, &index);
    let index_hash = index_result.as_ref().ok().map(|(_, hash)| hash.clone());
    if let Some(error) = index_result.err() {
        for outcome in &mut outcomes {
            if outcome.status != CaseStatus::Invalid {
                outcome.status = CaseStatus::Invalid;
                outcome.invalid_reason = Some(format!("failed to persist replay index: {error}"));
                outcome
                    .failures
                    .push("replay_index_persistence_failed".into());
                outcome.evidence.clear();
            }
        }
    }
    let mut output_metadata = BTreeMap::from([
        ("replay_cases".into(), outcomes.len().to_string()),
        (
            "replay_elapsed_ms".into(),
            run_started.elapsed().as_millis().to_string(),
        ),
    ]);
    if let Some(schema_hash) = schema_sha256 {
        output_metadata.insert("input_schema_sha256".into(), schema_hash);
    }
    if let Some(index_hash) = index_hash {
        output_metadata.insert("receipt_index_sha256".into(), index_hash);
        output_metadata.insert(
            "receipt_index".into(),
            artifact_root.join("index.json").display().to_string(),
        );
    }
    if let Ok(mut stored) = metadata.lock() {
        stored.extend(output_metadata);
    }
    outcomes
}

struct VariantTrace {
    stage_timings_us: BTreeMap<String, u64>,
    claim_rollout_stage: String,
    launch_profile: BTreeMap<String, String>,
    tool_schemas: Value,
    initialize_response: Value,
    ingest_receipts: Vec<Value>,
    extract_receipts: Vec<Value>,
    bindings: BTreeMap<String, String>,
    assemble_responses: Vec<Value>,
    observations: Vec<Observed>,
    explain_response: Option<Value>,
    explain_error: Option<String>,
    temporal_diagnostic_request: Option<Value>,
    temporal_diagnostic_response: Option<Value>,
    revision_checkpoint_receipts: Vec<Value>,
    temporal_request: Option<Value>,
    temporal_response: Option<Value>,
    temporal_observation: Option<Observed>,
    temporal_snapshot_valid: bool,
    temporal_read_measurement: Option<ReadMeasurement>,
    assemble_read_measurements: Vec<ReadMeasurement>,
    stderr_excerpt: String,
}

#[derive(Debug, Clone, Copy)]
struct ReadMeasurement {
    elapsed_us: u64,
    compact_jsonrpc_bytes: usize,
}

impl From<&MeasuredToolResponse> for ReadMeasurement {
    fn from(measured: &MeasuredToolResponse) -> Self {
        Self {
            elapsed_us: measured.elapsed_us,
            compact_jsonrpc_bytes: measured.compact_jsonrpc_bytes,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ScoredReadMetrics {
    elapsed_us: u64,
    compact_jsonrpc_bytes: usize,
    item_count: usize,
    budget_used: usize,
    requested_budget: usize,
}

struct VariantFailure {
    preflight_passed: bool,
    invalid_evaluation: bool,
    stage: String,
    message: String,
}

fn execute_variant(
    corpus: &DecisionCorpus,
    probe: &DecisionProbe,
    binary: &Path,
    verbose: bool,
    as_of_override: Option<chrono::DateTime<chrono::Utc>>,
    claim_stage: &str,
    deadline: Option<Instant>,
) -> Result<VariantTrace, VariantFailure> {
    let variant = match (verbose, claim_stage) {
        (true, _) => "verbose",
        (false, "evidence") => "evidence",
        (false, _) => "compact",
    };
    let started = Instant::now();
    let owned_store = ScenarioStore(fresh_store(&probe.id, variant));
    let store = &owned_store.0;
    std::fs::create_dir_all(store).map_err(|error| VariantFailure {
        preflight_passed: false,
        invalid_evaluation: true,
        stage: "store_preparation".into(),
        message: error.to_string(),
    })?;
    let prepared = Instant::now();
    let session = if let Some(limit) = deadline {
        StdioSession::spawn_with_deadline(binary, store, claim_stage, limit)
    } else if claim_stage == "default" {
        StdioSession::spawn(binary, store)
    } else {
        StdioSession::spawn_with_claim_stage(binary, store, claim_stage)
    };
    let mut session = session.map_err(|error| VariantFailure {
        preflight_passed: false,
        invalid_evaluation: true,
        stage: "server_preflight".into(),
        message: error.to_string(),
    })?;
    let ready = Instant::now();
    let initialize_response = session.initialized().clone();
    let tool_schemas = session.tool_schemas().clone();
    let launch_profile = session.launch_profile.clone();
    let mut ingest_receipts = Vec::new();
    let mut extract_receipts = Vec::new();
    let mut bindings = BTreeMap::new();
    let captures_revision_checkpoints = supports_temporal_revision(&tool_schemas);
    let mut revision_checkpoints = BTreeMap::<String, i64>::new();
    let mut revision_checkpoint_receipts = Vec::new();

    for event_id in &probe.event_ids {
        let Some(event) = corpus.events.iter().find(|event| event.id == *event_id) else {
            return Err(VariantFailure {
                preflight_passed: true,
                invalid_evaluation: true,
                stage: "corpus_binding".into(),
                message: format!("probe references missing event `{event_id}`"),
            });
        };
        let ingest_request = ingest_arguments(event);
        let ingest_response = session
            .call_tool("ingest", ingest_request.clone())
            .map_err(|error| product_failure("ingest", error))?;
        let episode_id = structured_result(&ingest_response)
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| VariantFailure {
                preflight_passed: true,
                invalid_evaluation: true,
                stage: "ingest_normalization".into(),
                message: format!("ingest response for `{}` lacks episode_id", event.id),
            })?
            .to_string();
        bindings.insert(episode_id.clone(), event.id.clone());
        ingest_receipts.push(json!({
            "event_id":event.id,
            "request":ingest_request,
            "response":ingest_response,
            "episode_id":episode_id,
            "t_ingested":"server_assigned_not_supplied"
        }));

        let extract_request = json!({"episode_id":episode_id});
        let extract_response = session
            .call_tool("extract", extract_request.clone())
            .map_err(|error| product_failure("extract", error))?;
        extract_receipts.push(json!({
            "event_id":event.id,
            "request":extract_request,
            "response":extract_response
        }));

        if captures_revision_checkpoints {
            let request = json!({
                "query":"",
                "valid_at":event.t_ref.to_rfc3339(),
                "budget":1,
                "compact":true
            });
            let response = session
                .call_tool("assemble_context", request.clone())
                .map_err(|error| product_failure("revision_checkpoint", error))?;
            let revision = response
                .pointer("/result/structuredContent/context/snapshot/known_revision")
                .and_then(Value::as_i64)
                .ok_or_else(|| VariantFailure {
                    preflight_passed: true,
                    invalid_evaluation: true,
                    stage: "revision_checkpoint_normalization".into(),
                    message: format!(
                        "revision checkpoint after `{}` lacks a snapshot token",
                        event.id
                    ),
                })?;
            revision_checkpoints.insert(event.id.clone(), revision);
            revision_checkpoint_receipts.push(json!({
                "after_event_id":event.id,
                "request":request,
                "response":response,
                "known_revision":revision
            }));
        }
    }

    let formed = Instant::now();
    let mut temporal_read_measurement = None;
    let (temporal_request, temporal_response, temporal_observation, temporal_snapshot_valid) =
        if captures_revision_checkpoints {
            let known_revision = temporal_revision_for_probe(probe, &revision_checkpoints)?;
            let request = json!({
                "query":probe.query,
                "valid_at":probe.valid_at.to_rfc3339(),
                "known_revision":known_revision,
                "budget":probe.budget,
                "compact":true
            });
            let measured = session
                .call_tool_measured("assemble_context", request.clone())
                .map_err(|error| product_failure("temporal_assemble_context", error))?;
            temporal_read_measurement = Some(ReadMeasurement::from(&measured));
            let response = measured.response;
            let observation = normalize_observation(std::slice::from_ref(&response), &bindings)
                .map_err(|error| VariantFailure {
                    preflight_passed: true,
                    invalid_evaluation: true,
                    stage: "temporal_response_normalization".into(),
                    message: error.to_string(),
                })?;
            let snapshot = response.pointer("/result/structuredContent/context/snapshot");
            let snapshot_valid = snapshot
                .and_then(|value| value.get("known_revision"))
                .and_then(Value::as_i64)
                == Some(known_revision)
                && snapshot
                    .and_then(|value| value.get("valid_at"))
                    .and_then(Value::as_str)
                    .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                    .is_some_and(|value| value.with_timezone(&chrono::Utc) == probe.valid_at);
            (
                Some(request),
                Some(response),
                Some(observation),
                snapshot_valid,
            )
        } else {
            (None, None, None, false)
        };

    let request_as_of = if verbose { None } else { as_of_override };
    let mut assemble_responses = Vec::new();
    let mut observations = Vec::new();
    let mut assemble_read_measurements = Vec::new();
    for _ in 0..probe.repeat_read {
        let arguments = assemble_arguments(probe, verbose, request_as_of);
        let measured = session
            .call_tool_measured("assemble_context", arguments)
            .map_err(|error| product_failure("assemble_context", error))?;
        assemble_read_measurements.push(ReadMeasurement::from(&measured));
        let response = measured.response;
        let observation = normalize_observation(std::slice::from_ref(&response), &bindings)
            .map_err(|error| VariantFailure {
                preflight_passed: true,
                invalid_evaluation: true,
                stage: "response_normalization".into(),
                message: error.to_string(),
            })?;
        assemble_responses.push(response);
        observations.push(observation);
    }
    let queried = Instant::now();
    let (explain_response, explain_error) =
        match explain_retrieved_items(&mut session, assemble_responses.first()) {
            Ok(response) => (response, None),
            Err(error) => (None, Some(error.to_string())),
        };
    let (temporal_diagnostic_request, temporal_diagnostic_response) = if !verbose {
        if let Some(as_of) = probe.temporal_diagnostic_as_of {
            let request = assemble_arguments(probe, false, Some(as_of));
            match session.call_tool("assemble_context", request.clone()) {
                Ok(response) => (Some(request), Some(response)),
                Err(error) => (
                    Some(request),
                    Some(json!({"diagnostic_error":error.to_string()})),
                ),
            }
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };
    let diagnosed = Instant::now();
    let stderr_excerpt = session.stderr_excerpt();
    session.shutdown().map_err(|error| VariantFailure {
        preflight_passed: true,
        invalid_evaluation: true,
        stage: "server_shutdown".into(),
        message: error.to_string(),
    })?;

    let closed = Instant::now();
    let micros = |from: Instant, to: Instant| {
        u64::try_from(to.duration_since(from).as_micros()).unwrap_or(u64::MAX)
    };
    let stage_timings_us = BTreeMap::from([
        ("preparation".into(), micros(started, prepared)),
        ("startup".into(), micros(prepared, ready)),
        ("formation".into(), micros(ready, formed)),
        ("query".into(), micros(formed, queried)),
        ("diagnostics".into(), micros(queried, diagnosed)),
        ("shutdown".into(), micros(diagnosed, closed)),
        ("wall".into(), micros(started, closed)),
    ]);
    Ok(VariantTrace {
        stage_timings_us,
        claim_rollout_stage: claim_stage.to_string(),
        launch_profile,
        tool_schemas,
        initialize_response,
        ingest_receipts,
        extract_receipts,
        bindings,
        assemble_responses,
        observations,
        explain_response,
        explain_error,
        temporal_diagnostic_request,
        temporal_diagnostic_response,
        revision_checkpoint_receipts,
        temporal_request,
        temporal_response,
        temporal_observation,
        temporal_snapshot_valid,
        temporal_read_measurement,
        assemble_read_measurements,
        stderr_excerpt,
    })
}

fn temporal_revision_for_probe(
    probe: &DecisionProbe,
    checkpoints: &BTreeMap<String, i64>,
) -> Result<i64, VariantFailure> {
    let boundary_revision = if let Some(boundary_event) = &probe.knowledge_before_event_id {
        let boundary_index = probe
            .event_ids
            .iter()
            .position(|event_id| event_id == boundary_event)
            .ok_or_else(|| VariantFailure {
                preflight_passed: true,
                invalid_evaluation: true,
                stage: "revision_checkpoint_selection".into(),
                message: format!("knowledge boundary `{boundary_event}` is outside the probe"),
            })?;
        boundary_index.checked_sub(1).map_or(Ok(0), |index| {
            checkpoints
                .get(&probe.event_ids[index])
                .copied()
                .ok_or_else(|| VariantFailure {
                    preflight_passed: true,
                    invalid_evaluation: true,
                    stage: "revision_checkpoint_selection".into(),
                    message: format!(
                        "probe `{}` lacks checkpoint before `{boundary_event}`",
                        probe.id
                    ),
                })
        })?
    } else {
        let event_id = probe.event_ids.last().ok_or_else(|| VariantFailure {
            preflight_passed: true,
            invalid_evaluation: true,
            stage: "revision_checkpoint_selection".into(),
            message: format!("probe `{}` contains no events", probe.id),
        })?;
        checkpoints
            .get(event_id)
            .copied()
            .ok_or_else(|| VariantFailure {
                preflight_passed: true,
                invalid_evaluation: true,
                stage: "revision_checkpoint_selection".into(),
                message: format!("probe `{}` lacks its final revision checkpoint", probe.id),
            })?
    };
    Ok(boundary_revision)
}

fn product_failure(stage: &str, error: EvalError) -> VariantFailure {
    let invalid_evaluation = matches!(
        error,
        EvalError::InvalidInput(_) | EvalError::Artifact(_) | EvalError::Timeout { .. }
    );
    VariantFailure {
        preflight_passed: true,
        invalid_evaluation,
        stage: stage.into(),
        message: error.to_string(),
    }
}

fn assemble_arguments(
    probe: &DecisionProbe,
    verbose: bool,
    as_of: Option<chrono::DateTime<chrono::Utc>>,
) -> Value {
    let mut arguments = json!({
        "query":probe.query,
        "budget":probe.budget,
        "compact":!verbose
    });
    if let Some(as_of) = as_of {
        arguments["as_of"] = Value::String(as_of.to_rfc3339());
    }
    arguments
}

fn ingest_arguments(event: &MemoryEvent) -> Value {
    json!({
        "source_type":event.source_type,
        "source_id":event.source_id,
        "content":event.content,
        "t_ref":event.t_ref.to_rfc3339()
    })
}

fn structured_result(response: &Value) -> Option<&Value> {
    response.pointer("/result/structuredContent/result")
}

fn explain_retrieved_items(
    session: &mut StdioSession,
    response: Option<&Value>,
) -> Result<Option<Value>, EvalError> {
    let Some(response) = response else {
        return Ok(None);
    };
    let Some(items) = structured_result(response).and_then(Value::as_array) else {
        return Ok(None);
    };
    if items.is_empty() {
        return Ok(None);
    }
    let context_pack: Vec<Value> = items
        .iter()
        .filter_map(|item| {
            let content = item.get("content")?.as_str()?;
            let source_episode = item.get("source_episode")?.as_str()?;
            Some(json!({
                "fact_id":item.get("fact_id"),
                "content":content,
                "quote":item.get("quote").and_then(Value::as_str).unwrap_or(content),
                "source_episode":source_episode,
                "provenance":item.get("provenance").cloned().unwrap_or(Value::Null)
            }))
        })
        .collect();
    if context_pack.is_empty() {
        return Ok(None);
    }
    session
        .call_tool("explain", explain_arguments(&context_pack)?)
        .map(Some)
}

fn explain_arguments(context_items: &[Value]) -> Result<Value, EvalError> {
    Ok(json!({
        "context_items":serde_json::to_string(context_items)?,
        "compact":true
    }))
}

fn supports_separate_time_cutoffs(schemas: &Value) -> bool {
    let Some(tools) = schemas.as_array() else {
        return false;
    };
    let Some(properties) = tools
        .iter()
        .find(|tool| tool.get("name").and_then(Value::as_str) == Some("assemble_context"))
        .and_then(|tool| tool.pointer("/inputSchema/properties"))
    else {
        return false;
    };
    ["valid_at", "valid_time", "t_ref"]
        .iter()
        .any(|key| properties.get(key).is_some())
        && ["known_at", "knowledge_at", "t_ingested"]
            .iter()
            .any(|key| properties.get(key).is_some())
}

fn supports_temporal_revision(schemas: &Value) -> bool {
    let Some(properties) = schemas
        .as_array()
        .and_then(|tools| {
            tools
                .iter()
                .find(|tool| tool.get("name").and_then(Value::as_str) == Some("assemble_context"))
        })
        .and_then(|tool| tool.pointer("/inputSchema/properties"))
    else {
        return false;
    };
    properties.get("valid_at").is_some() && properties.get("known_revision").is_some()
}

fn enforce_primary_reads(
    score: &mut ProbeScore,
    observations: &[Observed],
    oracle: Option<&DecisionOracle>,
    budget: usize,
) {
    for observation in observations {
        // Only apply the oracle to its registered request axes. An ordinary
        // latest read cannot be scored against a historical snapshot value.
        let Some(oracle) = oracle else {
            continue;
        };
        let read_score = score_probe(oracle, observation, budget, true);
        if !read_score.evidence_success {
            score.evidence_success = false;
            score.probe_success = false;
            score
                .reason_codes
                .extend(read_score.reason_codes.into_iter().filter(|reason| {
                    matches!(
                        reason,
                        ScoreReason::MissingEvidence
                            | ScoreReason::WrongAssertion
                            | ScoreReason::ForbiddenSource
                            | ScoreReason::UnboundSource
                            | ScoreReason::BudgetExceeded
                    )
                }));
        }
    }
    if observations.len() > 1 && !observations_repeat(observations) {
        score.evidence_success = false;
        score.decision_success = false;
        score.probe_success = false;
        score.reason_codes.push(ScoreReason::Nonrepeatable);
    }
    score
        .reason_codes
        .sort_by_key(|reason| serde_json::to_string(reason).unwrap_or_default());
    score.reason_codes.dedup();
}

fn observations_repeat(observations: &[Observed]) -> bool {
    let Some(first) = observations.first() else {
        return false;
    };
    observations.iter().skip(1).all(|observed| {
        observed.sources == first.sources
            && observed.disposition == first.disposition
            && observed.item_count == first.item_count
            && observed.budget_used == first.budget_used
            && observed.relation_outcomes == first.relation_outcomes
    })
}

struct SuccessfulReceipt<'a> {
    corpus: &'a DecisionCorpus,
    probe: &'a DecisionProbe,
    oracle: Option<&'a DecisionOracle>,
    primary: &'a VariantTrace,
    verbose: Option<&'a VariantTrace>,
    diagnostics_requested: bool,
    evidence_diagnostic: Option<&'a VariantTrace>,
    score_source: Option<ScoringObservationSource>,
    score: Option<&'a ProbeScore>,
    temporal_score: Option<&'a ProbeScore>,
    evidence_temporal_score: Option<&'a ProbeScore>,
    product_revision: &'a str,
    binary_sha256: &'a str,
    corpus_sha256: &'a str,
    config_sha256: &'a str,
    configuration: &'a str,
}

fn successful_receipt(input: SuccessfulReceipt<'_>) -> Value {
    let SuccessfulReceipt {
        corpus,
        probe,
        oracle,
        primary,
        verbose,
        diagnostics_requested,
        evidence_diagnostic,
        score_source,
        score,
        temporal_score,
        evidence_temporal_score,
        product_revision,
        binary_sha256,
        corpus_sha256,
        config_sha256,
        configuration,
    } = input;
    json!({
        "schema_version":if diagnostics_requested { "memory-decisions-receipt/v4" } else { "memory-decisions-receipt/v5" },
        "diagnostic_policy":if diagnostics_requested { "full" } else { "primary_only" },
        "case_id":probe.id,
        "product_revision":product_revision,
        "product_binary_sha256":binary_sha256,
        "corpus_sha256":corpus_sha256,
        "config_sha256":config_sha256,
        "provider_configuration":configuration,
        "desired_goal":{
            "valid_at":probe.valid_at,
            "knowledge_before_event_id":probe.knowledge_before_event_id,
            "requires_separate_time_cutoffs":probe.requires_separate_time_cutoffs,
            "oracle":oracle
        },
        "supported_api":{
            "input_schema":primary.tool_schemas,
            "independent_valid_and_known_cutoffs":supports_separate_time_cutoffs(&primary.tool_schemas)
        },
        "primary_claim_rollout_stage":primary.claim_rollout_stage,
        "primary_launch_profile":primary.launch_profile,
        "primary_stage_timings_us":primary.stage_timings_us,
        "score_observation_source":score_source.map(ScoringObservationSource::as_str),
        "initialize":primary.initialize_response,
        "events":probe.event_ids.iter().filter_map(|id| corpus.events.iter().find(|event| event.id == *id)).collect::<Vec<_>>(),
        "ingest_receipts":primary.ingest_receipts,
        "extract_receipts":primary.extract_receipts,
        "episode_bindings":primary.bindings,
        "primary_request":assemble_arguments(probe, false, probe.as_of),
        "primary_compact_responses":primary.assemble_responses,
        "primary_observation":primary.observations.first(),
        "repeat_observations":primary.observations,
        "explain_followup":primary.explain_response,
        "explain_error":primary.explain_error,
        "temporal_diagnostic_request":primary.temporal_diagnostic_request,
        "temporal_diagnostic_response":primary.temporal_diagnostic_response,
        "revision_checkpoint_receipts":primary.revision_checkpoint_receipts,
        "temporal_revision_request":primary.temporal_request,
        "temporal_revision_response":primary.temporal_response,
        "temporal_revision_observation":primary.temporal_observation,
        "temporal_revision_snapshot_valid":primary.temporal_snapshot_valid,
        "temporal_revision_score":temporal_score,
        "evidence_enabled_temporal_revision_score":evidence_temporal_score,
        "verbose_diagnostic":verbose.map(|trace| diagnostic_trace_receipt(trace, probe, true)),
        "evidence_enabled_diagnostic":evidence_diagnostic.map(|trace| diagnostic_trace_receipt(trace, probe, false)),
        "stderr_excerpt":primary.stderr_excerpt,
        "score":score,
        "normalization":"memory-decisions-public-read/v1"
    })
}

fn diagnostic_trace_receipt(trace: &VariantTrace, probe: &DecisionProbe, verbose: bool) -> Value {
    json!({
        "claim_rollout_stage":trace.claim_rollout_stage,
        "stage_timings_us":trace.stage_timings_us,
        "launch_profile":trace.launch_profile,
        "request":assemble_arguments(probe, verbose, None),
        "ingest_receipts":trace.ingest_receipts,
        "extract_receipts":trace.extract_receipts,
        "responses":trace.assemble_responses,
        "observation":trace.observations.first(),
        "explain_followup":trace.explain_response,
        "explain_error":trace.explain_error,
        "episode_bindings":trace.bindings,
        "revision_checkpoint_receipts":trace.revision_checkpoint_receipts,
        "temporal_revision_request":trace.temporal_request,
        "temporal_revision_response":trace.temporal_response,
        "temporal_revision_observation":trace.temporal_observation,
        "temporal_revision_snapshot_valid":trace.temporal_snapshot_valid
    })
}

fn failure_receipt(
    probe: &DecisionProbe,
    oracle: Option<&DecisionOracle>,
    product_revision: &str,
    binary_sha256: &str,
    reason: &str,
    stage: &str,
    configuration: &str,
) -> Value {
    json!({
        "schema_version":"memory-decisions-receipt/v4",
        "case_id":probe.id,
        "product_revision":product_revision,
        "product_binary_sha256":binary_sha256,
        "provider_configuration":configuration,
        "desired_goal":{"valid_at":probe.valid_at,"knowledge_before_event_id":probe.knowledge_before_event_id,"oracle":oracle},
        "failure":{"stage":stage,"reason":reason},
        "score":null
    })
}

struct ReplayCaseInput<'a> {
    case_id: &'a str,
    observed: Option<&'a Observed>,
    score_source: Option<ScoringObservationSource>,
    score: Option<&'a ProbeScore>,
    temporal_observation: Option<&'a Observed>,
    temporal_score: Option<&'a ProbeScore>,
    evidence_enabled_temporal_observation: Option<&'a Observed>,
    evidence_enabled_temporal_score: Option<&'a ProbeScore>,
    receipt_path: &'a Path,
    receipt_hash: &'a str,
}

fn replay_case(input: ReplayCaseInput<'_>) -> ReplayCase {
    let ReplayCaseInput {
        case_id,
        observed,
        score_source,
        score,
        temporal_observation,
        temporal_score,
        evidence_enabled_temporal_observation,
        evidence_enabled_temporal_score,
        receipt_path,
        receipt_hash,
    } = input;
    let (source_order, roles, disposition) = observed.map_or_else(
        || (Vec::new(), Vec::new(), "unobserved".into()),
        |observed| {
            (
                observed
                    .sources
                    .iter()
                    .map(|source| {
                        source
                            .event_id
                            .clone()
                            .unwrap_or_else(|| "<unbound>".into())
                    })
                    .collect(),
                observed
                    .sources
                    .iter()
                    .map(|source| source.role.as_str().to_string())
                    .collect(),
                format!("{:?}", observed.disposition).to_lowercase(),
            )
        },
    );
    ReplayCase {
        case_id: case_id.into(),
        source_order,
        roles,
        disposition,
        evidence_success: score.map(|score| score.evidence_success),
        decision_success: score.map(|score| score.decision_success),
        probe_success: score.map(|score| score.probe_success),
        score_observation_source: score_source.map(|source| source.as_str().into()),
        score_observation: observed.map(replay_observation),
        temporal_observation: temporal_observation.map(replay_observation),
        temporal_score: temporal_score.map(replay_score),
        evidence_enabled_temporal_observation: evidence_enabled_temporal_observation
            .map(replay_observation),
        evidence_enabled_temporal_score: evidence_enabled_temporal_score.map(replay_score),
        reason_codes: score
            .map(|score| {
                score
                    .reason_codes
                    .iter()
                    .map(|reason| reason.as_str().to_string())
                    .collect()
            })
            .unwrap_or_default(),
        receipt_path: receipt_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        receipt_sha256: receipt_hash.into(),
    }
}

fn replay_observation(observed: &Observed) -> Value {
    json!({
        "sources": observed.sources.iter().map(|source| json!({
            "event_id": source.event_id,
            "text": source.text,
            "role": source.role.as_str(),
        })).collect::<Vec<_>>(),
        "disposition": format!("{:?}", observed.disposition).to_lowercase(),
        "item_count": observed.item_count,
        "budget_used": observed.budget_used,
        "relation_outcomes": observed.relation_outcomes,
        "guidance": observed.guidance,
    })
}

fn replay_score(score: &ProbeScore) -> Value {
    json!({
        "evidence_success": score.evidence_success,
        "decision_success": score.decision_success,
        "probe_success": score.probe_success,
        "reason_codes": score.reason_codes.iter().map(ScoreReason::as_str).collect::<Vec<_>>(),
    })
}

fn make_outcome(
    case_id: &str,
    status: CaseStatus,
    score: Option<&ProbeScore>,
    temporal_score: Option<&ProbeScore>,
    evidence_stage_temporal_score: Option<&ProbeScore>,
    failure: Option<String>,
    duration_ms: u64,
) -> EvalCaseOutcome {
    let mut outcome = EvalCaseOutcome::new(
        "memory-decisions",
        case_id,
        EvalMode::EndToEnd,
        CorpusSplit::Development,
        LabelTrust::Reviewed,
        status,
    );
    outcome.duration_ms = duration_ms;
    if let Some(score) = score {
        outcome.evidence.insert(
            "evidence_success".into(),
            MetricEvidence::ratio(u64::from(score.evidence_success), 1),
        );
        outcome.evidence.insert(
            "decision_success".into(),
            MetricEvidence::ratio(u64::from(score.decision_success), 1),
        );
        outcome.evidence.insert(
            "probe_success".into(),
            MetricEvidence::ratio(u64::from(score.probe_success), 1),
        );
        outcome.failures.extend(
            score
                .reason_codes
                .iter()
                .map(|reason| format!("{reason:?}").to_lowercase()),
        );
    } else if status == CaseStatus::QualityFailed {
        outcome
            .evidence
            .insert("evidence_success".into(), MetricEvidence::ratio(0, 1));
        outcome
            .evidence
            .insert("decision_success".into(), MetricEvidence::ratio(0, 1));
        outcome
            .evidence
            .insert("probe_success".into(), MetricEvidence::ratio(0, 1));
    }
    for (key, value) in [
        (
            "temporal_evidence_success",
            temporal_score.map(|score| score.evidence_success),
        ),
        (
            "temporal_decision_success",
            temporal_score.map(|score| score.decision_success),
        ),
        (
            "temporal_probe_success",
            temporal_score.map(|score| score.probe_success),
        ),
        (
            "evidence_stage_temporal_evidence_success",
            evidence_stage_temporal_score.map(|score| score.evidence_success),
        ),
        (
            "evidence_stage_temporal_decision_success",
            evidence_stage_temporal_score.map(|score| score.decision_success),
        ),
        (
            "evidence_stage_temporal_probe_success",
            evidence_stage_temporal_score.map(|score| score.probe_success),
        ),
    ] {
        if let Some(value) = value {
            outcome
                .evidence
                .insert(key.into(), MetricEvidence::ratio(u64::from(value), 1));
        }
    }
    if let Some(failure) = failure {
        outcome.failures.push(failure.clone());
        if status == CaseStatus::Invalid {
            outcome.invalid_reason = Some(failure);
        }
    }
    outcome
}

fn invalid_outcome(case_id: &str, reason: String, duration_ms: u64) -> EvalCaseOutcome {
    make_outcome(
        case_id,
        CaseStatus::Invalid,
        None,
        None,
        None,
        Some(reason),
        duration_ms,
    )
}

pub(super) fn deterministic_configuration_for(claim_stage: &str) -> String {
    let primary = if claim_stage == "default" {
        "product_default"
    } else {
        claim_stage
    };
    format!(
        "storage=embedded-surreal-local;namespace=main;embeddings=disabled;retrieval=lexical-fallback;ner=regex-builtin;primary_claim_rollout={primary};evidence_diagnostic_claim_rollout=evidence;transaction_time=server-assigned;scoring_protocol=temporal-cutoff-v2"
    )
}

pub(super) fn registered_configuration(config_sha256: &str) -> Option<(&'static str, String)> {
    [
        ("shadow", deterministic_configuration_for("shadow")),
        ("default", deterministic_configuration_for("default")),
        ("shadow", legacy_deterministic_configuration_for("shadow")),
        ("default", legacy_deterministic_configuration_for("default")),
    ]
    .into_iter()
    .find_map(|(stage, configuration)| {
        (sha256(configuration.as_bytes()) == config_sha256).then_some((stage, configuration))
    })
}

fn legacy_deterministic_configuration_for(claim_stage: &str) -> String {
    let primary = if claim_stage == "default" {
        "product_default"
    } else {
        claim_stage
    };
    format!(
        "storage=embedded-surreal-local;namespace=main;embeddings=disabled;retrieval=lexical-fallback;ner=regex-builtin;primary_claim_rollout={primary};evidence_diagnostic_claim_rollout=evidence;transaction_time=server-assigned"
    )
}

fn fallback_artifact_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "memory-decisions-invalid-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ))
}

/// Scoped ownership also cleans failed formation/startup paths after the session drops.
struct ScenarioStore(PathBuf);

impl Drop for ScenarioStore {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fresh_store(case_id: &str, variant: &str) -> PathBuf {
    static NEXT_STORE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "memory-decisions-{}-{case_id}-{variant}-{}",
        std::process::id(),
        NEXT_STORE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

#[cfg(test)]
mod reducer_tests {
    use super::MemoryDecisionsReducer;
    use crate::domain::*;
    use crate::reducer::SuiteReducer;

    fn outcome(status: CaseStatus) -> EvalCaseOutcome {
        let mut outcome = EvalCaseOutcome::new(
            "memory-decisions",
            "case-1",
            EvalMode::EndToEnd,
            CorpusSplit::Development,
            LabelTrust::Reviewed,
            status,
        );
        if status != CaseStatus::Invalid {
            for key in ["evidence_success", "decision_success", "probe_success"] {
                outcome
                    .evidence
                    .insert(key.into(), MetricEvidence::ratio(1, 1));
            }
        }
        outcome
    }

    #[test]
    fn reducer_rejects_a_missing_score_on_a_valid_case() {
        let mut case = outcome(CaseStatus::Passed);
        case.evidence.remove("probe_success");

        let result = MemoryDecisionsReducer::new("memory-decisions").reduce(&[case]);

        assert!(matches!(result, Err(crate::EvalError::InvalidInput(_))));
    }

    #[test]
    fn reducer_does_not_invent_scores_for_invalid_cases() {
        let summaries = MemoryDecisionsReducer::new("memory-decisions")
            .reduce(&[outcome(CaseStatus::Invalid)])
            .unwrap();

        assert_eq!(summaries[0].invalid, 1);
        assert_eq!(summaries[0].metrics["probe_success_rate"], 0.0);
    }

    #[test]
    fn reducer_sums_ratio_evidence_for_quality_failures() {
        let mut passed = outcome(CaseStatus::Passed);
        let mut failed = outcome(CaseStatus::QualityFailed);
        for key in ["evidence_success", "decision_success", "probe_success"] {
            failed
                .evidence
                .insert(key.into(), MetricEvidence::ratio(0, 1));
        }

        let summaries = MemoryDecisionsReducer::new("memory-decisions")
            .reduce(&[passed.clone(), failed])
            .unwrap();

        assert_eq!(summaries[0].passed, 1);
        assert_eq!(summaries[0].quality_failed, 1);
        assert_eq!(summaries[0].metrics["probe_success_rate"], 0.5);
        passed.evidence.clear();
    }
}

#[cfg(test)]
mod merge_compatibility_tests {
    use crate::artifact::{EVAL_ARTIFACT_SCHEMA_V1, GateStatus, RunArtifact, RunFingerprint};
    use crate::domain::*;
    use crate::merge::merge_shards;
    use crate::profile::ProfileManifest;
    use crate::reducer::SuiteReducer;

    use super::MemoryDecisionsReducer;

    fn decision_outcome(case_id: &str, passed: bool) -> EvalCaseOutcome {
        let status = if passed {
            CaseStatus::Passed
        } else {
            CaseStatus::QualityFailed
        };
        let mut outcome = EvalCaseOutcome::new(
            "memory-decisions",
            case_id,
            EvalMode::EndToEnd,
            CorpusSplit::Development,
            LabelTrust::Reviewed,
            status,
        );
        for key in ["evidence_success", "decision_success", "probe_success"] {
            outcome
                .evidence
                .insert(key.into(), MetricEvidence::ratio(u64::from(passed), 1));
        }
        outcome
    }

    fn shard(outcome: EvalCaseOutcome, shard_id: &str) -> RunArtifact {
        RunArtifact {
            schema_version: EVAL_ARTIFACT_SCHEMA_V1.into(),
            run_id: shard_id.into(),
            profile: EvalProfile::Nightly,
            started_at: chrono::Utc::now(),
            duration_ms: 100,
            expected_case_ids: vec![outcome.case_id().clone()],
            expected_cases: vec![],
            outcomes: vec![outcome],
            suite_summaries: vec![],
            gates: vec![],
            fingerprint: RunFingerprint::default_for_test(),
            budget_status: Some(GateStatus::Passed),
            verdict: RunVerdict::default(),
            issues: vec![],
        }
    }

    #[test]
    fn merged_shards_match_full_run_memory_decision_reduction() {
        let passed = decision_outcome("unknown", true);
        let failed = decision_outcome("late-after", false);
        let full_run = MemoryDecisionsReducer::new("memory-decisions")
            .reduce(&[passed.clone(), failed.clone()])
            .unwrap();
        let manifest = ProfileManifest {
            schema_version: "memory-mcp-eval-profile/v1".into(),
            profile: EvalProfile::Nightly,
            time_budget_seconds: 600,
            suites: vec![],
            gates: vec![],
        };

        let merged = merge_shards(
            &[
                shard(passed, "passing-shard"),
                shard(failed, "failing-shard"),
            ],
            &manifest,
        )
        .unwrap();

        assert_eq!(merged.suite_summaries, full_run);
        assert_eq!(merged.verdict, RunVerdict::QualityFailed);
    }
}

#[cfg(test)]
mod diagnostic_helper_tests {
    use std::collections::BTreeMap;

    use serde_json::{Value, json};

    use super::{
        DecisionProbe, VariantTrace, diagnostic_trace_receipt, explain_arguments,
        supports_separate_time_cutoffs, supports_temporal_revision, temporal_revision_for_probe,
    };

    #[test]
    fn explain_receives_retrieved_context_as_the_documented_json_string() {
        let context = vec![json!({"fact_id":"fact:a","source_episode":"episode:a"})];

        let arguments = explain_arguments(&context).unwrap();

        assert_eq!(arguments["compact"], true);
        assert!(arguments.get("context_pack").is_none());
        assert_eq!(
            serde_json::from_str::<Value>(arguments["context_items"].as_str().unwrap()).unwrap(),
            json!([{"fact_id":"fact:a","source_episode":"episode:a"}])
        );
    }

    #[test]
    fn one_as_of_property_does_not_claim_independent_temporal_cutoffs() {
        let schemas = json!([{
            "name":"assemble_context",
            "inputSchema":{"properties":{"as_of":{"type":"string"}}}
        }]);

        assert!(!supports_separate_time_cutoffs(&schemas));
    }

    #[test]
    fn valid_and_known_time_properties_establish_independent_temporal_cutoffs() {
        let schemas = json!([{
            "name":"assemble_context",
            "inputSchema":{"properties":{"valid_at":{"type":"string"},"known_at":{"type":"string"}}}
        }]);

        assert!(supports_separate_time_cutoffs(&schemas));
    }

    #[test]
    fn exact_revision_support_requires_both_valid_time_and_revision_fields() {
        let schemas = json!([{
            "name":"assemble_context",
            "inputSchema":{"properties":{"valid_at":{"type":"string"},"known_revision":{"type":"integer"}}}
        }]);

        assert!(supports_temporal_revision(&schemas));
    }

    #[test]
    fn temporal_probe_uses_checkpoint_immediately_before_its_knowledge_boundary() {
        let probe: DecisionProbe = serde_json::from_value(json!({
            "id":"retrospective",
            "event_ids":["initial","correction","peer"],
            "knowledge_before_event_id":"correction",
            "query":"Where was it?",
            "valid_at":"2026-05-01T00:00:00Z",
            "budget":5
        }))
        .unwrap();
        let checkpoints = BTreeMap::from([
            ("initial".into(), 4),
            ("correction".into(), 9),
            ("peer".into(), 12),
        ]);

        assert_eq!(
            temporal_revision_for_probe(&probe, &checkpoints)
                .unwrap_or_else(|failure| panic!("{}", failure.message)),
            4
        );
    }

    #[test]
    fn evidence_diagnostic_keeps_its_ingest_and_extract_receipts() {
        let probe: DecisionProbe = serde_json::from_value(json!({
            "id":"correction",
            "event_ids":["initial","correction"],
            "query":"Where was it?",
            "valid_at":"2026-05-01T00:00:00Z",
            "budget":5
        }))
        .unwrap();
        let trace = VariantTrace {
            stage_timings_us: BTreeMap::new(),
            claim_rollout_stage: "evidence".into(),
            launch_profile: BTreeMap::new(),
            tool_schemas: json!([]),
            initialize_response: json!({}),
            ingest_receipts: vec![json!({"event_id":"correction"})],
            extract_receipts: vec![json!({"event_id":"correction","warning":"contradiction"})],
            bindings: BTreeMap::from([("episode:correction".into(), "correction".into())]),
            assemble_responses: vec![],
            observations: vec![],
            explain_response: None,
            explain_error: None,
            temporal_diagnostic_request: None,
            temporal_diagnostic_response: None,
            revision_checkpoint_receipts: vec![],
            temporal_request: None,
            temporal_response: None,
            temporal_observation: None,
            temporal_snapshot_valid: false,
            temporal_read_measurement: None,
            assemble_read_measurements: vec![],
            stderr_excerpt: String::new(),
        };

        let receipt = diagnostic_trace_receipt(&trace, &probe, false);

        assert_eq!(receipt["ingest_receipts"][0]["event_id"], "correction");
        assert_eq!(receipt["extract_receipts"][0]["warning"], "contradiction");
        assert_eq!(
            receipt["episode_bindings"]["episode:correction"],
            "correction"
        );
    }
}
