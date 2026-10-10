//! Profile runner: budget guard, warmup discard, sampling, artifact emission.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::artifact::{self, PerfArtifact};
use super::domain::{ARTIFACT_SCHEMA, HostFingerprint, ResourceSnapshot, ScenarioRecord};
use super::profile::{PerfProfile, ScenarioId, ScenarioSpec};
use super::reducer;
use super::scenarios::{PerfEnv, build};
use super::verdict::{PerfVerdict, evaluate};

/// Everything a run needs from its operator: where to write the artifact,
/// which baseline to judge against, and the identity of the run.
pub struct RunOptions {
    pub artifact_path: PathBuf,
    pub baseline_path: Option<PathBuf>,
    pub host_tag: String,
    pub commit: String,
    pub now_iso: String,
}

/// The written artifact plus its verdict.
pub struct RunOutcome {
    pub artifact: PerfArtifact,
    pub verdict: PerfVerdict,
}

/// Runs every scenario of `profile` in profile order (determinism), discards
/// warm-up iterations, records the rest, and writes the artifact. Budget
/// exhaustion ends the run normally: a partial artifact is still written and
/// the verdict is `Invalid` with a reason containing the budget failure.
pub async fn run_profile(
    profile: &PerfProfile,
    profile_digest: &str,
    opts: RunOptions,
) -> RunOutcome {
    let started = Instant::now();
    let mut records: BTreeMap<String, ScenarioRecord> = BTreeMap::new();
    let mut reasons: Vec<String> = Vec::new();

    let budget = Duration::from_secs(profile.budget_seconds);
    let work = async {
        for spec in &profile.scenarios {
            for &scale in &spec.scales {
                let (key, record) = run_scenario(spec, scale).await;
                records.insert(key, record);
            }
        }
    };
    if tokio::time::timeout(budget, work).await.is_err() {
        reasons.push("budget exceeded: budget_seconds exhausted".to_owned());
    }

    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let artifact = PerfArtifact {
        schema: ARTIFACT_SCHEMA.to_owned(),
        profile: profile.name.clone(),
        profile_digest: profile_digest.to_owned(),
        commit: opts.commit.clone(),
        host: HostFingerprint {
            host_tag: opts.host_tag.clone(),
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
        },
        started_at: opts.now_iso.clone(),
        duration_ms,
        scenarios: records,
    };
    if let Err(err) = artifact::write(&artifact, &opts.artifact_path) {
        reasons.push(format!("artifact write failed: {err}"));
    }

    for (key, record) in &artifact.scenarios {
        for invalid in &record.invalid_reasons {
            reasons.push(format!("{key}: {invalid}"));
        }
    }

    let baseline = match &opts.baseline_path {
        Some(path) => match artifact::read(path) {
            Ok(baseline) => Some(baseline),
            Err(err) => {
                reasons.push(format!("baseline read failed: {err}"));
                None
            }
        },
        None => None,
    };
    let verdict = merge(evaluate(profile, &artifact, baseline.as_ref()), reasons);
    RunOutcome { artifact, verdict }
}

/// Runner-level invalidity wins over the evaluated verdict; accumulated
/// reasons from both sides are preserved.
fn merge(evaluated: PerfVerdict, mut reasons: Vec<String>) -> PerfVerdict {
    if reasons.is_empty() {
        return evaluated;
    }
    match evaluated {
        PerfVerdict::Invalid(existing) => {
            reasons.extend(existing);
            PerfVerdict::Invalid(reasons)
        }
        _ => PerfVerdict::Invalid(reasons),
    }
}

/// One `(scenario, scale)`: a dedicated environment (isolated database and
/// ledger), warm-up iterations discarded, resource snapshots bracketing the
/// scenario, one SQL ledger snapshot.
async fn run_scenario(spec: &ScenarioSpec, scale: u32) -> (String, ScenarioRecord) {
    let key = format!("{}-{}", spec.id.as_str(), scale);
    let env = PerfEnv::connect().await;
    let mut scenario_spec = spec.clone();
    scenario_spec.scales = vec![scale];
    let run = build(&scenario_spec);

    let before = env.sampler.sample();
    let mut samples: Vec<u64> = Vec::new();
    let mut invalid_reasons: Vec<String> = Vec::new();
    let total = spec.warmup.saturating_add(spec.iterations);
    for iteration in 0..total {
        let evidence = run.run(&env, iteration).await;
        invalid_reasons.extend(evidence.invalid_reasons);
        if iteration >= spec.warmup {
            samples.extend(evidence.latencies_us);
        }
    }
    let after = env.sampler.sample();
    let sql = env.ledger.snapshot();

    let resources = match (before, after) {
        (Some(before), Some(after)) => Some(snapshot_delta(before, after)),
        _ => {
            invalid_reasons.push("resource sampler unavailable on this platform".to_owned());
            None
        }
    };

    let latencies = reducer::latency_stats(&samples);
    if latencies.is_none() {
        invalid_reasons.push("no latency samples recorded".to_owned());
    }
    let throughput_eps = if matches!(spec.id, ScenarioId::IngestScale | ScenarioId::AssembleScale) {
        latencies.and_then(|stats| reducer::throughput_eps(stats.total_us, stats.count))
    } else {
        None
    };

    let record = ScenarioRecord {
        latencies,
        throughput_eps,
        samples_raw: samples,
        sql,
        resources,
        invalid_reasons,
    };
    (key, record)
}

fn snapshot_delta(before: ResourceSnapshot, after: ResourceSnapshot) -> ResourceSnapshot {
    ResourceSnapshot {
        rss_bytes: delta(after.rss_bytes, before.rss_bytes),
        peak_rss_bytes: delta(after.peak_rss_bytes, before.peak_rss_bytes),
        disk_read_bytes: delta(after.disk_read_bytes, before.disk_read_bytes),
        disk_write_bytes: delta(after.disk_write_bytes, before.disk_write_bytes),
        io_read_ops: delta(after.io_read_ops, before.io_read_ops),
        io_write_ops: delta(after.io_write_ops, before.io_write_ops),
    }
}

fn delta(after: Option<u64>, before: Option<u64>) -> Option<u64> {
    match (after, before) {
        (Some(after), Some(before)) => Some(after.saturating_sub(before)),
        _ => None,
    }
}
