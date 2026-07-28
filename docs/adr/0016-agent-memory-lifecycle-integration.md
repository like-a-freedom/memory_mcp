# ADR 0016: Agent Memory Lifecycle Integration

Status: Proposed (2026-07-23)

## Context

`memory_mcp` exposes eight MCP tools (`ingest`, `extract`, `resolve`,
`assemble_context`, `explain`, `invalidate`, `open_app`, `app_command`) and the
ordinary CLI equivalents of the six core tools. Whether and when an agent host
actually calls `assemble_context` before consequential work, or captures a
significant outcome after it occurs, is currently left to model choice. That choice is unreliable: it depends on the model remembering the workflow across
compaction and restarts, and the server cannot enforce it.

This ADR records the architectural decisions that let supported agent hosts
consult `memory_mcp` at lifecycle boundaries without a new public tool, a new
ordinary CLI subcommand, or any caller-controlled trust argument.

The decision is grounded in the implementation plan
`docs/superpowers/plans/2026-07-23-agent-memory-lifecycle-integration.md`,
which is the single active implementation plan for this scope.

## Decision

### AD-1 — Lifecycle enforcement is a control plane, not agent UI

A supported host adapter observes lifecycle boundaries and invokes internal
MCP instructions and tool descriptions remain useful in bare-MCP mode. They do
not enforce; only the host adapter does.

### AD-2 — Freeze the current public tool and ordinary CLI surface

Internal capabilities (`LifecycleCapture`, `LifecycleWorkerRuntime`) are not
registered in `tools/list`, are not CLI subcommands, and have no public JSON
schema. They call the same service/tool modules used by `assemble_context`
and inline `extract`.

### AD-3 — Transport authority outside tool arguments

Trust is derived from the invocation channel and configured server policy; public
MCP and CLI arguments never set final trust. The ordinary path constructs
`InvocationOrigin::AgentSelected`; a configured lifecycle bridge constructs
`InvocationOrigin::LifecycleAdapter`. The model cannot choose either type or
its identity.

### AD-4 — Host bridge mechanism

Supported automatic integration uses **standard transports only** — no
custom Unix socket listener, no separate bridge binary. The lifecycle bridge
operates through three complementary surfaces:

1. **MCP stdio (primary)** — the existing `memory_mcp serve` path. The agent
   calls `assemble_context` / `ingest` / `extract` through the standard MCP
   protocol. This is the **default mechanism** and works with every
   MCP-compatible host. `AGENTS.md` and the `memory-mcp` skill instruct the
   agent when and how to use these tools.

2. **Hooks (supplementary)** — external shell scripts (not part of the Rust
   binary) installed per-host. Hooks fire on lifecycle events (SessionStart,
   PostToolUse, Stop, etc.) and call the memory server through the **ordinary
   CLI** (`memory_mcp ingest`, `memory_mcp assemble_context`) or a simple
   HTTP endpoint if configured. Hooks are agent-runtime-dependent: Claude Code
   supports them natively; Codex supports a subset; other harnesses may not
   support hooks at all. When hooks are unavailable, the MCP stdio path
   remains fully functional.

3. **AGENTS.md + skill (instructive layer)** — `AGENTS.md` at the project
   root and the `memory-mcp` skill provide the agent with clear instructions
   on when to recall before significant work and when to capture outcomes.
   This is the **primary, universal mechanism** — it works with every
   agent that reads project instructions, without requiring hooks.

The lifecycle integration uses these three surfaces only — no bridge adapters,
no host normalization code, no custom transport. Hook scripts call the ordinary
CLI directly; the CLI enforces scope, trust, and policy through the existing
path. Stable event identity and deduplication are handled by `LifecycleCapture`
via `load_event` + `compute_event_id`.

### AD-5 — Selective recall over existing `assemble_context`

For one eligible host event: normalize the host event and task, compute a
recall key, suppress a duplicate recall unless the task changed / compaction
occurred / relevant memory changed / the previous result is stale, call the
existing context pipeline exactly once, wrap the returned items in a stable
"memory is data" boundary, keep an in-memory trace, and persist that trace only
if a later significant event references it or an evaluation sample explicitly
requests persistence.

### AD-6 — Selective capture over existing inline `extract`

For one eligible host event: a deterministic salience policy classifies it as
ignored, accepted, quarantined, rejected, or degraded; derive a stable
event/source ID; store bounded canonical content and artifact references (not
an unbounded tool dump); reuse inline-extract validation, deterministic
episode preparation, extraction, embedding, and claim projection; persist
accepted raw evidence once before fallible projection; schedule durable
projection and return promptly.

### AD-7 — Exposure traces are ephemeral by default

There is no durable receipt row for every recall. A per-session LRU holds at
most 32 traces for 30 minutes, and a significant captured event may copy a
bounded trace link. This proves exposure, not causal use.

### AD-8 — Immutable evidence implies controlled, not zero, growth

The database stays append-oriented for evidence and facts. Growth is controlled
at ingestion: ignored and duplicate events create zero new durable rows;
accepted content is stored once; lifecycle content is bounded; quotas and
project daily budgets prevent unbounded automatic ingestion while preserving
immutable-domain growth.

## Consequences

- The public MCP surface stays at exactly eight tools. Any future proposal for
  a new public tool requires a separate ADR and the evidence gate.
- Agent hosts gain reliable, model-independent recall and capture at
  documented lifecycle boundaries.
- Trust is never caller-controlled; external content cannot become privileged
  instruction, preference, policy, retraction, or procedure.
- Procedural memory remains a separately gated bounded context projected through
  the existing `FactType::Experience` seam, exposed only after the procedure gate
  is met.
