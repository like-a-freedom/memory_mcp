# NER Extractor Comparison — 2026-08-11 (all five backends, fully working)

> **Machine:** Apple M5 Pro, 18 cores, macOS. All runs local, CPU-only, offline.
> **Corpus:** `evals/corpora/ner/ner_quality.json` — 10 hand-annotated RU/EN/mixed cases,
> labels `person,company,location,product,event,technology`, threshold 0.5 for model kinds.
> **Checkpoints:** all five present under `crates/memory-mcp/tests/models/ner/` (gitignored):
> `urchade--gliner_multi-v2.1` (1.1 GB), `deepanwa--NuNerZero_onnx` (1.7 GB),
> `VAGOsolutions--SauerkrautLM-LFM2.5-GLiNER` (1.5 GB) + the offline regex/anno rules.
> **Artifacts:** `/tmp/ner-quality-5kinds.json` (quality), bench log captured but not committed
> (reproduce with the commands below).

## Summary

| Kind | Mention F1 | Mention P | Mention R | Typed F1* | Warm single | Warm multi | Cold start |
|---|---|---|---|---|---|---|---|
| `regex` | 0.7447 | 0.7292 | 0.7609 | 0.4217 | **0.84 µs** | **2.33 µs** | 1.28 ms |
| `anno` | 0.7473 | 0.7556 | 0.7391 | 0.3533 | 5.23 µs | 39.6 µs | 177 µs |
| `anno-onnx` (`deepanwa/NuNerZero_onnx`) | 0.2185 | 0.1781 | 0.2826 | 0.2214 | **36.7 ms** | 101.7 ms | 1.03 s (session) |
| `gliner` (`urchade/gliner_multi-v2.1`) | 0.9184 | 0.8654 | **0.9783** | 0.8770 | 145 ms | 508 ms | 217 µs (ctor) |
| `vago` (`VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER`) | **0.9302** | **1.0000** | 0.8696 | **0.9038** | 159 ms | 704 ms | 227 µs (ctor) |

\* Typed F1 = per-case average of the `ner_typed_f1` diagnostic (name **and** label match); the
report's suite F1 is the authoritative mention-level comparison (aggregated tp/fp/fn across cases).
Per-case mention-F1 averages: regex 0.7626, anno 0.7767, anno-onnx 0.2214, gliner 0.9106, vago 0.9038.

**Takeaway — quality:** VAGO LFM2 is the quality leader (F1 0.9302, perfect precision, best typed
agreement at 0.90) at the highest latency (159 ms single). GLiNER is a close second (F1 0.9184)
with the best recall (0.978) at similar latency. The rule-based kinds are statistically tied
(F1 ≈ 0.745) and mislabel 55–65% of mentions (typed F1 0.35–0.42). **anno-onnx is the fastest
model backend (37 ms single, ~4× faster than the Candle models) and always labels what it finds
correctly (typed F1 = mention F1), but its export is `max_width=1` — single-word spans only — so
it scores 0.2185 against a corpus full of multi-word entities: it fragments "Alice Smith" into
"Alice" + "Smith" and splits product names, killing precision (0.178) and recall (0.283).**

**Takeaway — performance:** rules are 10⁵–10⁶× faster than the models (sub-µs vs 10s–100s of ms
per window). Among models, **anno-onnx is the fastest** (37 ms single — ONNX Runtime CPU beats
the Candle backends ~4×), then gliner (~145 ms), then vago (~159 ms). All model backends have
per-window latencies that make them unsuitable for high-throughput ingestion without batching.

## Performance (CPU latency, criterion `--noplot`, warm steady-state)

Command: `cargo bench -p eval-harness --bench ner_cpu -- --noplot`

| Bench | Median | vs regex (single) |
|---|---|---|
| `regex_single_window_warm` | 0.84 µs | 1× |
| `regex_multi_window_warm` | 2.33 µs | 2.8× |
| `anno_single_window_warm` | 5.23 µs | 6.2× |
| `anno_multi_window_warm` | 39.6 µs | 47× |
| `anno_onnx_single_window_warm` | 36.7 ms | ≈43,700× |
| `anno_onnx_multi_window_warm` | 101.7 ms | ≈121,000× |
| `gliner_single_window_warm` | 145 ms | ≈173,000× |
| `gliner_multi_window_warm` | 508 ms | ≈605,000× |
| `vago_single_window_warm` | 159 ms | ≈189,000× |
| `vago_multi_window_warm` | 704 ms | ≈839,000× |
| `default_service_extract_warm` | 1.71 ms | — (anno + DB round trip; not comparable) |

Cold starts (extractor build): regex 1.28 ms, anno 177 µs, gliner 217 µs (ctor; model loads on
first inference), vago 227 µs (ctor), anno-onnx 1.03 s (eager ONNX session build).

## Quality (10-case RU/EN/mixed corpus)

