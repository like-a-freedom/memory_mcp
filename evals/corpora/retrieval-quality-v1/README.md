# Native retrieval quality subset

This is a source-grounded functional subset, not an official benchmark score. `retrieval-quality.json` is consumed by the actual production ingest/extract/assemble path. Sources are duplicated per case to provide isolated complete candidate populations. Retrieval does not select which source episodes are ingested.

LoCoMo: snap-research/locomo revision `3eb6f2c585f5e1699204e3c3bdf7adc5c28cb376`, raw SHA-256 `79fa87e90f04081343b8c8debecb80a9a6842b76a7aa537dc9fdf651ea698ff4`, 2,805,274 bytes, MIT. Selection uses every turn from the first two sessions of all ten conversations, then the first eligible single-hop QA and first eligible multi-hop/temporal QA whose upstream evidence fits those sessions. This yields 19 QAs. Speaker text and session reference timestamps come from the pinned raw dataset. Image metadata is appended uniformly to all image-bearing turns (133 case-scoped rows, 71 distinct turns), labeled as dataset annotation rather than a user assertion. No remote image was inspected; annotations only identify the dataset's intended image referents. No upstream summaries or answers are ingested as memory.

Target source excerpts come from the Memory MCP repository at revision `355dc92206e341d02a61a6066cd7ec5e49f21ce0`, `docs/operations/LOGGING.md` and the Task 16 section of `docs/superpowers/plans/2026-10-07-http-bounded-memory.md`. Numeric, URL, task and cross-project distractors are sanitized representative cases. The two calendar regression cases use explicitly authored release-note excerpts: the answerable Atlas/Q2 case and a paired Atlas/Q1 abstention case with a Trackly/Q1 distractor. They exercise production ingestion, retrieval and admission but are controlled development regressions, not observations from a public benchmark. Unknown, ACL and historical scenarios have explicit source populations. No private mail, credentials or production memory database are bundled.

`label-review.json` records the initial selection. `independent-source-review.md`/`.json` and `phase-1-image-metadata-extension.md`/`.json` record source-only adjudication before the original native predictions. `independent-source-review-extension.json` records the two new calendar cases and the budget-one boundary change before their predictions, including exact native-extracted facts and input hashes. An upstream minimal answer citation does not exhaust useful evidence. Corrections name exact bindings and semantic support groups; the full population and six 1.0 floors are retained. All 37 cases are development regressions, not held-out accuracy estimates.

`retrieval-quality.r*.json` and `sources.r1.json` are historical contracts, not alternative profiles to select for an easier score. Active source-bundle SHA-256 is checked by the harness. Fact/episode bindings match exact persisted native readback, not injected gold facts.

Models are pinned in `evals/models/`. Weights and original large corpora are generated inputs kept outside the tracked corpus; the commands below use the ignored `target/eval-models/` cache. `prepare-model` streams immutable Hugging Face revisions into owned staging directories, verifies every byte count/SHA-256, and publishes only complete bundles. Cache hits are also verified; corrupt bundles fail closed. Optional mirrors require HTTPS or loopback HTTP without credentials. `prepare-retrieval-quality` refuses unavailable native providers. Operator configuration requires local E5 and optional explicit `RETRIEVAL_RERANKER_MODEL_DIR`; native acceptance pins the mMARCO cross-encoder at admission floor `0` and the exact recorded reranker signature.

Run the actual CLI from the workspace root with model directories set to absolute verified cache paths:

```sh
EMBEDDINGS_ENABLED=true EMBEDDINGS_PROVIDER=local-candle EMBEDDINGS_MODEL=intfloat/multilingual-e5-small \
EMBEDDINGS_MODEL_DIR=/absolute/e5-cache RETRIEVAL_RERANKER_MODEL_DIR=/absolute/reranker-cache \
RETRIEVAL_RERANKER_ADMISSION_FLOOR=0 \
MEMORY_CLAIM_ROLLOUT_STAGE=evidence cargo run -p eval-harness --bin memory-eval --locked -- \
run --profile evals/profiles/retrieval_quality.json --artifact /absolute/result.json
```

Prepare/read back sources independently of queries and labels:

```sh
cargo run -p eval-harness --bin memory-eval --locked -- prepare-retrieval-quality \
  --sources evals/corpora/retrieval-quality-v1/sources.json --output /absolute/inventory.json
```

