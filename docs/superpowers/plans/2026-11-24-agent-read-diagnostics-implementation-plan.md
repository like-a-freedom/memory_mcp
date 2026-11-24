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

- [ ] Write `actual_read_scores_source_coverage_and_missing_claim_status_independently`. Bind `episode:cedar` to the required event `cedar-ledger`. Return it in the read, but omit required `evidence_status: "conflicting"`. Read the CLI receipt and assert `source_coverage == Some(true)` and `annotation_match == Some(false)`. Preserve the legacy task score. Through `run_with_independent_probes`, independently exercise the same agent-trial observation with a successful `FixedTrialProbe` and assert that the independent score still passes.
- [ ] Run `cargo test -p eval-harness --locked --test paired_comparison_cli actual_read_scores_source_coverage_and_missing_claim_status_independently`. RED is absent per-read scoring.
- [ ] Extend `score_trial` to evaluate the captured fields. Reuse existing source-group and annotation matching semantics, leaving independent scoring and existing task acceptance intact. Persist only normalized outcomes. A source in the second reply cannot repair the *first* reply's diagnostics; task-level scoring can still use several successful reads by its existing rules.
- [ ] Run the same filter. Then run one Red → Green slice per case below. Use the case name as the test filter.

| Case | Fixture and assertion |
| --- | --- |
| `registered_cutoff_cannot_be_satisfied_by_a_different_snapshot` | Requested revision 7 and valid time `2026-05-01T10:00:00Z`; response echoes revision 8 and still returns the required event. Assert `source_coverage == Some(true)`, `requested_cutoff_match == Some(true)` and `returned_cutoff_match == Some(false)`. In a positive control an equivalent `+01:00` timestamp matches the same UTC instant. |
| `adequate_read_preserves_wrong_answer_diagnostic` | Required source, correct typed evidence, valid citation, answer `Cabinet-99` against registered `Cabinet-28`. Assert source/annotation/citation checks pass, `accepted_answer == Some(false)`, task fails. |
| `adequate_read_preserves_missing_citation_diagnostic` | Same adequate read and correct answer with empty citation list. Assert `required_evidence_cited == Some(false)` with zero cited returned sources, task fails. |
| `partial_empty_read_does_not_indicate_global_absence` | Return an empty array with `partial` historical context. Assert `Empty` plus `Partial`; absent completeness metadata yields unknown. |

- [ ] Run the focused CLI and adapter targets. Check native and offline scoring produce the same normalized diagnostics for the same projection. Commit `feat(eval): score actual read evidence independently`.

## Task A5. Prove diagnostic redaction and oracle isolation

**Files.** `crates/eval-harness/tests/{paired_prompt_contract,paired_comparison_cli}.rs`, `src/paired_agent/{observations,prompts}.rs`, `src/paired_agent.rs`.

**Interfaces.** Consume A4. Test `export-paired-inputs` and `score-paired-observations`. Legacy `final_answer` remains private scoring input under the existing contract; it never appears in published receipts. New diagnostics never copy raw final text, request parameters, MCP replies or oracle values.

- [ ] Write `read_expectations_stay_out_of_exported_runner_inputs`, with a distinct marker in the private oracle. Assert unchanged public projection and marker absence.
- [ ] Run `cargo test -p eval-harness --locked --test paired_comparison_cli read_expectations_stay_out_of_exported_runner_inputs`. Treat an already GREEN fixture as a guard. Implement only a demonstrated leak fix.
- [ ] Write and run `receipts_exclude_read_and_final_answer_sentinels`. Use distinct request, tool result, provider call ID and final-output markers. Assert that artifacts and journal contain none while task scoring still uses the private final text. Run `cargo test -p eval-harness --locked --test paired_comparison_cli --test paired_prompt_contract`.
- [ ] Update the public schema/replay guide and commit `docs(eval): specify read telemetry and archive compatibility` with any redaction fix.

## Task A6. Replay one registered public reply in a diagnostic capture