Command:
```bash
cargo run -p eval-harness --bin memory-eval -- run \
  --profile evals/profiles/ner_quality.json --artifact target/eval-ner.json \
  --suite ner-quality-anno --suite ner-quality-regex --suite ner-quality-anno-onnx \
  --suite ner-quality-gliner --suite ner-quality-vago
```

Suite-aggregated mention metrics (authoritative):

| Suite | Passed/Total | Precision | Recall | F1 |
|---|---|---|---|---|
| `ner-quality-anno` | 5/10 | 0.7556 | 0.7391 | 0.7473 |
| `ner-quality-regex` | 4/10 | 0.7292 | 0.7609 | 0.7447 |
| `ner-quality-anno-onnx` | 0/10 | 0.1781 | 0.2826 | 0.2185 |
| `ner-quality-gliner` | 3/10 | 0.8654 | 0.9783 | 0.9184 |
| `ner-quality-vago` | **7/10** | **1.0000** | 0.8696 | **0.9302** |

Per-case diagnostic averages (mean of 10 cases; `ner_typed_f1` = name + label agreement):

| Suite | mention_f1 avg | typed_f1 avg |
|---|---|---|
| `ner-quality-regex` | 0.7626 | 0.4217 |
| `ner-quality-anno` | 0.7767 | 0.3533 |
| `ner-quality-anno-onnx` | 0.2214 | 0.2214 |
| `ner-quality-gliner` | 0.9106 | 0.8770 |
| `ner-quality-vago` | 0.9038 | 0.9038 |

Note: `RESULT: QUALITY FAILED` is expected — the corpus is deliberately hard for rule-based
extractors; the harness renders the matrix regardless. anno-onnx and vago are the only
extractors whose typed agreement equals their mention agreement (they never mislabel a kept
mention); anno-onnx's low score is pure coverage loss from `max_width=1`, not label noise.

## Scenario guidance

- **High-throughput / privacy / offline-first ingestion** (no model in the loop): **regex or anno** —
  sub-50 µs per window, zero download. Pick by taste: regex recalls more (0.761 vs 0.739), anno is
  more precise (0.756 vs 0.729). Budget a correction layer — typed labels agree only ~35–42%.
- **Quality-sensitive extraction** (agents, enrichment, dedup feeding), recall matters most:
  **classic GLiNER** — recall 0.978, F1 0.918, ~145 ms/extraction, 1.1 GB checkpoint.
- **Maximum precision + typed correctness, latency-insensitive**: **VAGO LFM2** — F1 0.930,
  precision 1.000, typed 0.90, ~159 ms/extraction, 1.5 GB checkpoint. Best when a wrong label is
  worse than a miss (e.g. audit trails, claim typing).
- **Fastest neural option, single-word entities only**: **anno-onnx** — 37 ms/extraction
  (~4× faster than the Candle models), labels always right, but `max_width=1` means it cannot
  span multi-word entities ("Alice Smith", "Pixel 8 Pro"). Best for single-token entity
  extraction (names, orgs, locations) where speed per extraction beats recall.
- **Rules vs models**: if the downstream pipeline can fuse/split mentions, rules at 10⁵× less
  latency may beat anno-onnx's coverage; if labels must be trusted, anno-onnx/vago never
  mislabel.

## Implementation notes

1. **VAGO checkpoint-format fix** (2026-08-11). The upstream state dict wraps every
   `bert_layer.*` encoder tensor under a `token_rep_layer.` outer prefix (span-encoder wrapper);
   the RNN/head tensors stay bare. `Lfm2Gliner::new_from_checkpoint` prepends `token_rep_layer.`
   on `bert_layer.*` lookups via `VarBuilder::rename_f`, pinned by
   `adapt_weights_accepts_token_rep_layer_wrapped_checkpoint` in
   `crates/memory-mcp/src/service/entity_extraction/lfm2_gliner/tensors.rs`.
2. **anno-onnx span protocol implementation** (2026-08-11). The ONNX export requires six inputs —
   the GLiNER `SpanProcessor` span tensors `span_idx`/`span_mask` in addition to the four prompt
   tensors — and fixes the span dimension to `max_width=1` (output `[1, num_words, 1, num_classes]`,
   class ids 0-based). `anno_onnx.rs` now builds `span_idx = (i, i)` per word with an all-`true`
   `span_mask` and feeds all six inputs; the existing `decode_scores` already handled the
   `[1, len, 1, classes]` shape. Verified by a **Python parity gate**
   (`evals/corpora/ner/anno_onnx_release_parity.json`, generated by `gen_anno_onnx_parity.py`
   against the real model): the native extractor reproduces the Python reference (name, label)
   sets on all 10 corpus cases exactly. Real-model tests in
   `crates/memory-mcp/tests/anno_onnx_integration.rs` (`#[ignore]`d, checkpoint-gated).
