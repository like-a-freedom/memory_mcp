//! Full-process memory evidence validation and workload fixtures.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

const EVIDENCE_SCHEMA_VERSION: u32 = 4;
const REQUIRED_GLIBC_VERSION: &str = "2.41-12+deb13u4";
const REQUIRED_CACHE_MISSES: usize = 100;
const REQUIRED_REWRITE_FACTS: usize = 205;
const REQUIRED_REWRITE_BATCHES: [usize; 3] = [100, 100, 5];
const EXPECTED_DECAY_FACT_ID: &str = "fact:storage_memory_decay_old";
const EXPECTED_DECAY_CLAIM_IDS: [&str; 2] = [
    "claim:storage_memory_decay_subject",
    "claim:storage_memory_decay_object",
];
const REQUIRED_CONCURRENT_REQUESTS_PER_TENANT: usize = 4;
const REMOTE_PEAK_RSS_LIMIT_BYTES: u64 = 268_435_456;
const REMOTE_STEADY_LIMIT_BYTES: u64 = 134_217_728;
const REMOTE_CGROUP_PEAK_LIMIT_BYTES: u64 = 335_544_320;
const PLATEAU_GROWTH_LIMIT_BYTES: i64 = 8_388_608;
const ALLOWED_INJECTED_FAULTS: [&str; 3] = [
    "provider_unavailable",
    "request_cancelled",
    "storage_failure",
];
const REMOTE_ERROR_RELEASE_FAULTS: [&str; 3] = [
    "provider_unavailable",
    "request_cancelled",
    "storage_failure",
];
const DISABLED_ERROR_RELEASE_FAULTS: [&str; 2] = ["request_cancelled", "storage_failure"];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpMemoryReport {
    pub peak_rss_bytes: u64,
    pub steady_rss_plus_swap_bytes: Option<u64>,
    pub max_swap_bytes: u64,
    pub cgroup_peak_bytes: u64,
    pub oom_delta: u64,
    pub oom_kill_delta: u64,
    pub plateau_growth_bytes: Option<i64>,
    pub unexpected_failed_requests: usize,
    pub expected_fault_outcomes: usize,
    pub db_read_delta: u64,
    pub samples: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScenario {
    ColdDecay,
    RetrievalMisses,
    ConcurrentMaintenance,
    ErrorRelease,
    Backfill,
    Reembed,
    InterruptedResume,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalWorkload {
    LexicalFallback,
    Temporal,
    WakeUp,
    Experience,
    Ann,
    AssembleContext,
    Dashboard,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessComposition {
    Http,
    Stdio,
    Cli,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EmbeddingFixtureMode {
    Remote { dimension: usize },
    DisabledEmpty { dimension: usize },
    DisabledStored { dimension: usize },
    LocalNative { dimension: usize },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EmbeddingTransition {
    Backfill,
    Dimension { from: usize, to: usize },
    Signature { dimension: usize },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryPhase {
    Steady,
    Before,
    InProgress,
    After,
    AfterResume,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryCase {
    pub id: String,
    pub composition: ProcessComposition,
    pub mode: EmbeddingFixtureMode,
    pub scenario: MemoryScenario,
    pub transition: Option<EmbeddingTransition>,
    pub phase: MemoryPhase,
    pub workload: Option<RetrievalWorkload>,
    pub repetition: u8,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Evidence {
    schema_version: u32,
    case: MemoryCase,
    image: ImageEvidence,
    process: ProcessEvidence,
    sample_count: usize,
    samples_file: String,
    expected_case_ids: Vec<String>,
    executed_case_ids: Vec<String>,
    requests: Vec<RequestEvidence>,
    #[serde(default)]
    idle_checkpoints: Vec<IdleCheckpointEvidence>,
    expected_faults: Vec<String>,
    provider: ProviderEvidence,
    db_reads: DbReadEvidence,
    decay: DecayEvidence,
    #[serde(default)]
    maintenance: Option<MaintenanceEvidence>,
    #[serde(default)]
    rewrite: Option<RewriteEvidence>,
    cgroup: CgroupEvidence,
    #[serde(default)]
    baseline: Option<BaselineEvidence>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SuiteEvidence {
    schema_version: u32,
    expected_case_ids: Vec<String>,
    executed_case_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BaselineEvidence {
    image_oci_digest: String,
    peak_rss_bytes: u64,
    steady_rss_plus_swap_bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageEvidence {
    platform: String,
    runner_platform: String,
    emulated: bool,
    image_id: String,
    oci_digest: String,
    source_bundle_digest: String,
    expected_oci_digest: String,
    expected_source_bundle_digest: String,
    glibc_version: String,
    app_features: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessEvidence {
    binary_name: String,
    transport: String,
    pid: u32,
    start_time_ticks: u64,
    observed_pid: u32,
    observed_start_time_ticks: u64,
    sampler_exit_code: i32,
    sampler_started_before_app: bool,
    cold_start_observed: bool,
    steady_window_ms: u64,
    #[serde(default)]
    measurement_finished_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RequestOutcome {
    Success,
    ExpectedFault,
    ApplicationError,
    TransportError,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RequestPhase {
    Warmup,
    Measured,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestEvidence {
    request_id: String,
    outcome: RequestOutcome,
    category: Option<String>,
    #[serde(default)]
    tenant_id: Option<String>,
    #[serde(default)]
    started_ms: Option<u64>,
    #[serde(default)]
    finished_ms: Option<u64>,
    #[serde(default)]
    phase: Option<RequestPhase>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IdleCheckpointEvidence {
    request_id: String,
    elapsed_ms: u64,
    inflight_requests: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MaintenanceEvidence {
    started_ms: u64,
    finished_ms: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderEvidence {
    embedding_enabled: bool,
    provider_id: Option<String>,
    model_revision: Option<String>,
    configured_dimension: Option<usize>,
    provider_dimension: Option<usize>,
    schema_dimension: usize,
    configured_signature: Option<String>,
    persisted_embeddings: Vec<PersistedEmbeddingEvidence>,
    observed_signatures: Vec<String>,
    resolved_dimension: Option<usize>,
    stored_dimensions: Vec<usize>,
    observed_dimensions: Vec<usize>,
    index_dimension: Option<usize>,
    requests: usize,
    ann_queries: usize,
    trap_listener_started: bool,
    trap_requests: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedEmbeddingEvidence {
    dimension: usize,
    signature: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DbReadEvidence {
    events: Vec<DbReadEvent>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DbReadEvent {
    request_id: String,
    foreground: bool,
    statement_kind: String,
    cache_hit: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecayEvidence {
    completion_signals: Vec<String>,
    #[serde(default)]
    invalidated_fact_ids: Vec<String>,
    #[serde(default)]
    closed_claim_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RewriteEvidence {
    requested_facts: usize,
    completed_facts: usize,
    batch_sizes: Vec<usize>,
    source_fact_ids: Vec<String>,
    final_fact_ids: Vec<String>,
    last_completed_fact_id: Option<String>,
    interrupted_after_facts: Option<usize>,
    resume_after_fact_id: Option<String>,
    restarted: bool,
    failed_fact_ids: Vec<String>,
    state: RewriteState,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RewriteState {
    Ready,
    Running,
    Completed,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CgroupEvidence {
    memory_peak_bytes: Option<u64>,
    memory_events: Option<CgroupEvents>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CgroupEvents {
    oom_delta: u64,
    oom_kill_delta: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemorySample {
    binary_name: String,
    process_alive: bool,
    elapsed_ms: u64,
    rss_kib: Option<u64>,
    swap_kib: Option<u64>,
    hwm_kib: Option<u64>,
    cgroup_current_bytes: Option<u64>,
    cgroup_swap_bytes: Option<u64>,
    cgroup_memory_peak_bytes: Option<u64>,
    cgroup_oom: Option<u64>,
    cgroup_oom_kill: Option<u64>,
}

pub fn validate_http_memory_evidence(output_dir: &Path) -> Result<HttpMemoryReport, String> {
    validate_http_memory_evidence_for_case(output_dir, None)
}

fn validate_http_memory_evidence_for_case(
    output_dir: &Path,
    expected_case_id: Option<&str>,
) -> Result<HttpMemoryReport, String> {
    let run_path = output_dir.join("run.json");
    let run_bytes = fs::read(&run_path)
        .map_err(|error| invalid("schema", format!("cannot read run.json: {error}")))?;
    let evidence: Evidence = serde_json::from_slice(&run_bytes)
        .map_err(|error| invalid("schema", format!("invalid run.json: {error}")))?;

    if evidence.schema_version != EVIDENCE_SCHEMA_VERSION {
        return Err(invalid(
            "schema",
            format!("unsupported schema version {}", evidence.schema_version),
        ));
    }
    if expected_case_id.is_some_and(|expected| evidence.case.id != expected) {
        return Err(invalid(
            "manifest",
            format!(
                "case artifact directory does not match embedded case ID {:?}",
                evidence.case.id
            ),
        ));
    }
    validate_image(&evidence.image, &evidence.case.composition)?;
    validate_process(&evidence.process, &evidence.case.composition)?;
    validate_manifest(&evidence)?;
    validate_mode(&evidence.case, &evidence.provider)?;
    validate_case_shape(&evidence.case)?;
    let (samples, sample_path) = read_samples(output_dir, &evidence)?;
    let application_samples =
        validate_sample_coverage(&samples, &evidence.process, &evidence.case.composition)?;
    validate_cgroup(&evidence.cgroup, &samples)?;
    let expected_fault_outcomes = validate_requests(&evidence)?;
    let db_read_delta = validate_db_reads(&evidence)?;
    validate_scenario_evidence(&evidence)?;
    let plateau_growth_bytes = validate_plateau_growth(&evidence, &application_samples)?;

    let max_elapsed = application_samples
        .last()
        .map(|sample| sample.elapsed_ms)
        .ok_or_else(|| invalid("samples", "sample file is empty"))?;
    let steady_peak = if evidence.case.composition == ProcessComposition::Cli {
        None
    } else {
        let steady_start = max_elapsed.saturating_sub(evidence.process.steady_window_ms);
        let steady_samples = application_samples
            .iter()
            .filter(|sample| sample.elapsed_ms >= steady_start)
            .collect::<Vec<_>>();
        if steady_samples.is_empty() {
            return Err(invalid("samples", "steady window has no samples"));
        }
        let steady_peak = steady_samples
            .iter()
            .filter_map(|sample| sample.rss_kib.zip(sample.swap_kib))
            .map(|(rss, swap)| bytes(rss.saturating_add(swap)))
            .max()
            .unwrap_or_default();
        Some(steady_peak)
    };
    let max_swap_bytes = application_samples
        .iter()
        .filter_map(|sample| sample.swap_kib)
        .map(bytes)
        .max()
        .unwrap_or_default()
        .max(
            samples
                .iter()
                .filter_map(|sample| sample.cgroup_swap_bytes)
                .max()
                .unwrap_or_default(),
        );
    let peak_rss_bytes = application_samples
        .iter()
        .filter_map(|sample| sample.hwm_kib)
        .map(bytes)
        .max()
        .unwrap_or_default();
    let _ = sample_path;
    let report = HttpMemoryReport {
        peak_rss_bytes,
        steady_rss_plus_swap_bytes: steady_peak,
        max_swap_bytes,
        cgroup_peak_bytes: evidence
            .cgroup
            .memory_peak_bytes
            .ok_or_else(|| invalid("cgroup", "memory peak is missing"))?,
        oom_delta: evidence
            .cgroup
            .memory_events
            .as_ref()
            .ok_or_else(|| invalid("cgroup", "memory events are missing"))?
            .oom_delta,
        oom_kill_delta: evidence
            .cgroup
            .memory_events
            .as_ref()
            .ok_or_else(|| invalid("cgroup", "memory events are missing"))?
            .oom_kill_delta,
        plateau_growth_bytes,
        unexpected_failed_requests: 0,
        expected_fault_outcomes,
        db_read_delta,
        samples: application_samples.len(),
    };
    validate_resource_budget(&evidence, &report)?;
    Ok(report)
}

/// Validate an aggregated run against the case IDs required by the caller's
/// pinned matrix manifest. Each case lives in `cases/<case-id>/` and is checked
/// by the same public per-case validator used before returning a case report.
pub fn validate_http_memory_suite(
    output_dir: &Path,
    required_case_ids: &[String],
) -> Result<Vec<HttpMemoryReport>, String> {
    let suite_bytes = fs::read(output_dir.join("suite.json"))
        .map_err(|error| invalid("manifest", format!("cannot read suite.json: {error}")))?;
    let suite: SuiteEvidence = serde_json::from_slice(&suite_bytes)
        .map_err(|error| invalid("manifest", format!("invalid suite.json: {error}")))?;
    if suite.schema_version != EVIDENCE_SCHEMA_VERSION {
        return Err(invalid(
            "manifest",
            format!("unsupported suite schema version {}", suite.schema_version),
        ));
    }
    if required_case_ids.is_empty()
        || required_case_ids.iter().any(|id| !is_case_id(id))
        || has_duplicates(required_case_ids)
    {
        return Err(invalid(
            "manifest",
            "required case inventory must contain unique safe case IDs",
        ));
    }
    if suite.expected_case_ids != required_case_ids {
        return Err(invalid(
            "manifest",
            "suite expected case IDs differ from the pinned required inventory",
        ));
    }
    if suite.executed_case_ids != required_case_ids {
        return Err(invalid(
            "manifest",
            "one or more required cases were not executed exactly once",
        ));
    }

    let cases_dir = output_dir.join("cases");
    let mut artifact_case_ids = fs::read_dir(&cases_dir)
        .map_err(|error| invalid("manifest", format!("cannot read cases/: {error}")))?
        .map(|entry| {
            let entry = entry.map_err(|error| invalid("manifest", error.to_string()))?;
            let file_type = entry
                .file_type()
                .map_err(|error| invalid("manifest", error.to_string()))?;
            if !file_type.is_dir() {
                return Err(invalid("manifest", "cases/ contains a non-directory entry"));
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("manifest", "case directory name is not UTF-8"))?;
            if !is_case_id(&name) {
                return Err(invalid(
                    "manifest",
                    "case directory name is not a safe case ID",
                ));
            }
            Ok(name)
        })
        .collect::<Result<Vec<_>, _>>()?;
    artifact_case_ids.sort();
    let mut required_sorted = required_case_ids.to_vec();
    required_sorted.sort();
    if artifact_case_ids != required_sorted {
        return Err(invalid(
            "manifest",
            "case artifact directories do not exactly match the required inventory",
        ));
    }

    required_case_ids
        .iter()
        .map(|case_id| {
            validate_http_memory_evidence_for_case(&cases_dir.join(case_id), Some(case_id))
        })
        .collect()
}

fn validate_image(image: &ImageEvidence, composition: &ProcessComposition) -> Result<(), String> {
    if image.platform != "linux/amd64" {
        return Err(invalid(
            "platform",
            format!("expected linux/amd64, got {}", image.platform),
        ));
    }
    if image.runner_platform != "linux/amd64" || image.emulated {
        return Err(invalid(
            "platform",
            format!(
                "native linux/amd64 runner required, got {} (emulated={})",
                image.runner_platform, image.emulated
            ),
        ));
    }
    if image.glibc_version != REQUIRED_GLIBC_VERSION {
        return Err(invalid(
            "platform",
            format!(
                "expected glibc {REQUIRED_GLIBC_VERSION}, got {}",
                image.glibc_version
            ),
        ));
    }
    for (label, digest) in [
        ("image id", image.image_id.as_str()),
        ("OCI digest", image.oci_digest.as_str()),
        ("source bundle digest", image.source_bundle_digest.as_str()),
        ("expected OCI digest", image.expected_oci_digest.as_str()),
        (
            "expected source bundle digest",
            image.expected_source_bundle_digest.as_str(),
        ),
    ] {
        if !is_sha256(digest) {
            return Err(invalid("image", format!("{label} is not a SHA-256 digest")));
        }
    }
    if image.oci_digest != image.expected_oci_digest {
        return Err(invalid(
            "image",
            "captured OCI digest differs from the bundle manifest",
        ));
    }
    if image.source_bundle_digest != image.expected_source_bundle_digest {
        return Err(invalid(
            "bundle",
            "image was not built from the declared source bundle",
        ));
    }
    if image
        .app_features
        .iter()
        .any(|feature| matches!(feature.as_str(), "test-fixtures" | "eval-support"))
    {
        return Err(invalid(
            "features",
            "instrumented application features are forbidden",
        ));
    }
    let mut actual_features = image.app_features.clone();
    actual_features.sort_unstable();
    let mut expected_features = match composition {
        ProcessComposition::Http => vec!["fs-watch", "mcp-apps", "streamable-http"],
        ProcessComposition::Stdio | ProcessComposition::Cli => vec!["fs-watch", "mcp-apps"],
    };
    expected_features.sort_unstable();
    if actual_features
        != expected_features
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    {
        return Err(invalid(
            "features",
            format!(
                "application feature profile does not match the system-allocator profile for {composition:?}"
            ),
        ));
    }
    Ok(())
}

fn validate_resource_budget(evidence: &Evidence, report: &HttpMemoryReport) -> Result<(), String> {
    match evidence.case.mode {
        EmbeddingFixtureMode::LocalNative { .. } => {
            let baseline = evidence
                .baseline
                .as_ref()
                .ok_or_else(|| invalid("budget", "LocalNative paired baseline is missing"))?;
            if !is_sha256(&baseline.image_oci_digest)
                || baseline.image_oci_digest == evidence.image.oci_digest
            {
                return Err(invalid(
                    "budget",
                    "LocalNative baseline must identify a distinct pinned image digest",
                ));
            }
            if report.peak_rss_bytes > baseline.peak_rss_bytes
                || report
                    .steady_rss_plus_swap_bytes
                    .is_none_or(|steady| steady > baseline.steady_rss_plus_swap_bytes)
            {
                return Err(invalid(
                    "budget",
                    "LocalNative peak or steady memory exceeds the paired baseline",
                ));
            }
            validate_common_resource_gates(report)
        }
        EmbeddingFixtureMode::Remote { .. }
        | EmbeddingFixtureMode::DisabledEmpty { .. }
        | EmbeddingFixtureMode::DisabledStored { .. } => {
            if report.peak_rss_bytes >= REMOTE_PEAK_RSS_LIMIT_BYTES {
                return Err(invalid(
                    "budget",
                    format!(
                        "peak RSS {} bytes reaches the {} byte limit",
                        report.peak_rss_bytes, REMOTE_PEAK_RSS_LIMIT_BYTES
                    ),
                ));
            }
            if let Some(steady) = report.steady_rss_plus_swap_bytes
                && steady >= REMOTE_STEADY_LIMIT_BYTES
            {
                return Err(invalid(
                    "budget",
                    format!(
                        "steady RSS plus swap {steady} bytes reaches the {REMOTE_STEADY_LIMIT_BYTES} byte limit"
                    ),
                ));
            }
            if report.cgroup_peak_bytes >= REMOTE_CGROUP_PEAK_LIMIT_BYTES {
                return Err(invalid(
                    "budget",
                    format!(
                        "cgroup peak {} bytes reaches the {} byte limit",
                        report.cgroup_peak_bytes, REMOTE_CGROUP_PEAK_LIMIT_BYTES
                    ),
                ));
            }
            validate_common_resource_gates(report)
        }
    }
}

fn validate_common_resource_gates(report: &HttpMemoryReport) -> Result<(), String> {
    if report.max_swap_bytes != 0 {
        return Err(invalid(
            "budget",
            format!(
                "swap usage is {} bytes; zero is required",
                report.max_swap_bytes
            ),
        ));
    }
    if report.oom_delta != 0 || report.oom_kill_delta != 0 {
        return Err(invalid("budget", "OOM and OOM-kill deltas must be zero"));
    }
    if let Some(growth) = report.plateau_growth_bytes
        && growth > PLATEAU_GROWTH_LIMIT_BYTES
    {
        return Err(invalid(
            "budget",
            format!("plateau growth {growth} bytes exceeds {PLATEAU_GROWTH_LIMIT_BYTES} bytes"),
        ));
    }
    if report.unexpected_failed_requests != 0 {
        return Err(invalid("budget", "unexpected failed requests must be zero"));
    }
    if report.samples == 0 {
        return Err(invalid("budget", "at least one memory sample is required"));
    }
    Ok(())
}

fn validate_process(
    process: &ProcessEvidence,
    composition: &ProcessComposition,
) -> Result<(), String> {
    let (expected_binary, expected_transport) = match composition {
        ProcessComposition::Http => ("memory_mcp_http", "streamable_http"),
        ProcessComposition::Stdio => ("memory_mcp", "stdio_mcp"),
        ProcessComposition::Cli => ("memory_mcp", "cli"),
    };
    if process.binary_name != expected_binary || process.transport != expected_transport {
        return Err(invalid(
            "process",
            format!(
                "measured binary/transport {}/{} does not match {composition:?}",
                process.binary_name, process.transport
            ),
        ));
    }
    if process.pid == 0
        || process.start_time_ticks == 0
        || process.observed_pid != process.pid
        || process.observed_start_time_ticks != process.start_time_ticks
    {
        return Err(invalid(
            "sampler",
            "application PID or start time changed during sampling",
        ));
    }
    if process.sampler_exit_code != 0 || !process.sampler_started_before_app {
        return Err(invalid(
            "sampler",
            "sampler did not cover the application lifetime successfully",
        ));
    }
    if !process.cold_start_observed {
        return Err(invalid("sampler", "cold-start interval was not sampled"));
    }
    if process.steady_window_ms == 0 && composition != &ProcessComposition::Cli {
        return Err(invalid("samples", "steady observation window is missing"));
    }
    Ok(())
}

fn validate_manifest(evidence: &Evidence) -> Result<(), String> {
    let id = &evidence.case.id;
    if evidence.expected_case_ids.as_slice() != [id.as_str()]
        || evidence.executed_case_ids.as_slice() != [id.as_str()]
    {
        return Err(invalid(
            "manifest",
            "expected and executed case inventory does not exactly match this case",
        ));
    }
    if id.trim().is_empty() || evidence.case.repetition == 0 {
        return Err(invalid(
            "manifest",
            "case id and repetition must be explicit",
        ));
    }
    Ok(())
}

fn validate_case_shape(case: &MemoryCase) -> Result<(), String> {
    let composition_supports_scenario = match case.composition {
        ProcessComposition::Http => true,
        ProcessComposition::Stdio => matches!(
            case.scenario,
            MemoryScenario::ColdDecay | MemoryScenario::RetrievalMisses
        ),
        ProcessComposition::Cli => case.scenario == MemoryScenario::Reembed,
    };
    if !composition_supports_scenario {
        return Err(invalid(
            "manifest",
            format!(
                "scenario {:?} is unsupported for composition {:?}",
                case.scenario, case.composition
            ),
        ));
    }

    if case.scenario == MemoryScenario::RetrievalMisses && case.workload.is_none() {
        return Err(invalid(
            "manifest",
            "retrieval miss case must name one retrieval workload",
        ));
    }
    if case.scenario != MemoryScenario::RetrievalMisses && case.workload.is_some() {
        return Err(invalid(
            "manifest",
            "only retrieval miss cases may name a retrieval workload",
        ));
    }
    if case.workload == Some(RetrievalWorkload::Dashboard)
        && case.composition != ProcessComposition::Http
    {
        return Err(invalid(
            "manifest",
            "dashboard workload requires the HTTP composition",
        ));
    }
    if matches!(case.mode, EmbeddingFixtureMode::LocalNative { .. })
        && case.composition != ProcessComposition::Stdio
    {
        return Err(invalid(
            "manifest",
            "LocalNative process memory must use the stdio composition",
        ));
    }
    Ok(())
}

fn validate_mode(case: &MemoryCase, provider: &ProviderEvidence) -> Result<(), String> {
    let expected_dimension = match case.mode {
        EmbeddingFixtureMode::Remote { dimension }
        | EmbeddingFixtureMode::DisabledEmpty { dimension }
        | EmbeddingFixtureMode::DisabledStored { dimension }
        | EmbeddingFixtureMode::LocalNative { dimension } => Some(dimension),
    };
    if expected_dimension == Some(0) {
        return Err(invalid("mode", "embedding dimension must be positive"));
    }
    validate_provider_identity(case, provider, expected_dimension)?;
    if provider.resolved_dimension != expected_dimension {
        return Err(invalid(
            "mode",
            format!(
                "resolved embedding width {:?} does not match fixture {:?}",
                provider.resolved_dimension, expected_dimension
            ),
        ));
    }
    let disabled_empty = matches!(case.mode, EmbeddingFixtureMode::DisabledEmpty { .. });
    if let Some(dimension) = expected_dimension {
        if provider.index_dimension != Some(dimension) {
            return Err(invalid(
                "mode",
                format!(
                    "ANN index width {:?} does not match configured width {dimension}",
                    provider.index_dimension
                ),
            ));
        }
        if disabled_empty {
            if case.transition.is_some()
                || !provider.stored_dimensions.is_empty()
                || !provider.observed_dimensions.is_empty()
            {
                return Err(invalid(
                    "mode",
                    "DisabledEmpty requires a stable positive schema width and no stored vectors",
                ));
            }
        } else {
            validate_transition_dimensions(case, provider, dimension)?;
        }
    } else if !provider.stored_dimensions.is_empty() || !provider.observed_dimensions.is_empty() {
        return Err(invalid(
            "mode",
            "DisabledEmpty fixture unexpectedly contains stored vectors",
        ));
    }
    let disabled = matches!(
        case.mode,
        EmbeddingFixtureMode::DisabledEmpty { .. } | EmbeddingFixtureMode::DisabledStored { .. }
    );
    if disabled
        && (case.transition.is_some()
            || matches!(
                case.scenario,
                MemoryScenario::Backfill
                    | MemoryScenario::Reembed
                    | MemoryScenario::InterruptedResume
            ))
    {
        return Err(invalid(
            "mode",
            "embedding rewrite cannot run when embeddings are disabled",
        ));
    }
    if disabled && provider.requests != 0 {
        return Err(invalid(
            "provider",
            "disabled embedding mode made provider requests",
        ));
    }
    if disabled && (!provider.trap_listener_started || provider.trap_requests != 0) {
        return Err(invalid(
            "provider",
            "disabled mode requires a live trap listener with zero requests",
        ));
    }
    if disabled && matches!(case.workload, Some(RetrievalWorkload::Ann)) {
        return Err(invalid(
            "mode",
            "ANN workload is invalid when embeddings are disabled",
        ));
    }
    if disabled && provider.ann_queries > 0 {
        return Err(invalid(
            "mode",
            "ANN query observed while embeddings are disabled",
        ));
    }
    Ok(())
}

fn validate_provider_identity(
    case: &MemoryCase,
    provider: &ProviderEvidence,
    expected_dimension: Option<usize>,
) -> Result<(), String> {
    let Some(schema_dimension) = expected_dimension else {
        return Err(invalid("mode", "embedding schema dimension is required"));
    };
    if provider.schema_dimension != schema_dimension
        || provider.configured_dimension != Some(schema_dimension)
    {
        return Err(invalid(
            "mode",
            format!(
                "configured width {:?} and schema width {} must match case width {schema_dimension}",
                provider.configured_dimension, provider.schema_dimension
            ),
        ));
    }

    let embedding_enabled = matches!(
        case.mode,
        EmbeddingFixtureMode::Remote { .. } | EmbeddingFixtureMode::LocalNative { .. }
    );
    if provider.embedding_enabled != embedding_enabled {
        return Err(invalid(
            "mode",
            format!(
                "embedding enabled flag {} does not match fixture mode {}",
                provider.embedding_enabled, embedding_enabled
            ),
        ));
    }

    let mut persisted_dimensions = provider
        .persisted_embeddings
        .iter()
        .map(|embedding| embedding.dimension)
        .collect::<Vec<_>>();
    if provider
        .persisted_embeddings
        .iter()
        .any(|embedding| embedding.dimension == 0 || embedding.signature.trim().is_empty())
    {
        return Err(invalid(
            "provider",
            "persisted embedding evidence requires positive widths and nonempty signatures",
        ));
    }
    persisted_dimensions.sort_unstable();
    persisted_dimensions.dedup();
    if persisted_dimensions != provider.stored_dimensions {
        return Err(invalid(
            "mode",
            format!(
                "persisted embedding identities {:?} do not match stored widths {:?}",
                persisted_dimensions, provider.stored_dimensions
            ),
        ));
    }

    if !embedding_enabled {
        if provider.provider_id.is_some()
            || provider.model_revision.is_some()
            || provider.provider_dimension.is_some()
            || provider.configured_signature.is_some()
            || !provider.observed_signatures.is_empty()
        {
            return Err(invalid(
                "provider",
                "disabled embedding mode must not report an active provider identity",
            ));
        }
        return Ok(());
    }

    let provider_id = provider
        .provider_id
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    let expected_provider_id = match case.mode {
        EmbeddingFixtureMode::Remote { .. } => "openai-compatible",
        EmbeddingFixtureMode::LocalNative { .. } => "local-candle",
        EmbeddingFixtureMode::DisabledEmpty { .. }
        | EmbeddingFixtureMode::DisabledStored { .. } => {
            return Err(invalid(
                "provider",
                "disabled embedding mode must not report an enabled provider identity",
            ));
        }
    };
    let model_revision = provider
        .model_revision
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    let configured_signature = provider
        .configured_signature
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    if provider_id.is_none()
        || provider_id != Some(expected_provider_id)
        || model_revision.is_none()
        || configured_signature.is_none()
        || provider.provider_dimension != Some(schema_dimension)
    {
        return Err(invalid(
            "provider",
            "enabled embedding evidence requires provider, model revision, provider width, and signature",
        ));
    }
    let configured_signature = configured_signature.unwrap_or_default();
    if provider
        .observed_signatures
        .iter()
        .any(|signature| signature != configured_signature)
        || (provider.requests > 0 && provider.observed_signatures.is_empty())
    {
        return Err(invalid(
            "provider",
            "observed provider signatures do not match the configured embedding identity",
        ));
    }

    let transition = case.transition.as_ref();
    match (transition, &case.phase) {
        (None, MemoryPhase::Steady) => {
            if provider.persisted_embeddings.iter().any(|embedding| {
                embedding.dimension != schema_dimension
                    || embedding.signature != configured_signature
            }) {
                return Err(invalid(
                    "provider",
                    "stable stored embeddings must match the configured width and signature",
                ));
            }
        }
        (Some(transition), MemoryPhase::Before) => {
            if provider
                .persisted_embeddings
                .iter()
                .any(|embedding| embedding.signature == configured_signature)
            {
                return Err(invalid(
                    "provider",
                    "pre-transition evidence already contains the target signature",
                ));
            }
            if matches!(transition, EmbeddingTransition::Backfill)
                && !provider.persisted_embeddings.is_empty()
            {
                return Err(invalid(
                    "provider",
                    "backfill pre-state must not contain persisted embeddings",
                ));
            }
        }
        (Some(transition), MemoryPhase::InProgress) => {
            let has_target_signature = provider.persisted_embeddings.iter().any(|embedding| {
                embedding.dimension == schema_dimension
                    && embedding.signature == configured_signature
            });
            if !has_target_signature {
                return Err(invalid(
                    "provider",
                    "in-progress rewrite must contain a persisted target signature",
                ));
            }
            if matches!(transition, EmbeddingTransition::Signature { .. })
                && !provider.persisted_embeddings.iter().any(|embedding| {
                    embedding.dimension == schema_dimension
                        && embedding.signature != configured_signature
                })
            {
                return Err(invalid(
                    "provider",
                    "same-width signature transition must preserve both old and new identities while running",
                ));
            }
        }
        (Some(_), MemoryPhase::After | MemoryPhase::AfterResume) => {
            if provider.persisted_embeddings.iter().any(|embedding| {
                embedding.dimension != schema_dimension
                    || embedding.signature != configured_signature
            }) {
                return Err(invalid(
                    "provider",
                    "completed rewrite must persist only the configured target identity",
                ));
            }
        }
        _ => {
            return Err(invalid(
                "provider",
                "embedding identity evidence does not match the declared transition phase",
            ));
        }
    }
    Ok(())
}

fn validate_transition_dimensions(
    case: &MemoryCase,
    provider: &ProviderEvidence,
    target_dimension: usize,
) -> Result<(), String> {
    let rewrite_scenario = matches!(
        case.scenario,
        MemoryScenario::Backfill | MemoryScenario::Reembed | MemoryScenario::InterruptedResume
    );
    if rewrite_scenario != case.transition.is_some() {
        return Err(invalid(
            "mode",
            "rewrite scenario and embedding transition must be declared together",
        ));
    }
    let Some(transition) = &case.transition else {
        if case.phase != MemoryPhase::Steady
            || provider.stored_dimensions.as_slice() != [target_dimension]
            || provider.observed_dimensions.as_slice() != [target_dimension]
        {
            return Err(invalid(
                "mode",
                "stable mode must contain only the configured width in every observation",
            ));
        }
        return Ok(());
    };

    if !matches!(
        case.scenario,
        MemoryScenario::Backfill | MemoryScenario::Reembed | MemoryScenario::InterruptedResume
    ) {
        return Err(invalid(
            "mode",
            "embedding transition is only valid for a rewrite scenario",
        ));
    }
    if case.scenario == MemoryScenario::InterruptedResume && case.phase != MemoryPhase::AfterResume
    {
        return Err(invalid(
            "mode",
            "interrupted resume evidence must represent its post-resume phase",
        ));
    }
    if case.phase == MemoryPhase::AfterResume && case.scenario != MemoryScenario::InterruptedResume
    {
        return Err(invalid(
            "mode",
            "post-resume phase is only valid for interrupted resume scenarios",
        ));
    }
    let (source_dimension, transition_target) = match transition {
        EmbeddingTransition::Backfill => (None, target_dimension),
        EmbeddingTransition::Dimension { from, to } => {
            if *from == 0 || from == to {
                return Err(invalid(
                    "mode",
                    "dimension transition requires a positive source width different from its target",
                ));
            }
            (Some(*from), *to)
        }
        EmbeddingTransition::Signature { dimension } => (Some(*dimension), *dimension),
    };
    if transition_target != target_dimension {
        return Err(invalid(
            "mode",
            format!(
                "transition target width {transition_target} does not match configured width {target_dimension}"
            ),
        ));
    }

    let mut allowed_observed = source_dimension.into_iter().collect::<Vec<_>>();
    allowed_observed.push(target_dimension);
    allowed_observed.sort_unstable();
    allowed_observed.dedup();
    if provider.observed_dimensions != allowed_observed {
        return Err(invalid(
            "mode",
            format!(
                "observed vector widths {:?} do not exactly match transition widths {:?}",
                provider.observed_dimensions, allowed_observed
            ),
        ));
    }

    let mut expected_stored = match case.phase {
        MemoryPhase::Before => source_dimension.into_iter().collect::<Vec<_>>(),
        MemoryPhase::InProgress => source_dimension
            .map(|source| vec![source, target_dimension])
            .unwrap_or_else(|| vec![target_dimension]),
        MemoryPhase::After | MemoryPhase::AfterResume => vec![target_dimension],
        MemoryPhase::Steady => {
            return Err(invalid(
                "mode",
                "transition case is missing its explicit phase",
            ));
        }
    };
    expected_stored.sort_unstable();
    expected_stored.dedup();
    if provider.stored_dimensions != expected_stored {
        return Err(invalid(
            "mode",
            format!(
                "final stored vector widths {:?} do not match phase {:?} expectation {:?}",
                provider.stored_dimensions, case.phase, expected_stored
            ),
        ));
    }
    Ok(())
}

fn read_samples(
    output_dir: &Path,
    evidence: &Evidence,
) -> Result<(Vec<MemorySample>, String), String> {
    if evidence.samples_file != "samples.jsonl" {
        return Err(invalid(
            "samples",
            "sample path must be the local samples.jsonl file",
        ));
    }
    let input = fs::read_to_string(output_dir.join(&evidence.samples_file))
        .map_err(|error| invalid("samples", format!("cannot read samples.jsonl: {error}")))?;
    let mut samples = Vec::new();
    for (line_number, line) in input.lines().enumerate() {
        if line.trim().is_empty() {
            return Err(invalid(
                "samples",
                format!("blank sample record at line {}", line_number + 1),
            ));
        }
        let sample = serde_json::from_str::<MemorySample>(line).map_err(|error| {
            invalid(
                "samples",
                format!("invalid sample at line {}: {error}", line_number + 1),
            )
        })?;
        samples.push(sample);
    }
    if samples.is_empty() || samples.len() != evidence.sample_count {
        return Err(invalid(
            "samples",
            format!(
                "sample count mismatch: manifest {}, file {}",
                evidence.sample_count,
                samples.len()
            ),
        ));
    }
    Ok((samples, evidence.samples_file.clone()))
}

fn validate_sample_coverage<'a>(
    samples: &'a [MemorySample],
    process: &ProcessEvidence,
    composition: &ProcessComposition,
) -> Result<Vec<&'a MemorySample>, String> {
    if samples
        .windows(2)
        .any(|pair| pair[0].elapsed_ms >= pair[1].elapsed_ms)
    {
        return Err(invalid(
            "samples",
            "sample timestamps are not strictly increasing",
        ));
    }
    let application_samples = samples
        .iter()
        .filter(|sample| sample.binary_name == process.binary_name && sample.process_alive)
        .collect::<Vec<_>>();
    if application_samples.is_empty() {
        return Err(invalid(
            "sampler",
            format!(
                "no memory sample belongs to measured binary {}",
                process.binary_name
            ),
        ));
    }
    if application_samples.iter().any(|sample| {
        sample.rss_kib.is_none() || sample.swap_kib.is_none() || sample.hwm_kib.is_none()
    }) {
        return Err(invalid(
            "samples",
            "a live application sample is missing RSS, swap, or high-water memory",
        ));
    }
    if application_samples
        .first()
        .is_none_or(|sample| sample.elapsed_ms > 1_000)
    {
        return Err(invalid("sampler", "no sample covers cold process startup"));
    }
    if *composition == ProcessComposition::Cli {
        return Ok(application_samples);
    }
    if application_samples
        .last()
        .is_none_or(|sample| sample.elapsed_ms < process.steady_window_ms)
    {
        return Err(invalid("samples", "samples do not cover the steady window"));
    }
    let steady_start = application_samples
        .last()
        .map(|sample| sample.elapsed_ms.saturating_sub(process.steady_window_ms))
        .unwrap_or_default();
    if application_samples
        .iter()
        .filter(|sample| sample.elapsed_ms >= steady_start)
        .count()
        < 2
    {
        return Err(invalid(
            "samples",
            "at least two samples must cover the steady observation window",
        ));
    }
    Ok(application_samples)
}

fn validate_cgroup(cgroup: &CgroupEvidence, samples: &[MemorySample]) -> Result<(), String> {
    let peak = cgroup
        .memory_peak_bytes
        .ok_or_else(|| invalid("cgroup", "memory peak counter is missing"))?;
    let events = cgroup
        .memory_events
        .as_ref()
        .ok_or_else(|| invalid("cgroup", "memory event counters are missing"))?;
    if samples.iter().any(|sample| {
        sample.cgroup_current_bytes.is_none()
            || sample.cgroup_swap_bytes.is_none()
            || sample.cgroup_memory_peak_bytes.is_none()
            || sample.cgroup_oom.is_none()
            || sample.cgroup_oom_kill.is_none()
    }) {
        return Err(invalid(
            "cgroup",
            "one or more raw samples lack complete cgroup v2 memory counters",
        ));
    }
    let sampled_current_peak = samples
        .iter()
        .filter_map(|sample| sample.cgroup_current_bytes)
        .max()
        .ok_or_else(|| invalid("cgroup", "cgroup current samples are missing"))?;
    let sampled_peak = samples
        .iter()
        .filter_map(|sample| sample.cgroup_memory_peak_bytes)
        .max()
        .ok_or_else(|| invalid("cgroup", "cgroup memory peak samples are missing"))?;
    if peak != sampled_peak {
        return Err(invalid(
            "cgroup",
            "run.json memory peak differs from raw cgroup memory.peak samples",
        ));
    }
    if peak < sampled_current_peak {
        return Err(invalid(
            "cgroup",
            "memory peak is lower than sampled current memory",
        ));
    }
    let first = samples
        .first()
        .ok_or_else(|| invalid("cgroup", "cgroup samples are missing"))?;
    let last = samples
        .last()
        .ok_or_else(|| invalid("cgroup", "cgroup samples are missing"))?;
    let oom_delta = last
        .cgroup_oom
        .zip(first.cgroup_oom)
        .and_then(|(last, first)| last.checked_sub(first))
        .ok_or_else(|| invalid("cgroup", "OOM counter decreased during sampling"))?;
    let oom_kill_delta = last
        .cgroup_oom_kill
        .zip(first.cgroup_oom_kill)
        .and_then(|(last, first)| last.checked_sub(first))
        .ok_or_else(|| invalid("cgroup", "OOM-kill counter decreased during sampling"))?;
    if events.oom_delta != oom_delta || events.oom_kill_delta != oom_kill_delta {
        return Err(invalid(
            "cgroup",
            "run.json OOM deltas differ from raw cgroup memory.events samples",
        ));
    }
    if events.oom_delta != 0 || events.oom_kill_delta != 0 {
        return Err(invalid("cgroup", "OOM event counters increased"));
    }
    Ok(())
}

fn validate_requests(evidence: &Evidence) -> Result<usize, String> {
    let mut expected_faults = Vec::new();
    let mut unexpected_failures = 0;
    for request in &evidence.requests {
        if request.request_id.trim().is_empty() {
            return Err(invalid("request", "request id is missing"));
        }
        match request.outcome {
            RequestOutcome::Success => {}
            RequestOutcome::ExpectedFault => {
                if evidence.case.scenario != MemoryScenario::ErrorRelease {
                    unexpected_failures += 1;
                } else if let Some(category) = &request.category {
                    if !ALLOWED_INJECTED_FAULTS.contains(&category.as_str()) {
                        return Err(invalid(
                            "request",
                            format!("fault category {category:?} is not an approved injection"),
                        ));
                    }
                    expected_faults.push(category.clone());
                } else {
                    return Err(invalid("request", "expected fault has no category"));
                }
            }
            RequestOutcome::ApplicationError | RequestOutcome::TransportError => {
                unexpected_failures += 1;
            }
        }
    }
    if unexpected_failures != 0 {
        return Err(invalid(
            "request",
            format!("{unexpected_failures} unexpected request failures were recorded"),
        ));
    }
    expected_faults.sort();
    let mut declared = evidence.expected_faults.clone();
    declared.sort();
    if evidence.case.scenario == MemoryScenario::ErrorRelease {
        let required = match evidence.case.mode {
            EmbeddingFixtureMode::DisabledEmpty { .. }
            | EmbeddingFixtureMode::DisabledStored { .. } => {
                DISABLED_ERROR_RELEASE_FAULTS.as_slice()
            }
            EmbeddingFixtureMode::Remote { .. } | EmbeddingFixtureMode::LocalNative { .. } => {
                REMOTE_ERROR_RELEASE_FAULTS.as_slice()
            }
        };
        let mut required = required.to_vec();
        required.sort_unstable();
        if expected_faults != declared || declared != required {
            return Err(invalid(
                "request",
                "error release must record each mode-required fault exactly once",
            ));
        }
        let last_fault = evidence
            .requests
            .iter()
            .rposition(|request| matches!(request.outcome, RequestOutcome::ExpectedFault))
            .ok_or_else(|| invalid("request", "error release contains no injected fault"))?;
        if !evidence.requests[last_fault + 1..]
            .iter()
            .any(|request| matches!(request.outcome, RequestOutcome::Success))
        {
            return Err(invalid(
                "request",
                "error release has no successful request after its final injected fault",
            ));
        }
    } else if !declared.is_empty() || !expected_faults.is_empty() {
        return Err(invalid(
            "request",
            "ordinary scenarios cannot declare expected faults",
        ));
    }
    Ok(expected_faults.len())
}

fn validate_db_reads(evidence: &Evidence) -> Result<u64, String> {
    if evidence.case.scenario != MemoryScenario::RetrievalMisses {
        return Ok(0);
    }
    let workload = evidence
        .case
        .workload
        .as_ref()
        .ok_or_else(|| invalid("cache-miss", "retrieval miss case has no workload"))?;
    let successful_requests = evidence
        .requests
        .iter()
        .filter(|request| {
            request.phase == Some(RequestPhase::Measured)
                && matches!(request.outcome, RequestOutcome::Success)
        })
        .map(|request| request.request_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let misses = evidence
        .db_reads
        .events
        .iter()
        .filter(|event| {
            event.foreground
                && event.statement_kind.eq_ignore_ascii_case("SELECT")
                && !event.cache_hit
                && successful_requests.contains(event.request_id.as_str())
        })
        .collect::<Vec<_>>();
    let distinct_requests = misses
        .iter()
        .map(|event| event.request_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    if misses.len() < REQUIRED_CACHE_MISSES || distinct_requests.len() < REQUIRED_CACHE_MISSES {
        return Err(invalid(
            "cache-miss",
            format!(
                "only {} distinct verified foreground SELECT reads for workload {workload:?}; {REQUIRED_CACHE_MISSES} required",
                distinct_requests.len()
            ),
        ));
    }
    Ok(misses.len() as u64)
}

fn validate_plateau_growth(
    evidence: &Evidence,
    samples: &[&MemorySample],
) -> Result<Option<i64>, String> {
    if evidence.case.scenario != MemoryScenario::RetrievalMisses {
        if !evidence.idle_checkpoints.is_empty() {
            return Err(invalid(
                "plateau",
                "idle checkpoints are only valid for retrieval-miss cases",
            ));
        }
        return Ok(None);
    }

    const WARMUP_REQUESTS: usize = 20;
    const MEASURED_REQUESTS: usize = REQUIRED_CACHE_MISSES;
    const CHECKPOINT_REQUESTS_PER_BLOCK: usize = 20;
    const LAST_BLOCK_START: usize = MEASURED_REQUESTS - CHECKPOINT_REQUESTS_PER_BLOCK;

    if evidence.requests.len() != WARMUP_REQUESTS + MEASURED_REQUESTS {
        return Err(invalid(
            "plateau",
            format!(
                "expected {WARMUP_REQUESTS} warmup and {MEASURED_REQUESTS} measured requests, got {} total",
                evidence.requests.len()
            ),
        ));
    }
    let mut request_ids = HashSet::with_capacity(evidence.requests.len());
    let mut previous_finished_ms = None;
    for (index, request) in evidence.requests.iter().enumerate() {
        let expected_phase = if index < WARMUP_REQUESTS {
            RequestPhase::Warmup
        } else {
            RequestPhase::Measured
        };
        if request.phase != Some(expected_phase) {
            return Err(invalid(
                "plateau",
                "request phases must contain 20 warmups followed by 100 measured requests",
            ));
        }
        if !request_ids.insert(request.request_id.as_str()) {
            return Err(invalid(
                "plateau",
                format!("request ID {:?} occurs more than once", request.request_id),
            ));
        }
        if !matches!(request.outcome, RequestOutcome::Success) {
            return Err(invalid(
                "plateau",
                "warmup and measured retrieval requests must all succeed",
            ));
        }
        let started_ms = request
            .started_ms
            .ok_or_else(|| invalid("plateau", "request start time is missing"))?;
        let finished_ms = request
            .finished_ms
            .ok_or_else(|| invalid("plateau", "request completion time is missing"))?;
        if started_ms > finished_ms
            || previous_finished_ms.is_some_and(|previous| started_ms < previous)
        {
            return Err(invalid(
                "plateau",
                "retrieval request intervals must be ordered and non-overlapping",
            ));
        }
        previous_finished_ms = Some(finished_ms);
    }

    let measurement_finished_ms = evidence
        .process
        .measurement_finished_ms
        .ok_or_else(|| invalid("plateau", "measurement completion time is missing"))?;
    if previous_finished_ms.is_some_and(|finished_ms| measurement_finished_ms < finished_ms) {
        return Err(invalid(
            "plateau",
            "measurement ended before the final request completed",
        ));
    }

    let measured = &evidence.requests[WARMUP_REQUESTS..];
    let expected_checkpoint_ids = measured[..CHECKPOINT_REQUESTS_PER_BLOCK]
        .iter()
        .chain(&measured[LAST_BLOCK_START..])
        .map(|request| request.request_id.as_str())
        .collect::<Vec<_>>();
    let actual_checkpoint_ids = evidence
        .idle_checkpoints
        .iter()
        .map(|checkpoint| checkpoint.request_id.as_str())
        .collect::<Vec<_>>();
    if actual_checkpoint_ids != expected_checkpoint_ids {
        return Err(invalid(
            "plateau",
            "idle checkpoints must cover the first and last 20 measured requests in order",
        ));
    }

    let mut first_block_bytes = Vec::with_capacity(CHECKPOINT_REQUESTS_PER_BLOCK);
    let mut last_block_bytes = Vec::with_capacity(CHECKPOINT_REQUESTS_PER_BLOCK);
    let mut previous_checkpoint_ms = None;
    for (checkpoint_index, checkpoint) in evidence.idle_checkpoints.iter().enumerate() {
        if checkpoint.inflight_requests != 0 {
            return Err(invalid(
                "plateau",
                "idle checkpoint was captured while requests remained in flight",
            ));
        }
        if previous_checkpoint_ms.is_some_and(|previous| checkpoint.elapsed_ms <= previous) {
            return Err(invalid(
                "plateau",
                "idle checkpoints must have strictly increasing sample times",
            ));
        }
        previous_checkpoint_ms = Some(checkpoint.elapsed_ms);

        let measured_index = if checkpoint_index < CHECKPOINT_REQUESTS_PER_BLOCK {
            checkpoint_index
        } else {
            LAST_BLOCK_START + checkpoint_index - CHECKPOINT_REQUESTS_PER_BLOCK
        };
        let request = &measured[measured_index];
        let finished_ms = request
            .finished_ms
            .ok_or_else(|| invalid("plateau", "checkpoint request completion time is missing"))?;
        if checkpoint.elapsed_ms <= finished_ms {
            return Err(invalid(
                "plateau",
                "checkpoint must be sampled after its request completes",
            ));
        }
        if let Some(next_request) = measured.get(measured_index + 1) {
            let next_started_ms = next_request
                .started_ms
                .ok_or_else(|| invalid("plateau", "next request start time is missing"))?;
            if checkpoint.elapsed_ms >= next_started_ms {
                return Err(invalid(
                    "plateau",
                    "checkpoint must be sampled before the next measured request starts",
                ));
            }
        } else if checkpoint.elapsed_ms > measurement_finished_ms {
            return Err(invalid(
                "plateau",
                "final idle checkpoint occurs after the measurement ended",
            ));
        }

        let matching_samples = samples
            .iter()
            .filter(|sample| sample.elapsed_ms == checkpoint.elapsed_ms)
            .collect::<Vec<_>>();
        let [sample] = matching_samples.as_slice() else {
            return Err(invalid(
                "plateau",
                "each idle checkpoint must match exactly one raw sampler sample",
            ));
        };
        let (rss_kib, swap_kib) = sample
            .rss_kib
            .zip(sample.swap_kib)
            .ok_or_else(|| invalid("plateau", "idle sample lacks RSS or swap counters"))?;
        let rss_plus_swap_bytes = bytes(rss_kib.saturating_add(swap_kib));
        if checkpoint_index < CHECKPOINT_REQUESTS_PER_BLOCK {
            first_block_bytes.push(rss_plus_swap_bytes);
        } else {
            last_block_bytes.push(rss_plus_swap_bytes);
        }
    }

    Ok(Some(signed_delta(
        median_bytes(&last_block_bytes),
        median_bytes(&first_block_bytes),
    )))
}

fn validate_scenario_evidence(evidence: &Evidence) -> Result<(), String> {
    if evidence.case.scenario == MemoryScenario::ColdDecay {
        let mut signals = evidence.decay.completion_signals.clone();
        signals.sort();
        if signals != ["first", "repeat"] {
            return Err(invalid(
                "sampler",
                "cold decay must record first and repeat completion signals",
            ));
        }
        let mut invalidated_facts = evidence.decay.invalidated_fact_ids.clone();
        invalidated_facts.sort_unstable();
        if invalidated_facts.as_slice() != [EXPECTED_DECAY_FACT_ID] {
            return Err(invalid(
                "lifecycle",
                "cold decay did not durably invalidate the seeded old fact",
            ));
        }
        let mut closed_claims = evidence.decay.closed_claim_ids.clone();
        closed_claims.sort_unstable();
        let mut expected_claims = EXPECTED_DECAY_CLAIM_IDS.to_vec();
        expected_claims.sort_unstable();
        if closed_claims != expected_claims {
            return Err(invalid(
                "lifecycle",
                "cold decay did not durably close every derived claim",
            ));
        }
    }
    if evidence.case.scenario == MemoryScenario::ConcurrentMaintenance {
        validate_concurrent_maintenance(evidence)?;
    }
    match evidence.case.scenario {
        MemoryScenario::Backfill | MemoryScenario::Reembed | MemoryScenario::InterruptedResume => {
            let rewrite = evidence
                .rewrite
                .as_ref()
                .ok_or_else(|| invalid("rewrite", "rewrite scenario has no durable outcome"))?;
            validate_rewrite_evidence(&evidence.case, rewrite)?;
        }
        _ if evidence.rewrite.is_some() => {
            return Err(invalid(
                "rewrite",
                "non-rewrite scenario contains rewrite outcome evidence",
            ));
        }
        _ => {}
    }
    Ok(())
}

fn validate_concurrent_maintenance(evidence: &Evidence) -> Result<(), String> {
    if evidence.requests.len() != REQUIRED_CONCURRENT_REQUESTS_PER_TENANT * 2 {
        return Err(invalid(
            "concurrency",
            "maintenance must complete exactly four requests for each of two tenants",
        ));
    }

    let mut request_ids = std::collections::HashSet::with_capacity(evidence.requests.len());
    let mut tenant_requests = std::collections::BTreeMap::<String, Vec<(u64, u64)>>::new();
    let mut intervals = Vec::with_capacity(evidence.requests.len());
    for request in &evidence.requests {
        if !matches!(request.outcome, RequestOutcome::Success)
            || !request_ids.insert(request.request_id.as_str())
        {
            return Err(invalid(
                "concurrency",
                "maintenance requires eight unique successful request outcomes",
            ));
        }
        let (Some(tenant_id), Some(started_ms), Some(finished_ms)) = (
            request.tenant_id.as_deref(),
            request.started_ms,
            request.finished_ms,
        ) else {
            return Err(invalid(
                "concurrency",
                "maintenance request is missing tenant or monotonic timing evidence",
            ));
        };
        if tenant_id.trim().is_empty() || finished_ms <= started_ms {
            return Err(invalid(
                "concurrency",
                "maintenance request has an invalid tenant or non-positive interval",
            ));
        }
        tenant_requests
            .entry(tenant_id.to_owned())
            .or_default()
            .push((started_ms, finished_ms));
        intervals.push((started_ms, finished_ms));
    }

    if tenant_requests.len() != 2
        || tenant_requests
            .values()
            .any(|requests| requests.len() != REQUIRED_CONCURRENT_REQUESTS_PER_TENANT)
    {
        return Err(invalid(
            "concurrency",
            "maintenance must cover exactly two tenants with four requests each",
        ));
    }
    let (Some(common_start), Some(common_end)) = (
        intervals.iter().map(|(started_ms, _)| *started_ms).max(),
        intervals.iter().map(|(_, finished_ms)| *finished_ms).min(),
    ) else {
        return Err(invalid(
            "concurrency",
            "all eight foreground request intervals must share a common overlap",
        ));
    };
    if common_start >= common_end {
        return Err(invalid(
            "concurrency",
            "all eight foreground request intervals must share a common overlap",
        ));
    }
    let Some(maintenance) = evidence.maintenance.as_ref() else {
        return Err(invalid(
            "concurrency",
            "maintenance execution interval is missing",
        ));
    };
    if maintenance.finished_ms <= maintenance.started_ms
        || maintenance.started_ms >= common_end
        || common_start >= maintenance.finished_ms
    {
        return Err(invalid(
            "concurrency",
            "maintenance execution must overlap the shared foreground request window",
        ));
    }
    Ok(())
}

fn validate_rewrite_evidence(case: &MemoryCase, rewrite: &RewriteEvidence) -> Result<(), String> {
    let expected_fact_ids = (0..REQUIRED_REWRITE_FACTS)
        .map(|index| format!("fact:storage_memory_{index:04}"))
        .collect::<Vec<_>>();
    let mut source_fact_ids = rewrite.source_fact_ids.clone();
    let mut final_fact_ids = rewrite.final_fact_ids.clone();
    source_fact_ids.sort_unstable();
    final_fact_ids.sort_unstable();
    if rewrite.requested_facts != REQUIRED_REWRITE_FACTS
        || source_fact_ids != expected_fact_ids
        || final_fact_ids != expected_fact_ids
    {
        return Err(invalid(
            "rewrite",
            "rewrite must preserve the exact seeded 205-fact identity set",
        ));
    }
    if !rewrite.failed_fact_ids.is_empty() {
        return Err(invalid(
            "rewrite",
            "completed memory run contains failed rewrite facts",
        ));
    }

    let expected_last_fact_id = expected_fact_ids.last().cloned();
    match (&case.scenario, &case.phase, rewrite.state) {
        (
            MemoryScenario::Backfill | MemoryScenario::Reembed,
            MemoryPhase::Before,
            RewriteState::Ready,
        ) if rewrite.completed_facts == 0
            && rewrite.batch_sizes.is_empty()
            && rewrite.last_completed_fact_id.is_none()
            && rewrite.interrupted_after_facts.is_none()
            && rewrite.resume_after_fact_id.is_none()
            && !rewrite.restarted =>
        {
            Ok(())
        }
        (
            MemoryScenario::Backfill | MemoryScenario::Reembed,
            MemoryPhase::InProgress,
            RewriteState::Running,
        ) if (1..REQUIRED_REWRITE_FACTS).contains(&rewrite.completed_facts)
            && rewrite.batch_sizes.iter().all(|size| *size > 0)
            && rewrite
                .batch_sizes
                .iter()
                .try_fold(0usize, |total, size| total.checked_add(*size))
                == Some(rewrite.completed_facts)
            && rewrite.last_completed_fact_id.as_deref()
                == expected_fact_ids
                    .get(rewrite.completed_facts - 1)
                    .map(String::as_str)
            && rewrite.interrupted_after_facts.is_none()
            && rewrite.resume_after_fact_id.is_none()
            && !rewrite.restarted =>
        {
            Ok(())
        }
        (
            MemoryScenario::Backfill | MemoryScenario::Reembed,
            MemoryPhase::After,
            RewriteState::Completed,
        ) if rewrite.completed_facts == REQUIRED_REWRITE_FACTS
            && rewrite.batch_sizes == REQUIRED_REWRITE_BATCHES
            && rewrite.last_completed_fact_id == expected_last_fact_id
            && rewrite.interrupted_after_facts.is_none()
            && rewrite.resume_after_fact_id.is_none()
            && !rewrite.restarted =>
        {
            Ok(())
        }
        (MemoryScenario::InterruptedResume, MemoryPhase::AfterResume, RewriteState::Completed)
            if rewrite.completed_facts == REQUIRED_REWRITE_FACTS
                && rewrite.batch_sizes == [100, 1, 100, 4]
                && rewrite.last_completed_fact_id == expected_last_fact_id
                && rewrite.interrupted_after_facts == Some(101)
                && rewrite.resume_after_fact_id.as_deref()
                    == expected_fact_ids.get(100).map(String::as_str)
                && rewrite.restarted =>
        {
            Ok(())
        }
        _ => Err(invalid(
            "rewrite",
            format!(
                "rewrite outcome does not match {:?}/{:?} phase and durable progress",
                case.scenario, case.phase
            ),
        )),
    }
}

fn is_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn is_case_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn has_duplicates(values: &[String]) -> bool {
    let mut unique = std::collections::HashSet::with_capacity(values.len());
    values.iter().any(|value| !unique.insert(value))
}

fn bytes(kib: u64) -> u64 {
    kib.saturating_mul(1024)
}

fn median_bytes(values: &[u64]) -> u64 {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        ((u128::from(sorted[middle - 1]) + u128::from(sorted[middle])) / 2) as u64
    } else {
        sorted[middle]
    }
}

fn signed_delta(after: u64, before: u64) -> i64 {
    if after >= before {
        i64::try_from(after - before).unwrap_or(i64::MAX)
    } else {
        -i64::try_from(before - after).unwrap_or(i64::MAX)
    }
}

fn invalid(category: &str, detail: impl std::fmt::Display) -> String {
    format!("[{category}] {detail}")
}
