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
- [ ] Freeze and evaluate the remaining typed-contradiction,
  agent-managed-correction and operator-import populations on fresh synthetic
  families. The ordinary workflow is now eligible for a separately frozen
  study, but that does not transfer eligibility to these distinct workflows.
  Keep the previous 0/4 response-format screen and v2 budget exits in their
  original denominators and do not reuse exposed families as holdouts.

## Task C2. Design confirmation only for an eligible workflow

**Files.** `evals/paired/`, `crates/eval-harness/tests/paired_comparison_cli.rs`, and if a missing validator is reproduced, `src/{paired,paired_power}.rs`.

**Interfaces.** `plan-paired-power` reports power for a single registered paired binary endpoint. `compare-paired` validates a frozen confirmatory plan and emits a decision. Reports from differently scoped endpoints remain separate.

- [ ] Freeze the eligible workflow, new disjoint families, minimum useful effect, ordinary non-inferiority margin, primary and resource guards, joint power target and hard sample/time/token ceilings. Only use C1 data for design calibration. Preserve the old typed-conflict 123-family calculation as planning evidence.
- [ ] Keep the existing `cargo test -p eval-harness --locked --test paired_comparison_cli plan_paired_power_cli_uses_family_discordance_from_validated_pilot_summary`. If family-count coverage is missing, write `pilot_retests_do_not_inflate_independent_family_count`, two captures of four identical families, and assert `independent_family_count == 4` and `promotion_decision == "not_produced"` using the actual report's field paths. Run it before modifying counting logic.
- [ ] Use `plan-paired-power --pilot-report <summary> --stratum-id <stratum> --target-risk-difference <effect> --minimum-point-effect <margin> --one-sided-alpha <alpha> --target-power <power> --confidence-level <confidence> --maximum-sample-size <cap> --output <fresh-report>` only for the endpoint it sizes. For a joint claim, freeze an independently checked power simulation of the actual paired family-level decision including ordinary and resource guards. Add deterministic cases for disjoint families, no variation, and a primary win with an ordinary/resource regression. Confirm that the latter never counts as joint success. If calibration or joint power is insufficient, leave promotion open.
- [ ] Only after the whole gate is eligible, complete capture under the fixed cap and run `compare-paired --plan <confirmatory-plan> --incumbent <i> --candidate <c> --output <fresh-report>`. Exit 0 promotes, 1 rejects, 3 is insufficient evidence and 2 is invalid input. Preserve all decision outcomes; replay and compare the archived output. Commit the evidence and state exactly which claim the registration supports.

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
- [ ] Freeze new families that exercise multiple corrections, contradictory sources, lexical distractors, and longer histories. Reuse the historical scale family from B5. Keep exposed v4–v10 data as replay controls. Start with one family where a public expected answer and source set are specified and assert its observed hard-violation count, disposition and sources. Run incumbent and candidate with B5's public replay commands. Only introduce a product change on a frozen RED; otherwise commit the characterization.
- [ ] Measure query time distributions separately from deterministic decisions and actual public read/item/byte limits. Record the native command as the default contributor reproduction path and commit the registrations and receipts.
- [ ] Before shipping changes across crates, run the workspace-required Clippy command from `AGENTS.md` and `cargo fmt --all --check` on the same target. During Red → Green use the named focused tests first.

The native v9 gate passed with four matching-stage captures. See the
[v9 rollout report](../../../evals/paired/native-rollout-v9-2026-11-24/report.md).
The first fresh C3 family is checked in with an incumbent and two
matching candidate captures. See the
[multiple-correction report](../../../evals/paired/multiple-corrections-2026-11-24/report.md).
Its snapshot precedes both corrections and all three captures passed.

The focused paired CLI, chat-completions runner and public response-size
targets reran on native Apple Silicon with 39 passing tests and one
opt-in skip. Broader holdout histories and live confirmation retain their
separate acceptance criteria.

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

## Task D1. Bound and reconstruct public explain citations

**Files.** `crates/memory-mcp/src/tools/explain.rs`, `src/memory/api.rs` or its owning explain module, `crates/eval-harness/src/suites/response_size.rs`, `tests/response_size_public_tool.rs` under the harness. Use the [bounded explain spec](../specs/2026-11-13-explain-bounded-citation-output-design.md) and ratify the compatibility decision in a newly allocated ADR before changing public continuation parameters.

**Interfaces.** `async fn explain<T: ToolContext>(ctx: &T, params: ExplainParams) -> Result<ToolResponse<Value>, MemoryError>`. The current response-size suite serializes capability items. The plan measures the returned `ToolResponse<Value>` using the public tool twice, once per mode.

- [x] Write `explain_public_envelope_fits_32768_bytes_in_each_mode`, using source text with repeated quotes, backslashes and multibyte scalars. Assert `serde_json::to_vec(&response)?.len() <= 32_768` separately for compact and verbose and verify a supported small source retains its exact quote. Run `cargo test -p eval-harness --locked --test response_size_public_tool explain_public_envelope_fits_32768_bytes_in_each_mode`. RED is a real public-envelope overrun or loss of fidelity.
- [x] Implement the final-envelope budgeting at the adapter and domain source/window semantics in the owning capability. Keep the default excerpt target of 1,200 scalars and explicit read limit/default of 8,000 scalars. Take remaining field names, offsets and exact rejected inputs from the spec.
- [x] Write a separate `explain_source_continuations_reconstruct_original_unicode` case on a source with combining marks and emoji. Assert concatenated windows equal the original string, scalar offsets advance and item/source/provenance continuations stay distinct. Run the same target filter before and after minimal implementation. Add the synthetic reconstruction case while preserving the 61 shared response-size cases.
- [x] Run `cargo run -p eval-harness --locked --bin memory-eval -- run --profile evals/profiles/response_size.json --artifact <fresh.json>`. Verify both modes, partial metadata, exact reconstruction and compatibility on the native target. Commit as an independent product change.

## Task D2. Measure extraction on the deployed revision

- [ ] Use aggregate measurements from comparable windows and the fixed [extract baseline](../specs/2026-11-02-extract-quality-and-cost.md). Match revision/configuration, eligible traffic and denominator. Measure false relations, eligible entity recall and per-stage query/latency cost.
- [ ] If a stored misattribution is reproduced, add its public ingest → extract → assemble-context case as a native B5 guard before any product change. For repair follow B6. Record missing evidence when equivalent windows or access are unavailable.

No matched aggregate deployment windows and affected-record inventory
were available in this checkout. The existing local extraction baseline is
the evidence ceiling. No deployed measurement or historical repair is
claimed.

## Task D3. Re-establish the latency baseline

- [x] Follow the [performance design](../specs/2026-11-14-assemble-context-latency-design.md) using SDK 4.0.1 on a disposable profile. Reproduce normalized row digests, ordering/cutoff correctness and query count. Use the spec's twenty alternating AB/BA pairs and median candidate/incumbent target at most 0.50 for the eligible costly empty case, with no material positive-case regression. Keep `memory-512m` unavailable until it passes its own preflight.
- [x] On a proven bottleneck and accepted candidate, write the public result/policy regression, run RED, make the smallest owner change and repeat the same performance comparison. Preserve correctness and resource evidence even when the candidate misses the threshold. Commit only the eligible change and record the tested environment.

The production-path comparison measured 207.1–207.7 ms before the
guard and 0.43–0.63 ms after it for populated no-match. Positive-case
median differences changed sign by order and stayed within 1.7%.
Embedded and disposable-server checks passed, and retrieval quality
passed 37/37. [Current evidence](../../performance/ASSEMBLE_CONTEXT_LATENCY.md)
