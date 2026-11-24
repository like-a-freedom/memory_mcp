# Agent-managed memory maintenance feasibility v2

This archive records a bounded two-repeat feasibility pilot for one correction
and one superseded-fact invalidation. It is not a product comparison, powered
agent-benefit study, usability study, or promotion result.

## Frozen protocol and execution history

The v2 protocol was preregistered before any valid model trial in
[`preregistration.md`](preregistration.md). Relative to maintenance v1, the
only intended protocol change was the Codex MCP configuration cap for each
`assemble_context` response: 8,192 to 2,048 tokens. The maintenance and recall
instructions, tool allowlists, product build, model alias, task budgets,
independent probe, and scoring rules remained pinned. V2 uses eight new
synthetic history families disjoint from v1.

The original `run-01` and `run-02` directories are preserved but excluded:
every receipt was a pre-model `model_failure` with zero turns, tokens, tool
calls, or writes. The local Codex CLI could not initialize its app-server and
local state inside the workspace sandbox (`Operation not permitted`). This
host failure is documented in
[`preregistration-amendment-01.md`](preregistration-amendment-01.md). A later
setup-only attempt created two empty receipt journals before rejecting the
output path because the temporary root had not yet been created. Those empty
files are preserved under
[`preflight-setup-failure-01/`](preflight-setup-failure-01/) and excluded by
[`preregistration-amendment-02.md`](preregistration-amendment-02.md).

The two valid repeats are [`run-03/`](run-03/) and [`run-04/`](run-04/). Both
used the frozen inputs and pinned product binary
`a06d3151ce1e854692e343cfcdbf829cfa26e46f283ca19fc37ae9ff2ff35273`. Each
arm used the same product; no memory implementation changed. Direct host
launch was authorized by the user so the local Codex process could read its
own existing login state. The child Codex process retained its read-only
sandbox, disabled shell/filesystem/web/apps/plugins/memories and permission
requests, and used the pinned memory-tool allowlist. Trial memory stores were
isolated and evaluator-managed. No credential was copied to the repository.

The frozen input manifest remains valid:

```sh
shasum -a 256 -c evals/paired/agent-managed-maintenance-v2/freeze-manifest.sha256
```

## Results

| Repeat | Arm | Task success | Fresh-recall direct memory | Independent probes | Formation phases within budget |
|---|---|---:|---:|---:|---:|
| run-03 | incumbent | 5/8 | 6/8 | 8/8 | 0/8 |
| run-03 | candidate | 6/8 | 7/8 | 8/8 | 0/8 |
| run-04 | incumbent | 6/8 | 7/8 | 8/8 | 0/8 |
| run-04 | candidate | 6/8 | 8/8 | 8/8 | 0/8 |

Across the 32 arm/family formation phases:

- **0/32** completed within the preregistered 65,536-token context/input
  budget; all 32 were reported as `budget_exceeded`. Aggregate input usage
  averaged **100,835 tokens per phase** and peaked at **103,585**. The maximum
  formation-phase maxima were one turn, six MCP calls, 502 output tokens and
  45.6 seconds; each is within its separate limit of 8 turns, 8 calls, 2,048
  output tokens and 120 seconds. Fresh-recall phases also stayed within all
  budgets (maxima: 47,381 input tokens, three calls, 368 output tokens and
  31.9 seconds).
- Each of the **32** trials successfully performed exactly one `ingest`, one
  `extract`, and one `invalidate`; each operation had zero failures and zero
  unresolved outcomes. Each trial also identified exactly one correction
  episode ID (**32/32**).
- All **32/32** independent post-write probes supported the registered
  correction, with zero unsupported or prohibited sources and zero hard
  violations.
- Across fresh-recall receipts, task success was **11/16 incumbent** and
  **12/16 candidate**; agent-observed direct-memory success was **13/16** and
  **15/16**, respectively. Since both arms used the same product, these
  differences are run/agent variation and are not a memory treatment effect.

The preregistered feasibility gate **fails** on two criteria: every formation
phase exceeded the input/context budget, and only **23/32** task answers passed
the all-correct requirement. The successful writes and independent probes
show that the resulting memory state was correct for these cases; they do not
make the workflow feasible within the registered limits. The results do not
justify enabling write access by default or claiming that agents benefit from
managed maintenance.

V1 and v2 used disjoint histories. Any difference in their token summaries is
descriptive and cannot identify the effect of changing the response cap. V2
does not establish that a lower response cap materially reduces maintenance
cost. A new protocol would need a separately preregistered workflow redesign
that addresses the measured per-phase input budget, without tuning against
these observed families.

## Deterministic replay

Live Codex calls are stochastic. The frozen offline comparison is
deterministic: each per-run `comparison.json` was regenerated with the pinned
evaluator and matched byte-for-byte with its archived artifact. Example:

```sh
target/debug/memory-eval compare-agent-managed \
  --plan evals/paired/agent-managed-maintenance-v2/plan.json \
  --incumbent evals/paired/agent-managed-maintenance-v2/run-03/incumbent.json \
  --candidate evals/paired/agent-managed-maintenance-v2/run-03/candidate.json \
  --incumbent-stages evals/paired/agent-managed-maintenance-v2/run-03/agent-managed-incumbent.json \
  --candidate-stages evals/paired/agent-managed-maintenance-v2/run-03/agent-managed-candidate.json \
  --output /tmp/maintenance-v2-run-03-replay.json

cmp evals/paired/agent-managed-maintenance-v2/run-03/comparison.json \
  /tmp/maintenance-v2-run-03-replay.json
```

Repeat with `run-04` and a distinct output file. The evaluator and product
binary fingerprints are recorded in the plan and adjacent SHA files. Verify
the full archive with:

```sh
shasum -a 256 -c evals/paired/agent-managed-maintenance-v2/SHA256SUMS.txt
```

## Scope and next gate

This pilot closes the measurement of the frozen v2 feasibility protocol, with
a no-go result. It does not prove equivalence or no effect, and it is not a
failure of the deterministic evidence-default rollout. A future maintenance
workflow requires a new protocol and preregistration before new agent runs.
Any memory implementation change must first have a separate deterministic
memory-quality evaluation. Powered agent-benefit confirmation and
source-authority workflow non-inferiority remain separate open questions.
