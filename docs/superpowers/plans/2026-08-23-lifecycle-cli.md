# Lifecycle CLI Maintenance Plan

## Goal

Make the already-implemented lifecycle maintenance operations available to
portable CLI and automation users without expanding the MCP tool surface.

## Delivery status

**Delivered — 2026-08-24.** CLI wiring, service-owned confirmation policy,
parser/handler/output-shape coverage, and documentation are complete. Safe
unresolved-entity retention and generic garbage collection remain open design
work.

## Scope

- Add typed CLI arguments for lifecycle inspection and maintenance operations.
- Delegate handlers to existing `MemoryService` lifecycle methods.
- Keep confirmation and dry-run policy in a service-owned lifecycle type shared
  by the CLI and MCP Apps adapters.
- Preserve structured JSON CLI output through the shared writer and standard
  error envelope; lifecycle success uses its explicit operation/result envelope.
- Require explicit confirmation for mutating operations; retain dry-run support
  where the service already supports it.
- Add parser and handler coverage for safety rules and output shape.
- Keep the tracked plan and ADR status explicit about delivered lifecycle
  wiring and the unresolved-entity cleanup design that remains open. The
  repository's operator backlog is intentionally ignored and is not a release
  artifact.

## Out of scope

- New MCP tools or changes to the eight-tool MCP contract.
- Generic garbage collection or hard deletion of entities/facts.
- Changes to lifecycle policy, storage queries, or background workers.
- Estimating dry-run mutation counts where the existing service contract does
  not provide them.

## Implementation sequence

1. Add `LifecycleArgs` and typed lifecycle subcommands to the CLI surface.
2. Add a thin CLI handler that delegates to `MemoryService`; keep lifecycle
   confirmation policy in the service layer.
3. Wire the handler through command dispatch and one-shot error handling.
4. Add focused tests for clap parsing, confirmation behavior, and the stable
   lifecycle operation/result success envelope.
5. Record delivery status in this plan and validate with format, tests, and
   clippy.

## Decision record

See [ADR-0047](../../adr/0047-cli-only-lifecycle-maintenance.md).
