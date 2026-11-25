# A8 evaluator-only evidence-action hint

## Decision

**NO-GO for promotion; no product response change.** On this three-family
screen, the hint did not increase exact task success: both arms passed Opal and
Quartz and failed Helios (2/3 each). There was no candidate-only task success,
so the preregistered stop rule did not authorize a disjoint follow-up. The
hint remains an evaluator-only option; no product wire contract was changed.

This is a small, model-mediated workflow screen, not a powered estimate or a
memory treatment effect. The fixed public memory replies and their source
coverage were matched. The candidate added only the registered hint after the
validated public reply.

## Frozen setup

- Three synthetic families, one paired trial per family; all failures retained.
- Native Apple Silicon, same memory binary and same product configuration in
  both arms.
- Local OpenAI-compatible MLX endpoint `http://127.0.0.1:18767/v1`, exact
  model id pinned in `plan.json`; the runner made no requests to LM Studio's
  endpoint.
- Temperature 0.0, one memory call, three turns, 384 output tokens, 4,096
  context tokens and 90 seconds per trial.
- The evaluator plan, tasks, six fixed reads, sampling settings and static
  preflight are hashed in `SHA256SUMS`. A no-model invocation with a deliberately
  wrong endpoint hash verified the registration and stopped before capture.
  Two path-only setup attempts also stopped before model requests; the final
  command used absolute binary and output paths.
- The static public-MCP preflight returned `ledger` for Opal, `ledger` and
  `note` for Quartz, and no item for Helios. The first two returned no typed
  `evidence_status` or reconciliation relation. The candidate therefore
  correctly fell back to `verify_or_ask` for those nonempty replies; it had no
  typed basis to label Quartz as a conflict.

## Results

| Family | Current | Evidence-action hint | Returned memory evidence | Paired time, current → hint |
|---|---|---|---|---:|
| Opal support | Exact answer, cited `ledger`, no clarification; pass | Same; pass | `ledger` in both | 14.026s → 19.750s |
| Quartz conflict | Exact conflict answer, cited both sources, clarification; pass | Same; pass | `ledger` + `note` in both | 15.490s → 15.941s |
| Helios absent | Model failure (`invalid_response`), no clarification; fail | Valid JSON but accepted answer contained extra text; clarification matched; fail | Empty in both | 54.731s → 10.583s |

| Aggregate | Current | Evidence-action hint |
|---|---:|---:|
| Exact task success | 2/3 | 2/3 |
| Expected source coverage | 3/3 | 3/3 |
| Clarification matched | 2/3 | 3/3 |
| Hard violations | 0 | 0 |
| Input / output tokens | 6,028 / 756 | 6,378 / 600 |
| Median trial wall time | 15.490s | 15.941s |

Candidate wall time was lower only on Helios, where the incumbent exhausted
its output budget and returned an invalid response while the candidate
completed with an incorrect answer. The paired median time delta was 451ms
slower for the candidate. Do not interpret the large mean difference as a
latency gain. The three-family bootstrap in `capture-01/comparison.json` is
descriptive only.

## Artifacts and reproduction

- [Preregistration](preregistration.md)
- [Plan](plan.json), [task manifest](tasks.json), [fixed-read registry](fixed-read-inputs.json)
- [Static preflight](preflight.json)
- [Paired capture and offline comparison](capture-01/); output hashes are
  recorded in [`capture-01/SHA256SUMS`](capture-01/SHA256SUMS)

The paired artifact is replayable offline with `compare-paired-diagnostic`;
that command returned three paired trials and three independent families with
no promotion decision. The model-backed capture is not deterministic. The
acceptance rules, replay and artifact checks are deterministic.
