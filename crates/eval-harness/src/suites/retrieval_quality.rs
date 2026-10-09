pub(crate) mod cases;
pub(crate) mod scoring;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use sha2::Digest;

use crate::artifact::SuiteSummary;
use crate::domain::{CaseStatus, EvalCaseId, EvalCaseOutcome, EvalMode, MetricEvidence, SuiteId};
use crate::error::EvalError;
use crate::metrics::{CaseMetricNames, render_case_metrics};
use crate::profile::SuiteDecl;
use crate::reducer::SuiteReducer;
use crate::runner::{EvalSuite, RunContext};
use cases::{
    NativePreparationManifest, QualityCase, QualityCorpusManifest, QualityExpectation,
    QualityFactBinding, QualityObservation, RuntimeRetrievalIdentity, SourceBundleEntry,
};
use scoring::score_quality;

pub(crate) struct RetrievalQualityReducer {
    suite_id: SuiteId,
}

impl RetrievalQualityReducer {
    pub(crate) fn new() -> Self {
        Self {
            suite_id: SuiteId::parse("retrieval-quality")
                .expect("retrieval quality suite id is a valid constant"),
        }
    }
}

impl SuiteReducer for RetrievalQualityReducer {
    fn suite_id(&self) -> &SuiteId {
        &self.suite_id
    }

    fn reduce(&self, outcomes: &[EvalCaseOutcome]) -> Result<Vec<SuiteSummary>, EvalError> {
        const METRIC_NAMES: [&str; 6] = [
            "precision_macro",
            "required_group_recall",
            "unknown_abstention_rate",
            "answerable_non_abstention_rate",
            "duplicate_free_rate",
            "prohibited_free_rate",
        ];

        let mut values: BTreeMap<&str, Vec<(u64, u64)>> = BTreeMap::new();
        for outcome in outcomes
            .iter()
            .filter(|outcome| outcome.status != crate::domain::CaseStatus::Invalid)
        {
            for (name, evidence) in &outcome.evidence {
                if !METRIC_NAMES.contains(&name.as_str()) {
                    continue;
                }
                let MetricEvidence::Ratio {
                    numerator,
                    denominator,
                } = evidence
                else {
                    return Err(EvalError::InvalidConfig(format!(
                        "retrieval quality metric `{name}` must use Ratio evidence"
                    )));
                };
                if *denominator == 0 || numerator > denominator {
                    return Err(EvalError::InvalidConfig(format!(
                        "retrieval quality metric `{name}` has invalid ratio evidence"
                    )));
                }
                values
                    .entry(name)
                    .or_default()
                    .push((*numerator, *denominator));
            }
        }

        let mut metrics = BTreeMap::new();
        for name in METRIC_NAMES {
            let Some(samples) = values.get(name) else {
                continue;
            };
            if samples.is_empty() {
                continue;
            }

            let value = if name == "precision_macro" {
                samples
                    .iter()
                    .map(|(numerator, denominator)| *numerator as f64 / *denominator as f64)
                    .sum::<f64>()
                    / samples.len() as f64
            } else if name == "required_group_recall" {
                let numerator = samples.iter().map(|(numerator, _)| *numerator).sum::<u64>();
                let denominator = samples
                    .iter()
                    .map(|(_, denominator)| *denominator)
                    .sum::<u64>();
                numerator as f64 / denominator as f64
            } else {
                let numerator = samples.iter().map(|(numerator, _)| *numerator).sum::<u64>();
                let denominator = samples
                    .iter()
                    .map(|(_, denominator)| *denominator)
                    .sum::<u64>();
                numerator as f64 / denominator as f64
            };
            let applicable_count = if name == "required_group_recall" {
                samples
                    .iter()
                    .map(|(_, denominator)| *denominator)
                    .sum::<u64>()
            } else {
                u64::try_from(samples.len()).unwrap_or(u64::MAX)
            };
            metrics.insert(name.into(), value);
            metrics.insert(format!("{name}_applicable_count"), applicable_count as f64);
        }

        let total = outcomes.len();
        let passed = outcomes
            .iter()
            .filter(|outcome| outcome.status == crate::domain::CaseStatus::Passed)
            .count();
        let quality_failed = outcomes
            .iter()
            .filter(|outcome| outcome.status == crate::domain::CaseStatus::QualityFailed)
            .count();
        let invalid = outcomes
            .iter()
            .filter(|outcome| outcome.status == crate::domain::CaseStatus::Invalid)
            .count();

        Ok(vec![SuiteSummary {
            suite_id: self.suite_id.as_str().to_string(),
            mode: outcomes
                .first()
                .map_or(crate::domain::EvalMode::RetrievalOnly, |outcome| {
                    outcome.mode
                }),
            total,
            passed,
            quality_failed,
            invalid,
            metrics,
        }])
    }
}

