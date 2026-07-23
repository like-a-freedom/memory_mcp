# Agent Memory Lifecycle — Evaluation

> This document records the before-state baseline and release-gate results for
> agent-memory lifecycle integration. It is the single place where quality,
> latency, capacity, and security claims cite local results.

## Corpus

Version: `agent-memory-lifecycle/v1`

Location: `tests/fixtures/evals/agent_memory_lifecycle_cases.json`

The corpus covers at least three coding-task families and every release-gate
risk family:

- preference, constraint, decision, commitment, correction;
- verified outcome, failure diagnosis, checkpoint, task outcome, reusable lesson;
- read-only tool noise and repeated status polling;
- duplicate delivery and restart;
- cross-project, cross-scope, and policy near matches;
- stale and contradicted memory;
- external instruction injection and false-success precedent;
- outage and compaction/resume;
- capacity-budget exhaustion.

Every release-gate expectation is human reviewed.

## Verified baseline (Task 1)

**Repository baseline:** `3d7cef63` (tag v1.7.0, master) on 2026-07-23.

This is the actual `master` HEAD of the worktree; it does not match the
plan's referenced baseline `86d2bb96` because the plan was written against a
fork that was 41 commits ahead and contained migrations 027/028, claim
reconciliation, `CONTEXT.md`, and ADRs 0002–0015. Those artifacts are not
present in this worktree. The lifecycle integration is implemented against the
actual `master` state; later tasks that reference migrations 027/028 will
renumber or adapt as needed.

### Surface freeze

```bash
cargo test --test eval_agent_memory_lifecycle public_surface_snapshot -- --exact
```

Asserts exactly eight MCP tool names and the ordinary CLI command snapshot,
and the absence of `prepare_task`, `record_event`, `hook`, `checkpoint`,
`rollback`, and procedure CRUD.

### Fixture coverage

```bash
cargo test --test eval_agent_memory_lifecycle lifecycle_fixture_covers_core_risks -- --exact
```

Asserts every required risk family is represented.

### Baseline modes (Task 1 Step 5)

Modes:

```text
no_memory
bare_mcp
instructions_only
manual_existing_tools
```

The baseline harness (`run_agent_memory_lifecycle_baseline`, `#[ignore]`)
reports per task family:

- eligible and performed recalls;
- eligible and performed captures;
- correct, unsafe, and duplicate captures;
- grounded actions;
- stale influence and leakage;
- MCP tool-selection accuracy;
- tool calls per intent;
- p50/p95 latency;
- new rows and bytes per 1,000 simulated host events.

Improvement thresholds are **not** asserted in the baseline task.

## Vocabulary

```text
lifecycle event
lifecycle bridge
selective recall
selective capture
invocation origin
exposure trace
action grounding
projection job
procedure candidate
procedure version
```

"Do not use 'discipline' as a domain noun or public feature name."

## Release gate (cumulative)

The core release gate (Task 9) fails on any:

- MCP or ordinary CLI surface expansion;
- missed configured lifecycle event without degraded telemetry;
- duplicate raw episode/job;
- growth from ignored or duplicate events;
- cross-scope/project/policy exposure;
- trust elevation or external self-promotion;
- contradiction-triggered source-fact mutation;
- missing raw evidence after projection failure;
- hidden dead letter;
- persisted unlinked exposure trace;
- unsupported host event represented as enforced.

## Procedure gate

Tasks 10–11 do not start until:

- the core gate passes;
- at least three independent task families have successful and failed outcomes;
- one repeated lesson candidate has at least three independent trusted outcomes;
- the operator-review workflow has an owner and retention policy;
- the projected 365-day storage remains within the configured project budget.

If the gate is not met, stop after Task 9. Absence of procedural memory is the
correct result.
