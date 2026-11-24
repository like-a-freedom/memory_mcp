# Preregistration: local source-authority workflow pilot v2

**Plan ID:** `source-authority-local-chat-completions-2026-11-23-v2`  
**Stage:** development pilot  
**Independent families:** 16  
**Captures:** two repeats on the same frozen families  
**Purpose:** workflow feasibility and descriptive cost only

## Question and estimand boundary

On one pinned product binary, compare (A) an agent that reads one synthetic
relative-path source through public `ingest` and `extract` tools with (B) an
operator import and extraction followed by a read-only agent recall session.
The eight ordinary conversation controls exercise the same product in both
arms. The independent post-task probe verifies the product state without
reusing the agent's memory read.

Both arms use the exact same product executable, product configuration, tool
schema, storage profile, extraction configuration, model alias and endpoint.
This is not a candidate-vs-incumbent memory treatment, and no candidate
benefit, causal product effect, non-inferiority or population reliability can
be inferred. The agent-mediated outcomes are descriptive feasibility evidence.

## Registered cohort and procedure

- The task manifest contains eight conversation-control families and eight
  source-file-import families, each with one fixed recall question, an answer
  oracle and an independent public-MCP probe.
- All names, facts and answers are synthetic and disjoint from prior paired
  task families. No project, user or production memory is included.
- Each arm/family starts with a fresh isolated store. The incumbent source file
  is staged under the evaluator-owned source root. The candidate operator
  imports the identical bytes before the fresh read-only agent session.
- Arm order is counterbalanced within each stratum as fixed in `plan.json`.
- Run the frozen cohort twice in different output directories, with one
  attempt per arm/family and zero runner retries. Do not tune or edit after
  seeing the first capture. Preserve partial and failed captures.
- Both arms use product binary SHA-256
  `3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`,
  feature `fs-watch`, claim stage `evidence`, embedded SurrealDB, regex
  extraction, and embeddings disabled.
- The runner uses `openai-compatible-chat-completions`, model alias
  `memory-local-eval-qwen38`, endpoint `http://127.0.0.1:1234/v1`, and the
  normalized endpoint fingerprint in `plan.json`. The provider inventory
  records LM Studio CLI commit `dcefd5a`, family `qwen3.8-27b-mlx`, and its
  reported 16,384-token context. Prompt, policy, model alias and product
  binaries are pinned; the local model weights and server build have no
  available immutable identity, so identity across separate captures is not
  established. Input context is capped at 15,360 tokens with at most 1,024
  output tokens; tool calls and turns are capped at eight; wall time is capped
  at 120 seconds per phase.
- The adapter accepts numeric loopback only and disables ambient proxies and
  redirects. This restricts the adapter's destination but cannot prove that a
  local provider process does not forward traffic. Registered data is
  synthetic-only.

## Registration correction before model exposure

The initial CLI preflight rejected stale v1 corpus and oracle digests in the
v2 plan. The exact task manifest generated the current digests through
`hash-chat-completions-tasks`; only the two identity fields were corrected.
The runner stopped before any trial process or provider request. The failed
preflight and zero-call status are recorded in `runs/attempt-history.json`.
The tasks, prompts, model, product, budgets, analysis and outcomes were not
changed. The freeze manifest was then regenerated and verified.

## Outcomes and analysis

For each arm and family, retain task-oracle outcome, agent-observed memory
source coverage, independent-probe outcome, safety violations, turns, calls,
tokens, wall time, and operator setup time as separate measures. Every
registered family remains in the denominator. Do not infer success from a
missing trace or receipt. Report the two captures separately and summarize
family-level outcomes without treating repeats as additional independent
families. There are no promotion criteria or confirmatory hypotheses. If descriptive family-cluster intervals are reported, use the pinned seed `2026112324`, 10,000 draws and 95% confidence level from `plan.json`; these intervals do not make this a powered or confirmatory study.

The study does not test whether bounded fact evidence improves memory quality
or agent performance. Those claims require separate deterministic quality
gates and a feasible, powered agent-benefit comparison against an appropriate
workflow baseline.
