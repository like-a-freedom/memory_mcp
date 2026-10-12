# Plan: Architecture deepening — 8 behavior-preserving refactors

Date: 2026-10-11
Worktree: `.worktrees/perf-harness-core` (branch `perf-harness-core`)
Status: **verified 2026-10-11 against HEAD `c0e11230` — all line references
re-read from source; 7 review findings fixed in this revision (see
"Verification findings" at the end).**

## Goal

Eliminate measured architecture/cyclomatic-complexity friction found in the
2026-10-11 review, **without any behavior change**. Every item is a pure
refactor guarded by existing tests (item 7 needs new refusal tests first,
written TDD-style before the refactor).

## Constraints (hard)

- **No behavior change**: same SQL, same log ops/fields/levels, same error
  strings, same ordering of side effects, same public signatures.
- AGENTS.md boundaries: no `unwrap()` in production, no new business logic in
  `src/service/` (item A3 only reorganizes the existing `FactService` body),
  no new dependencies (Cargo.toml is "ask first"), ADR-0058 shims untouched.
- No params-struct / signature migrations at 100+ call sites (that would be a
  new interface — explicitly out of scope).
- Do not re-suggest anything in the ledger's rejected list (observer-tax,
  log-event inflation, track_fact_accesses batching, map-view N+1, key-case
  unification, iterations 3→5, cargo run → binary).

## Strategy: ordered rounds, one commit per item

Crates are independent (`memory-mcp` vs `eval-harness`), but a single
worktree + shared cargo target lock means builds serialize anyway; parallel
subagent *editing* risks conflicting fmt/clippy runs. Decision: **sequential
TDD rounds in priority order**, each ending in a commit, one-item-one-diff.

Order (priority = leverage × guard strength):

| # | Item | Crate | Commit |
|---|------|-------|--------|
| 1 | `reembed::process_reembed_batch` state helpers | memory-mcp | A1 |
| 2 | `fact_extraction` dedup (triad, query×3, loader×3, edge fns) | memory-mcp | A2 |
| 3 | `FactService::add_fact` phase methods | memory-mcp | A3 |
| 4 | `extract_entities` scheduling split | memory-mcp | A4 |
| 5 | `verdict::evaluate` gate helper | eval-harness | B1 |
| 6 | `scenarios.rs` timed-helper + id helper | eval-harness | B2 |
| 7 | `tools::extract_inner` — **TDD**: refusal tests first, then refactor | memory-mcp | C1 (tests) + C2 (refactor) |
| 8 | `episode.rs` test relocation + embedded-DB test → `tests/` | memory-mcp | D1 |

Rationale for the order:
- #1–#4 are the strongest (measured 140–220-line functions, 10–11-arg
  explosions, verbatim duplicated blocks) with the deepest guard suites.
- #5–#6 are harness-only; branchiest file in the harness (#5) and the
  6×-duplicated timing idiom (#6). Both have on-point guards
  (`build_covers_every_scenario_id`, ratio-message substring asserts).
- #7 is weaker-guarded: the three `"Invalid extract arguments…"` messages are
  asserted by **no test** — per AGENTS.md, tests come first (RED commit C1),
  then the refactor (GREEN commit C2). The duplicated success tail is safe to
  dedup once tests pin both paths (stored + inline already covered by
  `tools_extract_reporting` / `tools_stored_extract_reporting`).
- #8 last: pure churn (move tests to owner modules; one embedded-DB unit test
  to `tests/` per AGENTS.md test-tier rule). Doing it last avoids re-basing
  every earlier diff on a moving `episode.rs`.

## Per-item specification

### A1 — reembed pass-state helpers (`service/reembed.rs`)

Problem evidence (verified): `process_reembed_batch` ~220 lines (~475–694),
7 args + `#[allow(too_many_arguments)]` with an explanatory clippy comment;
`persist_reembed_job` ~11 params × 6 call sites (most args re-derived from
`pass` each time); `update_namespace_progress` 8 params × 4 sites; Ok/Err
arms duplicate bump→progress→persist (the **log payloads differ**:
`reembed.cursor_advanced` vs `reembed.fact_failed` — logs stay per-arm).

