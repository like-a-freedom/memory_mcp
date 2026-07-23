# Memory Management and Architecture Regression Fixes

> Status: Proposed (2026-07-23)
> Parent: Architecture Review #2

## Context

After the capability-seam migration and lifecycle wiring (commit `e744950f` +
`456e3de9`), a second audit found four new issues: a real memory leak in the
session-trace registry, a god-object regression on `ServiceContext`, detached
tokio tasks, and missing memory-management tests. This plan addresses all four
in priority order.

## Candidate 1 — Fix SessionTraceRegistry memory leak

**Problem:** `SessionTraceRegistry.sessions: HashMap<String, ExposureTraceStore>`
grows unbounded. Each new session_id creates a permanent entry. `evict_expired()`
exists but has zero production callers.

**Fix (2 parts):**

### Step 1 — Call `evict_expired` on every `record()`

Amortized cleanup: every time a trace is recorded, evict expired traces across
all sessions. This is O(n_sessions) per record, but n_sessions is small in
practice (one per active agent session). The lock is `std::sync::Mutex` and
the work is pure CPU (no I/O), so holding it briefly is safe.

```rust
pub fn record(&self, session_id: &str, trace: ExposureTrace) {
    let mut sessions = self.sessions.lock().expect("trace registry lock");
    // Amortized eviction: clean expired traces before adding new ones.
    let now = trace.created_at_secs;
    for store in sessions.values_mut() {
        store.evict_expired(now, TRACE_TTL_SECS);
    }
    let store = sessions.entry(session_id.to_string()).or_default();
    store.push(trace);
}
```

### Step 2 — Cap the number of sessions

Add `MAX_SESSIONS: usize = 256` constant. When the HashMap exceeds this,
evict the session with the oldest trace. This mirrors the `ExposureTraceStore`
LRU pattern one level up.

```rust
const MAX_SESSIONS: usize = 256;

// In record(), after push:
if sessions.len() > MAX_SESSIONS {
    // Evict the session with the oldest trace.
    let oldest_session = sessions
        .iter()
        .min_by_key(|(_, store)| store.oldest_trace_ts())
        .map(|(k, _)| k.clone());
    if let Some(key) = oldest_session {
        sessions.remove(&key);
    }
}
```

Requires adding `oldest_trace_ts()` to `ExposureTraceStore` (peek at front of
VecDeque).

### Step 3 — Remove `#[allow(dead_code)]` from `evict_expired`

It's now called from `record()`.

### Step 4 — Tests

Add to `tests/agent_memory_lifecycle_e2e.rs`:

1. `trace_registry_stays_bounded_across_many_sessions` — create 300 sessions,
   recall once each, assert the registry's session count ≤ 256.
2. `repeated_recall_same_session_caps_at_32_traces` — recall 100 times with
   different tasks in the same session, assert `ExposureTraceStore.len() ≤ 32`.
3. `expired_traces_are_evicted_on_record` — record a trace at t=0, then record
   another at t=31min, assert the old trace is gone.

**ADR needed?** No — this is a bug fix delivering ADR-0016 AD-7's "ephemeral by
default" promise. No new decision.

## Candidate 2 — Push ServiceContext logic into capabilities

**Problem:** `ServiceContext` grew from 58 to 1102 lines. It holds `add_fact`
(175 lines), `generate_embedding` (176 lines), `generate_query_embedding_with_background`
(49 lines), `spawn_triple_extraction`, `cached_query_embedding`, etc. The
capabilities are thin pass-throughs. The god-object moved, it didn't dissolve.

**Fix:** Push domain logic down into the modules that own it.

### Step 1 — Move embedding logic to `service/embedding.rs`

`generate_embedding`, `generate_query_embedding_with_background`,
`cached_query_embedding`, `build_fact_embedding_input` — these are embedding
concerns, not shared infrastructure. Move them to `embedding.rs` as methods on
a new `EmbeddingService` struct (or standalone functions taking `&dyn EmbeddingProvider`
+ cache handles). `ServiceContext` holds an `Arc<EmbeddingService>` field.

Callers: `context/semantic.rs`, `episode/fact_extraction.rs`, `reembed.rs`.

### Step 2 — Move `add_fact` to `service/fact.rs`

`add_fact` (175 lines) is fact creation logic. It belongs on `FactService`
(which already exists at `src/service/fact.rs` but is a narrow struct). Move
the method there, pass the `EmbeddingService` + `EntityService` handles it needs.

Callers: `core.rs`, `context/graph.rs`, `episode/fact_extraction.rs`,
`apps/ingestion_review.rs`.

