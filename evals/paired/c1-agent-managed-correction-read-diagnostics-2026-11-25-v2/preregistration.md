# C1 agent-managed correction: read-diagnostics recheck

**Status:** frozen before task requests on 2026-11-25.  
**Plan:** `c1-agent-managed-correction-read-diagnostics-2026-11-25-v2`.

## Question

On four fresh synthetic correction histories, does the current agent-managed
correction workflow finish safely, and does the Codex adapter normalize the
actual MCP JSON-RPC `result.structuredContent` response into per-read item
count, source IDs, and completeness instead of `unknown`?

## Frozen scope and identity

The four families in `tasks.json` are new and disjoint from prior C1 cohorts.
Each begins with one stale fact and asks the agent to extract one explicit
correction, verify it, invalidate only the superseded fact, and recall the
current value. Both arms use the same memory server and configuration; this
is agent-workflow qualification and adapter verification, not a product
-treatment comparison. The only model input is synthetic task content. The
Codex CLI runs locally on Apple Silicon using the already-authenticated local
Codex state; model inference is remote and contains only these synthetic
fixtures, as explicitly authorized. No LM Studio runtime is used.

The plan pins the active `memory-eval` executable, memory server binary, Codex
CLI digest/version, tool schema, reader protocol, prompts, simulator, retry
policy, product config, corpus, and oracle before the first task request.
`freeze-manifest.sha256` is the checksum root. Tasks use the frozen C1
agent-maintenance-v4 tool profile and registered eight-call/eight-turn budgets.

## Gates and stop rule

Capture 01 passes only if each arm has at least 3/4 exact answer-and-citation
task successes, all 4/4 independent post-run probes pass, all four corrections
are extracted and all four exact stale facts invalidated, and there are zero
unsupported/prohibited-source uses, hard violations, resource exits, or
timeouts. If any gate fails, do not run capture 02. A repeat of these four
families would establish repeatability only and would not increase the
independent-family count.

For every actual `assemble_context` response, also report normalized
`outcome`, `item_count`, `source_episode_ids`, and `completeness` from the
content-free read receipt. A successful structured response with a missing
or `unknown` normalized result is an adapter-verification failure; it is not
silently counted as an empty read. Missing reads caused by a task failure are
reported separately.

## Interpretation

A task-gate failure is a no-go for this exact workflow and model configuration.
It does not show a memory treatment effect. Even if the adapter diagnostic
passes, it proves only that the captured tool reply was normalized correctly;
it does not show that the agent used the returned evidence correctly or that
memory quality improved. Do not pool this cohort with the earlier C1 screens.
