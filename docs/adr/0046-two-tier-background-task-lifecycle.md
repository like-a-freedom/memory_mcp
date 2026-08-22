# ADR-0046: Two-tier background task lifecycle

## Status

Accepted — 2026-08-21, audit remediation wave 2.

## Decision summary

Background tasks are split into two tiers with different lifecycle
guarantees:

1. **Workers** — long-lived loops (lifecycle decay/archival, claim
   worker, embedding recovery). They are tracked: the runtime holds a
   `CancellationToken` and a `Vec<JoinHandle>`, and shutdown joins them.
   This is the existing `LifecycleWorkerRuntime` / `ClaimWorkerRuntime` /
   `EmbeddingRecoveryRuntime` pattern; it stays mandatory for anything
   that runs forever or owns a retry loop.
2. **Per-request derivations** — short-lived spawns issued while serving
   a single request (e.g. background embedding of a just-written fact,
   triple projection follow-ups). They are **best-effort**: not tracked
   in a join registry. They are bounded by the existing semaphore,
   cannot outlive a crash window that recovery does not already cover,
   and their failure mode is "work is redone later", never data loss.

## Context

The audit flagged three untracked `tokio::spawn` sites
(`service/embedding_service.rs`, `service/episode/triples.rs`) as
potential orphaned work. A full join registry was considered and
rejected: per-request tasks are created at high frequency, are bounded
by concurrency limits, and adding a registry would add contention and
shutdown latency for no correctness gain — recovery paths (embedding
backfill, reembed) already reconcile any task that dies mid-flight.

## Consequences

- New long-lived workers must register a cancellation token + join
  handle; reviewers should reject untracked infinite loops.
- Per-request spawns need only a comment citing this ADR; they may be
  dropped on shutdown by design.
- If a future feature makes a derivation's loss observable as data loss,
  it graduates to tier 1 (tracked worker) rather than growing the
  best-effort tier.
