//! Loading and validation of perf profile files (schema v1).

use crate::perf::PerfError;
use crate::perf::domain::PROFILE_SCHEMA;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// A parsed and validated perf profile (`memory-mcp-perf-profile/v1`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PerfProfile {
    pub name: String,
    pub budget_seconds: u64,
    pub host_tag: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub scenarios: Vec<ScenarioSpec>,
    #[serde(default)]
    pub gates: Vec<GateSpec>,
}

/// One scenario entry: closed id, driver, and run shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScenarioSpec {
    pub id: ScenarioId,
    pub driver: DriverKind,
    pub scales: Vec<u32>,
    pub iterations: u32,
    #[serde(default)]
    pub warmup: u32,
    #[serde(default)]
    pub queries: Option<u32>,
}

/// A regression gate over one scenario metric.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateSpec {
    pub scenario: ScenarioId,
    pub metric: GateMetric,
    pub max_ratio: f64,
    pub min_samples: u32,
    #[serde(default)]
    pub direction: GateDirection,
}

/// Whether a worse metric is larger or smaller than the baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateDirection {
    #[default]
    LowerIsBetter,
    HigherIsBetter,
}

/// Closed set of perf scenarios.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScenarioId {
    IngestScale,
    AssembleScale,
    LatencyExtract,
    LatencyResolve,
    LatencyExplain,
    LatencyInvalidate,
}

impl ScenarioId {
    /// The kebab-case wire name, matching the profile JSON and artifact keys.
    pub fn as_str(self) -> &'static str {
        match self {
            ScenarioId::IngestScale => "ingest-scale",
            ScenarioId::AssembleScale => "assemble-scale",
            ScenarioId::LatencyExtract => "latency-extract",
            ScenarioId::LatencyResolve => "latency-resolve",
            ScenarioId::LatencyExplain => "latency-explain",
            ScenarioId::LatencyInvalidate => "latency-invalidate",
        }
    }
}

/// How a scenario is executed; only `in_process` is supported in v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverKind {
    InProcess,
    Stdio,
    Http,
}

/// Metrics a gate can watch; names match `reducer::latency_key` suffixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateMetric {
    P50Us,
    P95Us,
    P99Us,
    MeanUs,
    ThroughputEps,
}

/// Load and validate a profile file.
///
/// The schema field is checked before typed deserialization so an unknown
/// version is `PerfError::Schema`, never a best-effort v1 parse; typed shape
/// errors surface as `PerfError::Validation`.
pub fn load(path: &Path) -> Result<PerfProfile, PerfError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| PerfError::Validation(format!("cannot read {}: {e}", path.display())))?;
    parse(&raw)
}

fn parse(raw: &str) -> Result<PerfProfile, PerfError> {
    let value: serde_json::Value = serde_json::from_str(raw)?;
    let schema = value.get("schema").and_then(|s| s.as_str()).unwrap_or("");
    if schema != PROFILE_SCHEMA {
        return Err(PerfError::Schema(schema.to_owned()));
    }
    let profile: PerfProfile =
        serde_json::from_value(value).map_err(|e| PerfError::Validation(e.to_string()))?;
    validate(&profile)?;
    Ok(profile)
}

