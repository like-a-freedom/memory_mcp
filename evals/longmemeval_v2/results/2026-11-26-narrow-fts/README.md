# Full saved-history retrieval: equivalent repairs, acceptance still fails

Native Apple Silicon, 26 November 2026. No model, remote service or LM Studio.
Base: `ab4a06ae56282d92efa1aa574d1a90fedcf23c42`.

## What the changes establish

- Actual embedded SurrealDB 4.0.1 execution plan sorts a narrow tuple
  `(record, fact_id, t_valid, ft_score)`, then LIMIT, then hydrates the existing
  canonical source fields in the same statement. An independent literal wide-SQL
  oracle preserves ordered rows, scores, source/quote/provenance and temporal
  visibility across tied dates/scores, type filters, repeated/missing terms and
  content/index-key matches. Five new contracts; 16 projection tests overall.
- Lexical candidate ranking computes immutable metrics once per record. Reusing
  base/phrase maxima avoids extra full-text passes for fallback decisions; stable
  comparison order and float accumulation order remain unchanged.
- Set-valued lexical operations normalize each distinct raw token once after the
  shared quote/episode preprocessing. Ordered/count-valued tokenization remains
  unchanged. Public repeated-vocabulary control: **30,044 → 54 allocation calls**,
  same exact vocabulary; 128 fixed-seed mixed-script/reference controls agree with
  the ordered-tokenizer set. This is an allocation-count result, not latency,
  allocated bytes, peak-memory or general tokenizer-quality evidence.

No terms/limits, candidate stores, source policy, temporal axes, schema or
fact/source content were reduced. The tests use real databases for SQL semantics;
controlled adapters are not substituted for those storage checks.

## Captures and their limits

Every registration preceded its own capture. See `attempt-ledger.json` and
`summary.json`; raw failures and invalid setup attempts remain in this package.
All 15 full-history process attempts failed to produce a complete context.
These are development diagnoses, not 15 independent quality examples. The same
100-trajectory / 151-episode snapshot was copied whole for each case. Original six
file hashes are unchanged; no reformation or migration was applied. Therefore
these runs also retain the historical store's formation/index state, rather than
proving the behavior of newly formed data. A separate metadata-only
`schema-audit/` confirmed **33/33** current local-profile migrations, all applied;
no static checksum mismatch (dynamic embedding migration008 excluded from raw-script
checksum comparison). Actual fact/edge/episode index definitions are retained.
There is no evidence here of a missing current migration; index presence alone is
not proof of an efficient graph plan. No schema was changed.

Matched optimized-application comparison (`CARGO_PROFILE_TEST_OPT_LEVEL=3`,
otherwise unchanged test profile; not the release/LTO profile):

| Case | Baseline completed SQL sum | Narrow SQL completed sum | Completed SQL | Full context |
| --- | ---: | ---: | ---: | ---: |
| dropdown | 4.655 s | 2.773 s | 7 / 7 | 0 / 0 |
| toolbar | 28.184 s | 9.319 s | 15 / 15 | 0 / 0 |

Each is one observation, fixed baseline-first order. Sums include only completed
SQL calls, not application work or any interrupted query. They support a narrower
SQL-cost observation, not an end-to-end speedup or a latency distribution. CPU
samples are short point observations; their different cases/times cannot establish
a controlled peak-memory improvement. Full-history requests inherit their default
live-clock cutoff; no concurrent store writes occurred. Exact query equivalence
is checked separately at a fixed cutoff, rather than inferred from these timings.

Final original-bound candidate (`unique-token-set/`): source carriers **2/2**;
completed contexts **0/2** within the registered 30 s read limit. Cooperative
failures return at 30.092 / 30.126 s; full process including initialization/drop
ends at 44.114 / 43.614 s. Dropdown completed 89 SQL calls; toolbar 24. Missing
candidate/selection/packing receipts remain **unknown**, not zero recall.

The separately registered `complete-stage-diagnostic/` raises only its diagnostic
read/process bounds to 120/150 s. It is **not acceptance** and also produces
**0/2** contexts: observed read failures at 129.927 / 132.537 s, processes exit101
at 148.222 / 145.572 s. This also demonstrates cooperative timer overrun, not a
hard 120 s deadline. Dropdown reaches 101 completed SQL, toolbar109; completed SQL
sums15.141 /26.343 s. Most elapsed time cannot be assigned to SQL from this observer.
The outer kill/wait bound remains necessary for embedded CPU work.

Archived seven answers remain **2/7**; no new answers or reader/judge calls.
C1 stays stopped; these exposed diagnostics do not admit C2.

## Reproduction

The generic integration target is `crates/memory-mcp/tests/full_history_evidence.rs`.
Provide a **disposable full copy** of the pinned store as
`MEMORY_FULL_HISTORY_STORE` and a registration as `MEMORY_FULL_HISTORY_INPUT`.
Build with `cargo test --locked -p memory_mcp --features eval-support --test
full_history_evidence --no-run --message-format=json`, pin a copy of the reported
executable, and invoke each named test with `--ignored --nocapture` under an
external process deadline that kills and waits the owned child. This needs the
registered historical input store; the archive contains hashes, not a secretly
reduced replacement. Copying/opening must never mutate the original store.

Routine no-model regression checks:

```sh
cargo test --locked -p memory_mcp --features eval-support --test lexical_term_set_resource --test retrieval_query_projection --test retrieval_query_batching --test long_history_stage_evidence
```

Historical `capture-command.py` files are invocation records with task-local paths,
not a second supported runner or portable entry point. Product and harness retain
the existing public commands. See `verification.json` for final fresh checks and
source identities; capture registrations retain their original source hashes.

## Next repair, in order

1. Preserve partial stage receipts on timeout and capture late application CPU
   work under one registered deadline. A completed-query sum cannot identify the
   remaining CPU stage. Keep source-linked boundary items and timing separately.
2. Existing episode fallback sorts in `budget.rs` and historical `pipeline.rs`
   recompute full lexical scores in comparisons. Test exact ordered results with
   duplicates/ties and an observable bounded-work contract, then reuse immutable
   scores once per episode. Do not drop fallback/graph coverage to hit a timer.
3. Schema inventory is checked:33/33 current migrations and actual indexes
   present. Check actual graph plans before proposing new indexes.
   Many completed edge calls are not proof of missing indexes or the sole delay.
   Any schema change needs its own source/temporal equivalence and migration gate.
4. One newly registered bounded capture after the diagnosed repair must return
   both reviewed blocks and complete stage receipts under the original acceptance
   budget, then pass the eight-case native gate. Only after that, separately
   register the unchanged seven reader/judge tasks locally and report source
   coverage, values, citations, failures and latency independently.

Final checks: production2810pass/0fail/45ignored; harness929pass/0fail/22ignored;
opt-in targets41pass. Native8/8, no regressions,0invalid/unobserved; strict workspace
and diagnostic clippy/fmt pass. Counts include existing architecture lints and
doctests; ignored cases are not passed. Native gate ran alongside ordinary
regression checks and is not used for a latency comparison. Intermediate adapter,
source-name inventory and helper-layout failures are preserved under `tdd/`;
no architecture rule was disabled to obtain the final result.
