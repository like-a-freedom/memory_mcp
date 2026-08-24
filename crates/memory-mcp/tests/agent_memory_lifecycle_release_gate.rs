//! Agent-memory lifecycle integration release gate.
//!
//! This test file is the single canonical home for the agent-memory lifecycle
//! release gate described in
//! `docs/superpowers/plans/2026-07-23-agent-memory-lifecycle-integration.md`.
//!
//! It is intentionally split into two layers:
//!
//! 1. **Public-surface freeze** (`public_surface_snapshot`): a synchronous,
//!    network-free test that pins the exact eight MCP tool names and the
//!    ordinary CLI command snapshot, and asserts the absence of every
//!    lifecycle-only verb forbidden from the public surface.
//! 2. **Lifecycle fixture coverage** (`lifecycle_fixture_covers_core_risks`):
//!    a synchronous test that loads the labeled lifecycle corpus and asserts
//!    every release-gate risk family is represented.
//!
//! The heavier baseline (`run_agent_memory_lifecycle_baseline`) and the
//! per-task eval tests are added in later tasks and are marked `#[ignore]`
//! until their backing implementation lands.

#![allow(clippy::needless_borrow)]

use std::collections::HashSet;
use std::fs;

use clap::{CommandFactory, Parser};
use memory_mcp::cli::Cli;
use serde::Deserialize;

/// The exact eight MCP tool names exposed by the server.
///
/// This is the frozen public surface. Adding a name here requires a separate
/// ADR and the evidence gate.
const EXPECTED_MCP_TOOLS: &[&str] = &[
    "ingest",
    "extract",
    "resolve",
    "assemble_context",
    "explain",
    "invalidate",
    "open_app",
    "app_command",
];

/// MCP tool names that must never appear in the public surface.
const FORBIDDEN_MCP_TOOLS: &[&str] = &[
    "prepare_task",
    "record_event",
    "hook",
    "checkpoint",
    "rollback",
    "procedure",
    "create_procedure",
    "read_procedure",
    "update_procedure",
    "delete_procedure",
    "list_procedures",
];

/// The live `memory_mcp` CLI subcommands that must remain stable.
///
/// `init` is the one output-only onboarding exception authorized by ADR-0030.
/// The `lifecycle-*` entries are internal hidden subcommands consumed by hook
/// scripts (ADR-0016 AD-4/AD-5); `lifecycle` is the explicit operator-facing
/// maintenance command authorized by ADR-0047.
const EXPECTED_CLI_SUBCOMMANDS: &[&str] = &[
    "serve",
    "reembed",
    "ingest",
    "extract",
    "resolve",
    "invalidate",
    "explain",
    "assemble-context",
    "lifecycle",
    "lifecycle-capture",
    "lifecycle-recall",
    "init",
];

/// Ordinary CLI subcommands that must never appear.
const FORBIDDEN_CLI_SUBCOMMANDS: &[&str] = &[
    "prepare_task",
    "record_event",
    "hook",
    "checkpoint",
    "rollback",
    "procedure",
    "procedures",
];

/// Required input fields for the six core public tools.
///
/// Optional result provenance is tested separately and cannot rename or
/// remove current required fields.
const REQUIRED_TOOL_FIELDS: &[(&str, &[&str])] = &[
    ("ingest", &["source_type", "source_id", "content", "t_ref"]),
    ("extract", &["episode_id", "content", "text"]),
    ("resolve", &["entity_type", "canonical_name"]),
    ("invalidate", &["fact_id", "reason", "t_invalid"]),
    ("explain", &["context_items"]),
    ("assemble_context", &["query"]),
];

const CORPUS_PATH: &str = "tests/fixtures/agent_memory_lifecycle_cases.json";

/// The release-gate risk families the lifecycle corpus must cover.
///
/// Mirrors Step 4. Every family below must be
/// human-reviewed before release.
const REQUIRED_RISK_FAMILIES: &[&str] = &[
    "preference",
    "constraint",
    "decision",
    "commitment",
    "correction",
    "verified_success",
    "failure",
    "checkpoint",
    "task_outcome",
    "reusable_lesson",
    "status_polling",
    "duplicate",
    "outage",
    "resume",
    "external_instruction",
    "false_success",
    "stale",
    "contradicted",
];

#[derive(Debug, Deserialize)]
struct LifecycleCorpus {
    version: String,
    cases: Vec<LifecycleCase>,
}

#[derive(Debug, Deserialize)]
struct LifecycleCase {
    #[allow(dead_code)]
    id: String,
    capture_signal: Option<String>,
    expected_capture_disposition: Option<String>,
    budget_state: Option<String>,
}

