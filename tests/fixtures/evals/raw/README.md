# External eval raw fixtures

This directory holds **upstream benchmark dataset artifacts** downloaded by
`scripts/convert_external_evals.py`.

## Fixture tiers

Two tiers of fixtures coexist:

| Tier | Purpose | File pattern |
|---|---|---|
| **Full raw** | Complete upstream datasets for real eval runs | `<dataset>/<primary_file>` |
| **Sample** | Small 1-record excerpts for smoke tests & normalization unit tests | `<dataset>/sample_*.json` |

### Full raw fixtures

Downloaded from upstream sources and stored verbatim. These are **not** committed
to git (too large) — regenerate via:

```bash
python scripts/convert_external_evals.py
```

### Sample fixtures

Small, deterministic, git-tracked excerpts used for:

- local normalization unit tests,
- ignored smoke retrieval runs,
- fast regression checks without downloading full corpora.

## Current dataset statistics

| Dataset | Full raw file | Source | Samples |
|---|---|---|---|
| longmemeval | `longmemeval_s_cleaned.json` (265 MB) | xiaowu0162/longmemeval-cleaned (HF) | 500 records |
| locomo | `locomo10.json` (3 MB) | snap-research/locomo (GitHub) | 10 convs / 1986 QAs |
| personamem | `questions_32k.csv` (bundled JSON, 2 MB) | bowen-upenn/PersonaMem (HF) | 589 questions / 37 contexts |
| prefeval | `travel_hotel_overall300_topk_history_persona.json` (340 KB) | amazon-science/PrefEval (GitHub) | 52 records |

## File inventory

### Full raw fixtures (git-ignored, generated)

- `longmemeval/longmemeval_s_cleaned.json`
  - source: `xiaowu0162/longmemeval-cleaned`
  - URL: <https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned/resolve/main/longmemeval_s_cleaned.json>
  - 500 evaluation instances; each ~30–40 sessions / ~115k tokens
- `locomo/locomo10.json`
  - source: `snap-research/locomo`
  - URL: <https://raw.githubusercontent.com/snap-research/locomo/main/data/locomo10.json>
  - 10 conversations with QA annotations
- `personamem/questions_32k.csv`
  - source: `bowen-upenn/PersonaMem`
  - URL: <https://huggingface.co/datasets/bowen-upenn/PersonaMem/resolve/main/questions_32k.csv>
  - Bundled JSON: `{questions: [...], shared_contexts: {<id>: [...]}}`
- `prefeval/travel_hotel_overall300_topk_history_persona.json`
  - source: `amazon-science/PrefEval`
  - URL: <https://raw.githubusercontent.com/amazon-science/PrefEval/main/benchmark_dataset/rag_retrieval/simcse_implicit_persona/travel_hotel_overall300_topk_history_persona.json>
  - PrefEval retrieval track for travel/hotel queries

### Sample fixtures (git-tracked)

- `longmemeval/sample_longmemeval_s_cleaned.json` — locator: `question_id=e47becba`
- `locomo/sample_locomo10.json` — locator: `sample_id=conv-26`
- `personamem/sample_personamem_32k.json` — locator: `question_id=acd74206-...`
- `prefeval/sample_travel_hotel_implicit_persona.json` — Las Vegas hotel preference record

## Reproducible verification

Provenance test verifies each sample fixture is a real upstream excerpt:

```bash
cargo test --test eval_external_provenance verify_external_fixtures_against_official_sources -- --ignored --nocapture --test-threads=1
```

## Full dataset cache

Complete upstream artifacts are also cached under `tests/fixtures/evals/full/`
(git-ignored). The Rust test harness (`tests/eval_support/external_full.rs`) loads
from this cache when running `ExternalDatasetFlavor::Full` eval suites.