Changes (behavior-preserving):
1. **Service-side** private helpers on the same `impl` (not on `ReembedPass` —
   `persist_reembed_job` is `&self`, a struct method cannot call it):
   - `async fn persist_pass_job(&self, pass, status, finished_at, error)`
     — absorbs the 11-arg re-derivation (`summary`, `target_signature`,
     `target_dimension`, `namespace_progress`, `started_at_rfc3339`,
     `started_at.elapsed()`, job id …). All 6 call sites pass the same
     subset they pass today, in the same order — byte-identical persist
     payloads.
   - `fn update_pass_namespace_progress(pass, ns_state)` — wraps the 8-arg
     `update_namespace_progress` free fn (free fn, so this one can live as a
     free fn or method; either is fine).
   - `NamespaceBatchState::record_success()` / `record_failure(fact_id)` —
     counter bumps only (including the double push into
     `pass.summary.failed_fact_ids` + `ns_state.failed_fact_ids` — the two
     lists are distinct; preserve both).
2. Private fns: `filter_retry_batch` (the `--retry-failed` filter, verbatim),
   `handle_rewrite_failure` (Err arm minus the quota abort, which keeps its
   own block because it interleaves `write_embedding_state` +
   `persist_job(failed)` + 12-field log + `progress.on_job_completed`).
   The Ok arm shrinks to counter bumps + two helper calls + the
   `reembed.cursor_advanced` log.
3. Do **not** introduce a shared log-payload constructor — the 16
   `HashMap::from([...])` literals have materially different shapes
   (3, 4, and 12 fields). Unifying them is a parity trap with no guard.
   (Rejected in revision: original plan item A1.3.)
4. Keep `#[allow(too_many_arguments)]` + its comment on
   `process_reembed_batch` only if the signature is unchanged; drop the
   allow only if the refactor genuinely reduces the arg count.

Guards: `cargo test -p memory_mcp reembed` (11 in-file) + `cargo test -p
memory_mcp --test http_embedding_reembed`. Log-op/field parity: every op
string and payload key stays byte-identical — visible in diff.

### A2 — fact_extraction dedup (`memory/episode/fact_extraction.rs`)

Verified facts: heuristic triad ~144–187 (metric / promise||action-item /
experience — three blocks, promise predicate is a two-part OR, computed from
`normalized` and `content` respectively); `RelationsForFactsQuery` appears at
**three** sites, not two — detect-contradiction (~250, `.await?`), the
evidence-exposure block (~478, `.await.unwrap_or_default()`), and
detect-contradiction loads fact records too; fact-record loader chains
(`find_fact_record` + `fact_from_value_or_wrapper`) at **three** sites
(296, 319, and inside the `involved_in` edge loop ~405); the two
`log_event` blocks (~95–111 structured_summary, ~191–206 note_fallback) share
shape; duplicated doc-comment line before the reconciliation block (~466).

Changes:
1. `push_heuristic(service, episode, fact_type, enabled, strategy)` replaces
   the three 14-line blocks; `enabled` is the precomputed predicate (for the
   promise case the caller passes the OR — short-circuit order of the two
   predicate calls is preserved at the call site). Call sites become ~2
   lines; iteration order unchanged (metric → promise → experience → note
   fallback).
2. Shared `async fn claim_relations_for_facts(service, fact_ids) ->
   Result<Vec<Relation>, MemoryError>` used at all query sites. **Error
   handling stays per-site**: detect-contradiction keeps `.await?`, the
   evidence-exposure block keeps `.await.unwrap_or_default()` on the Result
   (swallow errors → empty vec, same as today). Do not hoist to one call —
   each site's DB read keeps its current position in the side-effect order.
3. Shared `fn fact_content_from_record(record) -> Option<String>` for the
   loader chains; call sites keep their distinct error semantics
   (`.ok().and_then(...)` → empty string in detect-contradiction; `?` +
   `continue` in the edge loop).
4. `store_mentioned_in(service, episode, entities, t_valid)` /
   `store_involved_in(service, episode, facts, t_valid)` private fns for the
   two edge loops — same field literals (strength 1.0/0.9 vs 0.8/0.85),
   same order, same `?` propagation.
