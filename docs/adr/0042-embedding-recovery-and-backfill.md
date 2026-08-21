# ADR-0042: Automatic Embedding Recovery and Deferred Backfill

## Status

Accepted — implementation completed and hardened 2026-08-21.

## Decision summary

When a configured remote embedding provider is unreachable during startup, `serve` starts in a degraded lexical/graph-only mode and starts a cancellable in-process recovery worker for the exact preflight failure, or for a durable pending/missing-embedding resume condition discovered at startup. The worker probes with exponential backoff, enables the provider only when its dimension is compatible with the existing HNSW index, persists readiness safely, and backfills only facts whose `embedding` field is missing; provider switches, dimension changes, and existing stale vectors remain the explicit `reembed` path.

## Context

The startup preflight must not hold MCP startup hostage to an external HTTP provider. After the preflight fails, the current process has enough local state to continue accepting episodes and facts, but those facts are intentionally persisted without embeddings. A temporary network interruption should not require a process restart, and facts accepted during the interruption should become semantically searchable after connectivity returns.

The vector index is dimension-bound by SurrealDB. Enabling a provider with a dimension different from the active index would make writes fail or would mix incompatible vectors. A provider signature change is also a semantic target change even when the dimension happens to remain equal. Automatic recovery therefore cannot be a general replacement for the existing operator-driven `reembed` command.

## Decision

1. Add an opt-out recovery configuration and a probe interval:
   - `EMBEDDINGS_AUTO_RECOVERY` defaults to enabled and disables the worker only when explicitly false.
   - `EMBEDDINGS_RECOVERY_INTERVAL_SECS` defaults to `60` and controls the initial recovery probe delay.
   - Failed probes use a separate exponential retry schedule of `15s`, `30s`, `60s`, … capped at `300s`; every probe error, including fatal HTTP statuses such as `404`, remains retryable at the worker level. Warnings are demoted to debug logging after three consecutive failures.

2. Spawn the worker only when all of these are true:
   - normal startup selected the exact `DisableSemantic { reason: "embedding target preflight failed" }`, `ResumePendingBackfill`, or `RecoverMissingEmbeddings` decision;
   - the configured provider is remote (`openai-compatible` or `ollama`); and
   - automatic recovery was not opted out.

   A rebuilding/failed namespace, a signature mismatch with no missing embeddings, a legacy-dimension mismatch, disabled embeddings, and forced `reembed` mode never start this worker. `RecoverMissingEmbeddings` keeps semantic retrieval disabled while allowing safe recovery of facts that have no vector yet.

3. Store the active embedding provider and its identity in one process-wide runtime state behind `Arc<std::sync::RwLock<...>>`. `build_context()` takes a short read-lock snapshot and drops the lock before any asynchronous operation. Recovery replaces the snapshot atomically from the service's point of view; new requests see the recovered provider without rebuilding `MemoryService` or adding a dependency.

4. On each probe, compare the returned dimension with the persisted HNSW dimension (`embedding_state.dimension`, falling back to the configured fallback dimension):
   - equal dimension and equal/absent stored signature: persist `embedding_state:fact.status = "backfill_pending"` before swapping the provider, invalidate the context cache, swap in the provider, run backfill, then persist `status = "ready"` only after backfill completes;
   - equal dimension but different stored signature: enable the provider for new facts, backfill only facts with no vector, preserve the old persisted signature so semantic retrieval remains disabled after restart, and log `embedding.reembed_required`;
   - different dimension: keep semantic mode disabled and log `embedding.reembed_required`; do not write vectors that the HNSW index cannot accept.

5. The existing `embedding_state.status` field is the durable crash-resume marker; no schema migration or new field is required. Startup resumes `backfill_pending`, and also treats a matching ready state with `embedding IS NONE` facts as resumable for compatibility with states written before the marker existed. A signature-mismatch state with missing vectors uses the separate `RecoverMissingEmbeddings` startup decision so it can resume without making stale vectors semantically authoritative.

6. Backfill is a separate narrow store operation using `embedding IS NONE`, stable `fact_id` cursor pagination, and batches of `100`. It writes only embedding metadata and the generated vector, never drops/recreates the index, and never rewrites a fact that already has an embedding even if that embedding has a stale signature. A provider/network failure returns the worker to the probe state with backoff. The worker exits naturally only after a compatible recovery has completed and the applicable missing-embedding set is empty; cancellation always joins the task during server shutdown.

## Considered options

### Periodic recovery probe vs lazy first-embed probe

**Chosen: periodic background recovery probe.** It detects restored connectivity even when no new fact or query arrives, keeps the first user request after an outage fast, and gives the process a deterministic place to perform deferred backfill. A lazy probe would couple recovery to request traffic and could leave an offline-created corpus unprocessed indefinitely.

### Cyclic recovery/backfill vs one-shot recovery

**Chosen: cyclic probe → recovery → backfill flow.** A one-shot worker would either abandon facts after a second network interruption or require a restart. Returning to probe after a backfill network failure handles connectivity flapping without duplicating provider construction or index work. The durable `backfill_pending` state makes the flow restart-safe, and missing-vector counting resumes legacy states that predate the marker. Completion is explicit and bounded when no missing embeddings remain.

### `std::sync::RwLock` vs `arc-swap`

**Chosen: `Arc<std::sync::RwLock<EmbeddingRuntimeState>>`.** The state is small, replacement is infrequent, and the existing dependency policy does not justify a new crate. The lock is never held across `.await`; `build_context()` clones the provider and metadata before returning. `arc-swap` would reduce read-side locking but adds dependency and operational complexity for a non-hot-path swap.

### Narrow missing-embedding predicate vs broad stale-signature predicate

**Chosen: narrow `embedding IS NONE` predicate.** Backfill completes facts created while semantic mode was disabled. Rewriting vectors that already exist but have a different signature is a target migration and belongs to `reembed`, which owns HNSW index replacement and durable progress. The two query families remain separate so an outage recovery cannot silently become a provider migration.

## Consequences

### Positive

- Startup remains bounded by the existing one-shot preflight and does not wait for remote embedding retries.
- A long-lived MCP process can restore semantic writes without restart when the remote service returns.
- Facts persisted during degraded mode are eventually embedded in the background.
- HNSW dimension safety and provider-switch safety remain explicit; automatic recovery cannot silently perform a destructive reindex.
- The runtime state has one swap point, so request contexts consistently use either the disabled provider or the recovered provider.
- Shutdown is cancellable and joins the recovery task like the existing lifecycle workers.

### Negative

- Recovery and backfill consume remote provider capacity after connectivity returns and may take time proportional to the offline corpus.
- Existing contexts created before the swap retain their cloned provider; subsequent requests use the recovered state.
- A same-dimension provider signature change can safely backfill only missing vectors, but still requires operator-driven `reembed` for old vectors and durable semantic consistency; the old persisted signature is intentionally retained.
- A dimension mismatch keeps the worker degraded and requires configuration correction plus `reembed`; automatic recovery cannot change the index dimension safely.
- Operators must monitor structured recovery/backfill events when diagnosing a persistent endpoint error such as `404`.

## Related decisions

- ADR-0012: Backfill claims outside startup migrations.
- ADR-0018: Reembed interactive progress.
- ADR-0026: Adopt durable work mechanics.
- ADR-0027: Finish `DbClient` capability narrowing.
