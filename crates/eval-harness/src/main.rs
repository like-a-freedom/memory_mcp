use eval_harness::cli::{self, Command};
use eval_harness::{
    ActionGroundingSuite, CapacitySuite, ClaimReconciliationSuite, CorpusManifest,
    DownstreamQaSuite, EndToEndSuite, ExtractionSuite, LifecycleReleaseSuite, PoisoningSuite,
    ProfileManifest, RetrievalSuite, RunArtifact, Runner,
};

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = cli::parse();

    match cli.command {
        Command::Run {
            profile,
            artifact,
            baseline,
            suites: suite_filter,
        } => cmd_run(profile, artifact, baseline, suite_filter).await,
        Command::PrepareCorpus {
            manifest,
            output_root,
        } => cmd_prepare_corpus(manifest, output_root).await,
        Command::Merge {
            profile,
            artifact,
            shards,
        } => cmd_merge(profile, artifact, shards),
    }
}

async fn cmd_run(
    profile: std::path::PathBuf,
    artifact: std::path::PathBuf,
    baseline: Option<std::path::PathBuf>,
    suite_filter: Vec<String>,
) -> std::process::ExitCode {
    let manifest = match ProfileManifest::load(&profile) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("error: {e}");
            return std::process::ExitCode::from(2);
        }
    };

    let baseline_artifact = baseline.and_then(|path| {
        let raw = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str::<RunArtifact>(&raw).ok()
    });

    let mut suites: Vec<Box<dyn eval_harness::runner::EvalSuite>> = Vec::new();

    for suite_decl in &manifest.suites {
        if !suite_filter.is_empty() && !suite_filter.contains(&suite_decl.id) {
            continue;
        }

        match suite_decl.id.as_str() {
            "local-retrieval" => match RetrievalSuite::new() {
                Ok(s) => suites.push(Box::new(s)),
                Err(e) => eprintln!(
                    "warning: failed to load {suite}: {e}",
                    suite = suite_decl.id
                ),
            },
            "extraction" => match ExtractionSuite::new() {
                Ok(s) => suites.push(Box::new(s)),
                Err(e) => eprintln!(
                    "warning: failed to load {suite}: {e}",
                    suite = suite_decl.id
                ),
            },
            "claim-reconciliation" => match ClaimReconciliationSuite::new() {
                Ok(s) => suites.push(Box::new(s)),
                Err(e) => eprintln!(
                    "warning: failed to load {suite}: {e}",
                    suite = suite_decl.id
                ),
            },
            "end-to-end" => {
                suites.push(Box::new(EndToEndSuite::new()));
            }
            "action-grounding" => {
                suites.push(Box::new(ActionGroundingSuite::new()));
            }
            "capacity" => {
                suites.push(Box::new(CapacitySuite::new()));
            }
            "poisoning" => {
                suites.push(Box::new(PoisoningSuite::new()));
            }
            "lifecycle" => {
                suites.push(Box::new(LifecycleReleaseSuite::new()));
            }
            "downstream-qa" => {
                suites.push(Box::new(DownstreamQaSuite::new()));
            }
            other => {
                eprintln!("warning: unknown suite {other}");
            }
        }
    }

    if suites.is_empty() {
        eprintln!("error: no suites to run");
        return std::process::ExitCode::from(2);
    }

    let runner = Runner::new(suites);
    let result = runner
        .run(manifest.profile, baseline_artifact.as_ref())
        .await;

    match result {
        Ok(art) => {
            let has_failed_gate = art
                .gates
                .iter()
                .any(|g| g.status == eval_harness::GateStatus::Failed);
            let has_invalid = art
                .outcomes
                .iter()
                .any(|o| o.status == eval_harness::CaseStatus::Invalid);

            if let Err(e) = eval_harness::write_artifact(&artifact, &art) {
                eprintln!("error: failed to write artifact: {e}");
                return std::process::ExitCode::from(2);
            }

            print_summary(&art);

            if has_invalid {
                eprintln!("RESULT: INVALID");
                std::process::ExitCode::from(2)
            } else if has_failed_gate {
                eprintln!("RESULT: GATE FAILED");
                std::process::ExitCode::from(1)
            } else {
                eprintln!("RESULT: PASSED");
                std::process::ExitCode::SUCCESS
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::ExitCode::from(2)
        }
    }
}

