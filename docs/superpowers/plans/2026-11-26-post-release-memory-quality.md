# Post-release memory quality implementation plan

> **For agentic workers:** Use superpowers:executing-plans and test-driven-development. Execute one task at a time; retain observed RED → GREEN and failed evaluation attempts.

**Goal:** Reduce measured retrieval/reader failures and evaluation cost without weakening evidence, isolation or acceptance criteria.

**Architecture:** Diagnose each failure at its owning boundary. Keep runtime accounting in the evaluator, evidence selection in memory retrieval, and durable access statistics in the memory context. Reuse existing public runners and receipt formats rather than add another harness.

**Tech Stack:** Rust, Python standard library, provider-neutral loopback inference adapters; existing embedded-store integration tests.

**Spec:** `docs/evals/MEMORY_QUALITY_REPAIRS_2026-11-26.md`, “Что дорабатывать и почему”; existing temporal, source-authority and outcome-evaluation specs remain authoritative.

## Execution status — 27 November 2026

See [new results and action plan](../../evals/MEMORY_QUALITY_FOLLOWUP_2026-11-27.md). Checkboxes below indicate observed steps, not general correctness. No commits/push are made in this delivery; commit instructions are explicitly deferred.

| Task | Status | Evidence / limit |
| --- | --- | --- |
| 1 | Implemented and checked | Actual attempt accounting; portable controlled endpoints plus real SDK reader/judge controls. Startup/prefill/generation remain explicitly unobserved where unavailable. |
| 2 | Historical comparison complete; no reader gain | 21/21 outputs, 3/7 values and 0/3 refusals per variant; old captures unchanged. |
| 3 | Independent selection repair passed all source/native gates | Full selected100 histories/151 source segments restored and audited twice. V3/v4 strict spans4/7; sufficient sources6/7, with one retrieval timeout. V5 fresh pair completed7/7 both, source adequacy6/7. Independent33-source rank-bound and unrelated-dispute RED→GREEN yielded V7: native8/37 passed and all7 full reads completed, strict5/7 and independently sufficient7/7. V5 FAIL and all earlier captures remain unchanged. |
| 4 | Physical repair verified; original V5 combined FAIL retained | Heat phase localized. Registered native prototype120/120; high-key combined ratios.0295/.0266. User approved057 migration. Exact backfill, owner/readiness fencing, ranking/decay/archival integration and native upgrade/rollback3/3 verified; fresh14-read pair: both arms7/7 completed within30/45s, contexts equal7/7, strict4/7 and sufficient6/7. Physical contracts pass; combined source gate fails. |
| 5 | Finite repair and validation completed | 16 independent cases: precision .60→.75, recall unchanged .667; 927 native bindings unchanged after real v3 preparation. Four exact-label failures retained. |
| 6 | Completed diagnostic; handles not qualified | Initial formation failure retained. Explicit stored-episode fixed-only v2: value/quote8/8, source5/8, raw4/4 versus handles1/4. No default-format change, no successful-retry selection. |
| 7 | Admission decision completed: NO-GO | Qualification missing; pilot/confirmation not admitted, power/effect unknown. Conditional design is not a full preregistration. |
| 8 | Dynamic comparison executed; qualification failed |16 prompts×previous/current32/32 attempted. Full pass8/32;18 timeouts. Native5/14, controlled3/18. Versioned operation fixes preserve frozen outcomes; instructions unchanged because failures do not establish a missing instruction. |
| 9 | Complete source restoration verified |100 trajectories/151 exact stored bodies; two reopened audits and immutable-store hashes. |
| 10 | Both finite reader comparisons executed; qualification FAIL |Source-ledger32: values15/16, strict13/16. Source-graph48: values22/24, strict19/24 versus baseline19/24; one truncation, transport errors0. Full7 reader NOT_ACTIVATED, default unchanged. All165 earlier prompts and original labels preserved. |
| 11 | Dynamic execution/regrade complete |All32 outcomes retained; qualificationFAIL is a result, not a missing capture. |
| 12 | Verification/review/reconciliation completed; negative gates retained |Final guard12+5, required workspace/eval clippy+fmt passed. Latest Python116 passed+11 skipped, paired80/80, SDK6/6; diff check passed. Independent graph review:165 prompts,243 raw ranges,zero Important/Critical findings. Owned model runtime stopped. |

Final fresh-context review found five evaluator defects; all fixed through RED→GREEN controls. Delivered-quote audit corrects final-first quote4/7→3/7 while preserving original captures. New transports reject redirects/proxies and journal started requests before dispatch. Full affected Python discovery is green; details and exact counts are in the linked verification receipt.

