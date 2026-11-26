# Post-release memory quality implementation plan

> **For agentic workers:** Use superpowers:executing-plans and test-driven-development. Execute one task at a time; retain observed RED → GREEN and failed evaluation attempts.

**Goal:** Reduce measured retrieval/reader failures and evaluation cost without weakening evidence, isolation or acceptance criteria.

**Architecture:** Diagnose each failure at its owning boundary. Keep runtime accounting in the evaluator, evidence selection in memory retrieval, and durable access statistics in the memory context. Reuse existing public runners and receipt formats rather than add another harness.

**Tech Stack:** Rust, Python standard library, provider-neutral loopback inference adapters; existing embedded-store integration tests.

**Spec:** `docs/evals/MEMORY_QUALITY_REPAIRS_2026-11-26.md`, “Что дорабатывать и почему”; existing temporal, source-authority and outcome-evaluation specs remain authoritative.

## Global constraints

- Evals run locally on native Apple Silicon; no LM Studio dependency or remote inference. Test fixtures remain portable.
- No new dependency, migration or MCP tool without the repository's explicit approval requirements.
- Historical captures, labels and failed attempts are immutable. Register any new protocol before outputs; never repeat until success.
- Deterministic checks precede model runs. Use one frozen targeted capture and at most its preregistered repeat; report every scheduled case.
- Separate retrieval evidence, answer correctness, provenance, format, errors and latency. Small exposed development cases do not establish general benefit.
- C2 remains NO-GO until Task 7's preregistered qualification gate passes.

## Review focus

- Pending/timeout and unavailable measurements cannot become empty results or successful trials.
- Retry attempts must remain linked to their logical case and role, including abandoned requests and client disconnects.
- A correct alternative source does not erase a miss of the registered source; report both.
- Terminal snake_case project-scope handling is finite; arbitrary names, languages and ambiguous artifact references need independent labels before extending it.
- Performance changes preserve temporal, policy, byte and durability contracts.
- Reader protocol improvements are not memory-treatment effects; new prompts never relabel old captures.

## Task 1 — Measure inference attempts before optimizing runtime (P1.0)

**Files:** `evals/longmemeval_v2/scored_coverage.py`, `test_scored_coverage.py`; existing `crates/eval-harness/src/paired_agent/chat_completions.rs` and `tests/chat_completions_runner.rs` only if the shared adapter also lacks the required observation.

**Interface:** Extend versioned evaluator observations with logical case/role, request/attempt identity, terminal outcome, elapsed time and retry budget. Preserve the existing coverage denominator and raw reader response. Token/prefill timings unavailable from a provider remain null/unobserved, never zero.

- [ ] RED: controlled loopback endpoint returns timeout → success and disconnect → failure. Assert two attempts linked to one logical case, exact outcomes, finite cleanup, and one terminal case outcome. No model is needed.
- [ ] GREEN: observe attempts at the actual transport/retry boundary; configure a finite retry count and per-call deadline for the new protocol. Never infer SDK attempt counts from POST log lines alone.
- [ ] Distinguish server startup, cold request, warm request and retries. Record provider-reported prefill/generation only when actually available; do not derive them from total time.
- [ ] Run `python3 -m unittest evals.longmemeval_v2.test_scored_coverage`; when Rust changes, also `cargo test -p eval-harness --test chat_completions_runner --locked`.
- [ ] Commit `feat(evals): account for inference attempts and cold-start cost`.

**Acceptance:** logical trials and transport requests have separate exact counts; every fault is retained; the previous complete-capture positive control still yields its original score.

## Task 2 — Separate reader protocol failures from memory failures (P1.1)

**Files:** new versioned protocol fixtures under `evals/longmemeval_v2/fixtures/`; `scored_coverage.py`, `test_scored_coverage.py`, existing `exact_answer_quality.py`/`test_exact_answer_quality.py` if additional reporting is required.

**Interface:** Frozen contexts from the seven-case capture; new prompt/protocol identity, with independent value/refusal/format/error dimensions. Keep upstream official scoring separate.

