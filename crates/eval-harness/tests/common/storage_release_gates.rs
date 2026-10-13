use std::collections::{BTreeMap, BTreeSet};

use eval_harness::artifact::{EVAL_ARTIFACT_SCHEMA_V2, GateStatus, RunArtifact};
use eval_harness::domain::{CaseStatus, EvalMode, EvalProfile, RunVerdict};
use eval_harness::profile::ProfileManifest;
use sha2::Digest;

#[derive(Debug, PartialEq, Eq)]
pub enum ReleaseGateVerdict {
    Passed,
    Regressed(Vec<String>),
    Invalid(Vec<String>),
}

struct RunMetrics {
    latency_p50_ms: f64,
    latency_p95_ms: f64,
    latency_p99_ms: f64,
    throughput_rps: f64,
    app_cpu_ms_per_attempt: f64,
    db_cpu_ms_per_attempt: f64,
    error_rate: f64,
    action_fact_count: Option<u64>,
}

#[derive(Clone, Copy)]
struct PairRatios {
    latency_p50: f64,
    latency_p95: f64,
    latency_p99: f64,
    throughput: f64,
    app_cpu: f64,
    db_cpu: f64,
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    }
}

fn bootstrap_interval(values: &[f64]) -> (f64, f64) {
    const RESAMPLES: usize = 10_000;
    let mut state = 20_261_013u64;
    let mut means = Vec::with_capacity(RESAMPLES);
    for _ in 0..RESAMPLES {
        let mut block_sample = Vec::with_capacity(values.len());
        for _ in 0..values.len() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            block_sample.push(values[(state as usize) % values.len()]);
        }
        means.push(median(&block_sample));
    }
    means.sort_by(f64::total_cmp);
    let lower = ((0.05 * means.len() as f64).ceil() as usize).saturating_sub(1);
    let upper = ((0.95 * means.len() as f64).ceil() as usize).saturating_sub(1);
    (means[lower], means[upper])
}

fn action_metrics(run: &serde_json::Value) -> Result<RunMetrics, &'static str> {
    let case = run.get("case").ok_or("missing_action_case")?;
    let scenario = case
        .get("scenario")
        .and_then(serde_json::Value::as_str)
        .ok_or("missing_action_case")?;
    if !matches!(scenario, "startup" | "backfill" | "reembed") {
        return Err("invalid_action_scenario");
    }
    let action = run.get("action").ok_or("missing_action_evidence")?;
    if action.get("kind").and_then(serde_json::Value::as_str) != Some(scenario) {
        return Err("action_kind_mismatch");
    }
    if action.get("completed").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err("action_not_completed");
    }
    if action
        .get("durable_state_verified")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
    {
        return Err("action_durability_not_verified");
    }
    let requested_facts = action
        .get("requested_facts")
        .and_then(serde_json::Value::as_u64)
        .ok_or("missing_action_fact_counts")?;
    let completed_facts = action
        .get("completed_facts")
        .and_then(serde_json::Value::as_u64)
        .ok_or("missing_action_fact_counts")?;
    let persisted_facts = action
        .get("persisted_facts")
        .and_then(serde_json::Value::as_u64)
        .ok_or("missing_action_fact_counts")?;
    let failed_facts = action
        .get("failed_facts")
        .and_then(serde_json::Value::as_u64)
        .ok_or("missing_action_fact_counts")?;
    if requested_facts != completed_facts || requested_facts != persisted_facts {
        return Err("action_fact_count_mismatch");
    }
    if failed_facts != 0 {
        return Err("action_has_failed_facts");
    }
    if matches!(scenario, "backfill" | "reembed") && requested_facts == 0 {
        return Err("action_has_no_facts");
    }
    let elapsed_ms = action
        .get("elapsed_ms")
        .and_then(serde_json::Value::as_f64)
        .ok_or("missing_action_duration")?;
    let app_cpu_ms = run
        .get("app_cpu_ms")
        .and_then(serde_json::Value::as_f64)
        .ok_or("missing_app_cpu")?;
    let db_cpu_ms = run
        .get("db_cpu_ms")
        .and_then(serde_json::Value::as_f64)
        .ok_or("missing_db_cpu")?;
    if !elapsed_ms.is_finite()
        || elapsed_ms <= 0.0
        || !app_cpu_ms.is_finite()
        || app_cpu_ms <= 0.0
        || !db_cpu_ms.is_finite()
        || db_cpu_ms <= 0.0
    {
        return Err("invalid_action_resources");
    }
    let normalized_units = requested_facts.max(1) as f64;
    let throughput_rps = if requested_facts == 0 {
        1000.0 / elapsed_ms
    } else {
        requested_facts as f64 / (elapsed_ms / 1000.0)
    };
    let app_cpu_ms_per_attempt = app_cpu_ms / normalized_units;
    let db_cpu_ms_per_attempt = db_cpu_ms / normalized_units;
    if !throughput_rps.is_finite()
        || throughput_rps <= 0.0
        || !app_cpu_ms_per_attempt.is_finite()
        || app_cpu_ms_per_attempt <= 0.0
        || !db_cpu_ms_per_attempt.is_finite()
        || db_cpu_ms_per_attempt <= 0.0
    {
        return Err("invalid_derived_metric");
    }
    Ok(RunMetrics {
        latency_p50_ms: elapsed_ms,
        latency_p95_ms: elapsed_ms,
        latency_p99_ms: elapsed_ms,
        throughput_rps,
        app_cpu_ms_per_attempt,
        db_cpu_ms_per_attempt,
        error_rate: 0.0,
        action_fact_count: Some(requested_facts),
    })
}

