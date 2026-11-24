# Qwen source-authority qualification v6 — runner repair

**Status:** frozen before capture on 2026-11-23.
**Plan ID:** `source-authority-local-chat-completions-2026-11-23-v6-runner-fix-qualification`
**Purpose:** test whether preserving optional `reasoning_content` across Chat Completions tool follow-ups makes this Qwen configuration capable of executing the registered workflow. This does not evaluate a memory implementation change.

## Frozen design

- Four new synthetic families, disjoint from prior Qwen, LFM, Codex, maintenance and source-authority cohorts. Two use conversation formation; two use literal fact ingestion for incumbent and operator file import for candidate.
- For source-file families both arms receive the exact same factual bytes through supported paths: incumbent gets literal text in `formation_prompt`; candidate bytes are staged and imported through the operator CLI. `ingest.content` is never treated as a path.
- Both arms use the same memory binary, product configuration, public tool schema, Qwen alias, prompts, sampler, host policy, budgets and independent probe. Only the preparation workflow differs.
- Model alias `memory-local-eval-qwen38`, reported as `lmstudio-community/Qwen3.8-27B-MLX-4bit`, 4-bit, 16,384-token runtime context. Weight hash and LM Studio server build are unavailable.
- Numeric loopback `http://127.0.0.1:1234/v1`; no API key, bearer token, Codex runner, remote endpoint or network fetch.
- Sampler sent on every turn: temperature 1.0, top_p 0.95, top_k 20, min_p 0.0, presence_penalty 0.0, repeat_penalty 1.0. Retries disabled. Raw provider responses and retrieved source text are not persisted.

## Decision gate

Proceed only to a separately registered broader workflow pilot if capture validates, hard violations are zero, independent probes pass 4/4 per arm, task-oracle success is at least 3/4 per arm, and candidate source-file task success is at least 1/2. Otherwise stop qualification for this exact model/runner/sampler configuration.

A pass only supports a broader independent pilot. It cannot establish memory treatment effect, non-inferiority, powered acceptance or broad agent benefit. Do not reuse these exposed families.