- [ ] RED: deterministic fixtures expose U+0008 from the upstream `\b` escape, truncated final output, correct refusal and a fabricated quotation. Assert distinct classifications and the unchanged seven-case denominator.
- [ ] GREEN: define a new local protocol with literal escaped formatting instructions; retain raw prompts, finish_reason and transport attempt receipts from Task 1. Do not patch the pinned upstream capture in place.
- [ ] Preregister a bounded comparison of source-first versus final-answer-first on identical contexts/model/limits. Compare a readable source projection separately; preserve source identity and exact offsets. A different token cap is a separately labelled protocol intervention.
- [ ] Run the Python controls before one local capture. Record all seven results and stop under the frozen rule; do not select the best retry.
- [ ] Commit `feat(evals): separate reader protocol and evidence-use outcomes`.

**Acceptance:** false-premise refusal, answer value, final formatting and errors are independently observable; any gain is labelled reader/protocol gain, not improved retrieval or proven general accuracy.

## Task 3 — Diagnose the remaining source loss (P1.2)

**Files:** `crates/memory-mcp/tests/full_history_evidence.rs`, `src/eval_support/retrieval.rs`, owning modules under `src/memory/retrieval/`; existing `evals/longmemeval_v2/long_history_diagnosis.py`.

**Interface:** Actual formation → fetch → admission → ranking → selection → packing receipts, source/fact identity and stage cost. Reuse existing observations; extend only a genuinely missing boundary.

- [ ] Freeze the missing `922a0c56` case and exploratory 32-source case with exact trajectory/source hashes, actual query, temporal/policy selectors and process budget. Keep primary and exploratory labels distinct.
- [ ] Capture one bounded diagnostic on isolated whole-store copies. Mark any unobserved stage unknown rather than infer a cause.
- [ ] RED: derive a small independent public use-case regression for the first proven losing boundary, with a non-relevant opposite control and exact source/temporal assertions.
- [ ] GREEN: change only that owning stage; never promote gold IDs, query benchmark answers or raise limits merely to include the source.
- [ ] Run the affected public target, native eight-case gate and unchanged primary full-history pair. Preserve all case denominators and report alternative support separately from gold-span coverage.
- [ ] Commit `fix(memory): preserve evidence at the diagnosed retrieval boundary` only if a failure and repair are demonstrated; otherwise commit the bounded diagnosis with its explicit unresolved cause.

**Acceptance:** independent regression RED → GREEN, no temporal/policy/byte-cap regressions; a completed context alone does not count as a correct answer.

## Task 4 — Reduce durable access cost only after measuring it (P1.3)

**Files:** `crates/memory-mcp/src/memory/fact_access_store.rs`, owning retrieval access-update caller; `tests/fact_access_concurrency.rs`, `tests/fact_access_payload.rs`.

**Interface:** Preserve `record_fact_access(&self, fact_id: &str, boost: i64) -> Result<(), MemoryError>` and durability before return unless a separately approved contract explicitly replaces it.

- [ ] Characterize update count/identity and elapsed cost under controlled small/large source and index-key payloads. Separate concurrent waves from serial total latency.
- [ ] RED: observable resource test demonstrates the proven excess work while integration controls pin count, ordering, decay, vector-write concurrency and absent-record behavior.
- [ ] GREEN: minimize the owning write/projection. Consider a separate statistics record only if measurement justifies it; a schema migration requires approval before implementation. No fire-and-forget replacement.
- [ ] Run both access integration targets and affected caller tests; repeat one preregistered full-store pair only after deterministic success.
- [ ] Commit `perf(memory): reduce measured durable access write cost`.

**Acceptance:** exact persisted statistics and error/cancellation semantics remain intact; report latency and resource cost without claiming RSS reduction from allocation counts.

## Task 5 — Independently validate entity admission (P2.1)

**Files:** new versioned source-reviewed extraction corpus under `evals/corpora/`; existing extraction suite and `crates/memory-mcp/src/knowledge/entity_extraction/anno.rs`, `src/memory/episode/entity_extraction.rs` only for reproduced defects.

