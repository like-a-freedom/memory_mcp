# Agent-managed maintenance feasibility v4

Registered on 2026-11-22 before any live v4 Codex trial. Three setup-only
runner preflights failed before an agent invocation: the first rejected
`development_pilot` because that stage requires identical products in both
arms; the second rejected a relative output path; the third rejected a
temporary root that did not yet exist. The plan was corrected to
`exploratory_comparison`, the registered stage for two different product
binaries without a powered promotion claim. The output path is absolute and
the temporary root is created before invocation. All three rejected
preflights had zero Codex invocations, model turns, tool calls, and memory
writes. The third created only two empty receipt journals in `run-01`; those
remain preserved and are not a trial. Attempt records are retained under
`preflight-setup-failure-01/`, `preflight-setup-failure-02/`, and
`preflight-setup-failure-03/`. Valid paired invocations use fresh output paths
`run-02/` and `run-03/`.

## Question and limits

Does opt-in persisted fact evidence let the maintenance agent verify the exact
new statement and source quote from its inline `extract` response, remove the
second verification read, and complete the bounded correction workflow within
the existing resource limits? This is a development feasibility study for one
frozen Codex version/model alias and eight synthetic families. It is not a
memory-quality treatment comparison, a powered agent-benefit study, a
population estimate, a user study, or evidence of equivalence/non-inferiority.

The maintenance v1–v3 attempts remain immutable historical no-go results. The
v4 families are disjoint from all prior maintenance cohorts and will not be
reused or tuned after the first model invocation.

## Frozen protocol

- Eight fixed synthetic history families, each with one durable old fact and
  one explicit user correction; each trial starts with a fresh store.
- The same agent-maintenance v4 profile is used in both arms. It exposes only
  `assemble_context`, `extract`, and `invalidate`; shell, apps, plugins,
  memories, web search, and permission requests are disabled.
- Both arms receive the same task, prompts, model alias, tool allowlist,
  response caps, timeout and runner. The incumbent binary lacks
  `include_fact_evidence`, so the frozen task rules require one extra
  `assemble_context` read before invalidation. The candidate binary may omit
  that read only after checking successful `extract`, returned `episode_id`,
  nonempty persisted statement and source quote.
- In both arms the agent first identifies the exact active fact; then performs
  exactly one inline extraction and, only after verification, invalidates only
  that prior fact ID. The fresh recall and evaluator-owned independent probe
  run afterward against the same trial store.
- No tool may be called to infer evidence that the server did not return.
  Partial/empty extraction, missing episode/evidence, uncertainty, or any
  failed write must leave the prior fact intact and ask for clarification.

## Identities and ordering

`plan.json` binds all task/oracle, model, runner, evaluator, binary, feature,
configuration, schema, prompt, protocol, host-policy, retry-policy and
simulator identities. The incumbent and candidate differ only in product
binary and the declared additive `extract` schema migration recorded in
`schema-change.json`. Each of two invocations uses the same frozen eight
families, fresh stores, and its own run ID. Arm order is frozen per family in
the task keys. The hidden oracle is not sent to Codex.

Codex CLI `0.171.0` uses the existing local ChatGPT login through
`--codex-home`; no API key is required or copied into the runner or artifacts.
The runner executes on the user's laptop under explicit authorization; the
Codex child retains the frozen read-only sandbox and disabled shell.

## Budgets and feasibility rule

Each maintenance and recall phase has a 65,536 input/context-token limit, 8
turns, 8 tool calls, 1,024 generated output tokens and 120 seconds. The pinned
`assemble_context` output limit is 1,024 tokens. Both full paired invocations
must finish all 8 families in both arms. Candidate feasibility requires all
16 candidate maintenance phases (8 per repeat) and all candidate recall
phases to remain within limits; every candidate task answer and independent
probe must be correct; each maintenance phase must complete one exact inline
extract and one exact invalidation with successful writes; all episode IDs
must bind; unsupported/prohibited evidence and hard violations must be zero.
Every incumbent result and every failure remains in the report. A no-go does
not authorize tuning this cohort; another attempt requires a new frozen
workflow and disjoint families.

Deterministic per-run comparisons must reproduce the stored summaries from
their frozen receipts. A passing feasibility gate supports only the narrow
resource/workflow claim above. Even a pass does not prove the response change
improves memory metrics or broad agent utility; those are tested separately by
the deterministic memory gates and a future powered agent-benefit design.
