# Qwen source-authority qualification v5 — sampler correction

**Status:** frozen before capture on 2026-11-23.  
**Plan ID:** `source-authority-local-chat-completions-2026-11-23-v5-sampler-qualification`  
**Purpose:** determine whether the v4 Qwen screen's failure is sensitive to using the model author's recommended thinking-mode sampling values. This is a model/workflow qualification screen, not a memory implementation comparison.

## Frozen design

- Four new synthetic task families, disjoint from all earlier Qwen, LFM,
  Codex and maintenance cohorts. Two test conversation memory; two test
  literal file import.
- One trial per family in each arm. Both arms use the same memory binary,
  product configuration, tool schema, agent alias, prompts, host policy,
  budgets and independent-probe setup. Only the registered preparation
  workflow differs: model-managed `ingest` plus `extract`, or operator import
  followed by read-only agent recall.
- Local model alias: `memory-local-eval-qwen38`, reported by LM Studio as
  `lmstudio-community/Qwen3.8-27B-MLX-4bit`, 4-bit, 16,384-token runtime
  context. The model-weight hash and LM Studio server build identity are
  unavailable; this cannot support a confirmatory model claim.
- The endpoint is `http://127.0.0.1:1234/v1`; the runner accepts only numeric
  loopback. The capture uses synthetic text and no API key, Codex runner,
  bearer token, remote endpoint, external prompt service or network fetch.
- The sampler is hash-pinned in the plan and sent on every turn:
  `temperature=1.0`, `top_p=0.95`, `top_k=20`, `min_p=0.0`,
  `presence_penalty=0.0`, `repeat_penalty=1.0`. These correspond to the
  official [Qwen3.8 model-card thinking-mode recommendations](https://huggingface.co/Qwen/Qwen3.8-27B),
  mapped to the supported fields in the [LM Studio Chat Completions API](https://lmstudio.ai/docs/developer/openai-compat/chat-completions).
  Thinking remains at the server/model default; no model-specific template
  override is sent.
- Retries are disabled. Every model response is scored by the frozen task
  oracle. The independent probe uses a separate read-only `assemble_context`
  session. Raw provider responses and retrieved source text are not persisted.

## Screening decision fixed before capture

Proceed only to a separately preregistered, broader workflow pilot if the
capture and artifacts validate, there are zero hard violations, independent
probes pass 4/4 in each arm, task-oracle success is at least 3/4 in each arm,
and candidate file-import task success is at least 1/2. Otherwise stop
model-backed source-authority evaluation for this Qwen workflow configuration.

The screen cannot qualify a powered comparison, establish non-inferiority or
acceptance, or support an agent-benefit claim. Latency is descriptive within
this capture only. Do not compare it causally to earlier LFM or Qwen runs,
which use distinct families or evaluator configurations. Never reuse these
four exposed families for confirmation.

## Interpretation limits

A pass only shows that this Qwen configuration is worth a larger independent
pilot. Both product arms are identical, so task and probe results are
workflow-feasibility evidence, not a memory-quality treatment effect. A fail
is a no-go for this model/configuration and is not evidence of a memory defect.

