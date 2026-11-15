# Bounded `explain` Citation Output Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use `executing-plans` or `subagent-driven-development` to implement this plan task-by-task. Track steps with checkbox syntax.

**Goal:** Make every successful `explain` response fit a hard 32 KiB serialized-envelope limit while preserving exact quotes and making complete source text retrievable through bounded reads.

**Architecture:** The memory context owns quote verification, source-window semantics, provenance assembly, and citation metadata. The `explain` tool adapter owns item selection and final envelope budgeting. The MCP handler remains a thin schema/delegation adapter, and the container shim remains a container-shaped delegate only.

**Spec:** `docs/superpowers/specs/2026-11-13-explain-bounded-citation-output-design.md`

**Tech stack:** Rust 1.99.0, serde/serde_json, schemars, existing `memory_mcp` integration tests, and the existing `eval-harness`. No new dependency.

## Global constraints

- Measure and enforce at most **32 KiB serialized UTF-8 JSON** for the complete `ToolResponse<Value>` returned by `explain`, in compact and verbose modes.
- The limit excludes RMCP/JSON-RPC framing and is not a token guarantee.
- Default excerpt target: **1,200 Unicode scalar values** per item.
- Explicit source read: maximum **8,000 Unicode scalar values**; offsets are zero-based Unicode scalar offsets.
- Default provenance page: **20 records** per item.
- Use `source_limit_scalars`, `provenance_offset`, and `item_offset`; do not overload `ToolResponse.next_offset`, which remains an item index.
- Never serialize a partial quote as `quote`.
- Preserve the complete exact quote whenever it fits; compact mode includes this bounded citation-fidelity exception.
- Every `partial` response has at least one machine-readable reason code.
- Keep business logic in the memory context; keep transport byte budgeting in `tools/`; keep `mcp/handlers.rs` and the container shim thin.
- Keep the eight-tool surface frozen. Add no MCP tool, raw query, caller-selected table, or dependency.
- Do not modify generated code or migration files.
- Do not commit unless explicitly requested.

## Ownership map

| Concern | Owner |
|---|---|
| Flat request schema | `crates/memory-mcp/src/tools/params.rs` |
| Parsing and item selection | `crates/memory-mcp/src/tools/parsers.rs`, `crates/memory-mcp/src/tools/explain.rs` |
| Memory use-case seam | `crates/memory-mcp/src/memory/api.rs` |
| Quote/window/provenance semantics | `crates/memory-mcp/src/memory/explanation.rs` behind `crates/memory-mcp/src/memory/capabilities/explain.rs` |
| Container adaptation only | `crates/memory-mcp/src/service/memory_container_shims/memory_capabilities_explain.rs` |
| Final envelope size enforcement | `crates/memory-mcp/src/tools/explain_budget.rs` |
| Generic envelope constructors | `crates/memory-mcp/src/tools/response.rs` |
| MCP schema and delegation | `crates/memory-mcp/src/mcp/handlers.rs` |

## Review focus

1. **Hard-cap proof:** multi-megabyte source, repeated references, JSON escaping, oversized caller metadata, and a single pathological item all remain at or below 32 KiB in both modes.
2. **Quote fidelity:** complete exact quote is preserved when it fits; unverified and budget-omitted quotes remain distinguishable.
3. **Continuation semantics:** item, source, and provenance offsets have distinct names, units, ordering, and terminal behavior.
4. **Reconstruction:** explicit reads from offset zero reconstruct the source string exactly.
5. **Metadata atomicity:** every shortened citation window updates returned/next range metadata and partial reasons together.
6. **Provenance fidelity:** distinct relationship/path edges survive; full bodies do not.
7. **Architectural locality:** evidence rules exist in the memory context, byte policy in the tool adapter, and no duplicate policy appears in the handler or shim.

---

## Task 1: Ratify the behavioral decision before code changes

**Files:**
- Create: `docs/adr/0083-bound-explain-citation-output.md`
- Modify: `docs/adr/0022-compact-response-default-for-llm-consumers.md`
- Modify: `docs/superpowers/specs/2026-07-30-token-efficient-responses-design.md`

