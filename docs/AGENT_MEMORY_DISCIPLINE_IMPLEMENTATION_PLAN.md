# Agent Memory Discipline — Implementation Plan

**Status:** Proposed (2026-07-18), **revised same day after codebase verification**
**Scope:** Execution-level refinement of section 13 of `docs/AGENT_MEMORY_DISCIPLINE_PLAN.md`. The architecture plan defines *what* and *why*; this document defines *in what order, behind what blockers, with what prerequisites, and how sliced*.
**Relationship to §13:** Strengthens it by (a) inserting a tracer-bullet slice that proves the architecture in week 1–2, (b) splitting WP0 and WP3 which are overloaded, (c) making the claim-reconciliation dependency honest against the actual code, (d) decomposing to ticket-level with blocking edges.

> **v2 revision note.** The first draft of this plan was written against assumptions about the codebase. Verification against `master` at `3648bda5` (2026-07-18) corrected several:
> - rmcp 2.2.0 **does** support `ToolAnnotations` via the `annotations()` trait hook and builder — the "rmcp annotation spike" was unnecessary and is removed.
> - The CLI **already** has one-shot subcommands for all six existing tools under `src/cli/commands/` — adding `prepare_task`/`record_event` subcommands is a trivial extension of an established pattern, not new infrastructure.
> - `ContradictionWarning` **already** carries `conflicting_fact_id` — part of the supersession work is done.
> - Claim reconciliation is **partially implemented** (full `ClaimStore` trait with 11+ methods, SurrealDB backing, structural parser, claim service wiring), but `after_fact_persisted()` is still broken in specific places and 0/51 tasks in the parallel `claim-reconciliation-completion` plan are checked off. This is not "not started"; it is "infrastructure exists, orchestration pending, parallel active plan."
> - `detect_contradiction_warnings` does **not** have confidence tiers — that part of P1.4 remains real new work.
> The sections below reflect these verified facts.

---

## 1. Verified facts from the codebase (2026-07-18, master `3648bda5`)

These are established by reading code, not by spikes. They reshape the plan.

| Fact | Source | Consequence |
|------|--------|-------------|
| rmcp 2.2.0 exposes `ToolAnnotations` via `#[tool]`-trait `annotations()` hook + builder `.annotation()` | `rmcp-2.2.0/src/model/tool.rs:113`, `rmcp-2.2.0/src/handler/server/router/tool.rs:320`, `tool_traits.rs:77` | Tool annotations are implementable directly. No spike needed. |
| CLI has one-shot subcommands for all six existing tools | `src/cli/commands/{assemble_context,explain,extract,ingest,invalidate,resolve}.rs` | A capability subcommand for `prepare_task`/`record_event` is a trivial extension. Tracer-bullet's "CLI hook" half is cheaper than first draft assumed. |
| `view_mode="wake_up"` exists and routes through `build_wake_up_view` | `src/service/context.rs:33,254-266` | The tracer-bullet's `memory_prepare_task` composes an existing, working view. |
| `ContradictionWarning` already carries `conflicting_fact_id`, `new_fact_id`, `entity_ids`, `reason` | `src/models/request.rs:212-221` | Supersession Option A is partially done; only `confidence` tier and suggested-invalidate-args remain. |
| `detect_contradiction_warnings` returns a flat `Vec<ContradictionWarning>` with no confidence tiers | `src/service/episode/fact_extraction.rs:245-310` | P1.4 tiered-confidence work is real and not yet started. |
| `ClaimService::after_fact_persisted` hard-codes `EpisodeId::from("ep:inline")` (4x) and `policy_tags: &[]` (3x); marks projection job `Completed` immediately; creates no reconcile jobs | `src/service/claims/project.rs:98-208` | The exact defects the parallel `claim-reconciliation-completion` plan targets. `memory_record_event` cannot ship correctly until this is fixed or the two efforts are explicitly merged. |
| `ClaimStore` trait has full durable-job surface: `lease_next_job`, `persist_projection`, `select_candidates_page`, `commit_relation`, `select_facts_for_backfill`, `select_source_evidence`, `count_active_relations` | `src/storage/claims.rs`, `src/service/claims/project.rs:242-310` | The durable-job *infrastructure* for WP5 exists. What's missing is the worker orchestration and the richer `ClaimJobState`. |
| `ClaimJobState` is `Pending/Leased/Running/Completed/Failed` — no `partial/retry_wait/dead_letter/cancelled` | `src/models/claim.rs:542-548` | WP5 needs enum extension, not a from-scratch job system. S-M work, not M-L. |
| `MEMORY_CLAIM_ROLLOUT_STAGE` defaults to `Evidence`; `Lifecycle` returns an explicit unsupported-stage error | `src/config/claims.rs:21-52`; claim-completion plan hard constraint | Full auto-correction/supersession is gated on a separate safety review. The plan must function without it (and does, via `insufficient_support`). |
| `claim-reconciliation-completion` plan has 51 tasks, **0 checked off**; recent commits (`a88c712e`…`3648bda5`) landed the infrastructure but not the orchestration/fixes | git log + plan checkbox count | This is active parallel work touching the exact files WP2/WP5 need. Coordination is a hard prerequisite, not a spike. |
| `memory_prepare_task` / `memory_record_event` / any bridge or hook code: **none exists** | grep across `src/` | Greenfield for the capability facade and bridge. |
| memory_mcp is at v1.7.0 with the public tool surface in production use | `Cargo.toml:3` | Description/annotation changes are breaking-ish for existing clients; needs a versioning/migration note. |

