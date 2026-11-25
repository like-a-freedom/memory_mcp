# Agent Read Diagnostics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Explain why an agent failed a memory task using the actual read and the final answer, then isolate query choice from the evidence presented to the agent.

**Architecture:** Capture call-scoped evidence at the existing tool-session boundary and evaluate it against the private oracle in the paired scorer. Keep the independent probe and current task success as separate measures. Use the same tool-session boundary to replay a fixed, registered public response in an exploratory capture.

**Tech Stack:** Rust, Cargo, serde, MCP stdio, scripted loopback Chat Completions, offline JSON replay.

**Spec:** [Agent memory follow-up, Tasks 1–2](../specs/2026-11-24-agent-memory-follow-up-design.md#task-1-attribute-failures-on-the-actual-agent-read) · [Paired artifact contract](../../evals/PAIRED_AGENT_EVAL.md)

**Starting point.** `5595e0d1a8126612dd861b341bed4a8952a1a1ac`. The frozen ordinary probe passed 4/4 and the agent completed 0/4 in each pilot capture. The existing scorer consumes the union of returned source IDs, so this result alone cannot distinguish a failed read from an answer error.

## Global Constraints

- Keep the eight-tool surface. Use the public CLI and MCP seams, the owning memory/knowledge contexts, and narrow adapter dependencies. Preserve wire compatibility and historical artifact semantics.
- The required eval target is native `aarch64-apple-darwin`. The deterministic gate runs with Cargo locally, without provider credentials or a model server.
- Use the workspace's `rust-version = "1.100.0"` and locked dependencies. No new crate is needed for A1–A7.
- Legacy artifacts carry unknown observations. Keep oracle values out of prompts and adapters. Persist closed diagnostics and counts; scope opaque source references to private scorer input and never emit raw request, reply, model output or oracle text in new diagnostic fields.
- Each test is a vertical slice. Run its filter on the baseline and record the observed failure before the GREEN edit. A passing characterization becomes a guard. A compile failure or fixture setup error does not establish the behavioral RED.
- Confirm the proposed public test seams in plan review before writing test files. No approval is needed to draft this plan.

## Review Focus

- A failed call followed by a successful retry keeps both observations and can still complete the task. A2.
- A successful empty list differs from an unreadable or unobserved reply. A1 and A3.
- Swapped replies with valid event numbers still fail validation. A2.
- Sufficient source coverage at a different cutoff cannot satisfy a registered historical read. A4.
- The fixed reply reaches the model on the matched tool call, and another capture cannot silently replay it as a live response. A6.

## File Map and Proposed Seams

| Files | Responsibility and observable boundary |
| --- | --- |
| `crates/eval-harness/src/paired_agent/{chat_completions,tool_results}.rs` and `tests/chat_completions_runner.rs` | `ChatCompletionsAgent::run_phase<M: MemoryToolSession>(...) -> Result<AgentObservation, EvalError>` with scripted loopback HTTP and a controlled public MCP reply. Proves what reached the agent and what was captured. Provider intelligence is outside this seam. |
| `crates/eval-harness/src/codex_agent.rs`, `src/paired_agent/codex.rs` and `tests/codex_agent_runner.rs` | `run_codex_agent(&CodexAgentConfig, &str) -> Result<CodexAgentObservation, EvalError>` with the existing subprocess fixture. Covers JSONL observations even when the adapter cannot see a structured reply. |
| `crates/eval-harness/src/{paired_agent,paired}.rs`, `src/paired_agent/{observations,independent}.rs`, `tests/paired_comparison_cli.rs` | `RecordedPair::score`, `score-paired-observations`, and `run_with_independent_probes`. Covers private scoring, validation, and durable replay. |
| `crates/eval-harness/src/paired_agent/chat_completions_backend.rs`, `src/{cli,main}.rs`, `tests/paired_prompt_contract.rs` | Explicit diagnostic recall capture and hashed read policy. Existing managed source-authority entry point stays on its existing path. |
| `crates/memory-mcp/src/tools/assemble_context.rs`, `tests/assemble_context_wire_contract.rs` | Conditional product change at public `assemble_context`. Execute A8 only after A7 identifies a measurable presentation problem. |

In this plan paths starting with `src/` and `tests/` under a named crate are relative to that crate. All commands run from the workspace root. Use the exact test filter named in each slice.

## Task A1. Capture the result of one read

**Files.** `crates/eval-harness/src/paired_agent.rs`, `src/paired_agent/{chat_completions,tool_results}.rs`, `tests/chat_completions_runner.rs`.

**Interfaces.** Produce `AgentObservation.memory_reads: Option<Vec<MemoryReadObservation>>`. Each record has `request_sequence: u32`, `response_sequence: Option<u32>`, `outcome: MemoryReadOutcome` and `item_count: Option<u32>`. Pin serialized outcomes `failed`, `empty`, `nonempty`, `unknown`. `None` on the collection denotes legacy or unobserved capture. `Some([])` denotes a measured phase with no reads.

- [x] Write `read_error_does_not_become_empty_memory`. Script one `isError: true` reply and a subsequent valid final response. Assert `outcome == Failed`, `item_count == None` and the read and error sequences refer to `assemble_context`.

```rust
let captured = serde_json::to_value(&observation)?;
assert_eq!(captured["memory_reads"][0]["outcome"], "failed");
assert!(captured["memory_reads"][0]["item_count"].is_null());
```

- [x] Run `cargo test -p eval-harness --locked --test chat_completions_runner read_error_does_not_become_empty_memory`. The incumbent has a tool-error trace with no call-scoped read outcome. Establish that failure via serialized output before introducing the type.
- [x] Populate the record in `run_phase` from the actual MCP completion. Share the structured-reply projection with A3. Treat a missing result array on a successful transport as `unknown`, including an opaque text-only reply.
- [x] Run the same filter and verify GREEN.
- [x] Repeat this loop as `empty_read_reports_zero_items`, using `{"result":{"structuredContent":{"result":[]}}}` and asserting `Empty` with `Some(0)`. RED must show a missing outcome or a wrong distinction.
- [x] Run `cargo test -p eval-harness --locked --test chat_completions_runner`. Commit `test(eval): distinguish failed and empty agent reads` with the matching implementation.

Verified on native Apple Silicon in `1a68b03e`. Both focal cases failed on missing observations before their corresponding changes. The combined runner, managed-trial and paired CLI suite passed 46 tests, with one opt-in model smoke skipped. Versioned persistence starts in A2.

## Task A2. Validate versioned read/reply associations and replay legacy artifacts

**Files.** `crates/eval-harness/src/paired_agent/{observations,tool_results}.rs`, `src/{paired,paired_agent}.rs`, `tests/paired_comparison_cli.rs`.

**Interfaces.** Add observation `memory-mcp-agent-observations/v4`, paired artifact `memory-mcp-agent-paired/v7`, and native journal line `memory-mcp-agent-trial-receipt/v5`. Keep comparison v2 stable and keep old native captures on their existing writer if the read protocol is absent. V4 requires explicit `read_capture_status: ReadCaptureStatus` (`observed`, `partial`, `unavailable`) and a list of the observations known. V7 receipts require the same status and `read_diagnostics: Option<Vec<MemoryReadDiagnostic>>` for attempted trials. An empty list with unavailable capture asserts nothing about actual reads. Older versions omit both fields.

- [x] Extend the valid `scores_external_observations_offline_using_the_private_oracle` fixture. Write `v4_observations_reject_swapped_read_replies`. Two requests have IDs `read-1` and `read-2`; link them to one another's distinct completion events. Assert exit 2 and no output directory.
- [x] Run `cargo test -p eval-harness --locked --test paired_comparison_cli v4_observations_reject_swapped_read_replies`. Include a valid v4 control in the RED fixture so blanket rejection of a new version does not satisfy the test.
- [x] Match request and response sequence, call identity, tool, terminal kind, and increasing order. Require one observation per observed read request, at most one terminal per read, and no response shared by two requests. Use `response_sequence: None` for interrupted reads. Reject unknown free-form values and inconsistent status/counts before writing either arm. When emitting v7 trace events, map arbitrary provider call IDs to safe trace-local identifiers; keep already archived trace bytes.
- [x] Run the same filter. Add separate Red → Green cases `v4_rejects_missing_read_coverage` and `failed_read_and_successful_retry_remain_distinct`. In the retry case assert the sequence of outcomes `[Failed, Nonempty]` and a passing task if the final answer cites the successful read.
- [x] Add `v4_unavailable_capture_is_distinct_from_an_observed_no_read_phase`. In the guard, score the same no-read trace with `read_capture_status` of `unavailable` and `observed`. Assert the distinct serialized statuses with an empty diagnostics list, and verify that unsupported captures do not acquire a measured zero-rate claim in summaries.
- [x] Run the existing offline scorer fixture twice in new output directories. Assert byte-equal v6 receipts. Extend it with the v4/v7 case and fixed times to verify same-input replay and legacy field omission. Confirm archive SHA checks for the chosen legacy inputs. Run `cargo test -p eval-harness --locked --test paired_comparison_cli`. Commit `feat(eval): version agent read observations`.

A2 implementation is committed in `077251b6`. Native suite: 559 passed. CLI covers a mismatched call ID, missing coverage, unavailable capture and byte-identical fixture replay. CLI fixtures verify a successful retry after a failed read and reject swapped terminal sequences. Replaying archived typed-conflict runs 02 and 03 reproduced the frozen pilot summary SHA-256 `e59bd283ce544ddf91b3385176a91c3eb29fc910d0ae27222a451a216aafbba0`.

## Task A3. Bring Codex and unreadable replies to the same contract

**Files.** `crates/eval-harness/src/codex_agent.rs`, `src/paired_agent/{codex,tool_results}.rs`, `tests/{codex_agent_runner,chat_completions_runner}.rs`.

**Interfaces.** Reuse A1 and A2. `observed` accounts for all read requests, including failed ones. A read with insufficient wire fields remains unknown for those fields.

- [x] Write `text_only_tool_reply_has_unknown_read_outcome`. Use the existing scripted public runner. Return the literal `UNRECOGNIZED_REPLY_SENTINEL` in text-only MCP content. Assert `Unknown`, no count, and that JSON diagnostic output omits the sentinel. Run the test and record RED.
- [x] Normalize the structured tool response once in `tool_results.rs`. Decode Chat Completions and Codex outer envelopes at their respective adapters. A Codex completion without a trustworthy start or reply marks capture partial rather than inventing a matched read. The scoring context can still consume fully matched earlier reads.
- [x] Run `cargo test -p eval-harness --locked --test codex_agent_runner text_only_tool_reply_has_unknown_read_outcome`. Add the same outcome assertion through the loopback adapter and test the structured one-item case (`Nonempty`, `Some(1)`).
- [x] Run both adapter targets. Commit `fix(eval): preserve unknown agent read observations`.

A3 is committed in `7e18963e`. The text-only subprocess fixture failed before the change and then passed. The native Codex fixture suite passed 10 tests and the loopback suite passed 24. Both adapters share the structured reply projection.

## Task A4. Score actual reads with the private oracle

**Files.** `crates/eval-harness/src/paired_agent.rs`, `src/independent_probe.rs`, `src/paired_agent/{tool_results,observations,independent}.rs`, `src/paired.rs`, `tests/paired_comparison_cli.rs`.

**Interfaces.** Extend each private `HiddenTaskOracle` with `read_expectation: Option<ReadExpectation>` and omit it when absent. `ReadExpectation` contains `valid_at: Option<DateTime<Utc>>` and `required_claim_annotation_alternatives: Vec<Vec<ProbeClaimAnnotation>>`. Use the existing `PreparedPairedTask.expected_known_revision` for the transaction-time checkpoint. Validate expectations against registered required source events and include them in the oracle fingerprint only. Task-manifest v2 carries this required opt-in contract for diagnosed tasks; runner input projection remains unchanged.

Add an optional private `MemoryReadEvidence` to each `MemoryReadObservation`, with `source_episode_ids: Vec<String>`, `claim_annotations: Vec<ProbeClaimAnnotation>` referencing those same opaque IDs until scoring, `completeness`, `requested_cutoff_sha256: Option<String>` and `returned_cutoff_sha256: Option<String>`. Treat unavailable capture as absent evidence and a structured empty result as observed evidence with an empty source set. Validate count and membership before private event mapping. Canonical cutoff bytes are compact JSON `[valid_at_in_utc_rfc3339, known_revision]`, so offset-equivalent instants match.

Produced per-read diagnostics include `outcome`, `requested_cutoff_match`, `returned_cutoff_match`, `source_coverage`, `annotation_match`, `completeness`, and `cited_returned_source_count`. Use `Option<bool>` for checks that may be unobservable or unregistered, and the product's `complete`/`partial`/`unknown` vocabulary for completeness. Source IDs and annotation tuples in private scorer input are bound to the matched reply. Reuse the existing status/outcome/role vocabulary. For cutoff matching use the normalized UTC instant and revision from the request and returned snapshot. In normalized external input carry canonical pair digests for the request and response rather than raw query or snapshot text. Compare each against the checkpoint and valid time from private registration. Missing echo is unknown. A positively different echo is mismatch.

- [x] Verify source coverage, missing claim status, and task acceptance independently through the public CLI regression `scores_external_observations_offline_using_the_private_oracle`. It asserts `source_coverage == true`, `annotation_match == false`, and preserves task success; the separate `cli_blocks_confirmation_when_ordinary_workflow_fails_despite_passing_probes` regression proves that 4/4 independent probes cannot compensate for failed ordinary tasks. Reuse these assertions instead of adding duplicate large fixtures.
- [x] Run `cargo test -p eval-harness --locked --test paired_comparison_cli scores_external_observations_offline_using_the_private_oracle`; it passed on native Apple Silicon on 2026-11-25.
- [x] Implement and verify `score_trial` with the private source-group and annotation oracle. Persist only normalized outcomes; read diagnostics remain separate from task and independent-probe scores.
- [x] Cover the table cases in the existing public CLI scorer regression: wrong returned revision with the correct source, UTC-equivalent registered time, wrong final answer, missing citation, and empty partial read. The output is replayed byte-identically; private IDs, answers, annotations, and cutoff digests are absent from published receipts.

| Case | Fixture and assertion |
| --- | --- |
| `registered_cutoff_cannot_be_satisfied_by_a_different_snapshot` | Requested revision 7 and valid time `2026-05-01T10:00:00Z`; response echoes revision 8 and still returns the required event. Assert `source_coverage == Some(true)`, `requested_cutoff_match == Some(true)` and `returned_cutoff_match == Some(false)`. In a positive control an equivalent `+01:00` timestamp matches the same UTC instant. |
| `adequate_read_preserves_wrong_answer_diagnostic` | Required source, correct typed evidence, valid citation, answer `Cabinet-99` against registered `Cabinet-28`. Assert source/annotation/citation checks pass, `accepted_answer == Some(false)`, task fails. |
| `adequate_read_preserves_missing_citation_diagnostic` | Same adequate read and correct answer with empty citation list. Assert `required_evidence_cited == Some(false)` with zero cited returned sources, task fails. |
| `partial_empty_read_does_not_indicate_global_absence` | Return an empty array with `partial` historical context. Assert `Empty` plus `Partial`; absent completeness metadata yields unknown. |

- [x] Run the focused public CLI and adapter regressions. The native CLI replay produces byte-identical receipts for equivalent observations; `scores_external_observations_offline_using_the_private_oracle` passed on 2026-11-25. Implementation is present in the current workspace; no commit is claimed here.

## Task A5. Prove diagnostic redaction and oracle isolation

**Files.** `crates/eval-harness/tests/{paired_prompt_contract,paired_comparison_cli}.rs`, `src/paired_agent/{observations,prompts}.rs`, `src/paired_agent.rs`.

**Interfaces.** Consume A4. Test `export-paired-inputs` and `score-paired-observations`. Legacy `final_answer` remains private scoring input under the existing contract; it never appears in published receipts. New diagnostics never copy raw final text, request parameters, MCP replies or oracle values.

- [x] Verify the public runner projection excludes the private oracle and its read expectation in `scores_external_observations_offline_using_the_private_oracle`; the fixture asserts the annotation sentinel and `read_expectation` are absent.
- [x] Verify published receipts omit private source IDs, answer text, annotations, and cutoff digests while scoring still uses the private answer. Native journal tests also assert query and guidance text are not serialized. No raw argument or reply fields were added.
- [x] Update the public contract in [PAIRED_AGENT_EVAL.md](../../evals/PAIRED_AGENT_EVAL.md). The existing regression and journal tests cover the current redaction contract; no duplicate test-only fixture was added.

## Task A6. Replay one registered public reply in a diagnostic capture

**Files.** `crates/eval-harness/src/paired_agent/chat_completions_backend.rs`, `src/paired_agent/chat_completions.rs`, `src/{cli,main}.rs`, `tests/{chat_completions_runner,paired_comparison_cli}.rs`.

**Interfaces.** Introduce `RecallReadMode::{Live, FixedReply}` with a diagnostic-only `RecallMemorySession<M: MemoryToolSession>` implementing the existing trait. First add the adapter through `run_phase`, then expose explicit `run-paired-chat-completions --recall-read-mode live|fixed-reply` for a recall task manifest. The existing CLI without this flag still consumes its managed-task manifest. Use `--fixed-read-inputs <path>` only with `fixed-reply`. Registration keys are `CaseTrialKey` plus arm; each value pins canonical public `assemble_context` arguments and its expected normalized response shape. The payload delivered to the agent is the exact response to that fixed call in the trial's seeded session. Bind mode and registration hash to `SharedRunIdentity.reader_config_sha256`; compare runs of different modes as diagnostic controls, outside the promotion comparator.

- [x] Verify `fixed_reply_is_delivered_to_the_original_tool_call`: the scripted model call ID is preserved, the registered public reply reaches the model, and the live tool schema remains unchanged.
- [x] Verify registration and mode enforcement with `recall_capture_rejects_unregistered_read_mode`; unregistered modes fail before opening the session.
- [x] Keep fixed-reply mode bound to the registered trial/arm and response shape, with one public read and unchanged prompts, history, budgets, and oracle projection. The mode and registration hash are included in run identity.
- [x] Run `cargo test -p eval-harness --locked --test chat_completions_runner fixed_reply_is_delivered_to_the_original_tool_call` and `cargo test -p eval-harness --locked --test paired_comparison_cli recall_capture_rejects_unregistered_read_mode`; both passed on native Apple Silicon on 2026-11-25.

## Task A7. Run the bounded diagnostic screen

- [x] Register fresh cases of ordinary recall, typed contradiction and partial history. Freeze model/adapter configuration, family IDs, history, correct answer, required source, snapshot, byte/call budgets and runtime cap before running. Use one identical configuration for the two read modes, with synthetically seeded product state. Preserve the old pilot as NO-GO.
- [x] Run both registered modes through A6, with identical budgets and scoped read-only tools. Record counts by source coverage, cutoff, annotation, final answer and citation, then classify outcomes. If fixed reply succeeds and live reply misses, investigate the query and public guidance. If both fail on adequate evidence, run a compact presentation experiment. If neither arm has adequate replies or the agent never reads, fix the diagnostic reproduction before changing product behavior.
- [x] Save receipts and the decision in the paired eval guide. Advance only the corresponding product slice. The scripted A1–A6 gate has no dependency on this live screen.

A4–A6 shipped through the closed-oracle CLI scorer, normalized diagnostics
and registered fixed-reply controls. The archived case names and scope are
recorded in the [paired eval guide](../../evals/PAIRED_AGENT_EVAL.md).
A7 ran the local LFM and Qwen screens and reached NO-GO. Fixed reply
recovered the historical source and cutoff on LFM; neither local model
produced a usable answer under the registered historical controls.
[Receipts and qualified interpretation](../../../evals/paired/recall-read-local-screen-2026-11-24/report.md)

## Task A8. Test an evaluator-only evidence-action hint

**Current disposition (2026-11-25): no public product change is supported.** A7 recovered the registered source/snapshot with fixed reply in the LFM screen, but neither local model produced a passing final answer; Qwen did not provide a valid paired comparison. A response hint cannot know semantic answerability from retrieval alone, so the experiment must not expose `answer`, `clarify`, or `insufficient` as if they were answer-quality judgments.

The controlled candidate is an evaluator-only projection added after the public MCP reply is validated. It is derived solely from returned public fields and carries one of three bounded actions: `answer_with_sources` when non-conflicting items were returned, `clarify_conflict` when a returned item exposes contradiction or temporal ambiguity, and `verify_or_ask` when no usable items were returned. The hint must preserve source IDs, instruct the agent to answer only when the returned facts support the task, and never equate an empty or partial result with global absence. The raw public result, prompts, read arguments, model, budgets and safety oracle remain matched. The hint is not added to the product response unless a separate, successful promotion study supports it.

**Files.** `crates/eval-harness/src/paired_agent/{chat_completions,chat_completions_backend}.rs`, `tests/chat_completions_runner.rs`, `tests/paired_comparison_cli.rs`, `evals/paired/`, and the evaluation guide. No product wire change is part of this task.

- [x] Add a registered `current` versus `evidence_action_hint` projection to fixed-read diagnostics. Bind it to the reader fingerprint; default `current` preserves historical registration bytes. Public runner tests cover supported evidence, typed conflict, unknown/missing status, empty partial evidence, opaque replies, and unchanged source/call association.
- [x] Freeze a fresh all-synthetic cohort, identical agent prompts/model/budgets and registered read arguments, outcome/resource gates, stopping rule, static public-MCP preflight, and local native Apple Silicon command before any model request. The run used the direct local MLX endpoint and no remote data.
- [x] Run matched current/hint captures. Compare exact task answer, source citation, clarification, prohibited evidence and resource use. Keep task families as the sampling unit and preserve every failed run. The screen returned 2/3 exact tasks per arm, with zero hard violations; no candidate-only task success occurred. The absent-evidence clarification matched only in the hint arm, but its answer still failed acceptance. Median latency was 15.490s current versus 15.941s candidate; one incumbent invalid response accounts for the apparent mean difference. No disjoint repeat or product promotion was authorized by the stop rule.
- [x] Record the result and decision in the [paired evaluation guide](../../evals/PAIRED_AGENT_EVAL.md#registered-recall-controls) and [A8 report](../../../evals/paired/a8-evidence-action-hint-2026-11-25/README.md). The candidate was declined for promotion; product wire-contract tests and compatibility review were therefore not entered.

**A8 result (2026-11-25).** One native Apple Silicon matched capture on three
synthetic families produced 2/3 exact task successes in both arms. Expected
source coverage was 3/3 in each arm. The candidate improved clarification
matching on the empty case, but its final answer still contained unacceptable
extra text. The incumbent's corresponding run ended with `invalid_response`.
No hard violations occurred. Candidate median trial latency was 451ms slower;
the lower mean is driven by the mismatched failure outcomes and is not a
latency improvement. The typed status needed to label the conflict was absent
from the public replies, so the candidate safely used `verify_or_ask`. This
screen supports neither a product change nor an end-to-end benefit claim.

## Task A9. Separate valid-time selection from transaction-revision matching

**Purpose.** C1 recovery v5 showed empty partial reads, but its query and
non-query hashes could not distinguish a missed `valid_at` from a missed
query. The next diagnostic must attribute the valid-time axis before changing
retrieval or response behavior.

**Files.** `crates/eval-harness/src/paired_agent/{tool_results,observations}.rs`,
`src/{paired_agent,codex_agent}.rs`, `src/paired.rs`, and
`tests/paired_comparison_cli.rs`.

- [x] Capture only SHA-256 of normalized requested `valid_at` (or legacy
  `as_of`) and returned `context.snapshot.valid_at`. Normalize RFC3339 offsets
  to UTC. Never serialize raw timestamps.
- [x] Compare each fingerprint independently with the private registered
  `read_expectation.valid_at`. Preserve the existing full cutoff comparison
  for tasks that pin both valid time and `known_revision`; unknown echoes stay
  unknown.
- [x] Add observation v5, paired receipt v9 and native journal v7 with strict
  compatibility checks. Preserve v4 observations and v7/v8 paired artifact
  replay without mutation.
- [x] RED → GREEN through the public `score-paired-observations` CLI: UTC
  offset equivalence, matching and mismatching request/returned times,
  timestamp redaction, and byte-identical replay. Run the paired comparison
  CLI test target and helper unit tests on native Apple Silicon.
- [x] Register a fresh, disjoint synthetic C1 cohort with private expected
  `valid_at`; run locally on native Apple Silicon and report whether empty
  reads are explained by request-time mismatch, returned snapshot mismatch,
  or an unresolved query/selector issue. Do not change retrieval on an
  ambiguous result. V6 completed on 2026-11-25: both requested and returned
  times matched 8/8 per arm, but reads stayed empty/partial 8/8 and task
  success was 0/4. Query fingerprints exactly matched neither registered
  question; first-read non-query fingerprints matched across arms in 2/4
  families. This does not isolate semantic query quality or the differing
  selector. Review also exposed a scorer bug that compared a full cutoff when
  no transaction revision was registered; the false results are marked
  unusable in the [capture report](../../../evals/paired/c1-agent-managed-correction-recovery-v6-valid-time-2026-11-25/capture-01/REPORT.md).
  A public CLI regression failed before and passed after the scorer was changed
  to compare the full cutoff only when both axes are registered. The focused
  test and full `paired_comparison_cli` target passed (14 tests); the frozen
  offline capture evaluation completed with its expected NO-GO decision. No
  retrieval change was justified by this ambiguous result.

## Task A10. Attribute the remaining selector mismatch

**Purpose.** A9 proved that requested and returned `valid_at` were correct in
C1 v6, but its aggregate non-query digest cannot identify whether `fact_types`,
the knowledge boundary, budget, view, window, or compact selector differs.
Before changing memory retrieval, record a field-level comparison against a
private, task-registered selector oracle.

**Public seam.** `score-paired-observations` takes an observation bundle and
the private paired task manifest, then emits only per-field match booleans.
The agent adapter and persisted observation contain hashes only; query text,
argument values, returned memory, and private oracle values remain out of the
scored artifact. Exact query equality remains diagnostic and is never a
task-success gate.

**Compatibility.** Add observations v6, paired artifact v10, and native receipt
journal v8. Keep v1–v5 observations and paired artifact v2–v9 replay unchanged.
Fingerprint only the fixed `assemble_context` selector allowlist. Normalize
timestamps to UTC and resolve product defaults before hashing so omitted
`budget`, `compact`, and empty/default selectors are not reported as false
mismatches. A present selector oracle is complete: unspecified allowlisted
fields mean their effective product default.

**Files.** `crates/eval-harness/src/paired_agent/{tool_results,observations}.rs`,
`src/{paired_agent,codex_agent}.rs`, `src/paired.rs`,
`tests/paired_comparison_cli.rs`, and a fresh disjoint C1 registration and
report. Do not change product retrieval or presentation in this task.

- [x] First add a public CLI regression with different faults in two allowed
  selectors. Confirm RED before implementation. Assert that the evaluator
  names only those fields with false matches, reports true matches for the
  independent controls, emits no raw value/query sentinels, and replays old
  artifacts byte-identically.
- [x] Add request-side allowlisted selector fingerprints to both Codex and
  Chat Completions capture paths. Reject malformed/unknown selector-oracle
  fields at registration. Compare each normalized field to the private oracle
  in the scorer; never add the values or expected hashes to public output.
- [x] Bump and validate the three artifact schemas above. Require complete
  selector fingerprints for the new observation version and selector-match
  diagnostics for its paired artifact; preserve historical version-specific
  validation.
- [x] Run focused tests for timestamp normalization and defaults, then the
  whole `paired_comparison_cli` target and relevant adapter targets on native
  Apple Silicon. Replay at least one archived v6 report through the old schema
  path and compare output bytes.
- [x] Register and freeze a disjoint synthetic C1 selector-diagnostic cohort
  before any model call. Run capture 01 on local Apple Silicon without LM Studio;
  keep Codex input synthetic-only. Run capture 02 only after the registered
  capture-01 gates pass, then stop under the preregistered task gate. Report
  per-field selector matches, valid-time results, read/source coverage, task
  success, failures, and latency. A failed gate remains NO-GO; no memory code
  change follows unless a specific selector defect is reproduced and a
  deterministic product-level guard demonstrates it.

**A10 implementation and verification — 2026-11-25.** Selector fingerprints
and paired v10 validation are implemented for Codex and Chat Completions. The
public CLI regression verifies per-field booleans, redaction, mixed registered
and unregistered reads, and byte-identical replay. An archived C1 v6 artifact
replayed byte-identically. Selector normalization tests passed (10); the full
`paired_comparison_cli` target passed (15), the v7 standalone evaluator tests
passed (29), formatting passed, and workspace Clippy passed without warnings.

The fresh [C1 v7 cross-capture review](../../../evals/paired/c1-agent-managed-correction-recovery-v7-selector-diagnostics-2026-11-25/CROSS_CAPTURE_REVIEW.md)
has a NO-GO workflow result: capture 01 passed 4/4 exact cited tasks in both
arms, while the single allowed repeat fell to 0/4 in both. Independent probes,
correction and invalidation checks stayed 4/4. Capture 02 had six empty reads
per arm and one sufficient-evidence incumbent read whose final answer failed;
the candidate's sole nonempty read was insufficient and triggered one
`prohibited_evidence` hard violation. All valid-time checks matched. Selector
mismatches do not explain this result because similar `known_at` and `compact`
mismatches also occurred in successful capture 01. No product memory change is
supported. The cohort stop rule prohibits any further capture on these four
families.

The `v7` in this cohort name refers to the selector-diagnostics artifact
version, not maintenance prompt v7. Its frozen registration and run command pin
`agent_maintenance_v6`, with recall system v3 and recall task rules v4. Do not
cite this C1 result as a model-backed evaluation of `agent_maintenance_v7`.

Critical review of selector normalization found no current mismatch with the
server path: the server clamps retrieval budget to 1–250; trims and drops empty
fact types before membership filtering; uses RFC 3339 parsing for temporal
selectors and the shared `parse_datetime` for windows; and defaults `compact`
to true. The evaluator applies the same effective defaults. Its sort/dedup of
fact types is equivalent because filtering is set membership. This is a source
parity audit, not a cross-crate contract test; duplicated normalization remains
a drift risk, but no code defect was demonstrated.

## Task A11. Resolve the registered prompt conflict with B5

**Finding.** Critical review after A10 found that the maintenance-v6 answer
prompt said to preserve every read argument, including `as_of` and `known_at`,
while B5 requires an authorized retry to bind `valid_at` and `known_revision`
to the returned snapshot. Keep v6 immutable for artifact replay; make the
corrected prompt opt in under a new profile.

- [x] Add `agent_maintenance_v7` and its separate `recall_only_v4` phase. The
  new system/rule pair copies both snapshot axes, omits legacy temporal fields,
  keeps all other selectors fixed, permits at most one explicitly authorized
  query-only retry, and preserves partial/unknown semantics.
- [x] Pin v7 through CLI parsing, managed-manifest fingerprints, Codex identity
  validation and internal read-phase dispatch. Add regressions for prompt
  separation and retain the v6 conflicting prompt/fingerprint path for replay.
- [x] Document the profile and qualification limit in the
  [paired evaluation guide](../../evals/PAIRED_AGENT_EVAL.md#snapshot-consistent-retry--maintenance-v7).
- [x] Extend the registered read scorer to model the initial eligible empty
  partial read and its one possible retry as a state transition. Verify the
  exact returned snapshot cutoff, query-only change, absent legacy temporal
  selectors, preserved remaining selectors, and the retry limit with negative
  artifact tests.
- [x] Freeze a new four-family synthetic cohort and run one model-backed v7
  screen locally on Apple Silicon without LM Studio. Record read evidence,
  exact answers, probes, write/invalidation outcomes and total recall-phase
  latency separately. The earlier C1 selector-diagnostics cohort remains
  ineligible for A11: its frozen runner used `agent_maintenance_v6`.
- [x] Add content-free per-read elapsed timing at the observation boundary.
  Observation v8, paired artifact v12, and native receipt journal v10 carry
  the new per-read timing contract. Validation requires a measured duration
  for every completed read, forbids one for an interrupted read, preserves
  exact request/response pairing, and replays the duration byte-for-byte.
  Codex measures the interval between reading its matching `item.started` and
  terminal `item.completed` events; Chat Completions measures only the
  synchronous memory-tool call. The two adapter timings include different
  transport overhead and must not be compared as if they were the same
  population. The A11 frozen capture contains only total recall-phase latency
  and cannot satisfy this preregistered per-read measure retroactively.
- [x] Verify the new contract through Codex and Chat Completions runner tests,
  out-of-order concurrent-read pairing, offline scorer rejection of missing
  timing, v12 artifact and v10 journal propagation, legacy-schema rejection,
  and byte-identical replay. Final native Apple Silicon verification on
  2026-11-26 passed 877 eval-harness tests (23 ignored) and 2,796 memory_mcp
  tests (45 ignored); workspace Clippy, formatting, and diff checks were clean.
  Latency remains diagnostic and does not establish memory quality or agent
  benefit.

**Disposition — capture executed with one preregistered measurement missing;
downstream workflow NO-GO.** The deterministic transition scorer and v11
retry-diagnostic artifact validation are implemented and tested. In
the disjoint [A11 capture](../../../evals/paired/agent-maintenance-v7-snapshot-retry-screen-2026-11-25/capture-01/REPORT.md),
all four tasks per arm had eligible empty partial reads and compliant
snapshot-bound retries (4/4); the paired reads still returned no evidence and
exact answer-plus-citation success was 0/4 per arm. Independent fixed-query
probes, correction extraction, evidence-before-invalidation checks and exact
invalidation passed 4/4 per arm. Mean recall latency was 18.128 s for incumbent
and 19.199 s for candidate. Both arms used the same memory binary, so these
numbers do not estimate a product effect or v6-to-v7 improvement. The result
isolates that retry instruction adherence alone did not recover evidence for
the agent in this cohort; hashed queries and selector telemetry do not reveal
the exact query choice, so they do not yet establish a memory retrieval defect.
The preregistered per-read elapsed-time measure was not captured; only total
recall-phase latency is available, as documented in the immutable capture
report. Observation v8, paired artifact v12, and native receipt journal v10
now carry validated per-read timing for future captures, but the frozen A11
evidence remains incomplete for latency attribution and cannot be backfilled.
No repeat was run because the registered one-capture stop rule remains binding.

The C1 v7-selector-diagnostics cross-capture screen remains NO-GO and its repeat
stop rule is exhausted. Do not run C2 against it. A future agent workflow
qualification needs a materially changed procedure and fresh families. Any
memory change needs a separate deterministic product-level reproduction.
Production D2 remains gated on matched deployment aggregate windows and an
approved read-only inventory of affected historical records; no production
metric or historical repair is inferred from this local synthetic screen. A
future comparative prompt experiment also needs arm-specific prompt/profile
identity; current paired Codex runs require both arms to share one profile and
prompt identity.

Next independent plans: [Historical retrieval and identity](2026-11-24-historical-memory-implementation-plan.md) · [Qualification and remaining evaluations](2026-11-24-memory-evaluation-qualification-implementation-plan.md).
