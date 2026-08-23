# ADR-0047: Expose Lifecycle Maintenance Through the CLI Only

## Status

Accepted — 2026-08-23.

## Context

The service already implements lifecycle maintenance operations:

- dashboard inspection;
- archival candidate selection and archival;
- archived-episode restoration;
- confidence-decay invalidation; and
- community rebuilding.

The MCP Apps surface can invoke these operations through typed lifecycle
commands when the `mcp-apps` feature is enabled. The ordinary CLI has no
user-facing maintenance command, so the operations are not available in
portable or automation-oriented deployments that do not enable MCP Apps.

The MCP tool surface is intentionally frozen at eight tools. Adding a ninth
maintenance tool would duplicate the existing app workflow and require a
separate public-protocol decision. The project also preserves facts and their
bi-temporal audit trail; generic garbage collection or deletion of entities
therefore cannot be added safely without defining retention, provenance, and
rollback semantics.

## Decision

Expose the existing lifecycle service operations through a new top-level
`lifecycle` CLI command with typed subcommands:

- `dashboard` — inspect active-fact, archival-candidate, and community counts;
- `archive-candidates` — archive explicit episode IDs, requiring
  `--confirmed` for mutation and supporting `--dry-run`;
- `restore-archived` — restore explicit episode IDs, requiring `--confirmed`;
- `recompute-decay` — run the configured decay pass, requiring `--confirmed`
  for mutation and supporting `--dry-run`; and
- `rebuild-communities` — rebuild derived communities, requiring
  `--confirmed` for mutation and supporting `--dry-run`.

The CLI command delegates directly to the existing `MemoryService` methods and
writes structured JSON to stdout using the established one-shot CLI response
path. It does not add MCP tools, alter storage contracts, or introduce a new
service abstraction.

Unresolved-entity cleanup and generic record garbage collection remain
explicitly out of scope. They require a separate design covering which entity
records are safe to remove, how historical references are preserved, and how a
failed cleanup is recovered.

## Consequences

### Positive

- Existing maintenance behavior becomes available to shell scripts and
  deployments without MCP Apps.
- CLI automation receives the same structured JSON style as other one-shot
  commands.
- Confirmation and dry-run rules prevent accidental destructive lifecycle
  actions.
- No MCP protocol or eight-tool-surface change is required.
- The service remains the single owner of lifecycle policy and storage access.

### Negative

- The CLI and MCP Apps expose two adapters for the same lifecycle operations.
  Both remain thin and delegate to the same service methods.
- The CLI does not yet provide unresolved-entity garbage collection; that work
  needs its own domain decision rather than an unsafe generic delete command.
- Dry-run behavior follows the existing service contract. Decay and community
  rebuild currently report zero mutations during dry-run rather than estimating
  candidates.

## Alternatives considered

### Add a ninth MCP maintenance tool

Rejected: it duplicates the existing MCP Apps lifecycle workflow and violates
the frozen eight-tool public surface without a broader protocol decision.

### Put maintenance logic directly in CLI handlers

Rejected: it would duplicate lifecycle policy, confirmation rules, and storage
access outside the service layer.

### Add generic entity or record deletion now

Rejected: facts are append-only/invalidated rather than deleted, and the code
currently lacks a domain contract distinguishing safely orphaned entities from
historically referenced entities.
