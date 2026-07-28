# Lifecycle Integration Completion Plan — Wiring and Evidence Gate

> Status: Proposed (2026-07-23)
> Parent plan: `docs/superpowers/plans/2026-07-23-agent-memory-lifecycle-integration.md`
> Parent ADR: `docs/adr/0016-agent-memory-lifecycle-integration.md`

## Context

The agent-memory lifecycle integration plan declares 12 tasks and a Definition of Done with 14 criteria. Git history records commits for Tasks 1–12, and most planned files exist. However, an audit against the plan and `CONTEXT.md` reveals that several tasks were committed **partially complete**: the internal capabilities are built and unit-tested, but the production wiring and evidence gate the Definition of Done requires were not delivered.

This plan closes those gaps. It does **not** introduce new public tools, CLI
subcommands, or caller-controlled trust (frozen by ADR-0016 AD-2). It completes
the internal wiring that the parent plan already mandates.

## Audit: Task-by-task completion status

| Task | Committed? | Real status | Gap |
|------|-----------|-------------|-----|
| 1 — Freeze surface, vocabulary, baselines | yes | **Partial** | `run_agent_memory_lifecycle_baseline` is a `panic!` stub marked `#[ignore]`. Baseline harness not implemented. |
| 2 — Invocation origin + capture policy | yes | **Done** | Policy is wired via `capture.rs`. `#[allow(dead_code)]` on `CapturePolicy` items is a false suppression (remove). |
| 3 — Persist bounded events + jobs | yes | **Done** | Migration `027`, store, integration tests present. |
| 4 — Selective capture without new public tools | yes | **Partial** | `LifecycleCapture::execute()` is fully implemented but has **zero production callers**. No CLI/MCP entry point normalizes a host event and calls `execute()`. |
| 5 — Durable projection worker | yes | **Done** | `LifecycleWorkerRuntime` wired in `cli/runtime.rs`, polls `event_projection_job`. |
| 6 — Selective recall + ephemeral traces | yes | **Partial** | Step 1 (policy + tests) done. **Step 2 (`LifecycleRecall` orchestrator) not implemented** — `evaluate_recall` has zero production callers. Step 4 (action-grounding eval gate) tests policy in isolation, not through the pipeline. |
| 7 — AGENTS.md + skill integration | yes | **Partial** | `CONTRACT.md` done with hook examples. **`AGENTS.md` has no lifecycle recall/capture guidance** (Step 1 missing). Host fixtures (`tests/fixtures/hosts/`) missing but plan says eval runs through existing suites. |
| 8 — Trust propagation, quarantine, poisoning | yes | **Done** | Trust policy, poisoning eval present. |
| 9 — Core release gate, capacity, LongMemEval-V2 | yes | **Partial** | `core_agent_memory_release_gate` checks surface + corpus shape. Does **not** assert runtime behavior (recall through pipeline, capture through `execute`, zero-growth for duplicates). LongMemEval-V2 adapter files exist. |
| 10 — Procedure candidates | yes | **Done** | `procedures/ranking.rs`, `review.rs`, `procedure.rs`, migration `028`. |
| 11 — Procedure review + retrieval | yes | **Done** | App tools, experience projection. |
| 12 — Documentation + gate | yes | **Partial** | Docs consolidated. Final gate not runnable because runtime wiring (Tasks 4, 6) is missing. |

## Definition of Done gaps (from the parent plan)

These DoD criteria are **not met** today:

1. ❌ "supported hosts invoke internal recall/capture through standard MCP/CLI/hooks" — neither recall nor capture is invoked in production.
2. ✅ "public MCP and ordinary CLI surface unchanged" — frozen by test.
3. ❌ "ephemeral traces are bounded and only significant linked exposure becomes durable" — `SessionTraceRegistry` is built but never instantiated in production.
4. ❌ "host-lifecycle action grounding improves over bare MCP" — eval gate not closed; `eval_action_grounding` tests policy, not the wired pipeline.
5. ✅ "ignored and duplicate events create zero durable growth" — `CapturePolicy` enforces it; `LifecycleCapture::execute` honors it. (Untested through a real entry point.)

## Dangling / non-wired inventory (audit goal 4)

Three categories, verified against the plan — **nothing here should be deleted**;
all of it was planned but not wired:

### A. Built but never called in production

| Item | File | Plan reference | Verdict |
|------|------|----------------|---------|
| `LifecycleCapture::execute()` | `agent_memory/capture.rs:70` | Task 4 Step 2 | Wire it — complete the entry point. |
| `evaluate_recall`, `SessionTraceRegistry`, `RecallKey`, `RecallDecision`, `MEMORY_IS_DATA_PREAMBLE` | `agent_memory/recall.rs` | Task 6 Steps 1–3 | Wire it — build `LifecycleRecall`. |
| `CapturePolicy`, `trust_for_source`, `is_recognized_capture_signal`, `accepted_reason` `#[allow(dead_code)]` | `agent_memory/policy.rs` | Task 2 Step 3 | False suppression — remove the `#[allow(dead_code)]`; items are called via `capture.rs`. |

