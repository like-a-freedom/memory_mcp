# ADR-0005: Separate claim traces from metric labels

## Status

Accepted

## Context

Claim reconciliation needs enough detail to explain individual extraction and comparison decisions. Comparison keys and object identifiers are unbounded, however, and using them as Prometheus labels would create uncontrolled time-series cardinality.

## Decision

Trace-level structured events carry the full comparison key, correlation IDs, claim and fact IDs, normalized match details, decision outcome, reason code, and timing. Prometheus counters and histograms aggregate only by bounded dimensions such as claim schema, stage, match mode, outcome, and reason code. Raw comparison keys and object identifiers are never metric labels; traces and metrics are correlated through request or trace IDs.

## Consequences

- Individual reconciliation decisions remain diagnosable in trace mode.
- Prometheus series cardinality stays bounded as the knowledge base grows.
- Operational dashboards can compare extraction coverage, reconciliation outcomes, and latency by schema.
- Investigating a specific comparison requires following its correlation ID into trace logs.
