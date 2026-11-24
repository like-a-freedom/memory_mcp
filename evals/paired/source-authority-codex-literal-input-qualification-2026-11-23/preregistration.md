# Codex literal-source qualification — run 01

**Registered:** 2026-11-23, before this invocation.

## Question and scope

Can the current Codex CLI profile complete a supported source-memory workflow when the incumbent receives the literal file text through the frozen formation prompt and the candidate receives the same source bytes through evaluator-side `memory_mcp import file` before a read-only recall session? Both arms use the same memory product binary, configuration, tool schema, claim stage, model alias, isolated stores, recall question, independent probe, and per-trial limits. The contrast is workflow only; this is not a memory implementation comparison, causal effect, or powered agent-benefit study.

The four synthetic source families are disjoint from all previously exposed agent cohorts. The frozen incumbent prompt contains the exact source line as literal text and explicitly says not to pass a file path as `ingest.content`. The candidate fixture contains the same bytes and is imported by the evaluator. The agent has no shell or network access. Keep prior registrations and runs unchanged.

## Frozen screen

- Four independent source-file families, one paired trial per family.
- Same current `memory_mcp` product binary and config in both arms; fresh isolated per-arm trial stores.
- Incumbent forms memory using one `ingest` followed by one `extract`; candidate uses evaluator operator import and extraction before a new read-only recall session.
- Codex CLI 0.171.0, requested alias `gpt-6.1-sol`, existing local Codex login. The immutable served-model revision is not available; identity is same requested alias within this invocation.
- Limits per trial: 65,536 context/input tokens, 8 tool calls, 8 turns, 1,024 output tokens, 120 seconds; no runner retries.
- No second capture. A pass only permits planning a separate broader study with new families.

## Stop/continue rule

This screen passes only if both arms achieve at least 3/4 task-oracle successes, both arms pass all 4/4 independent fixed-query probes, the candidate operator-import task count is at least 3/4, and there are zero hard-violation and budget-exceeded trials. Any failed condition is a stop for this Codex configuration. A pass is only model/workflow qualification: it does not establish memory benefit, general agent usability, non-inferiority, or a population rate.

No real user data is included in the task manifest. The model invocation receives synthetic facts and questions; raw answer text is not retained in run artifacts.