async fn cmd_prepare_corpus(
    manifest_path: std::path::PathBuf,
    output_root: std::path::PathBuf,
) -> std::process::ExitCode {
    let raw = match std::fs::read_to_string(&manifest_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: failed to read manifest: {e}");
            return std::process::ExitCode::from(2);
        }
    };

    let manifest = match CorpusManifest::parse(&raw) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("error: invalid manifest: {e}");
            return std::process::ExitCode::from(2);
        }
    };

    eprintln!(
        "preparing corpus {} revision {}...",
        manifest.corpus_id, manifest.revision
    );

    struct LocalFetcher;
    #[async_trait::async_trait]
    impl eval_harness::CorpusFetcher for LocalFetcher {
        async fn fetch(
            &self,
            url: &str,
            _revision: &str,
        ) -> Result<Vec<u8>, eval_harness::EvalError> {
            let response = reqwest::get(url)
                .await
                .map_err(|e| eval_harness::EvalError::Suite(format!("fetch failed: {e}")))?;
            let bytes = response
                .bytes()
                .await
                .map_err(|e| eval_harness::EvalError::Suite(format!("read failed: {e}")))?;
            Ok(bytes.to_vec())
        }
    }

    match eval_harness::prepare_corpus(&manifest, &output_root, &LocalFetcher).await {
        Ok(prepared) => {
            eprintln!("prepared: {}", prepared.data_path.display());
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::ExitCode::from(1)
        }
    }
}

fn cmd_merge(
    _profile_path: std::path::PathBuf,
    artifact_path: std::path::PathBuf,
    shard_paths: Vec<std::path::PathBuf>,
) -> std::process::ExitCode {
    let mut shards = Vec::new();
    for path in &shard_paths {
        let raw = match std::fs::read_to_string(path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: failed to read {}: {e}", path.display());
                return std::process::ExitCode::from(2);
            }
        };
        match serde_json::from_str::<RunArtifact>(&raw) {
            Ok(shard) => shards.push(shard),
            Err(e) => {
                eprintln!("error: invalid artifact {}: {e}", path.display());
                return std::process::ExitCode::from(2);
            }
        }
    }

    match eval_harness::merge_shards(&shards) {
        Ok(merged) => {
            if let Err(e) = eval_harness::write_artifact(&artifact_path, &merged) {
                eprintln!("error: failed to write merged artifact: {e}");
                return std::process::ExitCode::from(2);
            }
            eprintln!(
                "merged {} shards into {}",
                shard_paths.len(),
                artifact_path.display()
            );
            print_summary(&merged);
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: merge failed: {e}");
            std::process::ExitCode::from(1)
        }
    }
}

fn print_summary(art: &RunArtifact) {
    let passed = art
        .outcomes
        .iter()
        .filter(|o| o.status == eval_harness::CaseStatus::Passed)
        .count();
    let failed = art
        .outcomes
        .iter()
        .filter(|o| o.status == eval_harness::CaseStatus::QualityFailed)
        .count();
    let invalid = art
        .outcomes
        .iter()
        .filter(|o| o.status == eval_harness::CaseStatus::Invalid)
        .count();

    println!(
        "profile={} total={} passed={} quality_failed={} invalid={} duration_ms={}",
        format!("{:?}", art.profile).to_lowercase(),
        art.outcomes.len(),
        passed,
        failed,
        invalid,
        art.duration_ms,
    );

    for gate in &art.gates {
        println!(
            "gate={} observed={:.4} status={}",
            gate.metric,
            gate.observed,
            format!("{:?}", gate.status).to_lowercase(),
        );
    }
}
