//! Regression verdict vs baseline.
//!
//! The runner merges its own failure reasons (budget, evidence health) with
//! this gate evaluation; see `runner::run_profile` for the merge order.

use super::artifact::PerfArtifact;
use super::domain::ScenarioRecord;
use super::profile::{GateDirection, GateMetric, PerfProfile};
use super::reducer::{self as reducer, latency_key};

/// Outcome of a perf run: pass, regression against a baseline, or an
/// invalid run (broken evidence — budget, missing samples, host mismatch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PerfVerdict {
    Passed,
    Regressed(Vec<String>),
    Invalid(Vec<String>),
}

impl PerfVerdict {
    pub fn reasons(&self) -> &[String] {
        match self {
            Self::Passed => &[],
            Self::Regressed(reasons) | Self::Invalid(reasons) => reasons,
        }
    }
}

/// Verdict against an optional baseline artifact. Without a baseline the
/// gates are skipped — the first run defines the baseline.
///
/// Precedence: any invalidity wins over any regression. Ratio math lives in
/// `reducer::ratio` exclusively.
pub fn evaluate(
    profile: &PerfProfile,
    artifact: &PerfArtifact,
    baseline: Option<&PerfArtifact>,
) -> PerfVerdict {
    let Some(baseline) = baseline else {
        return PerfVerdict::Passed;
    };
    if baseline.host.host_tag != artifact.host.host_tag {
        return PerfVerdict::Invalid(vec![format!(
            "host_tag mismatch: baseline {} != current {}",
            baseline.host.host_tag, artifact.host.host_tag
        )]);
    }
    // The tag is operator-asserted; the platform fingerprint is measured, so
    // a tag agreeing across different machines still fails here.
    if baseline.host.os != artifact.host.os || baseline.host.arch != artifact.host.arch {
        return PerfVerdict::Invalid(vec![format!(
            "host platform mismatch: baseline {}/{} != current {}/{}",
            baseline.host.os, baseline.host.arch, artifact.host.os, artifact.host.arch
        )]);
    }
    // Comparing against a baseline recorded by a different profile version
    // would silently gate on metrics the current profile never defined.
    if baseline.profile_digest != artifact.profile_digest {
        return PerfVerdict::Invalid(vec![format!(
            "profile digest mismatch: baseline {} != current {}",
            baseline.profile_digest, artifact.profile_digest
        )]);
    }

    let mut invalid: Vec<String> = Vec::new();
    let mut regressed: Vec<String> = Vec::new();
    for gate in &profile.gates {
        let scenario = gate.scenario.as_str();
        let prefix = format!("{scenario}-");
        let current_records: Vec<(&String, &ScenarioRecord)> = artifact
            .scenarios
            .iter()
            .filter(|(key, _)| key.starts_with(&prefix))
            .collect();
        if current_records.is_empty() {
            invalid.push(format!(
                "{}: no scenario record in current artifact",
                latency_key(scenario, gate.metric)
            ));
            continue;
        }
        for (key, record) in current_records {
            let metric_key = latency_key(scenario, gate.metric);
            let sample_count = record.latencies.map(|stats| stats.count).unwrap_or(0);
            if sample_count < gate.min_samples {
                invalid.push(format!(
                    "{key}: sample count {sample_count} below min_samples {}",
                    gate.min_samples
                ));
                continue;
            }
            let Some(current) = metric_value(record, gate.metric) else {
                invalid.push(format!("{key}: current metric missing"));
                continue;
            };
            let Some(baseline_record) = baseline.scenarios.get(key) else {
                invalid.push(format!("{metric_key}: gate baseline metric missing"));
                continue;
            };
            let Some(base_value) = metric_value(baseline_record, gate.metric) else {
                invalid.push(format!("{metric_key}: gate baseline metric missing"));
                continue;
            };
            let message = |ratio: f64| {
                format!(
                    "{metric_key} ratio {ratio:.3}>{} (current {current}, baseline {base_value})",
                    gate.max_ratio
                )
            };
            let beyond = match gate.direction {
                GateDirection::LowerIsBetter => match reducer::ratio(current, base_value) {
                    Some(ratio) => ratio > gate.max_ratio,
                    None => {
                        invalid.push(format!(
                            "{metric_key}: ratio undefined (baseline {base_value})"
                        ));
                        continue;
                    }
                },
                GateDirection::HigherIsBetter => {
                    if current <= 0.0 {
                        regressed.push(message(f64::INFINITY));
                        continue;
                    }
                    match reducer::ratio(base_value, current) {
                        Some(ratio) => ratio > gate.max_ratio,
                        None => {
                            invalid
                                .push(format!("{metric_key}: ratio undefined (current {current})"));
                            continue;
                        }
                    }
                }
            };
            if beyond {
                let ratio = match gate.direction {
                    GateDirection::LowerIsBetter => reducer::ratio(current, base_value),
                    GateDirection::HigherIsBetter => reducer::ratio(base_value, current),
                };
                if let Some(ratio) = ratio {
                    regressed.push(message(ratio));
                }
            }
        }
    }

    if !invalid.is_empty() {
        return PerfVerdict::Invalid(invalid);
    }
    if !regressed.is_empty() {
        return PerfVerdict::Regressed(regressed);
    }
    PerfVerdict::Passed
}