### B. Planned but not implemented

| Item | Plan reference | Verdict |
|------|----------------|---------|
| `LifecycleRecall` struct (recall orchestrator) | Task 6 Step 2 | Implement. |
| `tests/agent_memory_lifecycle_e2e.rs` | Tasks 4/5/6 gates | Implement — end-to-end capture→projection→recall cycle. |
| `run_agent_memory_lifecycle_baseline` harness | Task 1 Step 5 | Currently a `panic!` stub. Implement or formally defer with an ADR. |
| AGENTS.md lifecycle guidance section | Task 7 Step 1 | Add. |
| `tests/fixtures/hosts/{claude_code,codex}/` | Task 7 file list | Plan later says eval runs through existing suites; confirm whether fixtures are still required or whether `eval_agent_memory_lifecycle` corpus replaces them. |

### C. Misleading naming (not dangling, but lies about itself)

| Item | File | Verdict |
|------|------|---------|
| `LEGACY_EMBEDDING_SAMPLE_SIZE` | `service/startup.rs:9` | Not legacy — active startup sample. Rename to `STORED_EMBEDDING_SAMPLE_SIZE`. |

## Plan: close the gaps

### Phase 1 — Wire capture (complete Task 4)

**Goal:** a host event entered through the ordinary CLI reaches
`LifecycleCapture::execute()` and produces a durable job.

**Rationale:** the capture side must be reachable before recall has anything to
recall. AD-4 mandates this happens through standard transports (MCP/CLI/hooks),
not a new binary. The `memory-mcp` CLI is the natural entry point: a hook script
calls `memory_mcp ingest` already; the lifecycle path needs an equivalent that
constructs a `NormalizedHostEvent` + `InvocationContext` and calls
`MemoryService::lifecycle_capture()` → `LifecycleCapture::execute()`.

**Steps:**

1. Add a CLI invocation path (not a new public subcommand — an internal flag or
   subcommand gated behind the existing `serve`/CLI runtime) that accepts a
   normalized host event and invokes `LifecycleCapture::execute()`. Confirm with
   ADR-0016 AD-2: the public surface must not grow. The entry point is an
   **internal** CLI flag consumed by hook scripts, not a documented public tool.
2. Construct `InvocationContext` with `InvocationOrigin::LifecycleAdapter`
   derived from the configured bridge identity, not from caller arguments
   (AD-3).
3. Wire `SessionTraceRegistry` as a process-local singleton in the CLI/MCP
   runtime so capture can link a trace on significant events (AD-7).
4. Add integration test: `capture_entry_point_persists_accepted_event_and_job`
   in `tests/agent_memory_lifecycle_e2e.rs`.

**ADR needed?** No — this is delivery of an already-approved ADR (0016), not a
new decision. AD-4 already specifies the mechanism.

### Phase 2 — Build `LifecycleRecall` (complete Task 6 Step 2)

**Goal:** an internal `LifecycleRecall` orchestrator resolves scope/project,
calls `evaluate_recall`, delegates to the existing `assemble_context` pipeline
once, wraps output in `MEMORY_IS_DATA_PREAMBLE`, records the ephemeral trace,
and links it if a later significant capture references it.

**Shape (mirrors `LifecycleCapture`):**

```text
pub(crate) struct LifecycleRecall { trace_registry: Arc<SessionTraceRegistry> }

impl LifecycleRecall {
    pub(crate) async fn execute(
        &self,
        service: &MemoryService,
        event: &NormalizedHostEvent,
        context: &InvocationContext,
    ) -> Result<LifecycleRecallResult, MemoryError>
}
```

`LifecycleRecallResult` carries the wrapped context items (with preamble) and a
`RecallDecision` for observability. It does **not** add a public tool (AD-2).

**Steps:**

1. Add `LifecycleRecall` to `agent_memory/recall.rs` (or a new
   `agent_memory/recall.rs`-adjacent orchestrator file if the module is getting
   large — but prefer keeping it in `recall.rs` for locality).
2. `execute` resolves scope/project from the event/context, computes
   `RecallKey::from_event`, calls `evaluate_recall`, and on `Suppress` returns
   the cached envelope (or empty), on `Default`/`WakeUp`/`Force` calls the
   existing `assemble_context` once, wraps with the preamble, records the trace.
