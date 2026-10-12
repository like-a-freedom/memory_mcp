# Perf Evaluation Harness — Design (Core, In-Process)

> Status: approved 2026-10-10. Implements [ADR-0082](../../adr/0082-perf-evaluation-surface-in-eval-harness.md).
> Scope: phase 1+2 (in-process driver). Out of scope: stdio/HTTP subprocess
> drivers, `/metrics` scrape, `DECLARED_STAGES` extension, contention
> scenarios, deterministic embedding provider, RocksDB internals.

## Goal

A profile-driven perf run measures end-to-end capability latency, SQL
activity, and process resources of the real `MemoryService` over embedded
SurrealDB, and commits a comparable artifact with an honest verdict.

Module root: `crates/eval-harness/src/perf/`. One-way dependency
`eval-harness → memory_mcp` (ADR-0021). No new MCP tools (8-tool surface
frozen).

## Drivers

| Driver | Status |
|---|---|
| `in_process` | v1 — constructs `MemoryService` via the `test_support::TestMemory` pattern (`connect_in_memory_with_namespaces` + migrations + `MemoryService::new`, namespace always `main`) |
| `stdio`, `http` | declared in the closed `DriverKind` vocabulary for profile forward-compat; `profile::load` rejects them with `PerfError::UnsupportedDriver` until plan 2 |

## Sampling layer

`ResourceSampler` trait: `fn sample(&self) -> Option<ResourceSnapshot>`.

- `ResourceSnapshot` fields are all `Option<u64>`: `rss_bytes`,
  `peak_rss_bytes`, `disk_read_bytes`, `disk_write_bytes`, `io_read_ops`,
  `io_write_ops`.
- Linux: `/proc/self/status` (`VmRSS`, `VmSwap`, `VmHWM`, kB kept as parsed)
  and `/proc/self/io` (`read_bytes`, `write_bytes`, `syscr`, `syscw`).
- macOS: `libc::proc_pid_rusage(pid, RUSAGE_INFO_V2)` →
  `phys_footprint` (rss), `ri_resident_size` (peak), disk bytes from
  `ri_diskio_bytesread/written`; `io_*_ops` are `None` (no per-process op
  counters without IOKit — documented limitation, ADR-0082).
- Unsupported platform: sampler returns `None` for every field; the runner
  records an `invalid_reason` on the scenario. Zeros are never fabricated.

## SQL observer

Prod port behind `eval-support` (memory_mcp):

```rust
pub struct SqlObservation { pub op: &'static str, pub sql: String, pub micros: u64 }
pub trait QueryObserver: Send + Sync + 'static { fn observe(&self, obs: &SqlObservation); }
```

- Hooked at the two timed funnels (`execute_query`, `execute_raw_query`);
  successful executions only in v1; `server_version` outside the funnel.
- Cost when unset: one `OnceLock` read. No `request_id`/tenant/data in the
  observation (ADR-0048).

Harness side (`perf/db_observer.rs`):

- `normalize_sql`: literals → `?`, hex runs of 8+ characters (record ids
  like `episode:<sha256>`) → `#`, remaining digit runs → `#`, whitespace
  collapsed; literals first so digits inside them do not leak, hex runs
  before digits so content-addressed ids cannot fragment a template.
- Template cap `MAX_TEMPLATES = 64`; beyond, observations merge into a single
  `(overflow)` accumulator. Artifact key-space never grows with data.
- `QueryClass`: closed enum (`EpisodeWrite`, `FactWrite`, `EntityUpsert`,
  `EdgeProbe`, `RelateEdge`, `AnnSearch`, `AliasLookup`, `EdgeNeighbors`,
  `ActiveEdgeScan`, `CommunityScan`, `AccessLogWrite`, `ResolveLookup`,
  `ProvenanceRead`, `Migration`, `Other`), classified by op + markers in the
  lowercased template (`RULES: &[(QueryClass, &[&str])]`).
- `QueryLedger` (Mutex-protected) implements `QueryObserver`; `snapshot()`
  returns `Vec<SqlTemplateRecord>` via `reducer::latency_stats` per template.

## Metric catalog