/// Gate metric value of one scenario record; `None` when the record has no
/// value for that metric (missing latency stats or throughput).
fn metric_value(record: &ScenarioRecord, metric: GateMetric) -> Option<f64> {
    let stats = record.latencies?;
    Some(match metric {
        GateMetric::P50Us => stats.p50_us as f64,
        GateMetric::P95Us => stats.p95_us as f64,
        GateMetric::P99Us => stats.p99_us as f64,
        GateMetric::MeanUs => stats.mean_us as f64,
        GateMetric::ThroughputEps => record.throughput_eps?,
    })
}

/// Process exit code: pass 0, regression 1, invalid 2.
pub fn exit_code(verdict: &PerfVerdict) -> i32 {
    match verdict {
        PerfVerdict::Passed => 0,
        PerfVerdict::Regressed(_) => 1,
        PerfVerdict::Invalid(_) => 2,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::perf::domain::HostFingerprint;
    use crate::perf::profile::{DriverKind, GateSpec, ScenarioId, ScenarioSpec};
    use crate::perf::reducer::LatencyStats;

    fn stats(count: u32, p95: u64) -> LatencyStats {
        LatencyStats {
            count,
            p50_us: p95,
            p90_us: p95,
            p95_us: p95,
            p99_us: p95,
            mean_us: p95,
            total_us: p95 * u64::from(count),
        }
    }

    fn record(count: u32, p95: u64, throughput_eps: Option<f64>) -> ScenarioRecord {
        ScenarioRecord {
            latencies: Some(stats(count, p95)),
            throughput_eps,
            samples_raw: vec![p95; count as usize],
            sql: vec![],
            resources: None,
            invalid_reasons: vec![],
        }
    }

    fn artifact(host_tag: &str, scenarios: BTreeMap<String, ScenarioRecord>) -> PerfArtifact {
        PerfArtifact {
            schema: "memory-mcp-perf/v1".to_owned(),
            profile: "p".to_owned(),
            profile_digest: "d".to_owned(),
            commit: "c".to_owned(),
            host: HostFingerprint {
                host_tag: host_tag.to_owned(),
                os: "macos".to_owned(),
                arch: "aarch64".to_owned(),
            },
            started_at: "2026-10-10T00:00:00Z".to_owned(),
            duration_ms: 1,
            scenarios,
        }
    }

    fn profile_with_gates(gates: Vec<GateSpec>) -> PerfProfile {
        PerfProfile {
            name: "p".to_owned(),
            budget_seconds: 60,
            host_tag: "test".to_owned(),
            env: BTreeMap::new(),
            scenarios: vec![ScenarioSpec {
                id: ScenarioId::IngestScale,
                driver: DriverKind::InProcess,
                scales: vec![2],
                iterations: 1,
                warmup: 0,
                queries: None,
            }],
            gates,
        }
    }

    fn gate(metric: GateMetric, max_ratio: f64, min_samples: u32) -> GateSpec {
        GateSpec {
            scenario: ScenarioId::IngestScale,
            metric,
            max_ratio,
            min_samples,
            direction: GateDirection::LowerIsBetter,
        }
    }

    fn single(p95: u64) -> BTreeMap<String, ScenarioRecord> {
        let mut map = BTreeMap::new();
        map.insert("ingest-scale-2".to_owned(), record(10, p95, None));
        map
    }

    #[test]
    fn identical_artifacts_pass() {
        let profile = profile_with_gates(vec![gate(GateMetric::P95Us, 1.25, 1)]);
        let current = artifact("test", single(100));
        let baseline = artifact("test", single(100));
        assert_eq!(
            evaluate(&profile, &current, Some(&baseline)),
            PerfVerdict::Passed
        );
    }

    #[test]
    fn p95_regression_beyond_ratio_is_regressed_with_key_and_values() {
        let profile = profile_with_gates(vec![gate(GateMetric::P95Us, 1.25, 1)]);
        let current = artifact("test", single(130));
        let baseline = artifact("test", single(100));
        match evaluate(&profile, &current, Some(&baseline)) {
            PerfVerdict::Regressed(reasons) => {
                assert_eq!(reasons.len(), 1);
                let msg = &reasons[0];
                assert!(msg.contains("ingest-scale"), "key in message: {msg}");
                assert!(msg.contains("1.3"), "ratio in message: {msg}");
                assert!(msg.contains("130"), "current value in message: {msg}");
                assert!(msg.contains("100"), "baseline value in message: {msg}");
            }
            other => panic!("expected Regressed, got {other:?}"),
        }
    }

    #[test]
    fn host_tag_mismatch_is_invalid() {
        let profile = profile_with_gates(vec![]);
        let current = artifact("host-a", single(100));
        let baseline = artifact("host-b", single(100));
        match evaluate(&profile, &current, Some(&baseline)) {
            PerfVerdict::Invalid(reasons) => {
                assert!(reasons[0].contains("host_tag mismatch"), "{reasons:?}")
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn host_platform_mismatch_is_invalid() {
        let profile = profile_with_gates(vec![]);
        let mut current = artifact("host-a", single(100));
        let baseline = artifact("host-a", single(100));
        current.host.os = "linux".to_owned();
        match evaluate(&profile, &current, Some(&baseline)) {
            PerfVerdict::Invalid(reasons) => {
                assert!(reasons[0].contains("host platform mismatch"), "{reasons:?}")
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn profile_digest_mismatch_is_invalid() {
        let profile = profile_with_gates(vec![]);
        let current = artifact("host-a", single(100));
        let mut baseline = artifact("host-a", single(100));
        baseline.profile_digest = "stale".to_owned();
        match evaluate(&profile, &current, Some(&baseline)) {
            PerfVerdict::Invalid(reasons) => {
                assert!(
                    reasons[0].contains("profile digest mismatch"),
                    "{reasons:?}"
                )
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn gate_metric_missing_from_baseline_is_invalid() {
        let profile = profile_with_gates(vec![gate(GateMetric::ThroughputEps, 1.25, 1)]);
        let mut current_map = BTreeMap::new();
        current_map.insert("ingest-scale-2".to_owned(), record(10, 100, Some(7.0)));
        let mut baseline_map = BTreeMap::new();
        baseline_map.insert("ingest-scale-2".to_owned(), record(10, 100, None));
        let current = artifact("test", current_map);
        let baseline = artifact("test", baseline_map);
        match evaluate(&profile, &current, Some(&baseline)) {
            PerfVerdict::Invalid(reasons) => assert!(
                reasons[0].contains("gate baseline metric missing"),
                "{reasons:?}"
            ),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn without_baseline_gates_are_skipped() {
        let profile = profile_with_gates(vec![gate(GateMetric::P95Us, 1.25, 100)]);
        let current = artifact("test", single(9999));
        assert_eq!(evaluate(&profile, &current, None), PerfVerdict::Passed);
    }

    #[test]
    fn sample_count_below_min_samples_is_invalid() {
        let profile = profile_with_gates(vec![gate(GateMetric::P95Us, 1.25, 5)]);
        let mut current_map = BTreeMap::new();
        current_map.insert("ingest-scale-2".to_owned(), record(3, 100, None));
        let current = artifact("test", current_map);
        let baseline = artifact("test", single(100));
        match evaluate(&profile, &current, Some(&baseline)) {
            PerfVerdict::Invalid(reasons) => {
                assert!(reasons[0].contains("min_samples"), "{reasons:?}")
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn lower_throughput_regresses_when_higher_is_better() {
        let profile = profile_with_gates(vec![GateSpec {
            scenario: ScenarioId::IngestScale,
            metric: GateMetric::ThroughputEps,
            max_ratio: 1.25,
            min_samples: 1,
            direction: GateDirection::HigherIsBetter,
        }]);
        let mut current_map = BTreeMap::new();
        current_map.insert("ingest-scale-2".to_owned(), record(10, 100, Some(0.7)));
        let mut baseline_map = BTreeMap::new();
        baseline_map.insert("ingest-scale-2".to_owned(), record(10, 100, Some(1.0)));
        let current = artifact("test", current_map);
        let baseline = artifact("test", baseline_map);
        match evaluate(&profile, &current, Some(&baseline)) {
            PerfVerdict::Regressed(reasons) => {
                assert!(reasons[0].contains("throughput_eps"), "{reasons:?}")
            }
            other => panic!("expected Regressed, got {other:?}"),
        }
    }

    #[test]
    fn exit_codes_follow_the_contract() {
        assert_eq!(exit_code(&PerfVerdict::Passed), 0);
        assert_eq!(exit_code(&PerfVerdict::Regressed(vec![])), 1);
        assert_eq!(exit_code(&PerfVerdict::Invalid(vec![])), 2);
    }
}
