use crate::perf::PerfError;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteEvidence {
    pub server_digest: String,
    pub commit: String,
    pub schema_digest: String,
    pub model_digest: String,
    pub corpus_digest: String,
    pub native: bool,
    pub scenarios: Vec<RemoteScenarioEvidence>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteScenarioEvidence {
    pub scenario_id: String,
    pub dimension: Option<usize>,
    pub cache_mode: String,
    pub successful_samples: u64,
    pub app_cpu_percent: Option<f64>,
    pub db_cpu_percent: Option<f64>,
    pub app_rss_bytes: Option<u64>,
    pub db_rss_bytes: Option<u64>,
    pub sql_count: Option<u64>,
    pub rpc_count: Option<u64>,
    pub signin_count: Option<u64>,
    pub latency_us: Vec<u64>,
    pub throughput_ops_per_sec: Option<f64>,
    pub quality_cases: BTreeMap<String, QualityCaseEvidence>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityCaseEvidence {
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
    validate_digest("parent server", &parent.server_digest)?;
    validate_digest("candidate server", &candidate.server_digest)?;
    validate_commit("parent commit", &parent.commit)?;
    validate_commit("candidate commit", &candidate.commit)?;
    validate_digest("parent schema", &parent.schema_digest)?;
    validate_digest("candidate schema", &candidate.schema_digest)?;
    validate_digest("parent model", &parent.model_digest)?;
    validate_digest("candidate model", &candidate.model_digest)?;
    validate_digest("parent corpus", &parent.corpus_digest)?;
    validate_digest("candidate corpus", &candidate.corpus_digest)?;

    if parent.server_digest != candidate.server_digest
        || parent.schema_digest != candidate.schema_digest
        || parent.model_digest != candidate.model_digest
        || parent.corpus_digest != candidate.corpus_digest
    {
        return invalid("parent and candidate must use the same server, schema, model and corpus");
    }
    if parent.native != candidate.native {
        return invalid("parent and candidate architecture/emulation modes differ");
    }
    if parent.scenarios.is_empty() || candidate.scenarios.is_empty() {
        return invalid("scenario evidence is empty");
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
        {
            return invalid(format!("scenario identity differs for {id}"));
        }
        validate_scenario(id, parent_scenario, "parent")?;
        validate_scenario(id, candidate_scenario, "candidate")?;
        validate_quality_coverage(id, parent_scenario, candidate_scenario)?;
    }
    Ok(())
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
        if scenario.scenario_id.is_empty()
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
    if scenario.successful_samples == 0
        || scenario.latency_us.is_empty()
        || scenario.latency_us.len() as u64 != scenario.successful_samples
    {
        return invalid(format!(
            "{side} scenario {id} has incomplete latency samples"
        ));
    }
    for (name, value) in [
        ("app CPU", scenario.app_cpu_percent),
        ("database CPU", scenario.db_cpu_percent),
        ("throughput", scenario.throughput_ops_per_sec),
    ] {
        let Some(value) = value else {
            return invalid(format!("{side} scenario {id} is missing {name}"));
        };
        if !value.is_finite() || value < 0.0 {
            return invalid(format!("{side} scenario {id} has invalid {name}"));
        }
    }
    for (name, value) in [
        ("app RSS", scenario.app_rss_bytes),
        ("database RSS", scenario.db_rss_bytes),
        ("SQL count", scenario.sql_count),
        ("RPC count", scenario.rpc_count),
        ("signin count", scenario.signin_count),
    ] {
        if value.is_none() {
            return invalid(format!("{side} scenario {id} is missing {name}"));
        }
    }
    if scenario.quality_cases.is_empty() {
        return invalid(format!("{side} scenario {id} has no quality cases"));
    }
    for (case_id, case) in &scenario.quality_cases {
        if case_id.is_empty() {
            return invalid(format!("{side} scenario {id} has an empty quality case id"));
        }
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
        let relevant: BTreeSet<_> = before.relevant_ids.iter().collect();
        let after_relevant: BTreeSet<_> = after.relevant_ids.iter().collect();
        if relevant != after_relevant {
            return invalid(format!("relevant fact coverage differs for {id}/{case_id}"));
        }
        let parent_retrieved_relevant: BTreeSet<_> = before
            .retrieved_ids
            .iter()
            .filter(|fact_id| relevant.contains(fact_id))
            .collect();
        let candidate_retrieved_relevant: BTreeSet<_> = after
            .retrieved_ids
            .iter()
            .filter(|fact_id| relevant.contains(fact_id))
            .collect();
        if !parent_retrieved_relevant.is_subset(&candidate_retrieved_relevant) {
            return invalid(format!(
                "candidate lost previously retrieved relevant facts for {id}/{case_id}"
            ));
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
