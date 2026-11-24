# Ordinary agent workflow screen after tool-budget control

**Status:** frozen before any model call, 2026-11-25.
**Plan ID:** `native-qwen-agent-tool-budget-v1`.
**Purpose:** determine whether a local Qwen Chat Completions configuration can
complete ordinary memory-backed recall when the provider-neutral runner stops
advertising tools after the registered tool-call budget is consumed. This is a
workflow qualification screen, not a comparison of memory treatments.

## Frozen design

- Four fresh synthetic families, one trial per family, two separate captures.
- One registered memory read per trial; three-turn, 384-output-token,
  4,096-context-token and 25-second trial ceilings.
- Both arms use the same memory product SHA, build revision, configuration,
  evidence rollout stage, one-call tool budget, agent, prompt, tasks and model.
  No arm contrast can estimate a memory treatment effect.
- Each capture requires at least 3/4 oracle-correct task outcomes, 3/4 actual
  agent memory reads, all 4/4 independent fixed-query probes, zero hard
  violations, and zero budget or timeout exits. Both captures must pass before
  this exact workflow is considered eligible for a separately frozen design.
- The task, probe, plan and criteria bytes are pinned in
  `freeze-manifest.sha256`. The evaluator and product binary hashes are pinned
  in the plan.

## Local provider and data boundary

- Run only on native Apple Silicon with direct `mlx_lm.server` 0.32.2 at
  `http://127.0.0.1:18767/v1`, serving the local Qwen3.8-27B MLX 4-bit model.
- Start with `HF_HUB_OFFLINE=1`, temperature zero, one server worker and
  `enable_thinking=false`. Use no LM Studio process, app, API, or `lms` CLI; no
  Codex runner, remote model, API key, bearer token, or network retrieval.
- All episode text and queries are synthetic. Model requests stay on loopback.
- The local model path and serving configuration are recorded, but the model
  weight digest is unavailable. Therefore a passing result qualifies only this
  locally reported alias/runtime during the same-alias capture window; it is
  not an immutable model-weight reproduction.

## Decision limits

A pass only demonstrates that this exact local workflow is eligible for
separate evaluation. It does not prove memory quality improved, establish an
agent treatment effect, authorize the other C1 strata, or justify promotion.
A failure remains in the denominator and stops this qualification; exposed
families must not be tuned or replayed for another screen.
