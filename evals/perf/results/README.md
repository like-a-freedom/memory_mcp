# Perf results archive

Dated, committed copies of weekly perf artifacts. Each directory is one
recorded run of `evals/perf/profiles/perf-weekly.json` on the machine that
owns the committed baseline — the artifact is committed whole (v1 does not
cap the sample arrays). The machine-readable baseline that gates compare
against lives in `evals/perf/baselines/`; a results directory is history,
never a gate input.

| Directory | Artifact | Verdict at commit time |
|---|---|---|
| `2026-10-10/` | `weekly.json` | Passed — release-build baseline (`../baselines/perf-weekly-local.json`); first record was the dev-build run, superseded the same day when weekly moved to `--release` and the assemble scenario started measuring cold retrieval |
| `2026-10-11/` | `weekly.json` | Passed — new baseline after the incremental community walk (extract's `EdgeNeighbors` setup statements ~150× fewer at scale 100, run 97s → 3.7s) and the catalog's setup/timed phase split |

How to record another run: `make perf-weekly`, then copy
`target/perf/perf-weekly.json` into a new dated directory and add a row.
Baseline updates are a separate, explicit operation — see
[`docs/performance/PERF_HARNESS.md`](../../../docs/performance/PERF_HARNESS.md).
