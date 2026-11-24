# Source-authority agent workflow comparison v2

Status: frozen exploratory comparison; not a promotion, non-inferiority, or powered study.

## Replacement of v1

This is the retry registration for the unchanged v1 task cohort. The prior attempt is preserved under `source-authority-agent-workflow-v1/failed-run-01-sandbox-and-workspace-isolation/`; all 16 receipts had zero model turns and zero input/output tokens because the restricted sandbox could not initialize the local Codex app-server client. The source-file cases also exposed a runner setup defect before any usable observation. No model outcome was observed and no task was tuned from an outcome. The retry changes only the evaluator binary fingerprint and its per-trial source/workspace routing; all product binaries, tool schemas, prompts, task bytes, oracles, budgets, model alias, transport and paired order remain pinned to the same hashes as v1.

## Question

For the frozen synthetic source-file tasks, compare the incumbent's existing agent-directed `ingest`/`extract` workflow (where the legacy product resolves a relative file path from the MCP server working directory) with the candidate's bounded operator import followed by read-only agent recall. Eight ordinary conversation tasks are controls. Each arm gets a fresh store per task and a fixed independent probe after agent activity.

This measures workflow feasibility and descriptive interaction costs on one synthetic cohort. It does not isolate memory quality or estimate population benefit.

## Workspace and source boundaries

- Each trial has a unique evaluator-owned source root and a separate empty Codex workspace. Source fixture bytes never enter a run artifact.
- In the incumbent source-file arm, the Codex process still runs in the empty workspace. Only its stdio MCP child has the source root as its working directory, so the pinned incumbent binary can resolve the registered relative path. Shell, web, apps, plugins, memories and permission requests remain disabled.
- In the candidate source-file arm, the fixture remains in the evaluator-owned source root. The evaluator invokes the candidate's root-confined `memory_mcp import file` and then `extract` before starting Codex. Candidate Codex and its MCP server use the empty task workspace; the candidate agent is read-only and cannot access the fixture path.
- Conversation controls have no operator fixture. Both agents retain the frozen profile's registered tool surface.
- This routing was regression-tested through the public paired trial runner: incumbent MCP cwd points at the source root while Codex cwd is empty; candidate import succeeds from the source root while its Codex cwd remains empty; the independent probe finds the source episode in each path.

## Frozen cohort and execution

- 16 independent synthetic history families: 8 source-file imports and 8 ordinary conversation controls; one task per family.
- Two repeated invocations of the same frozen 16-family manifest. The repeats are reliability diagnostics over the same families, not new independent samples.
- Counterbalanced arm order is fixed in `tasks-v1.json`.
- Codex CLI 0.171.0 and `gpt-6.1-sol`, with same-alias identity checked within each invocation. The provider does not expose an immutable model revision; cross-invocation consistency is not assumed.
- Use the existing local Codex home for authentication. Credentials and source content are excluded from receipts.
- Both product arms use stdio, regex extraction, embeddings disabled, `memory_decisions/main`, and claim stage `evidence`.
- Per phase: 65,536 context/input tokens, 8 tool calls, 8 turns, 1,024 output tokens and 120 seconds. No retries.

## Outcomes

Report task success and agent-observed direct evidence by stratum/family; fixed independent-probe source support; unsupported/prohibited evidence and hard violations; agent turns, calls, clarifications, tokens and latency; operator setup actions/failures/time separately and combined descriptively; repeat consistency; and every failed, invalid or budget-exceeded trial in the denominator. Preserve raw receipts and deterministic summaries for both invocations.

The runner validates identities, coverage, source bindings, probe replayability and hard-violation accounting. Codex agent responses are stochastic model-alias-bound diagnostics, not deterministic memory-quality measurements. The fixed independent-probe outcome is deterministic given a product binary and trial setup.

## Interpretation limits

This is an exploratory feasibility/usability diagnostic on 16 synthetic families with two invocations. It has no pilot-derived power analysis, immutable provider revision, or preregistered non-inferiority margins. Do not claim broad workflow acceptance, agent productivity improvement, causal treatment effect, population reliability, temporal-selector improvement, or general memory-quality improvement. Any confirmatory migration decision needs a new disjoint cohort, a pre-run power analysis for the joint decision rule, deliberate non-inferiority margins, and stable enough provider identity. The incumbent exposes the legacy `as_of` selector; candidate additionally exposes `valid_at`/`known_revision` while retaining `as_of` as an alias. The independent probe selects the public API from observed schema; this run is not a temporal-quality test.

The inputs below are frozen by `freeze-manifest.sha256`. No model invocation begins until hashes, plan and product identities are verified.
