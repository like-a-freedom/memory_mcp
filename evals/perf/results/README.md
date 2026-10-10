# Perf results archive

Dated, committed copies of weekly perf artifacts. Each directory is one
recorded run of `evals/perf/profiles/perf-weekly.json` on the machine that
owns the committed baseline — the artifact is committed whole (v1 does not
cap the sample arrays). The machine-readable baseline that gates compare
against lives in `evals/perf/baselines/`; a results directory is history,
never a gate input.

| Directory | Artifact | Verdict at commit time |
|---|---|---|
| `2026-10-10/` | `weekly.json` | Passed — first baseline (`../baselines/perf-weekly-local.json`) |

How to record another run: `make perf-weekly`, then copy
`target/perf/perf-weekly.json` into a new dated directory and add a row.
Baseline updates are a separate, explicit operation — see
[`docs/performance/PERF_HARNESS.md`](../../../docs/performance/PERF_HARNESS.md).