- [ ] Freeze independent multilingual labels before predictions: currencies, units, metric/acronym ambiguity, named projects, genuine names with digits and manual resolve. Define exhaustive/non-exhaustive entity policy explicitly.
- [ ] Run current extractor once and report TP/FP/FN plus linking by class; audit older automatic projections separately from new formation.
- [ ] For each reproduced defect, add independent public RED and opposite controls, then the smallest owning admission fix and GREEN. Preserve manual identities and the no-delete audit rule.
- [ ] Run scoped extraction integration tests and the frozen corpus. Reprepare fingerprint-bound native inventories only from source, comparing bindings explicitly.
- [ ] Commit each independently justified repair; if none is required, commit the independent evaluation rather than invent a product change.

**Acceptance:** precision gains do not lose genuine/manual identities; finite language coverage is stated. Historical data migration is a separate decision, not a silent cleanup.

## Task 6 — Validate agent citation usability (P2.2)

**Files:** `evals/paired/memory_quality_diagnostic.py` and its existing public CLI tests; new independent multi-source task/probe fixtures under `evals/paired/`. Product explain/response changes require their own reproduced need.

- [ ] RED: wrong/unknown source ID, borrowed quote, duplicate handle and quote from another source must fail; equivalent source-bound output must pass.
- [ ] Freeze independent texts and compare raw IDs versus bounded handles with identical evidence/order and budget. Reuse existing runner; do not repair agent output before scoring.
- [ ] Evaluate value, source binding, exact quote, completeness and refusal separately, including every failed call.
- [ ] Only then propose a product response change; preserve exact IDs and provenance continuation. Commit `test(evals): qualify multi-source citation usability`.

**Acceptance:** deterministic negative controls pass and qualification is limited to the declared workflow. Text-only LongMem scores remain separate from citation quality.

## Task 7 — Decide whether an independent benefit study is warranted (P3)

**Files:** new spec/protocol under `docs/superpowers/specs/` and frozen independent plan/probes under `evals/paired/`; reuse current paired runner and power calculation rather than add new machinery.

- [ ] Require Task 1 fault accounting and a qualified targeted workflow from Tasks 2/6. Check actual autonomous queries and temporal selectors with independent probes; include formation if it is part of the benefit claim.
- [ ] Register unseen histories, treatment/control, family-level paired unit, minimum detectable effect, power, costs, failure handling and finite stopping rule before any outputs.
- [ ] Run a bounded pilot only if qualified; use its discordance to calculate confirmatory sample size. If infeasible, record NO-GO with cost rather than launch an underpowered study.
- [ ] Run the frozen confirmation only after its gate passes; retain failures in the denominator and report paired effect uncertainty and cost.
- [ ] Commit `docs(evals): record independent benefit study decision` and results if executed.

**Acceptance:** no restart of the old exposed C1 until success, no universal guarantee, and no C2 benefit claim before an independently qualified, adequately powered result.

## Task 8 — Qualify skill behavior beyond static review (P2.3)

**Files:** `skills/memory-mcp/evals/evals.json`, `skills/memory-cli/evals/evals.json`; corresponding `SKILL.md` and references only when an observed failure requires a change.

- [ ] Freeze the existing 16 prompts and objective checks before execution. Use disposable synthetic sources and advertised tool/CLI schemas; no real private memory writes.
- [ ] Compare current and previous skill instructions on the same provider-neutral local agent setup. Preserve attempted calls and responses, including partial extraction with exit zero, temporal refinement and truncated provenance.
- [ ] Score correct operation/arguments, pending versus completed classification, source binding and unnecessary context loading separately. Static link checks are not agent success rates.
- [ ] For each observed failure, add a discriminating control, edit the smallest instruction/reference and repeat only the registered regression check. Report model/setup limits and token/time cost.
- [ ] Commit `test(skills): qualify memory workflow behavior` with the actual results, or retain explicit untested status if no qualifying local agent is available.

**Acceptance:** the 16 prompts have observed outcomes rather than only expected-output descriptions; no assertion of universal triggering, completeness or correctness. Keep short entrypoints and task-selected references.

## Final verification and retention

Run smallest affected targets during each task; before shipping run required workspace clippy, fmt, affected crate/harness suites and deterministic memory gate. Use immutable product binaries for captures so concurrent Cargo builds cannot replace an evaluated executable. Archive compact command/outcome/hash receipts; keep generated weights, stores and caches outside tracked evidence. Preserve this plan until implementation, testing and user acceptance.
