use crate::perf::PerfError;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const REMOTE_EVIDENCE_SCHEMA_NAME: &str = "memory-mcp-surreal-query-evidence/v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemoteEvidenceSchema {
    #[serde(rename = "memory-mcp-surreal-query-evidence/v2")]
    V2,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteScenarioManifest {
    pub scenario_id: String,
    pub dimension: Option<usize>,
    pub cache_mode: String,
    pub quality_case_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteEvidence {
    pub schema: RemoteEvidenceSchema,
    pub server_digest: String,
    pub commit: String,
    pub schema_digest: String,
    pub model_digest: String,
    pub corpus_digest: String,
    pub scenario_manifest: Vec<RemoteScenarioManifest>,
    pub scenario_manifest_digest: String,
    pub run_id: String,
    pub started_at: String,
    pub as_of: String,
    pub native: bool,
    pub scenarios: Vec<RemoteScenarioEvidence>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteScenarioEvidence {
    pub scenario_id: String,
    pub dimension: Option<usize>,
    pub cache_mode: String,
    pub successful_samples: u64,
    pub measured_duration_ms: u64,
    pub resource_samples: Vec<RemoteResourceSample>,
    pub sql_count: Option<u64>,
    pub rpc_count: Option<u64>,
    pub session_attach_count: Option<u64>,
    pub session_create_count: Option<u64>,
    pub session_reuse_count: Option<u64>,
    pub session_eviction_count: Option<u64>,
    pub signin_count: Option<u64>,
    pub latency_us: Vec<u64>,
    pub throughput_ops_per_sec: Option<f64>,
    pub quality_cases: BTreeMap<String, QualityCaseEvidence>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteResourceSample {
    pub pair_id: String,
    pub interval_ms: u64,
    pub app: RemoteProcessSnapshot,
    pub database: RemoteProcessSnapshot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteProcessSnapshot {
    pub cpu_percent: Option<f64>,
    pub rss_bytes: Option<u64>,
    pub peak_rss_bytes: Option<u64>,
    pub cgroup_memory_bytes: Option<u64>,
    pub swap_bytes: Option<u64>,
    pub physical_read_bytes: Option<u64>,
    pub physical_write_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QualityCaseEvidence {
    pub retrieval_limit: usize,
    pub relevant_ids: Vec<String>,
    pub retrieved_ids: Vec<String>,
    pub recall: f64,
    pub precision: f64,
    pub mrr: f64,
    pub ndcg: f64,
    pub answer_support: f64,
}

pub fn validate_remote_evidence(
    parent: &RemoteEvidence,
    candidate: &RemoteEvidence,
) -> Result<(), PerfError> {
    validate_evidence(parent, "parent")?;
    validate_evidence(candidate, "candidate")?;

    if parent.run_id == candidate.run_id {
        return invalid("parent and candidate must be distinct measurement runs");
    }
    if parent.server_digest != candidate.server_digest
        || parent.schema_digest != candidate.schema_digest
        || parent.model_digest != candidate.model_digest
        || parent.corpus_digest != candidate.corpus_digest
        || parent.scenario_manifest != candidate.scenario_manifest
        || parent.scenario_manifest_digest != candidate.scenario_manifest_digest
        || parent.as_of != candidate.as_of
    {
        return invalid(
            "parent and candidate must use the same server, schema, model, corpus, manifest and as_of",
        );
    }
    if parent.native != candidate.native {
        return invalid("parent and candidate architecture/emulation modes differ");
    }
    let parent_scenarios = index_scenarios(&parent.scenarios, "parent")?;
    let candidate_scenarios = index_scenarios(&candidate.scenarios, "candidate")?;
    if parent_scenarios.keys().collect::<BTreeSet<_>>()
        != candidate_scenarios.keys().collect::<BTreeSet<_>>()
    {
        return invalid("parent and candidate scenario coverage differs");
    }

    for (id, parent_scenario) in parent_scenarios {
        let Some(candidate_scenario) = candidate_scenarios.get(id) else {
            return invalid(format!("candidate is missing scenario {id}"));
        };
        if parent_scenario.dimension != candidate_scenario.dimension
            || parent_scenario.cache_mode != candidate_scenario.cache_mode
            || parent_scenario.measured_duration_ms != candidate_scenario.measured_duration_ms
        {
            return invalid(format!("scenario identity differs for {id}"));
        }
        validate_resource_pairing(id, parent_scenario, candidate_scenario)?;
        validate_quality_coverage(id, parent_scenario, candidate_scenario)?;
    }
    Ok(())
}

fn validate_evidence(evidence: &RemoteEvidence, side: &str) -> Result<(), PerfError> {
    validate_digest(&format!("{side} server"), &evidence.server_digest)?;
    validate_commit(&format!("{side} commit"), &evidence.commit)?;
    validate_digest(&format!("{side} schema"), &evidence.schema_digest)?;
    validate_digest(&format!("{side} model"), &evidence.model_digest)?;
    validate_digest(&format!("{side} corpus"), &evidence.corpus_digest)?;
    validate_digest(
        &format!("{side} scenario manifest"),
        &evidence.scenario_manifest_digest,
    )?;
    if evidence.run_id.trim().is_empty()
        || evidence.started_at.trim().is_empty()
        || evidence.as_of.trim().is_empty()
    {
        return invalid(format!(
            "{side} evidence is missing run identity or fixed as_of"
        ));
    }
    if evidence.scenarios.is_empty() {
        return invalid(format!("{side} scenario evidence is empty"));
    }
    let scenarios = index_scenarios(&evidence.scenarios, side)?;
    if evidence.scenario_manifest.is_empty()
        || evidence.scenario_manifest.iter().any(|entry| {
            entry.scenario_id.trim().is_empty()
                || entry.dimension == Some(0)
                || !matches!(entry.cache_mode.as_str(), "cold" | "warm")
                || entry.quality_case_ids.iter().any(|id| id.trim().is_empty())
                || entry
                    .quality_case_ids
                    .windows(2)
                    .any(|pair| pair[0] >= pair[1])
        })
        || evidence
            .scenario_manifest
            .windows(2)
            .any(|pair| pair[0].scenario_id >= pair[1].scenario_id)
    {
        return invalid(format!(
            "{side} scenario manifest must be sorted, unique and non-empty"
        ));
    }
    let manifest_contents = serde_json::to_vec(&evidence.scenario_manifest)?;
    let computed_manifest_digest = crate::perf::artifact::sha256_hex(&manifest_contents);
    if evidence.scenario_manifest_digest != computed_manifest_digest {
        return invalid(format!(
            "{side} scenario manifest digest does not match its profiles"
        ));
    }
    let actual_scenario_ids: BTreeSet<_> = scenarios.keys().copied().collect();
    let manifest_scenario_ids: BTreeSet<_> = evidence
        .scenario_manifest
        .iter()
        .map(|entry| entry.scenario_id.as_str())
        .collect();
    if actual_scenario_ids != manifest_scenario_ids {
        return invalid(format!(
            "{side} scenarios do not match the scenario manifest"
        ));
    }
    for entry in &evidence.scenario_manifest {
        let Some(scenario) = scenarios.get(entry.scenario_id.as_str()) else {
            return invalid(format!(
                "{side} scenarios do not match the scenario manifest"
            ));
        };
        let quality_case_ids: Vec<_> = scenario.quality_cases.keys().cloned().collect();
        if scenario.dimension != entry.dimension
            || scenario.cache_mode != entry.cache_mode
            || quality_case_ids != entry.quality_case_ids
        {
            return invalid(format!(
                "{side} scenario {} does not match its frozen manifest profile",
                entry.scenario_id
            ));
        }
    }
    for (id, scenario) in scenarios {
        validate_scenario(id, scenario, side)?;
    }
    Ok(())
}

pub fn write(evidence: &RemoteEvidence, path: &Path) -> Result<(), PerfError> {
    validate_evidence(evidence, "evidence")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            PerfError::Validation(format!("cannot create {}: {error}", parent.display()))
        })?;
    }
    let json = serde_json::to_string_pretty(evidence)?;
    std::fs::write(path, format!("{json}\n"))
        .map_err(|error| PerfError::Validation(format!("cannot write {}: {error}", path.display())))
}

pub fn read(path: &Path) -> Result<RemoteEvidence, PerfError> {
    let raw = std::fs::read_to_string(path).map_err(|error| {
        PerfError::Validation(format!("cannot read {}: {error}", path.display()))
    })?;
    let value: serde_json::Value = serde_json::from_str(&raw)?;
    let schema = value
        .get("schema")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| PerfError::Validation("remote evidence is missing schema".to_owned()))?;
    if schema != REMOTE_EVIDENCE_SCHEMA_NAME {
        return Err(PerfError::Schema(schema.to_owned()));
    }
    let evidence: RemoteEvidence = serde_json::from_value(value)?;
    validate_evidence(&evidence, "evidence")?;
    Ok(evidence)
}

fn validate_digest(label: &str, value: &str) -> Result<(), PerfError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return invalid(format!(
            "{label} digest must be a 64-character hexadecimal SHA-256"
        ));
    }
    Ok(())
}

