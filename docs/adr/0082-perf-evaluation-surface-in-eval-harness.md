# ADR-0082: Perf evaluation surface lives in eval-harness

- Status: accepted — 2026-10-10
- Relates to: [ADR-0021](0021-eval-harness-isolated.md) (eval-harness is
  private and one-way dependent on `memory_mcp`), [ADR-0034](0034-allocator-and-accelerator-default-policy.md)
  (allocator/accelerator defaults; Criterion benches for micro-benchmarks),
  [ADR-0048](0048-bounded-runtime-observability.md) (bounded labels),
  [ADR-0064](0064-run-ci-on-cargo-only.md) (CI is cargo-only), truthful-eval
  (§5: invalid ≠ pass)

The product needs a committed, comparable record of how the memory engine
behaves under scale: latency and throughput of the six public capabilities
(`ingest`, `extract`, `assemble_context`, `explain`, `resolve`, `invalidate`),
the time and class of every SQL statement they issue, and the process's own
resource consumption. The Criterion benches in `eval-harness` measure isolated
micro-behavior of models and call shapes; they cannot answer "did this commit
regress the end-to-end capability path", and they write no artifact anyone can
diff in review.

## Decision

**A profile-driven perf surface is added to the private `eval-harness` crate,
driven by a separate binary `memory-perf`, emitting a versioned artifact
`memory-mcp-perf/v1` with a `passed / regressed / invalid` verdict.**

### Separate binary, non-Criterion runner

`memory-perf` (`crates/eval-harness/src/perf_main.rs`) is a `[[bin]]` of
`eval-harness` alongside `memory-eval`. The scenario runner is deliberately
**not** Criterion: Criterion remains the home of micro-benches (model
tokenization, embedding throughput — ADR-0034 survives this intentionally),
while perf scenarios are end-to-end capability calls against a real embedded
`MemoryService` (in-memory SurrealDB), whose duration is dominated by engine
behavior a statistical harness cannot meaningfully de-noise. The runner's
contract is warmup → fixed iterations → reduced statistics → committed
artifact, with formulas isolated in one module (`perf/reducer.rs`, the
perf-side analogue of ADR-0025's single-formula home).

### Schema strings

- Profile files: `"schema": "memory-mcp-perf-profile/v1"` (`PROFILE_SCHEMA`).
- Artifacts: `"schema": "memory-mcp-perf/v1"` (`ARTIFACT_SCHEMA`).

An unknown schema version is a hard error on load and an `Invalid` verdict on
baseline comparison. There is no "try as v1" fallback: a future v2 must be
read by a reader that knows v2.

### Host-identity gate policy

Absolute-value regression gates only apply between artifacts produced on the
same host tag. `host_tag != baseline.host_tag` → `Invalid("host_tag
mismatch")`, never a best-effort comparison; a differing measured `os`/`arch`
fingerprint is `Invalid("host platform mismatch")` (ADR-0034: PR runners may
check gross timeouts and integrity, never absolute latency). CI wiring: the PR
smoke profile runs integrity-only (`gates: []`); the weekly profile is
maintainer-invoked on the local machine and is never measured in CI —
absolute latency does not belong on shared GitHub-hosted runners, and there
is no dedicated perf machine.

### SQL observer as an eval-only seam

One seam in the single SQL funnel of `storage/client.rs`: the private
`execute_query` / `execute_raw_query` wrappers already time every statement
(migrations flow through them too). Under the `eval-support` feature a
`QueryObserver` port receives `(op, sql, micros)` for **successful
executions only** — errors are not recorded in v1; `server_version` probing is
deliberately outside the funnel and is never observed. When no observer is
installed the cost is one `OnceLock` read. Production builds do not enable
`eval-support`; the observation carries no `request_id`, tenant, or data —
only closed-vocabulary operation names and a normalized SQL template
(ADR-0048 bounded labels).

### Resource sampling and the `libc` decision

`ResourceSampler` reports RSS, peak RSS, disk read/write bytes, and I/O ops.
Linux: parsed from `/proc/self/status` and `/proc/self/io` — no dependency.
macOS: `proc_pid_rusage(RUSAGE_INFO_V2)` is the only per-process source of
`ri_diskio_bytesread/written` and `phys_footprint`; every maintained Rust
option (`sysinfo`, `prock`) calls the same C API under the hood, and the
RSS-only crates (`memory-stats`, `proc-memstat`) lack disk counters. The
minimal correct binding is one target-gated dependency:

```toml
[target.'cfg(target_os = "macos")'.dependencies]
libc = "0.2"
```

It lives only in `eval-harness` (`publish = false`); `memory_mcp` and
`memory_mcp_http` never depend on `eval-harness`, so production binaries never
link it. Maintainer approved 2026-10-10 after the alternatives review. If a
future platform lacks a sampler, the snapshot fields are `None` and the
scenario gains an `invalid_reason` — zeros are never written as fake data
(truthful-eval §5: invalid ≠ pass).

### Verdicts and exit codes

Semantics mirror the quality runner: the artifact is always written; the exit
code comes from the verdict — `passed → 0`, `regressed → 1`, `invalid → 2`.
Empty sample sets, a required-but-unavailable sampler, and host/schema
mismatches are `Invalid`, never an optimistic default.

### Local weekly workflow (no dedicated machine)

`make perf-smoke` runs the `perf-pr-smoke` profile with host tag
`MEMORY_PERF_HOST_TAG` (default `local`) and is the `quality` job's perf step
in `ci.yml`; the workflow uploads `target/perf/*.json` with `if: always()`.
The value must equal the profile's `host_tag` — the CLI rejects a mismatch
with exit 2, so overriding it to some other host is not a supported use.

The weekly profile has no CI job: there is no dedicated perf machine
(decision 2026-10-10), so absolute latency is never measured in CI at all.
It is invoked by the maintainer on the same local machine that recorded the
baseline: `make perf-weekly` runs `evals/perf/profiles/perf-weekly.json`
with `--host-tag local` against the committed
`evals/perf/baselines/perf-weekly-local.json` and writes
`target/perf/perf-weekly.json`. Updating the baseline is a separate,
explicit `--update-baseline` invocation documented in the runbook
(`docs/performance/PERF_HARNESS.md`) and never a side effect of a gate run.

`host_tag` is operator-asserted — the harness cannot measure the machine's
identity — so the control is layered: the CLI rejects a `--host-tag` that
does not match the profile's `host_tag` (a run cannot claim a machine the
profile was not written for), and a `host_tag` or measured `os`/`arch`
difference between artifact and baseline turns every gate `Invalid`. The
tag is `local` in the single-machine setup; if the baseline ever moves to
another machine, that machine records a distinct tag and the old baseline
is retired, not compared across tags.
