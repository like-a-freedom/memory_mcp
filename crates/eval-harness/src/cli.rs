use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "memory-eval", about = "Memory MCP evaluation harness")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Emit only the hashes and counts needed to preregister an agent-managed task manifest.
    HashAgentManagedTasks {
        #[arg(long)]
        tasks: PathBuf,
        #[arg(
            long,
            default_value = "agent_managed_memory",
            value_parser = ["agent_managed_memory", "agent_source_authority", "agent_source_authority_literal_input", "agent_maintenance", "agent_maintenance_v2", "agent_maintenance_v3", "agent_maintenance_v4", "agent_maintenance_v5", "agent_maintenance_v6", "agent_maintenance_v7"]
        )]
        memory_tool_profile: String,
    },
    /// Emit hashes for the provider-neutral loopback Chat Completions workflow.
    HashChatCompletionsTasks {
        #[arg(long)]
        tasks: PathBuf,
        /// Optionally pin the effective loopback endpoint in the emitted fingerprints.
        #[arg(long)]
        model_endpoint: Option<String>,
        /// Optional JSON file pinning model-generation parameters for both arms.
        #[arg(long)]
        sampling_config: Option<PathBuf>,
        /// Optional JSON file pinning the Chat Completions response format for both arms.
        #[arg(long)]
        response_format_config: Option<PathBuf>,
        /// Optional JSON object with extra provider request parameters, hash-pinned in the plan.
        #[arg(long)]
        request_parameters_config: Option<PathBuf>,
    },
    /// Capture a fixed public MCP probe in a new store, without an agent runner.
    CaptureIndependentProbe {
        #[arg(long, default_value = "stdio", value_parser = ["stdio", "streamable-http"])]
        transport: String,
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        history: PathBuf,
        #[arg(long)]
        server_binary: PathBuf,
        #[arg(long)]
        product_sha256: String,
        #[arg(long)]
        store: PathBuf,
        #[arg(long)]
        claim_rollout_stage: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Score an independent fixed-query observation offline, without an agent runner.
    ScoreIndependentProbe {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        oracle: PathBuf,
        #[arg(long)]
        observation: PathBuf,
        #[arg(long)]
        product_sha256: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Apply frozen finite-cohort gates to two independent repeats of each product.
    AcceptMemoryCandidate {
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        corpus_root: PathBuf,
        #[arg(long)]
        incumbent_first: PathBuf,
        #[arg(long)]
        incumbent_second: PathBuf,
        #[arg(long)]
        candidate_first: PathBuf,
        #[arg(long)]
        candidate_second: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Run {
        #[arg(long)]
        profile: PathBuf,
        #[arg(long)]
        artifact: PathBuf,
        #[arg(long)]
        baseline: Option<PathBuf>,
        #[arg(long = "suite")]
        suites: Vec<String>,
    },
    PrepareCorpus {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        output_root: PathBuf,
    },
    /// Extract a fact inventory from raw episodes with the production provider.
    PrepareRetrievalQuality {
        #[arg(long)]
        sources: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Fit a relevance boundary on reviewed development pairs with the real model.
    CalibrateReranking {
        #[arg(long)]
        pairs: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Provision an immutable native model bundle with hash verification.
    PrepareModel {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        output_root: PathBuf,
        #[arg(long)]
        mirror: Option<String>,
    },
    Merge {
        #[arg(long)]
        profile: PathBuf,
        #[arg(long)]
        artifact: PathBuf,
        shards: Vec<PathBuf>,
    },
    /// Regrade registered raw receipts and compare current memory without executing the product.
    CompareCurrentMemory {
        #[arg(long)]
        baseline: PathBuf,
        #[arg(long)]
        candidate: PathBuf,
        #[arg(long)]
        corpus: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Check repeatability of two deterministic memory-decision runs.
    CompareDecisionReplays {
        #[arg(long)]
        left: PathBuf,
        #[arg(long)]
        right: PathBuf,
    },
    /// Compare two preregistered agent-memory runs and apply their frozen promotion gate.
    ComparePaired {
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        incumbent: PathBuf,
        #[arg(long)]
        candidate: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Compare exploratory paired runs without producing a promotion decision.
    ComparePairedDiagnostic {
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        incumbent: PathBuf,
        #[arg(long)]
        candidate: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Estimate family-level incumbent test-retest variance without a promotion decision.
    SummarizePilot {
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        first_run_directory: PathBuf,
        #[arg(long)]
        second_run_directory: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Apply frozen ordinary-workflow, independent-memory, and safety gates to two captures.
    QualifyAgentWorkflow {
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        tasks: PathBuf,
        #[arg(long)]
        probes: PathBuf,
        #[arg(long)]
        criteria: PathBuf,
        #[arg(long)]
        first_incumbent: PathBuf,
        #[arg(long)]
        first_candidate: PathBuf,
        #[arg(long)]
        second_incumbent: PathBuf,
        #[arg(long)]
        second_candidate: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Plan exact paired McNemar sample size from a validated exploratory pilot report.
    PlanPairedPower {
        #[arg(long)]
        pilot_report: PathBuf,
        #[arg(long)]
        stratum_id: String,
        #[arg(long)]
        target_risk_difference: f64,
        #[arg(long)]
        minimum_point_effect: f64,
        #[arg(long)]
        one_sided_alpha: f64,
        #[arg(long)]
        target_power: f64,
        #[arg(long)]
        confidence_level: f64,
        #[arg(long)]
        maximum_sample_size: u32,
        #[arg(long)]
        output: PathBuf,
    },
    /// Report repeated case reliability (pass^k) for paired agent runs.
    SummarizePairedReliability {
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        incumbent: PathBuf,
        #[arg(long)]
        candidate: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Compare paired agent outcomes with the formation, write and independent-probe receipts.
    CompareAgentManaged {
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        incumbent: PathBuf,
        #[arg(long)]
        candidate: PathBuf,
        #[arg(long)]
        incumbent_stages: PathBuf,
        #[arg(long)]
        candidate_stages: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Export preregistered tasks and identities for an external runner, omitting the oracle.
    ExportPairedInputs {
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        tasks: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Score normalized observations from an external runner without starting an agent.
    ScorePairedObservations {
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        tasks: PathBuf,
        #[arg(long)]
        observations: PathBuf,
        #[arg(long)]
        output_directory: PathBuf,
    },
    /// Execute both preregistered arms through the optional Codex CLI adapter.
    #[command(name = "run-paired-codex", alias = "run-paired-agent")]
    RunPairedCodex {
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        tasks: PathBuf,
        #[arg(long)]
        codex_binary: PathBuf,
        /// New independent envelopes; omitted only for explicitly legacy agent observations.
        #[arg(long)]
        independent_probes: Option<PathBuf>,
        /// Use an already-authenticated local Codex home instead of CODEX_API_KEY.
        #[arg(long)]
        codex_home: Option<PathBuf>,
        #[arg(long)]
        incumbent_memory_server: PathBuf,
        #[arg(long)]
        candidate_memory_server: PathBuf,
        #[arg(long)]
        output_directory: PathBuf,
        #[arg(long)]
        temp_root: Option<PathBuf>,
        /// Common stage for legacy plans that do not pin a stage per arm.
        #[arg(long)]
        claim_rollout_stage: Option<String>,
        /// Fixed Codex tool surface; write access is opt-in and version-pinned.
        #[arg(long, default_value = "recall_only", value_parser = ["recall_only", "agent_managed_memory", "agent_source_authority", "agent_source_authority_literal_input", "agent_maintenance", "agent_maintenance_v2", "agent_maintenance_v3", "agent_maintenance_v4", "agent_maintenance_v5", "agent_maintenance_v6", "agent_maintenance_v7"])]
        memory_tool_profile: String,
    },
    /// Execute source-authority trials through any loopback OpenAI-compatible API.
    #[command(name = "run-paired-chat-completions")]
    RunPairedChatCompletions {
        #[arg(long, value_enum)]
        recall_read_mode: Option<crate::paired_agent::chat_completions_backend::RecallCaptureMode>,
        #[arg(long, requires = "recall_read_mode")]
        fixed_read_inputs: Option<PathBuf>,
        /// Add independent fixed-query evidence captures to ordinary recall runs.
        #[arg(long, requires = "recall_read_mode")]
        independent_probes: Option<PathBuf>,
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        tasks: PathBuf,
        /// Base URL such as http://127.0.0.1:1234/v1; only numeric loopback is accepted.
        #[arg(long)]
        model_endpoint: String,
        /// Optional JSON file pinning model-generation parameters for both arms.
        #[arg(long)]
        sampling_config: Option<PathBuf>,
        /// Optional JSON file pinning the Chat Completions response format for both arms.
        #[arg(long)]
        response_format_config: Option<PathBuf>,
        /// Optional JSON object with extra provider request parameters, hash-pinned in the plan.
        #[arg(long)]
        request_parameters_config: Option<PathBuf>,
        #[arg(long)]
        incumbent_memory_server: PathBuf,
        #[arg(long)]
        candidate_memory_server: PathBuf,
        #[arg(long)]
        output_directory: PathBuf,
        /// Retain exact public MCP arguments/responses. Use only public synthetic task data.
        #[arg(long)]
        public_synthetic_traces: Option<PathBuf>,
        #[arg(long)]
        temp_root: Option<PathBuf>,
    },
}

pub fn parse() -> Cli {
    Cli::parse()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_task_fingerprint_command_takes_only_a_task_manifest() {
        let parsed = Cli::try_parse_from([
            "memory-eval",
            "hash-agent-managed-tasks",
            "--tasks",
            "tasks.json",
        ])
        .expect("agent-managed task fingerprint command parses");

        assert!(matches!(
            parsed.command,
            Command::HashAgentManagedTasks { .. }
        ));

        let maintenance = Cli::try_parse_from([
            "memory-eval",
            "hash-agent-managed-tasks",
            "--tasks",
            "tasks.json",
            "--memory-tool-profile",
            "agent_maintenance",
        ])
        .expect("maintenance fingerprints select a distinct registered profile");
        assert!(matches!(
            maintenance.command,
            Command::HashAgentManagedTasks {
                memory_tool_profile,
                ..
            } if memory_tool_profile == "agent_maintenance"
        ));

        let maintenance_v2 = Cli::try_parse_from([
            "memory-eval",
            "hash-agent-managed-tasks",
            "--tasks",
            "tasks.json",
            "--memory-tool-profile",
            "agent_maintenance_v2",
        ])
        .expect("v2 maintenance fingerprints select a separately pinned profile");
        assert!(matches!(
            maintenance_v2.command,
            Command::HashAgentManagedTasks {
                memory_tool_profile,
                ..
            } if memory_tool_profile == "agent_maintenance_v2"
        ));

        let maintenance_v3 = Cli::try_parse_from([
            "memory-eval",
            "hash-agent-managed-tasks",
            "--tasks",
            "tasks.json",
            "--memory-tool-profile",
            "agent_maintenance_v3",
        ])
        .expect("v3 maintenance fingerprints select an independently pinned profile");
        assert!(matches!(
            maintenance_v3.command,
            Command::HashAgentManagedTasks {
                memory_tool_profile,
                ..
            } if memory_tool_profile == "agent_maintenance_v3"
        ));

        let maintenance_v4 = Cli::try_parse_from([
            "memory-eval",
            "hash-agent-managed-tasks",
            "--tasks",
            "tasks.json",
            "--memory-tool-profile",
            "agent_maintenance_v4",
        ])
        .expect("v4 maintenance fingerprints select a separately pinned profile");
        assert!(matches!(
            maintenance_v4.command,
            Command::HashAgentManagedTasks {
                memory_tool_profile,
                ..
            } if memory_tool_profile == "agent_maintenance_v4"
        ));

        let maintenance_v5 = Cli::try_parse_from([
            "memory-eval",
            "hash-agent-managed-tasks",
            "--tasks",
            "tasks.json",
            "--memory-tool-profile",
            "agent_maintenance_v5",
        ])
        .expect("v5 maintenance fingerprints select a separately pinned profile");
        assert!(matches!(
            maintenance_v5.command,
            Command::HashAgentManagedTasks {
                memory_tool_profile,
                ..
            } if memory_tool_profile == "agent_maintenance_v5"
        ));

        let maintenance_v6 = Cli::try_parse_from([
            "memory-eval",
            "hash-agent-managed-tasks",
            "--tasks",
            "tasks.json",
            "--memory-tool-profile",
            "agent_maintenance_v6",
        ])
        .expect("v6 maintenance fingerprints select a separately pinned profile");
        assert!(matches!(
            maintenance_v6.command,
            Command::HashAgentManagedTasks {
                memory_tool_profile,
                ..
            } if memory_tool_profile == "agent_maintenance_v6"
        ));

        let maintenance_v7 = Cli::try_parse_from([
            "memory-eval",
            "hash-agent-managed-tasks",
            "--tasks",
            "tasks.json",
            "--memory-tool-profile",
            "agent_maintenance_v7",
        ])
        .expect("v7 maintenance fingerprints select a separately pinned profile");
        assert!(matches!(
            maintenance_v7.command,
            Command::HashAgentManagedTasks {
                memory_tool_profile,
                ..
            } if memory_tool_profile == "agent_maintenance_v7"
        ));
    }

    #[test]
    fn paired_comparison_requires_both_arms_plan_and_output() {
        let missing = Cli::try_parse_from([
            "memory-eval",
            "compare-paired",
            "--plan",
            "plan.json",
            "--incumbent",
            "incumbent.json",
        ]);
        assert!(missing.is_err());

        let parsed = Cli::try_parse_from([
            "memory-eval",
            "compare-paired",
            "--plan",
            "plan.json",
            "--incumbent",
            "incumbent.json",
            "--candidate",
            "candidate.json",
            "--output",
            "report.json",
        ])
        .expect("complete paired comparison command parses");
        assert!(matches!(parsed.command, Command::ComparePaired { .. }));
    }

    #[test]
    fn paired_codex_runner_accepts_the_new_maintenance_profile() {
        let parsed = Cli::try_parse_from([
            "memory-eval",
            "run-paired-codex",
            "--plan",
            "plan.json",
            "--tasks",
            "tasks.json",
            "--codex-binary",
            "/usr/local/bin/codex",
            "--incumbent-memory-server",
            "/tmp/incumbent",
            "--candidate-memory-server",
            "/tmp/candidate",
            "--output-directory",
            "/tmp/paired",
            "--memory-tool-profile",
            "agent_maintenance_v5",
        ])
        .expect("v5 maintenance workflow is explicitly opt in");

        assert!(matches!(
            parsed.command,
            Command::RunPairedCodex {
                memory_tool_profile,
                ..
            } if memory_tool_profile == "agent_maintenance_v5"
        ));
    }

    #[test]
    fn paired_codex_runner_accepts_the_v6_partial_read_profile() {
        let parsed = Cli::try_parse_from([
            "memory-eval",
            "run-paired-codex",
            "--plan",
            "plan.json",
            "--tasks",
            "tasks.json",
            "--codex-binary",
            "/usr/local/bin/codex",
            "--incumbent-memory-server",
            "/tmp/incumbent",
            "--candidate-memory-server",
            "/tmp/candidate",
            "--output-directory",
            "/tmp/paired",
            "--memory-tool-profile",
            "agent_maintenance_v6",
        ])
        .expect("v6 partial-read workflow is explicitly opt in");

        assert!(matches!(
            parsed.command,
            Command::RunPairedCodex {
                memory_tool_profile,
                ..
            } if memory_tool_profile == "agent_maintenance_v6"
        ));
    }

    #[test]
    fn paired_codex_runner_accepts_the_v7_snapshot_consistent_profile() {
        let parsed = Cli::try_parse_from([
            "memory-eval",
            "run-paired-codex",
            "--plan",
            "plan.json",
            "--tasks",
            "tasks.json",
            "--codex-binary",
            "/usr/local/bin/codex",
            "--incumbent-memory-server",
            "/tmp/incumbent",
            "--candidate-memory-server",
            "/tmp/candidate",
            "--output-directory",
            "/tmp/paired",
            "--memory-tool-profile",
            "agent_maintenance_v7",
        ])
        .expect("v7 snapshot-consistent workflow is explicitly opt in");

        assert!(matches!(
            parsed.command,
            Command::RunPairedCodex {
                memory_tool_profile,
                ..
            } if memory_tool_profile == "agent_maintenance_v7"
        ));
    }

    #[test]
    fn paired_agent_run_requires_both_product_binaries_and_a_task_manifest() {
        let missing = Cli::try_parse_from([
            "memory-eval",
            "run-paired-agent",
            "--plan",
            "plan.json",
            "--tasks",
            "tasks.json",
            "--codex-binary",
            "/usr/local/bin/codex",
            "--incumbent-memory-server",
            "/tmp/incumbent",
            "--output-directory",
            "/tmp/paired",
        ]);
        assert!(missing.is_err());

        let parsed = Cli::try_parse_from([
            "memory-eval",
            "run-paired-agent",
            "--plan",
            "plan.json",
            "--tasks",
            "tasks.json",
            "--codex-binary",
            "/usr/local/bin/codex",
            "--incumbent-memory-server",
            "/tmp/incumbent",
            "--candidate-memory-server",
            "/tmp/candidate",
            "--output-directory",
            "/tmp/paired",
        ])
        .expect("complete paired agent command parses");
        assert!(matches!(parsed.command, Command::RunPairedCodex { .. }));
    }

    #[test]
    fn paired_agent_run_leaves_legacy_stage_resolution_to_the_plan() {
        let parsed = Cli::try_parse_from([
            "memory-eval",
            "run-paired-agent",
            "--plan",
            "plan.json",
            "--tasks",
            "tasks.json",
            "--codex-binary",
            "/usr/local/bin/codex",
            "--incumbent-memory-server",
            "/tmp/incumbent",
            "--candidate-memory-server",
            "/tmp/candidate",
            "--output-directory",
            "/tmp/paired",
        ])
        .expect("complete paired agent command parses");

        match parsed.command {
            Command::RunPairedCodex {
                claim_rollout_stage,
                ..
            } => assert_eq!(claim_rollout_stage, None),
            _ => panic!("expected paired agent command"),
        }
    }

    #[test]
    fn paired_agent_run_accepts_a_legacy_common_claim_stage_override() {
        let parsed = Cli::try_parse_from([
            "memory-eval",
            "run-paired-agent",
            "--plan",
            "plan.json",
            "--tasks",
            "tasks.json",
            "--codex-binary",
            "/usr/local/bin/codex",
            "--incumbent-memory-server",
            "/tmp/incumbent",
            "--candidate-memory-server",
            "/tmp/candidate",
            "--output-directory",
            "/tmp/paired",
            "--claim-rollout-stage",
            "shadow",
        ])
        .expect("legacy common rollout-stage override parses");

        match parsed.command {
            Command::RunPairedCodex {
                claim_rollout_stage,
                ..
            } => assert_eq!(claim_rollout_stage.as_deref(), Some("shadow")),
            _ => panic!("expected paired agent command"),
        }
    }

    #[test]
    fn paired_agent_run_accepts_an_explicit_local_codex_home() {
        let parsed = Cli::try_parse_from([
            "memory-eval",
            "run-paired-agent",
            "--plan",
            "plan.json",
            "--tasks",
            "tasks.json",
            "--codex-binary",
            "/usr/local/bin/codex",
            "--incumbent-memory-server",
            "/tmp/incumbent",
            "--candidate-memory-server",
            "/tmp/candidate",
            "--output-directory",
            "/tmp/paired",
            "--codex-home",
            "/Users/test/.codex",
        ])
        .expect("local Codex authentication home is an explicit option");

        assert!(matches!(parsed.command, Command::RunPairedCodex { .. }));
    }

    #[test]
    fn paired_agent_run_keeps_recall_only_as_default_and_accepts_opt_in_write_profile() {
        let base = [
            "memory-eval",
            "run-paired-codex",
            "--plan",
            "plan.json",
            "--tasks",
            "tasks.json",
            "--codex-binary",
            "/usr/local/bin/codex",
            "--incumbent-memory-server",
            "/tmp/incumbent",
            "--candidate-memory-server",
            "/tmp/candidate",
            "--output-directory",
            "/tmp/paired",
        ];
        let default = Cli::try_parse_from(base).expect("default Codex profile parses");
        match default.command {
            Command::RunPairedCodex {
                memory_tool_profile,
                ..
            } => assert_eq!(memory_tool_profile, "recall_only"),
            _ => panic!("expected paired Codex command"),
        }

        let write_enabled = Cli::try_parse_from([
            "memory-eval",
            "run-paired-codex",
            "--plan",
            "plan.json",
            "--tasks",
            "tasks.json",
            "--codex-binary",
            "/usr/local/bin/codex",
            "--incumbent-memory-server",
            "/tmp/incumbent",
            "--candidate-memory-server",
            "/tmp/candidate",
            "--output-directory",
            "/tmp/paired",
            "--memory-tool-profile",
            "agent_managed_memory",
        ])
        .expect("explicit write profile parses");
        assert!(matches!(
            write_enabled.command,
            Command::RunPairedCodex {
                memory_tool_profile,
                ..
            } if memory_tool_profile == "agent_managed_memory"
        ));

        let source_authority = Cli::try_parse_from([
            "memory-eval",
            "run-paired-codex",
            "--plan",
            "plan.json",
            "--tasks",
            "tasks.json",
            "--codex-binary",
            "/usr/local/bin/codex",
            "--incumbent-memory-server",
            "/tmp/incumbent",
            "--candidate-memory-server",
            "/tmp/candidate",
            "--output-directory",
            "/tmp/paired",
            "--memory-tool-profile",
            "agent_source_authority",
        ])
        .expect("source-authority workflow profile is an explicit opt-in");
        assert!(matches!(
            source_authority.command,
            Command::RunPairedCodex {
                memory_tool_profile,
                ..
            } if memory_tool_profile == "agent_source_authority"
        ));

        let literal_source_authority = Cli::try_parse_from([
            "memory-eval",
            "hash-agent-managed-tasks",
            "--tasks",
            "tasks.json",
            "--memory-tool-profile",
            "agent_source_authority_literal_input",
        ])
        .expect("literal-source workflow is an explicit opt-in profile");
        assert!(matches!(
            literal_source_authority.command,
            Command::HashAgentManagedTasks {
                memory_tool_profile,
                ..
            } if memory_tool_profile == "agent_source_authority_literal_input"
        ));

        let maintenance = Cli::try_parse_from([
            "memory-eval",
            "run-paired-codex",
            "--plan",
            "plan.json",
            "--tasks",
            "tasks.json",
            "--codex-binary",
            "/usr/local/bin/codex",
            "--incumbent-memory-server",
            "/tmp/incumbent",
            "--candidate-memory-server",
            "/tmp/candidate",
            "--output-directory",
            "/tmp/paired",
            "--memory-tool-profile",
            "agent_maintenance",
        ])
        .expect("maintenance profile is explicitly opt in");
        assert!(matches!(
            maintenance.command,
            Command::RunPairedCodex {
                memory_tool_profile,
                ..
            } if memory_tool_profile == "agent_maintenance"
        ));

        let maintenance_v2 = Cli::try_parse_from([
            "memory-eval",
            "run-paired-codex",
            "--plan",
            "plan.json",
            "--tasks",
            "tasks.json",
            "--codex-binary",
            "/usr/local/bin/codex",
            "--incumbent-memory-server",
            "/tmp/incumbent",
            "--candidate-memory-server",
            "/tmp/candidate",
            "--output-directory",
            "/tmp/paired",
            "--memory-tool-profile",
            "agent_maintenance_v2",
        ])
        .expect("v2 maintenance profile is explicitly opt in");
        assert!(matches!(
            maintenance_v2.command,
            Command::RunPairedCodex {
                memory_tool_profile,
                ..
            } if memory_tool_profile == "agent_maintenance_v2"
        ));

        let maintenance_v3 = Cli::try_parse_from([
            "memory-eval",
            "run-paired-codex",
            "--plan",
            "plan.json",
            "--tasks",
            "tasks.json",
            "--codex-binary",
            "/usr/local/bin/codex",
            "--incumbent-memory-server",
            "/tmp/incumbent",
            "--candidate-memory-server",
            "/tmp/candidate",
            "--output-directory",
            "/tmp/paired",
            "--memory-tool-profile",
            "agent_maintenance_v3",
        ])
        .expect("v3 maintenance profile is explicitly opt in");
        assert!(matches!(
            maintenance_v3.command,
            Command::RunPairedCodex {
                memory_tool_profile,
                ..
            } if memory_tool_profile == "agent_maintenance_v3"
        ));

        let maintenance_v4 = Cli::try_parse_from([
            "memory-eval",
            "run-paired-codex",
            "--plan",
            "plan.json",
            "--tasks",
            "tasks.json",
            "--codex-binary",
            "/usr/local/bin/codex",
            "--incumbent-memory-server",
            "/tmp/incumbent",
            "--candidate-memory-server",
            "/tmp/candidate",
            "--output-directory",
            "/tmp/paired",
            "--memory-tool-profile",
            "agent_maintenance_v4",
        ])
        .expect("v4 maintenance profile is explicitly opt in");
        assert!(matches!(
            maintenance_v4.command,
            Command::RunPairedCodex {
                memory_tool_profile,
                ..
            } if memory_tool_profile == "agent_maintenance_v4"
        ));
    }

    #[test]
    fn agent_managed_comparison_requires_plan_both_arms_and_stage_receipts() {
        let missing = Cli::try_parse_from([
            "memory-eval",
            "compare-agent-managed",
            "--plan",
            "plan.json",
            "--incumbent",
            "incumbent.json",
        ]);
        assert!(missing.is_err());

        let parsed = Cli::try_parse_from([
            "memory-eval",
            "compare-agent-managed",
            "--plan",
            "plan.json",
            "--incumbent",
            "incumbent.json",
            "--candidate",
            "candidate.json",
            "--incumbent-stages",
            "incumbent-stages.json",
            "--candidate-stages",
            "candidate-stages.json",
            "--output",
            "report.json",
        ])
        .expect("complete agent-managed comparison parses");

        assert!(matches!(
            parsed.command,
            Command::CompareAgentManaged { .. }
        ));
    }

    #[test]
    fn pilot_summary_requires_two_run_directories_and_an_output() {
        let missing = Cli::try_parse_from([
            "memory-eval",
            "summarize-pilot",
            "--plan",
            "pilot-plan.json",
            "--first-run-directory",
            "repeat-one",
            "--second-run-directory",
            "repeat-two",
        ]);
        assert!(missing.is_err());

        let parsed = Cli::try_parse_from([
            "memory-eval",
            "summarize-pilot",
            "--plan",
            "pilot-plan.json",
            "--first-run-directory",
            "repeat-one",
            "--second-run-directory",
            "repeat-two",
            "--output",
            "pilot-report.json",
        ])
        .expect("complete pilot summary command parses");
        assert!(matches!(parsed.command, Command::SummarizePilot { .. }));
    }

    #[test]
    fn provider_neutral_chat_completions_runner_command_parses_without_codex_arguments() {
        let parsed = Cli::try_parse_from([
            "memory-eval",
            "run-paired-chat-completions",
            "--plan",
            "plan.json",
            "--tasks",
            "tasks.json",
            "--model-endpoint",
            "http://127.0.0.1:1234/v1",
            "--incumbent-memory-server",
            "memory-incumbent",
            "--candidate-memory-server",
            "memory-candidate",
            "--output-directory",
            "run-output",
        ])
        .expect("Chat Completions runner does not require Codex flags or credentials");

        assert!(matches!(
            parsed.command,
            Command::RunPairedChatCompletions { .. }
        ));
    }

    #[test]
    fn public_synthetic_trace_capture_requires_explicit_cli_opt_in() {
        let parsed = Cli::try_parse_from([
            "memory-eval",
            "run-paired-chat-completions",
            "--plan",
            "plan.json",
            "--tasks",
            "tasks.json",
            "--model-endpoint",
            "http://127.0.0.1:1234/v1",
            "--incumbent-memory-server",
            "memory-incumbent",
            "--candidate-memory-server",
            "memory-candidate",
            "--output-directory",
            "runs",
            "--public-synthetic-traces",
            "public-traces",
        ]);
        assert!(
            parsed.is_ok(),
            "public synthetic capture must be explicitly selectable"
        );
    }

    #[test]
    fn chat_completions_task_fingerprint_command_parses() {
        let parsed = Cli::try_parse_from([
            "memory-eval",
            "hash-chat-completions-tasks",
            "--tasks",
            "tasks.json",
        ])
        .expect("generic runner task fingerprint command parses");

        assert!(matches!(
            parsed.command,
            Command::HashChatCompletionsTasks { .. }
        ));
    }

    #[test]
    fn chat_completions_task_fingerprint_command_accepts_an_optional_endpoint() {
        let parsed = Cli::try_parse_from([
            "memory-eval",
            "hash-chat-completions-tasks",
            "--tasks",
            "tasks.json",
            "--model-endpoint",
            "http://127.0.0.1:1234/v1",
        ])
        .expect("provider-neutral runner can optionally hash a local endpoint");

        assert!(matches!(
            parsed.command,
            Command::HashChatCompletionsTasks {
                model_endpoint: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn chat_completions_task_fingerprint_command_accepts_a_sampling_config() {
        let parsed = Cli::try_parse_from([
            "memory-eval",
            "hash-chat-completions-tasks",
            "--tasks",
            "tasks.json",
            "--sampling-config",
            "sampling.json",
        ])
        .expect("provider-neutral runner can fingerprint a sampling configuration");

        assert!(matches!(
            parsed.command,
            Command::HashChatCompletionsTasks {
                sampling_config: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn chat_completions_task_fingerprint_command_accepts_a_response_format_config() {
        let parsed = Cli::try_parse_from([
            "memory-eval",
            "hash-chat-completions-tasks",
            "--tasks",
            "tasks.json",
            "--response-format-config",
            "response-format.json",
        ])
        .expect("provider-neutral runner can fingerprint a response format");

        assert!(matches!(
            parsed.command,
            Command::HashChatCompletionsTasks {
                response_format_config: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn chat_completions_task_fingerprint_command_accepts_request_parameters_config() {
        let parsed = Cli::try_parse_from([
            "memory-eval",
            "hash-chat-completions-tasks",
            "--tasks",
            "tasks.json",
            "--request-parameters-config",
            "request-parameters.json",
        ])
        .expect("provider-neutral runner can fingerprint provider request parameters");

        assert!(matches!(
            parsed.command,
            Command::HashChatCompletionsTasks {
                request_parameters_config: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn chat_completions_runner_command_accepts_a_sampling_config() {
        let parsed = Cli::try_parse_from([
            "memory-eval",
            "run-paired-chat-completions",
            "--plan",
            "plan.json",
            "--tasks",
            "tasks.json",
            "--model-endpoint",
            "http://127.0.0.1:1234/v1",
            "--incumbent-memory-server",
            "memory-mcp",
            "--candidate-memory-server",
            "memory-mcp",
            "--output-directory",
            "runs",
            "--sampling-config",
            "sampling.json",
        ])
        .expect("provider-neutral runner can load a registered sampling configuration");

        assert!(matches!(
            parsed.command,
            Command::RunPairedChatCompletions {
                sampling_config: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn chat_completions_runner_command_accepts_a_response_format_config() {
        let parsed = Cli::try_parse_from([
            "memory-eval",
            "run-paired-chat-completions",
            "--plan",
            "plan.json",
            "--tasks",
            "tasks.json",
            "--model-endpoint",
            "http://127.0.0.1:1234/v1",
            "--incumbent-memory-server",
            "memory-mcp",
            "--candidate-memory-server",
            "memory-mcp",
            "--output-directory",
            "runs",
            "--response-format-config",
            "response-format.json",
        ])
        .expect("provider-neutral runner can load a registered response format");

        assert!(matches!(
            parsed.command,
            Command::RunPairedChatCompletions {
                response_format_config: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn chat_completions_runner_command_accepts_request_parameters_config() {
        let parsed = Cli::try_parse_from([
            "memory-eval",
            "run-paired-chat-completions",
            "--plan",
            "plan.json",
            "--tasks",
            "tasks.json",
            "--model-endpoint",
            "http://127.0.0.1:1234/v1",
            "--incumbent-memory-server",
            "memory-mcp",
            "--candidate-memory-server",
            "memory-mcp",
            "--output-directory",
            "runs",
            "--request-parameters-config",
            "request-parameters.json",
        ])
        .expect("provider-neutral runner can load pinned provider request parameters");

        assert!(matches!(
            parsed.command,
            Command::RunPairedChatCompletions {
                request_parameters_config: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn provider_neutral_chat_completions_runner_accepts_independent_probes_only_for_recall() {
        let base = [
            "memory-eval",
            "run-paired-chat-completions",
            "--plan",
            "plan.json",
            "--tasks",
            "tasks.json",
            "--model-endpoint",
            "http://127.0.0.1:1234/v1",
            "--incumbent-memory-server",
            "memory-mcp",
            "--candidate-memory-server",
            "memory-mcp",
            "--output-directory",
            "runs",
            "--independent-probes",
            "probes.json",
        ];
        assert!(Cli::try_parse_from(base).is_err());

        let parsed = Cli::try_parse_from([
            "memory-eval",
            "run-paired-chat-completions",
            "--plan",
            "plan.json",
            "--tasks",
            "tasks.json",
            "--model-endpoint",
            "http://127.0.0.1:1234/v1",
            "--incumbent-memory-server",
            "memory-mcp",
            "--candidate-memory-server",
            "memory-mcp",
            "--output-directory",
            "runs",
            "--recall-read-mode",
            "live",
            "--independent-probes",
            "probes.json",
        ])
        .expect("provider-neutral recall capture accepts independent probes");

        assert!(matches!(
            parsed.command,
            Command::RunPairedChatCompletions {
                independent_probes: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn agent_workflow_qualification_requires_both_paired_captures_and_all_frozen_inputs() {
        let missing = Cli::try_parse_from([
            "memory-eval",
            "qualify-agent-workflow",
            "--plan",
            "plan.json",
        ]);
        assert!(missing.is_err());

        let parsed = Cli::try_parse_from([
            "memory-eval",
            "qualify-agent-workflow",
            "--plan",
            "plan.json",
            "--tasks",
            "tasks.json",
            "--probes",
            "probes.json",
            "--criteria",
            "criteria.json",
            "--first-incumbent",
            "first/independent-incumbent.json",
            "--first-candidate",
            "first/independent-candidate.json",
            "--second-incumbent",
            "second/independent-incumbent.json",
            "--second-candidate",
            "second/independent-candidate.json",
            "--output",
            "qualification.json",
        ])
        .expect("qualification command requires two complete paired captures");

        assert!(matches!(
            parsed.command,
            Command::QualifyAgentWorkflow { .. }
        ));
    }
}