pub struct RetrievalQualitySuite {
    cases: Vec<QualityCase>,
    native_preparation: NativePreparationManifest,
    expected_ids: Vec<EvalCaseId>,
    reducer: RetrievalQualityReducer,
    fingerprint: Mutex<BTreeMap<String, String>>,
}

impl RetrievalQualitySuite {
    pub fn from_decl(decl: &SuiteDecl) -> Result<Self, EvalError> {
        let root = decl.corpus_root.as_deref().ok_or_else(|| {
            EvalError::InvalidConfig("retrieval-quality requires corpus_root".into())
        })?;
        let root = PathBuf::from(root);
        let manifest_path = root.join("retrieval-quality.json");
        let raw = std::fs::read_to_string(&manifest_path).map_err(|source| EvalError::Io {
            path: manifest_path.clone(),
            source,
        })?;
        let manifest = QualityCorpusManifest::parse(&raw).map_err(EvalError::InvalidConfig)?;
        verify_source_bundle(&root, &manifest)?;

        let mut fingerprint = BTreeMap::new();
        fingerprint.insert("corpus_id".into(), manifest.corpus_id.clone());
        fingerprint.insert("corpus_revision".into(), manifest.revision.clone());
        fingerprint.insert(
            "corpus_manifest_sha256".into(),
            hex::encode(sha2::Sha256::digest(raw.as_bytes())),
        );
        fingerprint.insert(
            "source_bundle_sha256".into(),
            manifest.source_sha256.clone(),
        );
        fingerprint.insert(
            "prepared_provider".into(),
            manifest.native_preparation.provider_label.clone(),
        );
        fingerprint.insert(
            "prepared_model".into(),
            manifest
                .native_preparation
                .model
                .clone()
                .unwrap_or_default(),
        );
        fingerprint.insert(
            "prepared_embedding_signature".into(),
            manifest.native_preparation.embedding_signature.clone(),
        );
        fingerprint.insert(
            "prepared_embedding_dimension".into(),
            manifest.native_preparation.embedding_dimension.to_string(),
        );
        fingerprint.insert(
            "prepared_similarity_threshold".into(),
            manifest.native_preparation.similarity_threshold.to_string(),
        );
        fingerprint.insert(
            "prepared_index_signature".into(),
            manifest.native_preparation.index_signature.clone(),
        );
        fingerprint.insert(
            "prepared_extractor_fingerprint".into(),
            manifest.native_preparation.extractor_fingerprint.clone(),
        );
        fingerprint.insert(
            "prepared_claim_rollout_stage".into(),
            manifest.native_preparation.claim_rollout_stage.clone(),
        );
        fingerprint.insert("provider_status".into(), "not_initialized".into());

        let expected_ids = manifest
            .cases
            .iter()
            .map(|case| EvalCaseId::parse(case.id.clone()))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            cases: manifest.cases,
            native_preparation: manifest.native_preparation,
            expected_ids,
            reducer: RetrievalQualityReducer::new(),
            fingerprint: Mutex::new(fingerprint),
        })
    }

    fn update_fingerprint(&self, values: impl IntoIterator<Item = (String, String)>) {
        let mut fingerprint = self
            .fingerprint
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        fingerprint.extend(values);
    }

    fn invalid_outcomes(&self, reason: &str) -> Vec<EvalCaseOutcome> {
        self.cases
            .iter()
            .map(|case| invalid_outcome(case, reason))
            .collect()
    }
}

#[derive(Clone)]
struct RetrievalRuntime {
    provider: Arc<dyn memory_mcp::embedding::providers::EmbeddingProvider>,
    provider_label: String,
    model: Option<String>,
    dimension: usize,
    signature: String,
    similarity_threshold: f64,
    index_signature: String,
    extractor_fingerprint: String,
    claim_stage: String,
}

impl RetrievalRuntime {
    async fn from_environment() -> Result<Self, &'static str> {
        let embedding_config = memory_mcp::config::EmbeddingConfig::from_env()
            .map_err(|_| "embedding provider configuration is invalid")?;
        let surreal_config = memory_mcp::config::SurrealConfig::from_env()
            .map_err(|_| "provider data-directory configuration is invalid")?;
        let data_dir = surreal_config.data_dir_or_default();
        let resolved = memory_mcp::eval_support::retrieval::create_eval_embedding_provider(
            &embedding_config,
            &data_dir,
        )
        .await
        .map_err(|_| "a real embedding provider could not be initialized")?;
        let claim_stage =
            std::env::var("MEMORY_CLAIM_ROLLOUT_STAGE").unwrap_or_else(|_| "disabled".into());

