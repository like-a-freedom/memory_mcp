# Memory Evaluation Qualification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Establish one fast native acceptance route and distinguish scripted memory correctness, real workflow reliability, resource cost and confirmatory evidence.

**Architecture:** Reuse the existing typed registration, native public MCP replay and offline comparators. Qualify an actual workflow on fresh synthetic families before authorizing larger captures. Run the official external benchmark and independent product investigations as separately gated slices.

**Tech Stack:** Rust, Cargo, serde JSON, MCP stdio, native Apple Silicon, optional provider-neutral capture, pinned Python upstream adapter for LongMemEval-V2.

**Spec:** [Agent memory follow-up, Tasks 4–5 and independent backlog](../specs/2026-11-24-agent-memory-follow-up-design.md#task-4-qualify-workflows-and-confirm-only-eligible-effects) · [Agent outcome acceptance](../specs/2026-11-09-agent-memory-outcome-evaluation-design.md#promotion-decision)

## Global Constraints

- The sole required eval target for this plan is native `aarch64-apple-darwin`. Run the deterministic gate locally with locked Cargo, without a model server, vendor login or provider keys. Existing CI jobs stay in place.
- Keep deterministic product gates separate from optional live provider captures. Remote Codex authorization applies to synthetic task data only.
- Preserve prior v4–v10 fixtures, preregistrations and negative receipts byte-for-byte. Freeze new families, oracle, implementation and runner identity, budgets and analysis rules before capture.
- Treat related cases and repeated captures as correlated. Use task families as the sampling unit. Keep failures in the denominator.
- Confirm proposed public test seams in plan review before writing tests. Use experiments to establish product gaps before choosing an implementation.

## Review Focus

- A perfect probe score alongside a failed ordinary task remains a qualification failure. C1.
- Two replays of the same family do not count as two independent families. C2.
- The native gate must prove it executed its ignored real-binary replay and wrote actual receipts. C3.
- A failed upstream shared memory build affects every dependent question. C4.
- Compact and verbose explain budgets apply to the returned tool envelope, including JSON escaping. D1.

## File Map and Proposed Seams

| Files | Responsibility and observable boundary |
| --- | --- |
| `crates/eval-harness/src/{paired,paired_power}.rs`, `src/paired_agent/managed.rs`, `tests/paired_comparison_cli.rs` | Offline CLI on frozen plan and receipts. Asserts eligibility, denominator, replay and resource policy. Provider-dependent workflow outcomes require live evidence. |
| `evals/paired/`, `docs/evals/PAIRED_AGENT_EVAL.md` | Exact runner/workflow registrations, qualification results and evidence scope. |
| `crates/eval-harness/tests/deterministic_quality_gate.rs`, `evals/{corpora,profiles}/`, `docs/evals/DETERMINISTIC_MEMORY_QUALITY.md` | Native public MCP replay, v9 compatibility and new frozen case families. |
| `evals/longmemeval_v2/` and [official run spec](../specs/2026-11-09-longmemeval-v2-official-run-design.md) | Upstream lifecycle, text cohort and official run receipts. |
| `crates/memory-mcp/src/tools/explain.rs`, `crates/eval-harness/src/suites/response_size.rs`, `crates/eval-harness/tests/response_size_public_tool.rs` | D1. Actual public tool envelope and lossless excerpt reconstruction. |
| [Extraction baseline](../specs/2026-11-02-extract-quality-and-cost.md) and [latency design](../specs/2026-11-14-assemble-context-latency-design.md) | D2/D3. Existing evidence and conditional follow-up. |

## Task C1. Qualify workflows under independent gates

**Interfaces.** Consume A1–A5 read diagnostics for recall. Ordinary recall can
be captured with `run-paired-chat-completions` plus its independent-probe
envelope, or by another runner that emits the versioned paired artifact
contract. `qualify-agent-workflow` applies the ordinary task, direct agent-read,
independent-probe, safety and budget gates to two paired captures. The existing
`summarize-pilot`, `summarize-paired-reliability`, and `compare-agent-managed`
remain available for their distinct analyses.

- [x] Add a provider-neutral offline qualification command, a positive and
  negative unit gate, CLI parsing checks, and an end-to-end CLI regression
  where 0/4 task successes cannot pass despite 4/4 independent probes.
- [x] Freeze four fresh ordinary families, two captures, one exact local
  configuration, per-capture thresholds, resource ceilings and probe inputs
  in `evals/paired/agent-workflow-qualification-withdraw-tools-v3-2026-11-25/`.
- [x] Diagnose and fix exhausted tool-budget behavior with a focused RED →
  GREEN runner regression. `tool_choice: "none"` was insufficient for the
  tested provider; the fixed runner omits tool definitions and tool-control
  fields after the registered budget is consumed. The initial response-format
  NO-GO and v2 budget-exit NO-GO remain archived as diagnostic evidence.
- [x] Run both v3 frozen captures on native Apple Silicon. Candidate and
  incumbent each scored task/read/probe 4/4 per capture, with zero hard
  violations and resource exits. The offline qualification replay produced
  an identical report. See the [v3 report](../../../evals/paired/agent-workflow-qualification-withdraw-tools-v3-2026-11-25/README.md).
- [x] Produce the v3 `qualify-agent-workflow` report and descriptive pilot
  summary. Preserve all outcomes and report latency as inconclusive; the
  same-binary arms do not estimate a memory treatment effect.
- [x] Screen typed contradiction on four fresh synthetic families with
  current direct MLX and the provider-neutral Chat Completions runner. The
  independent typed-evidence probes passed 4/4 in each arm and direct reads
  returned the expected sources 4/4, but exact task success was 0/4 in each
  arm (`final_answer_not_json`: 2; `unauthorized`: 2). The preregistered 3/4
  task floor failed, so the second capture was correctly not run. This is a
  workflow no-go for that exact agent setup, not evidence of a memory treatment
  effect. See the [typed-conflict screen](../../../evals/paired/c1-typed-conflict-qwen-2026-11-25/README.md).
- [x] Screen operator import on four fresh synthetic source-file families
  using identical registered source bytes in the literal prompt and imported
  file. Both arms passed direct memory and independent evidence checks 4/4;
  exact task success was 0/4 in both, and candidate formation used 31 agent
  tool calls versus 8 for incumbent. The 3/4 task gate failed, so no repeat
  was run. Import and extraction succeeded 4/4; this is a workflow no-go, not
  evidence of memory treatment effect. See the
  [operator-import screen](../../../evals/paired/c1-operator-import-qwen-direct-mlx-2026-11-25/README.md).
- [x] Freeze and evaluate agent-managed correction on fresh synthetic
  families. The first Codex attempt exited before any model turn; it is
  retained as an invalid setup artifact, not a scored workflow failure. A
  separate synthetic CLI preflight succeeded with the local Codex state
  available, then a fresh disjoint four-family cohort ran. All four corrections
  were extracted and invalidated in both arms, all evidence responses arrived
  before invalidation, and all independent memory probes passed. Exact task
  success was 3/4 incumbent and 2/4 candidate, below the frozen 3/4 per-arm
  gate, so no repeat ran and C2 is not eligible. Both arms used the same
  product; this is a workflow no-go, not a memory treatment effect. The
  receipts also leave agent-read completeness unknown. See the [C1 correction
  screen](../../../evals/paired/c1-agent-managed-correction-codex-direct-mlx-2026-11-25/README.md)
  and [archived setup attempt](../../../evals/paired/c1-agent-managed-correction-codex-2026-11-25/README.md).

- [x] Close the content-free read-normalization gap found in C1: the Codex
  adapter can receive the standard JSON-RPC `result` wrapper, while the
  normalizer previously recognized only a bare `CallToolResult`. RED → GREEN
  tests verify item count, source IDs and completeness through the wrapper,
  while an unrecognized content-only payload remains `unknown`. The frozen
  C1 receipts remain unchanged and still omit raw tool output.
- [x] Verify the wrapper and read diagnostics on a fresh disjoint Codex C1
  cohort after B5. Capture 01 produced 13 structured reads: all had recognized
  empty/nonempty outcomes and counts; all five nonempty reads had source
  coverage. Four reads reported partial completeness and nine reported
  unknown. The captured content-free contract cannot distinguish an absent
  completeness field from an unrecognized value, and does not persist raw
  source IDs. Candidate task success was 2/4 against the frozen 3/4 floor
  (incumbent 3/4); all independent probes, extraction, invalidation, and
  evidence-before-invalidation checks passed 4/4 in both arms. The stop rule
  forbids capture 02. The registration also over-specified completeness for
  reads that may not carry temporal metadata and requested raw IDs in
  content-free receipts. Preserve the no-go and correct those assumptions in
  a new registration; do not infer product quality or latency changes from
  these same-binary arms. See the [C1 read-diagnostics report](../../../evals/paired/c1-agent-managed-correction-read-diagnostics-2026-11-25-v3/README.md).

**Current C1 result (2026-11-25):** The archived correction workflow remains
NO-GO at 2/4 candidate task success. C1 recovery v1 was an invalid setup: the
Codex sandbox exited before a model turn. A fresh disjoint v2 synthetic cohort
then completed capture 01 on native Apple Silicon. In both same-product arms,
all four corrections were extracted, verified and invalidated with the exact
stale target; independent probes found the new source 4/4; no hard or resource
violations occurred. Yet every agent read was empty and task success was 0/4,
so the task floor failed and capture 02 correctly did not run. This is a
workflow no-go, not a memory treatment estimate. The v2 evaluation also exposed
a preregistration defect: it required at least one authorized query retry,
while all results were partial and the frozen recall instructions forbid retry
on partial reads. Correct that in a new registration. The receipts contain
only hashes of model-issued query and selector arguments, so the exact source
of empty agent reads is not yet identified. Add privacy-preserving query-shape
diagnostics before choosing a retrieval change. C2 remains ineligible. See the
[v1 setup record](../../../evals/paired/c1-agent-managed-correction-recovery-v1-2026-11-25/README.md)
and [v2 result](../../../evals/paired/c1-agent-managed-correction-recovery-v2-2026-11-25/README.md).

**Latest C1 result — recovery v5 (2026-11-25):** The corrected retry rule
completed a fresh native Apple Silicon capture. Both arms passed correction
extraction, exact stale-fact invalidation, evidence-before-invalidation, and
independent probes 4/4, with zero hard violations, timeouts, or prohibited
sources. However, all 16 agent-facing reads were empty and partial; exact cited
task success was 0/4 per arm, so the frozen 3/4 floor failed and capture 02 was
not run. First-query hashes matched between arms within each family but did not
match the registered question. Non-query argument hashes differed across arms
in three of four families. Since raw query and selector arguments are not
retained, the run cannot separate a query-formulation miss from a temporal or
other selector mismatch. Do not change retrieval or response presentation on
this evidence alone. The next diagnostic needs private-oracle comparisons of
the registered and observed read selectors, while persisting only normalized
match outcomes. See the [v5 report](../../../evals/paired/c1-agent-managed-correction-recovery-v5-2026-11-25/capture-01/REPORT.md)
and [audit-ready index](../../../evals/paired/c1-agent-managed-correction-recovery-v5-2026-11-25/README.md).

**Latest C1 result — valid-time diagnostic v6 (2026-11-25):** A fresh
four-family cohort again passed correction extraction, exact invalidation,
pre-invalidation evidence, and independent probes 4/4 in both arms, but agent
task success remained 0/4 and all 16 reads were empty/partial. The registered
stop rule prohibited capture 02. Requested and returned `valid_at` matched on
all reads, so valid-time selection does not explain this cohort. First query
hashes matched across arms but did not equal the registered question; aggregate
non-query hashes matched across arms in only 2/4 families, so selector details
remain unresolved. Critical review found the scorer had reported a full-cutoff
mismatch despite no registered transaction revision. The scorer now leaves
that result unknown without both registered axes; frozen v6 output is preserved
and the invalid booleans are not evidence. No memory behavior change is
authorized by this ambiguous screen. See the
[v6 report](../../../evals/paired/c1-agent-managed-correction-recovery-v6-valid-time-2026-11-25/capture-01/REPORT.md).

**Latest C1 result — selector diagnostics v7 (2026-11-25):** Four disjoint
synthetic histories registered the full ten-field selector allowlist. Capture
01 passed every gate, with 4/4 exact cited tasks in both same-product arms, so
the preregistered capture 02 was run. Capture 02 then failed the task-success
floor at 0/4 in both arms; it also recorded one candidate
`prohibited_evidence` violation. The repeat had six empty reads per arm across
three tasks; one incumbent read carried sufficient evidence but was not used
in the answer, while the candidate's single nonempty read lacked sufficient
evidence. Probes, correction/invalidation checks, and valid-time checks passed.
`known_at` and `compact` mismatches appeared in both the passing and failing
capture, so the boolean field diagnostics do not identify a cause. The
registered repeat was exhausted; this C1 workflow remains NO-GO, and C2 stays
ineligible. See the [cross-capture review](../../../evals/paired/c1-agent-managed-correction-recovery-v7-selector-diagnostics-2026-11-25/CROSS_CAPTURE_REVIEW.md)
and the [A10 implementation record](2026-11-24-agent-read-diagnostics-implementation-plan.md).

- [x] Run the disjoint C1 v6 valid-time diagnostic on native Apple Silicon;
  preserve the original artifacts and stop after capture 01 on the frozen
  task gate. Record valid-time matches, exact-question hashes, cross-arm
  selector fingerprint consistency, and latency. The first run exposed a
  scorer defect for unregistered transaction revisions; RED → GREEN CLI
  regression and the 14-test paired CLI target passed. See the v6 report and
  [A9 scorer contract](2026-11-24-agent-read-diagnostics-implementation-plan.md).

- [x] Freeze the corrected C1 registration and fresh disjoint families before
  capture. Keep exact fact IDs transient, persist only the evaluator's boolean
  stale-target check, and treat missing read capture as an explicit failure.
- [x] Run C1 recovery v2 capture 01 on native Apple Silicon; preserve both
  arms and every outcome. The frozen evaluator failed the task and retry-exposure
  gates, so capture 02 was not run. The separate result identifies successful
  correction writes and probes but empty agent reads; see the v2 report. A new
  registration must first fix the unreachable retry gate and add safe query
  diagnostics before any retrieval change is selected.
- [x] Add complete, privacy-preserving per-field selector diagnostics and
  replay archived artifacts unchanged; validate them at the public paired
  scorer seam. Run the fresh C1 v7 capture 01 and the one permitted repeat on
  Apple Silicon; stop after capture 02 failed its frozen task gate. The
  implementation, focused tests and review are in the [A10 plan](2026-11-24-agent-read-diagnostics-implementation-plan.md).

The separate [A11 maintenance-v7 screen](../../../evals/paired/agent-maintenance-v7-snapshot-retry-screen-2026-11-25/capture-01/REPORT.md)
then measured the corrected snapshot-retry prompt on four new synthetic
families. Retry compliance was 4/4 per arm, but agent-facing reads were empty
and exact task success was 0/4 per arm despite 4/4 independent probes. This
does not qualify C2 or isolate a deterministic memory retrieval defect; raw
queries were not retained. No follow-up capture is registered.

## Task C2. Design confirmation only for an eligible workflow

**Files.** `evals/paired/`, `crates/eval-harness/tests/paired_comparison_cli.rs`, and if a missing validator is reproduced, `src/{paired,paired_power}.rs`.

**Interfaces.** `plan-paired-power` reports power for a single registered paired binary endpoint. `compare-paired` validates a frozen confirmatory plan and emits a decision. Reports from differently scoped endpoints remain separate.

**Cycle closed NO-GO (2026-11-26):** No workflow currently qualifies for C2.
The ordinary positive control is a same-product control, while typed-conflict,
operator-import, and agent-managed-correction each missed the frozen 3/4 task
floor. Do not run a confirmatory capture or calculate power for a nonexistent
eligible treatment. Reopen C2 only after a materially changed and independently
qualified workflow passes the C1 gate on fresh families.

- [x] Close workflow eligibility as NO-GO: no treatment workflow passed the preregistered C1 task floor. No confirmatory workflow or thresholds were frozen; the ordinary v3 positive control remains process-only. Keep the historical typed-conflict 123-family result as planning evidence, not as power for an eligible treatment.
- [x] Keep the existing `cargo test -p eval-harness --locked --test paired_comparison_cli plan_paired_power_cli_uses_family_discordance_from_validated_pilot_summary`; it passed. The CLI report counts two discordant independent families and returns `promotion_decision == "not_produced"`. This covers the existing family-level validation, so the conditional duplicate-family test was not added. No power/promotion claim is made without an eligible C1 workflow.
- [x] Strengthen the deterministic ordinary-resource regression so it asserts all three primary promotion checks pass while the ordinary-workflow wall-time guard fails and the final decision rejects. This closes the specific false-positive gap where the test could previously reject for an unrelated primary failure. Focused `eval-harness` test passed.
- [x] Close this cycle without a treatment power estimate or `compare-paired` capture, as required by the failed eligibility gate. Preserve all C1 outcomes and make no promotion or memory-treatment claim.

**Reopen requirements:** On a newly qualified workflow and fresh disjoint families,
freeze the minimum useful effect, ordinary non-inferiority margin, primary and
resource guards, joint power target, and hard sample/time/token ceilings before
capture. Use `plan-paired-power` only for the endpoint it sizes. Independently
check joint power for the actual paired family-level decision, with deterministic
cases for disjoint families, no variation, and primary wins accompanied by an
ordinary/resource regression; the latter must never count as joint success.
If calibration or joint power is insufficient, do not promote. Only then run
`compare-paired`; preserve and replay its report, where exit 0 promotes, 1
rejects, 3 is insufficient evidence, and 2 is invalid input.

## Task C3. Verify the native gate and add fresh histories

**Files.** `crates/eval-harness/tests/deterministic_quality_gate.rs`, `evals/{corpora,profiles}/`, `docs/evals/DETERMINISTIC_MEMORY_QUALITY.md`.

**Interfaces.** Use the public binary through the existing `MEMORY_DECISIONS_SERVER_BIN` and `MEMORY_QUALITY_GATE_OUTPUT` environment contract. No new target matrix or platform adapter is needed.

- [x] Capture a native baseline in a fresh directory. Expect a nonzero number of executed real-binary tests and the acceptance artifact from the existing frozen v9 gate. Run from the repository root on Apple Silicon.

```sh
rustc -vV
cargo build -p memory_mcp --bin memory_mcp --features streamable-http --locked
MEMORY_DECISIONS_SERVER_BIN="$PWD/target/debug/memory_mcp" \
MEMORY_QUALITY_GATE_OUTPUT="$(mktemp -d)/native-quality" \
cargo test -p eval-harness --locked --test deterministic_quality_gate -- --ignored
```

- [x] Require `host: aarch64-apple-darwin` and a native arm64 product binary. Use the corresponding absolute binary path if Cargo uses a custom target directory. Save the toolchain identity, exact commands, seed/profile identity and `claim-rollout-default-acceptance.json`. A skipped test is missing evidence.
- [x] Freeze fresh histories covering overlapping measurements, independent-source conflict, numeric lexical neighbors and prior-knowledge cutoff. The v3 baseline failed the overlapping-measurement decision in both captures; after the clause-local retrieval fix, the v6 candidate passed all four cases in both captures with identical normalized decisions. The [v6 report](../../../evals/paired/c3-history-characterization-2026-11-25-v6/report.md) records the exact oracle and result. Multiple corrections remain covered by the [existing three-capture fixture](../../../evals/paired/multiple-corrections-2026-11-24/report.md); the v6 overlap case deliberately has no correction marker and is not evidence for an explicit correction workflow.
- [x] Reuse and capture the B5 historical-scale profile: 252 events, including a target among 250 lexical distractors, plus ordinary and absent controls. Two fresh captures passed 3/3 and the decision-replay comparator matched. See the [scale registration](../../../evals/paired/c3-historical-scale-2026-11-25/registration.json) and [comparison receipt](../../../evals/paired/c3-historical-scale-2026-11-25/replay-comparison.json).
- [x] Measure per-query latency separately from decision correctness and record returned items, budget and compact JSON-RPC bytes. The v6 and historical-scale reports give per-cohort min/median/max and case timings; native Apple Silicon reproduction commands and hashes are frozen in both registrations.
- [x] Before shipping the cross-crate evaluator measurement change, run the focused fixture regression, the retrieval test filter, the workspace-required strict Clippy command from `AGENTS.md`, `cargo fmt --all --check`, and `git diff --check`. All passed on native `aarch64-apple-darwin`.

The native v9 gate passed with four matching-stage captures. See the
[v9 rollout report](../../../evals/paired/native-rollout-v9-2026-11-24/report.md).
The first fresh C3 family is checked in with an incumbent and two
matching candidate captures. See the
[multiple-correction report](../../../evals/paired/multiple-corrections-2026-11-24/report.md).
Its snapshot precedes both corrections and all three captures passed.

The focused paired CLI, chat-completions runner and public response-size
targets reran on native Apple Silicon with 39 passing tests and one
opt-in skip. The later C3 closeout below adds the fresh deterministic
historical and scale controls. Real-provider workflow qualification remains a
separate C1/C2 gate and does not inherit C3 eligibility.

**Evaluator review follow-up (2026-11-25):** The decision normalizer used to
map an empty successful partial read to `unknown` only when `guidance` included
one exact phrase; otherwise the same response was scored `unobservable`. RED →
GREEN tests now distinguish a valid empty result (`unknown`) from no captured
read response (`unobservable`) and preserve malformed responses as errors.
The 11 normalization tests, 18 stdio/replay tests, `cargo fmt --all --check`,
and the required strict workspace Clippy command passed on native
`aarch64-apple-darwin`. A fresh ignored public v9 gate subsequently completed
with four matching-stage captures. Its frozen acceptance report records default
5/8 decisions and 8/8 evidence, shadow 1/8 decisions and 8/8 evidence, no
decision losses or evidence regressions, and byte-identical repeat captures.
This is finite rollout-gate evidence for the registered configurations, not a
memory-treatment or agent-utility claim. See the [acceptance artifact](../../../evals/paired/native-quality-candidate-2026-11-25/claim-rollout-default-acceptance.json).

**C3 closeout (2026-11-25):** The frozen four-case v6 candidate passed 4/4 in
both captures and the replay comparator matched. The 252-event historical
scale control passed 3/3 in two fresh captures with matching decisions. The
reports record read-time distributions and output budgets independently from
quality scores. The query-boundary change is supported for these finite
fixtures; its heuristic clause parser is not a multilingual or arbitrary
punctuation guarantee. See the [v6 report](../../../evals/paired/c3-history-characterization-2026-11-25-v6/report.md)
and [scale registration](../../../evals/paired/c3-historical-scale-2026-11-25/registration.json).

## Task C4. Qualify official LongMemEval-V2 in its own slice

**Files.** Use the [official spec and acceptance sequence](../specs/2026-11-09-longmemeval-v2-official-run-design.md#acceptance-sequence). The current smoke wrapper cannot run the scored cohort.

- [x] Repin upstream and data hashes, verify actual harness paths and freeze the text cohort with a per-case modality manifest before inspecting model outcomes. Capture longest-input and private-metadata preflights. Include a deliberate unmapped modality and assert visible exclusion under the preregistered rule.
- [x] At the upstream adapter boundary, use separate RED → GREEN tests for same-trajectory retry/restart, restored state or equivalent reset, reversed query order, and failed shared-haystack build. For the last case register two question IDs for one failing haystack and assert both are affected, neither can appear as a successful empty-context read, and the native score remains unavailable when the official harness aborts.
- [x] Run `python3 -m unittest discover -s evals/longmemeval_v2` and the documented pinned smoke before invoking external providers. Freeze reader/judge/model identities, storage/reset/clock, environment allowlist, cohort, runtime and cost cap per spec. Run the scored cohort only with the required dataset/egress authorization.
- [x] Publish the unmodified official score when emitted, separate full-selected-cohort failure accounting, exact denominators, timings and case-level outcomes. Require complete coverage before a baseline claim. Any retrieval fix discovered here gets a reproducible native B5 regression before promotion.

Retention, consolidation, feedback and index experiments stay trigger-based. Record a measured failure class, the candidate mechanism, a distinct deterministic comparator and a resource ceiling before opening an implementation plan.

C4 pins the reviewed opaque-query-context revision. All three pinned
Small input hashes were verified. The upstream save/restore smoke passed
after moving the original store. The adapter now uses stable input identity,
a dedicated local CLI store and bounded stdin. A two-question upstream
build aborted on extraction failure. The build preflight receipt lists both
dependent question IDs and records zero scored cases and unavailable official
metrics. The harness produced no prompt rows or score.
The real 7,600,069-byte trajectory passed ingest; extraction refused it
under the pinned local Anno input limit. The adapter offers
`ordered-segments-v1`, which preserves all ordered bytes and forms bounded
episodes through public ingest and extract. The native synthetic probe
retrieved evidence beyond the old cutoff from the restored store. The pinned
upstream build saved the real largest trajectory in eight segments in 198.4 s
on Apple Silicon with the debug binary. The frozen manifest holds seven
text-verified questions supported by three closed menus or toolbars. The
staging receipt accounts for all 451 questions and preserves all 100
ordered web trajectories. The scorer records upstream reader and judge
failures beside the native aggregate. Local MLX-LM runs both models via
separate loopback endpoints. Native synthetic probes verify restart/retry
readback and forward/reverse query order from independent copies of the
same store. The complete selected cohort and same-snapshot repeat each
scored 7/7 cases without stage failures and measured accuracy of 0/7.
The final reader prompts and case scores matched. Raw context sizes varied
on six cases and every memory item was discarded by the upstream token cap
in both runs. The bounded-read and stability findings are recorded for B5.
[Baseline report](../../evals/LONGMEMEVAL_V2_SMALL_WEB_BASELINE.md)

The post-B5 Apple Silicon recheck reused the same frozen seven cases,
100-trajectory order, saved snapshot, release binary, and model files, all
verified against the recorded profile. The valid run scored 7/7 cases with
official exact match 1/7. Its contexts match the prior B5 run-02 for all
cases; the complete reviewed source span is present in 1/7, including one
case whose answer remains wrong. The only exact-match pass lacks that reviewed
span and is unsupported in the response. Across three valid B5 runs the
official score and source-span count are stable at 1/7; average memory-query
latency is 678.2 s versus 678.5 s across the two baseline runs, so neither
answer-quality nor latency improvement is established. The first 2026-11-25
attempt is retained but excluded: it stopped before inference because the
pinned context processor was not found in the default cache. A processor
preflight and explicit offline cache fixed that harness failure before the
valid retry. See the [recheck report](../../../evals/longmemeval_v2/results/2026-11-25-b5-recheck/RECHECK.md)
and [execution manifest](../../../evals/longmemeval_v2/results/2026-11-25-b5-recheck/execution-manifest.json).

The post-C3 report now records a matched concise-prompt replay of the same
seven frozen cases, 100 ordered trajectories, saved snapshot, reader/judge
profiles and local MLX-LM runtime with the C3 release binary. This is a paired
diagnostic, **not an official full LongMemEval-V2 result**. The repeated
post-C3 arm scored 3/7 under the pinned scorer versus 1/7 for its concise B5
replay; strict phrase-set accuracy on list-valued cases was 2/4 versus 0/4,
and the full reviewed-span check was 2/7 versus 1/7. The `0c6b0c60` scorer pass
is unsupported (`Past hour` was absent from context); `d6da3938` passed exact
phrase scoring with adjacent evidence that the strict whole-span check does
not count. The repeated context payloads match the first C3 run for 7/7 cases.
The earlier C3 scoring attempt with two empty judge responses remains recorded
as a failed diagnostic and is excluded from the paired result. Mean query time
was 590.5 s versus 630.4 s in the concise B5 replay, with median 429.7 s versus
437.7 s; these seven-case descriptive differences do not establish a stable
speedup. Do not claim a general memory-quality gain from this sample. The exact
source-span scorer is reproducible without a model/provider. See the
[post-C3 report](../../../evals/longmemeval_v2/results/2026-11-25-c3-final-query-fix/RECHECK.md),
[execution manifest](../../../evals/longmemeval_v2/results/2026-11-25-c3-final-query-fix/execution-manifest.json),
and [repeated reviewed-span coverage receipt](../../../evals/longmemeval_v2/results/2026-11-25-c3-final-query-fix/run-02/reviewed-span-coverage.json).

The post-B5 current-master recheck repeated all seven frozen cases on native
Apple Silicon without LM Studio. The original scorer stopped at the third case
after an empty judge response, so that run has no complete official aggregate.
The paired replay on saved B5/current contexts scored 3/7 versus 1/7, strict
phrase-set equality 2/4 versus 0/4, full reviewed-span coverage 2/7 versus 1/7,
and mean query latency 588.0 s versus 630.4 s. These are finite-cohort
diagnostics, not guarantees: one current capture is too small for population
claims, the judge metric is model-backed, and one exact-match pass is
unsupported. A RED → GREEN harness regression now captures reader outputs
incrementally so a later judge failure cannot discard completed answers. The
[current-master recheck](../../../evals/longmemeval_v2/results/2026-11-26-current-master-repeat/RECHECK.md)
records the paired cases and all limits.

The ordinary agent-control gate was replayed offline and remained eligible for
its exact configuration: two captures each passed task, direct-read, and
independent-probe criteria 4/4 with no violations or resource exits. The
offline qualification report replayed byte-identically. The fresh C1 v7
selector-diagnostic workflow passed capture 01 but failed its one allowed
repeat at 0/4 exact cited tasks in both arms. Its cross-capture report records
the per-field selectors, valid-time results, evidence coverage, failures and
latency. Neither screen qualifies C2 or demonstrates a memory treatment
effect.

The 2026-11-26 offline replay independently recomputed the ordinary v3
qualification and both C1 v7 captures without a model, Codex process, provider
or memory server. The ordinary qualification output, both `compare-agent-managed`
outputs and both `evaluate_c1.py` outputs were byte-identical to their archived
reports. The bounded-recovery validator passed on both captures with zero
contract violations; capture 02 recorded three authorized retries per arm.
Capture 01 remains PASS and capture 02 remains NO-GO at 0/4 exact cited tasks
per arm, with one candidate prohibited-evidence violation. The frozen stop
rule is exhausted; no third capture was run. Replay artifacts are in the
[offline qualification replay](../../../evals/paired/offline-qualification-replay-2026-11-26/REPLAY.md).

## Task D1. Bound and reconstruct public explain citations

**Files.** `crates/memory-mcp/src/tools/explain.rs`, `src/memory/api.rs` or its owning explain module, `crates/eval-harness/src/suites/response_size.rs`, `tests/response_size_public_tool.rs` under the harness. Use the [bounded explain spec](../specs/2026-11-13-explain-bounded-citation-output-design.md) and ratify the compatibility decision in a newly allocated ADR before changing public continuation parameters.

**Interfaces.** `async fn explain<T: ToolContext>(ctx: &T, params: ExplainParams) -> Result<ToolResponse<Value>, MemoryError>`. The current response-size suite serializes capability items. The plan measures the returned `ToolResponse<Value>` using the public tool twice, once per mode.

- [x] Write `explain_public_envelope_fits_32768_bytes_in_each_mode`, using source text with repeated quotes, backslashes and multibyte scalars. Assert `serde_json::to_vec(&response)?.len() <= 32_768` separately for compact and verbose and verify a supported small source retains its exact quote. Run `cargo test -p eval-harness --locked --test response_size_public_tool explain_public_envelope_fits_32768_bytes_in_each_mode`. RED is a real public-envelope overrun or loss of fidelity.
- [x] Implement the final-envelope budgeting at the adapter and domain source/window semantics in the owning capability. Keep the default excerpt target of 1,200 scalars and explicit read limit/default of 8,000 scalars. Take remaining field names, offsets and exact rejected inputs from the spec.
- [x] Write a separate `explain_source_continuations_reconstruct_original_unicode` case on a source with combining marks and emoji. Assert concatenated windows equal the original string, scalar offsets advance and item/source/provenance continuations stay distinct. Run the same target filter before and after minimal implementation. Add the synthetic reconstruction case while preserving the 61 shared response-size cases.
- [x] Run `cargo run -p eval-harness --locked --bin memory-eval -- run --profile evals/profiles/response_size.json --artifact <fresh.json>`. Verify both modes, partial metadata, exact reconstruction and compatibility on the native target. Commit as an independent product change.

## Task D2. Measure extraction on the deployed revision

- [x] Capture the currently available production aggregates read-only and retain the exact query window and denominators in the [2026-11-25 observation](../../performance/extract-quality-production-observation-2026-11-25.md). It records two successful extracts in five days, a 1.295 s mean, 28 facts, 54 entities, 54 links and zero warnings; only the `embedding_provider` stage series was present. This is a low-volume operational snapshot, not a before/after or semantic-quality result.
- [x] Audit claim metric semantics against call sites and a fixed 22-day production aggregate. The receipt records 229 project events with `outcome="other"`, one reconciliation page duration with `outcome="other"`, and one candidate observation. Source inspection found known lifecycle statuses being collapsed by the label mapper and successful projection/backfill duplicating the outcome in `reason_code`. It also found the backfill counter called a successful idempotent job ensure `completed`, although projection may not have completed and repeats may count the same fact. RED → GREEN tests now require distinct bounded statuses, `reason_code="none"` for success, `ensured` for successful job ensure, truthful HELP text, and production exposition coverage. Critical review then aligned the recording rule and dashboard panel with job-ensure attempts for identified facts, not worker throughput. The read-only receipt and limits are in the [production observation](../../performance/extract-quality-production-observation-2026-11-25.md). Historical series cannot be reclassified; the fix is local and has not been deployed.
- [ ] Establish a qualified production comparison against the fixed [extract baseline](../specs/2026-11-02-extract-quality-and-cost.md): attest the deployed image/source digest and non-sensitive configuration fingerprint, use comparable revision/configuration windows with adequate eligible-operation denominators, and measure per-stage query/latency cost. Production reported 1.41.0, a tag descending from local fix `3baafb94`, but its metric label does not attest the deployed artifact/configuration; the Nov 2 window and current Nov 25 snapshot are also not matched and contain 9 versus 2 calls. Aggregate counters cannot label false relations or eligible entity misses; those quality gates must use the fixed source-grounded deterministic eval corpus. Any population claim about production semantic quality also needs an approved reviewed sample.
- [x] Preserve the fresh-write side of the reproduced dated-record misattribution with a fixed-time `ingest → extract → assemble_context` integration case. It first requires each extraction to return evidence containing its independent expected date and duration, then asserts zero contradiction relations and exact direct source coverage for both records. The targeted test passed; a negative control that removes the first extraction's returned facts fails at the new assertion. This does not hide or repair the separately reproduced stale derived rows in the restored legacy snapshot.
- [ ] Obtain an approved read-only inventory for any affected historical rows. Follow the append-only B6 recovery design only if a repair is separately approved; preserve the existing append-only history and source facts.

The current checkout now contains a dated production aggregate snapshot, but
no qualified matched comparison or approved historical-record inventory. No
deployed semantic-quality gain or historical repair is claimed.

## Task D3. Re-establish the latency baseline

- [x] Follow the [performance design](../specs/2026-11-14-assemble-context-latency-design.md) using SDK 4.0.1 on a disposable profile. Reproduce normalized row digests, ordering/cutoff correctness and query count. Use the spec's twenty alternating AB/BA pairs and median candidate/incumbent target at most 0.50 for the eligible costly empty case, with no material positive-case regression. Keep `memory-512m` unavailable until it passes its own preflight.
- [x] On a proven bottleneck and accepted candidate, write the public result/policy regression, run RED, make the smallest owner change and repeat the same performance comparison. Preserve correctness and resource evidence even when the candidate misses the threshold. Commit only the eligible change and record the tested environment.

The production-path comparison measured 207.1–207.7 ms before the
guard and 0.43–0.63 ms after it for populated no-match. Positive-case
median differences changed sign by order and stayed within 1.7%.
Embedded and disposable-server checks passed, and retrieval quality
passed 37/37. [Current evidence](../../performance/ASSEMBLE_CONTEXT_LATENCY.md)
