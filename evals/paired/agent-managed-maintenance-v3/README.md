# Agent-managed memory maintenance feasibility v3

This archive records a bounded evaluation of a shorter one-correction
maintenance workflow. It is not a memory-product comparison, a powered
agent-benefit study, a usability study, or a promotion result.

## Protocol and execution

The v3 protocol was preregistered before any live agent trial. It uses eight
synthetic history families disjoint from maintenance v1 and v2. Each trial
uses a fresh store. Both arms run the same memory binary, configuration, tool
schema, model alias, and task protocol; arm differences are agent/run
variation, not a memory treatment effect.

The required formation sequence is four tool calls: read the active fact,
write and extract the correction inline with `extract`, read to verify the new
fact, then invalidate the exact superseded fact. The fresh recall and
evaluator-owned independent probe run after the maintenance session.

The frozen `run-01` was interrupted after four incumbent and five candidate
recall receipts. It has no complete arm coverage, maintenance-stage
supplements, independent-probe receipts, or comparison; those partial receipts
remain preserved and count as an incomplete failure under the preregistered
denominator rule. The interruption is recorded in
[`run-01/attempt-status.json`](run-01/attempt-status.json). The completed
[`run-02/`](run-02/) is one full paired repeat only. It does not repair run-01
or satisfy the two-repeat gate.

The runner used the existing local Codex login via `--codex-home`; no API key
was required or copied to the repository. The child Codex process retained
its frozen read-only host policy and memory-tool allowlist. Direct local
execution had the user's explicit authorization.

The frozen inputs still match their manifest:

```sh
shasum -a 256 -c evals/paired/agent-managed-maintenance-v3/freeze-manifest.sha256
```

## Run-02 results

| Arm | Recall task success | Agent-observed direct memory | Formation phases within budget | Independent probes |
|---|---:|---:|---:|---:|
| Incumbent | 5/8 | 6/8 | 0/8 | 8/8 |
| Candidate | 4/8 | 5/8 | 0/8 | 8/8 |

Both arms used the same product binary (`a06d3151…2ff35273`). The descriptive
paired deltas are −12.5 percentage points for task success (95% family
bootstrap interval −50 to +25 points) and −12.5 points for agent-observed
direct memory (−62.5 to +37.5 points). The interval is wide, the run is one
repeat, and there is no product treatment contrast; these figures are not
evidence that the candidate memory is worse or better.

Across the 16 arm/family formation phases:

- **0/16** stayed within the preregistered 65,536 input/context-token limit;
  every phase was `budget_exceeded`. Mean input was **70,069** tokens for the
  incumbent (range 69,194–71,215) and **70,088** for the candidate (range
  69,125–71,240).
- Each phase used exactly four tool calls in one turn, with no clarification.
  Mean formation wall time was 32.9 seconds for incumbent and 32.0 seconds for
  candidate, within the separate 120-second limit.
- Both arms completed one inline extraction and one exact invalidation in all
  eight trials. Each returned and mapped one episode ID per trial. No separate
  `ingest` call was used.
- All **16/16** independent post-write probes supported the registered
  correction. Unsupported sources, prohibited sources, and hard violations
  were all zero.
- Fresh-recall phases remained within their separate resource limits. Their
  task-oracle success was not complete, so the frozen all-correct criterion
  was also unmet.

The deterministic `compare-agent-managed` replay matched the archived
`run-02/comparison.json` byte-for-byte. Its overall direct-memory and task
success deltas are both −0.125; the 95% family-cluster intervals cross zero.
This is a diagnostic repeatability result, not a memory improvement claim.

## Decision and next gate

The v3 feasibility gate is **no-go**: run-01 is incomplete, run-02 is only one
repeat, all run-02 formation phases exceed the token budget, and task success
is not all-correct. Successful writes and probes establish that the tested
corrections reached the intended memory state in these cases; they do not make
this workflow feasible within the frozen limits.

V3 reduced the v2 mean formation input from 100,835 tokens to about 70,078,
but this is descriptive across different disjoint cohorts and does not isolate
the workflow change. It still misses the budget by about 7%. Do not reuse or
tune on these histories for a confirmation claim. Any next feasibility attempt
needs a new, pre-registered workflow and disjoint cases, while preserving
correction provenance, successful single invalidation, complete probes, and
zero hard violations. Keep agent-managed writes opt-in. Powered agent benefit
and source-authority legitimate-workflow non-inferiority remain separate open
questions.

## Deterministic replay

```sh
target/debug/memory-eval compare-agent-managed \
  --plan evals/paired/agent-managed-maintenance-v3/plan.json \
  --incumbent evals/paired/agent-managed-maintenance-v3/run-02/incumbent.json \
  --candidate evals/paired/agent-managed-maintenance-v3/run-02/candidate.json \
  --incumbent-stages evals/paired/agent-managed-maintenance-v3/run-02/agent-managed-incumbent.json \
  --candidate-stages evals/paired/agent-managed-maintenance-v3/run-02/agent-managed-candidate.json \
  --output /tmp/maintenance-v3-run-02-replay.json

cmp evals/paired/agent-managed-maintenance-v3/run-02/comparison.json \
  /tmp/maintenance-v3-run-02-replay.json
```

Verify the full archive with:

```sh
shasum -a 256 -c evals/paired/agent-managed-maintenance-v3/SHA256SUMS.txt
```
