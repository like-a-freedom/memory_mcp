# Ordinary agent workflow screen after tool withdrawal — v3

**Status:** frozen before any model request, 2026-11-25.
**Plan ID:** `native-qwen-agent-tool-budget-v3-withdraw-tools`.
**Question:** does removing tool definitions entirely from the follow-up request
after the registered one-call budget is consumed let this local agent complete
ordinary recall, while retaining the same synthetic task and fixed-probe
acceptance gates?

## Frozen design and stop rule

- Four new synthetic families, disjoint from v1/v2 and all prior cohorts; two
  captures are preregistered.
- One tool call, three turns, 384 generated tokens, 4,096 context tokens and
  25 seconds maximum per trial.
- Both arms use the same binary, configuration, claim stage, prompt, model and
  task; this is not a memory-treatment comparison.
- For each capture, require at least 3/4 candidate task successes, 3/4 actual
  agent memory reads, 4/4 independent probes, no hard violations and no
  budget/timeout exits. Stop the workflow qualification if capture 01 fails
  any gate; only run capture 02 if capture 01 passes every gate. A complete
  qualification requires both captures to pass.
- Plan, task, probe and gate inputs are pinned in `freeze-manifest.sha256`.
  Corpus, oracle, reader-protocol, endpoint, product, model alias, and active
  evaluator binary identities are registered before capture.
- The only product-side change is the runner's tool-budget enforcement: after
  the tool-call budget is consumed, a follow-up request carries neither
  `tools`, `tool_choice`, nor `parallel_tool_calls`. The prior frozen v2
  capture demonstrated that `tool_choice: none` alone did not prevent this
  local provider from returning another tool call.

## Local execution boundary

- Native Apple Silicon, direct `mlx_lm.server` 0.32.2, loopback
  `http://127.0.0.1:18767/v1`, local Qwen3.8-27B MLX 4-bit weights.
- `HF_HUB_OFFLINE=1`, temperature 0, `enable_thinking=false`, single worker.
  No LM Studio app/process/server/CLI, no Codex runner, no API key or bearer
  token, and no remote model or network retrieval.
- All task and memory content is synthetic. Model weight digest is unavailable;
  the conclusion can qualify only this alias/runtime, not immutable weights.

## Interpretation

A pass qualifies this exact agent workflow for a separately registered study.
It does not demonstrate improved memory quality, a memory treatment effect,
general agent usability, or readiness of the other C1 strata. A failed gate is
a no-go for this exact runner/model/prompt/budget configuration; do not replay
or tune the exposed families.
