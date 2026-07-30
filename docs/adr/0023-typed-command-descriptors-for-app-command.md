# ADR-0023: Typed Command Descriptors for App Command Dispatch

> Status: Accepted (2026-07-30)
> Supersedes the dispatch *shape* implied by ADR-0001, without touching the protocol surface.

## Context

ADR-0001 moved app/lifecycle domain policy into `service/apps` as typed
`AppCommand` values and `LifecycleCommandOutcome` results. The *classification*
of a command moved into the domain layer; the *dispatch shape* did not.

`mcp/handlers.rs::app_command` is one function of ~700 lines containing ~10
match arms that each repeat the same pipeline: parse → validate → authorize →
confirm-gate → execute → shape → log. Every new command copies the last arm.
Because the match arm destructures a command *after* matching, each arm carries
a production `unreachable!()` to satisfy the borrow checker — directly against
the CONTEXT.md constraint: *"Production code uses `MemoryError` and `Result`;
no production `unwrap`, `expect`, or `panic`."*

This is the repo's highest-churn shallow module: the shape changes on every
lifecycle command addition, and bugs concentrate in the copy-paste between
arms.

## Decision

Replace the per-arm match with a **typed command executor** in
`service/apps/dispatch.rs`:

1. Each command declares a static `AppCommandDescriptor`:
   - action name(s), expected session app, confirmation requirement
   - `execute: fn(&AppContext, &AppCommand) -> Result<AppCommandOutcome, MemoryError>`
   - `shape: fn(&AppCommandOutcome) -> serde_json::Value` (response detail)
2. `app_command` in `handlers.rs` becomes a single ~50-line dispatch:
   parse → authorize → confirmation gate → lookup descriptor → `execute` →
   `shape` → log. No match arms. No `unreachable!()`.
3. Command identity moves from fragile stringly-matched `action_name()` +
   post-hoc destructuring to exhaustive dispatch on the *descriptor*; the
   compiler enforces coverage at declaration time, not at arm-matching time.
4. Logging, request-ID threading, timing, and `refresh_required` semantics
   are centralized in the executor — visible once, not per-arm.

## Concrete end-state (for the executing plan)

```rust
// crates/memory-mcp/src/service/apps/dispatch.rs
pub enum AppCommand {
    ApproveItems { item_ids: Vec<String> },
    RejectItems { item_ids: Vec<String>, reason: String },
    EditItem { item_id: String, patch: Value },
    ArchiveCandidates,
    RestoreArchived,
    RecomputeDecay,
    RebuildCommunities,
    GraphExpand,
    DiffExport,
    // …
}

pub struct AppCommandDescriptor {
    pub names: &'static [&'static str],
    pub app: &'static str,
    pub requires_confirmation: bool,
    pub execute: fn(&AppContext, &AppCommand)
        -> futures::future::BoxFuture<'_, Result<AppCommandOutcome, MemoryError>>,
}

pub const COMMAND_TABLE: &[AppCommandDescriptor] = &[ … ];

pub async fn app_command(
    handler: &MemoryMcp,
    params: AppCommandParams,
) -> Result<Json<ToolResponse<AppCommandResult>>, ErrorData> {
    // parse → authorize → confirm-gate → lookup descriptor → execute → shape → log
}
```

The handler's ~700-line match block is replaced by the ~50 lines above; each
existing arm becomes one `AppCommandDescriptor` row in `COMMAND_TABLE`.

## Consequences

- Adding a lifecycle/app command adds one descriptor row in `service/apps`,
  not one 60-line arm in the MCP adapter.
- The 15 production `unreachable!()` sites in `handlers.rs` are removed; the
  CONTEXT.md constraint is true again.
- The confirmation policy lives in the descriptor, testable without
  constructing an MCP request envelope.
- The handler no longer grows with domain vocabulary.
- One-time refactor reshuffles ~700 lines; test coverage in
  `apps_lifecycle.rs`, `apps_ingestion_review.rs`, and
  `action_grounding.rs` gates the behavior.

## Alternatives Considered

### Keep the match, remove the unreachable via `if let`

Rejected — the shape still multiplies per command; every new command still
copies ~60 lines into `handlers.rs`. Leak persists.

### Route each action to its own trait method on the service

Rejected — that re-introduces the god-object interface the ADR-0001
capabilities were designed to avoid; the trait surface grows per command.

## Verification

- `cargo test --workspace --all-targets --features cli-watch,mcp-apps` passes.
- `public_surface_snapshot` test remains green (frozen 8-tool surface unchanged).
- Lifecycle release gates (`action_grounding_pass_rate`, `poisoning_pass_rate`)
  and the `lifecycle-public-surface` gate keep passing at v5 levels.
- `grep -c "unreachable!" crates/memory-mcp/src/mcp/handlers.rs` = 0.