fn run_metrics(run: &serde_json::Value) -> Result<RunMetrics, &'static str> {
    match run
        .pointer("/case/phase")
        .and_then(serde_json::Value::as_str)
    {
        Some("action") => return action_metrics(run),
        Some("steady") => {}
        _ => return Err("invalid_measurement_phase"),
    }
    let samples = run
        .get("request_samples_ms")
        .and_then(serde_json::Value::as_array)
        .ok_or("missing_request_samples")?;
    if samples.len() < 1000 {
        return Err("insufficient_request_samples");
    }
    if samples.len() != 1000 {
        return Err("request_sample_count_mismatch");
    }
    let mut latencies = samples
        .iter()
        .map(serde_json::Value::as_f64)
        .collect::<Option<Vec<_>>>()
        .ok_or("invalid_request_sample")?;
    if latencies
        .iter()
        .any(|latency| !latency.is_finite() || *latency <= 0.0)
    {
        return Err("invalid_request_sample");
    }
    let warmup_count = run
        .get("warmup_count")
        .and_then(serde_json::Value::as_u64)
        .ok_or("insufficient_warmup_requests")?;
    if warmup_count < 30 {
        return Err("insufficient_warmup_requests");
    }
    if warmup_count != 30 {
        return Err("warmup_count_mismatch");
    }
    let successful = run
        .get("successful_requests")
        .and_then(serde_json::Value::as_u64)
        .ok_or("missing_success_count")?;
    let errors = run
        .get("error_count")
        .and_then(serde_json::Value::as_u64)
        .ok_or("missing_error_count")?;
    let attempted = successful
        .checked_add(errors)
        .ok_or("invalid_request_counts")?;
    if attempted != samples.len() as u64 {
        return Err("request_count_mismatch");
    }
    if successful == 0 {
        return Err("no_successful_requests");
    }
    let wall_time_ms = run
        .get("wall_time_ms")
        .and_then(serde_json::Value::as_f64)
        .ok_or("missing_wall_time")?;
    let app_cpu_ms = run
        .get("app_cpu_ms")
        .and_then(serde_json::Value::as_f64)
        .ok_or("missing_app_cpu")?;
    let db_cpu_ms = run
        .get("db_cpu_ms")
        .and_then(serde_json::Value::as_f64)
        .ok_or("missing_db_cpu")?;
    if !wall_time_ms.is_finite()
        || wall_time_ms <= 0.0
        || !app_cpu_ms.is_finite()
        || app_cpu_ms <= 0.0
        || !db_cpu_ms.is_finite()
        || db_cpu_ms <= 0.0
    {
        return Err("invalid_resource_counters");
    }

    latencies.sort_by(f64::total_cmp);
    let percentile = |quantile: f64| {
        let rank = (quantile * latencies.len() as f64).ceil() as usize;
        latencies[rank.saturating_sub(1)]
    };
    let latency_p50_ms = percentile(0.50);
    let latency_p95_ms = percentile(0.95);
    let latency_p99_ms = percentile(0.99);
    let throughput_rps = successful as f64 / (wall_time_ms / 1000.0);
    let app_cpu_ms_per_attempt = app_cpu_ms / attempted as f64;
    let db_cpu_ms_per_attempt = db_cpu_ms / attempted as f64;
    let error_rate = errors as f64 / attempted as f64;
    if !throughput_rps.is_finite()
        || throughput_rps <= 0.0
        || !app_cpu_ms_per_attempt.is_finite()
        || app_cpu_ms_per_attempt <= 0.0
        || !db_cpu_ms_per_attempt.is_finite()
        || db_cpu_ms_per_attempt <= 0.0
        || !error_rate.is_finite()
    {
        return Err("invalid_derived_metric");
    }
    Ok(RunMetrics {
        latency_p50_ms,
        latency_p95_ms,
        latency_p99_ms,
        throughput_rps,
        app_cpu_ms_per_attempt,
        db_cpu_ms_per_attempt,
        error_rate,
        action_fact_count: None,
    })
}

fn paired_action_work_matches(left: &RunMetrics, right: &RunMetrics) -> bool {
    left.action_fact_count == right.action_fact_count
}

fn run_revision_matches(run: &serde_json::Value, expected_revision: &str) -> bool {
    run.get("revision").and_then(serde_json::Value::as_str) == Some(expected_revision)
}

fn run_matches_case(run: &serde_json::Value, expected_case: &serde_json::Value) -> bool {
    run.get("case") == Some(expected_case)
}

fn run_embedding_activity_matches_mode(
    run: &serde_json::Value,
    embedding_mode: &str,
) -> Result<(), &'static str> {
    let ann_queries = run
        .get("ann_queries")
        .and_then(serde_json::Value::as_u64)
        .ok_or("missing_embedding_activity_counts")?;
    let provider_requests = run
        .get("embedding_provider_requests")
        .and_then(serde_json::Value::as_u64)
        .ok_or("missing_embedding_activity_counts")?;
    if matches!(embedding_mode, "disabled_empty" | "disabled_stored")
        && (ann_queries != 0 || provider_requests != 0)
    {
        return Err("disabled_mode_has_embedding_activity");
    }
    Ok(())
}

