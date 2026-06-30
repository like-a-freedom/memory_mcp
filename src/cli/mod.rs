//! CLI module — clap-based command surface shared between runtime modes
//! (serve / watch / reembed) and one-shot memory tool subcommands.

pub mod args;
pub mod commands;
pub mod runtime;

use clap::{Parser, Subcommand};

pub use runtime::{
    RunMode, WatchCommand, build_memory_service, log_session_duration, log_startup, parse_cli_args,
    run_reembed_mode, run_stdio_server, run_watch_mode,
};

/// `memory_mcp` command-line interface.
///
/// With no subcommand (or with `serve`), runs the stdio MCP server.
/// Every other subcommand is a one-shot tool invocation that prints
/// `ToolResponse<T>` as pretty JSON to stdout.
#[derive(Debug, Parser)]
#[command(
    name = "memory_mcp",
    version,
    about = "Memory MCP — long-term memory for AI agents (stdio MCP server or one-shot CLI)",
    long_about = None,
)]
pub struct Cli {
    /// Subcommand to run. If omitted, defaults to stdio MCP server mode.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Available subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the stdio MCP server (default when no subcommand is given).
    Serve,
    /// Watch a directory and auto-ingest files as they arrive.
    Watch(args::WatchArgs),
    /// Rebuild all fact embeddings for the current embedding provider/model.
    Reembed,
    /// Store raw source material as an episode.
    Ingest(args::IngestArgs),
    /// Extract entities, facts, and relationships from an episode or inline content.
    Extract(args::ExtractArgs),
    /// Resolve entity aliases to a canonical entity id.
    Resolve(args::ResolveArgs),
    /// Invalidate a fact while preserving historical traceability.
    Invalidate(args::InvalidateArgs),
    /// Explain context items with provenance-ready citations.
    Explain(args::ExplainArgs),
    /// Assemble ranked, relevant context for a query.
    AssembleContext(args::AssembleContextArgs),
}
