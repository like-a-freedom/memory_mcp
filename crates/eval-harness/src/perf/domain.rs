//! Shared perf domain types and schema constants.

use crate::perf::reducer::LatencyStats;
use serde::{Deserialize, Serialize};

/// Schema string for perf profile files (`evals/perf/profiles/*.json`).
pub const PROFILE_SCHEMA: &str = "memory-mcp-perf-profile/v1";
/// Schema string for committed run artifacts.
pub const ARTIFACT_SCHEMA: &str = "memory-mcp-perf/v1";

/// Machine identity of a run; gates only compare artifacts of the same tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostFingerprint {
    pub host_tag: String,
    pub os: String,
    pub arch: String,
}

/// Process resource deltas around one scenario; `None` fields mean the
/// platform sampler could not measure them — zeros are never fabricated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceSnapshot {
    pub rss_bytes: Option<u64>,
    pub peak_rss_bytes: Option<u64>,
    pub disk_read_bytes: Option<u64>,
    pub disk_write_bytes: Option<u64>,
    pub io_read_ops: Option<u64>,
    pub io_write_ops: Option<u64>,
}

/// One normalized SQL template aggregate observed during a scenario.
///
/// `phase` separates untimed setup SQL (`"setup"`) from the measured part of
/// the run (`"timed"`). A template observed in both phases yields one record
/// per phase. Artifacts predating the field deserialize as `"setup"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlTemplateRecord {
    pub template_hash: String,
    pub op: String,
    pub class: String,
    #[serde(default = "default_sql_phase")]
    pub phase: String,
    pub count: u32,
    pub stats: LatencyStats,
}

fn default_sql_phase() -> String {
    "setup".to_owned()
}

/// Everything one `(scenario, scale)` recording produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenarioRecord {
    pub latencies: Option<LatencyStats>,
    pub throughput_eps: Option<f64>,
    pub samples_raw: Vec<u64>,
    pub sql: Vec<SqlTemplateRecord>,
    pub resources: Option<ResourceSnapshot>,
    pub invalid_reasons: Vec<String>,
}
