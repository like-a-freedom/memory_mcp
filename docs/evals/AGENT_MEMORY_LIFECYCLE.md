# Agent Memory Lifecycle — Evaluation

> This document records the before-state baseline and release-gate results for
> agent-memory lifecycle integration. It is the single place where quality,
> latency, capacity, and security claims cite local results.

## Corpus

Version: `agent-memory-lifecycle/v1`

Location: `crates/memory-mcp/tests/fixtures/agent_memory_lifecycle_cases.json`

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

## Verified baseline

The lifecycle evaluation is now part of the `eval-harness` profile-driven
system. The `LifecycleReleaseSuite` implements the ADR-0017 release gate
through wired `LifecycleCapture` and `LifecycleRecall` entry points.

**Baseline:** `fa57d49b` (master) on 2026-07-28 — recorded at the time of the original gate run; master has since advanced.

The lifecycle suite exercises:
- Action grounding through all three modes (always_recall, selective_shadow, selective_enforced)
- Persisted capacity measurements (rows, bytes, zero growth for ignored/duplicate)
- Poisoning replay from capture through attempted action
- Trust non-elevation and boundary invariants
- Bounded envelope with `MEMORY_IS_DATA_PREAMBLE`

### Surface freeze

```bash
cargo test -p memory-mcp --test agent_memory_lifecycle_release_gate public_surface_snapshot -- --exact
```

Asserts exactly eight MCP tool names and the ordinary CLI command snapshot,
and the absence of `prepare_task`, `record_event`, `hook`, `checkpoint`,
`rollback`, and procedure CRUD.

### Fixture coverage

```bash
cargo test -p memory-mcp --test agent_memory_lifecycle_release_gate lifecycle_fixture_covers_core_risks -- --exact
```

Asserts every required risk family is represented.

### Evaluation modes

The lifecycle evaluation exercises three agent modes:

```text
always_recall        — forced recall on every eligible boundary
selective_shadow     — selective decision recorded, always-recall envelope used
selective_enforced   — selective decision applied
```

The `ActionGroundingSuite` records recall calls, suppressions, context items,
action outcomes, latency, and evidence IDs for every case. Action grounding
is determined from an observed consequential action outcome, never by a recall
trace alone.

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

Procedural memory is separately gated and does not affect the lifecycle
release gate. See `docs/evals/PROCEDURAL_MEMORY.md` for the procedure gate
requirements. Absence of procedural memory is the correct result until its
gate is met.
