# GPT-6.2 Sol source-authority workflow qualification

**Registered:** 2026-11-24 before the qualification capture.

## Question and scope

Can Codex CLI 0.171.0 using the requested `gpt-6.2-sol` alias complete the registered source-memory workflow on four disjoint, entirely synthetic families? Two cases are ordinary conversation-memory controls. Two cases compare literal source text supplied to the incumbent agent with evaluator-side root-confined source-file import for the candidate. Both arms use the same memory product binary, configuration, schema, model alias, independent-probe protocol, and resource budgets. This qualifies only this agent/workflow configuration; it is not a product-memory treatment effect, causal estimate, powered agent-benefit study, population rate, or proof of universal usefulness.

The served model revision is not exposed by the selected Codex CLI contract. The model identity is pinned to the same requested alias during one invocation, with CLI version and executable SHA-256 recorded.

## Data boundary

Every task fact, question, source fixture, identifier, and expected answer in `tasks-v3.json` is synthetic and newly authored for this registration. The user explicitly authorized remote Codex use for synthetic evaluation data. Remote model input is limited to these registered synthetic prompts plus the harness tool schema and synthetic MCP responses. Codex runs ephemerally in a read-only trial workspace, with shell, web search, apps, plugins and Codex memories disabled. No real user content or source files are supplied. The runner stores normalized outcomes and digests rather than raw model answers. Existing local Codex login is used; no API key is required or recorded.

## Frozen design

- Four new independent families, one paired trial each: two `conversation_control`, two `source_file_import`.
- Incumbent source-file formation passes the exact synthetic source text to `ingest`, followed by `extract`; candidate source-file setup imports and extracts the same bytes in evaluator-owned setup before a fresh read-only agent session.
- Independent fixed-query probe runs after the agent session on the same isolated per-trial store; it is scored locally by the deterministic harness.
- Same current product binary and config in both arms. Isolated stores. No runner retries. No second capture or tuning on these exposed families.
- Resource limits per trial: 65,536 context/input tokens, 8 tool calls, 8 turns, 1,024 output tokens, 120 seconds.

## Frozen continue rule

The model/workflow screen passes only if (a) task-oracle success is at least 3/4 in each arm; (b) independent fixed-query probes pass 4/4 in each arm; (c) candidate source-file task answers pass 2/2; (d) candidate operator import and extraction pass 2/2; and (e) there are zero hard violations and zero budget-exceeded trials. Any failed gate is NO-GO for this exact configuration; preserve the run and do not retune or repeat this cohort. The 2/2 source-specific threshold prevents the two conversation controls from concealing a failure on either operator-import task.

Even a full pass only permits designing a separate study. Both arms use the same product binary, so this contrast cannot establish memory improvement. Powered agent-benefit confirmation remains separate and requires its own estimand, worthwhile effect, power analysis, and fresh held-out data.
