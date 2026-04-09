# External eval raw fixtures

This directory holds **full upstream benchmark datasets** downloaded by
`scripts/convert_external_evals.py`.

## Single source of truth

All eval tests read from these raw fixtures. There are no separate "sample" or
"trimmed" copies — the full datasets are the only source. Sampling is controlled
at runtime via the `MEMORY_MCP_EVAL_SAMPLE_PCT` environment variable:

```bash
# Run with full datasets (default)
cargo test --test eval_external_retrieval run_longmemeval_retrieval -- --ignored

# Run with 10 % of cases for faster iteration
MEMORY_MCP_EVAL_SAMPLE_PCT=10 cargo test --test eval_external_retrieval run_longmemeval_retrieval -- --ignored

# Run with just 1 case
MEMORY_MCP_EVAL_SAMPLE_PCT=1 cargo test --test eval_external_retrieval run_longmemeval_retrieval -- --ignored
```

## Dataset inventory

| Dataset | File | Source | Records |
|---|---|---|---|
| longmemeval | `longmemeval_s_cleaned.json` (265 MB) | xiaowu0162/longmemeval-cleaned (HF) | 500 records |
| locomo | `locomo10.json` (3 MB) | snap-research/locomo (GitHub) | 10 convs / 1986 QAs |
| personamem | `questions_32k.csv` + `shared_contexts_32k.jsonl` | bowen-upenn/PersonaMem (HF) | 589 questions / 37 contexts |
| prefeval | `travel_hotel_overall300_topk_history_persona.json` (340 KB) | amazon-science/PrefEval (GitHub) | 52 records |

## File inventory

- `longmemeval/longmemeval_s_cleaned.json`
  - source: `xiaowu0162/longmemeval-cleaned`
  - URL: <https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned/resolve/main/longmemeval_s_cleaned.json>
  - 500 evaluation instances; each ~30–40 sessions / ~115k tokens
- `locomo/locomo10.json`
  - source: `snap-research/locomo`
  - URL: <https://raw.githubusercontent.com/snap-research/locomo/main/data/locomo10.json>
  - 10 conversations with QA annotations (1986 QA items total)
- `personamem/questions_32k.csv`
  - source: `bowen-upenn/PersonaMem`
  - URL: <https://huggingface.co/datasets/bowen-upenn/PersonaMem/resolve/main/questions_32k.csv>
  - Paired with `shared_contexts_32k.jsonl` (bundled at load time)
- `prefeval/travel_hotel_overall300_topk_history_persona.json`
  - source: `amazon-science/PrefEval`
  - URL: <https://raw.githubusercontent.com/amazon-science/PrefEval/main/benchmark_dataset/rag_retrieval/simcse_implicit_persona/travel_hotel_overall300_topk_history_persona.json>
  - PrefEval retrieval track for travel/hotel queries

## Full dataset cache

Multi-source datasets (PersonaMem, PrefEval) are bundled at load time and cached
under `tests/fixtures/evals/full/` (git-ignored) for faster subsequent runs.

## Reproducible verification

Provenance tests verify fixture metadata:

```bash
cargo test --test eval_external_provenance declares_full_dataset_metadata -- --nocapture
```

Raw fixture existence test:

```bash
cargo test --test eval_external_provenance raw_fixture_files_exist -- --nocapture
```

## Regenerating fixtures

```bash
python scripts/convert_external_evals.py
```
