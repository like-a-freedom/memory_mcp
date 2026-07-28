use eval_harness::cli::{self, Command};
use eval_harness::{
    ActionGroundingSuite, CapacitySuite, ClaimReconciliationSuite, DownstreamQaSuite,
    EndToEndSuite, ExtractionSuite, LifecycleReleaseSuite, PoisoningSuite, ProfileManifest,
    RetrievalSuite, RunArtifact, Runner,
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
        } => {
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
