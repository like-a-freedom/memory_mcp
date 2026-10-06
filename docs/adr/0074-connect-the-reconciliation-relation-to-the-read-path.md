# ADR-0074: Connect the reconciliation relation to the read path

- Status: accepted — 2026-10-05
- Implements: the read-path requirement of the landing plan in
  [`docs/superpowers/specs/2026-10-05-memory-quality-evidence-review.md`](../superpowers/specs/2026-10-05-memory-quality-evidence-review.md)

Claim reconciliation is currently write-only. `assemble_context` reads `fact`
and `edge`; the claim relation recorded by the reconciliation pipeline never
reaches a reader. A supersession is detected, persisted, scored `1.0` by the
`claim_f1` gate, and then never consulted, so the project's largest subsystem —
41 evaluated cases, a bi-temporal schema, relation versioning — has no effect on
what a reader is shown.

## Decision

An assembled context item must expose the reconciliation relations of the facts
it carries, and the assembly must rank a superseded fact below its successor
when both are present.

The relation lookup is owned by the **knowledge** context, not by retrieval.
Claims and relations belong to `knowledge` under ADR-0058, so the read path
consumes a narrow named method on a knowledge store (ADR-0044) rather than
composing claim SQL inside `memory/retrieval`. `ContextRetrievalPort` stays the
seam being decorated, never the owner of the query.

The ranking rule is conditional and carries no tunable coefficient:

> When a successor is present in the assembled pack, the superseded fact ranks
> strictly below it. When no successor is present, the fact is not demoted.

The conditional is the decision, not the magnitude. A fact demoted without a
present successor is the least-stale value a reader has, and demoting it can
push it out of the budget entirely — handing the reader nothing instead of the
stale-but-known value. A fixed coefficient was rejected as arbitrary and prone
to being tuned against the evaluation metric; excluding superseded facts was
rejected as a change of stance, since Phase 0 marks rather than removes.

`Duplicate` is explicitly **not** demoted. A duplicate is redundancy, not
staleness: demoting it can remove the only surviving copy once decay retires one
of the pair. Only `Supersession` and `Correction` demote.

## Consequences

- This is a post-assembly reordering, not a term in the scoring function. Successor
  presence is known only after candidates are selected and ordered, so the rule
  cannot be evaluated earlier without a second classification axis on the hot
  path. It introduces no constant to tune and no new candidate axis.
- The reordering needs to name the successor's fact, and `ClaimRelationSummary`
  carried no such field. Its `counterpart_fact_id` looked like one but was never
  populated — its only mention in the tree was its own declaration — and was
  removed in a separate commit ahead of this work, because `claim_relation`
  already records direction in `predecessor_claim_id` / `successor_claim_id`.
  Removing a field that was never populated is not a behavior change.
  **Amended 2026-10-05:** the original Consequences wording said "needs no
  schema change" on the strength of that dead field. Removing it made the
  justification evaporate rather than the decision, so the plan does change the
  schema by one field: `ClaimRelationSummary.superseded_by_fact_id:
  Option<String>`, populated only for `Supersession` / `Correction`. The
  decision itself — post-assembly reordering, no constant, no new candidate
  axis — is unaffected. `claim_relation` remains the source of truth and
  nothing recomputes direction.
- `explain` reads no claim or relation data today (`explanation.rs` touches only
  facts and entities). Whether `explain` gains the same projection is a
  deliberate open choice of this ADR, recorded in the accompanying spec, and is
  not a precondition: the evidence must not depend on a second public surface.
- `assemble_context` output changes, so ADR-0022's frozen compact default and the
  response-size budget apply. The new projection is surfaced under compact-mode
  `skip_serializing_if` gating.
- **Disclosure is gated by the claim rollout stage, not unconditional.** The
  Decision above says an item *must* expose its relations; read together with
  `docs/evals/CLAIM_RECONCILIATION.md`, which states that the default `shadow`
  stage "projects claims but does not expose relations in `assemble_context`"
  and makes promotion to `evidence` conditional on precision and recall
  thresholds. Both hold: the read path serves relations only when the stage is
  `evidence`, and at `shadow` / `relations` the projection returns nothing.
  The gate lives in `knowledge::api::SurrealRelationReader`, the adapter behind
  `RelationReadPort`, so `memory/retrieval` never learns the stage exists and a
  deployment cannot be talked into disclosure from the retrieval layer. One gate
  covers both halves of the decision: `demote_superseded` reads its input from
  the rows that gate withholds, so neither the metadata nor the reordering can
  happen while disclosure is forbidden.
- `MemoryService::with_claim_rollout_stage` is the configuration seam for the
  stage, complementing `MEMORY_CLAIM_ROLLOUT_STAGE`. It accepts the same
  vocabulary as the environment variable rather than exposing the crate-private
  `ClaimRolloutStage` type, and the promotion thresholds in
  `CLAIM_RECONCILIATION.md` remain the operator's to verify — this ADR changes
  where relations surface, not when an operator may turn them on.
- The supersession acceptance test that today calls `InvalidateCapability` is
  testing retraction, ADR-0009's deliberate opposite. It does not become evidence
  for this decision until it exercises a real supersession.

## Relationship to the plan

This ADR is the first phase and gates the rest. Measuring conflict-resolution
precision, or evaluating an external suite, before the relation reaches a reader
would measure a subsystem that cannot influence a response. `valid_to` is also
never written, which makes the `Disjoint` arm of `validity_relation` structurally
unreachable — a precision defect in reconciliation, not the mechanism of a stale
read, and one that fixing alone would change no user-visible behavior.