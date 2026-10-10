//! The single home of perf formulas (ADR-0025 analogue, ADR-0082).
//!
//! No other perf module contains percentile, ratio, or throughput math.

use crate::perf::profile::GateMetric;
use serde::{Deserialize, Serialize};

/// Reduced latency distribution of one sample set, microseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatencyStats {
    pub count: u32,
    pub p50_us: u64,
    pub p90_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub mean_us: u64,
    pub total_us: u64,
}

/// Nearest-rank percentiles (`ceil(p * n)`, 1-based) plus count/mean/total.
/// An empty slice has no statistics — `None`, never zeros.
pub fn latency_stats(samples: &[u64]) -> Option<LatencyStats> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let n = sorted.len() as u64;
    let total = sorted.iter().copied().sum::<u64>();
    let pick = |p: f64| -> u64 {
        let rank = (p * n as f64).ceil().max(1.0) as usize;
        sorted[rank.min(sorted.len()) - 1]
    };
    Some(LatencyStats {
        count: samples.len() as u32,
        p50_us: pick(0.50),
        p90_us: pick(0.90),
        p95_us: pick(0.95),
        p99_us: pick(0.99),
        mean_us: total / n,
        total_us: total,
    })
}

/// Episodes per second from a total duration; zero duration has no rate.
pub fn throughput_eps(total_us: u64, episodes: u32) -> Option<f64> {
    if total_us == 0 {
        return None;
    }
    Some(episodes as f64 / (total_us as f64 / 1_000_000.0))
}

/// log2-log2 least-squares slope of value vs scale; fewer than 3 points, or
/// any non-positive coordinate, has no exponent.
///
/// Reserved for external trend tooling: v1 gates compare against a baseline
/// artifact and do not consume exponents yet.
pub fn scaling_exponent(points: &[(f64, f64)]) -> Option<f64> {
    if points.len() < 3 {
        return None;
    }
    let logs: Vec<(f64, f64)> = points.iter().map(|(x, y)| (x.log2(), y.log2())).collect();
    if logs.iter().any(|(x, y)| !x.is_finite() || !y.is_finite()) {
        return None;
    }
    let n = logs.len() as f64;
    let mean_x = logs.iter().map(|(x, _)| x).sum::<f64>() / n;
    let mean_y = logs.iter().map(|(_, y)| y).sum::<f64>() / n;
    let mut sxx = 0.0;
    let mut sxy = 0.0;
    for (x, y) in &logs {
        sxx += (x - mean_x) * (x - mean_x);
        sxy += (x - mean_x) * (y - mean_y);
    }
    if sxx == 0.0 {
        return None;
    }
    Some(sxy / sxx)
}

/// current / baseline; a non-positive baseline has no ratio.
pub fn ratio(current: f64, baseline: f64) -> Option<f64> {
    if baseline <= 0.0 {
        return None;
    }
    Some(current / baseline)
}

/// Artifact key for a scenario latency metric: `"{scenario}.{metric}"`.
pub fn latency_key(scenario: &str, stat: GateMetric) -> String {
    let metric = match stat {
        GateMetric::P50Us => "p50_us",
        GateMetric::P95Us => "p95_us",
        GateMetric::P99Us => "p99_us",
        GateMetric::MeanUs => "mean_us",
        GateMetric::ThroughputEps => "throughput_eps",
    };
    format!("{scenario}.{metric}")
}

/// Artifact key for a SQL class aggregate: `"sql.{class}"`.
///
/// Reserved for gate metrics over SQL classes; v1 verdicts do not key on it
/// yet, but the key shape is fixed here so tooling can rely on it.
pub fn sql_key(class: &str) -> String {
    format!("sql.{class}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perf::profile::GateMetric;

    #[test]
    fn nearest_rank_percentiles_and_totals() {
        let samples: Vec<u64> = (1..=100).collect();
        let stats = latency_stats(&samples).expect("non-empty");
        assert_eq!(stats.count, 100);
        assert_eq!(stats.p50_us, 50);
        assert_eq!(stats.p90_us, 90);
        assert_eq!(stats.p95_us, 95);
        assert_eq!(stats.p99_us, 99);
        assert_eq!(stats.total_us, 5050);
        assert_eq!(stats.mean_us, 50);
    }

    #[test]
    fn empty_slice_is_none() {
        assert!(latency_stats(&[]).is_none());
    }

    #[test]
    fn single_sample_collapses_percentiles() {
        let stats = latency_stats(&[42]).expect("one sample");
        assert_eq!(stats.count, 1);
        assert_eq!(stats.p50_us, 42);
        assert_eq!(stats.p99_us, 42);
        assert_eq!(stats.total_us, 42);
        assert_eq!(stats.mean_us, 42);
    }

    #[test]
    fn zero_total_duration_has_no_throughput() {
        assert!(throughput_eps(0, 5).is_none());
    }

    #[test]
    fn throughput_is_episodes_per_second() {
        // 4 episodes over 2 seconds = 2 eps.
        let eps = throughput_eps(2_000_000, 4).expect("throughput");
        assert!((eps - 2.0).abs() < 1e-12);
    }

    #[test]
    fn scaling_exponent_of_quadratic_growth() {
        // Exactly quadratic: 10 * x^2 at x = 1, 2, 4 collinear in log-log.
        let points = [(1.0, 10.0), (2.0, 40.0), (4.0, 160.0)];
        let exponent = scaling_exponent(&points).expect("exponent");
        assert!((exponent - 2.0).abs() < 1e-9, "got {exponent}");
    }

    #[test]
    fn fewer_than_three_points_has_no_exponent() {
        assert!(scaling_exponent(&[(1.0, 10.0), (2.0, 40.0)]).is_none());
    }

    #[test]
    fn non_positive_baseline_has_no_ratio() {
        assert!(ratio(2.0, 0.0).is_none());
        assert!(ratio(2.0, -1.0).is_none());
    }

    #[test]
    fn ratio_divides_current_by_baseline() {
        let value = ratio(3.0, 2.0).expect("ratio");
        assert!((value - 1.5).abs() < 1e-12);
    }

    #[test]
    fn latency_key_is_scenario_dot_metric() {
        let key = latency_key("assemble-scale", GateMetric::P95Us);
        assert_eq!(key, "assemble-scale.p95_us");
    }

    #[test]
    fn sql_key_is_class_prefixed() {
        assert_eq!(sql_key("EpisodeWrite"), "sql.EpisodeWrite");
    }
}
