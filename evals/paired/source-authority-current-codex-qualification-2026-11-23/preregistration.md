# Current Codex source-authority qualification screen

**Registered:** 2026-11-23, before this model invocation.

## Question and scope

Can the existing Codex CLI model profile complete the registered source-authority tasks with the current Memory MCP product? The two arms use the **same product binary, schema, configuration, prompts, task cohort, budgets and model**. The only contrast is the workflow: agent-directed `ingest` + `extract` versus operator file import followed by read-only recall. This is a workflow/model qualification screen, not a memory implementation comparison, causal memory effect, population estimate or powered agent-benefit study.

All tasks contain synthetic histories. The 16 families were frozen in the earlier `source-authority-agent-workflow-v1` registration, whose only attempted Codex run stopped before a model turn (0 turns and 0 input/output tokens per receipt). The family set is disjoint from the exposed LFM2.5, Qwen and Ornith cohorts and has not been shown to a model. Keep the earlier failed run unchanged.

## Frozen design

- 16 independent families: 8 ordinary conversation controls and 8 literal source-file workflows; one paired trial per family per capture.
- Two captures are reserved on the same frozen plan. `run-02` is run only if `run-01` meets the screen gate below. The repeated families measure repeatability and are not 32 independent cases.
- Both arms use the same current `memory_mcp` binary SHA-256 `3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`, current configuration/schema, claim rollout stage `evidence`, and fresh per-trial stores.
- Incumbent: agent forms memory with `ingest` + `extract`, then recalls. Candidate: the evaluator imports the literal file before a fresh read-only agent recall session. Agent workspaces remain empty; shell and network are disabled.
- Agent: Codex CLI 0.171.0, model alias `gpt-6.1-sol`, existing local ChatGPT login via explicit `--codex-home`; no `CODEX_API_KEY`. The immutable provider model revision is not exposed, so identity is only same-alias within an invocation.
- Limits: 65,536 context/input tokens, 8 tool calls, 8 turns, 1,024 output tokens and 120 seconds per trial; retries disabled.
- The runner's temporary root is the pre-created `/private/tmp/memory-mcp-source-authority-codex-2026-11-23`; output artifacts remain in the repository registration directory.

## Stop/continue gate fixed before capture

Proceed to a separately preregistered broader agent workflow pilot only if **each valid capture** has: task-oracle success at least 12/16 in each arm; independent fixed-query probes 16/16 in each arm; candidate source-file task success at least 4/8; zero hard violations; and zero budget-exceeded trials. The 12/16 and 4/8 thresholds carry forward the 75% per-arm and 50% source-file screen rates from the earlier four-family qualification; they are screening thresholds, not evidence of benefit. If `run-01` fails any condition, stop and do not run `run-02`.

A passing screen only supports use of this runner/profile for a new study. It does not establish that memory got better, agents find memory more useful, non-inferiority, or any memory treatment effect. A powered comparison still requires a justified smallest worthwhile effect, appropriate baseline, disjoint histories and a preregistered joint-rule power analysis.

No real user data is sent. Codex receives only synthetic task histories and prompts; the local memory server and artifacts stay on this machine.