All arithmetic and metric-key names live in `perf/reducer.rs` only:

- `latency_stats(&[u64]) -> Option<LatencyStats>` — nearest-rank percentiles
  (p50/p90/p95/p99), mean, total, count; empty slice → `None` (never 0).
- `throughput_eps(total_us, episodes) -> Option<f64>` — episodes/second;
  `total_us == 0` → `None`.
- `scaling_exponent(&[(f64, f64)]) -> Option<f64>` — log2-log2 least squares,
  <3 points → `None`.
- `ratio(current, baseline) -> Option<f64>` — `baseline <= 0` → `None`.
- Keys: `latency_key(scenario, metric) -> "{scenario}.{metric}"`,
  `sql_key(class) -> "sql.{class}"`.

Gate metrics (closed): `p50_us`, `p95_us`, `p99_us`, `mean_us`,
`throughput_eps`.

## Profiles

Profile schema `memory-mcp-perf-profile/v1`; files under
`evals/perf/profiles/*.json`.

- `PerfProfile`: `name`, `budget_seconds`, `host_tag`, `env`
  (BTreeMap applied via `set_var` before `PerfEnv::connect`), `scenarios`,
  `gates`.
- `ScenarioSpec`: `id` (closed `ScenarioId`), `driver` (must be
  `in_process`), `scales` (non-empty, strictly increasing), `iterations > 0`,
  `warmup`, `queries` (Option, assemble-only).
- `GateSpec`: `scenario`, `metric`, `max_ratio`, `min_samples`, `direction`
  (serde default `lower_is_better`; `higher_is_better` inverts comparison in
  `verdict.rs`, not in the reducer).

Scenarios (one iteration = one `ScenarioRun::run` call; the runner drives
warmup with discard, then `iterations` with recording):

| ScenarioId | Behavior |
|---|---|
| `ingest_scale` | N ingests with unique ids `perf-{scenario}-{scale}-{iteration}-{i}` |
| `assemble_scale` | ingest N, then `queries` `assemble_context` calls on deterministic questions |
| `latency_extract`, `latency_resolve`, `latency_explain`, `latency_invalidate` | latency of one capability call over a fixed N=10 corpus fixture, fixed `t_ref` |

Runner semantics: per `(scenario, scale)` resource snapshots bracket the
scenario; ledger delta snapshot once per scenario; whole run wrapped in
`budget_seconds` timeout — budget exhaustion still writes the artifact and
yields `Invalid` with a reason containing `"budget"`. Scenario order = profile
order (determinism).

## Artifact

`memory-mcp-perf/v1`, camelCase, 2-space indent, stable bytes (BTreeMap
ordering), `profile_digest` = sha256 hex of the profile file. `read` rejects
any other schema with `PerfError::Schema`.

## Verdicts

`Passed` / `Regressed(Vec<String>)` / `Invalid(Vec<String>)`; exit 0/1/2.
With a baseline: host-tag mismatch or measured os/arch mismatch → Invalid;
per gate — sample count <
`min_samples` → Invalid; baseline metric missing → Invalid; ratio beyond
`max_ratio` → Regressed (direction-aware). Without baseline: gates skipped
(first run defines the baseline; `--update-baseline` copies the artifact).

## CI / Make

- `make perf-smoke` (PR): `perf-pr-smoke.json`, integrity-only, any host;
  artifact uploaded `if: always()`.
- Weekly: `make perf-weekly` on the maintainer's local machine — no CI job
  and no dedicated perf host (ADR-0082): gates against the committed
  `evals/perf/baselines/perf-weekly-local.json`.

## Risks

- Embedded `mem://` understates disk I/O vs RocksDB — acceptable: the
  artifact records what the profile ran; plan 2 adds RocksDB scenarios.
- macOS lacks I/O op counters — `None`, documented in the runbook.
- Dedup contamination: unique ids per iteration are mandatory; enforced by a
  scenario test that counts episodes == total calls.
- Env-var knobs mutate process state before connect; applied single-threaded
  with a harness-owned `Mutex` + SAFETY comment (edition 2024 `set_var` is
  unsafe).