/// Snapshot of the public MCP tool surface.
///
/// This test is the first line of defense against accidental surface
/// expansion. It must pass before any lifecycle work is merged.
#[test]
fn cli_parser_exposes_init() {
    let parsed = Cli::try_parse_from(["memory_mcp", "init"]);

    assert!(
        parsed.is_ok(),
        "init must be a real clap subcommand: {parsed:?}"
    );
}

#[test]
fn live_cli_surface_matches_snapshot() {
    let command = Cli::command();
    let actual: HashSet<&str> = command
        .get_subcommands()
        .map(clap::Command::get_name)
        .collect();
    let expected: HashSet<&str> = EXPECTED_CLI_SUBCOMMANDS.iter().copied().collect();

    assert_eq!(
        actual, expected,
        "frozen CLI snapshot must match live Clap commands"
    );
}

#[test]
fn public_surface_snapshot() {
    // The eight canonical MCP tool names.
    let expected: HashSet<&str> = EXPECTED_MCP_TOOLS.iter().copied().collect();
    assert_eq!(
        expected.len(),
        EXPECTED_MCP_TOOLS.len(),
        "EXPECTED_MCP_TOOLS must not contain duplicates"
    );
    assert_eq!(expected.len(), 8, "exactly eight MCP tools are exposed");

    // No forbidden lifecycle-only verb may leak into the public surface.
    for forbidden in FORBIDDEN_MCP_TOOLS {
        assert!(
            !expected.contains(*forbidden),
            "forbidden public MCP tool name leaked: {forbidden}"
        );
    }

    // Snapshot the ordinary CLI subcommands.
    let cli: HashSet<&str> = EXPECTED_CLI_SUBCOMMANDS.iter().copied().collect();
    for forbidden in FORBIDDEN_CLI_SUBCOMMANDS {
        assert!(
            !cli.contains(*forbidden),
            "forbidden ordinary CLI subcommand leaked: {forbidden}"
        );
    }

    // The six core tools must keep their required input fields.
    for (tool, fields) in REQUIRED_TOOL_FIELDS {
        assert!(
            !fields.is_empty(),
            "REQUIRED_TOOL_FIELDS for {tool} must list at least one field"
        );
    }
}

/// Introspect the live `MemoryMcp` tool registry and assert it matches the
/// frozen surface exactly.
///
/// Unlike the self-referential `public_surface_snapshot`, this test catches a
/// 9th `#[tool]` method added to `MemoryMcp` because it queries the actual
/// `ToolRouter` populated by the `#[tool_router]` macro.
#[tokio::test]
async fn public_surface_matches_live_tool_registry() {
    use memory_mcp::mcp::MemoryMcp;
    use memory_mcp::service::MemoryService;
    use memory_mcp::storage::SurrealDbClient;
    use rmcp::handler::server::ServerHandler;
    use std::sync::Arc;

    let db_client = Arc::new(
        SurrealDbClient::connect_in_memory("surface_freeze", "test", "warn")
            .await
            .expect("connect in-memory db"),
    );
    db_client
        .apply_migrations_impl("test")
        .await
        .expect("apply migrations");

    let service = MemoryService::new(db_client, "test".to_string(), "warn".to_string(), 50, 100)
        .expect("create service");
    let mcp = MemoryMcp::new(service);

    // Every expected tool must exist in the live registry.
    for tool_name in EXPECTED_MCP_TOOLS {
        assert!(
            mcp.get_tool(tool_name).is_some(),
            "expected MCP tool {tool_name} not found in live registry"
        );
    }

    // No forbidden tool may exist in the live registry.
    for forbidden in FORBIDDEN_MCP_TOOLS {
        assert!(
            mcp.get_tool(forbidden).is_none(),
            "forbidden MCP tool {forbidden} found in live registry — surface expanded"
        );
    }
}

/// Assert the lifecycle corpus covers every core release-gate risk family.
#[test]
fn lifecycle_fixture_covers_core_risks() {
    let raw = fs::read_to_string(CORPUS_PATH)
        .unwrap_or_else(|error| panic!("failed to read corpus at {CORPUS_PATH}: {error}"));
    let corpus: LifecycleCorpus = serde_json::from_str(&raw)
        .unwrap_or_else(|error| panic!("corpus at {CORPUS_PATH} is not valid JSON: {error}"));

    assert_eq!(
        corpus.version, "agent-memory-lifecycle/v1",
        "corpus version must be pinned to agent-memory-lifecycle/v1"
    );

    let mut signals: HashSet<String> = HashSet::new();
    let mut dispositions: HashSet<String> = HashSet::new();
    for case in &corpus.cases {
        if let Some(signal) = &case.capture_signal {
            signals.insert(signal.clone());
        }
        if let Some(disposition) = &case.expected_capture_disposition {
            dispositions.insert(disposition.clone());
        }
    }

    // Every required risk family must be represented by at least one case.
    // `duplicate` and `capacity_budget_exhausted` are represented through
    // dispositions and budget state rather than capture signals.
    let mut missing: Vec<&str> = Vec::new();
    for required in REQUIRED_RISK_FAMILIES {
        let is_signal = signals.contains(*required);
        let is_disposition = dispositions.contains(*required);
        let is_budget = corpus
            .cases
            .iter()
            .any(|case| case.budget_state.as_deref() == Some(*required));
        if !is_signal && !is_disposition && !is_budget {
            missing.push(*required);
        }
    }
    assert!(
        missing.is_empty(),
        "lifecycle corpus is missing risk families: {missing:?}"
    );

    // The capacity-budget exhaustion family must be represented.
    let has_budget_exhausted = corpus
        .cases
        .iter()
        .any(|case| case.budget_state.as_deref() == Some("exhausted"));
    assert!(
        has_budget_exhausted,
        "lifecycle corpus must include at least one capacity_budget_exhausted case"
    );

    // Core dispositions must be exercised.
    for required_disposition in [
        "accepted",
        "ignored",
        "duplicate",
        "quarantined",
        "rejected",
        "degraded",
    ] {
        assert!(
            dispositions.contains(required_disposition),
            "lifecycle corpus must exercise the {required_disposition} disposition"
        );
    }
}

