//! CLI module — clap-based command surface shared between runtime modes
//! (serve / watch / reembed) and one-shot memory tool subcommands.

pub mod args;
pub mod commands;
pub mod runtime;

use std::future::Future;
use std::pin::Pin;

use clap::{Parser, Subcommand};

use crate::service::{MemoryError, MemoryService};

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
    /// Inspect and run lifecycle maintenance operations.
    Lifecycle(args::LifecycleArgs),
    /// Print copy-paste configuration for an MCP host without changing files.
    Init(args::InitArgs),
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
    /// Searches stored facts matching `--query` in the process Active Namespace.
    /// Output: ToolResponse with ranked context_items. Next step: pipe items to `explain`.
    AssembleContext(args::AssembleContextArgs),
    /// Internal: capture a scope-free lifecycle event (hidden from --help).
    /// Consumed by hook scripts, not a public tool. Legacy scope/project event fields are rejected.
    #[command(hide = true)]
    LifecycleCapture(args::LifecycleCaptureArgs),
    /// Internal: recall scope-free lifecycle context (hidden from --help).
    /// Consumed by hook scripts, not a public tool. Legacy scope/project event fields are rejected.
    #[command(hide = true)]
    LifecycleRecall(args::LifecycleRecallArgs),
}

/// Erased one-shot runner: executes a CLI tool subcommand against a built
/// service. Each runner captures its own clap `*Args` at construction time,
/// so dispatch can build the service once and run any one-shot subcommand
/// through a single code path.
// Clippy: the nested boxed-future type is inherent to erasing heterogeneous
// async runners; unwrapping it into a struct adds noise without clarity.
#[allow(clippy::type_complexity)]
pub type OneShotRunner = Box<
    dyn for<'a> FnOnce(
            &'a MemoryService,
        )
            -> Pin<Box<dyn Future<Output = Result<(), MemoryError>> + Send + 'a>>
        + Send,
>;

impl Command {
    /// Stable log label for the startup log. Single source of truth for
    /// mode names — `runner.rs` derives its startup log from here.
    pub fn mode_label(&self) -> &'static str {
        match self {
            Command::Serve => "serve",
            Command::Watch(_) => "watch",
            Command::Reembed(_) => "reembed",
            Command::Lifecycle(_) => "cli.lifecycle",
            Command::Init(_) => "cli.init",
            Command::Ingest(_) => "cli.ingest",
            Command::Extract(_) => "cli.extract",
            Command::Resolve(_) => "cli.resolve",
            Command::Invalidate(_) => "cli.invalidate",
            Command::Explain(_) => "cli.explain",
            Command::AssembleContext(_) => "cli.assemble_context",
            Command::LifecycleCapture(_) => "cli.lifecycle_capture",
            Command::LifecycleRecall(_) => "cli.lifecycle_recall",
        }
    }

    /// Returns the erased one-shot runner for one-shot tool subcommands.
    ///
    /// Each runner captures its clap `*Args`, so `runner.rs` can build the
    /// service once and dispatch every one-shot subcommand through a single
    /// code path. The service modes (serve / watch / reembed / init) return
    /// `None` and are dispatched before this is reached.
    pub fn into_one_shot(self) -> Option<OneShotRunner> {
        macro_rules! one_shot {
            ($runner:ident, $args:expr) => {
                Some(Box::new(move |service| {
                    Box::pin(commands::$runner(service, $args))
                }))
            };
        }

        match self {
            Command::Serve | Command::Watch(_) | Command::Reembed(_) | Command::Init(_) => None,
            Command::Lifecycle(args) => one_shot!(run_lifecycle, args),
            Command::Ingest(args) => one_shot!(run_ingest, args),
            Command::Extract(args) => one_shot!(run_extract, args),
            Command::Resolve(args) => one_shot!(run_resolve, args),
            Command::Invalidate(args) => one_shot!(run_invalidate, args),
            Command::Explain(args) => one_shot!(run_explain, args),
            Command::AssembleContext(args) => one_shot!(run_assemble_context, args),
            Command::LifecycleCapture(args) => one_shot!(run_lifecycle_capture, args),
            Command::LifecycleRecall(args) => one_shot!(run_lifecycle_recall, args),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_subcommand_parses_typed_operation() {
        let cli = Cli::try_parse_from(["memory_mcp", "lifecycle", "recompute-decay", "--dry-run"])
            .expect("lifecycle command should parse");

        let Some(Command::Lifecycle(args)) = cli.command else {
            panic!("expected lifecycle command");
        };
        assert!(matches!(
            args.operation,
            args::LifecycleOperation::RecomputeDecay {
                dry_run: true,
                confirmed: false
            }
        ));
    }

    #[test]
    fn lifecycle_command_is_one_shot_and_has_stable_mode_label() {
        let cli = Cli::try_parse_from(["memory_mcp", "lifecycle", "dashboard"])
            .expect("lifecycle command should parse");
        let command = cli.command.expect("command");

        assert_eq!(command.mode_label(), "cli.lifecycle");
        assert!(command.into_one_shot().is_some());
    }
}