**Interfaces:**
- ADR-0083 supersedes only the July 30 full-body inlining policy.
- ADR-0022 keeps the compact default and its unrelated omissions, but records the `explain` exact-quote exception and the fact that verbose output is now bounded.

- [ ] **Step 1: Draft ADR-0083 from this design.** Record the problem, rejected alternatives, 32 KiB envelope boundary, three distinct continuation dimensions, quote-state rules, partial reason codes, reducer fallback, compatibility break for unbounded verbose output, and migration path.
- [ ] **Step 2: Amend ADR-0022 precisely.** Do not rewrite its historical rationale. Add an amendment section stating that compact `explain` now includes a complete bounded exact quote because citation fidelity overrides the earlier duplicate-content assumption; `compact=false` no longer means unbounded output.
- [ ] **Step 3: Mark the July 30 design as partially superseded.** Add a prominent status note pointing to ADR-0083 for `citation_context` and `all_sources.episode_content`; retain the historical body for auditability.
- [ ] **Step 4: Check cross-document consistency.** Verify that the spec, ADR-0083, and the ADR-0022 amendment use the same field names, byte boundary, offsets, and partial reason codes.

## Task 2: Add and plumb the bounded-read request contract

**Files:**
- Modify: `crates/memory-mcp/src/tools/params.rs`
- Modify: `crates/memory-mcp/src/models/request.rs`
- Modify: `crates/memory-mcp/src/tools/parsers.rs`
- Modify: `crates/memory-mcp/src/tools/explain.rs`
- Modify: `crates/memory-mcp/src/mcp/handlers.rs`
- Modify: `crates/memory-mcp/tests/tools_e2e.rs`

**Interfaces:**
- `ExplainParams` gains flat optional `source_offset`, `source_limit_scalars`, `provenance_offset`, and `item_offset` fields.
- `ExplainRequest` gains `source_offset`, `source_limit_scalars`, and `provenance_offset`. `item_offset` remains adapter-owned because it pages the submitted array before the capability call.
- All offsets are zero-based. Source offsets count Unicode scalar values; provenance and item offsets count list indices.
- Required identifiers are bounded during parsing so the minimal reducer fallback fits by construction.

- [ ] **Step 1: Write failing schema and parsing tests.** Assert all four fields are optional flat integers; `compact` still defaults to `true`; unknown and nested fields remain rejected; `source_limit_scalars` without `source_offset` is rejected; a source limit above 8,000 is rejected; an overlong required `source_episode` is rejected.
- [ ] **Step 2: Run the focused schema test and verify failure.**

  ```bash
  cargo test -p memory_mcp --lib tools::params::tests::explain_params_schema
  ```

  Expected: failure because the new request fields and validation do not exist.

- [ ] **Step 3: Add the request fields.** Add serde defaults and schema descriptions that state the unit, valid combinations, terminal behavior, and 8,000-scalar maximum. Update `ExplainRequest` literals at all call sites through Rust reference search, not textual guessing.
- [ ] **Step 4: Implement item selection in `tools/explain.rs`.** Parse the original array, retain its original length for `total_count`, apply `item_offset`, and reject out-of-range offsets. Require exactly one selected item when any source or provenance continuation field is present. Construct `ExplainRequest` in `tools/explain.rs`; do not move this construction into `mcp/handlers.rs`.
- [ ] **Step 5: Update the MCP description and schema assertions.** The handler remains a delegate. Its tool description must tell the agent when and how to use each continuation field.
- [ ] **Step 6: Run the focused tests and verify success.**

  ```bash
  cargo test -p memory_mcp --lib tools::params::tests::explain_params_schema
  cargo test -p memory_mcp --lib tools::parsers::tests
  ```

## Task 3: Own citation, source-window, and provenance semantics in the memory context

