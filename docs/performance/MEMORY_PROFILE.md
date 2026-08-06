# Memory Profile: GLiNER and Allocator Variants

Measured 2026-08-06 on macOS Tahoe 26.5.2, Apple Silicon ARM64, from
`master` at commit `2a592624`. This report records an actual before/after
matrix; it is not an estimate from the GLiNER implementation plan.

## Executive summary

For the reported local-use case (CPU GLiNER, one extraction, long-lived stdio
server), the best observed configuration is:

```text
GLINER_IDLE_UNLOAD_SECS=30
# default allocator; do not enable mimalloc by default
```

It reached approximately **277 MB macOS physical footprint** and **430 MB
`ps` RSS** after the model was unloaded. That is below the requested 1 GB
idle target in this fresh-process single-shot test.

The `mimalloc` feature produced an important platform-specific result:

- `mimalloc` without idle unload did not reduce the retained model footprint;
- `mimalloc` plus idle unload reduced macOS physical footprint to about
  **190 MB**, but `ps` RSS remained around **2.56 GB**;
- therefore `mimalloc` should remain optional, not become the default based on
  this benchmark. On macOS Tahoe, `footprint` is the more useful physical
  memory signal for this comparison, while RSS is still relevant because it is
  what many process monitors display.

The active extraction peak remains about **2.5--2.6 GB physical footprint**.
The committed model fixture is about 1.1 GB, so an active-extraction peak below
1 GB is not a realistic target for this model and runtime.

## Results

### Controlled matrix

All rows used a fresh server process and fresh embedded database directory.
Each process was initialized through the MCP stdio protocol, left idle for 12
seconds, sent exactly one successful `tools/call` request for `extract`, and
then observed after extraction. The idle-unload cases were observed for at
least 45 seconds; the combined `mimalloc` case was observed for 90 seconds.

| Variant | `GLINER_IDLE_UNLOAD_SECS` | Idle before extract: footprint / RSS | Active peak: footprint / RSS | Post-extract observed: footprint / RSS |
|---|---:|---:|---:|---:|
| Current default allocator, no unload | unset (`0` effective) | 113 MB / 135 MB | 2,566 MB / 2,644 MB | 1,458 MB / 1,541 MB |
| Default allocator + idle unload | `30` | 113 MB / 135 MB | 2,565 MB / 2,643 MB | **277 MB / 430 MB** |
| `mimalloc`, no unload | unset (`0` effective) | 118 MB / 142 MB | 2,532 MB / 2,557 MB | 1,462 MB / 2,557 MB |
| `mimalloc` + idle unload | `30` | 118 MB / 140 MB | 2,531 MB / 2,556 MB | **190 MB / 2,556 MB** |

`footprint` values are macOS physical-footprint readings. RSS values are
sampled from `ps -o rss`. Peak values are maxima observed during the
post-extraction observation window; the extraction response was successful in
every row.

### Diff against the current no-unload control

The default allocator plus idle unload reduced post-extraction physical
footprint from 1,458 MB to 277 MB:

- **-1,181 MB**, or approximately **-81%**;
- RSS fell from 1,541 MB to 430 MB: **-1,111 MB**, or approximately **-72%**.

The combined `mimalloc` plus idle-unload variant reduced physical footprint
from its own active peak of 2,531 MB to 190 MB, approximately **-2,341 MB
(-92%)**. However, its RSS did not fall in the same way. This is why the
allocator result must not be summarized as “mimalloc lowers RSS” on this
platform.

### Historical pre-change evidence

The original long-lived process observed before the memory-reduction work was
recorded in
`docs/superpowers/plans/2026-08-03-gliner-memory-reduction.baseline.txt` on
2026-08-05:

```text
RSS:               6,700,576 KB (about 6.7 GB decimal)
macOS footprint:   about 1.9 GB
MALLOC_LARGE:      about 503 MB resident (GLiNER weights)
MALLOC_SMALL:      about 766 MB resident
MALLOC_SMALL empty: about 5.1 GB resident (retained arenas)
```

