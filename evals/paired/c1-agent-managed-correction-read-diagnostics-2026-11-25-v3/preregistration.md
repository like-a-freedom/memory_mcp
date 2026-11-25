# C1 agent-managed correction: read-diagnostics recheck

**Status:** frozen before task requests on 2026-11-25.  
**Plan:** `c1-agent-managed-correction-read-diagnostics-2026-11-25-v3`.

## Question

On four fresh synthetic correction histories, can the current agent-managed
correction workflow finish safely, and does the Codex adapter normalize an
actual MCP JSON-RPC `result.structuredContent` reply into per-read item count,
source IDs, and completeness instead of `unknown`?

## Frozen scope and identity

The four families in `tasks.json` are disjoint from all earlier C1 and setup
cohorts. Each begins with one stale fact and asks the agent to extract one
explicit correction, verify it, invalidate only the superseded fact, and
recall the current value. Both arms use the same memory server and
configuration. This is workflow qualification and adapter verification, not
a product-treatment comparison.

The runner, storage and memory server execute locally on native Apple Silicon.
Codex CLI makes remote model requests through the user's already-authenticated
local Codex state; the only task content sent to the model is the synthetic
corpus in this registration, as explicitly authorized. No project, customer,
or personal memory content is included. No LM Studio runtime, CLI, or service
is used. A separate one-line synthetic Codex connectivity smoke passed before
this freeze; it is retained as setup evidence and excluded from C1 scoring.

The plan pins the active `memory-eval` executable, release memory-server
binary, Codex CLI digest/version, tool schema, reader protocol, prompts,
simulator, retry policy, product config, corpus, and oracle. The tool-schema
SHA-256 was read from an independent public-MCP preflight against the exact
release binary. The v1/v2 attempts stopped before a model turn with empty
receipt files or zero tokens/turns/reads; this registration uses new family
names and values. `freeze-manifest.sha256` is the checksum root.

## Gates and stop rule

Capture 01 passes only if each arm has at least 3/4 exact answer-and-citation
task successes, all 4/4 independent post-run probes pass, all four corrections
are extracted and all four exact stale facts invalidated, and there are zero
unsupported/prohibited-source uses, hard violations, resource exits, or
timeouts. If any gate fails, do not run capture 02. Repeating these four
families would establish repeatability only and would not increase the
independent-family count.

For each actual `assemble_context` response, report normalized `outcome`,
`item_count`, `source_episode_ids`, and `completeness` from its content-free
read receipt. A successful structured response with missing or `unknown`
normalized fields is an adapter-verification failure. Missing reads caused by
a task or model failure are reported separately and cannot be counted as
normalizer passes.

## Interpretation

A task-gate failure is a no-go for this exact workflow and model configuration.
It does not show a memory treatment effect. An adapter diagnostic pass proves
only that the captured tool reply was normalized correctly; it does not show
that the agent used the evidence correctly or that memory quality improved.
Do not pool this cohort with earlier C1 screens.