**Files:**
- Modify: `crates/memory-mcp/src/memory/api.rs`
- Modify: `crates/memory-mcp/src/memory/capabilities/explain.rs`
- Modify: `crates/memory-mcp/src/memory/explanation.rs`
- Modify: `crates/memory-mcp/src/service/memory_container_shims/memory_capabilities_explain.rs`
- Modify: `crates/memory-mcp/src/models/request.rs`
- Modify: `crates/memory-mcp/src/models/provenance.rs`
- Create: `crates/memory-mcp/src/models/partial.rs`
- Modify: `crates/memory-mcp/src/models/mod.rs`
- Modify: `crates/memory-mcp/tests/explain_provenance.rs`

**Interfaces:**
- `ExplanationPort` and `memory::api::explain_context` remain the public memory use-case seam.
- `ExplainItem` exposes the citation metadata and six independent quote-state booleans defined in the spec.
- A shared typed `PartialReason` enum is the single source of reason-code strings for item and envelope metadata.
- `ProvenanceSource.episode_content` becomes `Option<String>` and is `None` for `explain` output.

- [ ] **Step 1: Add failing capability-contract tests.** Cover:
  - default quote-anchored verbatim excerpt and the six quote-state fields;
  - explicit source read and exact reconstruction from offset zero;
  - source end, empty source, zero limit, and out-of-range offset;
  - supplied quote absent from source and source unavailable;
  - provenance ordering and exact-edge deduplication while retaining distinct relationship/path edges;
  - provenance page totals and terminal offsets;
  - empty caller content never falling back to the full episode body;
  - compact serialization retaining the complete exact quote.
- [ ] **Step 2: Run the focused integration target and verify failure.**

  ```bash
  cargo test -p memory_mcp --test explain_provenance
  ```

  Expected: failure because current output contains full bodies and lacks the new metadata contract.

- [ ] **Step 3: Implement scalar-safe source helpers in the memory context.** Use `char` counts and scalar offsets. Keep the helpers private to the memory implementation unless another owned context needs the same interface. Never use byte slicing for source offsets and never claim grapheme safety.
- [ ] **Step 4: Implement the two window modes.** Default mode centers a bounded excerpt around the first verified quote occurrence or returns the first bounded excerpt. It must not populate a continuation offset because it is not a reconstruction sequence. Explicit mode returns a contiguous range, defaults to 8,000 scalars, and emits the next contiguous offset.
- [ ] **Step 5: Implement the quote-state truth table.** Verify the entire supplied quote against the source. Return the complete quote when it fits; never return a prefix as `quote`. Set `quote_returned`, `quote_complete`, `quote_verified`, `quote_anchored`, and `quote_contained_in_window` independently according to the spec table.
- [ ] **Step 6: Implement metadata-only provenance paging.** Make `episode_content` optional, set it to `None`, deduplicate by `(episode_id, relationship, entity_path)`, apply the spec's deterministic total order, and emit totals/next offsets from the final list.
- [ ] **Step 7: Add the typed partial-reason enum.** Use serde `snake_case` names matching the spec exactly. Reuse it in `ExplainItem` and `ToolResponse`; do not maintain parallel string lists.
- [ ] **Step 8: Keep the seam thin.** Update `memory/api.rs` documentation and the capability/shim only as needed to pass the request through. Do not put source-window or provenance policy in the shim.
- [ ] **Step 9: Run the focused target and verify success.**

  ```bash
  cargo test -p memory_mcp --test explain_provenance
  ```

## Task 4: Enforce the hard envelope budget deterministically

**Files:**
- Create: `crates/memory-mcp/src/tools/explain_budget.rs`
- Modify: `crates/memory-mcp/src/tools/mod.rs`
- Modify: `crates/memory-mcp/src/tools/explain.rs`
- Modify: `crates/memory-mcp/src/tools/response.rs`

**Interfaces:**
- The budget module exposes one deep operation to the explain adapter: build a complete bounded `ToolResponse<Value>` from candidate items and item-pagination metadata.
- `ToolResponse` gains optional `partial_reasons` and a production partial-list constructor. The test-only partial constructor is removed or delegates to the production constructor.
- The reducer measures the final serialized envelope and guarantees the 32 KiB cap by deterministic reduction plus a minimal-envelope fallback.

