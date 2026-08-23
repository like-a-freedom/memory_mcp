# ADR-0048: Bounded runtime observability and artifact-backed evaluation

## Status

Accepted — 2026-08-23, audit remediation wave 3.

## Context

The server already has an optional Prometheus listener and claim-reconciliation
metrics. The remaining runtime paths expose useful structured logs, but they do
not provide a coherent low-cardinality view of operation volume, latency, or
result counts. This makes it difficult to answer basic operational questions
such as whether retrieval is slow, whether failures are increasing, or how much
structured memory an extraction produced.

The evaluation harness is different from the server runtime. It is a bounded,
batch process whose durable JSON artifact already records run duration, per-case
duration, suite quality metrics, pass/quality-failed/invalid counts, gates, and
capacity measurements. Emitting those values as Prometheus labels would create
ephemeral series and would make the artifact less useful as the reproducible
source of truth.

ADR-0005 also requires that unbounded identifiers never become Prometheus
labels. Operation metrics therefore need a closed vocabulary for operations,
outcomes, and result kinds.

## Decision

Add three generic, server-side metric families in `observability`:

- `memory_operation_calls_total{operation,outcome}` — invocation count;
- `memory_operation_duration_seconds{operation,outcome}` — operation latency;
- `memory_operation_results_total{operation,result}` — bounded result counts.

The operation vocabulary covers the six canonical tools and lifecycle
maintenance operations. The outcome vocabulary is `success` or `error`; the
result vocabulary is a fixed set of domain output kinds such as `episodes`,
`entities`, `facts`, `items`, `warnings`, and `invalidations`. Unknown values
collapse to `other`. No namespace, project, source, episode, fact, entity,
claim, relation, request, or job identifier may be a label.

Instrumentation lives at protocol-agnostic tool boundaries, so MCP and one-shot
CLI calls share the same counters without duplicating business logic. Lifecycle
service methods use the same operation recorder so CLI, MCP Apps, and background
maintenance callers observe the same domain operation where applicable. The
existing `memory_claim_*` families remain owned by claim telemetry and retain
their specialized bounded labels.

The evaluation harness keeps its current artifact contract as the canonical
batch observability surface. Its existing run/case durations, suite summaries,
quality metrics, gates, and capacity metrics are documented rather than
exported through the server Prometheus recorder.

Without the `prometheus` feature, metric calls remain no-ops. Without
`MEMORY_PROMETHEUS_LISTEN_ADDR`, no recorder or socket is installed, preserving
the zero-config default.

## Consequences

### Positive

- Operators get bounded volume, latency, and result metrics for the runtime
  paths that matter to memory quality and capacity.
- MCP and CLI adapters cannot drift in their instrumentation because they share
  protocol-agnostic tool functions.
- Evaluation remains reproducible and artifact-based instead of depending on a
  live metrics backend.
- The existing claim telemetry and cardinality guard remain intact.

### Negative

- The generic operation metrics intentionally do not identify individual
  requests or records; those details remain in structured logs and traces.
- Result counts are only emitted where the operation produces a bounded,
  decision-useful quantity. They are not a replacement for database capacity
  inspection.
- A future public metric family or label must update this ADR and its bounded
  vocabulary tests.

## Alternatives considered

### Export all evaluation metrics to Prometheus

Rejected: evaluation runs are short-lived and already produce versioned JSON
artifacts with provenance, fingerprints, gates, and case evidence. Prometheus
would add no durable value and would risk cardinality and semantic drift.

### Instrument storage methods individually

Rejected: storage-level instrumentation would double-count one logical request
across multiple queries and couple operational metrics to persistence details.

### Add identifiers as labels for debugging

Rejected by ADR-0005: identifiers are unbounded. Structured logs retain the
request and record context needed for individual diagnosis.
