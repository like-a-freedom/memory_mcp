# NER Extractor Comparison — 2026-08-11

> **Machine:** Apple M5 Pro, 18 cores, macOS. All runs local, CPU-only, offline.
> **Corpus:** `evals/corpora/ner/ner_quality.json` — 10 hand-annotated RU/EN/mixed cases,
> labels `person,company,location,product,event,technology`, threshold 0.5 for model kinds.
> **Artifacts:** `target/eval-ner.json` (this run: `/tmp/ner-quality-3kinds.json`),
> raw bench log captured but not committed (reproduce with the commands below).

## Summary

| Kind | Mention F1 | Mention P | Mention R | Typed F1* | Warm single | Warm multi | Cold start |
|---|---|---|---|---|---|---|---|
| `regex` | **0.7447** | 0.7292 | 0.7609 | 0.4217 | **0.92 µs** | **2.47 µs** | 1.87 ms |
| `anno` | 0.7473 | 0.7556 | 0.7391 | 0.3533 | 5.44 µs | 41.5 µs | 185 µs |
| `gliner` (`urchade/gliner_multi-v2.1`) | **0.9184** | 0.8654 | **0.9783** | **0.8770** | 142 ms | 521 ms | 404 µs (ctor); ~1.5 s first load |
| `anno-onnx` | — | — | — | — | — | — | fixture missing |
| `vago` (`VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER`) | — | — | — | — | — | — | fixture missing |

\* Typed F1 = per-case average of the `ner_typed_f1` diagnostic (name **and** label match); the
report's suite F1 is the authoritative mention-level comparison (aggregated tp/fp/fn across cases).
Per-case mention-F1 averages: regex 0.7626, anno 0.7767, gliner 0.9106.

**Takeaway:** GLiNER is ~0.17 F1 better than both rule-based kinds and near-perfect on recall
(0.978) at the cost of ~150 ms per extraction (≈150k× slower than regex, ≈26k× slower than anno).
Regex and anno are statistically tied on mention F1 (0.7447 vs 0.7473); anno is more precise but
misses more, regex recalls slightly more but noisier. Typed agreement is poor for both rules
(anno 0.353, regex 0.422) — they detect mentions but mislabel ~55–65% — while GLiNER types
correctly 88% of the time.

## Performance (CPU latency, criterion `--noplot`, warm steady-state)

Command: `cargo bench -p eval-harness --bench ner_cpu -- --noplot`

| Bench | Median | vs regex (single) | Notes |
|---|---|---|---|
| `regex_single_window_warm` | 0.922 µs | 1× | |
| `regex_multi_window_warm` | 2.47 µs | 2.7× | |
| `anno_single_window_warm` | 5.44 µs | 5.9× | |
| `anno_multi_window_warm` | 41.5 µs | 45× | |
| `gliner_single_window_warm` | 142 ms | ≈154,000× | lazy load on first call |
| `gliner_multi_window_warm` | 521 ms | ≈211,000× | |
| `default_service_extract_warm` | 1.74 ms | — | anno extractor + DB round trip; not comparable to raw benches |
| `anno_onnx_*` | skipped | — | `local fixture missing` |
| `vago_*` | skipped | — | `local fixture missing` |

Cold starts (extractor build time, printed before each bench): regex 1.87 ms, anno 185 µs,
gliner 404 µs (constructor is lazy — the ~1.5 s model load happens on the first inference,
which the warm numbers above include).

## Quality (10-case RU/EN/mixed corpus)

Command:
```bash
cargo run -p eval-harness --bin memory-eval -- run \
  --profile evals/profiles/ner_quality.json --artifact /tmp/ner-quality-3kinds.json \
  --suite ner-quality-anno --suite ner-quality-regex --suite ner-quality-gliner
```

Suite-aggregated mention metrics (authoritative):

| Suite | Passed/Total | Precision | Recall | F1 |
|---|---|---|---|---|
| `ner-quality-anno` | 5/10 | 0.7556 | 0.7391 | 0.7473 |
| `ner-quality-regex` | 4/10 | 0.7292 | 0.7609 | 0.7447 |
| `ner-quality-gliner` | 3/10 | 0.8654 | 0.9783 | 0.9184 |

Per-case diagnostic averages (mean of 10 cases; `ner_typed_f1` = name + label agreement):

| Suite | mention_f1 avg | typed_f1 avg |
|---|---|---|
| `ner-quality-regex` | 0.7626 | 0.4217 |
| `ner-quality-anno` | 0.7767 | 0.3533 |
| `ner-quality-gliner` | 0.9106 | 0.8770 |

Note: `RESULT: QUALITY FAILED` is expected — the corpus is deliberately hard for rule-based
extractors (multi-word, cross-lingual mentions); the harness renders the matrix regardless.
No suite produced `invalid` outcomes, and the reducer rendered every suite.

## Scenario guidance

- **High-throughput / privacy / offline-first ingestion** (no model in the loop): **regex or anno** —
  sub-50 µs per window. Pick by recall vs precision taste: regex recalls more (0.761 vs 0.739),
  anno is more precise (0.756 vs 0.729). Budget a correction layer, since typed labels agree only
  ~35–42%.
- **Quality-sensitive extraction** (agents, enrichment, dedup feeding): **classic GLiNER** —
  F1 0.918, recall 0.978, typed 0.877. Cost: ~150 ms/extraction and a 1.1 GB checkpoint; batch
  inputs to amortize.
- **anno-onnx / VAGO LFM2**: not measured here — fixtures absent (see below). VAGO targets the
  strongest RU/EN zero-shot profile when quality must beat classic GLiNER; anno-onnx is the
  single-language CPU middle ground.

## Gaps — fixtures not present on this machine

Only the classic GLiNER checkpoint exists locally
(`crates/memory-mcp/tests/models/ner/urchade--gliner_multi-v2.1`, 1.1 GB). To complete the
matrix, prepare the other two (gitignored, manual):

| Fixture dir | Populate with |
|---|---|
| `crates/memory-mcp/tests/models/ner/deepanwa--NuNerZero_onnx/` | HF `deepanwa/NuNerZero_onnx`: `model.onnx`, `tokenizer.json` (~1.85 GB) |
| `crates/memory-mcp/tests/models/ner/VAGOsolutions--SauerkrautLM-LFM2.5-GLiNER/` | HF `VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER`: `pytorch_model.bin`, `gliner_config.json`, `tokenizer.json` (~1.6 GB) |

The GLiNER tokenizer comes from the companion repo `MoritzLaurer/mDeBERTa-v3-base-mnli-xnli`
(see `docs/agent/EVALUATION.md`). After adding fixtures, re-run:

```bash
cargo run -p eval-harness --bin memory-eval -- run \
  --profile evals/profiles/ner_quality.json --artifact target/eval-ner.json \
  --suite ner-quality-anno --suite ner-quality-regex --suite ner-quality-anno-onnx \
  --suite ner-quality-gliner --suite ner-quality-vago
cargo bench -p eval-harness --bench ner_cpu -- --noplot
```

Then update this file with the two missing rows.
