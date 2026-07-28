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
}

pub fn parse() -> Cli {
    Cli::parse()
}