5. Single shared helper for the two identical `log_event` shapes — same op
   names/fields/levels; op string stays a parameter.
6. Delete the duplicated doc-comment line (~466). Keep
   `detect_contradiction_warnings` (names a real step — the name is
   locality; deletion would move, not concentrate).
7. Do NOT merge `should_extract_note_fact` with the reporting predicate in
   `build_extract_log_result` — preconditions legitimately differ; add a
   doc-comment cross-reference only.

Guards: `cargo test -p memory_mcp fact_extraction` + `should_extract_note_fact`
+ `build_extract_log_result` + `structured_summary` (31 in-file);
`--test service_integration extraction` (contradiction warning path);
`--test memory_extraction`.

### A3 — FactService::add_fact phases (`service/fact_orchestration.rs`)

Verified: `add_fact` 10 args, `#[allow(too_many_arguments)]`; embedding match
(three arms: Generated / Skipped / Err) with **three materially different
log payloads** (`embedding.generate.success` 3 fields, `embedding.write_skipped`
with `reason` only, `embedding.write_skipped` with provider+error+fact_type);
claim projection block with Ok/Err arms already calling
`record_post_fact_success/failure`.

Changes:
1. Private methods matching the module's own phase list:
   `prepare_index_keys`, `generate_embedding_fields`, `persist_fact`,
   `run_post_persist`. `add_fact` body becomes ~35 lines reading like the
   module doc. Same call order, same awaits.
