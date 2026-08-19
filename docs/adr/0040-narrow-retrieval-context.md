# ADR-0040: Narrow the Retrieval Infrastructure Context

## Status

Accepted — 2026-08-19. Implementation is task T10 of the
[architecture deepening round-2 plan](../superpowers/plans/2026-08-19-architecture-deepening.md).

## Context

`ServiceContext` is the shared seam for capability modules and context assembly.
It currently carries the full set of ingestion, extraction, resolution, fact,
claim, triple, explanation, embedding, cache, and lifecycle dependencies. The
retrieval pipeline only needs a small subset of that infrastructure, but every
retrieval module accepts the full `ServiceContext`.

The retrieval implementation also reconstructs a `ContextStoreClient` and a
`ContextAccessLogClient` repeatedly from the same database client and Active
Namespace. That widens the test seam and makes storage construction part of
every retrieval helper's interface. Context tests consequently pay for the
full `DbClient`-backed service fixture even when they exercise one retrieval
operation.

This is architectural friction, not a request to split the domain model:
context assembly remains one product operation, and the existing concrete
storage stores already provide the right adapters.

## Decision

Introduce a concrete, crate-private `RetrievalContext` owned by the service
layer. `ServiceContext::retrieval_context()` constructs it once for an
assembly operation. The outer `assemble_context(&ServiceContext, ...)`
function remains as the compatibility adapter used by capabilities, tools, and
lifecycle recall; it immediately enters the narrow seam.

`RetrievalContext` owns or reuses only retrieval infrastructure:

- one pre-bound `ContextStoreClient` for all retrieval reads;
- one pre-bound `ContextAccessLogClient` for query-log writes and pruning;
- one pre-bound `AppStoreClient` for graph/view reads;
- the configured `EmbeddingService` for semantic retrieval and availability;
- the process-local context cache;
- the logger, Active Namespace, query-log configuration, and fact-access
  tracking capability required to complete an assembly.

All internal modules under `service/context/` and the graph/view calls they
make accept `&RetrievalContext`, not `&ServiceContext`. Store access is through
that pre-bound context, so each assembly has one explicit storage construction
point. `RetrievalContext` is a concrete struct, not a new trait: there is one
production adapter and no demonstrated runtime variation requiring another
seam.

## What remains unchanged

- The public MCP and CLI interfaces are unchanged.
- `assemble_context` continues to accept the existing service context at its
  outer adapter.
- Retrieval ranking, temporal semantics, access-policy filtering, caching,
  logging, and embedding behavior are unchanged.
- `ServiceContext` remains the capability seam for ingestion, extraction,
  resolution, fact mutation, claims, and lifecycle operations.
- The concrete storage stores continue to own SQL; the retrieval context only
  binds and reuses them.

## Consequences

### Positive

- Retrieval callers learn a smaller interface and tests can exercise retrieval
  through the infrastructure it actually uses.
- Store construction is local to `retrieval_context()`, improving locality and
  avoiding repeated rebinding of the same Active Namespace.
- A retrieval-only change does not require understanding or constructing
  unrelated ingestion, NER, claims, or triple-extraction dependencies.
- The narrow seam is directly testable through a concrete adapter without
  introducing a broad mock trait.

### Negative

- Retrieval helper signatures change from `ServiceContext` to
  `RetrievalContext`, so internal tests and helper call sites must enter the
  narrow seam explicitly.
- `RetrievalContext` is an additional data structure and must be kept limited
  to retrieval dependencies; adding unrelated fields would recreate the
  original God-object problem.
- The outer compatibility adapter still constructs a retrieval context per
  assembly call. This is intentional: one assembly owns one coherent snapshot
  of its configured retrieval dependencies.

## Alternatives considered

### Keep passing `ServiceContext` and only cache store handles

Rejected: this reduces repeated construction but leaves the interface and test
seam coupled to every capability dependency.

### Introduce a retrieval trait

Rejected: there is one production adapter and no current runtime variation.
A trait would add an interface and fake without increasing leverage; the
concrete `RetrievalContext` is the deeper seam for the current product.

### Make `RetrievalContext` dereference to `ServiceContext`

Rejected: dereference would silently re-expose the wide interface and make the
narrowing cosmetic. Retrieval modules must be unable to reach unrelated
capabilities through the seam.
