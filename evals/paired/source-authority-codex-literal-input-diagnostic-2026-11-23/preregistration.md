# Codex source-authority diagnostic qualification — run 01

**Registered:** 2026-11-23, before the model invocation.

## Question and scope

Can the existing Codex CLI profile complete a corrected source-memory workflow on a fresh, disjoint four-family cohort, and which content-free task-oracle component accounts for any failure? Both arms use the same product binary, schema, configuration, prompts, task cohort, model alias, isolated stores, recall question, independent probe, and limits. The contrast is workflow only: agent-directed literal-text ingest/extract versus evaluator-side file import followed by read-only recall. This is a model/workflow screen, not a memory implementation comparison, causal effect, powered agent-benefit study, or population estimate.

The four synthetic families are newly authored and disjoint from all prior agent cohorts. The incumbent receives the exact source line as literal prompt text; candidate operator import uses identical bytes. The agent has no shell or network access. Raw model answers are not retained.

## Frozen design

- Four independent source-file families; one paired trial per family.
- Same current `memory_mcp` binary, config, tool schema and claim stage in both arms; fresh trial stores.
- Codex CLI 0.171.0, requested alias `gpt-6.1-sol`, existing local Codex login. Immutable served-model revision is unavailable; identity is same requested alias within this invocation.
- Limits per trial: 65,536 input/context tokens, 8 tool calls, 8 turns, 1,024 output tokens and 120 seconds; no retries.
- One capture only. Stop on any failed gate.

## Stop rule and interpretation

Pass only if each arm has at least 3/4 task-oracle successes, both arms pass 4/4 independent probes, candidate imports succeed in 4/4 cases, and there are zero hard violations or budget-exceeded trials. A pass permits planning a separate broader study with a new cohort; it does not establish memory benefit or general agent usability. If the screen fails, use v5 per-trial diagnostics (answer presence/schema, accepted answer, required evidence, clarification match) to describe the failure without retaining answer content. Never retry or tune on this exposed cohort.

No real user data is included. Codex receives synthetic facts and questions; the local memory service and artifacts stay on this machine.
