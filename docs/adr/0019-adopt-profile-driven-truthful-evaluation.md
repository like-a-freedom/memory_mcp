# ADR-0019: Adopt Profile-Driven Truthful Evaluation

> Status: Accepted (2026-07-29)
> Related: ADR-0017 (agent-memory lifecycle release evidence)

## Context

Evaluation currently consists mainly of independent ignored integration tests
and Makefile targets. The suites do not share one outcome model, orchestration
contract, artifact schema, or execution budget.

Several current behaviors can overstate quality: empty suites aggregate to
perfect values, some runners print failures without failing, retrieval seeding
mixes production extraction with direct insertion of oracle text, and proxy
evals are presented alongside wired system measurements. External evaluation
also takes hours because strict gating is coupled to serial execution and
expensive end-to-end setup is repeated where a retrieval-only measurement is
intended.

We need fast pull-request feedback, a release-quality result within 20 minutes,
and broad end-to-end diagnosis without weakening the meaning of any metric.

## Decision

Create a private Rust workspace crate named `eval-harness` and make it the
single manifest-driven orchestration boundary for evaluation.

The harness defines:

1. Three profiles:
   - `pr`, targeting at most 10 minutes;
   - `release`, targeting at most 20 minutes with complete declared
     retrieval-only corpus coverage and deterministic sharding;
   - `nightly`, covering full end-to-end and diagnostic work with a budget set
     after profiling.
2. Explicit modes: `retrieval-only`, `end-to-end`, `lifecycle`, and
   `performance`. Headline metrics from different modes are never merged.
3. Exactly three case outcomes: `passed`, `quality_failed`, and `invalid`.
   Missing, malformed, incomplete, or failed measurement is invalid and remains
   in coverage.
4. A versioned JSON artifact containing all outcomes, metrics, gates, coverage,
   selected IDs, durations, retries, and environment/configuration
   fingerprints. Human output is derived from this artifact.
5. Deterministic, bounded scheduling and stable hash sharding. Strictness is a
   gate policy, not a concurrency setting.
6. Post-run gates combining use-case-derived hard floors with a reviewed
   regression budget against an approved baseline artifact.

Retrieval-only evaluation imports canonical provenance-bearing facts through a
private eval adapter and then exercises production retrieval. End-to-end
evaluation uses only production ingest, extraction, reconciliation, and
retrieval paths. Direct insertion of oracle text is prohibited in end-to-end
mode.

The harness and its dependencies are excluded from the shipped binary.
Dataset-specific parsers sit behind one corpus adapter seam. Existing
`tests/eval_*.rs` files may temporarily delegate to the harness and are removed
after parity. The Makefile becomes a thin adapter. Performance measurement uses
Criterion under `benches/`; ordinary `cargo test` remains separate.

Lifecycle evaluation fulfills the wired evidence gate defined by ADR-0017
rather than creating a new lifecycle decision.

## Consequences

- Evaluation failures become distinguishable from infrastructure-invalid runs
  without treating either as success.
- The release time target is pursued through clean mode separation, bounded
  concurrency, and sharding rather than hidden case removal.
- Retrieval quality can be isolated quickly while nightly end-to-end runs
  retain extraction and reconciliation coverage.
- A new private crate and typed artifact schema add initial migration work.
- CI and local commands must consume profiles instead of maintaining separate
  suite lists.
- Baseline updates, threshold changes, and frozen test-split changes require
  review and before/after artifacts.
- Downstream reader QA remains a separate diagnostic until its model, prompt,
  parameters, provider version, and evaluator are pinned. The former unpinned
  placeholder suite is not registered; a pinned implementation may be added as
  a new decision-backed suite later.

## Alternatives Considered

### Keep ignored integration tests and improve the Makefile

Rejected because it preserves fragmented policy, untyped stdout comparison, and
evaluation-only dependencies in the main crate.

### Run only full end-to-end evaluation

Rejected because it is too slow for release feedback and makes retrieval
regressions difficult to isolate.

### Run only canonical-fact retrieval

Rejected because extraction, claims, and lifecycle behavior still require
truthful end-to-end evidence.

### Meet the time budget by sampling or skipping silently

Rejected because runtime would improve by changing the measured population.
Samples and shards must be explicit, stable, and recorded.

## Verification

The decision is satisfied when the acceptance criteria in
`docs/superpowers/specs/2026-07-28-truthful-evaluation-system-design.md` are
met, including complete artifact coverage, separate mode metrics, wired
lifecycle evidence, and the declared PR and release budgets.
