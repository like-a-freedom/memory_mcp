# Primary-only development capture — 2026-11-26

> **Scope correction, 2026-11-26:** The three v9 failures below are preserved
> historical evaluator results, not established product defects. V9 incorrectly
> requires temporal `current` roles from ordinary reads. ADR-0083 and v10 already
> prohibit that requirement. The active gate now uses v10 and explicitly rejects
> the incompatible contract; see [current evidence](../current-contract-baseline-2026-11-26/README.md).


Local native Apple Silicon; no model host or remote runner. Product source and
binary are identical to the frozen baseline; every raw receipt pins that identity.
The baseline ran 24 scenario variants; this capture ran 8. The independent review accepted the baseline; the final repaired gate is now active.
The original development capture below is preserved unchanged.

| Observation | Baseline with diagnostics | Primary only |
| --- | ---: | ---: |
| Wall time (ms) | 172603 | 54805 |
| Primary evidence | 8/8 | 8/8 |
| Primary decisions | 5/8 | 5/8 |
| Temporal decisions | 8/8 | 8/8 |
| Invalid cases | 0 | 0 |

Offline raw-receipt regrading finds no per-case/per-dimension regressions and
keeps all three failures: `office-entry`, `similar-name`, `single-current`, each
with `decision_unobservable`. All-cases-pass is false. These are finite exposed
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

## Final gate and review

The [independent review](independent-review.md) accepted the frozen baseline and
identified five Important findings. All five were fixed with deterministic controls:
missing/changed repeated reads; primary and diagnostic formation/runtime binding;
owned deadline cancellation and partial outcomes; typed execution timeouts; exact
grade provenance and preservation of failed paired grading attempts. No Critical
finding was reported. Phase-sidecar granularity and the scope of time budgets are
explicitly documented; no crash-safe per-call or total-CLI deadline claim is made.

The normal native gate passed on Apple Silicon on 2026-11-26 in 58.17 seconds.
Its [current regression report](native-gate/current-regression.json) binds the
actual capture and grader. The [raw index](native-gate/current/index.json) and all
raw receipts are retained. Capture duration: 54,470 ms; 8 primary scenario variants;
5 passed, 3 quality_failed, 0 invalid/unobserved; 0 new regressions. The existing
product executable has the baseline SHA-256. Absolute input paths in the original
grade report describe the observed local run; use the archived index for offline
regrading with a fresh output file. No fresh agent or external-benchmark run was needed.

Retained contract coverage includes repeated reads and raw-negative controls,
source order, compact/verbose public evidence, temporal exclusions, isolated stdio
stores, Python forward/reverse reads, default configuration and explicit rollout
selection, and real public disclosure/withholding behavior. These checks replace
routine redundant capture arms; they do not establish cross-run reliability.

The final [offline grade attempt 02](native-gate/current-regression-02.json) regrades
the same raw native capture with the final compiled verifier after removing its
redundant launch check. Its scorer/normalizer/registration/verification/comparison
digests were matched to the committed source. The original gate grade remains
unchanged; no memory process was executed for this additional grading attempt.
Both reports retain the same 3 failures and 0 regressions.
