# NER Extractor Comparison — 2026-08-11 (all five backends)

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
| `regex` | 0.7447 | 0.7292 | 0.7609 | 0.4217 | **0.87 µs** | **2.38 µs** | 1.24 ms |
| `anno` | 0.7473 | 0.7556 | 0.7391 | 0.3533 | 5.17 µs | 39.4 µs | 186 µs |
| `gliner` (`urchade/gliner_multi-v2.1`) | 0.9184 | 0.8654 | **0.9783** | 0.8770 | 134 ms | 860 ms† | 202 µs (ctor) |
| `vago` (`VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER`) | **0.9302** | **1.0000** | 0.8696 | **0.9038** | 790 ms | 619 ms | 872 µs (ctor) |
| `anno-onnx` (`deepanwa/NuNerZero_onnx`) | — | — | — | — | **skipped** | **skipped** | 999 ms (session) |

\* Typed F1 = per-case average of the `ner_typed_f1` diagnostic (name **and** label match); the
report's suite F1 is the authoritative mention-level comparison (aggregated tp/fp/fn across cases).
Per-case mention-F1 averages: regex 0.7626, anno 0.7767, gliner 0.9106, vago 0.9038.
† gliner multi-window is noisy (this run 706 ms–1.03 s; an earlier run 517–524 ms).

**Takeaway — quality:** VAGO LFM2 is the quality leader (F1 0.9302, **perfect precision**, and the
best typed agreement at 0.90 — every detected mention is correctly labeled), at the cost of the
highest latency (790 ms single) and the largest checkpoint (1.5 GB). GLiNER is a close second
(F1 0.9184) with the best recall (0.978) at ~6× lower latency (134 ms). The rule-based kinds are
statistically tied (F1 ≈ 0.745) and mislabel 55–65% of mentions (typed F1 0.35–0.42). **anno-onnx
does not run** — its production loader feeds a 4-input token-mode protocol, but the repo's only
ONNX export is a 6-input span-based protocol (`span_idx`/`span_mask`), so every case is `Invalid`
(see Gaps).

**Takeaway — performance:** rules are 10⁵–10⁶× faster than the models (sub-µs vs 100s of ms per
window). Among models, GLiNER is ~6× faster than VAGO single-window; VAGO's multi-window is
comparable to GLiNER's (619 ms vs 860 ms) because the batch/token limits amortize.

## Performance (CPU latency, criterion `--noplot`, warm steady-state)

Command: `cargo bench -p eval-harness --bench ner_cpu -- --noplot`

| Bench | Median | vs regex (single) |
|---|---|---|
| `regex_single_window_warm` | 0.867 µs | 1× |
| `regex_multi_window_warm` | 2.38 µs | 2.7× |
| `anno_single_window_warm` | 5.17 µs | 6× |
| `anno_multi_window_warm` | 39.4 µs | 45× |
| `gliner_single_window_warm` | 134 ms | ≈155,000× |
| `gliner_multi_window_warm` | 860 ms (noisy; earlier 521 ms) | ≈993,000× |
| `vago_single_window_warm` | 790 ms | ≈911,000× |
| `vago_multi_window_warm` | 619 ms | ≈714,000× |
| `default_service_extract_warm` | 1.77 ms | — (anno + DB round trip; not comparable) |
| `anno_onnx_*` | skipped | — `extraction fails: Missing Input: span_mask` |

Cold starts (extractor build): regex 1.24 ms, anno 186 µs, gliner 202 µs (ctor; model loads on
first inference), vago 872 µs (ctor), anno-onnx 999 ms (ONNX session build — before its
inference failure).

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
| `ner-quality-gliner` | 3/10 | 0.8654 | 0.9783 | 0.9184 |
| `ner-quality-vago` | **7/10** | **1.0000** | 0.8696 | **0.9302** |
| `ner-quality-anno-onnx` | 0/10 | — | — | — (all Invalid) |

Per-case diagnostic averages (mean of 10 cases; `ner_typed_f1` = name + label agreement):

| Suite | mention_f1 avg | typed_f1 avg |
|---|---|---|
| `ner-quality-regex` | 0.7626 | 0.4217 |
| `ner-quality-anno` | 0.7767 | 0.3533 |
| `ner-quality-gliner` | 0.9106 | 0.8770 |
| `ner-quality-vago` | 0.9038 | 0.9038 |

Note: `RESULT: QUALITY FAILED` is expected — the corpus is deliberately hard for rule-based
extractors; the harness renders the matrix regardless. VAGO is the only extractor whose typed
agreement equals its mention agreement (its perfect precision means every mention it keeps is
labeled right; it misses ~13% of gold mentions).

## Scenario guidance

- **High-throughput / privacy / offline-first ingestion** (no model in the loop): **regex or anno** —
  sub-50 µs per window, zero download. Pick by taste: regex recalls more (0.761 vs 0.739), anno is
  more precise (0.756 vs 0.729). Budget a correction layer — typed labels agree only ~35–42%.
- **Quality-sensitive extraction** (agents, enrichment, dedup feeding), latency-sensitive:
  **classic GLiNER** — recall 0.978, F1 0.918, ~134 ms/extraction, 1.1 GB checkpoint. Best when
  recall matters most.
- **Maximum precision + typed correctness, latency-insensitive**: **VAGO LFM2** — F1 0.930,
  precision 1.000, typed 0.90, but ~790 ms/extraction and a 1.5 GB checkpoint. Best when a wrong
  label is worse than a miss (e.g. audit trails, claim typing).
- **anno-onnx**: unusable until the loader supports the repo's span-based ONNX protocol (see Gaps).

## Gaps

1. **anno-onnx does not run (production loader ↔ ONNX export mismatch).** The repo
   `deepanwa/NuNerZero_onnx` ships a single `model.onnx` that requires six inputs (`input_ids`,
   `attention_mask`, `words_mask`, `text_lengths`, `span_idx`, `span_mask`) and returns span-level
   `logits [batch, seq, num_spans, num_classes]` (the GLiNER `SpanORTModel` protocol,
   `gliner==0.2.3`). The production `anno_onnx` loader feeds only four inputs and decodes
   token-mode logits, so every extraction fails with `Missing Input: span_mask`. The fixture was
   never present during development, so this latent mismatch was only exposed by this eval.
   **Follow-up:** implement the span-based ONNX inference protocol (span enumeration from word
   offsets up to `max_width`, `span_idx`/`span_mask` tensors, span-logits decode) as a dedicated
   production task, using `gliner/onnx/model.py` (SpanORTModel) + `data_processing/processor.py`
   as reference. Until then the eval suite honestly reports `Invalid` for this backend.
2. **VAGO loader checkpoint-format fix (done this session).** The upstream state dict wraps every
   `bert_layer.*` encoder tensor under a `token_rep_layer.` outer prefix (span-encoder wrapper);
   the RNN/head tensors stay bare. `Lfm2Gliner::new_from_checkpoint` now prepends
   `token_rep_layer.` on `bert_layer.*` lookups via `VarBuilder::rename_f`, pinned by
   `adapt_weights_accepts_token_rep_layer_wrapped_checkpoint` in
   `crates/memory-mcp/src/service/entity_extraction/lfm2_gliner/tensors.rs`.
