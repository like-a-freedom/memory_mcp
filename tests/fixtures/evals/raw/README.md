# External eval raw fixtures

These files are **not vendored full benchmark datasets**.
They are intentionally small, deterministic **source-derived trimmed excerpts** used for:

- local normalization tests,
- ignored smoke retrieval runs,
- fast regression checks without downloading multi-megabyte benchmark corpora into the repository.

## Why the files look small

That is intentional.

The official upstream datasets are much larger than the local fixture copies:

- **LongMemEval-cleaned** — the official `longmemeval_s_cleaned.json` release contains 500 evaluation instances; each `LongMemEval_S` instance contains roughly 30–40 sessions / ~115k tokens.
- **LoCoMo** — the official ACL benchmark release in `data/locomo10.json` contains 10 conversations; each conversation can span many sessions and QA annotations.
- **PersonaMem** — the official benchmark is split across large `questions_32k.csv` / `shared_contexts_32k.jsonl` artifacts (and larger 128k / 1M variants).
- **PrefEval** — the official benchmark contains thousands of preference-query pairs across many topics and tracks; our local fixture mirrors one retrieval-track JSON record.

## What is stored locally

Each local fixture keeps only the minimum official excerpt needed for stable retrieval evaluation:

- `longmemeval/sample_longmemeval_s_cleaned.json`
  - source: `xiaowu0162/longmemeval-cleaned`
  - locator: `question_id=e47becba`
- `locomo/sample_locomo10.json`
  - source: `snap-research/locomo`
  - locator: `sample_id=conv-26`
- `personamem/sample_personamem_32k.json`
  - source: `bowen-upenn/PersonaMem`
  - locator: `question_id=acd74206-37dc-4756-94a8-b99a395d9a21`
  - paired context: `shared_context_id=e898d03fec683b1cabf29f57287ff66f8a31842543ecef44b56766844c1c1301`
- `prefeval/sample_travel_hotel_implicit_persona.json`
  - source: `amazon-science/PrefEval`
  - locator: `travel_hotel_overall300_topk_history_persona.json` + Las Vegas hotel preference record

## Reproducible verification

The repo now includes an ignored provenance test that checks each local fixture against the official upstream source:

- `cargo test --test eval_external_provenance verify_external_fixtures_against_official_sources -- --ignored --nocapture --test-threads=1`

This test fetches the official source artifacts and verifies that the local fixture content is a real excerpt rather than a synthetic placeholder.
