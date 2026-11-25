# C1 typed-conflict local screen

## Result

**Not qualified.** The deterministic memory probes passed on all four fresh
synthetic families, but the first preregistered agent capture scored 0/4 exact
task outcomes in both arms. The frozen stop rule therefore forbade a second
capture. This is a workflow failure for this exact agent/model/prompt setup,
not evidence that the memory product regressed: both arms used the same binary,
same profile and same task data.

## Deterministic evidence

The public MCP preflight returned one allowed source within the one-item budget
for each family. The independent scorer verified the typed
`evidence_status=conflicting` claim and contradiction/conflict relation for
each source. The paired run repeated the independent probes for each arm:
incumbent 4/4 and candidate 4/4. Direct agent reads also returned an allowed
source once per trial: 4/4 in each arm, with source coverage true and zero hard
violations.

| Measure | Incumbent | Candidate |
| --- | ---: | ---: |
| Exact task success | 0/4 | 0/4 |
| Direct memory read with expected source | 4/4 | 4/4 |
| Independent typed-evidence probe | 4/4 | 4/4 |
| Hard violations | 0 | 0 |
| Model failures | 4/4 | 4/4 |
| Median task wall time | 9.81 s | 9.77 s |
| Mean task wall time | 10.36 s | 9.78 s |

Each arm had two `final_answer_not_json` failures and two `unauthorized`
failures. Raw model text is not retained by the runner; the receipts retain
normalized failure classes and payload hashes. Every direct read was counted
as successful by the source-coverage gate, but the receipt reported
`completeness=unknown`; that diagnostic requires a separate deterministic
contract check before it can be interpreted as a memory result.

## Gate and interpretation

The frozen minimum is 3/4 exact task successes, 3/4 actual memory successes,
and 4/4 probes. Both arms fail the task-success floor, so capture 02 was not
run. Do not pool this cohort with the ordinary v3 positive control: the
workflow stratum is different. The equal arms also cannot estimate a
candidate-versus-incumbent treatment effect.

This screen supports a narrow finding: on these four histories and this exact
Qwen/direct-MLX/runner configuration, current memory returned the expected
source and typed conflict, while the agent failed the final-response contract
after every read. It does not establish a population rate or a general agent
usability claim. The ordinary v3 positive control remains eligible only for
its separately registered ordinary-recall workflow.

## Frozen inputs and receipts

- `freeze-manifest.sha256` verifies the corrected pre-capture registration.
- `preflight-initial-freeze-manifest.sha256` preserves the superseded initial
  registration, which had a stale tool-schema digest.
- `probe-preflight/summary.json` and its per-family captures/scores show the
  four deterministic prerequisite results.
- `capture-01/` contains all eight arm/task receipts and paired artifacts.
- `runner-preflight-failure/attempt-01-no-provider-call/` records a prior
  runner validation refusal; no model task request was sent in that attempt.
- The exact model file manifest and task definitions are in this directory.

All eval execution was local on native Apple Silicon against direct MLX-LM
0.32.2 at loopback. No LM Studio process, CLI or service was used. The model
directory happens to reside under a `.cache/lm-studio` path; that path is only
where the weights were stored. All task content is synthetic.