fn validate_commit(label: &str, value: &str) -> Result<(), PerfError> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return invalid(format!(
            "{label} must be a full 40-character hexadecimal commit"
        ));
    }
    Ok(())
}

fn index_scenarios<'a>(
    scenarios: &'a [RemoteScenarioEvidence],
    side: &str,
) -> Result<BTreeMap<&'a str, &'a RemoteScenarioEvidence>, PerfError> {
    let mut indexed = BTreeMap::new();
    for scenario in scenarios {
        if scenario.scenario_id.trim().is_empty()
            || indexed
                .insert(scenario.scenario_id.as_str(), scenario)
                .is_some()
        {
            return invalid(format!("{side} has an empty or duplicate scenario id"));
        }
    }
    Ok(indexed)
}

fn validate_scenario(
    id: &str,
    scenario: &RemoteScenarioEvidence,
    side: &str,
) -> Result<(), PerfError> {
    if scenario.scenario_id.trim().is_empty() {
        return invalid(format!("{side} has an empty scenario id"));
    }
    if scenario.dimension == Some(0) {
        return invalid(format!(
            "{side} scenario {id} has a zero embedding dimension"
        ));
    }
    if !matches!(scenario.cache_mode.as_str(), "cold" | "warm") {
        return invalid(format!("{side} scenario {id} has an unknown cache mode"));
    }
    if scenario.latency_us.len() as u64 != scenario.successful_samples {
        return invalid(format!(
            "{side} scenario {id} has incomplete latency samples"
        ));
    }
    if scenario.measured_duration_ms == 0 {
        return invalid(format!("{side} scenario {id} has zero measured duration"));
    }
    let Some(recorded_throughput) = scenario.throughput_ops_per_sec else {
        return invalid(format!("{side} scenario {id} is missing throughput"));
    };
    if !recorded_throughput.is_finite() || recorded_throughput < 0.0 {
        return invalid(format!("{side} scenario {id} has invalid throughput"));
    }
    for (name, value) in [
        ("SQL count", scenario.sql_count),
        ("RPC count", scenario.rpc_count),
        ("session attach count", scenario.session_attach_count),
        ("session create count", scenario.session_create_count),
        ("session reuse count", scenario.session_reuse_count),
        ("session eviction count", scenario.session_eviction_count),
        ("signin count", scenario.signin_count),
    ] {
        if value.is_none() {
            return invalid(format!("{side} scenario {id} is missing {name}"));
        }
    }
    if scenario.resource_samples.is_empty() {
        return invalid(format!(
            "{side} scenario {id} has no paired resource samples"
        ));
    }
    let resource_samples = index_resource_samples(&scenario.resource_samples, id, side)?;
    let resource_duration_ms = resource_samples
        .values()
        .try_fold(0_u64, |total, sample| total.checked_add(sample.interval_ms));
    let Some(resource_duration_ms) = resource_duration_ms else {
        return invalid(format!("{side} scenario {id} resource duration overflows"));
    };
    if resource_duration_ms != scenario.measured_duration_ms {
        return invalid(format!(
            "{side} scenario {id} duration does not match resource samples"
        ));
    }
    for sample in resource_samples.values() {
        if sample.interval_ms == 0 {
            return invalid(format!(
                "{side} scenario {id} has a zero-length resource interval"
            ));
        }
        validate_process_snapshot(id, &sample.pair_id, "app", &sample.app, side)?;
        validate_process_snapshot(id, &sample.pair_id, "database", &sample.database, side)?;
    }
    let measured_throughput =
        scenario.successful_samples as f64 * 1_000.0 / scenario.measured_duration_ms as f64;
    if !rate_close_enough(recorded_throughput, measured_throughput) {
        return invalid(format!(
            "{side} scenario {id} throughput does not match successful samples and duration"
        ));
    }
    for (case_id, case) in &scenario.quality_cases {
        if case_id.is_empty() {
            return invalid(format!("{side} scenario {id} has an empty quality case id"));
        }
        validate_quality_metrics(id, case_id, case)?;
        for (metric, value) in [
            ("recall", case.recall),
            ("precision", case.precision),
            ("MRR", case.mrr),
            ("NDCG", case.ndcg),
            ("answer support", case.answer_support),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return invalid(format!(
                    "{side} scenario {id} case {case_id} has invalid {metric}"
                ));
            }
        }
    }
    Ok(())
}