pub fn compare_performance_pair_set(
    pair_set_dir: &std::path::Path,
) -> Result<ReleaseGateVerdict, Box<dyn std::error::Error + Send + Sync>> {
    let bytes = std::fs::read(pair_set_dir.join("pair-set.json"))?;
    let pair_set: serde_json::Value = serde_json::from_slice(&bytes)?;
    if pair_set
        .get("schema_version")
        .and_then(serde_json::Value::as_str)
        != Some("memory-mcp-storage-performance/v1")
    {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "unsupported_pair_set_schema".into(),
        ]));
    }
    if pair_set
        .get("baseline")
        .is_none_or(serde_json::Value::is_null)
    {
        return Ok(ReleaseGateVerdict::Invalid(vec!["missing_baseline".into()]));
    }
    let (Some(baseline_model), Some(candidate_model)) = (
        pair_set
            .pointer("/baseline/model")
            .and_then(serde_json::Value::as_str),
        pair_set
            .pointer("/candidate/model")
            .and_then(serde_json::Value::as_str),
    ) else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "missing_model_identity".into(),
        ]));
    };
    if baseline_model != candidate_model {
        return Ok(ReleaseGateVerdict::Invalid(vec!["model_mismatch".into()]));
    }
    let (Some(baseline_commit), Some(candidate_commit)) = (
        pair_set
            .pointer("/baseline/git_commit")
            .and_then(serde_json::Value::as_str),
        pair_set
            .pointer("/candidate/git_commit")
            .and_then(serde_json::Value::as_str),
    ) else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "missing_git_commit_identity".into(),
        ]));
    };
    if baseline_commit.is_empty()
        || candidate_commit.is_empty()
        || baseline_commit == candidate_commit
    {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "baseline_candidate_commits_not_distinct".into(),
        ]));
    }
    let (Some(baseline_provider), Some(candidate_provider)) = (
        pair_set
            .pointer("/baseline/provider")
            .and_then(serde_json::Value::as_str),
        pair_set
            .pointer("/candidate/provider")
            .and_then(serde_json::Value::as_str),
    ) else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "missing_provider_identity".into(),
        ]));
    };
    if baseline_provider != candidate_provider {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "provider_mismatch".into(),
        ]));
    }
    let (Some(baseline_revision), Some(candidate_revision)) = (
        pair_set
            .pointer("/baseline/model_revision")
            .and_then(serde_json::Value::as_str),
        pair_set
            .pointer("/candidate/model_revision")
            .and_then(serde_json::Value::as_str),
    ) else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "missing_model_revision".into(),
        ]));
    };
    if baseline_revision != candidate_revision {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "model_revision_mismatch".into(),
        ]));
    }
    let (Some(baseline_dimension), Some(candidate_dimension)) = (
        pair_set
            .pointer("/baseline/embedding_dimension")
            .and_then(serde_json::Value::as_u64),
        pair_set
            .pointer("/candidate/embedding_dimension")
            .and_then(serde_json::Value::as_u64),
    ) else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "missing_embedding_dimension".into(),
        ]));
    };
    if baseline_dimension == 0 || candidate_dimension == 0 {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "invalid_embedding_dimension".into(),
        ]));
    }
    if baseline_dimension != candidate_dimension {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "embedding_dimension_mismatch".into(),
        ]));
    }
    let (Some(baseline_host), Some(candidate_host)) = (
        pair_set
            .pointer("/baseline/host_fingerprint")
            .and_then(serde_json::Value::as_str),
        pair_set
            .pointer("/candidate/host_fingerprint")
            .and_then(serde_json::Value::as_str),
    ) else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "missing_host_fingerprint".into(),
        ]));
    };
    if baseline_host != candidate_host {
        return Ok(ReleaseGateVerdict::Invalid(vec!["host_mismatch".into()]));
    }
    let (Some(baseline_os), Some(candidate_os)) = (
        pair_set
            .pointer("/baseline/os")
            .and_then(serde_json::Value::as_str),
        pair_set
            .pointer("/candidate/os")
            .and_then(serde_json::Value::as_str),
    ) else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "missing_operating_system".into(),
        ]));
    };
    let (Some(baseline_arch), Some(candidate_arch)) = (
        pair_set
            .pointer("/baseline/arch")
            .and_then(serde_json::Value::as_str),
        pair_set
            .pointer("/candidate/arch")
            .and_then(serde_json::Value::as_str),
    ) else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "missing_architecture".into(),
        ]));
    };
    let (Some(baseline_runner_platform), Some(candidate_runner_platform)) = (
        pair_set
            .pointer("/baseline/runner_platform")
            .and_then(serde_json::Value::as_str),
        pair_set
            .pointer("/candidate/runner_platform")
            .and_then(serde_json::Value::as_str),
    ) else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "missing_runner_platform".into(),
        ]));
    };
    let (Some(baseline_emulated), Some(candidate_emulated)) = (
        pair_set
            .pointer("/baseline/emulated")
            .and_then(serde_json::Value::as_bool),
        pair_set
            .pointer("/candidate/emulated")
            .and_then(serde_json::Value::as_bool),
    ) else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "missing_emulation_identity".into(),
        ]));
    };
    if baseline_os != "linux"
        || candidate_os != "linux"
        || baseline_arch != candidate_arch
        || baseline_arch != "x86_64"
        || baseline_runner_platform != "linux/amd64"
        || candidate_runner_platform != "linux/amd64"
        || baseline_emulated
        || candidate_emulated
    {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "native_linux_amd64_required".into(),
        ]));
    }
    let (Some(baseline_ram), Some(candidate_ram)) = (
        pair_set
            .pointer("/baseline/ram_bytes")
            .and_then(serde_json::Value::as_u64),
        pair_set
            .pointer("/candidate/ram_bytes")
            .and_then(serde_json::Value::as_u64),
    ) else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "missing_ram_budget".into(),
        ]));
    };
    if baseline_ram != 3_221_225_472 || candidate_ram != baseline_ram {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "ram_budget_mismatch".into(),
        ]));
    }
    let (Some(baseline_swap), Some(candidate_swap)) = (
        pair_set
            .pointer("/baseline/swap_bytes")
            .and_then(serde_json::Value::as_u64),
        pair_set
            .pointer("/candidate/swap_bytes")
            .and_then(serde_json::Value::as_u64),
    ) else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "missing_swap_budget".into(),
        ]));
    };
    if baseline_swap != 0 || candidate_swap != baseline_swap {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "swap_must_be_disabled".into(),
        ]));
    }
    let (Some(baseline_cpu_quota), Some(candidate_cpu_quota)) = (
        pair_set
            .pointer("/baseline/cpu_quota_millis")
            .and_then(serde_json::Value::as_u64),
        pair_set
            .pointer("/candidate/cpu_quota_millis")
            .and_then(serde_json::Value::as_u64),
    ) else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "missing_cpu_quota".into(),
        ]));
    };
    if baseline_cpu_quota == 0 || candidate_cpu_quota != baseline_cpu_quota {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "cpu_quota_mismatch".into(),
        ]));
    }
    for (field, cause) in [
        ("kernel", "kernel_mismatch"),
        ("libc", "libc_mismatch"),
        ("runtime_base_image_digest", "runtime_base_image_mismatch"),
        ("provider_config_hash", "provider_config_mismatch"),
    ] {
        let (Some(baseline_value), Some(candidate_value)) = (
            pair_set
                .pointer(&format!("/baseline/{field}"))
                .and_then(serde_json::Value::as_str),
            pair_set
                .pointer(&format!("/candidate/{field}"))
                .and_then(serde_json::Value::as_str),
        ) else {
            return Ok(ReleaseGateVerdict::Invalid(vec![format!(
                "missing_{field}"
            )]));
        };
        if baseline_value != candidate_value {
            return Ok(ReleaseGateVerdict::Invalid(vec![cause.into()]));
        }
    }
    let (Some(baseline_cache_mode), Some(candidate_cache_mode)) = (
        pair_set
            .pointer("/baseline/cache_mode")
            .and_then(serde_json::Value::as_str),
        pair_set
            .pointer("/candidate/cache_mode")
            .and_then(serde_json::Value::as_str),
    ) else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "missing_cache_mode".into(),
        ]));
    };
    if baseline_cache_mode != candidate_cache_mode {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "cache_mode_mismatch".into(),
        ]));
    }
    let Some(case) = pair_set.get("case").filter(|case| case.is_object()) else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "performance_case_mismatch".into(),
        ]));
    };
    let (Some(case_id), Some(case_composition), Some(case_scenario), Some(case_workload)) = (
        case.get("id").and_then(serde_json::Value::as_str),
        case.get("composition").and_then(serde_json::Value::as_str),
        case.get("scenario").and_then(serde_json::Value::as_str),
        case.get("workload").and_then(serde_json::Value::as_str),
    ) else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "performance_case_mismatch".into(),
        ]));
    };
    let (Some(case_phase), Some(case_cache_mode), Some(case_embedding_mode)) = (
        case.get("phase").and_then(serde_json::Value::as_str),
        case.get("cache_mode").and_then(serde_json::Value::as_str),
        case.get("embedding_mode")
            .and_then(serde_json::Value::as_str),
    ) else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "performance_case_mismatch".into(),
        ]));
    };
    let (Some(case_dimension), Some(case_concurrency)) = (
        case.get("embedding_dimension")
            .and_then(serde_json::Value::as_u64),
        case.get("concurrency").and_then(serde_json::Value::as_u64),
    ) else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "performance_case_mismatch".into(),
        ]));
    };
    let (Some(baseline_embedding_mode), Some(candidate_embedding_mode)) = (
        pair_set
            .pointer("/baseline/embedding_mode")
            .and_then(serde_json::Value::as_str),
        pair_set
            .pointer("/candidate/embedding_mode")
            .and_then(serde_json::Value::as_str),
    ) else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "missing_embedding_mode".into(),
        ]));
    };
    if !matches!(
        case_embedding_mode,
        "remote" | "local_native" | "disabled_empty" | "disabled_stored"
    ) || case_id.trim().is_empty()
        || case_composition.trim().is_empty()
        || case_scenario.trim().is_empty()
        || case_workload.trim().is_empty()
        || case_phase.trim().is_empty()
        || case_cache_mode != baseline_cache_mode
        || case_cache_mode != candidate_cache_mode
        || case_embedding_mode != baseline_embedding_mode
        || case_embedding_mode != candidate_embedding_mode
        || case_dimension != baseline_dimension
        || case_dimension != candidate_dimension
        || case_concurrency == 0
    {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "performance_case_mismatch".into(),
        ]));
    }
    if matches!(case_embedding_mode, "disabled_empty" | "disabled_stored") && case_workload == "ann"
    {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "disabled_mode_has_ann_workload".into(),
        ]));
    }
    let exact_case_id_list = |key: &str| {
        pair_set
            .get(key)
            .and_then(serde_json::Value::as_array)
            .is_some_and(|ids| ids.len() == 1 && ids[0].as_str() == Some(case_id))
    };
    if !exact_case_id_list("expected_case_ids") || !exact_case_id_list("executed_case_ids") {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "performance_case_inventory_mismatch".into(),
        ]));
    }
    let Some(aa_pairs) = pair_set
        .get("aa_pairs")
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "insufficient_aa_pairs".into(),
        ]));
    };
    if aa_pairs.len() < 5 {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "insufficient_aa_pairs".into(),
        ]));
    }
    if aa_pairs.len() != 5 {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "aa_pair_count_mismatch".into(),
        ]));
    }
    let mut block_ids = BTreeSet::new();
    for pair in aa_pairs {
        let Some(block_id) = pair.get("block_id").and_then(serde_json::Value::as_str) else {
            return Ok(ReleaseGateVerdict::Invalid(vec![
                "missing_performance_block_id".into(),
            ]));
        };
        if block_id.is_empty() || !block_ids.insert(block_id) {
            return Ok(ReleaseGateVerdict::Invalid(vec![
                "duplicate_performance_block_id".into(),
            ]));
        }
        let (Some(baseline_value), Some(candidate_value)) =
            (pair.get("baseline"), pair.get("candidate"))
        else {
            return Ok(ReleaseGateVerdict::Invalid(vec!["invalid_aa_pair".into()]));
        };
        if !run_matches_case(baseline_value, case) || !run_matches_case(candidate_value, case) {
            return Ok(ReleaseGateVerdict::Invalid(vec![
                "performance_case_mismatch".into(),
            ]));
        }
        for run in [baseline_value, candidate_value] {
            if let Err(cause) = run_embedding_activity_matches_mode(run, case_embedding_mode) {
                return Ok(ReleaseGateVerdict::Invalid(vec![cause.into()]));
            }
        }
        let (Ok(baseline), Ok(candidate)) =
            (run_metrics(baseline_value), run_metrics(candidate_value))
        else {
            return Ok(ReleaseGateVerdict::Invalid(vec![
                "invalid_aa_measurement".into(),
            ]));
        };
        if !paired_action_work_matches(&baseline, &candidate) {
            return Ok(ReleaseGateVerdict::Invalid(vec![
                "aa_action_work_mismatch".into(),
            ]));
        }
        if !run_revision_matches(baseline_value, baseline_commit)
            || !run_revision_matches(candidate_value, baseline_commit)
        {
            return Ok(ReleaseGateVerdict::Invalid(vec![
                "aa_run_revision_mismatch".into(),
            ]));
        }
        if baseline.error_rate > 0.0 || candidate.error_rate > 0.0 {
            return Ok(ReleaseGateVerdict::Invalid(vec![
                "aa_error_rate_nonzero".into(),
            ]));
        }
        let noise = [
            candidate.latency_p50_ms / baseline.latency_p50_ms,
            candidate.latency_p95_ms / baseline.latency_p95_ms,
            candidate.latency_p99_ms / baseline.latency_p99_ms,
            candidate.throughput_rps / baseline.throughput_rps,
            candidate.app_cpu_ms_per_attempt / baseline.app_cpu_ms_per_attempt,
            candidate.db_cpu_ms_per_attempt / baseline.db_cpu_ms_per_attempt,
        ];
        if noise.iter().any(|ratio| (ratio - 1.0).abs() > 0.03) {
            return Ok(ReleaseGateVerdict::Invalid(vec![
                "aa_noise_over_3_percent".into(),
            ]));
        }
    }
    let Some(ab_pairs) = pair_set
        .get("ab_pairs")
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "insufficient_ab_pairs".into(),
        ]));
    };
    if ab_pairs.len() < 5 {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "insufficient_ab_pairs".into(),
        ]));
    }
    if ab_pairs.len() != 5 {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "ab_pair_count_mismatch".into(),
        ]));
    }
    let expected_order = ["AB", "BA", "AB", "BA", "AB"];
    if ab_pairs.iter().zip(expected_order).any(|(pair, expected)| {
        pair.get("order").and_then(serde_json::Value::as_str) != Some(expected)
    }) {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "invalid_ab_order_schedule".into(),
        ]));
    }
    for pair in ab_pairs {
        let Some(block_id) = pair.get("block_id").and_then(serde_json::Value::as_str) else {
            return Ok(ReleaseGateVerdict::Invalid(vec![
                "missing_performance_block_id".into(),
            ]));
        };
        if block_id.is_empty() || !block_ids.insert(block_id) {
            return Ok(ReleaseGateVerdict::Invalid(vec![
                "duplicate_performance_block_id".into(),
            ]));
        }
        for side in ["baseline", "candidate"] {
            let Some(run) = pair.get(side) else {
                return Ok(ReleaseGateVerdict::Invalid(vec![
                    "missing_performance_run".into(),
                ]));
            };
            if !run_matches_case(run, case) {
                return Ok(ReleaseGateVerdict::Invalid(vec![
                    "performance_case_mismatch".into(),
                ]));
            }
            if let Err(cause) = run_embedding_activity_matches_mode(run, case_embedding_mode) {
                return Ok(ReleaseGateVerdict::Invalid(vec![cause.into()]));
            }
            if let Err(cause) = run_metrics(run) {
                return Ok(ReleaseGateVerdict::Invalid(vec![cause.into()]));
            }
            let expected_revision = if side == "baseline" {
                baseline_commit
            } else {
                candidate_commit
            };
            if !run_revision_matches(run, expected_revision) {
                return Ok(ReleaseGateVerdict::Invalid(vec![format!(
                    "{side}_run_revision_mismatch"
                )]));
            }
        }
        let baseline = run_metrics(&pair["baseline"])
            .map_err(|cause| format!("invalid baseline performance run: {cause}"))?;
        let candidate = run_metrics(&pair["candidate"])
            .map_err(|cause| format!("invalid candidate performance run: {cause}"))?;
        if !paired_action_work_matches(&baseline, &candidate) {
            return Ok(ReleaseGateVerdict::Invalid(vec![
                "ab_action_work_mismatch".into(),
            ]));
        }
    }
    let mut ratios = Vec::with_capacity(ab_pairs.len());
    for pair in ab_pairs {
        let baseline = run_metrics(&pair["baseline"])
            .map_err(|cause| format!("invalid baseline performance run: {cause}"))?;
        let candidate = run_metrics(&pair["candidate"])
            .map_err(|cause| format!("invalid candidate performance run: {cause}"))?;
        ratios.push(PairRatios {
            latency_p50: candidate.latency_p50_ms / baseline.latency_p50_ms,
            latency_p95: candidate.latency_p95_ms / baseline.latency_p95_ms,
            latency_p99: candidate.latency_p99_ms / baseline.latency_p99_ms,
            throughput: candidate.throughput_rps / baseline.throughput_rps,
            app_cpu: candidate.app_cpu_ms_per_attempt / baseline.app_cpu_ms_per_attempt,
            db_cpu: candidate.db_cpu_ms_per_attempt / baseline.db_cpu_ms_per_attempt,
        });
        if candidate.error_rate > baseline.error_rate {
            return Ok(ReleaseGateVerdict::Regressed(vec![
                "error_rate_regression".into(),
            ]));
        }
        if baseline.error_rate > 0.0 {
            return Ok(ReleaseGateVerdict::Invalid(vec![
                "baseline_error_rate_nonzero".into(),
            ]));
        }
    }
    let latency_p50_ratios = ratios
        .iter()
        .map(|ratio| ratio.latency_p50)
        .collect::<Vec<_>>();
    if median(&latency_p50_ratios) > 1.0 {
        return Ok(ReleaseGateVerdict::Regressed(vec![
            "latency_p50_ratio".into(),
        ]));
    }
    let latency_p95_ratios = ratios
        .iter()
        .map(|ratio| ratio.latency_p95)
        .collect::<Vec<_>>();
    if median(&latency_p95_ratios) > 1.0 {
        return Ok(ReleaseGateVerdict::Regressed(vec![
            "latency_p95_ratio".into(),
        ]));
    }
    let latency_p99_ratios = ratios
        .iter()
        .map(|ratio| ratio.latency_p99)
        .collect::<Vec<_>>();
    if median(&latency_p99_ratios) > 1.0 {
        return Ok(ReleaseGateVerdict::Regressed(vec![
            "latency_p99_ratio".into(),
        ]));
    }
    let throughput_ratios = ratios
        .iter()
        .map(|ratio| ratio.throughput)
        .collect::<Vec<_>>();
    if median(&throughput_ratios) < 1.0 {
        return Ok(ReleaseGateVerdict::Regressed(vec![
            "throughput_ratio".into(),
        ]));
    }
    let app_cpu_ratios = ratios.iter().map(|ratio| ratio.app_cpu).collect::<Vec<_>>();
    if median(&app_cpu_ratios) > 1.0 {
        return Ok(ReleaseGateVerdict::Regressed(vec![
            "app_cpu_ms_per_attempt_ratio".into(),
        ]));
    }
    let db_cpu_ratios = ratios.iter().map(|ratio| ratio.db_cpu).collect::<Vec<_>>();
    if median(&db_cpu_ratios) > 1.0 {
        return Ok(ReleaseGateVerdict::Regressed(vec![
            "db_cpu_ms_per_attempt_ratio".into(),
        ]));
    }
    let (_, latency_p50_upper) = bootstrap_interval(&latency_p50_ratios);
    let (_, latency_p95_upper) = bootstrap_interval(&latency_p95_ratios);
    let (_, latency_p99_upper) = bootstrap_interval(&latency_p99_ratios);
    let (throughput_lower, _) = bootstrap_interval(&throughput_ratios);
    let (_, app_cpu_upper) = bootstrap_interval(&app_cpu_ratios);
    let (_, db_cpu_upper) = bootstrap_interval(&db_cpu_ratios);
    let mut inconclusive = Vec::new();
    if latency_p50_upper > 1.03 {
        inconclusive.push("latency_p50_uncertainty_wide".into());
    }
    if latency_p95_upper > 1.03 {
        inconclusive.push("latency_p95_uncertainty_wide".into());
    }
    if latency_p99_upper > 1.03 {
        inconclusive.push("latency_p99_uncertainty_wide".into());
    }
    if throughput_lower < 0.97 {
        inconclusive.push("throughput_uncertainty_wide".into());
    }
    if app_cpu_upper > 1.03 {
        inconclusive.push("app_cpu_uncertainty_wide".into());
    }
    if db_cpu_upper > 1.03 {
        inconclusive.push("db_cpu_uncertainty_wide".into());
    }
    if !inconclusive.is_empty() {
        return Ok(ReleaseGateVerdict::Invalid(inconclusive));
    }
    Ok(ReleaseGateVerdict::Passed)
}

