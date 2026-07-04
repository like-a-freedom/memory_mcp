# NER Performance

## Reproducible Benchmark Protocol

### Prerequisites

- Local GLiNER model at `tests/models/ner/urchade--gliner_multi-v2.1/`
- Release build: `cargo build --release`
- Stable machine power mode (no thermal throttling)
- No concurrent model workload

### Running the Benchmark

```bash
# Full NER latency benchmark
TEST_THREADS=1 cargo test --release --test eval_ner_latency run_gliner_latency_eval -- --ignored --exact --nocapture

# Or via make
make eval-ner-latency
```

### What is Measured

The benchmark runs 10 iterations of `extract_candidates` on a 40-paragraph corpus (~640 words) and reports:

- `p50_ms`: median latency
- `p95_ms`: 95th percentile latency
- `candidates`: final extracted entity list (for equivalence checking)

### Stage-Level Telemetry

With `RUST_LOG=debug`, the GLiNER extractor emits:

- `ner.gliner.span_scores.done`: vectorized span scoring duration and span count
- `ner.gliner.queue.done`: inference gate queue-wait time (Task 5)

## Accepted Configuration

| Parameter | Default | Description |
|-----------|---------|-------------|
| `NER_BATCH_SIZE` | 4 | Max windows per transformer forward pass |
| `NER_MAX_BATCH_TOKENS` | 1536 | Max padded tokens per batch |
| `NER_MAX_CONCURRENCY` | 1 | Concurrent inference limit |
| `NER_DEVICE` | `cpu` | Device: `cpu`, `metal`, or `auto` |
| `NER_THRESHOLD` | 0.5 | Confidence threshold for span acceptance |

## Original Release Baseline

> To be filled after first benchmark run with the unoptimized implementation.

```json
{
  "provider": "gliner",
  "iterations": 10,
  "content_words": 640,
  "p50_ms": null,
  "p95_ms": null,
  "candidates": []
}
```

## Rollout Gates

1. **Task 1–5 (CPU optimization):** Each task ships independently. The optimized CPU implementation must return the same ordered `(canonical_name, entity_type)` candidates as the reference implementation.
2. **Task 6 (MCP Tasks):** Protocol-level timeout avoidance. No performance gate — correctness and backward compatibility only.
3. **Task 7 (Metal):** Opt-in accelerator. Shipped only after CPU results are known. Failed Metal initialization falls back to CPU when `NER_DEVICE=auto`.

## Machine Description

> To be filled with OS, CPU, and build details after baseline capture.
