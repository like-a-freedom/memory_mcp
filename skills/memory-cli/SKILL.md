---
name: memory-cli
description: "Memory CLI workflows for shell automation and operations. Use when invoking `memory_mcp` commands to capture or recall memory, start the stdio server, watch files, re-embed facts, or integrate memory into scripts and hooks."
compatibility: Requires the `memory_mcp` binary in the current environment. This skill is CLI-only; use `memory-mcp` when the server is already connected as MCP tools.
---

# Memory CLI

Use the CLI for shell scripts, one-shot automation, hooks, server operation, file
watching, and re-embedding. Do not route an MCP-connected agent through the
shell merely because the CLI exposes equivalent memory operations.

Choose one branch before running a command:

- **capture** — store a source and extract facts through JSON-producing
  subcommands;
- **recall** — read facts and provenance without writes;
- **canonicalize** — create or update a canonical entity and its aliases;
- **serve** — start the stdio MCP server;
- **watch** — continuously ingest files;
- **re-embed** — rebuild stored fact embeddings.

## Principles

1. **Verified before claimed.** `ingest` alone records an episode; `extract`
   establishes the durable-fact outcome.
2. **Storage boundary is server config.** The Active Namespace is chosen once at
   server startup; commands never pass, invent, or probe a namespace, scope, or
   project.
3. **Bi-temporal truth.** Use the source's time for `--t-ref`; use `--as-of` for
   historical recall.
4. **Append, then invalidate.** Preserve source identity and the audit trail.
5. **Machine-readable boundary.** Parse stdout as JSON and use the process exit
   status. Keep logs and diagnostics separate from persisted content.
6. **Read back mutations.** A command invocation is not evidence of persisted
   state; verify its response or a subsequent read.

Read the [memory contract](references/memory-contract.md) before
the first write, when choosing time fields, or when interpreting an
empty, failed, or conflicting result.

Read the [CLI command reference](references/commands.md) before constructing a
command, automating exit handling, or running `serve`, `watch`, or `reembed`.
Exact flags remain behind that pointer; this file owns behavior and order.

## Workflow: one-shot capture

1. **Frame the source.** Establish the authoritative source, stable
   `source_id`, truthful reference time, and bounded, secret-free content.

   *Completion:* every required value is traceable and safe to place on the
   command line in the current environment.

2. **Ingest once.** Run `memory_mcp ingest` with explicit flags. Capture stdout,
   stderr, and exit status separately.

   *Completion:* parse a returned `episode_id`, or report `pending` while
   preserving the intended source handles.

3. **Extract and inspect.** Run `memory_mcp extract --episode-id ...`. Inspect
   facts and warnings rather than treating exit zero as fact capture.

   *Completion:* classify the result as `verified`, `episode-only`, or
   `pending`.

4. **Reconcile supersession when required.** Run `memory_mcp invalidate` for an
   outdated active fact after the replacement has been verified.

   *Completion:* the invalidation response names the fact, reason, and time; a
   subsequent recall no longer treats it as current.

## Workflow: canonicalize

Use after extraction when evidence-backed aliases must converge on one durable
entity. `memory_mcp resolve` is a write command: it may create the canonical
entity or persist aliases.

1. Establish the entity type, canonical name, and source-backed aliases.
2. Run `memory_mcp resolve` once and parse its response.
3. Retain the returned `entity_id` for later linking.

*Completion:* report the canonical ID and persisted reconciliation outcome, or
leave the operation `pending`.

## Workflow: one-shot recall

1. Frame the query and time boundary.
2. Run `memory_mcp assemble-context` and parse the complete JSON envelope.
3. Run `memory_mcp explain` for every item used as evidence.

*Completion:* all used claims have provenance; empty results and failures are
reported explicitly; no write command ran during the recall workflow.

## Workflow: serve

1. Validate required configuration without printing credentials.
2. Start `memory_mcp serve` and preserve stdio for MCP framing.
3. Treat unexpected stdout as a protocol defect; route diagnostics to stderr.
4. Confirm readiness through the MCP client's initialization handshake.

*Completion:* a client completes initialization and can list the expected tools,
or startup is reported failed with the process status and diagnostic.

## Workflow: watch

1. Resolve the exact directory to watch.
2. Start `memory_mcp watch` only after confirming continuous ingestion is
   intended.
3. Observe per-file results; distinguish successful episode creation, empty
   extraction, and failures.
4. Stop the watcher deliberately and report unprocessed or failed files.

*Completion:* the watched boundary and outcome counts are known; failures remain
actionable and no directory outside the intended boundary was watched.

## Workflow: re-embed

1. Confirm that the embedding provider, model, or dimension actually changed,
   or that a failed run is being resumed.
2. Run `memory_mcp reembed`; choose failure budget and retry mode deliberately.
3. Monitor terminal status and preserve failed fact identifiers.
4. Verify the terminal job state and failure count.

*Completion:* the job is `completed` or `completed_with_errors` with counts
reported; `failed` and `interrupted` remain unfinished states.

## Exit gate

The workflow is complete only when every step in the chosen branch satisfies
its completion criterion, structured output has been parsed, and persisted
mutations have been read back. A zero exit code alone is never the completion
criterion.