2. **The three log statements stay verbatim inside `generate_embedding_fields`** —
   do NOT unify them into one constructor (different op/field sets; the
   Skipped/Err same-op-different-fields quirk is preserved exactly).
   (Rejected in revision: original plan item A3.2's shared log helper.)
3. `project_claims(...)` private fn absorbs the clone-staging block (the
   `if outcome.created { ... }` body) — Ok/Err arms preserved verbatim.
4. Leave `MemoryService::add_fact` (core.rs) untouched — retiring it touches
   ~97 test lines; out of scope (recorded as future churn decision).

Guards: `cargo test -p memory_mcp add_fact` (in-file core.rs + fact_orch) +
`--test service_integration add_fact` + `--test claim_reconciliation_e2e`.

### A4 — extract_entities split (`memory/episode/entity_extraction.rs`)

Verified: scheduling match (~34–78) owns spawn_blocking/block_on and the
correlation-id re-entry (existing comment moves to the new fn's doc); error
path logs `log_ner_error` then `?`; resolve loop (~106–144) owns the
`inspect_err` `ner.resolve.error` logging closure.

Changes (2 private fns + slim `extract_entities`, same file):
1. `run_ner_extraction(extractor, scheduling, content, labels) -> Result<...>`
   — the scheduling match verbatim, including error mapping
   (`entity extraction task panicked: {err}`).
2. `resolve_and_dedupe(service, provider, candidates) -> Result<Vec<...>>` —
   dedupe + resolve loop incl. the `inspect_err` closure unchanged
   (`provider` passed explicitly so the log result keeps its field).
3. `extract_entities` = run → error-log → done-log → resolve →
   `persist_extraction_projection`; ~35 lines linear.
No change to `dedupe_entity_candidates` internals (weak guard — 1 test —
left alone; recorded as future test-then-refactor).

Guards: `cargo test -p memory_mcp extract_entities_does_not_block_runtime`
(the ticker test — primary regression detector) + `--test memory_extraction`
+ `--test service_integration extraction`.

### B1 — verdict::evaluate (`eval-harness/src/perf/verdict.rs`)

Verified: preconditions return `Invalid` early (host_tag → platform →
digest, three distinct messages); per-gate loop filters records by
`{scenario}-` prefix; `beyond` match dispatches on `gate.direction` and uses
**inverted fractions** (LowerIsBetter: `ratio(current, base)`;
HigherIsBetter: `ratio(base, current)`, plus a special case
`current <= 0.0 → regressed(INFINITY)` *before* the ratio call); a second
direction match recomputes the ratio for the message; `invalid` wins over
`regressed`; message closure format
`"{metric_key} ratio {ratio:.3}>{max_ratio} (current …, baseline …)"`.

Changes:
1. `fn gate_verdict(direction, current, base, max_ratio) -> GateCheck` where
   `GateCheck` is a private enum `Beyond(f64) | Within | Undefined(&'static
   str /* "baseline" | "current" */)`. **The inverted fraction and the
   `current <= 0.0` INFINITY special case live inside this fn, byte-for-byte
   the current semantics** — the second direction match at the call site
   disappears because `Beyond` carries the already-computed ratio.
2. `fn host_mismatch(baseline, current) -> Option<String>` — first-difference
   wins, same three messages, same check order (host_tag, platform, digest).
3. `fn check_record(...) -> Option<RecordIssue>` for the per-record body;
   `evaluate` = preconditions → loop gates → precedence. `continue`
   semantics mapped to `return None` (record ok) / `Some(issue)`.
4. Precedence (`invalid` before `regressed`) and the `min_samples` /
   missing-metric early `continue`s preserved exactly.

Guards: `cargo test -p eval-harness verdict` (11 tests, incl. substring
asserts on `1.3`, `130`, `100`) + `--test perf_cli baseline` +
`--test perf_runner`.

### B2 — scenarios timing idiom (`eval-harness/src/perf/scenarios.rs`)

Verified: timing idiom (`Instant::now` → await → push latency /
`invalid_reasons.push(format!("{label}: {err}"))`) repeated ~6×; reason
strings carry per-arm labels (`extract`, `assemble`, `resolve`, `explain`,
`invalidate`, and the setup-extract arm uses `assemble setup extract
{episode_id}`); `perf_scenarios` tests assert `invalid_reasons.is_empty()`
on success and print reasons only via `{:?}` in failure messages — **no test
parses reason text**, but the strings are still observable artifact content,
so they stay byte-identical. `set_phase` calls interleave with the timed
work and are baseline-gated (catalog recorded).

Changes:
1. Private `async fn timed<F, Fut>(ev, label, fut)` — one place for
   Instant/await/push. Reason format stays `"{label}: {err}"`; call sites
   pass the exact current labels. The setup-extract arm keeps its current
   error text (call site composes the label or keeps its own match — if the
   composed label would alter the string, keep that one arm inline).
2. `fn perf_source_id(scenario, scale, iteration, i) -> String` — one copy
   of the format (used by IngestScale + AssembleScale).
3. `"fixture produced no facts"` guard: only merge the arms that load the
   fixture the same way *before* any `set_phase`; an arm whose guard fires
   at a different position relative to `set_phase` keeps its own copy
   (position is baseline-visible).
4. Keep every `set_phase` call exactly where it is relative to the timed
   work.
5. Do not touch the `ScenarioRun` trait shape.

Guards: `cargo test -p eval-harness perf_scenarios` (incl.
`build_covers_every_scenario_id`, source-id uniqueness test) + `--test
perf_runner` (phase-split tests) + `make perf-smoke` at the end of the
harness batch.

### C1+C2 — tools::extract_inner (`crates/memory-mcp/src/tools/extract.rs`)

Verified order and messages: (1) `content && text` → dual-field message,
record `has_content: true`; (2) `episode_id && inline` → dual-source
message, `has_content: true`; (3) neither → neither message,
`has_content: false`. All three: op `extract.invalid_input`, level Warn,
args `{"episode_id": …, "has_content": …}`. Success tails (stored ~98–121,
inline ~176–199) are near-verbatim: `record_extract_results` →
`success()` → `find_episode` (Err arm still builds a log result with
`None`) → `extract.done` record → `extract_guidance` → return. **Error
tails are NOT identical**: stored-path error records args
`{"episode_id": episode_id}`; the two inline-path errors record args `{}`.

C1 (RED, own commit): refusal tests pinning exact messages and check order —
- `content` + `text` → dual-field message;
- `episode_id` + `content` → dual-source message;
- no source → neither message;
- **order-pinning case**: `episode_id` + `content` + `text` → dual-field
  message wins (first check), not dual-source.
Location: new `tests/tools_extract_validation.rs` (reporting tests stay
about the done-tail).

C2 (GREEN, own commit):
1. `fn validate_extract_inputs(episode_id, content, text) ->
   Option<ExtractInputError { message, has_content }>` — the three checks in
   the exact current order; one recording site at the call site emits the
   identical `extract.invalid_input` event with the per-check `has_content`.
2. `async fn record_extract_done(...)` — dedups only the **success** tails
   (verbatim: same find_episode fallback, same args
   `{"episode_id": …}`, same guidance). Error arms stay per-site with their
   distinct args.
3. `extract_inner` 198 → ~130 lines.

Guards: C1 tests + existing `tools_extract_reporting` (3) +
`tools_stored_extract_reporting` (1) + `--test memory_extraction`.

### D1 — episode.rs test relocation

Verified: `pub(crate) use fact_extraction::{build_extract_log_result,
extract_from_episode};` at `episode.rs:14` must survive (public_surface_audit
re-derives inventory). The ticker test constructs
`MemoryService::new(...)`, assigns `service.entity_extractor = Arc::new(...)`
(pub field — re-verified reachable from an integration test), uses
`SurrealDbClient::connect_in_memory` + `apply_migrations`, and
`crate::memory::capabilities::deps::ExtractDeps::from(&service)` — for the
`tests/` move these paths must be reachable as
`memory_mcp::...`. **Precheck is part of D1**: if any of the three
(`entity_extractor` field write, `ExtractDeps`, `NerScheduling`) is not
publicly reachable, fall back per the test-tier rule's intent — move the
test to `entity_extraction.rs`'s `#[cfg(test)] mod tests` (in-crate, but at
least owned by the right module) and record the deviation in the ledger.
The `collect_connected_entity_component_uses_neighbor_queries_instead_of_edge_scan`
test uses crate-internal `DbClient` mocking — it moves to `communities.rs`
tests (in-crate), NOT to `tests/`.

Changes:
1. Move each `#[cfg(test)]` test in `episode.rs` to the `#[cfg(test)] mod
   tests` of the file that owns the function under test
   (`fact_extraction.rs`, `entity_extraction.rs`, `summary_parser.rs`,
   `communities.rs`, `edges.rs`, `statement_detection.rs`, `triples.rs`) —
   or to `episode/<mod>/tests.rs` where the owner file would bloat. Test
   bodies unchanged.
2. Move the ticker test to `crates/memory-mcp/tests/` after the visibility
   precheck (fallback above). Keep the same scenario, ticker, and
   assertions.
3. Preserve `pub(crate) use` re-exports at `episode.rs:14`.
4. `episode.rs` shrinks to module decls + accurate doc comment.

Guards: `cargo test -p memory_mcp episode` (name-filter still matches new
paths), then full `cargo test -p memory_mcp`; `--test public_surface_audit`.

## Per-item workflow (TDD-strict, matches repo practice)

For pure refactors with existing guards (A1–A4, B1–B2, D1):
1. Implement refactor.
2. Run the item's narrow guard set.
3. `cargo fmt --all` + item-crate clippy (`-p memory_mcp` or `-p
   eval-harness`, `--all-targets`, the AGENTS feature set, `-D warnings`).
4. Commit with a `refactor(<scope>):` message describing the deepening.
5. Ledger entry in `.superpowers/sdd/2026-10-10-perf-harness-core/progress.md`
   (append; item, commits, guard results, any deviations).
6. Self-review the commit diff before the next item starts.

For item 7: RED commit (C1 — new tests pass and pin current behavior) →
implement → GREEN commit (C2).

## Batch validation (after each crate's last item, and at the end)

After A4: `cargo test -p memory_mcp` (full, with `--features eval-support`),
workspace clippy `-D warnings`, fmt check.
After B2: `cargo test -p eval-harness` (full), `make perf-smoke`.
After D1: full `cargo test -p memory_mcp` again (test paths moved).
Final: `make perf-weekly` (must stay exit 0 — refactors must not move any
timed SQL; if weekly moves, the refactor leaked a behavior change — stop and
bisect).

## Code review (user-mandated, right after implementation)

Per skill `code-review` (Standards + Spec axes, two parallel sub-agents,
side-by-side report), run **once on the whole branch range**
(`c0e1123..HEAD`) after D1:
- Standards axis: AGENTS.md (no unwrap, no new deps, main.rs thin, no
  business logic in service beyond reorganizing existing, error variants,
  logging conventions — ops/fields/levels unchanged), rust-skills basics.
- Spec axis: each of the 8 items' promised behavior-preservation (diff-read
  for op strings, error strings, SQL builders, side-effect order, phase
  wiring).
