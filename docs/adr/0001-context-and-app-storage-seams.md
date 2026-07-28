# ADR-0001: Capability seams for Context Assembly and MCP Apps

## Status

Accepted

## Context

`DbClient` adapts all SurrealDB operations. Context Assembly needs only a subset; MCP Apps need domain mutations and typed state transitions. Passing the universal interface through every caller made retrieval code depend on unrelated writes, migrations, and maintenance operations. The MCP handler also held lifecycle confirmation policy and ingestion-review status transitions.

## Decision

Context Assembly depends on the narrower `ContextStore` capability. Access-log writes use the separate `ContextAccessLog` capability. App-facing lifecycle, graph, and diff workflows use the separate `AppStore` capability. These capabilities are implemented by adapters over `DbClient`, which preserves the existing SurrealDB implementation while making each caller seam explicit.

Lifecycle commands are represented by typed `LifecycleCommand` values and executed in `service/apps`. App action classification, cross-app invariants, confirmation requirements, and patch parsing are represented by the protocol-neutral `AppCommand` workflow in `service/apps`. Ingestion-review status and edit transitions operate on typed `IngestionReviewItem` values in the service layer. The MCP handler remains responsible for session lookup, payload persistence, and response shaping.

Graph expansion and diff export remain in the MCP adapter for now because they mutate session presentation state and do not yet have a stable domain outcome independent of an app session.

## Consequences

- Context and app workflows no longer reach directly for the universal `DbClient` interface.
- Storage capability seams can be narrowed further without changing MCP contracts.
- Domain policy is testable without constructing MCP request envelopes.
- The adapter still owns session-specific presentation behavior by design.
- A future storage implementation must satisfy the capability adapters or provide explicit implementations for them.

## Verification

The default suite and the `mcp-apps` suite pass after the migration. `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all --check` also pass.
