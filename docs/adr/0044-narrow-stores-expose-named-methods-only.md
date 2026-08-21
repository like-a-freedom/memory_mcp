# ADR-0044: Narrow stores expose named methods only

## Status

Accepted — 2026-08-21, architecture deepening round 3.

## Decision summary

Store clients expose named methods that express intent; they do not
re-expose raw `query(sql, vars)`. Every table's SQL lives in its owning
store. The `query()` escape hatches on `AppStoreClient`,
`EpisodeStoreClient`, and `ContextAccessLogClient` are deleted, and the
service call sites that wrote inline SQL now call named store methods.

## Context

ADR-0027 narrowed `DbClient` and introduced per-table store clients, but
three stores kept a public `query()` re-export and five service call sites
routed around the seam with inline SQL: entity alias operations
(`service/entity.rs`), explanation episode lookup
(`service/explanation.rs`), extraction projection persistence
(`service/episode/entity_extraction.rs`), archival hotness check
(`service/lifecycle/archival.rs`), and query-log pruning
(`service/context/logging.rs`). Table SQL locality — the point of the
ownership seam — was no longer guaranteed; the stores were ceremony around
a leak.

## Consequences

- A new query touching several tables must be assigned to one owning store;
  graph-shaped read-model queries belong to `ContextStoreClient`.
- Stores remain thin adapters over `BoundDbClient`; this decision
  constrains their interface, not their implementation.
- The `DbClient` trait's own `query` stays for storage-internal use
  (migrations, stores); only service-layer inline SQL is banned.
