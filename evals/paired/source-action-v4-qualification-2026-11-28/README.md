# Source-action v4: sole local development qualification

Decision: **PASS for the six current development scenarios**, not general agent-memory benefit.

| Metric | Previous | Current |
| --- | ---: | ---: |
| Scheduled / answered | 6 / 6 | 6 / 6 |
| Actual operations and exact arguments | 6 / 6 | 6 / 6 |
| Full source/status/time/quote-correct final | 4 / 6 | 6 / 6 |
| Lost previously passing scenario | — | 0 |

Fixed denominator12;54 study request starts and54 successful terminals, zero retries/faults. Capture431.674s; owned runtime including startup/cleanup444.589s. Native offline Apple Silicon MLX; local weight files only, no LM Studio service or remote inference. Whole-capture timing is descriptive, not a paired speed estimate.

The two previous timed answers falsely label canonical facts episode fallback. The current answers preserve actual snapshot and fact/quote distinctions. All12 final answers were manually checked clause-by-clause against actual native receipts by the primary, non-blinded reviewer. Exact prior extraction decisions and IDs match; displayed ingestion timestamps are rounded to seconds. This is linked manual adjudication, not an automatic entailment oracle or independent semantic replication.

The plan, source review, inputs, skills, binary/model/rubric and strict all6-current/no-loss gate were pinned before inference. These are newly authored Sable/Tern/Cinder development sources. Both arms use the new v4 evaluator; previous skills come from the captured44 current baseline. Multiple runtime/instruction differences are bundled: do not attribute the difference causally to one rule. No independent history-family qualification or confirmatory power is supplied.

Replay without model/server/native binary:

```sh
python3 -B evals/paired/source-action-v4-qualification-2026-11-28/replay_capture.py
```

It replays12 operation grades and their hash-bound primary manual judgments and verifies all54 request lifecycles using archived implementations. [Decision](decision.json), [manual source review](source-review.json), [pre-inference review](pre-inference-source-review.json), [acceptance](acceptance.json), [replay receipt](replay-verification.json).

The old v3 full6/12 FAIL and whole-item strict49/56 FAIL remain unchanged. Raw-only reader delivery is deterministic-tested but dynamically unqualified. Conditional full7 reader inference remains NOT_ACTIVATED. [C2 decision-v6](../benefit-decision-2026-11-27/decision-v6.json) remains NO_GO. No repeat-until-green and no further workflow inference in this task.

## Cost and next-study feasibility

[Descriptive cost](descriptive-cost-v1.json) is reproducible with `summarize_cost.py` without inference. Each arm used27 requests. Previous/current reported prompt tokens87429/89722, completion2183/2198; cached prompt tokens70351/76088. Trial wall sums219.365/212.179s are descriptive; cache/order/source/skill differences prevent a causal speed conclusion. Summed prompt tokens count repeated cached context, not a billing estimate.

All12 observed whole trials exceed20s; the current per-request20s setting never established a20s end-to-end task. At the present180s full-trial cap plus prospective30s formation, a24-family/two-arm worst-case pilot alone is10080s before setup/export, beyond the prospective7200s study envelope. This is a planning bound, not expected latency or a new C2 preregistration. Qualification must establish a feasible full-trial/resource design before any benefit pilot; do not silently turn request deadlines into task deadlines or exclude slow failures.

Whole-execution accounting:54 study inference requests plus one separately retained startup inference request =55 observed requests. The startup response,1.179s and reported15 prompt/8 completion tokens are in `runtime-preflight.json`; it is not one of the12 scored trials. Health probes are not inference. Model scores and54 study lifecycle records remain unchanged.