- Additionally self-review each commit diff before the next item starts
  (cheap first pass); the thorough review is the end-of-branch pass.
- Findings fixed in follow-up commits (never amend, per repo rules); re-run
  affected narrow guards + clippy after fixes.

## Risks and mitigations

- **A1 persist-payload drift**: `persist_pass_job` must reproduce every
  argument currently passed (job id `REEMBED_JOB_ID`, `finished_at` per
  status). Mitigation: read all 6 call sites side by side during the edit;
  guards + diff review of persisted JSON fields.
- **A2 error-handling asymmetry** (`.await?` vs `.unwrap_or_default()`):
  the shared query helper must not erase it. Mitigation: call-site error
  handling explicitly specified above.
- **Reason-string drift in B2**: labels stay byte-identical; the
  setup-extract arm keeps its composed message. No test parses reasons, but
  the artifact is operator-visible.
- **Phase wiring leak in B2**: moving `set_phase` relative to work would
  change the baseline catalog. Mitigation: keep call sites in place; only
  dedup the timing idiom.
- **D1 visibility**: ticker test needs pub reachability of
  `entity_extractor` / `ExtractDeps` / `NerScheduling` from `tests/`;
  precheck first, in-module fallback second. `public_surface_audit` in
  guards.
- **Cargo target lock**: builds serialize; expected, no mitigation needed.