The plan remains until testing and user acceptance. Full source restoration, physical paired capture and dynamic execution are complete; their failed quality/resource gates remain visible. V7 source selection delivered all7 contexts with strict5/7; its native8/37 gates passed. Do not claim skill qualification or C2 benefit. The finite implementation/capture/review steps are complete; this does not satisfy the failed reader/skill acceptance criteria. Reader and autonomous qualification remain residual quality work in the linked action plan. C2 pilot/confirmation remain deliberately unactivated; no improvement guarantee or full-plan100% acceptance is claimed.

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

- [x] RED: controlled loopback endpoint returns timeout → success and disconnect → failure. Assert two attempts linked to one logical case, exact outcomes, finite cleanup, and one terminal case outcome. No model is needed.
- [x] GREEN: observe attempts at the actual transport/retry boundary; configure a finite retry count and per-call deadline for the new protocol. Never infer SDK attempt counts from POST log lines alone.
- [x] Distinguish server startup, cold request, warm request and retries. Record provider-reported prefill/generation only when actually available; do not derive them from total time.
- [x] Run `python3 -m unittest evals.longmemeval_v2.test_scored_coverage`; when Rust changes, also `cargo test -p eval-harness --test chat_completions_runner --locked`.

**Acceptance:** logical trials and transport requests have separate exact counts; every fault is retained; the previous complete-capture positive control still yields its original score.

## Task 2 — Separate reader protocol failures from memory failures (P1.1)

**Files:** new versioned protocol fixtures under `evals/longmemeval_v2/fixtures/`; `scored_coverage.py`, `test_scored_coverage.py`, existing `exact_answer_quality.py`/`test_exact_answer_quality.py` if additional reporting is required.

**Interface:** Frozen contexts from the seven-case capture; new prompt/protocol identity, with independent value/refusal/format/error dimensions. Keep upstream official scoring separate.

- [x] RED: deterministic fixtures expose U+0008 from the upstream `\b` escape, truncated final output, correct refusal and a fabricated quotation. Assert distinct classifications and the unchanged seven-case denominator.
- [x] GREEN: define a new local protocol with literal escaped formatting instructions; retain raw prompts, finish_reason and transport attempt receipts from Task 1. Do not patch the pinned upstream capture in place.
- [x] Preregister a bounded comparison of source-first versus final-answer-first on identical contexts/model/limits. Compare a readable source projection separately; preserve source identity and exact offsets. A different token cap is a separately labelled protocol intervention.
- [x] Run the Python controls before one local capture. Record all seven results and stop under the frozen rule; do not select the best retry.

**Acceptance:** false-premise refusal, answer value, final formatting and errors are independently observable; any gain is labelled reader/protocol gain, not improved retrieval or proven general accuracy.

## Task 3 — Diagnose the remaining source loss (P1.2)

**Files:** `crates/memory-mcp/tests/full_history_evidence.rs`, `src/eval_support/retrieval.rs`, owning modules under `src/memory/retrieval/`; existing `evals/longmemeval_v2/long_history_diagnosis.py`.

**Interface:** Actual formation → fetch → admission → ranking → selection → packing receipts, source/fact identity and stage cost. Reuse existing observations; extend only a genuinely missing boundary.

- [x] Freeze the missing `922a0c56` case and exploratory 32-source case with exact trajectory/source hashes, actual query, temporal/policy selectors and process budget. Keep primary and exploratory labels distinct.
- [x] Capture one bounded diagnostic on isolated whole-store copies. Mark any unobserved stage unknown rather than infer a cause.
- [x] RED: derive a small independent public use-case regression for the first proven losing boundary, with a non-relevant opposite control and exact source/temporal assertions.
- [x] GREEN: change only that owning stage; never promote gold IDs, query benchmark answers or raise limits merely to include the source.
- [x] Run the affected public target and native8/37 gates; retain the unchanged V5 primary14-read pair and separate registered V7 seven-read source-selection replay. V7 runtime7/7, strict5/7, independent sufficiency7/7; alternate6d/d6 remain strict misses. V7 replay is not a new paired latency study.

**Acceptance:** independent regression RED → GREEN, no temporal/policy/byte-cap regressions; a completed context alone does not count as a correct answer.

## Task 4 — Reduce durable access cost only after measuring it (P1.3)

**Files:** `crates/memory-mcp/src/memory/fact_access_store.rs`, owning retrieval access-update caller; `tests/fact_access_concurrency.rs`, `tests/fact_access_payload.rs`.

**Interface:** Preserve `record_fact_access(&self, fact_id: &str, boost: i64) -> Result<(), MemoryError>` and durability before return unless a separately approved contract explicitly replaces it.

