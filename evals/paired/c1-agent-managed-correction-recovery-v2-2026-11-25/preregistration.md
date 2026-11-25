# Preregistration: C1 agent-managed correction with bounded read recovery — v2

**Predecessor:** v1 was invalid setup and yielded zero model turns; its artifacts are archived separately.


**Plan ID:** `c1-agent-managed-correction-recovery-v2-2026-11-25`  
**Stage:** development pilot  
**Independent families:** 4  
**Captures:** capture 01; capture 02 only if every capture-01 gate passes  
**Profile:** `agent_maintenance_v5`

## Question and scope

On four fresh disjoint synthetic histories, can the v5 agent workflow apply one
explicit correction, verify the new fact, invalidate only the superseded fact,
and answer the later question from the corrected memory? The v5 recall phase
allows at most one retry only after a successful empty read whose exact
guidance says to refine the query. It changes only query wording; all other
request arguments and the original time boundary remain fixed. Failed,
partial, nonempty, and unsupported reads are never retried.

Both paired arms use the same current product binary, product configuration,
public MCP schema, model alias, profile, task inputs, and budgets. The arm labels
are execution order only. This is workflow qualification, not a memory
implementation treatment comparison, population estimate, or causal estimate
of retry benefit.

## Frozen data and execution

`tasks.json` contains four synthetic organizations, old facts, corrections,
update instructions, event IDs, valid-time boundaries, questions and exact
answer/source oracles. Their family IDs and values are disjoint from archived
C1 cohorts. No project, personal, customer or production memory is included.

Run the eval harness and memory server on native Apple Silicon. The optional
Codex CLI adapter makes remote model requests through the user's existing
local Codex login; only this preregistered synthetic content is sent. The model
alias is `gpt-6.1-sol`, Codex CLI is pinned to `0.171.0`, and model identity is
only guaranteed as the same alias within an invocation. No LM Studio process,
CLI or endpoint is used. The harness and scorer do not depend on LM Studio.

Each task starts from a fresh isolated store. One attempt is permitted per arm
and family, runner retries are zero, the context budget is 65,536 tokens, the
tool-call and turn budgets are eight, the output budget is 1,024 tokens, and
each phase has a 120-second wall-time limit. Agent reads remain private except
for content-free receipt diagnostics; raw arguments, returned text and fact IDs
are not written to receipts. The private scorer temporarily matches the fact
ID from a successful nonempty read response to the single invalidation request
and checks that the fact came from a source the frozen oracle marks as
superseded. Event sequence numbers and IDs remain transient; only the boolean
result is persisted in the v6 stage sidecar. The independent probe is a
separate evaluator read through the public memory interface and is scored
against the frozen source oracle. Its synthetic source event IDs remain in the
stage sidecar so the evaluator can replay the probe oracle exactly.

## Gates and stop rule

Capture 01 passes only when **each arm** meets every criterion in `criteria.json`:

- at least 3/4 exact task answers with the required citation;
- 4/4 independent probe successes;
- 4/4 successful correction extractions and 4/4 exact stale-fact invalidations;
- 4/4 private exact-target checks showing a successful read response returned
  the fact from a superseded source before the invalidation request;
- 4/4 extract-evidence responses observed before invalidation;
- zero unsupported or prohibited source uses, hard violations, budget exits,
  or timeouts.
- complete read-capture status and zero bounded-retry contract violations;
- at least one exact guidance-authorized query-only retry in each arm, so the
  recovery path is actually exercised.

After the live run, `compare-agent-managed` writes a descriptive comparison;
`evaluate_c1.py` combines the paired receipts and sidecars and is the only
capture-02 gate. It requires paired artifact schema v8 and stage schema v6,
checks exact task and family coverage, requires the task and probe floors,
exact one-call extract/invalidate outcomes, exact stale-fact target matches,
4/4 evidence responses before invalidation, and zero safety or resource
violations in each arm. It reuses `validate_recovery.py` to verify complete
read capture and every bounded retry's guidance hash, changed query digest,
and unchanged non-query argument digest. Deterministic tests are
`test_validate_recovery.py` and `test_evaluate_c1.py`. No retry exposure in
either arm makes the screen inconclusive and blocks capture 02.

A probe pass does not substitute for an agent task success. If any criterion
fails, stop and preserve the failed capture without repetition. If every
criterion passes, capture 02 may repeat the same frozen cases exactly once to
check workflow repeatability. The repeat remains four independent families;
it does not increase the sample size. No prompt, data, model, binary, budget,
profile or criterion may be changed after first exposure.

## Outcomes and interpretation

Report exact task success, agent-observed evidence, independent probe success,
write/invalidation receipts, violations, status, latency, turns, tool calls and
tokens separately. Keep all trials in the denominator. Report the two paired
arms as same-product workflow noise. Do not claim that v5 improves memory
metrics, agent utility or latency from a four-family development screen.

The frozen profile hash covers both phases: the unchanged v4 correction phase
and a separately pinned read-only recovery phase. The registered product hash,
tool schema, evaluator, Codex CLI, task/oracle hashes, policies and exact
commands are in `plan.json`, `fingerprints.json` and `freeze-manifest.sha256`.
