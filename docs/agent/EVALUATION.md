# Evaluation

The `eval-harness` crate (`memory-eval` binary) provides profile-driven evaluation. It is never linked into the production binary.

## Profiles

```bash
# PR profile (deterministic regression, target 10 min)
make eval-pr

# Release profile (full retrieval + lifecycle, target 20 min)
make eval-release

# Nightly profile (full end-to-end + diagnostics)
make eval-nightly
```

## Corpus Preparation (one-time, requires network)

```bash
cargo run -p eval-harness --bin memory-eval -- prepare-corpus \
  --manifest evals/corpora/longmemeval.json \
  --output-root data/corpora
```

## Performance Benchmarks

Criterion benchmarks, separate from `cargo test`:

```bash
cargo bench -p eval-harness --bench pipeline -- --noplot
cargo bench -p eval-harness --bench ner_cpu -- --noplot
cargo bench -p eval-harness --bench contention -- --noplot
```

## NER Extractor Comparison

Every `NER_EXTRACTOR` backend (`anno`, `regex`, `anno-onnx`, `urchade/gliner_multi-v2.1`,
`VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER`) can be evaluated on the same RU/EN/mixed
quality corpus and latency bench, so you can pick the extractor that fits your scenario.

### Quality (per-extractor mention F1)

```bash
# All five extractors (needs the local model checkpoints under
# crates/memory-mcp/tests/models/ner/, see the README for where to get them):
cargo run -p eval-harness --bin memory-eval -- run \
  --profile evals/profiles/ner_quality.json \
  --artifact target/eval-ner.json

# Only the offline extractors (no checkpoints needed):
cargo run -p eval-harness --bin memory-eval -- run \
  --profile evals/profiles/ner_quality.json \
  --artifact target/eval-ner.json \
  --suite ner-quality-anno --suite ner-quality-regex

# Only the extractors whose checkpoints you have, e.g. GLiNER:
cargo run -p eval-harness --bin memory-eval -- run \
  --profile evals/profiles/ner_quality.json \
  --artifact target/eval-ner.json --suite ner-quality-gliner
```

The markdown report renders one suite summary per extractor:
`entity_mention_precision`, `entity_mention_recall`, `entity_mention_f1` (mention matching is
case-insensitive on canonical names, so type-vocabulary differences between backends
do not distort the comparison). The artifact carries per-case diagnostics —
`entity_mention_typed_f1` and the list of missing/unexpected mentions — and the report surfaces
them on invalid cases. Selecting a suite whose checkpoint is missing produces explicit
`invalid` cases — filter with `--suite` to what you have.

### Performance (latency + cold start)

```bash
cargo bench -p eval-harness --bench ner_cpu -- --noplot
```

Criterion reports `{regex,anno,anno_onnx,gliner,vago}_single_window_warm` and
`_multi_window_warm`; each bench prints the extractor's cold-start time (model load)
before measuring. Model-backed benches skip with a note when the checkpoint is absent.
`default_service_extract_warm` measures the production `ServiceContext::extract` path
(Anno extractor + DB round trip) and is not comparable with the raw-extractor benches.

> **Run one extractor at a time** — the five backends can be very different in
> latency, so compare across suites inside a dedicated run: `cargo run -p
> eval-harness -- run --profile evals/profiles/ner_quality.json ...` (the `--suite`
> flag is repeatable, so you can pass it multiple times).

## Checkpoints

The model-backed suites read **local, gitignored** checkpoints only — nothing is
downloaded and no upstream revision is resolved at eval time. Prepare them by placing
the folders under `crates/memory-mcp/tests/models/ner/`:

| Suite | Fixture dir | How to populate it (all offline after first download) |
|---|---|---|
| `ner-quality-anno-onnx` | `crates/memory-mcp/tests/models/ner/deepanwa--NuNerZero_onnx/` | Download HF `deepanwa/NuNerZero_onnx` (`model.onnx`, `tokenizer.json`) into this dir. |
| `ner-quality-gliner` | `crates/memory-mcp/tests/models/ner/urchade--gliner_multi-v2.1/` | Download `model.safetensors` + `gliner_config.json` from HF `urchade/gliner_multi-v2.1` and `tokenizer.json` from the companion repo `MoritzLaurer/mDeBERTa-v3-base-mnli-xnli` (the GLiNER repo ships no tokenizer) into this dir. |
| `ner-quality-vago` | `crates/memory-mcp/tests/models/ner/VAGOsolutions--SauerkrautLM-LFM2.5-GLiNER/` | Download HF `VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER` (`pytorch_model.bin`, `gliner_config.json`, `tokenizer.json`, ~1.6 GB) into this dir. |

### Interpreting the results

- **regex / anno**: near-instant, zero-download, deterministic. Best for offline-first,
  privacy-sensitive, or high-throughput ingestion where recall of noisy mentions is
  acceptable.
- **anno-onnx**: CPU NuNER via ONNX Runtime; fastest neural backend (~37 ms/extraction, ~4×
  faster than the Candle models) and never mislabels a kept mention. Limitation: the export is
  `max_width=1` (single-word spans only), so multi-word entities are fragmented — recall is
  poor against multi-word gold. Best for fast single-token extraction. Measured values in
  `evals/results/ner-comparison-2026-08-11.md`.
- **classic GLiNER**: best general-purpose quality/coverage across RU/EN; largest
  ecosystem default.
- **VAGO LFM2**: strongest RU/EN multilingual zero-shot coverage in a native Candle
  backend; largest checkpoint (~1.6 GB) and longest cold start.
