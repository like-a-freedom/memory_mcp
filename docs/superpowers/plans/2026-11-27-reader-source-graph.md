# Reader Source Graph Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Qualify one source-graph reader protocol against literal-source citations and consistent/conflicting repeated labels.

**Architecture:** Extend the existing question-independent projection with typed source locations and observed successor pointers. Preserve every previous variant and capture. Gate the candidate before any full-history reader capture.

**Tech Stack:** Python standard library; existing provider-neutral local inference client; native Apple Silicon MLX runtime.

**Spec:** [Source graph design](../specs/2026-11-27-reader-source-graph-design.md)

## Global Constraints

- No new dependency, tool, product route or provider feature.
- Preserve old prompts, raw sources, offsets, labels and failed captures.
- Actual inference stays local native Apple Silicon; no remote inference or LM Studio server.
- Same boxed answer and raw-source literal QUOTE contract; never repair model output.
- One new comparison: 256 completion tokens, 60 seconds/request, zero retries, 1800 seconds/capture.
- Candidate acceptance requires every case completed correct, format-valid, literal-source-quoted; zero faults/truncations.
- Stop protocol iteration if this gate fails; C2 remains separately gated.
- Work on authorized master; no commit or push in this delivery. Keep plan/workspace until acceptance.

## Review Focus

- Same name in different roles/scopes: retain separate occurrence sets.
- Same successor value at different locations: preserve both occurrences, report one distinct value.
- Opaque intervening content: never invent adjacency.
- Clipped/virtualized group: never manufacture an end or completeness.
- Escaped source text: preserve exact raw ranges and quote only delivered source.

---

### Task 1: Typed source graph at public projection and protocol seams

**Files:** Modify `evals/longmemeval_v2/reader_protocol.py`; test `evals/longmemeval_v2/test_reader_protocol.py`.

**Interfaces:** Consume `project_sources(items, ...)`, `protocol_messages(row, variant)`; produce additive `source_graph=True` projection and `source-graph-final-first` variant. Graph fields: parent location, peers with role/name/ordinal/source location and successor state (`observed`, `end`, `unresolved`); repeated labels grouped by role with all occurrence lines and distinct observed successor names. Source text remains fully present. No question/truth argument in graph construction.

- [x] Add public test `test_graph_preserves_literal_locations_and_observed_successor` with a two-button closed toolbar: first successor is source line 3; last successor is end; raw range slices exactly the original button line.
- [x] Run `python3 -m unittest evals.longmemeval_v2.test_reader_protocol.SourceGraphProtocolTests -v`; expected RED: unregistered variant.
- [x] Implement only additive graph projection/rendering and new protocol.
- [x] Repeat focused command; expected PASS.
- [x] Add one public worked control at a time for repeated equal/different successors, role/scope selection, opaque, clipped, virtualized groups. Run each RED before minimal GREEN when a missing rule is exposed; already-supported controls are regression verification, not claimed RED.
- [x] Test `freeze` registers new treatment/projection and `classify` rejects graph-only quotes. Run reader tests plus existing attempt/coverage controls; expected all pass.

### Task 2: Frozen finite qualification

**Files:** Create `evals/longmemeval_v2/fixtures/source-graph-independent-v1/`; create `evals/longmemeval_v2/results/2026-11-27-source-graph-qualification/`.

**Interfaces:** Consume Task 1 public `freeze`/`capture` and unchanged classifier; produce registered prompts, source-reviewed truth, raw outputs, attempt/event ledger and strict decision.

- [x] Preserve all 151 earlier frozen prompts byte-for-byte by recomputing from their pinned rows and variants; expected zero mismatches.
- [x] Author eight source-reviewed development controls before inference; combine with original sixteen, preserving original labels. Cover all five Review Focus conditions. Record authorship/exposure limits.
- [x] Freeze source-ledger and source-graph arms, acceptance, runtime/model identities before capture. Expected 48 registered calls, no outputs.
- [x] Execute once locally; retain all faults. Calculate exposed sixteen and new eight separately; expected objective decision, not assumed PASS.

### Task 3: Conditional full history, review and delivery

**Files:** New full-reader result directory only if Task 2 passes; update main report/plan and qualification README.

**Interfaces:** Consume strict qualification decision and V7 seven-case source review. Produce full seven-case candidate evidence or explicit non-activation.

- **NOT ACTIVATED:** Task 2 failed19/24 strict (22/24 values,one truncation); no conditional fourteen-call full-history capture or product default change. V7 seven-source evidence gate remains7/7, independently of reader success.
- [x] On FAIL: record non-activation; preserve negative candidate, stop protocol tuning.
- [x] Review final source/projection/quote semantics independently of metric aggregation. Run affected Python tests and `git diff --check`; expected PASS.
- [x] Reconcile all actual metrics and conditional C2 NO-GO, stop only the owned MLX process, record cleanup. No commit/push.


## Recorded outcome

Task1 public TDD and regression controls passed. Task2 one48-call capture finished252.861s, candidate completed values22/24, format23/24, literal quotes20/24, strict joint19/24; baseline strict19/24. CandidateFAIL, no retries/label changes. Task3 took the registered FAIL branch: full-history calls0, no default changes, C2NO-GO. Independent review found no Important/Critical projection issue and verified165 earlier prompts plus243 exact raw ranges. Latest portable Python116 passed/11 skipped, paired80, SDK6; owned MLX runtime stopped. Failed qualification is not100% reader quality or successful plan acceptance. Residual work is the main report's versioned source-bound response and autonomous qualification action plan.
