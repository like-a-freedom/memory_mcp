# ADR-0035: GLiNER Lazy Load with Idle Unload

## Status
Accepted (renumbered from duplicate ADR-0030 on 2026-08-07)

Amended by: ADR-0034 for allocator and recommended idle-unload policy.

## Context
The GLiNER model (~1.1 GB f32 weights) is loaded eagerly at service startup
(core/builder.rs) and retained for the process lifetime. Live measurement
showed RSS of 7.3 GB, dominated by (a) ~1.5 GB live heap weights and (b)
~5.2 GB of freed-but-retained macOS malloc arenas accumulated from repeated
extract activity. 99% of usage is single-shot extract followed by long idle.

## Decision
Load the GLiNER model lazily on first extraction for every configuration.
Optionally unload it after N seconds of inactivity, controlled by
`GLINER_IDLE_UNLOAD_SECS` (seconds). The default `0` disables idle unloading:
the model is still loaded on first extraction, then retained for the process
lifetime. Implementation: a
generic LazyModel<T> state machine (tokio Mutex + spawn_blocking load +
tokio::time::sleep unload task). Unload is armed AFTER each extract
completes (arm_unload), so the idle clock measures time since last USE
COMPLETION, not load time — long extracts cannot trigger a mid-inference
unload. Unload semantics: guard.loaded = None; in-flight extracts keep their
Arc<T> clone alive until they finish (never dropped mid-use). Model weights
are loaded via from_buffered_safetensors so the buffer is the single owner
of the weight bytes and is freed deterministically.

Note on the allocator: unload alone returns the ~1.5 GB MALLOC_LARGE model
allocation to the OS even with default malloc (large zones unmap on free),
but the ~5.2 GB MALLOC_SMALL (empty) arena term persists without mimalloc
(ADR-0032). Both levers are required to bring the per-process RSS number
down; unload alone fixes the physical footprint.

## Consequences
+ Idle footprint (memory pressure) collapses to the SurrealDB floor.
+ Idle RSS collapses to ~50-300 MB when built with mimalloc (ADR-0032).
+ Peak during an active extract is unchanged (~1.6-2.2 GB) — unavoidable
  without a smaller model (weights are 1.1 GB).
+ First extract after idle pays cold-load latency (~1-2 s, single-shot OK).
+ Concurrency is safe: exactly-once load under the state lock; unload task
  re-checks last_used before dropping; arms only after use completes.
- Lazy loading is always active. Idle unloading is opt-in; the default `0`
  retains a model after its first extraction.
- RSS benefit of unload without mimalloc is partial (arena retention).