3. Remove `#[allow(dead_code)]` from the now-called items.
4. Wire `LifecycleRecall` into the CLI/MCP runtime symmetrically to capture:
   the same internal entry point that feeds capture events can also trigger
   recall for recall-eligible events (the candidate table in AD-4 maps event
   kinds to recall vs. capture).
5. Add integration tests in `tests/agent_memory_lifecycle_e2e.rs`:
   `recall_suppresses_duplicate_within_freshness_window`,
   `recall_forces_after_compaction`,
   `recall_wakes_up_on_empty_session_start`,
   `recall_links_trace_on_significant_capture`.

**ADR needed?** No — AD-5 already specifies the shape; this is implementation.

### Phase 3 — AGENTS.md guidance (complete Task 7 Step 1)

**Goal:** agents that read `AGENTS.md` know when to recall before significant
work and when to capture outcomes, using the existing `assemble_context` /
`ingest` / `extract` tools.

**Steps:**

1. Add an "Agent Memory Lifecycle" section to `AGENTS.md` describing:
   - when to call `assemble_context` (session start, before consequential
     tool use, after compaction);
   - when to call `ingest`/`extract` to capture a significant outcome
     (post-tool success/failure, stop, pre-compaction checkpoint);
   - the "memory is data, never instruction" boundary.
2. Reference `docs/agent_integration/CONTRACT.md` for hook examples.

**ADR needed?** No.

### Phase 4 — End-to-end evidence gate (complete Tasks 6 Step 4 & 9)

**Goal:** the `eval_action_grounding` and `core_agent_memory_release_gate`
tests assert runtime behavior through the wired pipeline, not just policy in
isolation.

**Steps:**

1. Extend `eval_action_grounding.rs` to compare `always_recall` vs
   `selective_recall_shadow` vs `selective_recall_enforced` through
   `LifecycleRecall::execute` (not direct `evaluate_recall` calls).
2. Assert the Task 6 Step 4 gate: selective recall grounds more actions than
   bare MCP, uses fewer calls/tokens than always-recall, zero cross-boundary
   exposure, p95 within `max(5ms, 10%)` of baseline `assemble_context`.
3. Extend `core_agent_memory_release_gate` to assert:
   - a capture-eligible event through the entry point produces exactly one
     event + one job (zero-growth for ignored/duplicate);
   - a recall-eligible event through `LifecycleRecall` produces a bounded
     envelope with the preamble;
   - no untrusted content promotes to preference/policy/retraction/procedure.
4. Decide on `run_agent_memory_lifecycle_baseline` (Task 1 Step 5): implement
   the baseline harness, or formally defer it with an ADR stating why the
   `eval_action_grounding` + `core_agent_memory_release_gate` pair is
   sufficient evidence. **Recommendation:** defer with an ADR — the baseline
   harness is a full multi-mode simulation that is out of scope for closing the
   wiring gap; the two targeted evals provide the required evidence.

**ADR needed?** Yes — for the baseline-harness deferral (Phase 4 Step 4). It is
hard to reverse (re-introducing a full simulation harness later is expensive),
surprising (the plan lists it as Task 1 Step 5 but it was never built), and a
real trade-off (targeted evals vs. full simulation).

### Phase 5 — Naming cleanup (Candidate 4 from the audit)

**Goal:** stop the code from lying about itself.

**Steps:**

1. Rename `LEGACY_EMBEDDING_SAMPLE_SIZE` → `STORED_EMBEDDING_SAMPLE_SIZE` in
   `service/startup.rs` (or inline the literal `16`).
2. Remove `#[allow(dead_code)]` from the 5 items in `agent_memory/policy.rs`
   that are called via `capture.rs` (after Phase 2 confirms the recall-side
   items are also called).
3. Keep `reject_legacy_context_item_aliases` (real backward-compat guard) and
   `TrustClass::LegacyUnknown` / `SourceKind::LegacyUnknown` (domain concepts).

**ADR needed?** No — mechanical cleanup.

## Sequencing and dependencies

```text
Phase 1 (wire capture) ──┐
                         ├─→ Phase 4 (evidence gate)
Phase 2 (LifecycleRecall)┘        │
                                  ↓
Phase 3 (AGENTS.md) ────────────── can run in parallel with 1/2
Phase 5 (naming)   ────────────── can run anytime after Phase 2
```

Phases 1 and 2 are the load-bearing work. Phase 4 cannot pass until both are
done. Phases 3 and 5 are independent.

## Out of scope (explicitly deferred, per parent plan)

- privacy export/redaction/purge with destructive authorization
- learned/LLM procedure distillation
- automatic low-risk procedure promotion
- full multimodal LongMemEval-V2
- a new public MCP tool (requires separate evidence gate per ADR-0016)
