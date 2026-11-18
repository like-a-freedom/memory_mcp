# ADR-0033: SurrealDB/RocksDB Memory Footprint

## Status
Historical. The RocksDB-specific observations below describe the embedded engine used before v1.40.0; the current embedded engine is SurrealKV.

## Context

In the v1.39.0 implementation, embedded SurrealDB used `kv-rocksdb`. That engine
contributed to the process floor and, with its defaults, was the largest hidden
memory term after the GLiNER model targeted by the unload work. v1.40.0 switches
embedded storage to SurrealKV. The tuning variables and measurements below are
historical RocksDB notes, not configuration advice for SurrealKV. See the
[upgrade guide](../../README.md#upgrade-to-v1400) for current storage behavior.

Verified against surrealdb-core 3.0.0 sources
(`~/.cargo/registry/src/*/surrealdb-core-3.0.0/src/kvs/rocksdb/cnf.rs`);
the workspace now pins SurrealDB 3.3.5, so treat these engine defaults as a
dated observation and re-verify against the pinned version before acting on them:

| Knob                                | Env variable                          | Engine default |
|-------------------------------------|---------------------------------------|----------------|
| Block cache (reads)                 | `SURREAL_ROCKSDB_BLOCK_CACHE_SIZE`    | `max(16 MiB, total_system_ram/2 - 1 GiB)` — on a 64 GB Mac ≈ **31 GiB ceiling** |
| Write buffer size (per buffer)      | `SURREAL_ROCKSDB_WRITE_BUFFER_SIZE`   | 32-128 MiB dynamic by RAM |
| Max write buffers                   | `SURREAL_ROCKSDB_MAX_WRITE_BUFFER_NUMBER` | engine default |

The block cache default is deliberately generous (designed for a dedicated
server); embedded use on a developer laptop gets the same number. It is a
ceiling, not an allocation — RSS only grows as pages are actually cached —
but over days of uptime it explains part of the non-GLiNER drift.

## Original decision

The original investigation did not set these knobs because its workload was
dominated by GLiNER and malloc arenas. It recorded them as a possible follow-up
for measuring the idle-RSS floor after model unload and allocator changes.

Historical follow-up: if the post-unload idle floor exceeded about 300 MB,
the proposal was to set `SURREAL_ROCKSDB_BLOCK_CACHE_SIZE=268435456` (256 MiB)
and re-measure. This SurrealDB 3.0.0 setting does not tune the current SurrealKV engine.

## Historical consequences

- The original idle-RSS budget enumerated the RocksDB contribution.
- The database floor was workload-dependent; no hard number was established by
  that investigation.