pub fn compare_quality_artifacts(
    baseline: &std::path::Path,
    candidate: &std::path::Path,
    release_profile: &std::path::Path,
) -> Result<ReleaseGateVerdict, Box<dyn std::error::Error + Send + Sync>> {
    let profile_bytes = std::fs::read(release_profile)?;
    let profile_text = std::str::from_utf8(&profile_bytes)?;
    let profile = match ProfileManifest::parse(profile_text) {
        Ok(profile) => profile,
        Err(_) => {
            return Ok(ReleaseGateVerdict::Invalid(vec![
                "invalid_release_profile".into(),
            ]));
        }
    };
    if profile.profile != EvalProfile::Release {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "profile_not_release".into(),
        ]));
    }
    let Some(suite_decl) = profile
        .suites
        .iter()
        .find(|suite| suite.id == "retrieval-quality")
    else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "missing_retrieval_quality_suite".into(),
        ]));
    };
    let Some(expected_count) = suite_decl
        .expected_coverage
        .as_ref()
        .map(|coverage| coverage.exact_cases)
    else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "missing_retrieval_quality_coverage".into(),
        ]));
    };
    let Some(corpus_root) = suite_decl.corpus_root.as_deref() else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "missing_retrieval_quality_corpus".into(),
        ]));
    };
    let Some(repository_root) = release_profile
        .parent()
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::parent)
    else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "invalid_profile_path".into(),
        ]));
    };
    let corpus_bytes = std::fs::read(
        repository_root
            .join(corpus_root)
            .join("retrieval-quality.json"),
    )?;
    let corpus: serde_json::Value = serde_json::from_slice(&corpus_bytes)?;
    let Some(cases) = corpus.get("cases").and_then(serde_json::Value::as_array) else {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "invalid_retrieval_quality_corpus".into(),
        ]));
    };
    let mut case_metadata = BTreeMap::new();
    for case in cases {
        let (Some(case_id), Some(split), Some(label_trust)) = (
            case.get("id").and_then(serde_json::Value::as_str),
            case.get("split"),
            case.get("label_trust"),
        ) else {
            return Ok(ReleaseGateVerdict::Invalid(vec![
                "invalid_retrieval_quality_corpus".into(),
            ]));
        };
        case_metadata.insert(case_id.to_owned(), (split.clone(), label_trust.clone()));
    }
    let expected_ids = cases
        .iter()
        .filter_map(|case| {
            case.get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .collect::<BTreeSet<_>>();
    if cases.len() != expected_count || expected_ids.len() != cases.len() {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "profile_corpus_coverage_mismatch".into(),
        ]));
    }
    let required_group_ids = cases
        .iter()
        .filter(|case| {
            case.get("required_evidence_groups")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|groups| !groups.is_empty())
        })
        .filter_map(|case| {
            case.get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .collect::<BTreeSet<_>>();
    let profile_digest = hex::encode(sha2::Sha256::digest(&profile_bytes));
    let baseline_bytes = std::fs::read(baseline)?;
    let candidate_bytes = std::fs::read(candidate)?;
    let baseline_artifact: RunArtifact = serde_json::from_slice(&baseline_bytes)?;
    let candidate_artifact: RunArtifact = serde_json::from_slice(&candidate_bytes)?;

    for (role, artifact) in [
        ("baseline", &baseline_artifact),
        ("candidate", &candidate_artifact),
    ] {
        if artifact.schema_version != EVAL_ARTIFACT_SCHEMA_V2 {
            return Ok(ReleaseGateVerdict::Invalid(vec![format!(
                "{role}_artifact_schema_mismatch"
            )]));
        }
        if artifact.profile != EvalProfile::Release
            || artifact.fingerprint.profile_digest != profile_digest
            || artifact.fingerprint.build_profile != "release"
        {
            return Ok(ReleaseGateVerdict::Invalid(vec![format!(
                "{role}_profile_identity_mismatch"
            )]));
        }
        if let Err(error) = artifact.validate() {
            let error = error.to_string();
            let cause = if error.contains("missing outcome for expected case") {
                format!("missing_{role}_case_coverage")
            } else {
                format!("invalid_{role}_artifact")
            };
            return Ok(ReleaseGateVerdict::Invalid(vec![cause]));
        }
        let profile_suite_ids = profile
            .suites
            .iter()
            .map(|suite| suite.id.as_str())
            .collect::<BTreeSet<_>>();
        let expected_suite_ids = artifact
            .expected_cases
            .iter()
            .map(|case| case.suite_id.as_str())
            .collect::<BTreeSet<_>>();
        let outcome_suite_ids = artifact
            .outcomes
            .iter()
            .map(|outcome| outcome.suite_id())
            .collect::<BTreeSet<_>>();
        if expected_suite_ids != profile_suite_ids || outcome_suite_ids != profile_suite_ids {
            return Ok(ReleaseGateVerdict::Invalid(vec![format!(
                "{role}_profile_suite_coverage_mismatch"
            )]));
        }
        for suite in &profile.suites {
            let Some(exact_cases) = suite
                .expected_coverage
                .as_ref()
                .map(|coverage| coverage.exact_cases)
            else {
                return Ok(ReleaseGateVerdict::Invalid(vec![format!(
                    "{role}_missing_suite_coverage:{}",
                    suite.id
                )]));
            };
            let expected_case_ids = artifact
                .expected_cases
                .iter()
                .filter(|case| case.suite_id.as_str() == suite.id)
                .map(|case| case.case_id.as_str())
                .collect::<BTreeSet<_>>();
            let outcome_case_ids = artifact
                .outcomes
                .iter()
                .filter(|outcome| outcome.suite_id() == suite.id)
                .map(|outcome| outcome.case_id().as_str())
                .collect::<BTreeSet<_>>();
            let summaries = artifact
                .suite_summaries
                .iter()
                .filter(|summary| summary.suite_id == suite.id)
                .collect::<Vec<_>>();
            let [summary] = summaries.as_slice() else {
                return Ok(ReleaseGateVerdict::Invalid(vec![format!(
                    "{role}_suite_coverage_mismatch:{}",
                    suite.id
                )]));
            };
            if expected_case_ids.len() != exact_cases
                || outcome_case_ids != expected_case_ids
                || summary.total != exact_cases
                || summary.passed != exact_cases
                || summary.quality_failed != 0
                || summary.invalid != 0
            {
                return Ok(ReleaseGateVerdict::Invalid(vec![format!(
                    "{role}_suite_coverage_mismatch:{}",
                    suite.id
                )]));
            }
        }
        let actual_expected = artifact
            .expected_cases
            .iter()
            .filter(|key| key.suite_id.as_str() == "retrieval-quality")
            .map(|key| key.case_id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        let actual_outcomes = artifact
            .outcomes
            .iter()
            .filter(|outcome| outcome.suite_id() == "retrieval-quality")
            .map(|outcome| outcome.case_id().as_str().to_owned())
            .collect::<BTreeSet<_>>();
        if actual_expected != expected_ids || actual_outcomes != expected_ids {
            return Ok(ReleaseGateVerdict::Invalid(vec![format!(
                "{role}_case_coverage_mismatch"
            )]));
        }
        for outcome in artifact
            .outcomes
            .iter()
            .filter(|outcome| outcome.suite_id() == "retrieval-quality")
        {
            let case_id = outcome.case_id().as_str();
            let Some((expected_split, expected_label_trust)) = case_metadata.get(case_id) else {
                return Ok(ReleaseGateVerdict::Invalid(vec![format!(
                    "{role}_case_coverage_mismatch"
                )]));
            };
            let actual_split = serde_json::to_value(outcome.split)?;
            let actual_label_trust = serde_json::to_value(outcome.label_trust)?;
            if outcome.mode != EvalMode::EndToEnd
                || actual_split != *expected_split
                || actual_label_trust != *expected_label_trust
            {
                return Ok(ReleaseGateVerdict::Invalid(vec![format!(
                    "{role}_case_metadata_mismatch:{case_id}"
                )]));
            }
            if outcome.status != CaseStatus::Passed {
                if role == "baseline" {
                    return Ok(ReleaseGateVerdict::Invalid(vec![
                        "baseline_case_not_passed".into(),
                    ]));
                }
                return Ok(ReleaseGateVerdict::Regressed(vec![
                    "case_quality_regression".into(),
                ]));
            }
        }
        if artifact.verdict != RunVerdict::Passed
            || !matches!(artifact.budget_status, Some(GateStatus::Passed))
            || !artifact.issues.is_empty()
        {
            return Ok(ReleaseGateVerdict::Invalid(vec![format!(
                "{role}_run_not_passed"
            )]));
        }
        let Some(identity) = artifact
            .fingerprint
            .suite_fingerprints
            .get("retrieval-quality")
        else {
            return Ok(ReleaseGateVerdict::Invalid(vec![format!(
                "{role}_missing_retrieval_identity"
            )]));
        };
        let provider = identity.get("provider").map(String::as_str).unwrap_or("");
        let model = identity.get("model").map(String::as_str).unwrap_or("");
        let provider_status = identity
            .get("provider_status")
            .map(String::as_str)
            .unwrap_or("");
        let dimension = identity
            .get("embedding_dimension")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or_default();
        if provider.is_empty()
            || provider == "disabled"
            || model.is_empty()
            || provider_status != "ready"
            || dimension == 0
            || identity.get("corpus_manifest_sha256").map(String::as_str)
                != Some(hex::encode(sha2::Sha256::digest(&corpus_bytes)).as_str())
            || identity.get("source_bundle_sha256").map(String::as_str)
                != corpus
                    .get("source_sha256")
                    .and_then(serde_json::Value::as_str)
        {
            return Ok(ReleaseGateVerdict::Invalid(vec![format!(
                "{role}_retrieval_identity_invalid"
            )]));
        }
        for case_id in &required_group_ids {
            let outcome = artifact.outcomes.iter().find(|outcome| {
                outcome.suite_id() == "retrieval-quality" && outcome.case_id().as_str() == case_id
            });
            let Some(outcome) = outcome else {
                return Ok(ReleaseGateVerdict::Invalid(vec![format!(
                    "{role}_missing_required_case:{case_id}"
                )]));
            };
            let Some(recall) = outcome.metrics.get("required_group_recall") else {
                return Ok(ReleaseGateVerdict::Invalid(vec![format!(
                    "{role}_missing_required_group_recall:{case_id}"
                )]));
            };
            if !recall.is_finite() || *recall < 0.0 || *recall > 1.0 {
                return Ok(ReleaseGateVerdict::Invalid(vec![format!(
                    "{role}_invalid_required_group_recall:{case_id}"
                )]));
            }
        }
    }

    let baseline_case_inventory = baseline_artifact
        .expected_cases
        .iter()
        .map(|case| {
            (
                case.suite_id.as_str().to_owned(),
                case.case_id.as_str().to_owned(),
            )
        })
        .collect::<BTreeSet<_>>();
    let candidate_case_inventory = candidate_artifact
        .expected_cases
        .iter()
        .map(|case| {
            (
                case.suite_id.as_str().to_owned(),
                case.case_id.as_str().to_owned(),
            )
        })
        .collect::<BTreeSet<_>>();
    if baseline_case_inventory != candidate_case_inventory {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "incompatible_profile_case_inventory".into(),
        ]));
    }
    let quality_gates = profile.gates.iter().collect::<Vec<_>>();
    if quality_gates.is_empty() {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "missing_release_quality_gates".into(),
        ]));
    }
    for gate in quality_gates {
        let Some(floor) = gate.hard_floor else {
            return Ok(ReleaseGateVerdict::Invalid(vec![format!(
                "quality_gate_direction_missing:{}:{}",
                gate.target.suite_id, gate.target.metric
            )]));
        };
        let suite_id = gate.target.suite_id.as_str();
        let metric = gate.target.metric.as_str();
        let base_summary = baseline_artifact
            .suite_summaries
            .iter()
            .find(|summary| summary.suite_id == suite_id)
            .expect("profile suite summary validated above");
        let candidate_summary = candidate_artifact
            .suite_summaries
            .iter()
            .find(|summary| summary.suite_id == suite_id)
            .expect("profile suite summary validated above");
        let (Some(baseline_value), Some(candidate_value)) = (
            base_summary.metrics.get(metric),
            candidate_summary.metrics.get(metric),
        ) else {
            return Ok(ReleaseGateVerdict::Invalid(vec![format!(
                "missing_quality_metric:{suite_id}:{metric}"
            )]));
        };
        if !baseline_value.is_finite() || !candidate_value.is_finite() {
            return Ok(ReleaseGateVerdict::Invalid(vec![format!(
                "nonfinite_quality_metric:{suite_id}:{metric}"
            )]));
        }
        if baseline_value + f64::EPSILON < floor {
            return Ok(ReleaseGateVerdict::Invalid(vec![format!(
                "baseline_below_quality_floor:{suite_id}:{metric}"
            )]));
        }
        // Release quality gates are expressed as hard floors, so these metrics
        // are higher-is-better and zero regression means candidate >= baseline.
        if candidate_value + f64::EPSILON < floor
            || candidate_value + f64::EPSILON < *baseline_value
        {
            let label = if suite_id == "retrieval-quality" {
                metric.to_owned()
            } else {
                format!("{suite_id}:{metric}")
            };
            return Ok(ReleaseGateVerdict::Regressed(vec![format!(
                "quality_metric_regression:{label}"
            )]));
        }
    }
    let baseline_identity = &baseline_artifact.fingerprint.suite_fingerprints["retrieval-quality"];
    let candidate_identity =
        &candidate_artifact.fingerprint.suite_fingerprints["retrieval-quality"];
    if baseline_identity != candidate_identity {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "incompatible_retrieval_identity".into(),
        ]));
    }
    let comparable_fingerprints = |artifact: &RunArtifact| {
        let fingerprint = &artifact.fingerprint;
        (
            fingerprint.rust_version.clone(),
            fingerprint.os_arch.clone(),
            fingerprint.package_version.clone(),
            fingerprint.build_profile.clone(),
            fingerprint.enabled_features.clone(),
            fingerprint.provider.clone(),
            fingerprint.model.clone(),
            fingerprint.device.clone(),
            fingerprint.configuration_hash.clone(),
            fingerprint.evaluator_versions.clone(),
        )
    };
    if comparable_fingerprints(&baseline_artifact) != comparable_fingerprints(&candidate_artifact) {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "incompatible_runtime_fingerprint".into(),
        ]));
    }
    if baseline_artifact.fingerprint.suite_fingerprints
        != candidate_artifact.fingerprint.suite_fingerprints
    {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "incompatible_suite_identities".into(),
        ]));
    }
    if baseline_artifact.fingerprint.git_commit.is_none()
        || candidate_artifact.fingerprint.git_commit.is_none()
        || baseline_artifact.fingerprint.git_commit == candidate_artifact.fingerprint.git_commit
    {
        return Ok(ReleaseGateVerdict::Invalid(vec![
            "baseline_candidate_commits_not_distinct".into(),
        ]));
    }
    for case_id in &required_group_ids {
        let baseline_outcome = baseline_artifact
            .outcomes
            .iter()
            .find(|outcome| {
                outcome.suite_id() == "retrieval-quality" && outcome.case_id().as_str() == case_id
            })
            .expect("validated expected case");
        let candidate_outcome = candidate_artifact
            .outcomes
            .iter()
            .find(|outcome| {
                outcome.suite_id() == "retrieval-quality" && outcome.case_id().as_str() == case_id
            })
            .expect("validated expected case");
        let baseline_recall = baseline_outcome.metrics["required_group_recall"];
        let candidate_recall = candidate_outcome.metrics["required_group_recall"];
        if candidate_outcome.status != CaseStatus::Passed
            || candidate_recall + f64::EPSILON < baseline_recall
            || candidate_recall + f64::EPSILON < 1.0
        {
            return Ok(ReleaseGateVerdict::Regressed(vec![format!(
                "required_fact_lost:{case_id}"
            )]));
        }
    }
    if candidate_artifact
        .outcomes
        .iter()
        .filter(|outcome| outcome.suite_id() == "retrieval-quality")
        .any(|outcome| outcome.status != CaseStatus::Passed)
    {
        return Ok(ReleaseGateVerdict::Regressed(vec![
            "case_quality_regression".into(),
        ]));
    }
    Ok(ReleaseGateVerdict::Passed)
}
