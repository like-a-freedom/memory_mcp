//! Build automation that CI and a release both run.
//!
//! `cargo run -p xtask -- <command>`
//!
//! Two commands exist because two contracts do:
//!
//!   * `package` produces the release artifacts the release workflow attaches
//!     to a GitHub Release. Its output shape is a published contract.
//!   * `check-ui-bundle` asserts the console bundle's shape before it is
//!     embedded, so a bundler that changes its output fails the build instead
//!     of shipping a UI that cannot boot.
//!   * `check-observability` runs the recording-rule, alert and dashboard
//!     checkers, so 45 rules, 20 alerts and 2 dashboards cannot drift away
//!     from the metrics the crate exports without anything noticing.
//!
//! Two of the three replaced Python tooling under `scripts/ci`; the
//! repository runs nothing outside cargo. The observability checkers are still
//! Python, and this subcommand is the cargo entry point that makes them
//! reachable from a cargo-only CI.

mod bundle;
mod memory;
mod observability;
mod pack;
mod toolchain;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask", about = "Build automation for memory-mcp")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
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
        /// Sampling duration, from 1 to 86400 seconds.
        #[arg(long)]
        duration_secs: u64,
        /// Sampling interval, from 1 to 60000 milliseconds.
        #[arg(long)]
        interval_ms: u64,
        /// Output JSON-lines file. Existing files are replaced.
        #[arg(long)]
        output: PathBuf,
    },
    /// Regenerate the dashboards and check the rules, alerts and panels
    /// against the metrics the crate actually exports.
    CheckObservability,
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Package { build_dir, target } => {
            pack::package(&build_dir, &target, &PathBuf::from("dist")).map(|_| ())
        }
        Command::CheckUiBundle { dist } => bundle::check(&dist),
        Command::CheckDioxusPin => bundle::check_cli_pin(),
        Command::CheckToolchainPin => toolchain::check(),
        Command::SampleMemory {
            pid,
            duration_secs,
            interval_ms,
            output,
        } => memory::run(pid, duration_secs, interval_ms, &output).map_err(pack::PackError::Smoke),
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