fn validate(profile: &PerfProfile) -> Result<(), PerfError> {
    if profile.budget_seconds == 0 {
        return Err(PerfError::Validation(
            "budget_seconds must be greater than zero".to_owned(),
        ));
    }
    if profile.scenarios.is_empty() {
        return Err(PerfError::Validation(
            "scenarios must not be empty".to_owned(),
        ));
    }
    for spec in &profile.scenarios {
        if !matches!(spec.driver, DriverKind::InProcess) {
            let name = match spec.driver {
                DriverKind::Stdio => "stdio",
                DriverKind::Http => "http",
                DriverKind::InProcess => unreachable!(),
            };
            return Err(PerfError::UnsupportedDriver(name.to_owned()));
        }
        if spec.scales.is_empty() {
            return Err(PerfError::Validation(format!(
                "scenario {}: scales must not be empty",
                spec.id.as_str()
            )));
        }
        if spec.scales.windows(2).any(|w| w[0] >= w[1]) {
            return Err(PerfError::Validation(format!(
                "scenario {}: scales must be strictly increasing",
                spec.id.as_str()
            )));
        }
        if spec.iterations == 0 {
            return Err(PerfError::Validation(format!(
                "scenario {}: iterations must be greater than zero",
                spec.id.as_str()
            )));
        }
    }
    for gate in &profile.gates {
        if !gate.max_ratio.is_finite() || gate.max_ratio <= 0.0 {
            return Err(PerfError::Validation(format!(
                "gate {}: max_ratio must be a positive finite number",
                gate.scenario.as_str()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perf::PerfError;
    use std::io::Write;

    const HAPPY: &str = r#"{
        "schema": "memory-mcp-perf-profile/v1",
        "name": "pr-smoke",
        "budget_seconds": 300,
        "host_tag": "dev-m2",
        "env": { "NER_EXTRACTOR": "regex" },
        "scenarios": [
            {
                "id": "ingest-scale",
                "driver": "in_process",
                "scales": [10],
                "iterations": 3,
                "warmup": 1,
                "queries": 5
            }
        ],
        "gates": [
            {
                "scenario": "ingest-scale",
                "metric": "throughput_eps",
                "max_ratio": 1.25,
                "min_samples": 5,
                "direction": "higher_is_better"
            }
        ]
    }"#;

    fn write_temp(content: &str) -> std::path::PathBuf {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("profile.json");
        let mut file = std::fs::File::create(&path).expect("create");
        file.write_all(content.as_bytes()).expect("write");
        std::mem::forget(dir);
        path
    }

    #[test]
    fn happy_path_reads_all_fields() {
        let profile = load(&write_temp(HAPPY)).expect("happy path loads");
        assert_eq!(profile.name, "pr-smoke");
        assert_eq!(profile.budget_seconds, 300);
        assert_eq!(profile.host_tag, "dev-m2");
        assert_eq!(
            profile.env.get("NER_EXTRACTOR").map(String::as_str),
            Some("regex")
        );
        assert_eq!(profile.scenarios.len(), 1);
        let spec = &profile.scenarios[0];
        assert_eq!(spec.id, ScenarioId::IngestScale);
        assert_eq!(spec.driver, DriverKind::InProcess);
        assert_eq!(spec.scales, vec![10]);
        assert_eq!(spec.iterations, 3);
        assert_eq!(spec.warmup, 1);
        assert_eq!(spec.queries, Some(5));
        assert_eq!(profile.gates.len(), 1);
        assert_eq!(profile.gates[0].direction, GateDirection::HigherIsBetter);
    }

    #[test]
    fn unknown_schema_version_is_schema_error() {
        let content = HAPPY.replace("memory-mcp-perf-profile/v1", "memory-mcp-perf-profile/v2");
        let err = load(&write_temp(&content)).expect_err("v2 must not load");
        assert!(matches!(err, PerfError::Schema(_)), "got {err:?}");
    }

    #[test]
    fn stdio_driver_is_unsupported() {
        let content = HAPPY.replace("in_process", "stdio");
        let err = load(&write_temp(&content)).expect_err("stdio rejected");
        assert!(
            matches!(&err, PerfError::UnsupportedDriver(d) if d == "stdio"),
            "got {err:?}"
        );
    }

    #[test]
    fn zero_budget_seconds_is_validation_error() {
        let content = HAPPY.replace("\"budget_seconds\": 300", "\"budget_seconds\": 0");
        let err = load(&write_temp(&content)).expect_err("zero budget rejected");
        assert!(matches!(err, PerfError::Validation(_)), "got {err:?}");
    }

    #[test]
    fn empty_scales_are_validation_error() {
        let content = HAPPY.replace("\"scales\": [10]", "\"scales\": []");
        let err = load(&write_temp(&content)).expect_err("empty scales rejected");
        assert!(matches!(err, PerfError::Validation(_)), "got {err:?}");
    }

    #[test]
    fn non_monotonic_scales_are_validation_error() {
        let content = HAPPY.replace("\"scales\": [10]", "\"scales\": [100, 10]");
        let err = load(&write_temp(&content)).expect_err("descending scales rejected");
        assert!(matches!(err, PerfError::Validation(_)), "got {err:?}");
    }

    #[test]
    fn unknown_scenario_id_is_validation_error() {
        let content = HAPPY.replace("ingest-scale", "quantum-flux");
        let err = load(&write_temp(&content)).expect_err("unknown id rejected");
        assert!(matches!(err, PerfError::Validation(_)), "got {err:?}");
    }

    #[test]
    fn gate_direction_defaults_to_lower_is_better() {
        let content = HAPPY.replace(",\n                \"direction\": \"higher_is_better\"", "");
        let profile = load(&write_temp(&content)).expect("loads without direction");
        assert_eq!(profile.gates[0].direction, GateDirection::LowerIsBetter);
    }
}