**Files.** `crates/eval-harness/src/paired_agent/chat_completions_backend.rs`, `src/paired_agent/chat_completions.rs`, `src/{cli,main}.rs`, `tests/{chat_completions_runner,paired_comparison_cli}.rs`.

**Interfaces.** Introduce `RecallReadMode::{Live, FixedReply}` with a diagnostic-only `RecallMemorySession<M: MemoryToolSession>` implementing the existing trait. First add the adapter through `run_phase`, then expose explicit `run-paired-chat-completions --recall-read-mode live|fixed-reply` for a recall task manifest. The existing CLI without this flag still consumes its managed-task manifest. Use `--fixed-read-inputs <path>` only with `fixed-reply`. Registration keys are `CaseTrialKey` plus arm; each value pins canonical public `assemble_context` arguments and its expected normalized response shape. The payload delivered to the agent is the exact response to that fixed call in the trial's seeded session. Bind mode and registration hash to `SharedRunIdentity.reader_config_sha256`; compare runs of different modes as diagnostic controls, outside the promotion comparator.

- [ ] Write `fixed_reply_is_delivered_to_the_original_tool_call`. Script `read-1`. Have the public tool fixture return empty for the model-selected query and its one registered response containing `episode:cedar` and `Cabinet-28` for the fixed query. Assert the next model request has the same `tool_call_id`, a tool message whose content deserializes to the exact fixed response JSON, and the same advertised schema as the live path.
- [ ] Run `cargo test -p eval-harness --locked --test chat_completions_runner fixed_reply_is_delivered_to_the_original_tool_call`. The incumbent forwards the empty live result.
- [ ] Validate a unique, bounded registration before capture. For the first allowed `assemble_context` call, substitute the registered public request and deliver its exact reply. Execute one public read in either mode. Use a separate diagnostic registration with `tool_call_budget = 1` and track the model's attempted arguments distinctly from the fixed executed request. Never supply a reply for a forbidden tool or without a read. Validate that the response sources and snapshot bind to this seeded trial. This reuse avoids cross-run episode IDs and extra retrieval side effects.
- [ ] Run the same filter. Red → Green for `recall_capture_rejects_unregistered_read_mode` at the CLI. Assert exit 2 before opening a session when CLI mode or registry hash disagrees with the exploratory plan. Reuse existing public seed logic, fingerprints, loopback checks and budgets. Do not accept confirmation plans in this diagnostic mode.
- [ ] Run both target suites. Assert unchanged history inputs, budgets and oracle projection and report successful reads, clarification turns, input/output tokens and budget exits separately. Commit `feat(eval): add fixed-reply recall diagnostics`.

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

## Task A8. Change response presentation only after A7 reproduces the failure

**Files.** `crates/memory-mcp/src/tools/assemble_context.rs`, owning `src/memory/api.rs` or retrieval module if domain logic is needed, `tests/assemble_context_wire_contract.rs`. Any wire addition requires an approved compatibility contract first.

- [ ] Freeze a public candidate response fixture that gives a typed disposition (`answer`, `clarify`, `insufficient`) and existing source IDs. Compare the same sources and budgets with the current projection in the controlled runner. Proceed only with an observed presentation-specific gain under A7 and the unchanged safety oracle.
- [ ] Write one wire-contract test per disposition. Assert the exact serialized disposition, existing source ID and no new claims. For partial input assert `insufficient` or `clarify` as specified by the accepted fixture, never a global absence conclusion. Pin the old consumer projection and existing payload limit in separate cases.
- [ ] Run each new test through `cargo test -p memory_mcp --locked --test assemble_context_wire_contract <test-name>` before and after its minimal implementation. Replay ordinary and contradiction controls through the native deterministic plan. Require no new ordinary miss, zero prohibited/unsupported evidence, unchanged compatibility and compliance with the frozen item/byte budgets.
- [ ] Commit a product change only with reproducible GREEN receipts. Document the screen and its scope even if the candidate is declined.

Next independent plans: [Historical retrieval and identity](2026-11-24-historical-memory-implementation-plan.md) · [Qualification and remaining evaluations](2026-11-24-memory-evaluation-qualification-implementation-plan.md).