        Ok(Self {
            provider: resolved.provider,
            provider_label: resolved.provider_label,
            model: resolved.model,
            dimension: resolved.dimension,
            signature: resolved.signature,
            similarity_threshold: resolved.similarity_threshold,
            index_signature: resolved.index_signature,
            extractor_fingerprint: resolved.extractor_fingerprint,
            claim_stage,
        })
    }

    fn fingerprint(&self) -> BTreeMap<String, String> {
        let mut values = BTreeMap::new();
        values.insert("provider_status".into(), "ready".into());
        values.insert("provider".into(), self.provider_label.clone());
        if let Some(model) = &self.model {
            values.insert("model".into(), model.clone());
        }
        values.insert("embedding_dimension".into(), self.dimension.to_string());
        values.insert("embedding_signature".into(), self.signature.clone());
        values.insert(
            "similarity_threshold".into(),
            self.similarity_threshold.to_string(),
        );
        values.insert("index_signature".into(), self.index_signature.clone());
        values.insert(
            "extractor_fingerprint".into(),
            self.extractor_fingerprint.clone(),
        );
        values.insert("claim_rollout_stage".into(), self.claim_stage.clone());
        values
    }
}

fn verify_source_bundle(
    corpus_root: &Path,
    manifest: &QualityCorpusManifest,
) -> Result<(), EvalError> {
    let root = std::fs::canonicalize(corpus_root).map_err(|source| EvalError::Io {
        path: corpus_root.to_path_buf(),
        source,
    })?;
    let bundle_path = corpus_root.join(&manifest.source_bundle_path);
    let canonical_bundle = std::fs::canonicalize(&bundle_path).map_err(|source| EvalError::Io {
        path: bundle_path.clone(),
        source,
    })?;
    if !canonical_bundle.starts_with(&root) {
        return Err(EvalError::InvalidConfig(
            "source bundle resolves outside corpus_root".into(),
        ));
    }
    let bytes = std::fs::read(&canonical_bundle).map_err(|source| EvalError::Io {
        path: canonical_bundle,
        source,
    })?;
    let digest = hex::encode(sha2::Sha256::digest(&bytes));
    if digest != manifest.source_sha256.to_ascii_lowercase() {
        return Err(EvalError::InvalidConfig(
            "source bundle SHA-256 does not match the frozen corpus manifest".into(),
        ));
    }
    let bundle: Vec<SourceBundleEntry> = serde_json::from_slice(&bytes).map_err(|_| {
        EvalError::InvalidConfig("frozen source bundle is not a valid source episode array".into())
    })?;
    let expected = manifest
        .cases
        .iter()
        .flat_map(|case| {
            case.sources
                .iter()
                .cloned()
                .map(|source| SourceBundleEntry {
                    case_id: case.id.clone(),
                    source,
                })
        })
        .collect::<Vec<_>>();
    if bundle != expected {
        return Err(EvalError::InvalidConfig(
            "frozen source bundle episodes do not exactly match the case inputs".into(),
        ));
    }
    Ok(())
}

#[async_trait]
impl EvalSuite for RetrievalQualitySuite {
    fn id(&self) -> &str {
        "retrieval-quality"
    }

    fn mode(&self) -> EvalMode {
        EvalMode::RetrievalOnly
    }

    fn expected_case_ids(&self) -> &[EvalCaseId] {
        &self.expected_ids
    }

    fn reducer(&self) -> &dyn SuiteReducer {
        &self.reducer
    }

    fn fingerprint_metadata(&self) -> BTreeMap<String, String> {
        self.fingerprint
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    async fn run(&self, _context: &RunContext) -> Vec<EvalCaseOutcome> {
        let runtime = match RetrievalRuntime::from_environment().await {
            Ok(runtime) => runtime,
            Err(reason) => {
                self.update_fingerprint([("provider_status".into(), "unavailable".into())]);
                return self.invalid_outcomes(reason);
            }
        };
        self.update_fingerprint(runtime.fingerprint());
        let runtime_identity = RuntimeRetrievalIdentity {
            provider_label: &runtime.provider_label,
            model: runtime.model.as_deref(),
            embedding_signature: &runtime.signature,
            embedding_dimension: runtime.dimension,
            similarity_threshold: runtime.similarity_threshold,
            index_signature: &runtime.index_signature,
            extractor_fingerprint: &runtime.extractor_fingerprint,
            claim_rollout_stage: &runtime.claim_stage,
        };
        if let Err(reason) = self.native_preparation.validate_runtime(&runtime_identity) {
            self.update_fingerprint([("provider_status".into(), "incompatible".into())]);
            return self.invalid_outcomes(reason);
        }

        let mut outcomes = Vec::with_capacity(self.cases.len());
        for (index, case) in self.cases.iter().enumerate() {
            outcomes.push(run_native_case(case, &runtime, index).await);
        }
        outcomes
    }
}

async fn run_native_case(
    case: &QualityCase,
    runtime: &RetrievalRuntime,
    case_index: usize,
) -> EvalCaseOutcome {
    let started = Instant::now();
    let result = run_native_case_inner(case, runtime, case_index).await;
    let mut outcome = match result {
        Ok((scores, evidence)) => {
            let failures = quality_failures(case, &scores);
            let status = if failures.is_empty() {
                CaseStatus::Passed
            } else {
                CaseStatus::QualityFailed
            };
            let mut outcome = EvalCaseOutcome::new(
                "retrieval-quality",
                case.id.clone(),
                EvalMode::RetrievalOnly,
                case.split,
                case.label_trust,
                status,
            );
            outcome.failures = failures;
            for (name, metric_evidence) in evidence {
                outcome.metrics.extend(render_case_metrics(
                    &metric_evidence,
                    &CaseMetricNames::named(name.clone()),
                ));
                outcome.evidence.insert(name, metric_evidence);
            }
            attach_case_tags(&mut outcome, &case.tags);
            outcome
        }
        Err(reason) => invalid_outcome(case, reason),
    };
    outcome.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    outcome
}

async fn run_native_case_inner(
    case: &QualityCase,
    runtime: &RetrievalRuntime,
    case_index: usize,
) -> Result<(scoring::QualityScores, BTreeMap<String, MetricEvidence>), &'static str> {
    use memory_mcp::knowledge::api::{KnowledgeReadPort, KnowledgeReadScope};
    use memory_mcp::knowledge::infra::KnowledgeReadAdapter;
    use memory_mcp::models::{AssembleContextRequest, IngestRequest};
    use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability as ServiceAssembleContextCapability;
    use memory_mcp::service::memory_container_shims::memory_capabilities_extract::ExtractCapability as ServiceExtractCapability;
    use memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability as ServiceIngestCapability;
    use memory_mcp::storage::{DbClient, SurrealDbClient};