- [ ] **Step 1: Write failing reducer tests.** Cover:
  - exact quote and citation context preserved when the envelope fits;
  - JSON-escaping-heavy content measured by serialized bytes, not raw string length;
  - a single item with oversized metadata;
  - citation-context shortening updating returned/next range and partial reasons atomically;
  - oversized `content` setting `content_truncated` and never being represented as complete;
  - an oversized quote omitted, not serialized as a prefix;
  - tail-item omission with correct `has_more`, `total_count`, and `next_offset`;
  - the minimal empty-result fallback fitting the cap and remaining actionable.
- [ ] **Step 2: Run the focused reducer tests and verify failure.**

  ```bash
  cargo test -p memory_mcp --lib tools::explain_budget::tests
  ```

  Expected: failure because no reducer exists.

- [ ] **Step 3: Implement the reducer over typed `ExplainItem` values.** Serialize the complete candidate envelope after applying the compact guard. When over budget, remove optional diagnostics first, then reduce optional evidence in the spec's deterministic order, then omit tail items, then apply the minimal fallback.
- [ ] **Step 4: Keep metadata atomic.** Whenever citation text or content changes, update its completeness/range fields and the corresponding typed `PartialReason` values in the same transformation.
- [ ] **Step 5: Implement the production partial envelope constructor.** `partial_reasons` is omitted when empty and serialized in a stable order when present. A `partial` response must never have an empty reason list.
- [ ] **Step 6: Route `tools/explain.rs` through the budget module.** The adapter keeps timing, logging, request construction, and invocation. The budget module owns only response shaping and byte policy.
- [ ] **Step 7: Run focused tool tests and verify success.**

  ```bash
  cargo test -p memory_mcp --lib tools::explain_budget::tests
  cargo test -p memory_mcp --lib tools::explain::tests
  ```

## Task 5: Prove the public MCP contract and the frozen surface

**Files:**
- Modify: `crates/memory-mcp/tests/tools_e2e.rs`
- Modify: `crates/memory-mcp/src/mcp/handlers.rs`
- Create: `crates/memory-mcp/tests/public_tool_surface.rs`

**Interfaces:**
- Public tests call the existing `MemoryMcp::explain` path and measure the returned `ToolResponse<Value>`.
- The canonical tool-name snapshot contains exactly: `ingest`, `explain`, `extract`, `resolve`, `invalidate`, `open_app`, `app_command`, `assemble_context`.

- [ ] **Step 1: Add failing end-to-end tests.** Ingest a multi-megabyte direct source and linked source, submit repeated references, and call `explain` in compact and verbose modes. Assert the complete envelope is at most 32 KiB, reasons are present, no full body appears in `all_sources`, and an exact quote that fits survives.
- [ ] **Step 2: Add continuation tests.** Cover item, provenance, and source continuation independently. Verify the Unicode source reconstructs exactly from consecutive explicit reads beginning at offset zero.
- [ ] **Step 3: Add the canonical tool-name snapshot.** Do not rely on the existing router test that checks only `extract` and `ingest`, and do not treat `public_surface_audit.rs` as a canonical tool-name snapshot.
- [ ] **Step 4: Run the focused public tests and verify failure.**

  ```bash
  cargo test -p memory_mcp --test tools_e2e test_mcp_explain_bounded
  cargo test -p memory_mcp --test public_tool_surface
  ```

  Expected: failure before the bounded behavior and snapshot test exist.

- [ ] **Step 5: Make only the remaining adapter/schema changes required by the failing tests.** Do not move evidence policy into `mcp/handlers.rs`.
- [ ] **Step 6: Run the focused public tests and verify success.**

  ```bash
  cargo test -p memory_mcp --test tools_e2e test_mcp_explain_bounded
  cargo test -p memory_mcp --test public_tool_surface
  ```

## Task 6: Add a deterministic envelope-and-reconstruction eval

**Files:**
- Modify: `crates/eval-harness/src/suites/response_size.rs`
- Modify: `evals/profiles/response_size.json`

**Interfaces:**
- `ResponseSizeSuite` keeps the 61 shared retrieval-fixture cases and adds one explicitly synthetic `citation-progressive-disclosure` case. The synthetic case is not appended to the shared retrieval fixture.
- The suite calls the public tool path, `memory_mcp::tools::explain`, so it measures the real `ToolResponse` envelope rather than only serialized capability results.
- The profile expects 62 exact cases.

