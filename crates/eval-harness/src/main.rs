use std::collections::BTreeSet;
use std::io::Write;
use std::time::Duration;

use eval_harness::cli::{self, Command};
use eval_harness::{
    CorpusManifest, ProfileManifest, RunArtifact, RunRequest, Runner, SuiteId, suites::registry,
};

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = cli::parse();

    match cli.command {
        Command::HashChatCompletionsTasks {
            tasks,
            model_endpoint,
            sampling_config,
            response_format_config,
            request_parameters_config,
        } => {
            let result = read_json_input::<
                eval_harness::paired_agent::managed::AgentManagedTaskManifest,
            >(&tasks, "agent-managed task manifest")
            .and_then(|manifest| {
                manifest
                    .fingerprints_for_chat_completions()
                    .map_err(|error| format!("invalid source-authority task manifest: {error}"))
            })
            .and_then(|fingerprints| {
                let mut output = serde_json::to_value(fingerprints)
                    .map_err(|error| format!("cannot serialize task fingerprints: {error}"))?;
                if let Some(path) = sampling_config.as_deref() {
                    let sampling = load_chat_completions_sampling(Some(path))?;
                    let sampling_sha256 = sampling
                        .fingerprint_sha256()
                        .map_err(|error| format!("invalid sampling configuration: {error}"))?;
                    let fields = output.as_object_mut().ok_or_else(|| {
                        "task fingerprints did not serialize as a JSON object".to_string()
                    })?;
                    fields.insert(
                        "chat_completions_sampling_config_sha256".into(),
                        serde_json::Value::String(sampling_sha256),
                    );
                }
                if let Some(path) = response_format_config.as_deref() {
                    let response_format = load_chat_completions_response_format(Some(path))?
                        .ok_or_else(|| "response-format configuration is missing".to_string())?;
                    let response_format_sha256 = eval_harness::paired_agent::chat_completions::response_format_config_sha256(&response_format)
                        .map_err(|error| format!("invalid Chat Completions response-format configuration: {error}"))?;
                    let fields = output.as_object_mut().ok_or_else(|| {
                        "task fingerprints did not serialize as a JSON object".to_string()
                    })?;
                    fields.insert(
                        "chat_completions_response_format_config_sha256".into(),
                        serde_json::Value::String(response_format_sha256),
                    );
                }
                if let Some(path) = request_parameters_config.as_deref() {
                    let parameters = load_chat_completions_request_parameters(Some(path))?
                        .ok_or_else(|| "request-parameters configuration is missing".to_string())?;
                    let parameters_sha256 = eval_harness::paired_agent::chat_completions::request_parameters_config_sha256(&parameters)
                        .map_err(|error| format!("invalid Chat Completions request-parameters configuration: {error}"))?;
                    let fields = output.as_object_mut().ok_or_else(|| {
                        "task fingerprints did not serialize as a JSON object".to_string()
                    })?;
                    fields.insert(
                        "chat_completions_request_parameters_config_sha256".into(),
                        serde_json::Value::String(parameters_sha256),
                    );
                }
                if let Some(endpoint) = model_endpoint.as_deref() {
                    let endpoint_sha256 =
                        eval_harness::paired_agent::chat_completions::endpoint_config_sha256(
                            endpoint,
                        )
                        .map_err(|error| format!("invalid model endpoint: {error}"))?;
                    let fields = output.as_object_mut().ok_or_else(|| {
                        "task fingerprints did not serialize as a JSON object".to_string()
                    })?;
                    fields.insert(
                        "chat_completions_endpoint_config_sha256".into(),
                        serde_json::Value::String(endpoint_sha256),
                    );
                }
                Ok(output)
            });
            match result {
                Ok(fingerprints) => match serde_json::to_string_pretty(&fingerprints) {
                    Ok(json) => {
                        println!("{json}");
                        std::process::ExitCode::SUCCESS
                    }
                    Err(error) => {
                        eprintln!("error: cannot serialize fingerprints: {error}");
                        std::process::ExitCode::from(2)
                    }
                },
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::ExitCode::from(2)
                }
            }
        }
        Command::HashAgentManagedTasks {
            tasks,
            memory_tool_profile,
        } => {
            let profile = match eval_harness::codex_agent::CodexMemoryToolProfile::from_name(
                &memory_tool_profile,
            ) {
                Ok(profile) => profile,
                Err(error) => {
                    eprintln!("error: {error}");
                    return std::process::ExitCode::from(2);
                }
            };
            let result = read_json_input::<
                eval_harness::paired_agent::managed::AgentManagedTaskManifest,
            >(&tasks, "agent-managed task manifest")
            .and_then(|manifest| {
                manifest
                    .fingerprints_for_profile(profile)
                    .map_err(|error| format!("invalid agent-managed task manifest: {error}"))
            });
            match result {
                Ok(fingerprints) => match serde_json::to_string_pretty(&fingerprints) {
                    Ok(json) => {
                        println!("{json}");
                        std::process::ExitCode::SUCCESS
                    }
                    Err(error) => {
                        eprintln!("error: cannot serialize fingerprints: {error}");
                        std::process::ExitCode::from(2)
                    }
                },
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::ExitCode::from(2)
                }
            }
        }
        Command::AcceptMemoryCandidate {
            plan,
            corpus_root,
            incumbent_first,
            incumbent_second,
            candidate_first,
            candidate_second,
            output,
        } => {
            use eval_harness::suites::{
                DeterministicAcceptancePlan, accept_candidate, load_registered_replay,
            };
            let result = (|| -> Result<_, String> {
                let p = read_json_input::<DeterministicAcceptancePlan>(
                    &plan,
                    "frozen acceptance plan",
                )?;
                let a = load_registered_replay(&incumbent_first, &corpus_root)
                    .map_err(|e| e.to_string())?;
                let aa = load_registered_replay(&incumbent_second, &corpus_root)
                    .map_err(|e| e.to_string())?;
                let b = load_registered_replay(&candidate_first, &corpus_root)
                    .map_err(|e| e.to_string())?;
                let bb = load_registered_replay(&candidate_second, &corpus_root)
                    .map_err(|e| e.to_string())?;
                let report =
                    accept_candidate(&p, [&a, &aa], [&b, &bb]).map_err(|e| e.to_string())?;
                let bytes = serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(output)
                    .map_err(|e| e.to_string())?;
                file.write_all(&bytes).map_err(|e| e.to_string())?;
                file.sync_all().map_err(|e| e.to_string())?;
                Ok(report)
            })();
            match result {
                Ok(report) => {
                    eprintln!(
                        "accepted={} wins={} losses={} hard_failures={}",
                        report.accepted,
                        report.wins.len(),
                        report.losses.len(),
                        report.hard_failures.len()
                    );
                    std::process::ExitCode::from(if report.accepted { 0 } else { 1 })
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::ExitCode::from(2)
                }
            }
        }
        Command::PrepareModel {
            manifest,
            output_root,
            mirror,
        } => {
            match eval_harness::model_preparation::prepare_model(
                &manifest,
                &output_root,
                mirror.as_deref(),
            )
            .await
            {
                Ok(path) => {
                    println!("{}", path.display());
                    std::process::ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::ExitCode::from(2)
                }
            }
        }
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
        Command::PrepareRetrievalQuality { sources, output } => {
            match eval_harness::suites::retrieval_quality::prepare_native_inventory(&sources).await
            {
                Ok(inventory) => match serde_json::to_vec_pretty(&inventory)
                    .map_err(std::io::Error::other)
                    .and_then(|bytes| std::fs::write(&output, bytes))
                {
                    Ok(()) => std::process::ExitCode::SUCCESS,
                    Err(error) => {
                        eprintln!(
                            "error: cannot write preparation {}: {error}",
                            output.display()
                        );
                        std::process::ExitCode::from(2)
                    }
                },
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::ExitCode::from(2)
                }
            }
        }
        Command::Merge {
            profile,
            artifact,
            shards,
        } => cmd_merge(profile, artifact, shards),
        Command::CompareDecisionReplays { left, right } => {
            use eval_harness::suites::{ReplayIndex, compare_replays};

            let replays = (|| {
                Ok::<_, String>((
                    read_json_input::<ReplayIndex>(&left, "left decision replay")?,
                    read_json_input::<ReplayIndex>(&right, "right decision replay")?,
                ))
            })();
            match replays {
                Ok((left, right)) => match compare_replays(&left, &right) {
                    Ok(()) => {
                        eprintln!("memory decision replays match");
                        std::process::ExitCode::SUCCESS
                    }
                    Err(error) => {
                        eprintln!("error: {error}");
                        std::process::ExitCode::from(1)
                    }
                },
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::ExitCode::from(2)
                }
            }
        }
        Command::ComparePaired {
            plan,
            incumbent,
            candidate,
            output,
        } => cmd_compare_paired(plan, incumbent, candidate, output),
        Command::CompareAgentManaged {
            plan,
            incumbent,
            candidate,
            incumbent_stages,
            candidate_stages,
            output,
        } => cmd_compare_agent_managed(
            plan,
            incumbent,
            candidate,
            incumbent_stages,
            candidate_stages,
            output,
        ),
        Command::ComparePairedDiagnostic {
            plan,
            incumbent,
            candidate,
            output,
        } => cmd_compare_paired_diagnostic(plan, incumbent, candidate, output),
        Command::SummarizePilot {
            plan,
            first_run_directory,
            second_run_directory,
            output,
        } => cmd_summarize_pilot(plan, first_run_directory, second_run_directory, output),
        Command::PlanPairedPower {
            pilot_report,
            stratum_id,
            target_risk_difference,
            minimum_point_effect,
            one_sided_alpha,
            target_power,
            confidence_level,
            maximum_sample_size,
            output,
        } => cmd_plan_paired_power(
            pilot_report,
            stratum_id,
            eval_harness::paired_power::ExactMcNemarPowerConfig {
                target_risk_difference,
                minimum_point_effect,
                one_sided_alpha,
                target_power,
                confidence_level,
                maximum_sample_size,
            },
            output,
        ),
        Command::SummarizePairedReliability {
            plan,
            incumbent,
            candidate,
            output,
        } => cmd_summarize_paired_reliability(plan, incumbent, candidate, output),
        Command::CaptureIndependentProbe {
            transport,
            input,
            history,
            server_binary,
            product_sha256,
            store,
            claim_rollout_stage,
            output,
        } => {
            let result = async {
                let input = read_json_input(&input, "independent probe input")?;
                let history: Vec<eval_harness::paired_agent::SeedEpisode> =
                    read_json_input(&history, "independent probe history")?;
                let capture = match transport.as_str() {
                    "stdio" => eval_harness::independent_probe::capture_stdio_probe(
                        &input,
                        &history,
                        &server_binary,
                        &product_sha256,
                        &store,
                        &claim_rollout_stage,
                    ),
                    "streamable-http" => {
                        eval_harness::independent_probe::capture_http_probe(
                            &input,
                            &history,
                            &server_binary,
                            &product_sha256,
                            &store,
                            &claim_rollout_stage,
                        )
                        .await
                    }
                    _ => return Err("unsupported independent probe transport".to_string()),
                }
                .map_err(|error| error.to_string())?;
                let bytes =
                    serde_json::to_vec_pretty(&capture).map_err(|error| error.to_string())?;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(output)
                    .map_err(|error| error.to_string())?;
                file.write_all(&bytes).map_err(|error| error.to_string())
            }
            .await;
            match result {
                Ok(()) => std::process::ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::ExitCode::from(2)
                }
            }
        }
        Command::ScoreIndependentProbe {
            input,
            oracle,
            observation,
            product_sha256,
            output,
        } => {
            let result = (|| -> Result<(), String> {
                let input = read_json_input(&input, "independent probe input")?;
                let oracle = read_json_input(&oracle, "independent probe oracle")?;
                let observation = read_json_input(&observation, "independent probe observation")?;
                let score = eval_harness::independent_probe::score_probe(
                    &input,
                    &oracle,
                    Some(&observation),
                    &product_sha256,
                )
                .map_err(|error| error.to_string())?;
                let bytes = serde_json::to_vec_pretty(&score).map_err(|error| error.to_string())?;
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(output)
                    .map_err(|error| error.to_string())?;
                file.write_all(&bytes).map_err(|error| error.to_string())
            })();
            match result {
                Ok(()) => std::process::ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::ExitCode::from(2)
                }
            }
        }
        Command::ExportPairedInputs {
            plan,
            tasks,
            output,
        } => {
            use eval_harness::paired_agent::observations::export_paired_inputs;

            let result = (|| {
                let plan = read_json_input(&plan, "paired plan")?;
                let tasks = read_json_input(&tasks, "paired tasks")?;
                export_paired_inputs(&plan, &tasks, &output).map_err(|error| error.to_string())
            })();
            match result {
                Ok(()) => std::process::ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::ExitCode::from(2)
                }
            }
        }
        Command::ScorePairedObservations {
            plan,
            tasks,
            observations,
            output_directory,
        } => {
            use eval_harness::paired_agent::observations::score_recorded_pair;

            let result = (|| {
                let plan = read_json_input(&plan, "paired plan")?;
                let tasks = read_json_input(&tasks, "paired tasks")?;
                let observations = read_json_input(&observations, "recorded observations")?;
                score_recorded_pair(&plan, &tasks, &observations, &output_directory)
                    .map_err(|error| error.to_string())
            })();
            match result {
                Ok(()) => std::process::ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::ExitCode::from(2)
                }
            }
        }
        Command::RunPairedCodex {
            plan,
            tasks,
            codex_binary,
            independent_probes,
            codex_home,
            incumbent_memory_server,
            candidate_memory_server,
            output_directory,
            temp_root,
            claim_rollout_stage,
            memory_tool_profile,
        } => {
            let memory_tool_profile =
                match eval_harness::codex_agent::CodexMemoryToolProfile::from_name(
                    &memory_tool_profile,
                ) {
                    Ok(profile) => profile,
                    Err(error) => {
                        eprintln!("error: {error}");
                        return std::process::ExitCode::from(2);
                    }
                };
            cmd_run_paired_codex(
                plan,
                tasks,
                codex_binary,
                independent_probes,
                codex_home,
                incumbent_memory_server,
                candidate_memory_server,
                output_directory,
                temp_root,
                claim_rollout_stage,
                memory_tool_profile,
            )
            .await
        }
        Command::RunPairedChatCompletions {
            recall_read_mode,
            fixed_read_inputs,
            plan,
            tasks,
            model_endpoint,
            sampling_config,
            response_format_config,
            request_parameters_config,
            incumbent_memory_server,
            candidate_memory_server,
            output_directory,
            temp_root,
        } => {
            cmd_run_paired_chat_completions(
                recall_read_mode,
                fixed_read_inputs,
                plan,
                tasks,
                model_endpoint,
                sampling_config,
                response_format_config,
                request_parameters_config,
                incumbent_memory_server,
                candidate_memory_server,
                output_directory,
                temp_root,
            )
            .await
        }
        Command::CalibrateReranking { pairs, output } => {
            match eval_harness::reranker_calibration::calibrate_native(&pairs).await {
                Ok(report) => match serde_json::to_vec_pretty(&report)
                    .map_err(std::io::Error::other)
                    .and_then(|bytes| std::fs::write(&output, bytes))
                {
                    Ok(()) => std::process::ExitCode::SUCCESS,
                    Err(error) => {
                        eprintln!("error: cannot write calibration: {error}");
                        std::process::ExitCode::from(2)
                    }
                },
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::ExitCode::from(2)
                }
            }
        }
    }
}