    let database = format!("retrieval_quality_{case_index}");
    let db: Arc<dyn DbClient> = Arc::new(
        SurrealDbClient::connect_in_memory_with_dimension(
            &database,
            "main",
            "error",
            runtime.dimension,
        )
        .await
        .map_err(|_| "isolated evaluation storage could not be initialized")?,
    );
    db.apply_migrations("main")
        .await
        .map_err(|_| "isolated evaluation schema could not be initialized")?;

    let service = memory_mcp::eval_support::retrieval::new_eval_retrieval_service(
        db.clone(),
        "main".into(),
        memory_mcp::eval_support::retrieval::EvalRetrievalConfig {
            embedding_provider: runtime.provider.clone(),
            semantic_threshold: runtime.similarity_threshold,
            claim_stage: runtime.claim_stage.clone(),
        },
    )
    .await
    .map_err(|_| "native retrieval service configuration is invalid")?;

    let mut episode_ids = BTreeMap::new();
    let mut extracted_fact_ids = BTreeMap::<String, BTreeSet<String>>::new();
    for source in &case.sources {
        let mut request = IngestRequest::builder()
            .source_type(source.source_type.clone())
            .source_id(source.source_id.clone())
            .content(source.content.clone())
            .t_ref(source.t_ref)
            .policy_tags(source.policy_tags.clone());
        if let Some(t_ingested) = source.t_ingested {
            request = request.t_ingested(t_ingested);
        }
        let request = request
            .build()
            .map_err(|_| "a frozen source episode does not satisfy the ingest contract")?;
        let episode_id = ServiceIngestCapability::ingest_from_service(&service, request, None)
            .await
            .map_err(|_| "native episode ingestion failed")?;
        let extracted =
            ServiceExtractCapability::extract_from_service(&service, &episode_id, None, None)
                .await
                .map_err(|_| "native extraction failed")?;
        episode_ids.insert(source.key.clone(), episode_id);
        extracted_fact_ids.insert(
            source.key.clone(),
            extracted
                .facts
                .into_iter()
                .map(|fact| fact.fact_id)
                .collect(),
        );
    }

    let readback = KnowledgeReadAdapter::new(db, "main");
    let fact_rows = readback
        .read_scope(KnowledgeReadScope::Facts)
        .await
        .map_err(|_| "persisted fact readback failed")?;
    let facts = fact_rows
        .iter()
        .map(memory_mcp::knowledge::fact_parsing::fact_from_value_or_wrapper)
        .collect::<Option<Vec<_>>>()
        .ok_or("persisted fact readback contained an invalid fact record")?;

    let mut actual_id_by_gold_key = BTreeMap::new();
    for binding in &case.bindings {
        resolve_binding(
            binding,
            &episode_ids,
            &extracted_fact_ids,
            &facts,
            &mut actual_id_by_gold_key,
        )?;
    }

    let items = ServiceAssembleContextCapability::assemble_context_from_service(
        &service,
        AssembleContextRequest {
            query: case.query.clone(),
            fact_types: case.fact_types.clone(),
            as_of: Some(case.as_of),
            budget: i32::try_from(case.budget)
                .map_err(|_| "case budget is outside the supported range")?,
            view_mode: case.view_mode.clone(),
            window_start: case.window_start,
            window_end: case.window_end,
            access: case.access.clone(),
            compact: case.compact,
        },
    )
    .await
    .map_err(|_| "native context assembly failed")?;

