# ADR-0025: Single Formula Home for Evaluation Metrics

> Status: Accepted (2026-07-30)
> Guards the ADR-0019 / ADR-0020 evaluation truth layer against metric drift.

## Context

ADR-0019 established typed `MetricEvidence` as the single source of truth for a
case's measured state; the reducer derives suite aggregates from evidence, and
the gate layer compares against floors.

The current eval-harness still carries the pre-evidence computation path:
each suite (e.g. `suites/retrieval.rs`) computes per-case floats
(`recall_at_k`, `mrr`, `top_1_hit_rate`), writes them into a
`BTreeMap<String, f64>` keyed by hardcoded strings (`"recall_at_5"`), *and*
constructs typed `MetricEvidence::retrieval(…)` for the same outcome. The
reducer (`reducer.rs`) then recomputes the same three quantities from the
evidence and emits suite-level keys via `format!("recall_at_{}", cutoff)`.

Two computation paths produce the same numbers via different code, keyed
differently. This is the exact shape the v3 truth layer remediation was
fighting (float copies, dropped child metrics, stringly key drift), and 13 of
the last 80 commits touched this area — debt is still accruing.

## Decision

Suites produce **typed evidence only**; all metric arithmetic and all metric
key naming move behind the reducer/metrics interface.

1. A case outcome carries evidence; it does not carry per-case metric floats.
2. Per-case diagnostic values (e.g. the numbers shown in failure reports) are
   rendered from evidence through the *same* code path that produces the
   aggregate.
3. Metric key naming (e.g. `recall_at_5` vs `recall_at_{cutoff}`) is a
   function of evidence type + cutoff inside `metrics.rs` — suites never
   construct metric keys.
4. `EvalCaseOutcome.metrics` becomes derived-at-report-time; the wire-format
   remains a map, but the values are rendered, not stored as strings at
   case-build time.

## Consequences

- Each metric formula exists exactly once; a change to the formula or the
  naming is a one-place edit.
- Adding a metric adds an arms upstream (evidence variant → renderer), not N
  case-build sites across N suites.
- Gate evaluation and case reporting cannot disagree about the same number.
- The report shape (artifact JSON) does not change; PR/Release/Nightly gate
  semantics are preserved by construction.

## Alternatives Considered

### Keep the float map for case diagnostics only

Rejected — two computation paths is the defect being removed; a "diagnostic
only" path still drifts.

### Move metric computation into suites, remove the reducer

Rejected — the reducer owns cross-case aggregation and gate semantics;
pushing computation into suites fragments gate invariants.

## Verification

- `cargo test -p eval-harness` passes.
- Running PR, Release, and Nightly profiles at HEAD reproduces the v5
  observed values exactly (17/17 gates; 119/119, 123/123, 121/121 cases;
  identical headline metrics — recall_at_5 = 1.0000, mrr = 0.9924,
  top_1_hit_rate = 0.9848, entity_f1 = 0.75, claim f1 = 1.0000,
  recall-same).
- No suite module contains a `metric_map.insert("<string>", <float>)` site.
