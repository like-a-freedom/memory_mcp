# NER Performance

## Result

On the measured Apple M2 Pro CPU profile, vectorized span scoring is 20.6–23.8×
faster on the 520-word corpus. Complete `extract_candidates` latency is 3.45×
faster at p50 and 3.53× faster at the observed p95/max. These effects are not
multiplied together: after vectorization, transformer inference and tokenization
dominate the request.

The accepted CPU default is `NER_BATCH_SIZE=1`. On this corpus, batching three
uneven windows increased padding work and was slower than three batch-one forward
passes.

## Measurement provenance

- Baseline commit: `32db8f99f44a` (telemetry and harness present, before vectorization)
- Final working tree: audited implementation after the NER and MCP Tasks fixes
- Model: `urchade/gliner_multi-v2.1`
- Backend: CPU
- Labels: `person, company, location, product, event, technology`
- Threshold: `0.5`
- Batch token cap: `1536`
- Max inference concurrency: `1`
- Machine: Apple M2 Pro, 10 CPU cores, 32 GB RAM, arm64
- OS/toolchain: macOS 26.5.1, rustc/cargo 1.96.1
- Method: one warm-up followed by 10 measured release iterations; model loading excluded

The detached baseline worktree changed only the benchmark harness. Candidate output
was stable on every iteration, and both final scenario signatures matched the ordered
baseline signatures exactly. The final batch-one and batch-four extractors also
returned exactly the same ordered default-label and zero-shot candidates.

## Reproduction

Performance measurements now use Criterion benchmarks under `crates/eval-harness/benches/`.
The old `eval_ner_latency` and `eval_latency` integration tests have been removed.

```bash
# NER CPU benchmarks (one-window and multi-window)
cargo bench -p eval-harness --bench ner_cpu -- --noplot

# NER Metal benchmarks (macOS only, requires --features metal)
cargo bench -p eval-harness --features metal --bench ner_metal -- --noplot

# Contention benchmarks (multi-client concurrency)
cargo bench -p eval-harness --bench contention -- --noplot

# Full pipeline benchmarks (ingest, extraction, claims, retrieval, end-to-end)
cargo bench -p eval-harness --bench pipeline -- --noplot
```

Run without another model workload and with stable machine power/thermal settings.
The harness fixes the model, device, threshold, token cap, and concurrency, and emits
all raw samples plus the effective configuration as JSON.

## Baseline versus final CPU latency

| Scenario | Words | Windows | Baseline p50 | Final p50 | p50 speedup | Baseline p95 | Final p95 | p95 speedup |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| One window | 104 | 1 | 1622.121 ms | 446.138 ms | 3.64× | 1866.864 ms | 486.088 ms | 3.84× |
| Multi-window | 520 | 3 | 8791.471 ms | 2546.692 ms | 3.45× | 9369.734 ms | 2653.017 ms | 3.53× |

The multi-window p50 reduction is 71.0%; the p95/max reduction is 71.7%.

Raw `extract_candidates` samples, milliseconds:

```text
baseline one-window:
1866.864, 1800.196, 1617.935, 1663.307, 1617.166,
1616.423, 1620.805, 1640.807, 1622.121, 1610.651

final one-window, batch=1:
473.051, 441.724, 442.734, 443.285, 442.950,
452.075, 446.138, 454.994, 457.707, 486.088

baseline multi-window:
8411.930, 8395.734, 9369.734, 8876.638, 8649.346,
8429.107, 8818.323, 8725.948, 9035.288, 8791.471

final multi-window, batch=1:
2523.445, 2601.681, 2589.907, 2653.017, 2640.516,
2574.170, 2504.591, 2543.258, 2546.692, 2524.613
```

With 10 samples, nearest-rank p95 is the maximum observed sample. Treat it as a
reproducible local comparison, not as a production tail-latency estimate.

## Span-scoring stage

`ner.gliner.span_scores.done` was measured per request; the multi-window value is
the sum of its three window events.

| Scenario | Baseline span p50 | Final span p50 | Speedup | Baseline span p95 | Final span p95 | Speedup |
|---|---:|---:|---:|---:|---:|---:|
| One window | 1165 ms | 44 ms | 26.5× | 1379 ms | 137 ms | 10.1× |
| Multi-window | 6120 ms | 257 ms | 23.8× | 6743 ms | 327 ms | 20.6× |