Use the same explicit native provider configuration. Claim configuration is parsed through the production `ClaimConfig::from_env()` contract, including the default `evidence` stage; invalid claim configuration fails before provider initialization. Preparation metadata does not constitute a quality score. PR and release profiles require this suite and fail closed when models are unavailable or identities mismatch.

The active r28 preparation was rerun from the unchanged `sources.json` after identity-admission v2 changed the extractor fingerprint. `native-preparation-r28.json` retains the actual source-only native inventory; `native-preparation-refresh-r28.json` records its byte hash and an exact per-case multiset comparison with all 927 existing native fact bindings. All 37 case definitions, 980 episode bindings, queries, labels and six 1.0 acceptance floors are unchanged. Only the active revision and the fingerprint obtained from this preparation changed. This establishes preparation compatibility, not a new accuracy estimate.

Prepare the pinned production models reproducibly:

```sh
cargo run -p eval-harness --bin memory-eval --locked -- prepare-model --manifest evals/models/multilingual-e5-small.json --output-root target/eval-models
cargo run -p eval-harness --bin memory-eval --locked -- prepare-model --manifest evals/models/mmarco-minilm.json --output-root target/eval-models
```

The deployed default remains without a reranker; activation requires an explicit local model directory. The native acceptance configuration uses the pinned mMARCO cross-encoder with admission floor `0`; the separately fitted mMARCO floor `-6.293555736541748` was rejected after it materially harmed precision and prohibited-free retrieval. Qwen3-Reranker-0.6B was also evaluated as a development experiment, but its fitted floor is not the acceptance profile. Both calibration reports use 45 source-reviewed LongMemEval development pairs (9 positives, 36 negatives), many easy and correlated; they are not held-out accuracy or probability calibration. Raw score artifacts, source lineage, exclusions and input token visibility are retained. See `../reranker-calibration-v1/README.md`.

Interpret evidence limits literally: the film date is approximate, the January planning chronology is underspecified, and a planned Japanese mansion stay does not prove real-estate purchase. Task 16 excerpts do not establish current worktree/review status. These cases test retrieval of useful support and qualifying context, not correctness of a generated final answer. No official full LoCoMo/LongMemEval score is claimed.

The original dated bowling case (`locomo-conv-47-001`) keeps its full two-session source population and original question with output budget `5`. The `logging-sink` case is the one-item response-budget boundary; it tests real candidate retrieval under a one-item response limit.

## r28 native validation and regression repairs

The native refresh exposed two separate retrieval defects without changing any case, source, label or acceptance floor:

- [`native-quality-r28-before-scope.json`](native-quality-r28-before-scope.json): 36/37 native cases passed. `logging-original` returned no evidence because its terminal `in memory_mcp` scope became a mandatory content identifier; all three required groups were missed.
- [`native-quality-r28-scope-only.json`](native-quality-r28-scope-only.json): 153/154 release cases passed, including 36/37 native cases. Scope admission recovered the answerable case, but final reranking lost two required groups. A previously verified secondary-facet proof was omitted when lexical evidence had already retained the candidate.
- [`native-quality-r28-release-final.json`](native-quality-r28-release-final.json): all 154 release cases passed, including all 37 native cases and all six unchanged 1.0 native floors. This run used the pinned E5/mMARCO models, admission floor `0`, the production default `evidence` stage, and the existing release baseline. Its 272.621-second duration is descriptive under concurrent host load.

[`native-preparation-refresh-r28.json`](native-preparation-refresh-r28.json) records report digests, runtime/model identities, frozen executable and source hashes, and the independent RED/GREEN checks. Eight public lexical scope controls cover exact targets, other-project ownership and mixed sources; two additional negative-reranker controls distinguish a complete independent facet from weak words spread across incomplete facets. The final public semantic suite passed 50 tests with one intentionally ignored test; 106 admission unit tests and five existing reranking-related unit tests also passed. Raw compilation logs are not duplicated in the corpus.

The repairs have a finite contract: terminal lowercase namespace scope is distinct from an explicit value/artifact target, and existing facet proof survives both reranking stages. They do not change source/time/access eligibility, model scores, thresholds or response limits. These are closed development regressions, not a held-out accuracy estimate, a full official benchmark score or evidence of improved agent-reader answers.
