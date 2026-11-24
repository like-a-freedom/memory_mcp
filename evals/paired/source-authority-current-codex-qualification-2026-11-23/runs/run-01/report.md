# Current Codex source-authority qualification — run 01

**Decision: NO-GO for proceeding to the broader agent workflow pilot under the preregistered screen.** This is an exploratory workflow/model qualification, not a memory implementation comparison or a powered agent-benefit study.

## Capture integrity

- All 16 preregistered synthetic families produced a receipt in each arm (16/16 paired trials). The capture ran from `2026-11-23T14:43:06.251015Z` to `2026-11-23T15:03:22.111765Z` (20m 15.861s).
- Both arms used the same current `memory_mcp` product binary (`3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`), product configuration, tool schema, claim stage, model alias, task set and limits. The registered workflow contrast was agent `ingest` + `extract` versus evaluator operator import followed by read-only recall.
- The families contain synthetic data only. The service received synthetic histories and prompts; local memory processes and artifacts stayed on this laptop. No raw model answers are retained.
- The frozen registration manifest validates. The offline comparison replay matched the archived comparison byte-for-byte; both files have SHA-256 `a9bf97169dda1e497802282b12971ee2cee7977d6d2c8539bc52f4377bdb5b59`.
- Agent identity was Codex CLI `0.171.0` and requested model alias `gpt-6.1-sol`, using the existing local ChatGPT login. The provider's immutable served-model revision was not available, so identity is limited to the same requested alias within this invocation.

## Frozen outcomes

| Measure | Incumbent | Candidate | Registered criterion |
|---|---:|---:|---|
| Task-oracle success | 8/16 | 15/16 | At least 12/16 in each arm — **fail** |
| Independent fixed-query probe | 16/16 | 16/16 | 16/16 in each arm — pass |
| Candidate source-file task success | — | 8/8 | At least 4/8 — pass |
| Agent-observed direct-memory success | 8/16 | 16/16 | Descriptive only |
| Hard-violation trials | 0 | 0 | Zero — pass |
| Budget-exceeded trials | 0 | 0 | Zero — pass |

The paired deterministic scorer reports task-success difference `+0.4375` (95% family-cluster interval `[0.125, 0.75]`) and agent-observed direct-memory difference `+0.5` (95% interval `[0.25, 0.75]`). By stratum, task success was 8/8 incumbent versus 7/8 candidate for conversation controls, and 0/8 incumbent versus 8/8 candidate for source-file workflows. These are descriptive results from 16 exposed synthetic families under one requested model alias. The interval does not convert this workflow contrast into a memory effect or population-level claim.

In the incumbent source-file stratum, all eight task receipts completed but failed both direct-memory and task-oracle checks; its eight separate independent fixed-query probes passed. A post-capture contract audit found a benchmark defect: every incumbent formation prompt instructs the agent to pass the relative file path as `ingest.content`, while the published product contract says MCP `content` is literal and must never trigger file I/O. The supported path-reading operation is the explicit operator CLI `import file` ([contract](../../../../../docs/superpowers/specs/2026-11-09-ingestion-source-authority-design.md#contract), [implementation](../../../../../crates/memory-mcp/src/cli/commands/import.rs)). Thus the 0/8 result records an unsupported task setup, not a memory defect or valid comparison against a supported incumbent workflow. The separate probes show the source state was readable through the registered probe protocol. Ordinary conversation controls were 8/8 incumbent and 7/8 candidate, so this is not evidence of general task incompetence either.

The registered screen fails because incumbent task-oracle success is 8/16, below the required 12/16. The candidate's higher scores and passing file subset do not override that predeclared stop rule. Do not run `run-02`, do not expand this cohort, and do not claim general source-authority usability, non-inferiority, memory improvement or agent benefit. The pre-registration gate and archived scores remain unchanged; the post-capture audit narrows their interpretation because the incumbent file task contradicted the product contract. Any new paired study must preregister a supported incumbent path that gives the agent the literal file content through an authorized host interface, or test operator import as a user workflow separately. It needs disjoint families and a revised validity gate. An agent-facing import capability is outside the current frozen tool surface and needs an ADR before implementation.

The end-to-end 20m 15.861s capture is a single observation, not a controlled model-speed benchmark. It cannot be compared directly with the 73.88-second LFM, Qwen or Ornith screens: those used different models, cohorts, turn/error mixes and adapter settings. Keep LFM for fast endpoint/tool smoke and deterministic evals for the fast inner loop. Any further agent study needs a newly registered disjoint cohort and an agent/model configuration that passes its qualification screen.