fn cmd_compare_paired(
    plan_path: std::path::PathBuf,
    incumbent_path: std::path::PathBuf,
    candidate_path: std::path::PathBuf,
    output_path: std::path::PathBuf,
) -> std::process::ExitCode {
    let plan = match read_json_input::<eval_harness::paired::PairedEvaluationPlan>(
        &plan_path,
        "paired plan",
    ) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("error: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    let incumbent = match read_json_input::<eval_harness::paired::PairedRunArtifact>(
        &incumbent_path,
        "incumbent run",
    ) {
        Ok(artifact) => artifact,
        Err(error) => {
            eprintln!("error: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    let candidate = match read_json_input::<eval_harness::paired::PairedRunArtifact>(
        &candidate_path,
        "candidate run",
    ) {
        Ok(artifact) => artifact,
        Err(error) => {
            eprintln!("error: {error}");
            return std::process::ExitCode::from(2);
        }
    };

    let comparison = match eval_harness::paired::compare_paired_runs(&incumbent, &candidate, &plan)
    {
        Ok(comparison) => comparison,
        Err(error) => {
            eprintln!("error: invalid paired evaluation: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    let promotion_criteria = match plan.promotion_criteria.as_ref() {
        Some(criteria) => criteria,
        None => {
            eprintln!(
                "error: this plan has no promotion criteria; use compare-paired-diagnostic for exploratory results"
            );
            return std::process::ExitCode::from(2);
        }
    };
    let promotion = match eval_harness::paired::evaluate_promotion(&comparison, promotion_criteria)
    {
        Ok(promotion) => promotion,
        Err(error) => {
            eprintln!("error: paired promotion gate failed to evaluate: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    let report = eval_harness::paired::PairedEvaluationReport {
        comparison,
        promotion,
    };
    if let Err(error) = report.validate() {
        eprintln!("error: paired report failed its self-check: {error}");
        return std::process::ExitCode::from(2);
    }
    let bytes = match serde_json::to_vec_pretty(&report) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("error: cannot serialize paired evaluation report: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    if let Err(error) = std::fs::write(&output_path, bytes) {
        eprintln!(
            "error: cannot write paired evaluation report {}: {error}",
            output_path.display()
        );
        return std::process::ExitCode::from(2);
    }

    eprintln!(
        "paired evaluation wrote {} ({} trials, {} independent families)",
        output_path.display(),
        report.comparison.paired_trial_count,
        report.comparison.independent_family_count
    );
    match report.promotion.decision {
        eval_harness::paired::PromotionDecision::Promote => {
            eprintln!("RESULT: PROMOTE");
            std::process::ExitCode::SUCCESS
        }
        eval_harness::paired::PromotionDecision::Reject => {
            eprintln!("RESULT: REJECT");
            std::process::ExitCode::from(1)
        }
        eval_harness::paired::PromotionDecision::InsufficientEvidence => {
            eprintln!("RESULT: INSUFFICIENT EVIDENCE");
            std::process::ExitCode::from(3)
        }
    }
}

fn cmd_compare_agent_managed(
    plan_path: std::path::PathBuf,
    incumbent_path: std::path::PathBuf,
    candidate_path: std::path::PathBuf,
    incumbent_stages_path: std::path::PathBuf,
    candidate_stages_path: std::path::PathBuf,
    output_path: std::path::PathBuf,
) -> std::process::ExitCode {
    let result = (|| -> Result<(), String> {
        let plan = read_json_input::<eval_harness::paired::PairedEvaluationPlan>(
            &plan_path,
            "paired plan",
        )?;
        let incumbent = read_json_input::<eval_harness::paired::PairedRunArtifact>(
            &incumbent_path,
            "incumbent run",
        )?;
        let candidate = read_json_input::<eval_harness::paired::PairedRunArtifact>(
            &candidate_path,
            "candidate run",
        )?;
        let incumbent_stages = read_json_input::<
            eval_harness::paired_agent::managed::AgentManagedRunSupplement,
        >(&incumbent_stages_path, "incumbent agent-managed stages")?;
        let candidate_stages = read_json_input::<
            eval_harness::paired_agent::managed::AgentManagedRunSupplement,
        >(&candidate_stages_path, "candidate agent-managed stages")?;
        let report = eval_harness::paired_agent::managed::compare_agent_managed_runs(
            &plan,
            &incumbent,
            &candidate,
            &incumbent_stages,
            &candidate_stages,
        )
        .map_err(|error| format!("invalid agent-managed paired evaluation: {error}"))?;
        let bytes = serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output_path)
            .map_err(|error| format!("cannot create report {}: {error}", output_path.display()))?;
        use std::io::Write;
        file.write_all(&bytes)
            .and_then(|()| file.write_all(b"\n"))
            .map_err(|error| format!("cannot write report {}: {error}", output_path.display()))
    })();
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::ExitCode::from(2)
        }
    }
}

fn cmd_compare_paired_diagnostic(
    plan_path: std::path::PathBuf,
    incumbent_path: std::path::PathBuf,
    candidate_path: std::path::PathBuf,
    output_path: std::path::PathBuf,
) -> std::process::ExitCode {
    use eval_harness::paired::{
        PairedEvaluationPlan, PairedEvaluationStage, PairedRunArtifact, compare_paired_runs,
    };

    let result = (|| {
        let plan: PairedEvaluationPlan = read_json_input(&plan_path, "paired plan")?;
        if plan.stage != PairedEvaluationStage::ExploratoryComparison {
            return Err(format!(
                "compare-paired-diagnostic requires the exploratory_comparison plan stage, found {:?}",
                plan.stage
            ));
        }
        let incumbent: PairedRunArtifact = read_json_input(&incumbent_path, "incumbent run")?;
        let candidate: PairedRunArtifact = read_json_input(&candidate_path, "candidate run")?;
        let comparison = compare_paired_runs(&incumbent, &candidate, &plan)
            .map_err(|error| format!("invalid paired diagnostic: {error}"))?;
        let bytes = serde_json::to_vec_pretty(&comparison)
            .map_err(|error| format!("cannot serialize paired diagnostic: {error}"))?;
        std::fs::write(&output_path, bytes).map_err(|error| {
            format!(
                "cannot write paired diagnostic {}: {error}",
                output_path.display()
            )
        })?;
        Ok::<_, String>(comparison)
    })();
    match result {
        Ok(comparison) => {
            eprintln!(
                "paired diagnostic wrote {} ({} trials, {} independent families; no promotion decision)",
                output_path.display(),
                comparison.paired_trial_count,
                comparison.independent_family_count
            );
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            std::process::ExitCode::from(2)
        }
    }
}

fn cmd_summarize_pilot(
    plan_path: std::path::PathBuf,
    first_run_directory: std::path::PathBuf,
    second_run_directory: std::path::PathBuf,
    output_path: std::path::PathBuf,
) -> std::process::ExitCode {
    let plan = match read_json_input::<eval_harness::paired::PairedEvaluationPlan>(
        &plan_path,
        "paired pilot plan",
    ) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("error: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    let read_artifact = |directory: &std::path::Path, name: &str| {
        read_json_input::<eval_harness::paired::PairedRunArtifact>(
            &directory.join(name),
            "paired pilot run artifact",
        )
    };
    let first_incumbent = match read_artifact(&first_run_directory, "incumbent.json") {
        Ok(artifact) => artifact,
        Err(error) => {
            eprintln!("error: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    let first_candidate = match read_artifact(&first_run_directory, "candidate.json") {
        Ok(artifact) => artifact,
        Err(error) => {
            eprintln!("error: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    let second_incumbent = match read_artifact(&second_run_directory, "incumbent.json") {
        Ok(artifact) => artifact,
        Err(error) => {
            eprintln!("error: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    let second_candidate = match read_artifact(&second_run_directory, "candidate.json") {
        Ok(artifact) => artifact,
        Err(error) => {
            eprintln!("error: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    let serialized_report = match plan.stage {
        eval_harness::paired::PairedEvaluationStage::DevelopmentPilot => {
            eval_harness::paired::summarize_development_pilot(
                &plan,
                &first_incumbent,
                &first_candidate,
                &second_incumbent,
                &second_candidate,
            )
            .map_err(|error| format!("invalid development pilot: {error}"))
            .and_then(|report| {
                serde_json::to_vec_pretty(&report)
                    .map_err(|error| format!("cannot serialize development pilot report: {error}"))
            })
        }
        eval_harness::paired::PairedEvaluationStage::ExploratoryComparison => {
            eval_harness::paired::summarize_exploratory_pilot(
                &plan,
                &first_incumbent,
                &first_candidate,
                &second_incumbent,
                &second_candidate,
            )
            .map_err(|error| format!("invalid exploratory pilot: {error}"))
            .and_then(|report| {
                serde_json::to_vec_pretty(&report)
                    .map_err(|error| format!("cannot serialize exploratory pilot report: {error}"))
            })
        }
        eval_harness::paired::PairedEvaluationStage::Confirmatory => {
            Err("summarize-pilot does not accept confirmatory plans".into())
        }
    };
    let bytes = match serialized_report {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("error: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    match std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&output_path)
        .and_then(|mut file| file.write_all(&bytes))
    {
        Ok(()) => {
            eprintln!(
                "paired pilot report wrote {} ({} preregistered independent families)",
                output_path.display(),
                plan.planned_independent_families
            );
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!(
                "error: cannot create pilot report {}: {error}",
                output_path.display()
            );
            std::process::ExitCode::from(2)
        }
    }
}

fn cmd_plan_paired_power(
    pilot_report_path: std::path::PathBuf,
    stratum_id: String,
    config: eval_harness::paired_power::ExactMcNemarPowerConfig,
    output_path: std::path::PathBuf,
) -> std::process::ExitCode {
    let pilot = match read_json_input::<eval_harness::paired::ExploratoryPilotReport>(
        &pilot_report_path,
        "exploratory pilot report",
    ) {
        Ok(pilot) => pilot,
        Err(error) => {
            eprintln!("error: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    let report =
        match eval_harness::paired_power::plan_paired_pilot_power(&pilot, &stratum_id, config) {
            Ok(report) => report,
            Err(error) => {
                eprintln!("error: {error}");
                return std::process::ExitCode::from(2);
            }
        };
    let bytes = match serde_json::to_vec_pretty(&report) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("cannot serialize paired power report: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    match std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&output_path)
        .and_then(|mut file| file.write_all(&bytes))
    {
        Ok(()) => {
            eprintln!(
                "paired power analysis wrote {} ({} discordant of {} independent families; no promotion decision)",
                output_path.display(),
                report.discordant_family_count,
                report.independent_family_count
            );
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!(
                "cannot write paired power report {}: {error}",
                output_path.display()
            );
            std::process::ExitCode::from(2)
        }
    }
}

fn cmd_summarize_paired_reliability(
    plan_path: std::path::PathBuf,
    incumbent_path: std::path::PathBuf,
    candidate_path: std::path::PathBuf,
    output_path: std::path::PathBuf,
) -> std::process::ExitCode {
    let result = (|| -> Result<_, String> {
        let plan = read_json_input::<eval_harness::paired::PairedEvaluationPlan>(
            &plan_path,
            "paired reliability plan",
        )?;
        let incumbent = read_json_input::<eval_harness::paired::PairedRunArtifact>(
            &incumbent_path,
            "paired reliability incumbent",
        )?;
        let candidate = read_json_input::<eval_harness::paired::PairedRunArtifact>(
            &candidate_path,
            "paired reliability candidate",
        )?;
        let report = eval_harness::paired_reliability::summarize_paired_reliability(
            &plan, &incumbent, &candidate,
        )
        .map_err(|error| format!("invalid paired reliability inputs: {error}"))?;
        let bytes = serde_json::to_vec_pretty(&report)
            .map_err(|error| format!("cannot serialize paired reliability report: {error}"))?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output_path)
            .map_err(|error| format!("cannot create {}: {error}", output_path.display()))?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("cannot write {}: {error}", output_path.display()))?;
        Ok(report)
    })();

    match result {
        Ok(report) => {
            eprintln!(
                "paired reliability wrote {} (k={}, {} independent families)",
                output_path.display(),
                report.repetitions_per_case,
                report.overall.task_success.independent_family_count
            );
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            std::process::ExitCode::from(2)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn cmd_run_paired_codex(
    plan_path: std::path::PathBuf,
    tasks_path: std::path::PathBuf,
    codex_binary: std::path::PathBuf,
    independent_probes_path: Option<std::path::PathBuf>,
    local_codex_home: Option<std::path::PathBuf>,
    incumbent_memory_server: std::path::PathBuf,
    candidate_memory_server: std::path::PathBuf,
    output_directory: std::path::PathBuf,
    temp_root: Option<std::path::PathBuf>,
    claim_rollout_stage: Option<String>,
    memory_tool_profile: eval_harness::codex_agent::CodexMemoryToolProfile,
) -> std::process::ExitCode {
    let plan = match read_json_input::<eval_harness::paired::PairedEvaluationPlan>(
        &plan_path,
        "paired plan",
    ) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("error: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    let (tasks, managed_tasks) = match memory_tool_profile {
        eval_harness::codex_agent::CodexMemoryToolProfile::RecallOnly => {
            let tasks = match read_json_input::<eval_harness::paired_agent::PairedTaskManifest>(
                &tasks_path,
                "paired task manifest",
            ) {
                Ok(tasks) => tasks,
                Err(error) => {
                    eprintln!("error: {error}");
                    return std::process::ExitCode::from(2);
                }
            };
            if let Err(error) = tasks.validate(&plan) {
                eprintln!("error: invalid paired agent plan or task manifest: {error}");
                return std::process::ExitCode::from(2);
            }
            (Some(tasks), None)
        }
        eval_harness::codex_agent::CodexMemoryToolProfile::AgentManagedMemory
        | eval_harness::codex_agent::CodexMemoryToolProfile::AgentSourceAuthority
        | eval_harness::codex_agent::CodexMemoryToolProfile::AgentSourceAuthorityLiteralInput
        | eval_harness::codex_agent::CodexMemoryToolProfile::AgentMaintenance
        | eval_harness::codex_agent::CodexMemoryToolProfile::AgentMaintenanceV2
        | eval_harness::codex_agent::CodexMemoryToolProfile::AgentMaintenanceV3
        | eval_harness::codex_agent::CodexMemoryToolProfile::AgentMaintenanceV4 => {
            if independent_probes_path.is_some() {
                eprintln!(
                    "error: --independent-probes is incompatible with agent-managed tasks; use the frozen post-write probe in the task manifest"
                );
                return std::process::ExitCode::from(2);
            }
            let tasks = match read_json_input::<
                eval_harness::paired_agent::managed::AgentManagedTaskManifest,
            >(&tasks_path, "agent-managed task manifest")
            {
                Ok(tasks) => tasks,
                Err(error) => {
                    eprintln!("error: {error}");
                    return std::process::ExitCode::from(2);
                }
            };
            if let Err(error) = tasks.validate(&plan) {
                eprintln!("error: invalid agent-managed plan or task manifest: {error}");
                return std::process::ExitCode::from(2);
            }
            if let Err(error) = tasks.validate_for_profile(memory_tool_profile) {
                eprintln!("error: agent-managed workflow profile mismatch: {error}");
                return std::process::ExitCode::from(2);
            }
            (None, Some(tasks))
        }
        eval_harness::codex_agent::CodexMemoryToolProfile::AgentFormation
        | eval_harness::codex_agent::CodexMemoryToolProfile::AgentOperatorPrepared => {
            eprintln!("error: agent formation is an internal phase profile, not a paired-run mode");
            return std::process::ExitCode::from(2);
        }
    };
    let (incumbent_claim_rollout_stage, candidate_claim_rollout_stage) =
        match plan.claim_rollout_stages(claim_rollout_stage.as_deref()) {
            Ok(stages) => stages,
            Err(error) => {
                eprintln!("error: invalid paired claim rollout stages: {error}");
                return std::process::ExitCode::from(2);
            }
        };
    let independent_probes = match independent_probes_path
        .as_deref()
        .map(|p| {
            read_json_input::<Vec<eval_harness::paired_agent::independent::FixedTrialProbe>>(
                p,
                "independent probes",
            )
        })
        .transpose()
    {
        Ok(p) => p,
        Err(error) => {
            eprintln!("error: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    let probe_binaries = (
        incumbent_memory_server.clone(),
        candidate_memory_server.clone(),
    );
    let shared_model = plan.shared_identity.model.clone();
    let codex_api_key = std::env::var_os("CODEX_API_KEY").filter(|value| !value.is_empty());
    let (codex_api_key, local_codex_home) = match (codex_api_key, local_codex_home) {
        (Some(_), Some(_)) => {
            eprintln!("error: choose either CODEX_API_KEY or --codex-home, not both");
            return std::process::ExitCode::from(2);
        }
        (Some(api_key), None) => (Some(api_key), None),
        (None, Some(home)) if home.is_absolute() && home.is_dir() => (None, Some(home)),
        (None, Some(_)) => {
            eprintln!("error: --codex-home must be an existing absolute directory");
            return std::process::ExitCode::from(2);
        }
        (None, None) => {
            eprintln!("error: set CODEX_API_KEY or pass --codex-home for an existing Codex login");
            return std::process::ExitCode::from(2);
        }
    };
    let transport = match eval_harness::paired_agent::codex::PairedAgentTransport::from_identity(
        &plan.shared_identity.transport,
    ) {
        Ok(transport) => transport,
        Err(error) => {
            eprintln!("error: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    let runtime = |memory_server_binary, claim_rollout_stage| {
        eval_harness::paired_agent::codex::CodexArmRuntime {
            codex_binary: codex_binary.clone(),
            memory_server_binary,
            model: shared_model.clone(),
            temp_root: temp_root.clone(),
            claim_rollout_stage,
            transport,
            memory_tool_profile,
            codex_api_key: codex_api_key.clone(),
            local_codex_home: local_codex_home.clone(),
        }
    };
    let incumbent_runtime = runtime(incumbent_memory_server, incumbent_claim_rollout_stage);
    let candidate_runtime = runtime(candidate_memory_server, candidate_claim_rollout_stage);
    let mut backend = match eval_harness::paired_agent::codex::CodexPairedBackend::new(
        &plan,
        incumbent_runtime,
        candidate_runtime,
    ) {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("error: paired Codex preflight failed: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    let worker_output_directory = output_directory.clone();
    let (incumbent, candidate) = match tokio::task::spawn_blocking(move || {
        if let Some(tasks) = managed_tasks {
            let (incumbent, candidate, _, _) =
                eval_harness::paired_agent::managed::run_agent_managed_paired_evaluation(
                    &plan,
                    &tasks,
                    &mut backend,
                    &worker_output_directory,
                )?;
            Ok((incumbent, candidate))
        } else if let Some(probes) = independent_probes {
            let tasks = tasks.ok_or_else(|| {
                eval_harness::EvalError::InvalidInput(
                    "paired task manifest was not loaded for the recall-only run".into(),
                )
            })?;
            let (left, right) = eval_harness::paired_agent::independent::run_with_public_probes(
                &plan,
                &tasks,
                &probes,
                &mut backend,
                probe_binaries,
                &worker_output_directory,
            )?;
            Ok((left.agent_observed, right.agent_observed))
        } else {
            let tasks = tasks.ok_or_else(|| {
                eval_harness::EvalError::InvalidInput(
                    "paired task manifest was not loaded for the recall-only run".into(),
                )
            })?;
            eval_harness::paired_agent::run_paired_agent_evaluation(
                &plan,
                &tasks,
                &mut backend,
                &worker_output_directory,
            )
        }
    })
    .await
    {
        Err(error) => {
            eprintln!("error: paired agent worker failed: {error}");
            return std::process::ExitCode::from(2);
        }
        Ok(Err(error)) => {
            eprintln!("error: paired agent execution failed: {error}");
            return std::process::ExitCode::from(2);
        }
        Ok(Ok(artifacts)) => artifacts,
    };
    eprintln!(
        "paired agent run wrote {} ({} trials per arm)",
        output_directory.display(),
        incumbent.receipts.len()
    );
    let incumbent_successes = incumbent
        .receipts
        .iter()
        .filter(|receipt| receipt.task_success())
        .count();
    let candidate_successes = candidate
        .receipts
        .iter()
        .filter(|receipt| receipt.task_success())
        .count();
    eprintln!(
        "task success: incumbent {incumbent_successes}/{}, candidate {candidate_successes}/{}; run comparison separately",
        incumbent.receipts.len(),
        candidate.receipts.len()
    );
    std::process::ExitCode::SUCCESS
}

fn read_json_input<T: serde::de::DeserializeOwned>(
    path: &std::path::Path,
    label: &str,
) -> Result<T, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {label} {}: {error}", path.display()))?;
    serde_json::from_str(&raw)
        .map_err(|error| format!("cannot parse {label} {}: {error}", path.display()))
}

struct StderrProgressReporter;

impl eval_harness::ProgressReporter for StderrProgressReporter {
    fn suite_started(&self, position: usize, total: usize, suite_id: &str, expected_cases: usize) {
        eprintln!("suite {position}/{total}: {suite_id} started ({expected_cases} expected cases)");
    }

    fn suite_finished(
        &self,
        position: usize,
        total: usize,
        suite_id: &str,
        outcome_count: usize,
        elapsed: Duration,
    ) {
        eprintln!(
            "suite {position}/{total}: {suite_id} finished ({outcome_count} cases, {} ms)",
            elapsed.as_millis()
        );
    }
}

#[allow(clippy::too_many_arguments)]
async fn cmd_run_paired_chat_completions(
    recall_read_mode: Option<
        eval_harness::paired_agent::chat_completions_backend::RecallCaptureMode,
    >,
    fixed_read_inputs: Option<std::path::PathBuf>,
    plan_path: std::path::PathBuf,
    tasks_path: std::path::PathBuf,
    model_endpoint: String,
    sampling_config_path: Option<std::path::PathBuf>,
    response_format_config_path: Option<std::path::PathBuf>,
    request_parameters_config_path: Option<std::path::PathBuf>,
    incumbent_memory_server: std::path::PathBuf,
    candidate_memory_server: std::path::PathBuf,
    output_directory: std::path::PathBuf,
    temp_root: Option<std::path::PathBuf>,
) -> std::process::ExitCode {
    let plan = match read_json_input::<eval_harness::paired::PairedEvaluationPlan>(
        &plan_path,
        "paired plan",
    ) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("error: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    use eval_harness::paired_agent::chat_completions_backend::{
        ChatCompletionsPairedBackend, ChatCompletionsRecallBackend, RecallRegistration,
    };
    let registration = match recall_read_mode
        .map(|mode| {
            let inputs = fixed_read_inputs
                .as_deref()
                .map(|path| read_json_input(path, "fixed read inputs"))
                .transpose()?;
            RecallRegistration::new(mode, inputs, &plan).map_err(|error| error.to_string())
        })
        .transpose()
    {
        Ok(registration) => registration,
        Err(error) => {
            eprintln!("error: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    let sampling = match load_chat_completions_sampling(sampling_config_path.as_deref()) {
        Ok(sampling) => sampling,
        Err(error) => {
            eprintln!("error: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    let response_format =
        match load_chat_completions_response_format(response_format_config_path.as_deref()) {
            Ok(response_format) => response_format,
            Err(error) => {
                eprintln!("error: {error}");
                return std::process::ExitCode::from(2);
            }
        };
    let request_parameters =
        match load_chat_completions_request_parameters(request_parameters_config_path.as_deref()) {
            Ok(parameters) => parameters,
            Err(error) => {
                eprintln!("error: {error}");
                return std::process::ExitCode::from(2);
            }
        };
    let (incumbent_claim_rollout_stage, candidate_claim_rollout_stage) =
        match plan.claim_rollout_stages(None) {
            Ok(stages) => stages,
            Err(error) => {
                eprintln!("error: invalid paired claim rollout stages: {error}");
                return std::process::ExitCode::from(2);
            }
        };
    if let Err(error) = std::fs::create_dir_all(&output_directory) {
        eprintln!("error: cannot create paired output directory: {error}");
        return std::process::ExitCode::from(2);
    }
    let bearer_token = std::env::var("MEMORY_EVAL_BEARER_TOKEN")
        .ok()
        .filter(|token| !token.is_empty());
    let incumbent_runtime =
        eval_harness::paired_agent::chat_completions_backend::ChatCompletionsArmRuntime {
            memory_server_binary: incumbent_memory_server,
            endpoint: model_endpoint.clone(),
            bearer_token: bearer_token.clone(),
            temp_root: temp_root.clone(),
            claim_rollout_stage: incumbent_claim_rollout_stage,
        };
    let candidate_runtime =
        eval_harness::paired_agent::chat_completions_backend::ChatCompletionsArmRuntime {
            memory_server_binary: candidate_memory_server,
            endpoint: model_endpoint,
            bearer_token,
            temp_root,
            claim_rollout_stage: candidate_claim_rollout_stage,
        };
    let worker_output_directory = output_directory.clone();
    let worker = tokio::task::spawn_blocking(move || {
        if let Some(registration) = registration {
            let tasks = read_json_input(&tasks_path, "recall task manifest")
                .map_err(eval_harness::EvalError::InvalidInput)?;
            let mut backend = ChatCompletionsRecallBackend::new(
                &plan,
                &tasks,
                registration,
                incumbent_runtime,
                candidate_runtime,
                sampling,
                response_format,
                request_parameters,
            )?;
            return eval_harness::paired_agent::run_paired_agent_evaluation(
                &plan,
                &tasks,
                &mut backend,
                &worker_output_directory,
            );
        }
        let tasks: eval_harness::paired_agent::managed::AgentManagedTaskManifest =
            read_json_input(&tasks_path, "agent-managed task manifest")
                .map_err(eval_harness::EvalError::InvalidInput)?;
        tasks.validate(&plan)?;
        tasks.validate_for_source_authority_workflow()?;
        let mut backend = ChatCompletionsPairedBackend::new_with_request_parameters(
            &plan,
            &tasks,
            incumbent_runtime,
            candidate_runtime,
            sampling,
            response_format,
            request_parameters,
        )?;
        let (incumbent, candidate, _, _) =
            eval_harness::paired_agent::managed::run_agent_managed_paired_evaluation(
                &plan,
                &tasks,
                &mut backend,
                &worker_output_directory,
            )?;
        Ok::<_, eval_harness::EvalError>((incumbent, candidate))
    })
    .await;
    let (incumbent, candidate) = match worker {
        Err(error) => {
            eprintln!("error: local paired-agent worker failed: {error}");
            return std::process::ExitCode::from(2);
        }
        Ok(Err(error)) => {
            eprintln!("error: local paired-agent execution failed: {error}");
            return std::process::ExitCode::from(2);
        }
        Ok(Ok(artifacts)) => artifacts,
    };
    let incumbent_successes = incumbent
        .receipts
        .iter()
        .filter(|receipt| receipt.task_success())
        .count();
    let candidate_successes = candidate
        .receipts
        .iter()
        .filter(|receipt| receipt.task_success())
        .count();
    eprintln!(
        "local paired agent run wrote {} ({} cases per arm); task success: incumbent {incumbent_successes}/{}, candidate {candidate_successes}/{}",
        output_directory.display(),
        incumbent.receipts.len(),
        incumbent.receipts.len(),
        candidate.receipts.len()
    );
    std::process::ExitCode::SUCCESS
}

fn load_chat_completions_sampling(
    path: Option<&std::path::Path>,
) -> Result<eval_harness::paired_agent::chat_completions::ChatCompletionsSampling, String> {
    let Some(path) = path else {
        return Ok(Default::default());
    };
    let sampling: eval_harness::paired_agent::chat_completions::ChatCompletionsSampling =
        read_json_input(path, "Chat Completions sampling configuration")?;
    sampling
        .validate()
        .map_err(|error| format!("invalid Chat Completions sampling configuration: {error}"))?;
    Ok(sampling)
}

fn load_chat_completions_response_format(
    path: Option<&std::path::Path>,
) -> Result<Option<serde_json::Value>, String> {
    let Some(path) = path else {
        return Ok(None);
    };
    let response_format: serde_json::Value =
        read_json_input(path, "Chat Completions response-format configuration")?;
    Ok(Some(response_format))
}

fn load_chat_completions_request_parameters(
    path: Option<&std::path::Path>,
) -> Result<Option<serde_json::Value>, String> {
    let Some(path) = path else {
        return Ok(None);
    };
    let parameters: serde_json::Value =
        read_json_input(path, "Chat Completions request-parameters configuration")?;
    eval_harness::paired_agent::chat_completions::request_parameters_config_sha256(&parameters)
        .map_err(|error| {
            format!("invalid Chat Completions request-parameters configuration: {error}")
        })?;
    Ok(Some(parameters))
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

    let baseline_artifact = match baseline {
        Some(path) => {
            let raw = match std::fs::read_to_string(&path) {
                Ok(raw) => raw,
                Err(error) => {
                    eprintln!("error: failed to read baseline {}: {error}", path.display());
                    return std::process::ExitCode::from(2);
                }
            };
            match serde_json::from_str::<RunArtifact>(&raw) {
                Ok(artifact) => {
                    if artifact.schema_version != eval_harness::artifact::EVAL_ARTIFACT_SCHEMA_V2
                        || artifact.profile != manifest.profile
                        || artifact.validate().is_err()
                    {
                        eprintln!(
                            "error: baseline {} is incompatible with profile {:?}",
                            path.display(),
                            manifest.profile
                        );
                        return std::process::ExitCode::from(2);
                    }
                    Some(artifact)
                }
                Err(error) => {
                    eprintln!("error: invalid baseline {}: {error}", path.display());
                    return std::process::ExitCode::from(2);
                }
            }
        }
        None => None,
    };

    if manifest
        .gates
        .iter()
        .filter(|gate| {
            suite_filter.is_empty()
                || suite_filter
                    .iter()
                    .any(|suite| suite == gate.target.suite_id.as_str())
        })
        .any(|gate| gate.baseline_required || gate.regression_budget.is_some())
        && baseline_artifact.is_none()
    {
        eprintln!("error: profile contains regression budgets but no baseline was supplied");
        return std::process::ExitCode::from(2);
    }

    let mut suites: Vec<Box<dyn eval_harness::runner::EvalSuite>> = Vec::new();
    let mut issues = Vec::new();

    for suite_decl in &manifest.suites {
        if !suite_filter.is_empty() && !suite_filter.contains(&suite_decl.id) {
            continue;
        }

        match registry::build_suite(suite_decl) {
            Ok(Some(suite)) => suites.push(suite),
            Ok(None) => {
                eprintln!("warning: unknown suite {}", suite_decl.id);
                issues.push(eval_harness::RunIssue::empty_suite(&suite_decl.id));
            }
            Err(e) => {
                eprintln!("warning: failed to load {}: {e}", suite_decl.id);
                issues.push(eval_harness::RunIssue::empty_suite(&suite_decl.id));
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
    let result = runner
        .run_with_progress(&request, &StderrProgressReporter)
        .await;

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
            let client = reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(15))
                .timeout(std::time::Duration::from_secs(180))
                .build()
                .map_err(|e| eval_harness::EvalError::Suite(format!("build fetch client: {e}")))?;
            let response = client
                .get(url)
                .send()
                .await
                .map_err(|e| eval_harness::EvalError::Suite(format!("fetch failed: {e}")))?;
            if !response.status().is_success() {
                return Err(eval_harness::EvalError::Suite(format!(
                    "fetch failed: HTTP {}",
                    response.status()
                )));
            }
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
    profile_path: std::path::PathBuf,
    artifact_path: std::path::PathBuf,
    shard_paths: Vec<std::path::PathBuf>,
) -> std::process::ExitCode {
    let manifest = match ProfileManifest::load(&profile_path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("error: {e}");
            return std::process::ExitCode::from(2);
        }
    };

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

    match eval_harness::merge_shards(&shards, &manifest) {
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
