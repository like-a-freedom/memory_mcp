# C1 agent-managed correction feasibility screen

**Status:** frozen before the first Codex task request on 2026-11-25.

## Question and scope

On four fresh synthetic histories, can the current agent-managed maintenance workflow safely apply one explicit correction, verify it, invalidate only the superseded fact, and answer a later query from the corrected memory? Both paired arms use the same current memory server binary, feature set, config and tool schema. This is workflow qualification; it is not a memory treatment comparison, population estimate, or proof of general agent usability.

## Frozen cohort and identity

`tasks.json` defines four disjoint synthetic families, old facts, corrections, event IDs, valid-time boundaries, recall prompts and exact answer/source oracles. `plan.json` pins the trial order, Codex CLI and model alias, product binaries and configuration, budgets, provider fingerprints, and bootstrap settings. `fingerprints.json`, the v4 protocol/host/simulator/retry configs, prompt files, criteria and this preregistration are covered by `freeze-manifest.sha256`.

The evaluation runner, memory MCP server, and local persistence execute on native Apple Silicon. Codex CLI makes remote model requests using the user's existing local Codex login. The only task content sent to that model is the synthetic corpus in `tasks.json`; no project, customer, or personal memory content is included. No model response or prompt may be copied into tracked artifacts except the content-free normalized receipts produced by the runner. No LM Studio runtime is used in this cohort.

## Gates and stop rule

Capture 01 passes only if, in each arm, at least 3/4 exact task oracles pass; all four independent post-run probes pass; all four corrections are extracted and all four exact stale facts are invalidated; and there are zero unsupported/prohibited-source uses, hard violations, resource exits, or timeouts. If any gate fails, stop and do not run a repeat. If every gate passes, one capture 02 may repeat these same families for workflow repeatability only. It does not add independent families and cannot support a treatment or population claim. Do not change tasks, prompts, model, sampling, binaries, tool profile, budgets or gates after the first task request.

## Interpretation

Report answer correctness, agent-visible memory evidence, independent probes, extraction/invalidation receipts, safety, status, turns, tool calls, token counts and wall time separately. An independent probe pass does not substitute for agent task success. Since both arms use the same product binary, any paired difference is workflow noise and not a product treatment effect. Do not use this four-family screen to claim memory quality improved.
