# Agent-managed maintenance feasibility v4

**Decision: no promotion and no default change.** The frozen feasibility gate
requires two complete paired invocations and every candidate task to pass. Only
one invocation completed; its candidate scored 7/8. The existing
`include_fact_evidence` opt-in therefore remains opt-in.

## Captures

Three setup-only preflights were rejected before any model call and remain
archived. `run-02` is a failed model invocation, not a usable repeat: all 8
receipts in each arm are `model_failure`, with zero turns, tool calls and
tokens. The Codex child failed to initialize under the desktop's restricted
execution context. The same local Codex login worked when run directly on the
authorized laptop; no API key was needed. These receipts are preserved and
count against the preregistered two-run gate.

`run-03` is the only complete paired invocation. It used the frozen eight
synthetic history families and fresh stores:

| Measure | Incumbent | Candidate |
|---|---:|---:|
| Completed task receipts | 8/8 | 8/8 |
| Task-oracle and direct-memory success | 6/8 | 7/8 |
| Maintenance phases within 65,536 input-token budget | 0/8 | 8/8 |
| Mean / maximum maintenance input tokens | 72,454.5 / 86,670 | 56,450.4 / 56,720 |
| Successful extract / invalidate writes | 8/8 / 8/8 | 8/8 / 8/8 |
| Independent post-write probes | 8/8 | 8/8 |
| Unsupported / prohibited sources | 0 / 0 | 0 / 0 |

The paired task/direct-memory estimate is +0.125 with a family-cluster 95%
interval of [-0.25, 0.5] across eight families. It is descriptive and crosses
zero. The candidate's lower formation input and three rather than four memory
calls per correction are also descriptive. This is one exploratory repeat,
not a powered usability result or proof that the feature improves memory
quality.

## Evidence limitation

The frozen v4 runner retained hashes of raw tool calls and responses, but not
normalized booleans for `include_fact_evidence=true`, nonempty persisted
statement/quote returned, or response-before-invalidation ordering. Therefore
the v4 captures cannot prove that the agent received and used the opt-in
evidence before invalidating a fact. The current comparator successfully read
these legacy stage receipts and replayed the report byte-for-byte; its SHA-256
is `9b0d0e7bc2394ff2ee44ae2337ff1b943ec7e3cac51e2f799f8b2ab67955d3c7`.
Missing evidence telemetry is represented as *not measured*, not zero.

The evaluator now has a content-free normalized observation for future
runners: request sequence, whether the flag was true, response sequence, and
whether a nonempty episode ID, statement and quote were returned. Stage reports
count only evidence that arrived before all invalidation calls. Raw statements
and quotes are never copied to receipts. Codex reports complete telemetry only
after a parseable trace; unresolved tool calls make the trace invalid. Other
runners must explicitly declare that their adapter captured all extract
requests and responses, so an absent field cannot be misread as a measured zero.

## Why evidence stays opt-in

For the `episode_heuristic` extraction path, the persisted fact statement and
source quote are both the full episode text. The response currently has no
per-fact or total payload cap. Turning the flag on by default could return
entire stored episodes to callers and increase context usage substantially;
it would also change the compact response contract for callers that omit the
field. The bounded response and privacy behavior has not been evaluated, and
the single valid v4 run does not meet the preregistered gate. These are concrete
reasons to keep the default `false`.

This is separate from the claim-reconciliation `evidence` rollout stage, which
is already the product default and passed its deterministic v9 acceptance.

## Next evidence needed

Before reconsidering a global default, define and test a bounded response
policy that preserves usable evidence or omits it safely when it exceeds the
budget. Then preregister a new disjoint agent cohort; do not reuse or tune the
observed v4 families. Its adapter must record the normalized telemetry above,
and its gates must include exact invalidation, task success, independent probes,
hard-violation checks, response-size and context budgets, and repeated
candidate completion. A passing feasibility result would still not prove a
memory-quality gain; the registered deterministic memory-quality suites remain
the acceptance gates for those claims.

## Artifacts

- Frozen protocol: [`preregistration.md`](preregistration.md)
- Only complete invocation: [`run-03/`](run-03/)
- Failed invocation retained: [`run-02/`](run-02/)
- Current comparator replay and byte-identical repeat:
  `run-03/comparison-telemetry-audit.json` and
  `run-03/comparison-telemetry-audit-replay.json`
