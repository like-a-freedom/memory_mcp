# Agent-managed memory maintenance feasibility v3

Registered: 2026-11-22, before any live v3 agent trial.

## Question and estimand

Does a narrower, one-call inline-extraction workflow complete one source-
supported correction and exact superseded-fact invalidation within the frozen
per-turn resource limits while preserving the correction and history contracts?
This is a bounded Codex workflow-feasibility pilot. Both paired arms use the
same product binary and configuration, so the run does not estimate a memory
product treatment effect, population benefit, equivalence, non-inferiority, or
broad agent usability. No memory-quality improvement claim follows.

## Frozen workflow

The maintenance session has exactly three allowed tools:
`assemble_context`, `extract`, and `invalidate`. Its task requires four calls in
order: (1) read the exact active fact and retain its `fact_id`; (2) call
`extract` once with the correction as inline content plus its registered
`source_type`, `source_id`, and `t_ref` (the tool performs ingestion followed
by extraction in one request); (3) read again and verify the corrected value
is supported by that returned episode; (4) invalidate only the exact stale
`fact_id` after verification. If extraction is partial, has no episode ID, or
produces no facts, the agent must leave the old fact intact and ask for
clarification. `ingest` is not enabled. The formation session omits a final
read because the protocol already has a fresh read-only recall session after
maintenance, followed by a separate evaluator-owned fixed-query probe on the
same store. This is not a claim that `extract` is transactional across its
internal persistence steps.

The 8 v3 history families are disjoint from the v1 and v2 maintenance cohorts.
They share a simple synthetic fact/correction template; results are limited to
workflow feasibility on these fixtures. Each trial gets a fresh isolated
store. Two complete repeats run both arms; each arm points to the identical
product binary, product configuration, tool schemas, and evidence rollout
stage. Arm differences therefore describe agent/run variation only.

## Frozen identities and execution

- `plan.json` pins the task, hidden oracle, Codex CLI, model alias, evaluator,
  product binary/configuration, prompt, protocol, retry policy and simulator.
- `tasks.json` and `fingerprints.json` are the full runner input and scorer
  identity. The hidden oracle is never sent to the agent.
- Exact prompt bytes, host policy, simulator, protocol and retry policy are
  archived in this directory. `freeze-manifest.sha256` covers the registered
  inputs and binary identity files. Any change after the first model
  invocation invalidates this cohort; preserve outputs and register a new one.
- Use the existing local Codex login through `--codex-home`; no API key is
  needed or copied to artifacts. The child Codex process is read-only, with
  shell, filesystem, web, plugins, apps, memories and permission-request tools
  disabled. The runner itself is launched directly on the user's laptop under
  the user's explicit authorization.
- Complete all 8 families in both arms for two repeats. Keep every trial in
  the denominator after the first agent invocation; do not exclude failures,
  timeouts, partial writes, over-budget turns or unresolved receipts.

## Frozen budgets and decision criteria

Maintenance and fresh-recall phases are each limited to 65,536 input/context
tokens per model turn, 8 turns, 8 tool calls, 2,048 generated output tokens,
and 120 seconds. The v3 `assemble_context` response cap is 2,048 tokens.
Maintenance feasibility requires all 32 arm/family/repeat trials to satisfy
all of the following:

1. The maintenance agent completes exactly four tool calls: two
   `assemble_context` reads, one `extract` call with inline correction
   content, and one `invalidate` call. There must be no `ingest` call or other
   write, and every write response must succeed exactly once.
2. The agent identifies and returns exactly one episode ID for the inline
   extraction. The second read supports the corrected value from that episode
   before invalidation. The exact old fact is invalidated only after this
   verification; the fresh recall phase must answer correctly from current
   memory.
3. The independent fixed-query probe supports the registered correction with
   zero unsupported sources, prohibited sources, or hard violations.
4. Every maintenance and recall phase completes within all frozen resource
   limits. Clarification, missing/duplicate/failed writes, unbound episode
   IDs, incorrect task answers, unsupported evidence, or budget overruns fail
   the gate.
5. Offline comparisons regenerated from the frozen receipts match the
   archived deterministic reports byte-for-byte.

Report maintenance/recall correctness, fixed-probe evidence, exact tool
accounting, source binding, and per-turn/total input, output, tool, turn and
wall-time measures by family, arm and repeat. A passing gate establishes only
that this specific workflow is feasible on the pinned Codex CLI, model alias,
host and synthetic cases. It does not guarantee usefulness to agents or users,
prove improvement over another workflow, or justify changing the product
memory implementation. If the gate fails, retain the no-go result and do not
tune this cohort after observation.
