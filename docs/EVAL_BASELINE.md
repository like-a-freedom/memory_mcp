# Eval Baseline — 2026-04-12

## Retrieval (кастомный)

suite=eval_retrieval total=60 passed=60 recall_at_5=1.00 precision_at_5=n/a mrr=n/a pass_rate=1.00
expected_tier=alias total=10 passed=10 pass_rate=1.00
expected_tier=direct total=15 passed=15 pass_rate=1.00
expected_tier=graph total=15 passed=15 pass_rate=1.00
expected_tier=reasoning total=10 passed=10 pass_rate=1.00
expected_tier=temporal total=10 passed=10 pass_rate=1.00
actual_tier=direct total=31
actual_tier=graph total=14
actual_tier=temporal total=15

### Coverage snapshot

retrieval_case_coverage {"alias": 10, "direct": 15, "graph": 15, "reasoning": 10, "temporal": 10}

## LongMemEval

suite=longmemeval total=1 passed=1 recall_at_5=1.00 pass_rate=1.00
expected_tier=direct total=1 passed=1 pass_rate=1.00
actual_tier=direct total=2

## LoCoMo

suite=locomo total=1 passed=1 recall_at_5=1.00 pass_rate=1.00
expected_tier=direct total=1
actual_tier=direct total=5

## Extraction

suite=eval_extraction total=9 passed=9 entity_precision=0.57 entity_recall=1.00 entity_f1=0.73 fact_type_accuracy=1.00 warning_recall=1.00

## NER Latency (local GLiNER, CPU)

Measured on 2026-07-11. See `docs/performance/NER_PERFORMANCE.md` for raw samples,
methodology, stage telemetry, batching comparison, and limitations.

```json
{
  "baseline_commit": "32db8f99f44a",
  "final_revision": "audited_working_tree_2026-07-11",
  "provider": "gliner",
  "model": "urchade/gliner_multi-v2.1",
  "device": "cpu",
  "batch_size": 1,
  "max_batch_tokens": 1536,
  "max_concurrency": 1,
  "iterations": 10,
  "one_window": {
    "content_words": 104,
    "baseline_p50_ms": 1622.121,
    "baseline_p95_ms": 1866.864,
    "final_p50_ms": 446.138,
    "final_p95_ms": 486.088,
    "speedup_p50": 3.64,
    "speedup_p95": 3.84
  },
  "multi_window": {
    "content_words": 520,
    "window_count": 3,
    "baseline_p50_ms": 8791.471,
    "baseline_p95_ms": 9369.734,
    "final_p50_ms": 2546.692,
    "final_p95_ms": 2653.017,
    "speedup_p50": 3.45,
    "speedup_p95": 3.53
  },
  "vectorized_span_scoring_multi_window": {
    "baseline_p50_ms": 6120,
    "baseline_p95_ms": 6743,
    "final_p50_ms": 257,
    "final_p95_ms": 327,
    "speedup_p50": 23.8,
    "speedup_p95": 20.6
  },
  "candidate_parity": true,
  "contention": {
    "clients": 4,
    "rounds": 3,
    "wall_p50_ms": 10228.698,
    "wall_p95_ms": 10392.037,
    "per_request_p95_ms": 10392.007,
    "throughput_requests_per_s": 0.391
  },
  "peak_rss_bytes": null,
  "full_extract_from_episode": null
}
```

## Latency (in-memory)

suite=eval_latency ingest_p50_ms=0.41 ingest_p95_ms=2.90 assemble_p50_ms=4.14 assemble_p95_ms=12.72

## Notes

- `precision_at_5` and `mrr` are not emitted by the current retrieval harness yet, so they remain `n/a` in this baseline.
- External retrieval adapters currently exist for `longmemeval-cleaned` and `locomo`.
- Temporal tier wiring is now live for explicit temporal-marker queries: the custom retrieval suite still passes 60/60 on content recall, and the runtime now reports `actual_tier=temporal total=15` alongside `direct`/`graph`.
- Sprint 3 graph bridge coverage added five community-co-membership fixtures (`ret-056..ret-060`), raising expected graph coverage to 15 cases and observed `actual_tier=graph total=14` in the latest run.
- The current single-sample `LongMemEval` and `LoCoMo` normalized excerpts still report only `direct` actual tiers in these baseline runs; temporal coverage there remains limited by the present sample tracks and adapter mappings.
- `run_retrieval_evals` now derives `as_of` from `max(Utc::now(), latest_fixture_timestamp) + 1s` so the suite stays stable even when fixture `t_valid` values are ahead of the current wall clock while seeded facts still receive runtime `t_ingested` timestamps.
- `run_extraction_evals` now covers contradiction warnings plus preference-style `experience` and email action-item extraction, growing the suite from 2 to 9 fixture cases while keeping `fact_type_accuracy=1.00`.
- `run_retrieval_evals` now enforces the plan thresholds directly: global `recall_at_5 ≥ 0.90` plus expected-tier pass-rate targets for `direct`, `alias`, `temporal`, `graph`, and `reasoning`.
- `run_latency_evals` now enforces the in-memory p95 targets directly (`ingest ≤ 200ms`, `assemble ≤ 50ms`) instead of only printing the measured percentiles.
- **Adaptive memory alignment (2026-03-27)**: `index_keys`, `access_count`, and `last_accessed` fields added to facts. Heat-aware lifecycle workers skip hot facts during decay/archival. Timeline view mode added to `assemble_context`. LongMemEval-style acceptance tests cover 5 benchmark categories.
