# ADR-0012: Backfill claims outside startup migrations

## Status

Accepted

## Context

Older databases contain facts but no claim projections. Extracting claims for the full history during startup would make upgrade time proportional to database size, increase the chance of an unusable partial upgrade, and couple schema safety to derived-data processing. Facts remain the durable source evidence and are already retrievable without claims.

## Decision

Startup migrations create only the additive claim schema, indexes, and durable job records. They do not synchronously extract claims from historical facts. Existing facts remain readable immediately after migration.

After startup, a zero-configuration local backfill runs in bounded batches. It stores per-namespace progress, uses a stable fact-ID cursor and an extractor fingerprint, resumes after restart, and retries without duplicating claims. New facts use the same projection code on the normal extraction path. Backfill failure is observable but does not make legacy facts unavailable or roll back the schema migration.

The implementation should extract a reusable durable batch-job mechanism from the existing `reembed` pattern only when the claim backfill is implemented; it should not couple claim logic to embedding logic.

## Consequences

- Upgrade startup time is bounded by schema migration rather than memory volume.
- Contradiction coverage grows progressively while old retrieval continues to work.
- Operators can observe coverage, lag, failures, throughput, and the active extractor fingerprint.
- The backfill must be idempotent, restart-safe, and fair across namespaces.
