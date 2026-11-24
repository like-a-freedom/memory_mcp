# Source-authority agent workflow comparison v1

Status: preregistered exploratory comparison; not a promotion or non-inferiority study.

## Question

For the frozen synthetic source-file tasks, compare the existing agent-directed
`ingest`/`extract` workflow with a bounded operator import performed before the
candidate agent session, followed by read-only agent recall. Eight ordinary
conversation tasks are included as workflow controls. Each arm uses a fresh
store per task. A fixed evaluator probe runs after the agent session and is
scored independently of agent-selected reads.

The incumbent product is a clean archive of `1ae1da4a4e3120f2de94753fc1a949caee3aced7`.
The candidate is the authorized dirty master binary frozen by its SHA-256 in
`plan.json`. This comparison covers those exact product builds and the
registered workflow; it does not isolate one memory mechanism.

## Frozen cohort and execution

- 16 independent synthetic history families: 8 source-file imports and 8
  ordinary conversation controls, one task per family.
- Two repeated paired invocations of the same frozen 16-family manifest.
- Counterbalanced arm order is fixed in `tasks-v1.json`.
- Codex CLI 0.171.0 and the `gpt-6.1-sol` alias, with same-alias identity
  checked within each invocation. The provider does not expose an immutable
  model revision here; cross-invocation consistency is not assumed.
- Use the existing local Codex home for authentication. Authentication data
  and source content are excluded from receipts.
- Both product arms use stdio, regex extraction, embeddings disabled, the
  `memory_decisions/main` embedded store profile, and claim stage `evidence`.
- Per-phase limits are 65,536 input/context tokens, 8 tool calls, 8 turns,
  1,024 output tokens, and 120 seconds. The runner performs no retries.
- Each operator fixture is staged in a unique evaluator-owned workspace. The
  candidate import uses a confined relative path under that workspace, runs
  before the agent session, extracts the returned episode, and records only
  content-free success/action/time telemetry. The candidate agent receives
  `assemble_context` only. The agent cannot use shell or network tools.

## Outcomes

Report, without promotion thresholds:

1. Task success and agent-observed direct evidence, separately, by source-file
   and ordinary-conversation stratum and by independent family.
2. Independent fixed-probe source support, unsupported/prohibited evidence,
   and hard violations. These outcomes are independent of the agent's chosen
   memory reads.
3. Agent turns, tool calls, clarifications, input/output tokens, and wall time.
4. Operator setup actions, failures, and elapsed time, kept separate from
   agent interaction metrics. Also report the combined action count and
   combined elapsed time descriptively so work shifted to the operator is
   visible.
5. Repeat-to-repeat consistency and all invalid, failed, or budget-exceeded
   trials. Every registered family remains in the denominator.

The deterministic evaluator verifies artifact identities, fixed probe
replayability, source bindings, task coverage and hard-violation accounting.
The Codex observations themselves are stochastic, model-alias-bound
diagnostics; they are not deterministic product-quality scores.

## Interpretation limits

This is a feasibility/usability diagnostic on synthetic tasks, not a powered
test. It has no pilot-derived power analysis, immutable provider model
revision, or preregistered non-inferiority margins. Do not claim broad
workflow acceptance, agent productivity improvement, causal treatment effect,
population reliability, or temporal-selector improvement from this cohort.
Any confirmatory migration decision requires a new disjoint cohort, a
pre-run power analysis for the joint decision rule, deliberate non-inferiority
margins, and a provider identity stable enough for the claim.

The incumbent exposes only the legacy `as_of` selector; the candidate exposes
`valid_at` and `known_revision` while retaining `as_of` as a compatibility
alias. The paired independent probe selects the public API from the observed
tool schema. On the incumbent it records source coverage without claiming a
revision snapshot; on the candidate it validates the requested revision.
Temporal quality is therefore outside this comparison.

The immutable task, protocol, host-policy, simulator, retry-policy,
product-config, schema-change, fingerprint and plan files are checksummed in
`freeze-manifest.sha256`. No model invocation may begin until those hashes and
both real product schemas have been verified.