- [x] Characterize update count/identity and elapsed cost under controlled small/large source and index-key payloads. Separate concurrent waves from serial total latency.
- [x] RED: counterfactual old writer mutates the full immutable fact; independent fixtures pin arithmetic, count/order, absent-record behavior and vector-write concurrency. Controlled phase faults and native high-key prototype demonstrate the physical-design need. Initial nondiscriminating decay fake is retained and explicitly excluded; corrected fault adapter establishes genuine RED.
- [x] GREEN: after the passed prototype and explicit user approval, add057 fact_access and bounded exact-key reads. Preserve awaited attempt completion, signed saturation, source identity, temporal/policy rules, zero/timestamp-only migration states, durable reopen and error propagation. No fire-and-forget replacement.
- [x] Run both access integration targets and affected caller tests, including native upgrade/rollback3/3.
- [x] Complete the separately preregistered14-read full-store comparison after deterministic/native success; preserve30/45s and all original assertions. Both arms7/7 runtime, strict4/7, sufficient6/7; combined source gate FAIL, physical/native contracts PASS.

**Acceptance:** exact persisted statistics and error/cancellation semantics remain intact; report latency and resource cost without claiming RSS reduction from allocation counts.

## Task 5 — Independently validate entity admission (P2.1)

**Files:** new versioned source-reviewed extraction corpus under `evals/corpora/`; existing extraction suite and `crates/memory-mcp/src/knowledge/entity_extraction/anno.rs`, `src/memory/episode/entity_extraction.rs` only for reproduced defects.

- [x] Freeze independent multilingual labels before predictions: currencies, units, metric/acronym ambiguity, named projects, genuine names with digits and manual resolve. Define exhaustive/non-exhaustive entity policy explicitly.
- [x] Run current extractor once and report TP/FP/FN plus linking by class; audit older automatic projections separately from new formation.
- [x] For each reproduced defect, add independent public RED and opposite controls, then the smallest owning admission fix and GREEN. Preserve manual identities and the no-delete audit rule.
- [x] Run scoped extraction integration tests and the frozen corpus. Reprepare fingerprint-bound native inventories only from source, comparing bindings explicitly.

**Acceptance:** precision gains do not lose genuine/manual identities; finite language coverage is stated. Historical data migration is a separate decision, not a silent cleanup.

## Task 6 — Validate agent citation usability (P2.2)

**Files:** `evals/paired/memory_quality_diagnostic.py` and its existing public CLI tests; new independent multi-source task/probe fixtures under `evals/paired/`. Product explain/response changes require their own reproduced need.

- [x] RED: wrong/unknown source ID, borrowed quote, duplicate handle and quote from another source must fail; equivalent source-bound output must pass.
- [x] Freeze independent texts and compare raw IDs versus bounded handles with identical evidence/order and budget. Reuse existing runner; do not repair agent output before scoring.
- [x] Evaluate value, source binding, exact quote, completeness and refusal separately, including every failed call.
- [x] Decision: keep current provenance format; handles failed qualification. No product response change is justified by these results.

**Acceptance:** deterministic negative controls pass and qualification is limited to the declared workflow. Text-only LongMem scores remain separate from citation quality.

## Task 7 — Decide whether an independent benefit study is warranted (P3)

**Files:** new spec/protocol under `docs/superpowers/specs/` and frozen independent plan/probes under `evals/paired/`; reuse current paired runner and power calculation rather than add new machinery.

- [x] Check Task 1 fault accounting and the required targeted/autonomous qualification from Tasks 2/6: the latter is not qualified. The admission decision is NO-GO; exposed fixed-reply results do not establish autonomous query/time/formation success.
- Not admitted: full unseen-history pilot registration requires the qualification gate; see the separate NO-GO decision. No synthetic power result replaces a pilot.
- Not activated: the workflow gate failed, so no pilot, discordance estimate or confirmatory sample size is claimed.
- Not activated: no qualified/powered confirmation exists; retain this as a conditional research stage, not an executed implementation task.

**Acceptance:** no restart of the old exposed C1 until success, no universal guarantee, and no C2 benefit claim before an independently qualified, adequately powered result.

## Task 8 — Qualify skill behavior beyond static review (P2.3)

**Files:** `skills/memory-mcp/evals/evals.json`, `skills/memory-cli/evals/evals.json`; corresponding `SKILL.md` and references only when an observed failure requires a change.

