# Flint source-authority qualification v1

**Status:** frozen before model load/capture on 2026-11-23.
**Plan ID:** `source-authority-local-flint-2026-11-23-v1-qualification`
**Purpose:** qualify one local tool-use model/runner configuration for a larger workflow pilot. It does not compare memory implementations.

## Frozen design

- Four new synthetic families disjoint from all prior Qwen, LFM, Codex, maintenance and source-authority cohorts: two conversation facts and two source-file import facts.
- For source-file families, the incumbent receives exact literal fact text through `formation_prompt`; the candidate receives the identical bytes through an evaluator-owned file import and read-only recall. The task manifest is v3 and binds the literal-input protocol.
- Both arms use the same current memory binary/configuration/tool schema, same agent alias, prompts, sampler, independent-probe setup and budgets. Only the preparation workflow differs.
- Model alias: `memory-local-eval-flint`, backed by the installed `recursivesi/flint` Q4_K_M checkpoint. The official model card reports 39.8B total parameters and about 2.5B active per token, multi-turn tool use, Apache-2.0 licensing, and sampling values temperature 1.0, top_p 0.95, top_k 20. The serving guidance says to preserve `reasoning_content`; the runner's RED/GREEN test covers that forwarding while keeping receipts content-free. Model weight hash and LM Studio server build are unavailable.
- Numeric-loopback endpoint `http://127.0.0.1:1234/v1`; synthetic facts only, no API key or bearer token, no Codex, no remote endpoint and no network fetch.
- Sampler is frozen and sent on each turn: temperature 1.0, top_p 0.95, top_k 20. The model output budget is 4,096 tokens per trial and the wall-time budget is 120 seconds per phase; retries are disabled.

## Frozen decision gate

Proceed only to a separately registered broader workflow pilot if capture artifacts validate, hard violations are zero, independent fixed-query probes pass 4/4 in each arm, task-oracle success is at least 3/4 in each arm, and candidate source-file task success is at least 1/2. Otherwise stop qualification for this exact model/runner/sampler configuration.

Even a pass supports only a broader independent pilot. It cannot establish memory treatment effect, non-inferiority, powered acceptance or broad agent benefit. Do not reuse these exposed families.

## Capture command

After loading the model with `lms load recursivesi/flint --context-length 16384 --parallel 1 --identifier memory-local-eval-flint` and verifying `lms ps` shows the frozen alias and context, run from the repository root:

```sh
env -u MEMORY_EVAL_BEARER_TOKEN target/debug/memory-eval run-paired-chat-completions \
  --plan evals/paired/source-authority-local-flint-v1-qualification/plan.json \
  --tasks evals/paired/source-authority-local-flint-v1-qualification/tasks-v1.json \
  --model-endpoint http://127.0.0.1:1234/v1 \
  --sampling-config evals/paired/source-authority-local-flint-v1-qualification/sampling-config.json \
  --incumbent-memory-server /Users/solovey/Documents/dev/memory_mcp/target/debug/memory_mcp \
  --candidate-memory-server /Users/solovey/Documents/dev/memory_mcp/target/debug/memory_mcp \
  --output-directory /Users/solovey/Documents/dev/memory_mcp/evals/paired/source-authority-local-flint-v1-qualification/runs/run-01
```