## Verification findings (this revision, all fixed above)

1. **A1 design error (blocking)**: plan had `ReembedPass::persist_job`
   calling `persist_reembed_job` — impossible, the latter is `&self` on the
   service. Fixed: service-side `persist_pass_job(pass, …)`.
2. **A1 log-helper overreach**: 16 log literals have 3/4/12-field shapes;
   a shared constructor is a parity trap. Fixed: removed (item 3).
3. **A2 count error**: `RelationsForFactsQuery` at 3 sites (not 2) with
   **different error handling** (`.await?` vs `.unwrap_or_default()`); loader
   chains at 3 sites (not 2). Fixed: item 2/3 rewritten.
4. **A3 log-helper overreach**: three embedding log arms have different
   op/field sets; unifying is unsafe. Fixed: logs stay verbatim (item 2).
5. **B1 semantics gap**: plan omitted the inverted fraction for
   HigherIsBetter and the `current <= 0.0 → INFINITY` special case. Fixed:
   explicitly inside `gate_verdict`.
6. **C error-tail asymmetry**: stored-path error records
   `{"episode_id": …}`, inline-path errors record `{}` — success tails only
   are shared. Fixed: C2.2 scoped; C1 gains an order-pinning case.
7. **D1 reachability risk**: ticker test's crate-internal dependencies may
   not be publicly reachable; visibility precheck + fallback added; the
   NeighborOnlyDbClient test stays in-crate (`communities.rs`), not `tests/`.

## Out of scope (recorded, not done)

- `MemoryService::add_fact` retirement (~97 test call sites) — future churn
  decision.
- `dedupe_entity_candidates` internals — needs more tests first (1 guard).
- `startup::decide_embedding_startup` (89 lines, 12 branches, 9 tests) —
  strong candidate but lower leverage than the 8; defer to a follow-up round.
- `explanation::explain` (220 lines) — own round, not now.
- Single-variant enums (`RecoveryCycleOutcome`, `BackfillOutcome`) —
  cosmetic; skip (YAGNI).
- `QueryClass::as_str` strum migration — new dep, "ask first"; skip.

## Deliverables

- 9 commits (A1–A4, B1–B2, C1, C2, D1) on `perf-harness-core`.
- Ledger entries per item.
- End-of-branch code-review report + fix commits for any findings.
- Confirmation: `make perf-weekly` exit 0 (behavior/latency parity proof).