---

## 2. Analysis of section 13 — what holds, what needs sharpening

### 2.1 What is strong and must be preserved

- **WP0 first.** TDD discipline (AD-11) and the baseline-before-default-on rollout gate (§12.4) require the discipline contract and a labeled corpus before any enforcement ships.
- **Strict write-side discipline.** AD-3 (foreground durability, background derivation), AD-4 (source facts ≠ claims), AD-5 (origin/trust), AD-6 (data boundary), AD-9 (visible degradation) are non-negotiable.
- **Rollout gates.** Observe-only → shadow → opt-in → default-on (§14) is the right risk curve.
- **Hard deterministic gates** (§12.4) as release blockers.

### 2.2 Weaknesses found against the actual repository state

**W1 — The claim-reconciliation dependency is real but mis-framed by §13.**

§13 WP5 says *"Reuse or generalize durable-job mechanics from claim reconciliation."* Verified reality: the durable-job **infrastructure** exists (`ClaimStore` trait is deep; SurrealDB queries implemented), but the **orchestration** does not (no worker; `after_fact_persisted` broken; 0/51 tasks in the parallel completion plan done; `Lifecycle` stage explicitly disabled pending safety review). WP5 is therefore not "reuse in 1–2 weeks" — it depends on (a) the parallel `claim-reconciliation-completion` plan landing its worker + fixes, (b) extending `ClaimJobState`, (c) the safety review authorizing `Lifecycle`. **But the infrastructure depth means WP5's *generalization* work is smaller than first implied** — enum extension + worker wiring, not a from-scratch system.

**W2 — `memory_prepare_task` cannot wait for full claim enrichment, and shouldn't.**

§7.2 returns "active contradiction/temporal-ambiguity summaries." Given W1, claim relations are not production-ready. The tool's `insufficient_support` / `degraded_components` fields exist for exactly this. **The first `memory_prepare_task` ships with claim enrichment explicitly degraded.** This is the contract working as designed, not a hack.

**W3 — WP0 is overloaded and blocks the critical path asymmetrically.**

WP0 bundles six items (contract, current-behavior recording, ADR-conflict correction, oracle corpus, baseline runs, host-contract fixtures) with different shapes and different downstream blockers. The canonical contract blocks WP1/WP2/WP4; the oracle corpus blocks only WP7/release gates. Bundling them makes WP1 wait on the slowest sub-item (manual corpus labeling). Split so the contract — cheap and most-blocking — ships first.

**W4 — No tracer-bullet slice. The plan is strictly layered.**

§13 is a waterfall; the first end-to-end observable architecture is after WP4 (week 8+). The thinnest proving slice is: **minimal `memory_prepare_task` (read-only, no receipts, degraded claims) + minimal Claude Code `SessionStart` hook + observe-only classification.** Given the verified facts (CLI subcommand pattern exists, `wake_up` view exists), this slice is cheaper than first draft assumed — roughly 1 week, not 2.

**W5 — `memory_prepare_task` and `memory_record_event` have asymmetric risk and must be split.**

The read tool composes existing retrieval and is low-risk. The write tool enforces policy, requires idempotency, atomically creates a durable job, and touches `after_fact_persisted()` — the exact seam the parallel claim-completion plan is rewriting (W1). Bundling them in WP3 means the low-risk read tool waits for the high-risk write tool. Split: read tool first (tracer-bullet half), write tool after capture foundation and the claim-coordination decision.

