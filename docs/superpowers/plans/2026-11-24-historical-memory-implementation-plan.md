# Historical Memory Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preserve correct historical evidence across large candidate sets, similar entity names and later writes.

**Architecture:** Use immutable, revision-visible evidence for historical reads and record query completeness honestly. Capture baseline behavior at the public capability and confirm product changes through model-free public MCP replay.

**Tech Stack:** Rust, Cargo, embedded SurrealDB, MCP stdio, deterministic synthetic cases.

**Spec:** [Agent memory follow-up, Task 3](../specs/2026-11-24-agent-memory-follow-up-design.md#task-3-establish-historical-retrieval-and-identity-boundaries) · [ADR-0083](../../adr/0083-revision-bounded-temporal-context.md) · [Temporal context compatibility](../../compatibility/TEMPORAL_CONTEXT.md)

## Global Constraints

- Required eval target is native `aarch64-apple-darwin`. Use locked Cargo, synthetic histories, a fresh local database and model-free retrieval.
- Keep the eight-tool surface and public temporal parameter semantics. Preserve `partial` or `unknown` for incomplete historical visibility. A zero result in a partial snapshot cannot establish global absence.
- Freeze case identities, inputs, expected outcomes, product configuration, budgets and decision rules before reading results. Retests are correlated. Use task families as the analysis unit.
- Follow approval rules in `AGENTS.md` for dependencies and migrations. Confirm the public test seams in plan review before writing tests.

## Review Focus

- Exactly 250 eligible facts fill the scan; the 251st marks it truncated. B1.
- The oldest matching fact may have no direct full-question hit and fall beyond the supplement. B2.
- A later write with an earlier valid time cannot change a saved knowledge snapshot. B3.
- Canonical identity must distinguish numeric subject IDs and must not use today's alias table for a past read. B4.
- Public replay must reject regressions even where both arms remain honestly partial. B5.

## File Map and Proposed Seams

| Files | Responsibility and observable boundary |
| --- | --- |
| `crates/memory-mcp/tests/{historical_query_grounding,historical_selector_exclusions}.rs`, `tests/common/mod.rs` | `AssembleContextCapability::assemble_context_temporal_from_service(&MemoryService, AssembleContextRequest, TemporalContextQuery) -> Result<ContextAssemblyResult, MemoryError>` and public writes against real embedded storage. Proves retrieval and cutoff behavior independently of transport serialization. |
| `crates/memory-mcp/src/memory/retrieval/{pipeline,admission}.rs`, `src/knowledge/{knowledge_store,queries,entity_resolution}.rs` | Existing owners for snapshot selection, explicit subject admission and identity resolution. Modify the owner implicated by a witnessed failure. |
| `crates/memory-mcp/tests/{assemble_context_reconciliation,assemble_context_wire_contract}.rs` | Public claim roles, provenance and revision metadata controls. |
| `evals/corpora/memory-decisions-historical-boundaries/`, `evals/profiles/memory_decisions_historical_boundaries.json`, `evals/paired/historical-boundaries-2026-11-24/` | New registration, public MCP fixture, acceptance rule and reproducible receipts. The test-only `add_fact` setup cannot substitute for a public replay fixture. |

## Task B1. Freeze the scan boundary on the incumbent

**Files.** `crates/memory-mcp/tests/historical_query_grounding.rs`. Use `common::seed_fact_at` with fixed UTC times and unique contents.

**Interfaces.** The above capability accepts `TemporalContextQuery { valid_at, known_at, known_revision }`. Read the checkpoint from `metadata.snapshot.known_revision` after setup. Observe `items`, `metadata.candidate_scan_truncated`, and `metadata.completeness`.

- [x] Write one case `historical_249_facts_fit_candidate_scan`. Seed 249 eligible `note` facts at distinct seconds before `2026-03-02T00:00:00Z`. Make a public query with empty text, `budget = 250` and that valid time. Assert 249 distinct result IDs, `candidate_scan_truncated == false`, `completeness == Partial`.

```rust
assert_eq!(result.items.len(), 249);
assert!(!metadata.candidate_scan_truncated);
assert_eq!(metadata.completeness, ContextCompleteness::Partial);
```

- [x] Run `cargo test -p memory_mcp --locked --test historical_query_grounding historical_249_facts_fit_candidate_scan` and save its outcome. This is a characterization guard; an incumbent pass requires no product change.
- [x] Repeat for `historical_250_facts_fit_candidate_scan` and `historical_251_facts_report_truncated_scan`, expecting 250 results in both, truncation false and true respectively, with partial completeness preserved. Run each filter and record the incumbent results. If a case fails, diagnose the fixture's number of eligible facts and unrelated omitted channels before proposing code.
- [x] Commit `test(memory): capture historical scan boundaries`.

## Task B2. Retrieve an old declared subject beyond the supplement

**Files.** `crates/memory-mcp/tests/historical_query_grounding.rs`; `src/memory/retrieval/pipeline.rs` and `src/knowledge/{knowledge_store,queries}.rs` if the RED points to historical candidate discovery.

**Interfaces.** Same capability as B1. Keep its signature. Use the existing `select_facts_at_revision` access path and fact type filters if they suffice. A new narrow knowledge selector requires proof that the existing port cannot express the bounded query.

- [x] Write `historical_old_declared_subject_survives_distractor_history`. Seed the literal content `subject: Copper Warehouse\ndimension: capacity\nvalue: 42 crates for Copper Warehouse` at `2026-03-01T00:00:00Z`. Seed 250 later eligible facts about distinct numbered depots without the Copper subject. Query `What capacity was recorded for Copper Warehouse?` with `budget = 5` and the same cutoff as B1. Assert one returned fact with the seeded target ID and value, zero foreign IDs, `Partial` and a truthful truncation flag. Include a short exact query as a positive characterization that the target is visible under that revision.
- [x] Run `cargo test -p memory_mcp --locked --test historical_query_grounding historical_old_declared_subject_survives_distractor_history`. Expected defect signal is an absent target on the long query with a present target on the exact query. If both succeed, record that the intended FTS miss was unproved and use a natural equivalent phrasing with independently known expected value. Proceed only with a confirmed failure.
- [x] Add the smallest bounded discovery step under the same revision, valid time and access restrictions. Preserve immutable source content, existing direct matches, explicit-subject admission, work bounds and stable ordering. Keep the observable scan metadata truthful and version the changed selector policy. Use the existing global supplement as a bounded fallback. Raising its cap does not satisfy the fix.
- [x] Rerun the focal and B1 filters. Keep a characterization or fix commit eligible independently from identity changes. Commit `fix(memory): retrieve older revision-visible subject evidence` only after B5's product gate.

## Task B3. Preserve an exact snapshot after a late correction

**Files.** `crates/memory-mcp/tests/assemble_context_wire_contract.rs` and existing `tests/assemble_context_reconciliation.rs`.

**Interfaces.** `tools::assemble_context<T: ToolContext>(&T, AssembleContextParams) -> Result<ToolResponse<Value>, MemoryError>`. Capture `context.snapshot.known_revision` from a successful public tool response. Set `valid_at` and `known_revision` explicitly for replay.

- [x] Write `historical_large_history_replay_excludes_a_late_backdated_correction`. Use the B2 target/history, capture a response at revision R, then add a source-backed correction from `42 crates` to `48 crates` with valid time on or before the saved cutoff. Rerun the original query at R. Compare selected fact IDs, source IDs, typed evidence/relation roles, completeness and truncation to the original. Assert that the later source is absent. Verify at the new revision that the correction's registered source is visible. Use the existing reconciliation fixture to form the correction through the supported write path.
- [x] Run `cargo test -p memory_mcp --locked --test assemble_context_wire_contract historical_large_history_replay_excludes_a_late_backdated_correction`. Save the result. Fix the identified revision leak or selector change only on a confirmed RED. An incumbent pass remains a guard.
- [x] Rerun the focal test and `cargo test -p memory_mcp --locked --test assemble_context_reconciliation`. Target B5 before shipping any product change.

## Task B4. Exercise identity controls at their owning boundary

**Files.** `crates/memory-mcp/tests/{historical_query_grounding,historical_selector_exclusions}.rs`; `src/memory/retrieval/admission.rs` or `src/knowledge/entity_resolution.rs` as indicated by the failing public path.

**Interfaces.** Use the B1 public read and supported public writes. Reuse existing controls `historical_numeric_subject_identifiers_remain_distinct`, `historical_multi_subject_question_does_not_require_combined_name`, `historical_similar_explicit_subjects_do_not_create_false_conflict`, and `historical_explicit_subject_does_not_trust_prior_fuzzy_alias`.

- [x] Write `historical_explicit_subject_matches_case_variant`. Store `Copper Warehouse` with a distinct declared subject and query the same name with changed case. Assert the same target ID, no unrelated subject, and no extra contradiction annotation. Run the focused test and capture RED or a GREEN guard before modifying normalization.
- [x] Write `historical_transliterated_alias_reports_omitted_expansion`. Use a stored explicit subject `Северный склад` and the distinct transliteration `Severnyi sklad`. Verify a positive canonical read and a historical alias query whose response advertises `alias expansion` in omitted channels and remains partial. Add an alias through the supported resolver fixture after the captured revision, then verify the old snapshot does not change. If no public explicit-alias write fixture exists, reproduce through supported extraction plus declared-subject ingestion. Keep a future alias recall proposal behind a separate revisioned identity design.
- [x] Run `cargo test -p memory_mcp --locked --test historical_query_grounding --test historical_selector_exclusions --test assemble_context_reconciliation`. For each measured false merge, false admission or false relation, assert its own public effect and make the smallest owner change. Keep one behavioral RED → GREEN cycle per fix.
- [x] Pass B5 and commit an identity fix independently of B2.

## Task B5. Accept a candidate through public MCP

**Files.** New corpus, profile and acceptance registration from the file map. Existing evaluator entry points `memory-eval run`, `compare-decision-replays` and `accept-memory-candidate`.

- [x] Model the reproduced failure through public ingest/extract and `assemble_context` in a fresh registered corpus. Freeze event ordering, distinct source IDs, exact checkpoint and valid time, expected/prohibited evidence, ordinary controls and maximum items/serialized bytes. Register the minimum gain on the affected failure class before observing the candidate. Require at least one additional passing family in that class, zero new ordinary misses and zero prohibited evidence or completeness violations. If a test-only fact cannot be reproduced through public formation, keep its capability result scoped and document the public workflow gap.
- [x] Preserve the incumbent built from `5595e0d1` and the candidate built with identical Cargo features and product config. For each binary run twice, setting `MEMORY_DECISIONS_SERVER_BIN` and `MEMORY_DECISIONS_ARTIFACT_ROOT` to absolute paths and `MEMORY_DECISIONS_PRODUCT_REVISION` to the actual revision. Use a fresh receipt root per capture. Run `cargo run -p eval-harness --locked --bin memory-eval -- run --profile evals/profiles/memory_decisions_historical_boundaries.json --artifact <capture-root>/run.json`.
- [x] Compare each pair with `cargo run -p eval-harness --locked --bin memory-eval -- compare-decision-replays --left <first-root>/index.json --right <second-root>/index.json`. Require matching decisions and registered deterministic fields. Keep runtime observations separate.
- [x] Use `score_channel = "evidence_enabled_temporal"` for this cross-build plan. The existing `primary` channel requires one binary on both sides and is reserved for the same-build rollout gate. Pin distinct incumbent/candidate SHA-256 values in the registration and verify them against the two replay identities; `product_binary_sha256` in the existing acceptance schema, if set, requires one common binary.
- [x] Run `cargo run -p eval-harness --locked --bin memory-eval -- accept-memory-candidate --plan <frozen-plan.json> --corpus-root evals/corpora/memory-decisions-historical-boundaries --incumbent-first <i1>/index.json --incumbent-second <i2>/index.json --candidate-first <c1>/index.json --candidate-second <c2>/index.json --output <fresh-verdict.json>`. Expect exit 0 and an accepted verdict only if the registered gain and safety checks pass. For public byte and completeness limits absent from its current schema, use deterministic wire assertions in B1–B4 as additional required gates.
- [x] Save the accepted or rejected result and reproducible commands. Run the established native deterministic gate from [Qualification C3](2026-11-24-memory-evaluation-qualification-implementation-plan.md#task-c3-verify-the-native-gate-and-add-fresh-histories). Commit accepted product and evidence slices as they clear their gates.

The B4 public acceptance verdict is recorded in
[the independent identity report](../../../evals/paired/historical-identity-2026-11-24/report.md).
The checked-in indexes distinguish the two binaries. The broader
Qualification C3 v9 regression gate is tracked separately.

### B5 verification record

The [historical-boundaries](../../../evals/paired/historical-boundaries-2026-11-24/report.md)
and [historical-identity](../../../evals/paired/historical-identity-2026-11-24/report.md)
comparisons archived their accepted verdicts and both replay indexes.
The B1–B3 tests landed together in `8b404755`. The B4 fix landed in
`99ebbfca`. The historical-boundaries comparison used the captured incumbent `4a5774d0`. Its product source is
identical to `5595e0d1` in `crates/memory-mcp`. The committed indexes
identify each binary by SHA-256. The historical case receipts were not
included with the indexes. Per-arm binary pins are verified for newly
registered plans. The archived comparisons predate that registration field.
A fresh replay with pinned binaries `4a5774d0` and `8b404755` matched across
four captures and passed at 1/3 → 2/3 families, with no losses or hard
failures. The new CLI also rejected a substituted candidate hash and a
cross-build pair with an incompatible input schema. Two fresh public MCP
runs of the current historical-boundaries corpus matched and retained
2/3 decision success and 3/3 evidence success.

The separate LongMemEval retrieval follow-up is complete in `7689e736`.
Both scoped regressions had focused RED → GREEN tests. On two restores
of frozen case `f43926d9`, the same ordered context kept five evidence
items and 3,598 tokens after the 4,096-token limit. The full native v9
rollout accepted all four repeat captures with no losses or hard failures.
See [the B5 eval report](../../evals/LONGMEMEVAL_V2_SMALL_WEB_BASELINE.md#b5-deterministic-retrieval-regressions).

## Task B6. Decide whether historic repair is warranted

- [x] Recover one reproducible misattribution on pre-change state through a locally generated or approved backup fixture. Record the impacted record set with existing owner interfaces, exact revision history and downstream evidence effect.
- [x] If reproduced, draft a separate spec with a public dry-run report, an idempotent append-only repair, prior-revision replay and a verified recovery procedure. Require explicit review before creating a repair interface or modifying migrations. A current-path fix alone does not prove historic data requires repair.


The B4 read-admission failure involved correctly stored attribution. A
separate B6 experiment recovered a durable false relation and graph
links from a synthetic pre-change store. The public read of its restored
copy reproduced both symptoms. The legacy store had no committed
revision provenance. See the [repair spec](../specs/2026-11-24-legacy-evidence-repair-design.md)
for the exact inventory, verified recovery and approval required before
repair implementation.