### Step 3 — Move `spawn_triple_extraction` to `episode/`

Triple extraction is an episode concern. Move the spawn + SQL logic to
`src/service/episode/edges.rs` or a new `episode/triples.rs`. `ServiceContext`
calls `episode::spawn_triple_extraction(ctx, fact_id, content, namespace)`.

### Step 4 — Keep shared infrastructure on `ServiceContext`

These stay (they're genuine cross-cutting concerns):
- `find_record_by_id`, `find_episode_record`, `find_fact_record`,
  `find_entity_record_by_id` — shared record lookups
- `enforce_rate_limit` — cross-cutting
- `namespace_for_scope` — shared
- `record_fact_access` — shared
- `log_tool_event`, `log_tool_event_with_duration` — shared logging
- `is_query_logging_enabled`, `query_log_retention_days` — shared config
- `context_store`, `context_access_log` — shared accessors

Target: `ServiceContext` returns to ~250 lines (fields + shared helpers).

**ADR needed?** No — this completes the capability-seam plan (Candidate 1 from
the first audit), Step 6 which was deferred. No new decision.

## Candidate 3 — Join detached tokio::spawn tasks

**Problem:** 5 `tokio::spawn` call sites discard `JoinHandle`s.

### Step 1 — Fix lifecycle workers (decay/archival/community)

`spawn_workers_from_config` currently returns `()` and discards 3 handles.
Change it to return a `LifecycleWorkerRuntime` struct (mirroring
`ClaimWorkerRuntime`):

```rust
pub struct LifecycleWorkerRuntime {
    shutdown: CancellationToken,
    handles: Vec<JoinHandle<()>>,
}

impl LifecycleWorkerRuntime {
    pub async fn shutdown(&self) { /* cancel + drain */ }
}
```

Store it on `MemoryService`. Call `shutdown()` in `cli/runtime.rs` alongside
the existing `claim_worker.shutdown()` and `lifecycle_worker.shutdown()`.

### Step 2 — Fix `spawn_triple_extraction`

Two options:
- **(a)** Return the `JoinHandle` from `spawn_triple_extraction` and store it
  in a `JoinSet` on `ServiceContext` (or `MemoryService`). On shutdown, abort
  remaining extractions.
- **(b)** Use `tokio::task::JoinSet` with a bounded concurrency limit (e.g. 4
  concurrent extractions). When the set is full, `spawn` blocks (or the
  extraction is skipped with a warning log).

**Recommendation:** (b) — `JoinSet` with bounded concurrency. Triple extraction
is best-effort; if the set is full, skip with a log. This prevents unbounded
task creation under high write load.

### Step 3 — Background embedding tasks (service_context.rs:709,740)

These are retry/background embedding tasks. Verify they have a natural
termination condition (they do: the retry loop exhausts attempts). If they
don't, add a `CancellationToken`. If they do, document that they're
self-terminating and add a comment.

**ADR needed?** No — this is a resource-management bug fix. No architectural
decision.

## Candidate 4 — Add memory-management tests

**Depends on:** Candidate 1 (the leak fix must exist before we can test
bounded behavior).

### Step 1 — Trace registry bounds tests

(See Candidate 1 Step 4 — same tests, listed there.)

### Step 2 — ExposureTraceStore LRU through the orchestrator

Add to `src/service/agent_memory/recall.rs::orchestrator_tests`:
`repeated_recall_same_session_does_not_grow_unbounded` — call `execute` 100
times with distinct tasks, assert the trace store for that session has ≤ 32
entries.

### Step 3 — Worker shutdown test

Add to `tests/agent_memory_lifecycle_e2e.rs`:
`lifecycle_workers_shutdown_cleanly` — start a service with lifecycle enabled,
trigger `shutdown()`, assert all worker handles are joined (no panic, no hang).

**ADR needed?** No.

## Sequencing

```text
C1 (fix leak) ──► C4 (memory tests) ── can run in parallel with ──► C2 (push logic down)
                                  C3 (join spawns) ── can run in parallel with ──► C2
```

C1 is first (unblocks C4). C3 is independent. C2 is the largest and can run
in parallel with C1+C4 since it touches different files (capabilities vs
recall.rs).

## Out of scope

- Removing the `MemoryService` delegator methods (deferred per the capability-
  seam plan Step 6 — keep for backward compat)
- Changing the `std::sync::Mutex` on `SessionTraceRegistry` to `tokio::sync::Mutex`
  (not needed — the lock is never held across `.await`)
- Adding a dedicated eviction timer task (amortized eviction in `record()` is
  simpler and sufficient)
