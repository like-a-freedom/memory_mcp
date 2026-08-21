# ADR-0043: One owner for the embedding state record

## Status

Accepted — 2026-08-21, architecture deepening round 3.

## Decision summary

The durable `embedding_state` record has exactly one owner: a narrow store
module that holds the record ID, the typed status vocabulary (`ready`,
`backfill_pending`, `rebuilding`, `failed`), the record shape, and every
write. Startup bootstrap, Embedding Recovery, and Reembed all write through
it. The startup decision remains a pure function over the record's JSON so
its exhaustive branch tests survive unchanged.

## Context

The record previously had two writers with different implicit schemas:
`write_bootstrap_ready_state` (startup/recovery: `ready`,
`backfill_pending`) and reembed's `write_embedding_state` (`rebuilding`,
`ready`, `failed`, plus `last_job_id`). Nothing enforced agreement, and a
new identity field had to be added in several places. ADR-0042 made this
record the durable crash-resume marker for recovery, so its schema is now a
load-bearing invariant.

## Considered options

### Extend EmbeddingBackfillStoreClient vs a separate store

**Chosen: a separate `EmbeddingStateStoreClient`.** The backfill store's
purpose is the `embedding IS NONE` cursor API; adding record ownership
would make both modules shallower. Two adapters of the narrow-store pattern
justify the seam.

### Typed reads everywhere vs typed writes only

**Chosen: typed writes; reads stay JSON-shaped at the decision seam.**
`decide_embedding_startup` is a pure, exhaustively tested function over the
record JSON; retyping its input would churn its tests without changing
behavior. Unknown statuses still fall into the existing `DisableSemantic`
"invalid or incomplete" branch.

## Consequences

- No schema migration: existing rows parse unchanged; writes serialize to
  the same field set as before.
- The embedding runtime state of ADR-0042 is untouched; consolidating the
  identity tuple beyond the record is deliberately out of scope (YAGNI).
