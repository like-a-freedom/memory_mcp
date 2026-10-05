# ADR-0076: Route retrieval by intent, and type memory by retention class

- Status: accepted — 2026-10-05
- Related: [ADR-0074](0074-connect-the-reconciliation-relation-to-the-read-path.md),
  [ADR-0075](0075-carry-the-existing-trustclass-on-claims-and-edges.md)

Retrieval currently runs a fixed fusion of every available signal for every
query, and retention applies a broadly uniform half-life across fact types.
Both are defensible defaults and both are wrong for a fixed share of queries:
a "when did X change" question is temporal, a "why did X happen" question is
causal, and neither benefits from paying for dense similarity over the same
candidates as a direct lookup.

MAGMA separates semantic, temporal, causal and entity views and selects among
them by query intent. Its own numbers do not argue for adopting its
representation — its LoCoMo advantage is dominated by the adversarial column,
its temporal margin over the nearest competitor is 0.001, and its default
embedder is a 22M-parameter distilled encoder. The transferable idea is the
*selection*, not the vocabulary.

## Decision

Two decisions, taken together because they are the same principle: **select by
declared purpose rather than applying every mechanism uniformly.**

**Retrieval.** Add a typed retrieval intent and let it choose the traversal plan.
The intent is a schema-skipped field on the request, exactly as `access` already
is (`#[serde(skip_serializing)]` plus `#[schemars(skip)]`), so the frozen
eight-tool surface is untouched and no tool caller can set it. `view_mode` is
not reused or renamed: it is a public string field that already means "shape of
the presentation" and accepts values such as `timeline`. Intent means "what to
look for". Sharing the field would make an existing public parameter mean two
things.

Fact-type filtering already exists as the caller-supplied `fact_types` array,
and a test pins it as intentional. The intent must not become a second way to
express the same filter; it selects a traversal plan, not a type predicate.

**Memory classes.** Keep one `FactType` axis. `FactType` today has five
variants (`Note`, `Decision`, `Metric`, `Promise`, `Experience`); the review's
proposed episodic/semantic/identity/working/uncertainty taxonomy is not adopted
as a second axis. Retention and write policy become pure functions over the
existing `FactType` in the owning context (ADR-0066).

Retention today is not type-aware at all: `LifecyclePolicy.decay_half_life_days`
is a single scalar defaulted to `365.0`, threaded into `run_decay_pass` for
every fact regardless of type. So the problem is not a mis-tuned per-type table
but the absence of one. A uniform half-life applied to `Promise`, `Metric` and
`Experience` alike means an unreferenced procedural experience decays on the same
schedule as a durable promise — which is how a uniform half-life ends up either
forgetting a stated preference or preserving operational noise.

## Consequences

- Routing must be measured per intent before it is trusted. Fixed fusion is the
  incumbent and the comparison is the deliverable; an intent router that is
  never shown to beat fusion on the intents it claims to serve has added
  complexity and a second failure mode for nothing.
- Per-type write gates land at the extraction seam, where type selection already
  happens (`summary_parser.rs:209`, `classify_structured_summary_fact_type`), not
  as a parallel gate.
- The `access_count` novelty penalty (`ranking.rs:486–488`) measures *retrievals*,
  not usefulness, and inverts the feedback loop by penalising frequently-needed
  memories. Replacing it with a use-based signal requires outcome trace data, so
  it is scheduled with the trace work rather than here — until then the plan
  states plainly that no use signal exists, rather than shipping a proxy.
- Selecting by intent presupposes the relation is visible to the reader, which is
  ADR-0074. Intent routing over a write-only reconciliation layer would route
  into nothing.