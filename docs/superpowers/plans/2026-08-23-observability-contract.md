# Bounded Runtime Observability Plan

## Goal

Close the observability backlog item with a coherent, low-cardinality runtime
metric surface while preserving the evaluation harness's artifact-first truth
contract.

## Delivery status

**Delivered — 2026-08-24.** Bounded runtime metrics, canonical tool and
lifecycle instrumentation, contract tests, and runtime/evaluation documentation
are complete. Deployment-specific dashboards and alerts remain operator work.

## Scope

- Add bounded operation call, duration, and result metric helpers to
  `crates/memory-mcp/src/observability.rs`.
- Instrument the protocol-agnostic canonical tools so MCP and CLI calls share
  metrics.
- Instrument lifecycle service operations used by CLI and MCP Apps.
- Add contract tests for metric names, allowed labels, and no-op behavior.
- Document the runtime/evaluation boundary and metric configuration.
- Keep the tracked plan and ADR status explicit about the delivered scope and
  remaining operator-owned limitations. The repository's operator backlog is
  intentionally ignored and is not a release artifact.

## Out of scope

- Prometheus instrumentation inside every storage query.
- Unbounded labels or per-record metric series.
- A new metrics dependency or a new MCP tool.
- Exporting eval artifacts to Prometheus.
- Replacing structured logs, query logs, or durable evaluation artifacts.

## Implementation sequence

1. Record ADR-0048 and define the closed operation/outcome/result vocabularies.
2. Implement the no-op-safe operation recorder and contract tests.
3. Instrument canonical tools and lifecycle service methods.
4. Update README and record delivery status in this plan and ADR.
5. Run focused tests, full crate tests, clippy, formatting, and diff checks.

## Acceptance criteria

- Prometheus builds expose operation volume, latency, and bounded result counts
  when enabled and configured.
- Default builds and unset configuration remain behaviorally unchanged.
- MCP and CLI use the same instrumentation boundary.
- No identifier-like label appears in the generic metric helpers.
- The eval artifact remains the documented source of truth for batch quality,
  capacity, and evaluation latency.

## Decision record

See [ADR-0048](../../adr/0048-bounded-runtime-observability.md).