fn index_resource_samples<'a>(
    samples: &'a [RemoteResourceSample],
    scenario_id: &str,
    side: &str,
) -> Result<BTreeMap<&'a str, &'a RemoteResourceSample>, PerfError> {
    let mut indexed = BTreeMap::new();
    for sample in samples {
        if sample.pair_id.trim().is_empty()
            || indexed.insert(sample.pair_id.as_str(), sample).is_some()
        {
            return invalid(format!(
                "{side} scenario {scenario_id} has an empty or duplicate resource pair id"
            ));
        }
    }
    Ok(indexed)
}

fn validate_process_snapshot(
    scenario_id: &str,
    pair_id: &str,
    process: &str,
    snapshot: &RemoteProcessSnapshot,
    side: &str,
) -> Result<(), PerfError> {
    let Some(cpu) = snapshot.cpu_percent else {
        return invalid(format!(
            "{side} scenario {scenario_id} pair {pair_id} is missing {process} CPU"
        ));
    };
    if !cpu.is_finite() || cpu < 0.0 {
        return invalid(format!(
            "{side} scenario {scenario_id} pair {pair_id} has invalid {process} CPU"
        ));
    }
    for (name, value) in [
        ("RSS", snapshot.rss_bytes),
        ("peak RSS", snapshot.peak_rss_bytes),
        ("cgroup memory", snapshot.cgroup_memory_bytes),
        ("swap", snapshot.swap_bytes),
        ("physical read", snapshot.physical_read_bytes),
        ("physical write", snapshot.physical_write_bytes),
    ] {
        if value.is_none() {
            return invalid(format!(
                "{side} scenario {scenario_id} pair {pair_id} is missing {process} {name}"
            ));
        }
    }
    Ok(())
}