- [x] Freeze the existing 16 prompts and objective checks before execution. Use disposable synthetic sources and advertised tool/CLI schemas; no real private memory writes.
- [x] Compare previous/current instructions on one frozen provider-neutral local setup:32 attempted trials, native/controlled arms reported separately. Partial extraction, temporal refinement, provenance and failures retained.
- [x] Score operation/arguments, pending/completed state, source binding, reference adherence and final semantic success separately. All32 hashes verified; full8/32. Static links are not counted as agent success.
- [x] Audit failure against actual instructions and add discriminating evaluator controls. Required status/provenance text already exists; timeouts and model omissions do not justify tuning instructions to outputs. Preserve qualification FAIL and resource limits instead of retrying for success.

**Acceptance:** the 16 prompts have observed outcomes rather than only expected-output descriptions; no assertion of universal triggering, completeness or correctness. Keep short entrypoints and task-selected references.

## Task 9 — Restore and prove complete history (remaining Task 3 acceptance)

**Seams:** existing source-only `MemoryMcpBackend.insert`, public CLI/MCP formation, snapshot readback through the episode store. The previously authorized public seams remain unchanged.

- [x] RED→GREEN: pending extraction cannot increment the completed trajectory count.
- [x] Freeze all ordered100 input trajectories and deterministic segment identities before formation; retain every attempt and bound child lifetimes. Original1800s resource gate failed; the same attempt completed4501s under predeclared5400s rescue.
- [x] Rebuild from source without answers/labels; verify151 exact stored source segments in two reopened clones. Per-call extraction responses were not retained by the archived backend: completion is inferred from its checked CLI status contract, not directly journaled. Future backend now retains strict start/terminal receipts.
- [x] Run unchanged primary source checks and all seven frozen questions in v3 and v4. Strict spans4/7, sufficient source support6/7; retain922 timeout. Final approved physical candidate is a separate preregistered paired gate, never a replacement for these failures.

## Task 10 — Repair reader failures at the proven boundary

- [x] Inspect raw failures and delivered evidence; distinguish missing source, incorrect reasoning, derived annotations misquoted as source, format and length truncation. Retain completed-value and completed+literal-quote metrics separately.
- [x] Add independent deterministic regressions before reader changes. Ordered relation view then source-ledger/quote separation have public RED→GREEN controls;119 previous registered prompts remain byte-identical. Keep budgets, labels and denominator fixed.
- [x] Register one bounded local protocol comparison before inference; capture all scheduled cases without retry selection. Source-ledger32-call gate FAIL: completed values15/16, completed+format+literal13/16. Full7 conditional comparison is NOT_ACTIVATED, not a missing capture. A separately bounded [source-graph implementation](2026-11-27-reader-source-graph.md) records one follow-up at unchanged limits; no protocol retry selection or memory-gain claim.

## Task 11 — Execute dynamic skill comparison

- [x] Build a bounded provider-neutral agent host at authorized MCP/CLI/reference-read seams via TDD. Native14 trials and controlled18 trials remain separate.
- [x] Freeze exact16 prompts, previous/current instructions, tools, rubric, model and9-turn/384-token/20s-request/180s-trial/0-retry stopping rule before outputs.
- [x] Execute both arms locally; preserve all32 calls/identities/reference loads/errors/cost. MCP semantic review independent; CLI adjudication blind to arms. Full criterion8/32, answers14/32, timeouts18/32.
- [x] Repair demonstrated evaluator defects through discriminating controls: legitimate default evidence and completed operations before answer timeout; future operation-v2 additionally accepts strictly linked authorized CLI readback. Frozen regrade16/32 is immutable; prospective18/32 is not a new full pass or inference capture. No demonstrated missing instruction justified a skill-text edit.

## Task 12 — Verify and reconcile remaining acceptance

- [x] Run affected deterministic tests, native memory gates and required clippy/fmt after owning code changes. Final Rust guard12+5, native8/37, required clippy/fmt and latest Python116+80+6 passed;11 optional LongMem tests skipped.
- [x] Obtain independent final review and fix substantive findings through RED→GREEN. Final physical readiness finding fixed12+5; graph review found zero Important/Critical defects and validated243 raw ranges. Actual reader FAIL19/24 and skill FAIL8/32 retained.
- [x] Reassess C2 qualification: versioned decision-v3 retains NO-GO after V7 source7/7 and graph19/24 FAIL; fresh qualified autonomous captures0, pilot/power/effect unobserved. No commits or publication in this delivery.

## Final verification and retention

Run smallest affected targets during each task; before shipping run required workspace clippy, fmt, affected crate/harness suites and deterministic memory gate. Use immutable product binaries for captures so concurrent Cargo builds cannot replace an evaluated executable. Archive compact command/outcome/hash receipts; keep generated weights, stores and caches outside tracked evidence. Preserve this plan until implementation, testing and user acceptance.