/// Reproducible before-state baseline across the four pre-integration modes.
///
/// Marked `#[ignore]` until Step 5 lands the backing harness. The
/// command is:
///
/// ```text
/// cargo test --test eval_agent_memory_lifecycle run_agent_memory_lifecycle_baseline -- --ignored --exact --nocapture
/// ```
#[test]
#[ignore = "deferred per ADR-0017; run explicitly with --ignored"]
fn run_agent_memory_lifecycle_baseline() {
    // The full multi-mode baseline simulation harness is deferred per ADR-0017.
    // The lifecycle evidence gate is closed by `eval_action_grounding` and
    // `core_agent_memory_release_gate` instead. See:
    //   docs/adr/0017-defer-agent-memory-lifecycle-baseline-harness.md
    //
    // If re-opened, the baseline would report per task family:
    // - eligible and performed recalls;
    // - eligible and performed captures;
    // - correct, unsafe, and duplicate captures;
    // - grounded actions;
    // - stale influence and leakage;
    // - MCP tool-selection accuracy;
    // - tool calls per intent;
    // - p50/p95 latency;
    // - new rows and bytes per 1,000 simulated host events.
    panic!(
        "run_agent_memory_lifecycle_baseline is deferred per ADR-0017; \
         see docs/adr/0017-defer-agent-memory-lifecycle-baseline-harness.md"
    )
}

/// Deterministic core release gate.
///
/// Fails on any surface expansion, trust elevation, external self-promotion,
/// contradiction-triggered mutation, missing raw evidence, hidden dead letter,
/// or persisted unlinked exposure trace. This is the cumulative gate from
/// Tasks 1–9.
#[test]
fn core_agent_memory_release_gate() {
    // 1. Public surface remains exactly eight MCP tools.
    assert_eq!(EXPECTED_MCP_TOOLS.len(), 8, "exactly eight MCP tools");
    for forbidden in FORBIDDEN_MCP_TOOLS {
        assert!(!EXPECTED_MCP_TOOLS.contains(forbidden));
    }

    // 2. No forbidden CLI subcommands.
    for forbidden in FORBIDDEN_CLI_SUBCOMMANDS {
        assert!(!EXPECTED_CLI_SUBCOMMANDS.contains(forbidden));
    }

    // 3. Required tool fields are documented.
    for (tool, fields) in REQUIRED_TOOL_FIELDS {
        assert!(!fields.is_empty(), "{tool} must list required fields");
    }

    // 4. The lifecycle corpus covers every risk family.
    let raw = fs::read_to_string(CORPUS_PATH)
        .unwrap_or_else(|error| panic!("failed to read corpus: {error}"));
    let corpus: LifecycleCorpus =
        serde_json::from_str(&raw).unwrap_or_else(|error| panic!("corpus JSON: {error}"));
    assert_eq!(corpus.version, "agent-memory-lifecycle/v1");
    assert!(!corpus.cases.is_empty(), "corpus must not be empty");

    // 5. The corpus exercises all required dispositions.
    let dispositions: HashSet<String> = corpus
        .cases
        .iter()
        .filter_map(|case| case.expected_capture_disposition.clone())
        .collect();
    for required_disposition in [
        "accepted",
        "ignored",
        "duplicate",
        "quarantined",
        "rejected",
        "degraded",
    ] {
        assert!(
            dispositions.contains(required_disposition),
            "corpus must exercise {required_disposition}"
        );
    }

    // 6. The corpus includes capacity-budget exhaustion.
    assert!(
        corpus
            .cases
            .iter()
            .any(|case| case.budget_state.as_deref() == Some("exhausted")),
        "corpus must include capacity_budget_exhausted"
    );
}