**W6 — The Claude Code hook contract is the only genuine remaining spike.**

rmcp annotation support is verified (fact table above). Claim state is established by reading code (not a spike — a coordination decision, see §4). The one assumption that still needs live verification is **which hook events Claude Code actually exposes** (SessionStart/UserPromptSubmit/PreToolUse/PostToolUse/PreCompact/Stop/SessionEnd) and their payload shapes. §8.2/§8.3 list them "as supported by current hook contract" without verification. If, say, PreCompact or PostToolUse are absent, the Claude adapter scope changes materially. This is the only true spike.

**W7 — No "definition of ready" per WP.** Each WP lists acceptance but not entry prerequisites.

**W8 — No sizing.** Sequencing is dependency-only; sprint planning and interleaving with claim-completion is impossible without rough sizes.

---

## 3. Revised execution strategy

Three changes to §13's sequencing:

1. **Run the one remaining spike (Claude Code hook contract) as Sprint 0a.** One day; may reshape WP4 and the tracer-bullet's hook half.
2. **Make the claim-completion coordination decision up front (§4), not as a spike.** Either sequence behind it, or explicitly merge tasks. This governs whether WP2/WP5 start now or wait.
3. **Insert a tracer-bullet after the contract.** Prove host→capability→injection end-to-end (week 1–2) before building the write side, background, or full enrichment. Split WP0 (contract vs corpus), WP3 (read vs write), WP5 (generalize vs full-lifecycle).

Everything else from §13 is preserved.

---

## 4. Claim-reconciliation coordination decision (prerequisite, not a spike)

The parallel `docs/superpowers/plans/2026-07-18-claim-reconciliation-completion.md` is active work touching the exact files WP2 (`memory_record_event`) and WP5 (background projection) need: `src/service/claims/project.rs`, `src/models/claim.rs`, `src/storage/claims.rs`, `src/service/episode/fact_extraction.rs`. 0/51 of its tasks are checked off, but the infrastructure (ClaimStore trait, SurrealDB queries, structural parser) is already on master.

**Decision required before WP2/WP5 start — pick one:**

- **Option A (recommended): sequence behind claim-completion.** Let `claim-reconciliation-completion` land its worker + `after_fact_persisted` fixes first. WP2/WP5 then build on a correct foundation. Cost: WP2 starts later. Benefit: no concurrent edits to the same seam; no rework.
- **Option B: explicitly merge overlapping tasks.** Pull the `after_fact_persisted` fix and worker wiring into T04 (capture foundation) as a single coordinated effort, and mark the corresponding claim-completion tasks as done-by-this-plan. Cost: the two plans must be kept in sync. Benefit: T04 starts now.
- **Option C: develop WP2/WP5 against a stable interface and let claim-completion refactor behind it.** Only viable if `ClaimStore` trait is stable enough to act as the seam. Verify before choosing.

**This is a prerequisite decision, not investigation work.** Whichever option is chosen, record it in `docs/agent_integration/claim_coordination.md` before WP2 starts. The tracer-bullet and the entire read side do not depend on this decision (they degrade claims by design).

---

## 5. The one remaining spike (Sprint 0a)

### S1 — Claude Code hook contract verification

**Question:** For the installed Claude Code version, which of {SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, PreCompact, Stop, SessionEnd} are actually exposed, and what are their payload shapes?

**Method:** Install current Claude Code; write a hook that logs every event received with its payload; drive a test session exercising each boundary.

**Output:** Versioned fixture at `docs/agent_integration/spikes/claude_code_hooks_observed.md`. Update §8.2/§8.3 if any assumed event is absent.

**Blocks:** the tracer-bullet's hook half, the full Claude adapter (T15b).

**Timebox:** 1 day.

---

## 6. Ticket breakdown

Tickets are independently deliverable, declare blocking edges, and carry acceptance criteria. Naming: `T01…` — numbers are grouped by phase (the grouping reflects the WP the ticket came from), not strictly contiguous across phases. Effort: S (~1–2 days), M (~3–5 days), L (~1–2 weeks), XL (gated/long-horizon). Estimates reflect the verified codebase facts.

### Phase 0 — Contract and the one spike

#### T01 — Spike: Claude Code hook contract (S)
**Prerequisite:** none.
**Blocks:** T07 (tracer-bullet hook), T15b (Claude adapter).
**Acceptance:** observed-contract fixture committed; §8.2/§8.3 corrected if needed.