- [ ] **Step 1: Add a failing suite expectation and profile count.** Append the synthetic ID to `ResponseSizeSuite.expected_case_ids` and update `exact_cases` from 61 to 62. The synthetic run case must be produced by the suite itself.
- [ ] **Step 2: Run the focused suite test and verify failure.**

  ```bash
  cargo test -p eval-harness suites::response_size
  ```

  Expected: failure because the synthetic case and envelope measurement are absent.

- [ ] **Step 3: Implement the synthetic case.** Seed one source with a known scalar sequence, call the public explain tool in compact mode, follow the explicit source continuation until the end, and assert:
  - compact and verbose envelopes are at most 32 KiB;
  - the exact supplied quote is unchanged;
  - concatenated explicit windows equal the original source exactly;
  - no scalar is skipped or duplicated.
- [ ] **Step 4: Emit honest metrics.** Record actual envelope byte sizes and a deterministic `citation_source_reconstruction_exact` metric. Do not label this case as downstream answer-quality evidence.
- [ ] **Step 5: Run the focused suite and verify success.**

  ```bash
  cargo test -p eval-harness suites::response_size
  ```

## Task 7: Reconcile documentation and run the shipping checks

**Files:**
- Modify: `docs/superpowers/specs/2026-11-13-explain-bounded-citation-output-design.md`
- Modify: `docs/adr/0083-bound-explain-citation-output.md`

- [ ] **Step 1: Update the spec status and implementation notes.** Record the final field names, any approved deviations, and links to the implemented ADR and tests. Do not silently rewrite the normative contract.
- [ ] **Step 2: Run narrow validation first.**

  ```bash
  cargo test -p memory_mcp --lib tools::params::tests::explain_params_schema
  cargo test -p memory_mcp --test explain_provenance
  cargo test -p memory_mcp --lib tools::explain_budget::tests
  cargo test -p memory_mcp --test tools_e2e test_mcp_explain_bounded
  cargo test -p memory_mcp --test public_tool_surface
  cargo test -p eval-harness suites::response_size
  ```

- [ ] **Step 3: Run package tests.**

  ```bash
  cargo test -p memory_mcp
  cargo test -p eval-harness
  ```

- [ ] **Step 4: Run the project-required formatting and lint checks.**

  ```bash
  cargo fmt --all --check
  cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
  ```

- [ ] **Step 5: Run the eval artifact.**

  ```bash
  cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/response_size.json --artifact target/evals/explain-bounded-citation.json
  ```

  Expected: 62 cases are present, the synthetic reconstruction case passes, and both measured explain envelopes are at most 32 KiB.

- [ ] **Step 6: Inspect the final diff.** Confirm no dependency, generated file, migration, unrelated untracked document, or ninth tool was changed.

## Acceptance-criteria traceability

| Spec criterion | Primary proof |
|---|---|
| Hard 32 KiB cap in both modes | Task 4 reducer tests; Task 5 public test; Task 6 eval |
| No partial quote masquerades as exact | Task 3 quote-state tests; Task 4 reducer test |
| Exact reconstruction from offset zero | Task 3 capability test; Task 5 continuation test; Task 6 eval |
| Correct terminal and invalid boundaries | Task 2 validation tests; Task 3 capability tests |
| Provenance fidelity and deterministic order | Task 3 capability tests |
| Explicit partial reasons | Task 3 enum/contract tests; Task 4 envelope tests |
| Eight-tool surface frozen | Task 5 canonical snapshot |
| Correct ownership/no duplicated policy | Task 3 seam review; Task 4 adapter review; Task 5 handler review |
| Compatibility decisions recorded | Task 1 ADRs; Task 7 documentation |

## Explicit non-claim

The deterministic eval proves bounded envelopes and evidence fidelity. It does not prove that a downstream model writes a better final answer. If that claim is required later, add a separate model-based answer/citation evaluation rather than relabeling the deterministic case.