    let gold_key_by_actual_id = actual_id_by_gold_key
        .into_iter()
        .map(|(gold_key, actual_id)| (actual_id, gold_key))
        .collect::<BTreeMap<_, _>>();
    let ranked_keys = items
        .iter()
        .map(|item| {
            gold_key_by_actual_id
                .get(&item.fact_id)
                .cloned()
                .unwrap_or_else(|| format!("__unknown:{}", item.fact_id))
        })
        .collect::<Vec<_>>();
    let expectation = case.expectation();
    let observation = QualityObservation {
        ranked_keys: ranked_keys.clone(),
        budget: case.budget,
    };
    let scores = score_quality(&expectation, &observation)
        .map_err(|_| "quality oracle rejected its own validated case")?;
    let evidence = quality_evidence(case, &scores, &ranked_keys, &expectation);
    Ok((scores, evidence))
}

fn resolve_binding(
    binding: &QualityFactBinding,
    episode_ids: &BTreeMap<String, String>,
    extracted_fact_ids: &BTreeMap<String, BTreeSet<String>>,
    facts: &[memory_mcp::models::Fact],
    actual_id_by_gold_key: &mut BTreeMap<String, String>,
) -> Result<(), &'static str> {
    let episode_id = episode_ids
        .get(&binding.episode_key)
        .ok_or("gold binding references a source that was not ingested")?;
    let candidates = facts
        .iter()
        .filter(|fact| {
            fact.source_episode == *episode_id
                && fact.fact_type == binding.fact_type
                && fact.content == binding.content
        })
        .collect::<Vec<_>>();
    if candidates.len() != 1 {
        return Err("gold binding did not resolve to exactly one persisted extracted fact");
    }
    let fact = candidates[0];
    if !extracted_fact_ids
        .get(&binding.episode_key)
        .is_some_and(|ids| ids.contains(&fact.fact_id))
    {
        return Err("gold binding resolved to a fact absent from native extraction output");
    }
    if actual_id_by_gold_key
        .insert(binding.key.clone(), fact.fact_id.clone())
        .is_some()
    {
        return Err("multiple gold bindings resolved to the same key");
    }
    Ok(())
}

fn quality_evidence(
    case: &QualityCase,
    scores: &scoring::QualityScores,
    ranked_keys: &[String],
    expectation: &QualityExpectation,
) -> BTreeMap<String, MetricEvidence> {
    let mut evidence = BTreeMap::new();
    if !case.expect_empty {
        let denominator = u64::try_from(scores.unique_items)
            .unwrap_or(u64::MAX)
            .max(1);
        evidence.insert(
            "precision_macro".into(),
            MetricEvidence::ratio(
                u64::try_from(scores.allowed_items).unwrap_or(u64::MAX),
                denominator,
            ),
        );
        if scores.required_groups > 0 {
            evidence.insert(
                "required_group_recall".into(),
                MetricEvidence::ratio(
                    u64::try_from(scores.covered_groups).unwrap_or(u64::MAX),
                    u64::try_from(scores.required_groups).unwrap_or(u64::MAX),
                ),
            );
        }
        evidence.insert(
            "answerable_non_abstention_rate".into(),
            MetricEvidence::ratio(u64::from(!ranked_keys.is_empty()), 1),
        );
    } else {
        evidence.insert(
            "unknown_abstention_rate".into(),
            MetricEvidence::ratio(u64::from(ranked_keys.is_empty()), 1),
        );
    }
    evidence.insert(
        "duplicate_free_rate".into(),
        MetricEvidence::ratio(u64::from(scores.duplicate_items == 0), 1),
    );
    if !expectation.prohibited_keys.is_empty() {
        evidence.insert(
            "prohibited_free_rate".into(),
            MetricEvidence::ratio(u64::from(scores.prohibited_items == 0), 1),
        );
    }
    evidence
}

fn quality_failures(case: &QualityCase, scores: &scoring::QualityScores) -> Vec<String> {
    let mut failures = Vec::new();
    if !case.expect_empty && scores.recall != Some(1.0) {
        failures.push("retrieval missed one or more required evidence groups".into());
    }
    if !case.expect_empty && scores.precision.is_some_and(|precision| precision < 1.0) {
        failures.push("retrieval returned unlabeled or disallowed evidence".into());
    }
    if scores.prohibited_items > 0 {
        failures.push("prohibited evidence was retrieved".into());
    }
    if scores.duplicate_items > 0 {
        failures.push("duplicate evidence was retrieved".into());
    }
    if !scores.budget_ok {
        failures.push("retrieval exceeded the requested budget".into());
    }
    if (case.expect_empty && scores.abstention != Some(true))
        || scores.false_abstention == Some(true)
    {
        failures.push("retrieval did not follow the expected abstention behavior".into());
    }
    if scores.order_ok == Some(false) {
        failures.push("retrieved evidence did not match the labeled order".into());
    }
    failures
}

