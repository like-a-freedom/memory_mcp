# C1 agent-managed correction screen

## Decision

**Not qualified; stop after capture 01.** The frozen gate requires at least
3/4 exact final-task successes in each arm. The incumbent passed 3/4 and the
candidate 2/4. The other safety and memory-state gates passed, but the workflow
gate did not. A second capture was therefore not run. This is a development
screen on four synthetic families, not a treatment comparison or a population
estimate; both arms used the same memory binary and configuration.

## Results

| Measure | Incumbent | Candidate |
| --- | ---: | ---: |
| Completed recall tasks | 4/4 | 4/4 |
| Exact answer plus required citation | 3/4 | 2/4 |
| Agent-visible direct-memory evidence | 3/4 | 2/4 |
| Independent fixed-query evidence probe | 4/4 | 4/4 |
| Successful correction extraction | 4/4 | 4/4 |
| Successful invalidation | 4/4 | 4/4 |
| Evidence returned before invalidation | 4/4 | 4/4 |
| Unsupported / prohibited source uses | 0 / 0 | 0 / 0 |
| Hard violations / timeout or resource exits | 0 / 0 | 0 / 0 |
| Recall latency, mean / median | 17.866 / 17.284 s | 17.108 / 16.506 s |
| Recall input / output tokens | 117,418 / 776 | 129,037 / 723 |
| Maintenance input / output tokens | 230,449 / 1,326 | 231,317 / 1,346 |
| Maintenance wall time, total | 112.416 s | 113.239 s |

Every formation stage completed. Each trial performed one successful
`extract` and one successful `invalidate`; the content-free audit observed
the persisted statement/quote response before invalidation. The starting
fixture contained exactly one stale fact. Each independent post-write probe
returned only that trial's corrected event, passed its answer oracle, and
reported no unsupported or prohibited source. Thus the closed fixture and
post-write probes support that the old fact was superseded in each trial.
The receipts deliberately do not preserve the raw tool arguments, statement
or quote.

The probe results show that the corrected evidence was available from memory
in all eight arm/trial stores. The agent did not produce an accepted value and
citation in one incumbent trial and two candidate trials. The paired task and
direct-memory risk-difference estimate was -0.25 (candidate minus incumbent),
with a deterministic family-cluster 95% interval of [-0.75, 0.00] across four
families. This is imprecise and descriptive. It cannot establish that either
arm is better; both arms ran the same product.

Candidate recall was 758.5 ms faster on average, but the paired latency
interval was [-2,367.75, 2,699.75] ms and crosses zero. Latency improvement is
not established. Maintenance formation consumed 12 agent tool calls in each
arm. There is no evidence here that the memory product itself improved.

### Read-capture limitation

For all agent reads, the receipt's `read_diagnostics` retain request/response
sequence associations but report `outcome=unknown` and no item count or
completeness. The separate `direct_memory_success` measure is still scored
from the source event IDs observed in the agent trace against the frozen
oracle; it is not inferred from that `unknown` diagnostic. Independent probes
give a deterministic view of stored memory, but this capture cannot show the
full content/completeness of the context received by the agent. Keep the
reader-completeness instrumentation gap open for a separate harness fix.

## Execution and privacy

The runner and memory server ran locally on native Apple Silicon. Codex CLI
0.171.0 used the `gpt-6.1-sol` remote model through the user's existing local
Codex login; every case and value sent to it was synthetic. Codex's own agent
sandbox was `read-only`. No LM Studio runtime participated. The first setup
attempt in the sibling `c1-agent-managed-correction-codex-2026-11-25/`
directory exited before a model turn in all receipts (zero turns, calls and
tokens); it is retained as an invalid setup attempt and uses disjoint data
from this cohort.

The input freeze manifest passed before capture and remains valid. The offline
comparison was replayed and its output matched byte-for-byte. Output hashes
are in `capture-01/SHA256SUMS`.

## Reproduction

Run from the repository root on Apple Silicon with the same Codex CLI version
and an authenticated local Codex home. This invokes remote model inference on
the frozen synthetic tasks:

```sh
target/debug/memory-eval run-paired-codex \
  --plan "$PWD/evals/paired/c1-agent-managed-correction-codex-direct-mlx-2026-11-25/plan.json" \
  --tasks "$PWD/evals/paired/c1-agent-managed-correction-codex-direct-mlx-2026-11-25/tasks.json" \
  --codex-binary /opt/homebrew/bin/codex \
  --codex-home /Users/solovey/.codex \
  --memory-tool-profile agent_maintenance_v4 \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --output-directory "$PWD/evals/paired/c1-agent-managed-correction-codex-direct-mlx-2026-11-25/reproduction-capture"
```

The offline deterministic comparison needs no model call:

```sh
target/debug/memory-eval compare-agent-managed \
  --plan "$PWD/evals/paired/c1-agent-managed-correction-codex-direct-mlx-2026-11-25/plan.json" \
  --incumbent "$PWD/evals/paired/c1-agent-managed-correction-codex-direct-mlx-2026-11-25/capture-01/incumbent.json" \
  --candidate "$PWD/evals/paired/c1-agent-managed-correction-codex-direct-mlx-2026-11-25/capture-01/candidate.json" \
  --incumbent-stages "$PWD/evals/paired/c1-agent-managed-correction-codex-direct-mlx-2026-11-25/capture-01/agent-managed-incumbent.json" \
  --candidate-stages "$PWD/evals/paired/c1-agent-managed-correction-codex-direct-mlx-2026-11-25/capture-01/agent-managed-candidate.json" \
  --output /tmp/c1-agent-managed-correction-comparison.json
```
