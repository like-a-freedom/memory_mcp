# Perf harness runbook

How to run, read, and update the perf evaluation introduced by
[ADR-0082](../adr/0082-perf-evaluation-surface-in-eval-harness.md). One-line
summary: `memory-perf` runs committed profiles against an embedded
in-process stack, writes one artifact per run, and compares gates against a
committed baseline of the same host identity.

## Commands

```bash
make perf-smoke    # PR integrity profile (gates: []), any host; CI runs this
make perf-weekly   # weekly gates vs evals/perf/baselines/perf-weekly-local.json
```

Exit codes, identical to the quality runner: `0` passed, `1` regressed, `2`
invalid evidence (budget exhausted, missing samples, unreadable baseline,
host identity or platform mismatch, profile digest mismatch). The artifact
is always written to `target/perf/`, even for invalid runs.

`perf-weekly` runs `--release`: the weekly gates measure the shipped
product surface, and the dev profile would gate on unoptimized code.
Baselines must therefore be recorded with the same `--release` build.
`perf-smoke` stays on the dev profile — it is an execution-integrity step,
not a latency measurement.

Direct invocation (what `make perf-weekly` wraps):

```bash
cargo run --release -p eval-harness --bin memory-perf --locked -- run \
  --profile evals/perf/profiles/perf-weekly.json \
  --artifact target/perf/perf-weekly.json \
  --baseline evals/perf/baselines/perf-weekly-local.json \
  --host-tag local
```

## Where weekly runs happen

Locally, on the maintainer's machine. There is no CI job and no dedicated
perf machine (decision 2026-10-10, recorded in ADR-0082): absolute latency
is never measured on GitHub-hosted runners, and nothing waits for a
self-hosted one. `ci.yml` only runs the gate-free `perf-smoke` profile as an
execution-integrity step.

## Host-tag policy

`host_tag` names the machine a baseline belongs to. It is operator-asserted
— the harness cannot measure machine identity — so three checks layer on
top of it:

1. The CLI rejects `--host-tag` values that differ from the profile's
   `host_tag` (exit 2, before anything runs).
2. `verdict::evaluate` rejects an artifact whose `host_tag` differs from
   the baseline's (`host_tag mismatch`).
3. The same comparison rejects a differing measured `os`/`arch` fingerprint
   (`host platform mismatch`) — a tag can lie, a fingerprint cannot.
4. A differing `profile_digest` is rejected (`profile digest mismatch`):
   a baseline recorded by another profile version may gate on metrics the
   current profile never defined, so any profile edit requires an explicit
   re-baseline.

The single-machine setup uses the tag `local` everywhere. If the baseline
ever moves to another machine, record a distinct tag there and retire the
old baseline; never compare across tags.

## Updating the baseline

A baseline update is explicit and separate from a gate run — `make perf-weekly`
never writes it. To record a new baseline:

1. Make sure the tree is clean and the reason is understood (new hardware,
   an intentional performance change, a harness change). A gate run that
   regressed is not a reason to update; it is a reason to investigate.
2. Run:

   ```bash
   cargo run --release -p eval-harness --bin memory-perf --locked -- run \
     --profile evals/perf/profiles/perf-weekly.json \
     --artifact target/perf/weekly.json \
     --host-tag local \
     --update-baseline evals/perf/baselines/perf-weekly-local.json
   ```

   The copy happens only when the verdict is `Passed`; any invalidity
   aborts with exit 2 and leaves the old baseline in place. A profile edit
   (for example a scenario change) changes the profile digest and the next
   gate run exits 2 until the baseline is re-recorded — that is the
   intended fail-closed flow, not an obstacle.
3. Write a dated rationale following the `docs/evals/BASELINE_REVIEW_*`
   pattern: what changed, why the numbers moved, which runs are expected to
   differ from the retired baseline.
4. Commit the new baseline together with a dated copy of the full artifact
   under `evals/perf/results/YYYY-MM-DD/` and a row in
   `evals/perf/results/README.md`.

## Reading an artifact

One JSON file per run. Top level: `schema`, `profile`, `profileDigest`,
`commit`, `host` (`hostTag`, `os`, `arch`), `startedAt`, `durationMs`, and
`scenarios` keyed `"{scenario}-{scale}"` (for example `ingest-scale-100`).

Per scenario record:

- `latencies` — nearest-rank percentiles in microseconds (`p50_us` …
  `p99_us`, `mean_us`, `total_us`, `count`). Warm-up iterations are
  discarded before this is computed; `null` latencies mean an invalid
  scenario.
- `throughputEps` — episodes per second, ingest/assemble scenarios only.
- `resources` — RSS delta and peak RSS of the run, plus disk bytes and I/O
  ops. Convention: read both RSS numbers — `rssBytes` (net growth) and
  `peakRssBytes` — never one alone. On macOS the I/O op counters are
  unavailable (`null`, documented ADR-0082); with the embedded `mem://`
  engine disk bytes are legitimately zero.
- `sql` — the SQL catalog: one record per distinct normalized template
  (`templateHash`, `op`, `class`, `count`, `stats`), plus one `(overflow)`
  row if the 64-template cap was hit. Classes are the closed vocabulary in
  `crates/eval-harness/src/perf/db_observer.rs`.
- `invalidReasons` — must be empty for a healthy run; the runner merges
  these into an `Invalid` verdict.

Known gaps and measurement notes:

- The committed weekly profile (ingest + assemble) records `EpisodeWrite`,
  `ProvenanceRead`, `AliasLookup` and `Other` only. `AnnSearch` needs an
  embedding provider and `FactWrite` needs an extract scenario — both
  deferred to plan 2 of the perf design; the catalog is not expected to
  contain them yet.
- `assemble-scale` is a **cold-retrieval** measurement: the corpus is
  fact-producing and extracted in setup, and each assemble call uses a
  unique query so the context cache (keyed by query) cannot answer. A
  repeated-query variant would record cache-hit latencies of tens of
  microseconds while claiming to gate the retrieval pipeline.
- The SQL observer's bookkeeping (normalize + hash + classify, a few
  microseconds per statement) runs inside the scenario-timed span. It is a
  constant factor per statement and self-normalizes for regression
  detection; it is not excluded from the measured latencies.

## Profile files

- `evals/perf/profiles/perf-pr-smoke.json` — integrity-only (`gates: []`),
  runs in CI and via `make perf-smoke`.
- `evals/perf/profiles/perf-weekly.json` — the gated weekly profile
  (ingest/assemble at scales 10 and 100, `p95_us` and `throughput_eps`
  gates, `max_ratio` 1.25). Pins `NER_EXTRACTOR=regex` because the
  assemble setup runs extraction; an ambient model-backed extractor would
  make weekly numbers depend on shell state and machine resources.

Profiles are schema-versioned (`memory-mcp-perf-profile/v1`) and validated
on load; gates must reference scenarios present in the same profile.