fn validate_resource_pairing(
    scenario_id: &str,
    parent: &RemoteScenarioEvidence,
    candidate: &RemoteScenarioEvidence,
) -> Result<(), PerfError> {
    let parent_samples = index_resource_samples(&parent.resource_samples, scenario_id, "parent")?;
    let candidate_samples =
        index_resource_samples(&candidate.resource_samples, scenario_id, "candidate")?;
    if parent_samples.keys().collect::<BTreeSet<_>>()
        != candidate_samples.keys().collect::<BTreeSet<_>>()
    {
        return invalid(format!("resource sample pairing differs for {scenario_id}"));
    }
    for (pair_id, before) in parent_samples {
        let Some(after) = candidate_samples.get(pair_id) else {
            return invalid(format!(
                "candidate is missing resource pair {scenario_id}/{pair_id}"
            ));
        };
        if before.interval_ms != after.interval_ms {
            return invalid(format!(
                "resource interval differs for {scenario_id}/{pair_id}"
            ));
        }
    }
    Ok(())
}

fn validate_quality_metrics(
    scenario_id: &str,
    case_id: &str,
    case: &QualityCaseEvidence,
) -> Result<(), PerfError> {
    if case.relevant_ids.iter().any(String::is_empty) {
        return invalid(format!(
            "scenario {scenario_id} case {case_id} has an empty relevant id"
        ));
    }
    if case.retrieved_ids.iter().any(String::is_empty) {
        return invalid(format!(
            "scenario {scenario_id} case {case_id} has an empty retrieved id"
        ));
    }

    let relevant: BTreeSet<_> = case.relevant_ids.iter().collect();
    let retrieved: BTreeSet<_> = case.retrieved_ids.iter().collect();
    let hits = retrieved.intersection(&relevant).count();
    let expected_recall = ratio(hits, relevant.len());
    let expected_precision = ratio(hits, retrieved.len());
    let expected_mrr = case
        .retrieved_ids
        .iter()
        .position(|fact_id| relevant.contains(fact_id))
        .map_or(0.0, |rank| 1.0 / (rank + 1) as f64);
    if case.retrieval_limit == 0 || case.retrieved_ids.len() > case.retrieval_limit {
        return invalid(format!(
            "scenario {scenario_id} case {case_id} has an invalid retrieval limit"
        ));
    }
    let expected_ndcg = ndcg(&case.retrieved_ids, &relevant, case.retrieval_limit);

    for (name, recorded, expected) in [
        ("recall", case.recall, expected_recall),
        ("precision", case.precision, expected_precision),
        ("MRR", case.mrr, expected_mrr),
        ("NDCG", case.ndcg, expected_ndcg),
    ] {
        if !close_enough(recorded, expected) {
            return invalid(format!(
                "scenario {scenario_id} case {case_id} has {name} inconsistent with ranked ids"
            ));
        }
    }
    Ok(())
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn ndcg(retrieved: &[String], relevant: &BTreeSet<&String>, limit: usize) -> f64 {
    let mut seen = BTreeSet::new();
    let dcg = retrieved
        .iter()
        .take(limit)
        .enumerate()
        .filter(|(_, id)| relevant.contains(id) && seen.insert(*id))
        .map(|(rank, _)| 1.0 / ((rank + 2) as f64).log2())
        .sum::<f64>();
    let ideal_hits = relevant.len().min(limit);
    let ideal_dcg = (0..ideal_hits)
        .map(|rank| 1.0 / ((rank + 2) as f64).log2())
        .sum::<f64>();
    if ideal_dcg == 0.0 {
        0.0
    } else {
        dcg / ideal_dcg
    }
}

fn close_enough(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() <= 1e-9
}

fn rate_close_enough(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() <= expected.abs().max(1.0) * 1e-6
}

fn validate_quality_coverage(
    id: &str,
    parent: &RemoteScenarioEvidence,
    candidate: &RemoteScenarioEvidence,
) -> Result<(), PerfError> {
    if parent.quality_cases.keys().collect::<BTreeSet<_>>()
        != candidate.quality_cases.keys().collect::<BTreeSet<_>>()
    {
        return invalid(format!("quality case coverage differs for scenario {id}"));
    }
    for (case_id, before) in &parent.quality_cases {
        let Some(after) = candidate.quality_cases.get(case_id) else {
            return invalid(format!("candidate is missing quality case {id}/{case_id}"));
        };
        if before.retrieval_limit != after.retrieval_limit {
            return invalid(format!("retrieval limit differs for {id}/{case_id}"));
        }
        let relevant: BTreeSet<_> = before.relevant_ids.iter().collect();
        let after_relevant: BTreeSet<_> = after.relevant_ids.iter().collect();
        if relevant != after_relevant {
            return invalid(format!("relevant fact coverage differs for {id}/{case_id}"));
        }
        if before.retrieved_ids != after.retrieved_ids {
            return invalid(format!("ranked result ids differ for {id}/{case_id}"));
        }
        for (name, old, new) in [
            ("recall", before.recall, after.recall),
            ("precision", before.precision, after.precision),
            ("MRR", before.mrr, after.mrr),
            ("NDCG", before.ndcg, after.ndcg),
            (
                "answer support",
                before.answer_support,
                after.answer_support,
            ),
        ] {
            if new < old {
                return invalid(format!("candidate {name} regressed for {id}/{case_id}"));
            }
        }
    }
    Ok(())
}

fn invalid<T>(message: impl Into<String>) -> Result<T, PerfError> {
    Err(PerfError::Validation(message.into()))
}
