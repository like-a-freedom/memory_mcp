# C1 agent-managed correction feasibility screen — direct local Codex

**Status:** frozen before task capture on 2026-11-25. This cohort replaces an earlier invalid setup attempt; it uses four fresh, disjoint synthetic families.

## Question and scope

On four fresh synthetic histories, can the current agent-managed maintenance workflow safely apply one explicit correction, verify it, invalidate only the superseded fact, and answer a later query from corrected memory? Both arms use the same current memory server binary, feature set, config and tool schema. This is workflow qualification, not a memory-treatment comparison, population estimate, or proof of broad agent usability.

## Prior setup attempt

The archived cohort `c1-agent-managed-correction-codex-2026-11-25/` produced zero Codex turns, calls and tokens in every receipt, with process exits around 0.3 s. It did not demonstrate an agent workflow result and is excluded from this screen. A separate synthetic `codex exec` preflight then succeeded when run directly on the authorized laptop with access to its existing Codex state; Codex itself remained `read-only`. This registration uses new families to avoid any exposure ambiguity.

## Frozen cohort and identity

`tasks.json` defines four fresh synthetic families, old facts, corrections, event IDs, valid-time boundaries, recall prompts and exact answer/source oracles. `plan.json` pins trial order, Codex CLI and model alias, product binary/configuration, budgets, provider fingerprints and bootstrap settings. `fingerprints.json`, protocol/host/simulator/retry configs, prompt files, criteria and this preregistration are covered by `freeze-manifest.sha256`.

The evaluator, memory MCP server and local persistence execute on native Apple Silicon. Codex CLI makes remote model requests using the user's existing local Codex login. The only task content sent to the model is the synthetic corpus in this registration; no project, customer or personal memory content is included. The model's own command sandbox remains `read-only`. No LM Studio runtime is used.

## Gates and stop rule

Capture 01 passes only if, in each arm, at least 3/4 exact task oracles pass; all four independent post-run probes pass; all four corrections are extracted and all four exact stale facts are invalidated; and there are zero unsupported/prohibited-source uses, hard violations, resource exits or timeouts. If any gate fails, stop and do not run a repeat. If every gate passes, one capture 02 may repeat these same families for workflow repeatability only. It does not add independent families and cannot support a treatment or population claim. Do not change tasks, prompts, model, sampling, binaries, tool profile, budgets or gates after the first task request.

## Interpretation

Report answer correctness, agent-visible memory evidence, independent probes, extraction/invalidation receipts, safety, status, turns, tool calls, token counts and wall time separately. An independent probe pass does not substitute for agent task success. Since both arms use the same product binary, a paired difference is workflow noise, not a product effect. Four families are a development screen, not a basis to claim memory quality improved.