fn invalid_outcome(case: &QualityCase, reason: &str) -> EvalCaseOutcome {
    let mut outcome = EvalCaseOutcome::new(
        "retrieval-quality",
        case.id.clone(),
        EvalMode::RetrievalOnly,
        case.split,
        case.label_trust,
        CaseStatus::Invalid,
    );
    outcome.invalid_reason = Some(reason.into());
    attach_case_tags(&mut outcome, &case.tags);
    outcome
}

fn attach_case_tags(outcome: &mut EvalCaseOutcome, tags: &[String]) {
    for tag in tags {
        outcome
            .evidence
            .insert(format!("case_tag:{tag}"), MetricEvidence::count(1));
    }
}

#[cfg(test)]
mod reducer_tests {
    use crate::domain::{CaseStatus, CorpusSplit, EvalCaseOutcome, EvalMode, LabelTrust};
    use chrono::{DateTime, Utc};

    use super::*;

    fn outcome(case_id: &str, evidence: Option<MetricEvidence>) -> EvalCaseOutcome {
        let mut outcome = EvalCaseOutcome::new(
            "retrieval-quality",
            case_id,
            EvalMode::RetrievalOnly,
            CorpusSplit::Test,
            LabelTrust::Reviewed,
            CaseStatus::Passed,
        );
        if let Some(evidence) = evidence {
            outcome.evidence.insert("quality".into(), evidence);
        }
        outcome
    }

    fn case(expect_empty: bool) -> QualityCase {
        let timestamp = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .expect("fixed valid timestamp")
            .with_timezone(&Utc);
        QualityCase {
            id: "case".into(),
            split: CorpusSplit::Test,
            label_trust: LabelTrust::Reviewed,
            query: "What is the answer?".into(),
            budget: 5,
            as_of: timestamp,
            tags: vec!["contract".into()],
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: true,
            sources: vec![],
            bindings: vec![],
            required_evidence_groups: vec![],
            expect_empty,
            expected_order: None,
        }
    }

    #[test]
    fn case_tags_are_preserved_in_the_artifact_evidence_even_for_invalid_cases() {
        let invalid = invalid_outcome(&case(false), "provider unavailable");

        assert_eq!(
            invalid.evidence.get("case_tag:contract"),
            Some(&MetricEvidence::count(1))
        );
    }

    #[test]
    fn quality_case_fails_when_a_required_evidence_group_is_missing() {
        let expected = QualityExpectation {
            required_evidence_groups: [BTreeSet::from(["answer".into()])].into(),
            allowed_keys: BTreeSet::from(["answer".into()]),
            prohibited_keys: BTreeSet::new(),
            expect_empty: false,
            expected_order: None,
        };
        let scores = score_quality(
            &expected,
            &QualityObservation {
                ranked_keys: vec![],
                budget: 5,
            },
        )
        .expect("valid quality observation");

        let failures = quality_failures(&case(false), &scores);

        assert!(
            failures
                .iter()
                .any(|failure| failure.contains("required evidence"))
        );
    }

    #[test]
    fn quality_case_fails_when_it_returns_unlabeled_noise() {
        let expected = QualityExpectation {
            required_evidence_groups: [BTreeSet::from(["answer".into()])].into(),
            allowed_keys: BTreeSet::from(["answer".into()]),
            prohibited_keys: BTreeSet::new(),
            expect_empty: false,
            expected_order: None,
        };
        let scores = score_quality(
            &expected,
            &QualityObservation {
                ranked_keys: vec!["answer".into(), "__unknown:noise".into()],
                budget: 5,
            },
        )
        .expect("valid quality observation");

        let failures = quality_failures(&case(false), &scores);

        assert!(failures.iter().any(|failure| failure.contains("unlabeled")));
    }

    #[test]
    fn reducer_keeps_all_invalid_provider_runs_artifactable_without_fake_metrics() {
        let mut invalid = outcome("unavailable-provider", None);
        invalid.status = CaseStatus::Invalid;
        invalid.invalid_reason = Some("embedding provider unavailable".into());
        let summary = RetrievalQualityReducer::new()
            .reduce(&[invalid])
            .expect("invalid runs still need an artifact summary");

        assert_eq!(summary[0].invalid, 1);
        assert!(summary[0].metrics.is_empty());
    }

