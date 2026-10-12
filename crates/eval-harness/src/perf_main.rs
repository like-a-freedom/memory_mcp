//! `memory-perf` CLI (ADR-0082): thin clap parsing around `execute`.
//!
//! `main` only parses arguments and forwards the process exit code;
//! `execute` is the testable seam the integration tests drive.

use std::path::PathBuf;
use std::process::Command as GitCommand;

use clap::{Parser, Subcommand};
use eval_harness::perf::artifact;
use eval_harness::perf::profile;
use eval_harness::perf::runner::{self, RunOptions};
use eval_harness::perf::verdict::{self, PerfVerdict};

#[derive(Parser)]
#[command(
    name = "memory-perf",
    about = "Profile-driven performance evaluation (ADR-0082)"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a profile and judge it against an optional baseline.
    Run {
        /// Profile file (memory-mcp-perf-profile/v1)
        #[arg(long)]
        profile: PathBuf,
        /// Where to write the run artifact
        #[arg(long)]
        artifact: PathBuf,
        /// Baseline artifact to compare against
        #[arg(long)]
        baseline: Option<PathBuf>,
        /// Copy the written artifact here after a passing run
        #[arg(long)]
        update_baseline: Option<PathBuf>,
        /// Stable identity of the running host (required)
        #[arg(long)]
        host_tag: String,
    },
}

fn main() -> std::process::ExitCode {
    let code = execute(Cli::parse());
    std::process::ExitCode::from(u8::try_from(code).unwrap_or(2))
}

/// `git rev-parse --short HEAD`; any failure keeps the artifact valid as
/// `unknown` (ADR-0082).
fn git_short_head() -> String {
    GitCommand::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|head| !head.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Runs the parsed command; returns the process exit code (0/1/2).
fn execute(cli: Cli) -> i32 {
    let Cmd::Run {
        profile: profile_path,
        artifact: artifact_path,
        baseline,
        update_baseline,
        host_tag,
    } = cli.command;

    let perf_profile = match profile::load(&profile_path) {
        Ok(perf_profile) => perf_profile,
        Err(err) => {
            eprintln!("error: profile load failed: {err}");
            return 2;
        }
    };
    // `host_tag` is operator-asserted, not measured: the cross-check keeps a
    // run from recording a machine identity the profile was not written for
    // (ADR-0082, pinned-host control).
    if perf_profile.host_tag != host_tag {
        eprintln!(
            "error: --host-tag {host_tag} does not match profile host_tag {} (profile {})",
            perf_profile.host_tag,
            profile_path.display()
        );
        return 2;
    }
    let digest = match std::fs::read(&profile_path) {
        Ok(bytes) => artifact::sha256_hex(&bytes),
        Err(err) => {
            eprintln!("error: cannot read {}: {err}", profile_path.display());
            return 2;
        }
    };

    // SAFETY contract of `apply_env`: applied while the process is still
    // single-threaded, before the tokio thread pool exists.
    runner::apply_env(&perf_profile.env);

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("error: tokio runtime: {err}");
            return 2;
        }
    };
    let written_artifact = artifact_path.clone();
    let outcome = runtime.block_on(runner::run_profile(
        &perf_profile,
        &digest,
        RunOptions {
            artifact_path,
            baseline_path: baseline,
            host_tag,
            commit: git_short_head(),
            now_iso: chrono::Utc::now().to_rfc3339(),
        },
    ));

    match &outcome.verdict {
        PerfVerdict::Passed => eprintln!("perf verdict: passed"),
        PerfVerdict::Regressed(reasons) => {
            eprintln!("perf verdict: regressed");
            for reason in reasons {
                eprintln!("  {reason}");
            }
        }
        PerfVerdict::Invalid(reasons) => {
            eprintln!("perf verdict: invalid");
            for reason in reasons {
                eprintln!("  {reason}");
            }
        }
    }

    if let Some(baseline_path) = update_baseline {
        if outcome.verdict == PerfVerdict::Passed {
            if let Some(parent) = baseline_path.parent()
                && let Err(err) = std::fs::create_dir_all(parent)
            {
                eprintln!("error: cannot create {}: {err}", parent.display());
                return 2;
            }
            match std::fs::copy(&written_artifact, &baseline_path) {
                Ok(_) => eprintln!("baseline updated: {}", baseline_path.display()),
                Err(err) => {
                    eprintln!(
                        "error: cannot update baseline {}: {err}",
                        baseline_path.display()
                    );
                    return 2;
                }
            }
        } else {
            eprintln!("baseline not updated: verdict is not passed");
        }
    }

    verdict::exit_code(&outcome.verdict)
}
