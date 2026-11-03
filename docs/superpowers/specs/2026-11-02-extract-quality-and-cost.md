# Extract quality and cost contract

**Status:** Implemented locally, 2026-11-03. Verification and measured limitations are recorded in `docs/performance/EXTRACT_QUALITY_COST_BASELINE.md`. No production deployment or historical repair was performed.

## Problem and evidence

An `extract` response classified dates and percentages as entities, and training terms as people or companies. It also reported contradictions between unrelated historical records. The supplied response is truncated, so its source episode and full output cannot be reconstructed from the attachment alone.

Before this change, the attribute projector treated a colon-delimited free-text line as a single-valued assertion. An unresolved subject became an empty string. Distinct records with the same heading could therefore share a comparison slot and produce false contradictions. The previous extract path resolved or created every deduplicated NER candidate before writing `mentioned_in` edges. The previous warning path loaded both fact bodies separately for every contradiction relation, creating repeat reads and large responses. Production Anno is the explicit RegexNER + HeuristicNER stack; this proposal does not depend on GLiNER or more hardware.

Grafana `grafana-pushkino`, VictoriaMetrics `edr4onptd6hogd`, `job=memory_mcp`, `env=prod`, evaluated at 2026-11-02T10:55:00Z: in 24h, 9 extract calls (7 success, 2 error), 378 facts, 377 entities, 30,116 warnings; all 30,116 reconcile outcomes were contradictions. Mean extract duration was 19.51 s, versus 3.02 s for the preceding six days. Four `extract/extraction` stage observations averaged 30.38 s. Attribute reconciliation considered 82.35 candidates per observation on average. These are aggregate metrics, not per-request attribution.

## Required behavior

1. Preserve episode and fact provenance and bi-temporal history. No automatic deletion or silent rewriting of historical facts.
2. A claim is comparable only when its subject and comparison key have an unambiguous identity, the schema can justify its cardinality, and validity semantics support the comparison. Unknown information stays as a sourced fact, without a fabricated contradiction. A colon alone does not establish those properties.
3. A genuine same-slot, overlapping-validity, different-value claim still yields a contradiction; duplicates, corrections and supersessions keep their existing semantics. Avoid new confidence probabilities or a parallel belief layer.
4. Anno output is an entity candidate, not proof of type or identity. Preserve manual `resolve` semantics; qualify only automatic extraction before creating entities/edges. A rejected candidate must not erase the source episode or facts. Do not assert a universal domain-term blacklist or a calibrated confidence value without evidence. Entity identity decisions must remain provider-agnostic.
5. `extract` latency and work must be tested against input size and existing same-slot data. Eliminate avoidable repeated fact reads and output amplification at the demonstrated hot path. If a response needs bounding, define complete/partial semantics and schema compatibility before changing the public `ExtractResult`; the current response has no continuation token. Existing eight-tool surface stays fixed.
6. Measure quality and cost together: false and true contradiction cases, entity precision and retrieval, request latency, candidate work, output size, and error rate. Compare like-for-like cohorts and preserve source/metric provenance.

## Decision gates

- First obtain per-request or reproducible stage attribution using existing `gcx` metrics, traces and logs. Existing operation, stage, claim-candidate, reconciliation and result counters are available; do not add duplicate instrumentation. A slow aggregate stage is not a root cause. If tracing coverage is insufficient, use a controlled local profile.
- Test small/medium/large and repeat-extract inputs, both structured and prose, with Anno. Choose an optimization after the bottleneck is established; do not substitute a warning cap for a semantic fix.
- Before any historical re-projection, identify whether affected entities, edges, claims, relations and extraction projections can be corrected through existing append-only/invalidation APIs. If not, describe the required migration/ADR and obtain the repository-required approval. Do not assume fact invalidation repairs derived records. That is a separate deployment decision.
