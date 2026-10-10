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
}

pub fn parse() -> Cli {
    Cli::parse()
}