That process was not a clean, reproducible benchmark: it had been running for
about two days, used the then-existing production environment including a
remote OpenAI-compatible embedding provider, and represented the pre-change
behavior. It is retained as historical evidence of the reported 4--7 GB RSS
ratchet, not mixed into the controlled table above.

Compared with that historical process, the controlled current configuration
with default allocator and 30-second unload reached 430 MB RSS after one
extraction. The comparison is directionally useful but not a strict causal
A/B measurement because the process age, code revision, and environment
were different.

## Methodology and provenance

Builds were isolated so enabling the feature could not overwrite the default
binary:

```bash
cargo build --release --locked \
  --target-dir target/memory-bench-default

cargo build --release --features mimalloc --locked \
  --target-dir target/memory-bench-mimalloc
```

Runtime settings for every controlled row:

```text
NER_PROVIDER=local-gliner
NER_MODEL=urchade/gliner_multi-v2.1
NER_MODEL_DIR=crates/memory-mcp/tests/models/ner/urchade--gliner_multi-v2.1
NER_DEVICE=cpu
NER_MAX_CONCURRENCY=1
EMBEDDINGS_ENABLED=false
SURREALDB_EMBEDDED=true
SURREALDB_DATA_DIR=<fresh temporary directory>
SURREALDB_DB_NAME=memory_bench
SURREALDB_NAMESPACES=org,personal
SURREALDB_USERNAME=root
SURREALDB_PASSWORD=root
```

The extraction payload was a small inline document containing person,
company, location, and project text. The same committed model fixture was used
for all rows. Embeddings were disabled deliberately so the measurement stayed
offline and did not include network-provider memory or latency; the historical
baseline used a remote embedding provider and is therefore not directly
comparable in absolute terms.

The process was driven through newline-delimited JSON-RPC:

1. `initialize` with protocol version `2025-06-18`;
2. `notifications/initialized`;
3. one synchronous `tools/call` for `extract`;
4. RSS sampled once per second with `ps`;
5. macOS physical footprint sampled with `footprint`.

The scratch driver and raw logs were kept outside the repository under
`/tmp/memory_mcp_*.log`; no application source or test code was added for the
measurement.

## Interpretation

### Why the process starts below 1 GB

The current implementation is lazy: the model is not loaded before the first
extraction. Therefore the “idle before extract” rows measure a server with no
GLiNER weights resident. This is a separate improvement from idle unloading.
With `GLINER_IDLE_UNLOAD_SECS` unset or `0`, the loaded model remains cached for
process lifetime after first use; with a positive value, it is released after
inactivity.

### Why RSS and `footprint` disagree for `mimalloc`

The combined `mimalloc` run released the model’s physical pages according to
macOS `footprint` but retained a high RSS value. This is allocator/OS accounting
behavior, not evidence that the model was still physically active: the
physical footprint dropped from about 2.53 GB to 190 MB after the idle timer.
It does mean that a user watching only RSS or Activity Monitor may still see a
large number, so enabling `mimalloc` is not a universal answer to the reported
symptom.

### Recommendation

1. Keep the current default allocator.
2. Recommend `GLINER_IDLE_UNLOAD_SECS=30` (or another workload-appropriate
   positive value) for local single-shot or infrequent extraction workloads.
3. Keep `mimalloc` as an explicit opt-in feature for users who care about
   physical footprint and have verified their own workload; do not make it the
   default from this data.
4. Treat approximately 2.5 GB as the expected active-extraction peak for this
   1.1 GB model fixture. The meaningful target is idle memory after unload,
   where the default-allocator run measured below 1 GB in both metrics.
5. For future regressions, record both `ps` RSS and macOS `footprint`; either
   metric alone can give the wrong conclusion on macOS allocator behavior.
