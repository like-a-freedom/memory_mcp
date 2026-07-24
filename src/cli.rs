//! CLI module — clap-based command surface shared between runtime modes
//! (serve / watch / reembed) and one-shot memory tool subcommands.

pub mod args;
pub mod commands;
pub mod runtime;

use clap::{Parser, Subcommand};

pub use runtime::{
    WatchCommand, build_memory_service, log_session_duration, log_startup, run_reembed_mode,
    run_stdio_server, run_watch_mode,
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
    Reembed(args::ReembedArgs),
    /// Store raw source material as an episode (source_type, source_id, content).
    /// Requires `--source-type`, `--source-id`, `--content`, `--t-ref` (ISO 8601).
    /// Output: ToolResponse with status, episode_id. Next step: `extract --episode-id <id>`.
    Ingest(args::IngestArgs),
    /// Extract entities, facts, and relationships from an episode or inline content.
    /// Provide exactly one input: `--episode-id` (from ingest) or `--content` inline.
    /// Output: ToolResponse with lists of entities and facts. Next step: `resolve`
    /// to deduplicate aliases, or `assemble-context` to query stored facts.
    Extract(args::ExtractArgs),
    /// Resolve entity aliases to a canonical entity id (deduplication).
    /// Merges `--aliases` under `--canonical-name` for `--entity-type`.
    /// Output: ToolResponse with canonical_id. Run after extraction to clean up entities.
    Resolve(args::ResolveArgs),
    /// Invalidate a fact while preserving historical traceability.
    /// Requires `--fact-id`, `--reason`, `--t-invalid` (ISO 8601).
    /// Marks the fact as no longer valid without deleting it.
    Invalidate(args::InvalidateArgs),
    /// Explain context items with provenance-ready citations.
    /// Takes a JSON array from `assemble-context` output as `--context-items`.
    /// Output: ToolResponse with source snippets and provenance data.
    Explain(args::ExplainArgs),
    /// Assemble ranked, relevant context for a query.
    /// Searches stored facts matching `--query`, scoped by `--scope` (org/team/personal).
    /// Output: ToolResponse with ranked context_items. Next step: pipe items to `explain`.
    AssembleContext(args::AssembleContextArgs),
    /// Internal: capture a lifecycle event (hidden from --help).
    /// Consumed by hook scripts, not a public tool. See ADR-0016 AD-4.
    #[command(hide = true)]
    LifecycleCapture(args::LifecycleCaptureArgs),
    /// Internal: recall lifecycle context (hidden from --help).
    /// Consumed by hook scripts, not a public tool. See ADR-0016 AD-5.
    #[command(hide = true)]
    LifecycleRecall(args::LifecycleRecallArgs),
}