#### T02 — Canonical discipline contract (M)
**Prerequisite:** none.
**Blocks:** T03, T04, T05, T06, T07, T08, T15a, T15b, T15c.
**Work:** `docs/agent_integration/discipline_contract.md` — triggers, event kinds, source/trust classes, degraded behavior, host mappings. The single source other tickets derive from (per DRY risk #7 in the architecture plan).
**Acceptance:** every trigger has eligible/non-eligible example; every event kind has source, trust, scope, redaction rules; claim/retraction terminology matches ADR-0002…0015.

#### T03 — Minimal public-contract corrections (M)
**Prerequisite:** T02.
**Blocks:** nothing on the critical path; must land before default-on.
**Work:** the §13 WP1 corrections independent of new tooling — rewrite `invalidate` description (source-fact retraction only, per AD-4); mark `resolve` as mutating; document `ingest` idempotency tuple precisely; strengthen `SERVER_INSTRUCTIONS`. **Annotations are implementable directly (verified fact) — include them here**, not as a deferred sub-ticket. Add a read-only discipline/integration resource for discovery (not for automatic injection).
**Acceptance:** no public text tells an agent to invalidate a source fact for routine supersession; annotations verified to reach the published schema via a conformance test; existing clients remain schema-compatible (additive only).
**Versioning note:** memory_mcp is v1.7.0 in production use. Description/annotation changes are non-breaking at the wire level but may change agent behavior. Note this in the changelog; consider a minor version bump.

### Phase 1 — Tracer-bullet (prove the architecture end-to-end)

#### T07 — Tracer-bullet: minimal `memory_prepare_task` + Claude Code `SessionStart` hook + observe-only (S–M)
**Prerequisite:** T01, T02. **Does not require:** T04, T05, T06, T08, T14, T16, or the claim-coordination decision (§4).
**Why this estimate dropped from M to S–M:** the CLI subcommand pattern is established (`src/cli/commands/*.rs`), `view_mode="wake_up"` exists and works, and there are no receipts, writes, or background work in this slice.
**Work:**
- Minimal `memory_prepare_task`: composes existing `assemble_context` + `view_mode="wake_up"`; returns current facts + recent state; **claim enrichment returns `insufficient_support=true`** (degraded by design); **no context receipts yet** (deferred to T08); idempotency via `request_id`.
- Add a `prepare_task` CLI subcommand following the established pattern in `src/cli/commands/`.
- Minimal Claude Code `SessionStart` hook: shells out to that subcommand; injects the result as session context. Only `SessionStart`.
- Observe-only classification: a **standalone logging path** (not in the bridge core — that comes in T15a) that classifies which lifecycle events *would* trigger a call, without making the call. This validates the trigger policy (T02) against reality before any enforcement.
**Acceptance:**
- A real Claude Code session fires `SessionStart`; the hook calls `memory_prepare_task`; the returned context appears in the agent's view. Demo recorded.
- Observe-only log shows trigger decisions with reasons for a 30-min real session.
- `memory_prepare_task` returns `insufficient_support=true` for claim fields (honest degradation).
- No receipts, no writes, no background work — read-only and safe.
**Why this ticket matters:** if the host→capability→injection flow has a hidden problem (hook payload shape, injection format ignored by the model, capability surface confusing), it surfaces in week 1, not week 8.

### Phase 2 — Capability tools (split from WP3)

#### T05 — `memory_prepare_task` full version (M)
**Prerequisite:** T02, T07 (tracer-bullet validated the shape).
**Blocks:** T08 (receipts build on the tool), T15a (adapter calls it).
**Work:** add provenance handles, pagination, `risk_level` handling, partial/degraded responses and `guidance`. Claim enrichment stays degraded until T16b lands — surface via `degraded_components`.
**Acceptance:** matches §7.2 contract except claim-relation fields, which are explicitly `insufficient_support`; MCP + CLI contract tests pass.

#### T08 — Context receipts for `memory_prepare_task` (AD-7) (M)
**Prerequisite:** T05.
**Blocks:** T13 (audit), T15b (adapter propagates receipts).
**Work:** append-only receipt store; `context_receipt_id` returned and persisted; receipt covers query, scope, policy version, retrieved fact/claim IDs, ranking mode, timestamps, latency. **AD-7 annotation consequence:** receipts are an audit write, so `memory_prepare_task` is `readOnlyHint=false` (already set correctly in T03 — do not lie).
**Acceptance:** receipt stable and replayable; later events reference it; receipt write atomic with the read or explicitly best-effort with visible failure.

#### T06 — `memory_record_event` (L)
**Prerequisite:** T02, T04 (capture foundation), §4 claim-coordination decision.
**Blocks:** T15b (adapter calls it for PostToolUse/Stop), T16b (background consumes the durable job it creates).
**Work:** the write-side capability tool per §7.3. Idempotency via `event_id`; atomic raw-episode + durable-job creation; capture-status (`stored`/`duplicate`/`quarantined`/`rejected`) and projection-status returns.
**Acceptance:** retry returns original result; same `event_id` with conflicting identity fails loudly; policy fields survive foreground→background; untrusted content quarantined per AD-5.

### Phase 3 — Capture foundation and background (WP2, WP5)

#### T04 — Source-aware event capture foundation (L)
**Prerequisite:** T02, §4 claim-coordination decision (this touches `after_fact_persisted` and the claim job seam).
**Blocks:** T06.
**Work:** per §13 WP2. Event identity/kind/source/actor/trust/session-task/policy fields; validation and precedence; create-or-validate idempotency; write-policy classification (accept/quarantine/reject/redact); atomic raw-episode + durable-job persistence; one-shot CLI subcommand (pattern exists — extension).
**Acceptance:** per §13 WP2 acceptance.
**Coordination:** the §4 decision governs whether the `after_fact_persisted` fix lands here or in claim-completion.

#### T16a — Extend `ClaimJobState` and wire the worker (M)
**Prerequisite:** §4 claim-coordination decision.
**Blocks:** T16b.
**Work:** extend `ClaimJobState` with `partial / retry_wait / dead_letter / cancelled`; add expiring leases, bounded retries with reason codes, dead-letter inspection/replay, cancellation at safe boundaries, restart recovery. Wire the worker that `claim-reconciliation-completion` describes (its tasks target this; 0/51 currently done). **The `ClaimStore` infrastructure already exists** (verified fact) — this is enum extension + orchestration, not a new system.
**Acceptance:** failed stage visible in dead-letter; process restart recovers leased work; per-scope/project isolation holds.
**Note:** this may land as part of `claim-reconciliation-completion` rather than as separate work, per the §4 decision.

#### T16b — Full projection and consolidation lifecycle (XL, gated)
**Prerequisite:** T16a, T06, **`claim-reconciliation-completion` landed**, **`MEMORY_CLAIM_ROLLOUT_STAGE=lifecycle` authorized via safety review** (hard constraint in the claim-completion plan).
**Work:** per §13 WP5 — move extraction/embedding/linking/claim-projection behind the durable job; incremental recomputation via fingerprints; bounded consolidation; procedural promotion gates; surface queue lag in `memory_prepare_task`.
**Acceptance:** per §13 WP5 acceptance.
**Why XL and gated:** the `lifecycle`-stage authorization is explicitly deferred to a separate safety review. The rest of the plan functions without this (T05/T08 degrade gracefully).

### Phase 4 — Lifecycle bridge (WP4, split by client)

#### T15a — Bridge core + CLI transport (M)
**Prerequisite:** T02, T05.
**Blocks:** T15b, T15c, T15d.
**Work:** host-neutral bridge per §8.1 — event normalization, project/scope/session resolution, trigger classification, risk policy, stable event IDs, retries, degraded-mode behavior, receipt propagation, redaction, telemetry. CLI transport reuses the established subcommand pattern. **Absorb the tracer-bullet's observe-only classifier here** (T07's standalone version becomes the bridge's classifier).
**Acceptance:** bridge core has conformance tests independent of any specific host; CLI transport round-trips a `memory_prepare_task` call; the T07 observe-only path is now bridge-managed.

#### T15b — Claude Code adapter (M)
**Prerequisite:** T15a, T01 (hook contract verified), T06, T08.
**Work:** full Claude Code plugin per §8.3 — SessionStart, UserPromptSubmit, PreToolUse (configured tools only), PostToolUse, PreCompact, Stop/SessionEnd with once-only finalization.
**Acceptance:** per §13 WP4 acceptance for Claude Code; duplicate session-end signals do not double-capture.

#### T15c — Codex adapter (M)
**Prerequisite:** T15a.
**Work:** per §8.3 Codex mapping — SessionStart, PreToolUse, PostToolUse, PermissionRequest; model-instructions + compact-prompt fallback for unsupported boundaries (answer-only turns).
**Acceptance:** supported events deterministic; unsupported coverage reported honestly as prompt-assisted.

#### T15d — Custom-harness middleware reference (S–M)
**Prerequisite:** T15a.
**Work:** reference middleware (pseudocode + minimal runnable example) for integrators writing their own agent loop. The custom-harness case from the architecture plan's adoption tiers.
**Acceptance:** copy-paste example compiles and runs against `memory_mcp`; trigger table matches T02.

### Phase 5 — Security, audit, eval (WP6, WP7, split)

#### T09 — Tiered contradiction confidence + supersession Option A completion (M)
**Prerequisite:** T02.
**Work:** add `confidence: "high"|"potential"` to `ContradictionWarning` (the field doesn't exist today — verified); default-to-surface behavior per the architecture plan; complete supersession Option A by adding suggested `invalidate` args to the warning (`conflicting_fact_id` already exists — verified — so this is additive); dynamic `guidance` in `extract` conditional on warning tier.
**Acceptance:** `potential`-tier warnings never block; `high`-tier warnings surface to user or carry ready-to-use `invalidate` args; false-positive cost contained.

#### T13 — Audit views and exposure correlation (WP6 subset) (M)
**Prerequisite:** T08.
**Work:** surface context-receipt → action correlation in `explain` and an operator view; distinguish exposed / agent-claimed-used / action-grounded evidence per §11.3.
**Acceptance:** operator can answer "was memory exposed before this action?" for any action; view does not claim causal proof where only exposure is known.

#### T14 — Security controls: trust inheritance, quarantine, high-risk policy, poisoning suites (L)
**Prerequisite:** T04, T06.
**Work:** per §13 WP6 — trust inheritance across facts/claims/summaries/lessons; quarantine review/release workflow; high-risk retrieval/action policy; poisoning and cross-scope adversarial test suites.
**Acceptance:** per §13 WP6 acceptance and §12.4 hard gates (no cross-scope leakage; external_untrusted never auto-promoted; contradictions don't retract source facts).

#### T17 — Discipline eval harness and corpus (WP0b + WP7) (L, ongoing)
**Prerequisite:** T02 (defines oracle labels).
**Work:** oracle-labeled corpus from real/replayable coding-agent sessions (§12.3); metrics in §11.2; experimental modes in §12.1; reproduction of selected external benchmarks (§12.2); release-evidence report.
**Note:** long-pole non-blocking item from original WP0. Runs parallel from Phase 1; blocks only the default-on rollout gate.
**Acceptance:** §12.4 hard gates pass; release report exists with client versions, corpus version, confusion matrices, latency/token cost.

### Phase 6 — Rollout gates

#### T18 — Observe-only and shadow rollout (S)
**Prerequisite:** T15a, T17 (corpus for measurement).
**Work:** enable observe-only and shadow modes in production for selected users/projects; collect metrics.
**Acceptance:** no side effects in observe-only; shadow-mode latency/token cost measured and within budget.

#### T19 — Opt-in enforced and default-on per adapter (M, gated)
**Prerequisite:** T18, T14 (security), T17 (release gates pass), per-adapter contract tests.
**Work:** enable enforcement per adapter only after that adapter passes its contract + security gates.
**Acceptance:** §13 WP7 acceptance; §16 definition of done.

---

## 7. Dependency graph and critical path

```
Sprint 0a:
  T01 (Claude hook spike)        T02 (discipline contract)
       |                                |
       |                                +----> T03 (minimal corrections + annotations)
       |                                |
       |                       ========= Tracer-bullet (Phase 1) =========
       +----> T07 (minimal prepare_task + SessionStart hook + observe-only)
                    |                         (T17 eval corpus starts here, long pole)
                    |
                    T05 (prepare_task full) ── T08 (receipts) ── T13 (audit)
                    |
                    T15a (bridge core) ──┬── T15b (Claude, needs T01/T06/T08)
                                        ├── T15c (Codex)
                                        └── T15d (custom ref)
                    |
       §4 claim-coordination decision (prerequisite for write side)
                    |
                    T04 (capture foundation) ── T06 (record_event)
                                                 |
                    T09 (tiered contradictions — independent of write side)
                                                 |
                    T16a (extend ClaimJobState + worker)
                                                 |
                    T16b (full lifecycle, GATED by claim completion + safety review)
                                                 |
                    T14 (security)
                    |
       ========= Rollout (Phase 6) =========
                    T18 (observe/shadow) ── T19 (opt-in / default-on, gated)
```

**Critical path to a testable, deployed, receipt-bearing read capability:** T02 → T07 → T05 → T08 → T15a → T15b. With the verified facts (CLI pattern exists, `wake_up` exists, rmcp annotations supported), this is realistically **~2–3 weeks** to a deployed read capability on Claude Code — not the 3–4 the first draft claimed. **The read side ships and proves value before the write side, background, or full claims are done.**

**Critical path to default-on write discipline:** §4 decision → T04 → T06 → T16a → T16b (gated) → T14 → T19. The `lifecycle`-stage safety review gate on T16b is the single largest schedule risk.

**Parallel tracks (no blocking edges between them):**
- Eval corpus (T17) — from Phase 1 onward; blocks only rollout gates.
- Custom-harness reference (T15d) — once T15a lands.
- Minimal corrections + annotations (T03) — once T02 lands; independent of capability tools.
- Tiered contradictions (T09) — once T02 lands; independent of the write side (operates on the existing `detect_contradiction_warnings`).

---

## 8. What this plan does NOT do (YAGNI, inherited from §15)

All non-goals from §15 of the architecture plan carry over. Additionally:

- **Does not commit to the `Lifecycle` claim stage timeline.** T16b is gated on a safety review that is not this plan's authority to schedule.
- **Does not build a second memory-rules path in the CLI fallback.** Per AD-8 / §8.1, CLI transport delegates to the same service/tool modules.
- **Does not split tickets further into per-file tasks.** Per-file breakdown happens at `/implement` time per ticket.
- **Does not re-investigate rmcp annotation support or claim state.** Both are verified facts (§1); only the Claude Code hook contract remains a genuine spike.

---

## 9. Definition of done (per phase)

Inherited from §16 of the architecture plan. Phase-level gates:

| Phase | Done when |
|-------|-----------|
| 0 (spike + contract) | T01 + T02 + T03 merged |
| 1 (tracer-bullet) | T07 demoed end-to-end on a real Claude Code session; observe-only log validates triggers |
| 2 (capability tools) | T05 + T08 + T06 merged; read and write paths each one agent-facing call |
| 3 (capture + background) | T04 + T16a merged; T16b either merged or explicitly gated with a tracked safety-review ticket |
| 4 (bridge) | T15a–d merged; §8.2 matrix verified against installed host versions |
| 5 (security + eval) | T09 + T13 + T14 + T17 merged; §12.4 hard gates pass |
| 6 (rollout) | T18 measured; T19 enabled per adapter only after that adapter's gates pass |

---

## 10. Risks specific to this sequencing

1. **Claim-completion slips or reshapes.** T04/T06/T16 touch its seam. *Mitigation:* the §4 coordination decision is made before T04 starts; the read side (T05/T08) and tracer-bullet ship regardless.
2. **Claude Code hook contract differs from §8.2 assumptions.** T01 front-loads this; if PreCompact or PostToolUse are absent, T15b scope shrinks and §8.2/§8.3 are corrected.
3. **The `lifecycle` safety review never authorizes full supersession.** Then T16b never lands; the system runs with claim enrichment permanently in `Evidence` stage. This is viable — `memory_prepare_task` surfaces contradictions as `insufficient_support` and the agent/user resolves them manually. T09 (tiered warnings) plus the existing `conflicting_fact_id` field already give the agent enough to surface conflicts. The plan must not pretend this can't happen.
4. **Tracer-bullet reveals the capability-facade shape is wrong.** This is the point of T07 — discover it in week 1, not week 8. If it happens, T05/T06 reshape before significant work piles on top.
5. **Public-contract corrections destabilize existing v1.7.0 clients.** *Mitigation:* T03 changes are wire-additive (descriptions and annotations are hints); changelog notes the behavioral shift; minor version bump.

---

## 11. Next action

The first tickets to start are **T01 (Claude hook spike, 1 day)** and **T02 (discipline contract)** — both unblocked, parallel. **The §4 claim-coordination decision** should be made and recorded before T04 starts, but it does not block the read side.

If you want these materialized as ticket files under `.scratch/agent_memory_discipline/issues/` (ask-matt `/to-tickets` convention), say so and I'll create them with full per-ticket task breakdowns.
