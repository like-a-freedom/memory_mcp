# C1 operator-import local qualification screen

## Decision

**Not qualified; stop after capture 01.** The preregistered minimum was 3/4
exact task successes in each arm. Both arms scored 0/4, so a second capture was
not run. This screen is a workflow qualification result on four synthetic
families; it is not an estimate of a memory treatment effect or a population
success rate. Both arms used the same product binary and configuration.

## Frozen-input check

Before sending any task request, the preregistered freeze manifest passed and a
deterministic UTF-8 check verified for all four families that:

- the source sentence in `input-equivalence.json` is byte-identical to the
  candidate operator-import file content;
- the same sentence occurs verbatim in the incumbent formation prompt; and
- both recorded SHA-256 values equal the digest of those exact bytes.

This corrects the source-input mismatch in the historical v6 cohort. The v6
results remain historical and were not reused as holdouts. The check proves
equivalence of the registered inputs; it does not assert that an agent's
memory write itself was byte-identical. Raw model text and source contents are
not retained in the run receipts.

## Results

| Measure | Incumbent: literal ingest | Candidate: operator import |
| --- | ---: | ---: |
| Exact task success | 0/4 | 0/4 |
| Agent-observed direct memory success | 4/4 | 4/4 |
| Independent public evidence probe | 4/4 | 4/4 |
| Ingest/extract success or import/extract success | 4/4 | 4/4 |
| Unsupported / prohibited sources | 0 / 0 | 0 / 0 |
| Hard violations | 0 | 0 |
| Agent task wall time, mean | 36.753 s | 36.971 s |
| Agent task wall time, median | 36.292 s | 36.523 s |
| Agent task wall-time range | 31.784–42.643 s | 34.452–40.385 s |

The paired comparison's `wall_time_gain_ms` is incumbent time minus candidate
time: -218 ms on average, with a deterministic 95% interval of -1,874 to
1,451 ms across four families. It crosses zero and is inconclusive. It
describes the paired task receipt; the separately measured formation-stage
cost below is the relevant workflow-effort signal.

The independent probes show that both memory stores contained retrievable,
oracle-accepted evidence on all four families. The agent task metric remained
0/4 because no exact final answer satisfied the oracle. All incumbent paired
task receipts ended at the eight-turn budget. Candidate receipts had two
budget exits, one model failure and one completed interaction with an answer
that did not state an accepted value. In the managed formation phase,
incumbent status was 2 completed / 2 model failures; candidate status was
1 completed / 3 budget exits. There were no hard safety violations.

| Formation/setup measure | Incumbent | Candidate |
| --- | ---: | ---: |
| Formation-stage agent tool calls | 8 | 31 |
| Formation-stage agent wall time | 82.978 s | 168.368 s |
| Operator import + extraction successes | n/a | 4/4 |
| Operator setup actions / failures | n/a | 8 / 0 |
| Operator setup wall time | n/a | 1.836 s total |

The candidate spent substantially more agent effort rereading already
available evidence. In the first completed candidate receipt, all eight tool
calls were successful `assemble_context` reads with one returned item, but no
final answer was produced before the budget. Across the cohort, every
independent probe passed, so this is evidence of an agent-workflow failure,
not a demonstrated memory retrieval failure. The same-binary design cannot
show that operator import improves memory quality.

Read receipts marked completeness as `unknown`; therefore the nonempty reads
must not be interpreted as proof that a model-visible snapshot was complete.
The independent public probes separately establish that the registered
answer evidence was retrievable.

## Reproduction

All task execution was on native Apple Silicon through the provider-neutral
`run-paired-chat-completions` runner and direct local `mlx-lm.server` 0.32.2 at
numeric loopback `127.0.0.1:18768`. No LM Studio process, CLI or service was
used. The model files currently reside under a directory named
`.cache/lm-studio`; that path is storage provenance only. The preregistration
and freeze hashes identify the exact current run inputs.

The run created `capture-01/` once and completed all four task families per
arm. Reproduce the offline comparison without invoking a model:

```sh
target/debug/memory-eval compare-agent-managed \
  --plan "$PWD/evals/paired/c1-operator-import-qwen-direct-mlx-2026-11-25/plan.json" \
  --incumbent "$PWD/evals/paired/c1-operator-import-qwen-direct-mlx-2026-11-25/capture-01/incumbent.json" \
  --candidate "$PWD/evals/paired/c1-operator-import-qwen-direct-mlx-2026-11-25/capture-01/candidate.json" \
  --incumbent-stages "$PWD/evals/paired/c1-operator-import-qwen-direct-mlx-2026-11-25/capture-01/agent-managed-incumbent.json" \
  --candidate-stages "$PWD/evals/paired/c1-operator-import-qwen-direct-mlx-2026-11-25/capture-01/agent-managed-candidate.json" \
  --output /tmp/c1-operator-import-comparison.json
```

The comparator output in `capture-01/agent-managed-comparison.json` was
produced from these same frozen artifacts. `capture-01/SHA256SUMS` records
their content hashes. The pre-run `freeze-manifest.sha256` remains valid and
unchanged.
