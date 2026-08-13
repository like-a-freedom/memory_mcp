# ADR 0016: Agent Memory Lifecycle Integration

Status: Accepted (2026-07-23; implemented)

Amended by:

- ADR-0030 (`memory_mcp init` is the sole output-only exception to the ordinary CLI freeze).
- ADR-0038 (authorizes the scope-free breaking schema revision while preserving
  the eight MCP tool names and command set; removes scope routing/enforcement;
  replaces project daily budgets with an Active-Namespace/process budget).

## Context

`memory_mcp` exposes eight MCP tools (`ingest`, `extract`, `resolve`,
`assemble_context`, `explain`, `invalidate`, `open_app`, `app_command`) and the
ordinary CLI equivalents of the six core tools. Whether and when an agent host
actually calls `assemble_context` before consequential work, or captures a
significant outcome after it occurs, is currently left to model choice. That choice is unreliable: it depends on the model remembering the workflow across
compaction and restarts, and the server cannot enforce it.

This ADR records the architectural decisions that let supported agent hosts
consult `memory_mcp` at lifecycle boundaries without a new public MCP tool, a new
ordinary lifecycle CLI subcommand, or any caller-controlled trust argument. The
separate output-only onboarding exception, `memory_mcp init`, is authorized by
ADR-0030 and is not part of lifecycle integration.

The decision is grounded in the implementation plan
`docs/superpowers/plans/2026-07-23-agent-memory-lifecycle-integration.md`,
which is the single active implementation plan for this scope.

## Decision

### AD-1 — Lifecycle enforcement is a control plane, not agent UI

A supported host adapter observes lifecycle boundaries and invokes internal
MCP instructions and tool descriptions remain useful in bare-MCP mode. They do
not enforce; only the host adapter does.

### AD-2 — Freeze the current public tool and ordinary CLI surface

The eight MCP tools remain the frozen public memory surface. Internal capabilities
(`LifecycleCapture`, `LifecycleWorkerRuntime`) are not registered in `tools/list`
and have no public JSON schema. Their hidden lifecycle CLI subcommands are
internal transport entry points, not part of the ordinary public CLI surface. They
call the same service/tool modules used by `assemble_context` and inline `extract`.

The ordinary CLI freeze has one explicit exception: the output-only onboarding
command `memory_mcp init`, authorized by ADR-0030. It is not a lifecycle verb, does
not expose a memory capability, does not build a service, and does not change the
eight-tool MCP surface.

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
CLI directly. As amended by ADR-0038, there is no request-scope enforcement;
channel-derived trust and independent policy-tag enforcement remain on the
existing path. Stable event identity and deduplication are handled by
`LifecycleCapture` via `load_event` + `compute_event_id`.

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
accepted content is stored once; lifecycle content is bounded. As amended by
ADR-0038, quotas and a process/Active-Namespace daily budget replace project
daily budgets while preserving the same bounded-growth intent.

## Consequences

- The public MCP surface stays at exactly eight tools. `memory_mcp init` is the
  sole output-only onboarding exception to the ordinary CLI freeze; it is not an
  MCP tool or lifecycle capability. Any future proposal for a new public tool or
  ordinary CLI exception requires a separate ADR and the evidence gate.
- Agent hosts gain reliable, model-independent recall and capture at
  documented lifecycle boundaries.
- Trust is never caller-controlled; external content cannot become privileged
  instruction, preference, policy, retraction, or procedure.
- Procedural memory remains a separately gated bounded context projected through
  the existing `FactType::Experience` seam, exposed only after the procedure gate
  is met.
