# Codex source-authority diagnostic qualification — run 01

**Registered:** 2026-11-23, before model invocation.

## Question and scope

Can the existing Codex CLI profile complete a corrected source-memory workflow on four fresh synthetic source families, and which content-free task-oracle component accounts for any failure? Both arms use the same product binary, schema, configuration, prompts, model alias, isolated stores, recall question, independent probe and limits. The contrast is workflow only: agent-directed literal-text ingest/extract versus evaluator-side file import followed by read-only recall. This is a model/workflow screen, not a memory implementation comparison, causal effect, powered agent-benefit study or population estimate.

All four families are disjoint from prior exposed agent cohorts and from the first sandboxed preflight attempt registered in the parallel diagnostic directory. The incumbent receives the exact source line as literal prompt text; candidate operator import uses identical bytes. The agent has no shell or network access. Raw answers are not retained.

## Frozen design and gate

- Four independent source-file families, one paired trial per family.
- Same current `memory_mcp` binary, config, tool schema and claim stage in both arms; fresh isolated trial stores.
- Codex CLI 0.171.0, requested alias `gpt-6.1-sol`, existing local Codex login. Immutable served-model revision is unavailable; identity is same requested alias within this invocation.
- Per-trial limits: 65,536 input/context tokens, 8 tool calls, 8 turns, 1,024 output tokens and 120 seconds; no retries.
- One capture only. Pass only if each arm has at least 3/4 task-oracle successes, both arms pass 4/4 independent probes, candidate imports succeed in 4/4 cases, and there are zero hard violations or budget exceedances.

A passing screen only permits planning a separate broader study with new histories. On failure, v5 diagnostics distinguish answer presence/schema, accepted answer, required evidence and clarification matching, but a pre-turn runner failure is not a model-quality result. Do not repeat or tune on this cohort.

No real user data is included. Codex receives synthetic facts and questions; local services and artifacts stay on this machine.
