//! Build automation that CI and a release both run.
//!
//! `cargo run -p xtask -- <command>`
//!
//! The commands exist because the contracts do:
//!
//!   * `package` produces the release artifacts the release workflow attaches
//!     to a GitHub Release. Its output shape is a published contract.
//!   * `check-ui-bundle` asserts the console bundle's shape before it is
//!     embedded, so a bundler that changes its output fails the build instead
//!     of shipping a UI that cannot boot.
//!   * `check-observability` runs the recording-rule, alert and dashboard
//!     checkers, so 42 rules, 18 alerts and 2 dashboards cannot drift away
//!     from the metrics the crate exports without anything noticing.
//!
//! `package` and `check-ui-bundle` replaced Python tooling under `scripts/ci`;
//! the observability checkers are still Python, and this subcommand is the
//! cargo entry point that makes them reachable from a cargo-only CI.

mod bundle;
mod image_handoff;
mod memory;
mod observability;
mod pack;
mod release_bundle;
mod release_candidate;
mod release_channel;
mod toolchain;

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Clone, ValueEnum)]
enum HandoffMode {
    Seal,
    Verify,
}

#[derive(Parser)]
#[command(name = "xtask", about = "Build automation for memory-mcp")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Decide stable-channel promotion without downgrade or prerelease leakage.
    ReleaseChannel {
        #[arg(long)]
        candidate_tag: String,
        #[arg(long)]
        candidate_digest: String,
        #[arg(long, requires = "current_digest")]
        current_tag: Option<String>,
        #[arg(long, requires = "current_tag")]
        current_digest: Option<String>,
        #[arg(long)]
        output: PathBuf,
    },
    /// Seal or verify a same-run Docker image without rebuilding it.
    ImageHandoff {
        mode: HandoffMode,
        directory: PathBuf,
        #[arg(long)]
        source_sha: String,
        #[arg(long)]
        run_id: String,
        #[arg(long)]
        image_id: Option<String>,
    },
    /// Seal or verify release-mode qualification and every packaged checksum.
    ReleaseBundle {
        mode: HandoffMode,
        directory: PathBuf,
        #[arg(long)]
        source_sha: String,
        #[arg(long)]
        run_id: String,
        #[arg(long)]
        version: String,
    },
    /// Verify a selected CI run is a successful same-commit master candidate.
    VerifyReleaseCandidate {
        run_json: PathBuf,
        #[arg(long)]
        source_sha: String,
        #[arg(long)]
        repository: String,
        #[arg(long)]
        workflow_id: u64,
        #[arg(long)]
        run_id: u64,
    },
    /// Package the binaries in a build directory for a target triple.
    Package {
        /// Directory holding the built binaries, e.g. `target/debug`.
        build_dir: PathBuf,
        /// Rust target triple, e.g. `x86_64-unknown-linux-gnu`.
        target: String,
    },
    /// Require a console bundle to have the shape the build embeds.
    CheckUiBundle {
        /// Directory holding the bundle, e.g. `/src/ui-dist/public`.
        dist: PathBuf,
    },
    /// Require the image's Dioxus CLI version to match the UI crate's pin.
    CheckDioxusPin,
    /// Require the image and CI to build on the channel `rust-toolchain.toml`
    /// pins.
    CheckToolchainPin,
    /// Sample Linux process and cgroup memory into bounded JSON-lines output.
    SampleMemory {
        /// Linux process ID to sample.
        #[arg(long)]
        pid: u32,
        /// Finish successfully after the target exits, provided a matching
        /// application sample was captured and a final cgroup snapshot flushed.
        #[arg(long)]
        allow_process_exit: bool,
        /// Sampling duration, from 1 to 86400 seconds.
        #[arg(long)]
        duration_secs: u64,
        /// Sampling interval, from 1 to 60000 milliseconds.
        #[arg(long)]
        interval_ms: u64,
        /// Output JSON-lines file. Existing files are replaced.
        #[arg(long)]
        output: PathBuf,
        /// Optional marker written after the first sample has been flushed.
        #[arg(long)]
        ready_file: Option<PathBuf>,
        /// Require at least one sample from this executable basename.
        #[arg(long)]
        target_binary_name: Option<String>,
    },
    /// Regenerate the dashboards and check the rules, alerts and panels
    /// against the metrics the crate actually exports.
    CheckObservability,
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::ReleaseChannel {
            candidate_tag,
            candidate_digest,
            current_tag,
            current_digest,
            output,
        } => {
            let current = current_tag.as_deref().zip(current_digest.as_deref());
            release_channel::choose(&candidate_tag, current, &candidate_digest).and_then(
                |promote| {
                    std::fs::write(&output, format!("{promote}\n")).map_err(|source| {
                        pack::PackError::Io {
                            path: output,
                            source,
                        }
                    })
                },
            )
        }
        Command::ImageHandoff {
            mode,
            directory,
            source_sha,
            run_id,
            image_id,
        } => match mode {
            HandoffMode::Seal => image_id
                .as_deref()
                .ok_or_else(|| pack::PackError::Bundle("--image-id is required for sealing".into()))
                .and_then(|id| image_handoff::seal(&directory, id, &source_sha, &run_id)),
            HandoffMode::Verify => {
                image_handoff::verify(&directory, &source_sha, &run_id, image_id.as_deref())
                    .map(|_| ())
            }
        },
        Command::ReleaseBundle {
            mode,
            directory,
            source_sha,
            run_id,
            version,
        } => match mode {
            HandoffMode::Seal => release_bundle::seal(&directory, &source_sha, &run_id, &version),
            HandoffMode::Verify => {
                release_bundle::verify(&directory, &source_sha, &run_id, &version)
            }
        },
        Command::VerifyReleaseCandidate {
            run_json,
            source_sha,
            repository,
            workflow_id,
            run_id,
        } => std::fs::read(&run_json)
            .map_err(|error| pack::PackError::Io {
                path: run_json,
                source: error,
            })
            .and_then(|bytes| {
                serde_json::from_slice(&bytes)
                    .map_err(|error| pack::PackError::Bundle(error.to_string()))
            })
            .and_then(|run| {
                release_candidate::verify(&run, &source_sha, &repository, workflow_id, run_id)
            }),
        Command::Package { build_dir, target } => {
            pack::package(&build_dir, &target, &PathBuf::from("dist")).map(|_| ())
        }
        Command::CheckUiBundle { dist } => bundle::check(&dist),
        Command::CheckDioxusPin => bundle::check_cli_pin(),
        Command::CheckToolchainPin => toolchain::check(),
        Command::SampleMemory {
            pid,
            allow_process_exit,
            duration_secs,
            interval_ms,
            output,
            ready_file,
            target_binary_name,
        } => memory::run(
            pid,
            duration_secs,
            interval_ms,
            &output,
            allow_process_exit,
            ready_file.as_deref(),
            target_binary_name.as_deref(),
        )
        .map_err(pack::PackError::Smoke),
        Command::CheckObservability => observability::run(&observability::scripts_dir()),
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("memory-xtask: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
