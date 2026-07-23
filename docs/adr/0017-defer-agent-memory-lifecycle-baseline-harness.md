# Defer the Agent-Memory Lifecycle Baseline Simulation Harness

Status: Accepted (2026-07-23)

## Context

Task 1 Step 5 of the agent-memory lifecycle integration plan
(`docs/superpowers/plans/2026-07-23-agent-memory-lifecycle-integration.md`)
calls for a `run_agent_memory_lifecycle_baseline` harness that simulates
multiple modes (`no_memory`, `bare_mcp`, `instructions_only`,
`manual_existing_tools`) across task families and reports per-mode metrics
(recalls, captures, grounded actions, latency, rows/bytes per 1,000 events).

The test exists today as a `panic!` stub marked `#[ignore]`. Building the full
simulation is a large effort that is orthogonal to closing the wiring gap
(production callers for `LifecycleCapture::execute` and `LifecycleRecall`),
which is the actual blocker for the Definition of Done.

## Decision

Defer the full baseline simulation harness. Close the lifecycle evidence gate
with two targeted evaluations instead:

1. `eval_action_grounding` — compares `always_recall` vs
   `selective_recall_shadow` vs `selective_recall_enforced` through the wired
   `LifecycleRecall` pipeline (not direct `evaluate_recall` calls).
2. `core_agent_memory_release_gate` — asserts runtime invariants through the
   wired entry points: capture produces one event + one job (zero-growth for
   ignored/duplicate), recall produces a bounded envelope with the "memory is
   data" preamble, no untrusted content promotes to privileged instruction.

## Rationale

The baseline harness measures *relative improvement* across simulated agent
modes. The two targeted evals assert the *absolute invariants* the Definition of
Done requires (zero-growth, bounded exposure, no trust elevation, action
grounding beats bare MCP). The targeted pair is sufficient evidence to release
the lifecycle integration; the full simulation is a richer signal but not a
release blocker.

## Consequences

- The lifecycle integration can be declared complete once the wiring
  (`LifecycleCapture` entry point, `LifecycleRecall` orchestrator) and the two
  targeted evals land.
- A future ADR is required to re-open the baseline harness if richer
  comparative metrics (token cost, p95 across modes, per-task-family grounding
  rates) become a product requirement.
- The `#[ignore]` stub in `eval_agent_memory_lifecycle.rs` is replaced with a
  reference to this ADR rather than left as an unimplemented `panic!`.
