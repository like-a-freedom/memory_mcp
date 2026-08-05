# 0033: SurrealDB/RocksDB Memory Footprint

## Status
Accepted (investigation — controls EXIST, not enabled by this plan)

## Context
Embedded SurrealDB (kv-rocksdb) contributes to the process floor and, with
engine defaults, is the LARGEST hidden memory term after the GLiNER model it
would leave behind once unload lands.

Verified against surrealdb-core 3.0.0 sources
(`~/.cargo/registry/src/*/surrealdb-core-3.0.0/src/kvs/rocksdb/cnf.rs`):

| Knob                                | Env variable                          | Engine default |
|-------------------------------------|---------------------------------------|----------------|
| Block cache (reads)                 | `SURREAL_ROCKSDB_BLOCK_CACHE_SIZE`    | `max(16 MiB, total_system_ram/2 - 1 GiB)` — on a 64 GB Mac ≈ **31 GiB ceiling** |
| Write buffer size (per buffer)      | `SURREAL_ROCKSDB_WRITE_BUFFER_SIZE`   | 32-128 MiB dynamic by RAM |
| Max write buffers                   | `SURREAL_ROCKSDB_MAX_WRITE_BUFFER_NUMBER` | engine default |

The block cache default is deliberately generous (designed for a dedicated
server); embedded use on a developer laptop gets the same number. It is a
ceiling, not an allocation — RSS only grows as pages are actually cached —
but over days of uptime it explains part of the non-GLiNER drift.

## Decision
Do NOT set these knobs in this plan (out of scope; user workload is dominated
by GLiNER + malloc arenas). DOCUMENT them here so the idle-RSS investigation
in Task 11 has a next lever to pull if the post-mimalloc floor proves higher
than the ~50-300 MB target.

Follow-up (NOT in this plan): if the post-unload idle floor exceeds ~300 MB,
set e.g. `SURREAL_ROCKSDB_BLOCK_CACHE_SIZE=268435456` (256 MiB) and re-measure.
That is a documented user-facing env var of surrealdb 3.0.0, no code change.

## Consequences
+ The full idle-RSS budget is now enumerated; no unexplained term remains.
- Actual DB floor is workload-dependent; do not claim a hard number until
  Task 11 measures it.
