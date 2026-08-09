use std::collections::BTreeSet;

use eval_harness::cli::{self, Command};
use eval_harness::{
    ActionGroundingSuite, CapacitySuite, ClaimReconciliationSuite, CorpusManifest, DatasetKind,
    DownstreamQaSuite, EndToEndSuite, ExternalRetrievalSuite, ExtractionSuite,
    LifecycleReleaseSuite, PoisoningSuite, ProfileManifest, ResponseSizeSuite, RetrievalSuite,
    RunArtifact, RunRequest, Runner, SuiteId, suites::ner_quality,
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
    let mut issues = Vec::new();

    for suite_decl in &manifest.suites {
        if !suite_filter.is_empty() && !suite_filter.contains(&suite_decl.id) {
            continue;
        }

        match suite_decl.id.as_str() {
            "local-retrieval" => match RetrievalSuite::new() {
                Ok(s) => suites.push(Box::new(s)),
                Err(e) => {
                    eprintln!("warning: failed to load {}: {e}", suite_decl.id);
                    issues.push(eval_harness::RunIssue::empty_suite(&suite_decl.id));
                }
            },
            "extraction" => match ExtractionSuite::new() {
                Ok(s) => suites.push(Box::new(s)),
                Err(e) => {
                    eprintln!("warning: failed to load {}: {e}", suite_decl.id);
                    issues.push(eval_harness::RunIssue::empty_suite(&suite_decl.id));
                }
            },
            "claim-reconciliation" => match ClaimReconciliationSuite::new() {
                Ok(s) => suites.push(Box::new(s)),
                Err(e) => {
                    eprintln!("warning: failed to load {}: {e}", suite_decl.id);
                    issues.push(eval_harness::RunIssue::empty_suite(&suite_decl.id));
                }
            },
            "end-to-end" => {
                suites.push(Box::new(EndToEndSuite::new()));
            }
            "external-retrieval" => {
                let Some(root) = suite_decl.corpus_root.as_deref() else {
                    eprintln!("warning: external-retrieval requires corpus_root");
                    issues.push(eval_harness::RunIssue::empty_suite(&suite_decl.id));
                    continue;
                };
                let root = std::path::PathBuf::from(root);
                let manifest_path = root.join("manifest.json");
                let loaded = std::fs::read_to_string(&manifest_path)
                    .map_err(|e| format!("read {}: {e}", manifest_path.display()))
                    .and_then(|raw| CorpusManifest::parse(&raw).map_err(|e| e.to_string()))
                    .and_then(|manifest| {
                        let kind = DatasetKind::parse_name(&manifest.corpus_id)
                            .ok_or_else(|| format!("unsupported corpus {}", manifest.corpus_id))?;
                        let prepared = manifest.validate_at(&root).map_err(|e| e.to_string())?;
                        eval_harness::corpus::adapters::load_and_normalize(kind, &prepared)
                            .map(|cases| (kind, cases))
                            .map_err(|e| e.to_string())
                    });
                match loaded {
                    Ok((kind, cases)) => {
                        suites.push(Box::new(ExternalRetrievalSuite::new(kind, cases)))
                    }
                    Err(e) => {
                        eprintln!("warning: failed to load external corpus: {e}");
                        issues.push(eval_harness::RunIssue::empty_suite(&suite_decl.id));
                    }
                }
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
            "response-size" => match ResponseSizeSuite::new() {
                Ok(s) => suites.push(Box::new(s)),
                Err(e) => {
                    eprintln!("warning: failed to load response-size: {e}");
                    issues.push(eval_harness::RunIssue::empty_suite(&suite_decl.id));
                }
            },
            "ner-quality-anno"
            | "ner-quality-regex"
            | "ner-quality-anno-onnx"
            | "ner-quality-gliner"
            | "ner-quality-vago" => {
                if let Err(e) = ner_quality::register(&suite_decl.id, &mut suites) {
                    eprintln!("warning: failed to load {}: {e}", suite_decl.id);
                    issues.push(eval_harness::RunIssue::empty_suite(&suite_decl.id));
                }
            }
            other => {
                eprintln!("warning: unknown suite {other}");
                issues.push(eval_harness::RunIssue::empty_suite(other));
            }
        }
    }

    if suites.is_empty() {
        eprintln!("error: no suites to run");
        return std::process::ExitCode::from(2);
    }

    let suite_filter_set: BTreeSet<SuiteId> = suite_filter
        .iter()
        .filter_map(|s| SuiteId::parse(s).ok())
        .collect();

    let runner = Runner::new(suites);
    let request = RunRequest {
        manifest,
        manifest_path: profile,
        artifact_path: artifact.clone(),
        baseline: baseline_artifact,
        suite_filter: suite_filter_set,
        issues,
    };
    let result = runner.run(&request).await;

    match result {
        Ok(art) => {
            if let Err(e) = eval_harness::write_artifact(&artifact, &art) {
                eprintln!("error: failed to write artifact: {e}");
                return std::process::ExitCode::from(2);
            }

            let report = eval_harness::render_markdown(&art).unwrap_or_default();
            println!("{report}");

            print_summary(&art);

            match art.verdict {
                eval_harness::RunVerdict::Passed => {
                    eprintln!("RESULT: PASSED");
                    std::process::ExitCode::SUCCESS
                }
                eval_harness::RunVerdict::QualityFailed => {
                    eprintln!("RESULT: QUALITY FAILED");
                    std::process::ExitCode::from(1)
                }
                eval_harness::RunVerdict::Invalid => {
                    eprintln!("RESULT: INVALID");
                    std::process::ExitCode::from(2)
                }
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
            "gate={}/{} observed={:.4} status={}",
            gate.suite_id,
            gate.metric,
            gate.observed,
            format!("{:?}", gate.status).to_lowercase(),
        );
    }
}