    #[test]
    fn reducer_uses_frozen_ratio_evidence_and_preserves_metric_applicability() {
        fn with_metrics(case_id: &str, metrics: &[(&str, u64, u64)]) -> EvalCaseOutcome {
            let mut outcome = outcome(case_id, None);
            for (name, numerator, denominator) in metrics {
                outcome.evidence.insert(
                    (*name).into(),
                    MetricEvidence::ratio(*numerator, *denominator),
                );
            }
            outcome
        }

        let outcomes = vec![
            with_metrics(
                "answerable-good",
                &[
                    ("precision_macro", 1, 1),
                    ("required_group_recall", 1, 1),
                    ("answerable_non_abstention_rate", 1, 1),
                    ("duplicate_free_rate", 1, 1),
                    ("prohibited_free_rate", 1, 1),
                ],
            ),
            with_metrics(
                "answerable-partial",
                &[
                    ("precision_macro", 0, 4),
                    ("required_group_recall", 1, 2),
                    ("answerable_non_abstention_rate", 1, 1),
                    ("duplicate_free_rate", 0, 1),
                    ("prohibited_free_rate", 0, 1),
                ],
            ),
            with_metrics(
                "unknown-empty",
                &[
                    ("unknown_abstention_rate", 1, 1),
                    ("duplicate_free_rate", 1, 1),
                    ("prohibited_free_rate", 1, 1),
                ],
            ),
            with_metrics(
                "unknown-noisy",
                &[
                    ("unknown_abstention_rate", 0, 1),
                    ("duplicate_free_rate", 1, 1),
                    ("prohibited_free_rate", 0, 1),
                ],
            ),
        ];

        let summary = RetrievalQualityReducer::new()
            .reduce(&outcomes)
            .expect("ratio evidence must reduce");
        let metrics = &summary[0].metrics;

        assert_eq!(metrics.get("precision_macro"), Some(&0.5));
        assert_eq!(metrics.get("precision_macro_applicable_count"), Some(&2.0));
        assert_eq!(metrics.get("required_group_recall"), Some(&(2.0 / 3.0)));
        assert_eq!(
            metrics.get("required_group_recall_applicable_count"),
            Some(&3.0)
        );
        assert_eq!(metrics.get("unknown_abstention_rate"), Some(&0.5));
        assert_eq!(
            metrics.get("unknown_abstention_rate_applicable_count"),
            Some(&2.0)
        );
        assert_eq!(metrics.get("answerable_non_abstention_rate"), Some(&1.0));
        assert_eq!(
            metrics.get("answerable_non_abstention_rate_applicable_count"),
            Some(&2.0)
        );
        assert_eq!(metrics.get("duplicate_free_rate"), Some(&0.75));
        assert_eq!(
            metrics.get("duplicate_free_rate_applicable_count"),
            Some(&4.0)
        );
        assert_eq!(metrics.get("prohibited_free_rate"), Some(&0.5));
        assert_eq!(
            metrics.get("prohibited_free_rate_applicable_count"),
            Some(&4.0)
        );
        assert!(!metrics.contains_key("precision_at_budget"));
        assert!(!metrics.contains_key("recall_at_budget"));
    }
}

#[cfg(test)]
mod source_bundle_tests {
    use chrono::{DateTime, Utc};
    use sha2::Digest;

    use super::*;
    use crate::domain::{CorpusSplit, LabelTrust};
    use cases::{NativePreparationManifest, SourceBundleEntry, SourceEpisode};

    #[test]
    fn source_bundle_must_contain_the_exact_episodes_used_by_cases() {
        let temp = tempfile::tempdir().expect("temporary source bundle directory");
        let source_bytes = b"[]";
        std::fs::write(temp.path().join("sources.json"), source_bytes)
            .expect("write a correctly hashed but unrelated source bundle");
        let digest = hex::encode(sha2::Sha256::digest(source_bytes));
        let timestamp = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .expect("fixed valid timestamp")
            .with_timezone(&Utc);
        let source = SourceEpisode {
            key: "episode-1".into(),
            source_type: "conversation".into(),
            source_id: "source-1".into(),
            content: "The user prefers tea.".into(),
            t_ref: timestamp,
            t_ingested: None,
            policy_tags: vec![],
        };
        let manifest = QualityCorpusManifest {
            schema_version: "memory-mcp-retrieval-quality/v1".into(),
            corpus_id: "test-corpus".into(),
            revision: "r1".into(),
            source_bundle_path: "sources.json".into(),
            source_sha256: digest.clone(),
            native_preparation: NativePreparationManifest {
                schema_version: "memory-mcp-native-preparation/v1".into(),
                source_bundle_sha256: digest,
                provider_label: "openai-compatible".into(),
                model: Some("embed-model-v1".into()),
                embedding_signature: "embsig:test".into(),
                embedding_dimension: 1536,
                similarity_threshold: 0.7,
                index_signature: "indexsig:test".into(),
                extractor_fingerprint: "anno".into(),
                claim_rollout_stage: "disabled".into(),
            },
            cases: vec![QualityCase {
                id: "case-1".into(),
                split: CorpusSplit::Test,
                label_trust: LabelTrust::Reviewed,
                query: "What is the user's preference?".into(),
                budget: 5,
                as_of: timestamp,
                tags: vec!["preference".into()],
                fact_types: vec![],
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
                compact: true,
                sources: vec![source],
                bindings: vec![],
                required_evidence_groups: vec![],
                expect_empty: false,
                expected_order: None,
            }],
        };

        let result = verify_source_bundle(temp.path(), &manifest);

        assert!(
            result.is_err(),
            "matching SHA alone must not let unused source bytes masquerade as native inputs"
        );
    }

