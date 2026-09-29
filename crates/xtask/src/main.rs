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
//!
//! Both replaced Python tooling under `scripts/ci`; the repository now runs
//! nothing outside cargo.

mod bundle;
mod pack;

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
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Package { build_dir, target } => {
            pack::package(&build_dir, &target, &PathBuf::from("dist")).map(|_| ())
        }
        Command::CheckUiBundle { dist } => bundle::check(&dist),
        Command::CheckDioxusPin => bundle::check_cli_pin(),
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("memory-xtask: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