The final stage-level sample set above was captured with `batch=4`; span scoring
itself still runs per decoded window. The accepted end-to-end CPU numbers use
`batch=1`.

## Batch-size decision

| Multi-window setting | p50 | p95/max | Padded execution shape |
|---|---:|---:|---|
| `NER_BATCH_SIZE=1` | 2546.692 ms | 2653.017 ms | 3 batches, largest batch 1 |
| `NER_BATCH_SIZE=4` | 3039.927 ms | 3367.643 ms | 1 batch, 3 windows, 1152 padded tokens |

Batch one was 16.2% faster at p50 and 21.2% faster at p95/max. Larger batches
remain available for other models and window distributions, but must be enabled
only after workload-specific measurement.

## Four-client contention

With `NER_MAX_CONCURRENCY=1`, four simultaneous 520-word requests are deliberately
serialized:

```text
round wall times: 10228.698, 10064.770, 10392.037 ms
wall p50:         10228.698 ms
wall p95/max:     10392.037 ms
request p95/max:  10392.007 ms
throughput:       0.391 requests/s
queue wait range: 0–7814 ms
```

This is a stability/oversubscription tradeoff, not a throughput gain. Raising
`NER_MAX_CONCURRENCY` requires a separate CPU, latency, and memory benchmark.

## Quality gates

- Exact ordered candidate parity for batch 1 versus batch 4, including per-call
  zero-shot labels.
- Padded versus unpadded CPU hidden-state diagnostic with `atol=5e-5` and
  `rtol=1e-4`. Different float32 GEMM/softmax reduction shapes produce an observed
  maximum drift of `2.3305416e-5`; final candidate parity remains exact.
- Batch telemetry records window count, batch count, largest batch, configured token
  cap, and actual maximum padded tokens.
- Per-call custom labels are used when decoding scores; they no longer fall back to
  the extractor's default label list.
- Criterion benchmarks under `crates/eval-harness/benches/ner_cpu.rs` record
  candidate signatures, expected entities, and deterministic windowing as unit
  tests that pass without measuring milliseconds.

The CPU tolerance is not an acceptance threshold for Metal. Metal needs its own
candidate, quality, latency, contention, and memory measurements.

## MCP Tasks

Task-capable clients add task metadata to the normal `tools/call` request for
`extract`, then use `tasks/get`, `tasks/list`, `tasks/result`, and `tasks/cancel`.
Synchronous `tools/call` remains backward compatible.

The project uses rmcp 2.2.0 model/transport types but owns the lifecycle store in the
MCP adapter because rmcp's generated task handler still consumes results and loses
some terminal states. Wire-level stdio tests cover stable timestamps, list/get,
repeatable results, related-task metadata, original tool/protocol failures,
cancellation, invalid cursors, unknown IDs, and forbidden task augmentation.

Operational bounds are intentionally fixed and small:

- 64 active tasks
- 1024 retained tasks
- accepted TTL range: 1 second to 1 hour (default 5 minutes)
- result poll interval: 100 ms

Cancellation is cooperative at the Tokio task boundary. It cannot preempt a Candle
CPU kernel that is already executing synchronously; the NER concurrency gate still
bounds such work.

## Device policy

- `NER_DEVICE=cpu` is the production default.
- `NER_DEVICE=metal` is strict: missing feature support or initialization failure is
  a configuration error, with no silent CPU fallback.
- `NER_DEVICE=auto` may fall back to CPU and logs the selected backend.

Metal remains experimental. Its build and performance gates were not completed in
this audit, so neither `metal` nor `auto` is a production recommendation.

## Known measurement gaps

- Peak RSS was not captured: macOS `/usr/bin/time -l` could not read the required
  sysctl inside the sandbox, and the out-of-sandbox measurement was unavailable.
- The benchmark measures direct GLiNER `extract_candidates`, not SurrealDB writes,
  graph updates, MCP transport, or full `extract_from_episode` latency. Therefore the
  plan's 70% full-extract NER-cost exit valve is not claimed as completed.
- CoreML is not used. The optional Apple backend is Candle Metal, and no Metal
  speedup is claimed.

## Production settings

```bash
NER_DEVICE=cpu \
NER_BATCH_SIZE=1 \
NER_MAX_BATCH_TOKENS=1536 \
NER_MAX_CONCURRENCY=1 \
cargo run --release -- serve
```