    #[test]
    fn source_bundle_accepts_the_exact_declared_case_episodes() {
        let temp = tempfile::tempdir().expect("temporary source bundle directory");
        let timestamp = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .expect("fixed valid timestamp")
            .with_timezone(&Utc);
        let source = SourceEpisode {
            key: "episode-1".into(),
            source_type: "conversation".into(),
            source_id: "source-1".into(),
            content: "The user prefers tea.".into(),
            t_ref: timestamp,
            t_ingested: None,
            policy_tags: vec![],
        };
        let source_bytes = serde_json::to_vec(&[SourceBundleEntry {
            case_id: "case-1".into(),
            source: source.clone(),
        }])
        .expect("serialize exact source bundle");
        std::fs::write(temp.path().join("sources.json"), &source_bytes)
            .expect("write source bundle");
        let digest = hex::encode(sha2::Sha256::digest(&source_bytes));
        let manifest = QualityCorpusManifest {
            schema_version: "memory-mcp-retrieval-quality/v1".into(),
            corpus_id: "test-corpus".into(),
            revision: "r1".into(),
            source_bundle_path: "sources.json".into(),
            source_sha256: digest.clone(),
            native_preparation: NativePreparationManifest {
                schema_version: "memory-mcp-native-preparation/v1".into(),
                source_bundle_sha256: digest,
                provider_label: "openai-compatible".into(),
                model: Some("embed-model-v1".into()),
                embedding_signature: "embsig:test".into(),
                embedding_dimension: 1536,
                similarity_threshold: 0.7,
                index_signature: "indexsig:test".into(),
                extractor_fingerprint: "anno".into(),
                claim_rollout_stage: "disabled".into(),
            },
            cases: vec![QualityCase {
                id: "case-1".into(),
                split: CorpusSplit::Test,
                label_trust: LabelTrust::Reviewed,
                query: "What is the user's preference?".into(),
                budget: 5,
                as_of: timestamp,
                tags: vec!["preference".into()],
                fact_types: vec![],
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
                compact: true,
                sources: vec![source],
                bindings: vec![],
                required_evidence_groups: vec![],
                expect_empty: false,
                expected_order: None,
            }],
        };

        assert!(verify_source_bundle(temp.path(), &manifest).is_ok());
    }

    #[test]
    fn source_bundle_digest_mismatch_invalidates_the_corpus() {
        let temp = tempfile::tempdir().expect("temporary source bundle directory");
        std::fs::write(temp.path().join("sources.json"), b"[]").expect("write source bundle");
        let manifest = QualityCorpusManifest {
            schema_version: "memory-mcp-retrieval-quality/v1".into(),
            corpus_id: "test-corpus".into(),
            revision: "r1".into(),
            source_bundle_path: "sources.json".into(),
            source_sha256: "a".repeat(64),
            native_preparation: NativePreparationManifest {
                schema_version: "memory-mcp-native-preparation/v1".into(),
                source_bundle_sha256: "a".repeat(64),
                provider_label: "openai-compatible".into(),
                model: Some("embed-model-v1".into()),
                embedding_signature: "embsig:test".into(),
                embedding_dimension: 1536,
                similarity_threshold: 0.7,
                index_signature: "indexsig:test".into(),
                extractor_fingerprint: "anno".into(),
                claim_rollout_stage: "disabled".into(),
            },
            cases: vec![],
        };

        assert!(verify_source_bundle(temp.path(), &manifest).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn source_bundle_symlink_cannot_escape_the_corpus_root() {
        let root = tempfile::tempdir().expect("temporary corpus directory");
        let outside = tempfile::tempdir().expect("temporary external source directory");
        let source_bytes = b"[]";
        let outside_path = outside.path().join("sources.json");
        std::fs::write(&outside_path, source_bytes).expect("write external source bundle");
        std::os::unix::fs::symlink(&outside_path, root.path().join("linked.json"))
            .expect("link external source bundle");
        let digest = hex::encode(sha2::Sha256::digest(source_bytes));
        let manifest = QualityCorpusManifest {
            schema_version: "memory-mcp-retrieval-quality/v1".into(),
            corpus_id: "test-corpus".into(),
            revision: "r1".into(),
            source_bundle_path: "linked.json".into(),
            source_sha256: digest.clone(),
            native_preparation: NativePreparationManifest {
                schema_version: "memory-mcp-native-preparation/v1".into(),
                source_bundle_sha256: digest,
                provider_label: "openai-compatible".into(),
                model: Some("embed-model-v1".into()),
                embedding_signature: "embsig:test".into(),
                embedding_dimension: 1536,
                similarity_threshold: 0.7,
                index_signature: "indexsig:test".into(),
                extractor_fingerprint: "anno".into(),
                claim_rollout_stage: "disabled".into(),
            },
            cases: vec![],
        };

        assert!(verify_source_bundle(root.path(), &manifest).is_err());
    }
}
