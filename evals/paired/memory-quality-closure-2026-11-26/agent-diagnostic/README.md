# Bounded local three-way memory diagnostic

Two separately executed captures each passed all nine scheduled case/arm combinations: **18/18 exact answers, 18/18 exact source citations, 18/18 expected abstention decisions**, with no invalid or omitted measured arms. Observed combined capture wall time was **117.060 seconds**. These are finite synthetic development scenarios, not an estimated accuracy rate or treatment benefit.

| Family | Source-grounded answer | Registered query | Model-selected query | Sufficient fixed reply |
| --- | --- | --- | --- | --- |
| ordinary | Larch retry cap `47` | 2/2 | 2/2 | 2/2 |
| correction/selector | Sorrel key `Cabinet-62` after old `Cabinet-18` is explicitly invalidated | 2/2 | 2/2 | 2/2 |
| false premise | Quartz vault is pending, with no approved vault: answer null, abstain true, cite pending source | 2/2 | 2/2 | 2/2 |

All 12 actual public reads (registered and selected queries across both captures) exposed the expected source episode. The Larch/Sorrel reads also contained the expected value literally. False-premise success is graded from the explicit negative source and exact null/abstention/citation contract, not from absence of a value. Fixed replies were constructed from frozen source events and actual source episode IDs; they did not consume the extractor's predicted answer as an oracle.

**C2 eligibility: NO-GO.** This demonstrates feasibility of reading sufficient public evidence under a short exact JSON contract. The harness supplied source formation, the correction invalidation and the registered queries. The selected arm lets a model choose one search query, not an autonomous memory-formation/correction workflow. It therefore does not independently qualify the autonomous memory treatment. The earlier C1 stop rule remains binding; no C1 run was repeated or discarded. No larger confirmation or statistical benefit claim follows from this diagnostic.

## What ran

The current native `memory_mcp` binary exposed public stdio MCP. Each capture launched a fresh isolated SurrealKV store, ingested the frozen synthetic documents, extracted actual durable facts, explicitly invalidated the old Sorrel fact, and read through `assemble_context` with budget 5 and verbose provenance. Local cached multilingual-e5-small embeddings and the local cached reranker were enabled. MCP arguments, responses, errors and durations are in `capture-01/mcp.jsonl` and `capture-02/mcp.jsonl`.

The reader/selector was Qwen3.8-27B-MLX-4bit served directly by offline `mlx_lm.server` on numeric loopback. A cache directory happens to live under an LM Studio path; the LM Studio application/runtime was not used. Temperature was zero, maximum output 192 tokens, thinking disabled at the server, decode/prompt concurrency one. The runner made 18 reader calls (3.004–8.060 seconds each, 89.238 seconds summed) and 6 selector calls (1.108–1.489 seconds each, 7.474 seconds summed). The shared 300-second budget controls eligibility to start model calls; each HTTP request uses the smaller of 20 seconds and the remaining budget as its socket I/O timeout. This is not an absolute whole-response wall deadline. MCP operations have separate 30-second response waits and may run after model eligibility expires, so neither captured nor reusable runner establishes a 300-second whole-capture wall bound. No measured model call failed or timed out. The model server has been stopped and the disposable capture databases removed; raw JSON receipts remain.

Selected queries were identical across repeats:

- ordinary: `Larch transfer approved retry cap`
- correction: `Sorrel calibration key cabinet location after correction`
- false premise: `Quartz release key approved vault`

Arms ran in fixed registered/selected/fixed order, and the model server/prompt cache was shared. Fresh MCP processes separate the memory captures; this is not a counterbalanced model study or a claim of statistical independence between deterministic generations.

## Frozen identities and preflights

`frozen-plan.json` freezes all three families, source events, exact independent expectations, prompt text, budgets, arm order, capture-repeat eligibility and the captured script/product/source/model-config hashes. `capture-script.py` is the exact historical, host-specific capture source; its bytes match `frozen-plan.json.script_sha256`. It is an archived receipt, not the reusable runner. `result.json` retains both complete 9-arm captures and the C2 decision. Per-arm reader/selector files retain complete requests, responses, provider usage and latencies.

Preflight failures are retained separately and were not measured trials or successful retries. Initial sandbox storage initialization was denied. The next native preflight exposed that an empty ingestion-inbox setting is invalid and that a `mem://` URL alone does not select an ephemeral CLI store. Before any measured ingest/read/model call, the launch was changed to an explicit capture-local SurrealKV directory and the inbox setting removed. Subsequent public-schema preflights established that ingest returns an episode string, and that assemble_context accepts `budget` rather than `token_budget`. Preflight plan versions and transcripts remain intact. No measured outcome was used to revise labels, prompts or selected queries.

The product binary and production Rust source tree are hash-pinned before capture; no repair of the production memory implementation was made for this diagnostic. The pre-capture model identity includes model path and config SHA-256, not complete weight hashes. `model-assets-post-capture.json` checksums the offline model assets after capture and runtime shutdown, explicitly labeled as post-capture evidence. It cannot retroactively establish an immutable pre-capture weight freeze.

## Reusable runner and checks

`evals/paired/memory_quality_diagnostic.py` is the parameterized, provider-neutral standard-library runner. It accepts an opaque API model identifier, explicit numeric-loopback endpoint and caller-supplied local memory environment. A local model-config hash is optional. It does not depend on a particular model cache path or model-serving application. `local-config.json` is this run's explicit machine-specific reproduction configuration, not a runner default.

```sh
python3 evals/paired/memory_quality_diagnostic.py freeze \
  --config evals/paired/memory-quality-closure-2026-11-26/agent-diagnostic/local-config.json \
  --binary target/debug/memory_mcp --output /tmp/new-memory-diagnostic
python3 evals/paired/memory_quality_diagnostic.py capture --output /tmp/new-memory-diagnostic
python3 -m unittest discover -s evals/paired -p test_memory_quality_diagnostic.py
```

The local loopback model runtime must be started separately. The reusable runner uses a bounded reader thread/queue instead of Unix-only select on subprocess pipes. Failed startup/formation preserves the full scheduled arm denominator as invalid evidence, and startup errors close owned processes and files. It makes a second capture only if every first-capture arm passes; failed captures are never replaced by a selected success.

Public CLI TDD exposed and then rejected a numeric-prefix answer, a correct answer citing the wrong source, missing `answer:null` on an abstention, startup failure losing all scheduled cases, and a repeated capture overwriting an earlier result. Each was observed RED before its corresponding change. The final suite has eight passing CLI tests, including exact-answer and supported-abstention positive controls plus opaque model-ID freezing. Existing capture directories and results are refused before execution; completed result creation is exclusive. Runner robustness/configuration changes occurred after the successful capture; the archived source identifies the narrower harness actually used for these results. No model capture was rerun to validate those later engineering changes.
