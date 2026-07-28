# External eval raw fixtures

This directory holds **full upstream benchmark datasets** used by the
`eval-harness` corpus preparation pipeline.

## How preparation works

Corpus data is never downloaded during evaluation. A separate preparation
command fetches, validates, and stages the data:

```bash
# Prepare a specific corpus
cargo run -p eval-harness --bin memory-eval -- prepare-corpus \
  --manifest evals/corpora/longmemeval.json \
  --output-root data/corpora
```

Preparation pins an immutable revision, verifies SHA-256, and writes data
to a prepared location outside the measured run.

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

## Corpus manifests

Immutable corpus manifests live in `evals/corpora/` and declare the source URL,
revision, SHA-256 digest, license, byte size, case count, and adapter version
for each dataset. See:

- `evals/corpora/longmemeval.json`
- `evals/corpora/locomo.json`
- `evals/corpora/personamem.json`
- `evals/corpora/prefeval.json`

## License obligations

Each manifest records the dataset license. Preparation respects these licenses.
Review the manifest before redistributing prepared corpus data.
