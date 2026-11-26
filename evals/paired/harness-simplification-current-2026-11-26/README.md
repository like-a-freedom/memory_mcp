# Primary-only development capture — 2026-11-26

Local native Apple Silicon; no model host or remote runner. Product source and
binary are identical to the frozen baseline; every raw receipt pins that identity.
The baseline ran 24 scenario variants; this capture ran 8. Independent baseline
review and the active gate switch are still pending.

| Observation | Baseline with diagnostics | Primary only |
| --- | ---: | ---: |
| Wall time (ms) | 172603 | 54805 |
| Primary evidence | 8/8 | 8/8 |
| Primary decisions | 5/8 | 5/8 |
| Temporal decisions | 8/8 | 8/8 |
| Invalid cases | 0 | 0 |

Offline raw-receipt regrading finds no per-case/per-dimension regressions and
keeps all three failures: `office-entry`, `similar-name`, `single-current`, each
with `decisionunobservable`. All-cases-pass is false. These are finite exposed
synthetic fixtures, not agent utility, semantic extraction, or benchmark claims.

Primary phase sums: startup 52.651s, formation 1.301s, query 0.383s, diagnostics
0.117s, shutdown 0.193s, preparation 0.001s. The small diagnostics phase here is
in-scenario protocol checking, not an extra rollout variant. All 8 stores remain
isolated. The expensive repeated server startup explains why removing redundant
variants helps this cohort; sharing mutable stores is not justified.

The observed elapsed reduction is from a single sequential before/after pair;
cache/order/system load are uncontrolled. The baseline predates stage timings,
and the exact dirty harness executable digest for the primary-only capture was
not recorded before that executable was rebuilt. Product/corpus/config/schema
identities and raw observations are retained; do not present this as a controlled
latency benchmark or a general speedup estimate.

`run.json` preserves the original capture, including an evaluator reporting bug:
unrequested evidence-stage diagnostics were represented as zero success rates.
A deterministic CLI regression reproduced that error; current reporting omits
unmeasured diagnostics. Primary and temporal scores were unaffected. The original
artifact is retained rather than rewritten. `comparison.json` is an offline
comparison, not another product capture. `stage-totals.json` sums recorded phases.

A bounded context-collection cost pilot copied the existing 5.1-GiB local
LongMemEval snapshot in 1.169s. The CLI then reached its explicit 15s timeout
(15.101s measured including cleanup); overall wall time was 16.327s. There was
no answer and no quality score. This diagnostic does not verify that snapshot's
full content identity or replace a benchmark run. It supplies no reason to
sacrifice isolation or add filesystem-specific clone logic: copying was a small
part of this bounded attempt, while the product process remained the bottleneck.
The untouched source snapshot was read through a disposable private copy.
