# Retrieval Final Comparison (Blocked)

**Date:** 2026-05-01
**Status:** ⚠️ Blocked — baseline artifacts are missing, so numeric before/after deltas cannot be computed honestly.

## Baseline recovery status

The original Task 0 / Task 5 plan expects baseline and final output files under `docs/superpowers/plans/baselines/`.

What was checked:

- the `docs/superpowers/plans/baselines/` directory in the repository;
- the current chat transcript at `.../transcripts/13fc892d-d825-486c-8ecb-bd25e79f8af2.jsonl`;
- the current session resource files under `.../chat-session-resources/13fc892d-d825-486c-8ecb-bd25e79f8af2/`;
- the current session debug log under `.../debug-logs/13fc892d-d825-486c-8ecb-bd25e79f8af2`.

Result: no restorable baseline metric artifact was found. Because of that, this report captures verified post-refinement evidence only and leaves true deltas as unavailable.

## Verified post-refinement evidence

### Internal retrieval eval

Verified from:

- `cargo test --test eval_retrieval run_retrieval_evals -- --ignored --nocapture --test-threads=1`

Observed summary:

| Metric | Final |
|--------|-------|
| total | 66 |
| passed | 66 |
| recall_at_5 | 1.00 |
| mrr | 0.99 |
| top1_hit_rate | 0.98 |
| diversity_pass_rate | 1.00 |
| pass_rate | 1.00 |

Tagged slices added by this refinement:

| Tag | Total | Passed | Pass Rate |
|-----|-------|--------|-----------|
| timeline_auto | 1 | 1 | 1.00 |
| graph_anchor | 1 | 1 | 1.00 |
| first_person_rescue | 1 | 1 | 1.00 |

### Broad eval smoke sweep

Verified from the workspace task `eval-quick-sweep`.

Observed results:

- `eval_retrieval`: `44 passed; 0 failed; 1 ignored`
- `eval_external_provenance`: `39 passed; 0 failed`
- `eval_external_full_datasets`: `40 passed; 0 failed; 4 ignored`
- `eval_extraction`: `4 passed; 0 failed; 1 ignored`
- `eval_latency`: `3 passed; 0 failed; 1 ignored`
- `eval_document_ingest`: `suite=eval_document_ingest total=8 passed=8`

These runs confirm the post-refinement suite remains green, but they do not reconstruct the missing pre-change baseline numbers.

## Delta tables

### Internal retrieval

| Metric | Baseline | Final | Delta | Pass? |
|--------|----------|-------|-------|-------|
| recall_at_5 | unavailable | 1.00 | n/a | ⚠️ baseline missing |
| mrr | unavailable | 0.99 | n/a | ⚠️ baseline missing |
| top1_hit_rate | unavailable | 0.98 | n/a | ⚠️ baseline missing |

### New tagged slices (post-refinement only)

| Tag | Total | Passed | Pass Rate |
|-----|-------|--------|-----------|
| timeline_auto | 1 | 1 | 1.00 |
| graph_anchor | 1 | 1 | 1.00 |
| first_person_rescue | 1 | 1 | 1.00 |

### External — LongMemEval (full)

| Metric | Baseline | Final | Delta | Pass? |
|--------|----------|-------|-------|-------|
| recall_at_5 | unavailable | unavailable in this artifact | n/a | ⚠️ baseline missing |
| mrr | unavailable | unavailable in this artifact | n/a | ⚠️ baseline missing |
| top1_hit_rate | unavailable | unavailable in this artifact | n/a | ⚠️ baseline missing |

### External — LoCoMo (full)

| Metric | Baseline | Final | Delta | Pass? |
|--------|----------|-------|-------|-------|
| recall_at_5 | unavailable | unavailable in this artifact | n/a | ⚠️ baseline missing |
| mrr | unavailable | unavailable in this artifact | n/a | ⚠️ baseline missing |
| top1_hit_rate | unavailable | unavailable in this artifact | n/a | ⚠️ baseline missing |

### Extraction eval

| Metric | Baseline | Final | Pass? |
|--------|----------|-------|-------|
| suite status | unavailable | pass | ⚠️ baseline missing |

### Latency eval

| Metric | Baseline | Final | Pass? |
|--------|----------|-------|-------|
| suite status | unavailable | pass | ⚠️ baseline missing |

## Summary

- Regressions: none observed in currently re-run post-refinement suites.
- Improvements: new tagged internal retrieval slices (`timeline_auto`, `graph_anchor`, `first_person_rescue`) are covered and pass.
- Blocker: missing Task 0 baseline artifacts prevent a truthful before/after delta report.
- Verdict: ⚠️ NEED BASELINE RECOVERY OR USER DECISION
