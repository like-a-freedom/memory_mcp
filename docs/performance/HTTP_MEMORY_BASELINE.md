# HTTP memory baseline protocol

Status: procedure and sampler contract; **no controlled baseline has been collected yet**. The staging safety tests pass, but no disposable staging deployment/database/provider was supplied. Production has not been seeded or sent workload requests; Task 2 attribution remains unresolved, so no RAM-reduction claim is supported.

## Sampler

Build/run the pinned Linux `xtask` binary against a verified staging PID:

```sh
cargo run -p xtask -- sample-memory \
  --pid <verified-staging-pid> \
  --duration-secs 5 \
  --interval-ms 250 \
  --output <dedicated-evidence-path>
```

`sample-memory` writes and flushes one JSON object per line. Fields are `elapsed_ms` from a single monotonic start, process `rss_kib`, `swap_kib`, `hwm_kib`, and nullable cgroup v2 `cgroup_current_bytes` / `cgroup_swap_bytes`. If the target has no visible cgroup v2 memory counters, it writes `null` for both cgroup fields and emits one warning to stderr. A missing process/status or unreadable available cgroup counter is an error; it is never recorded as zero. The sampler accepts 1–86400 seconds and a 1–60000 ms interval. Its output path is replaced, so use a dedicated evidence path.

The process values and cgroup values are distinct observations. Record `/proc` RSS+swap and cgroup current+swap separately; do not combine one denominator with the other. Record UTC timestamps and phase labels outside this sampler. The sampler intentionally records no command line, environment, host identity, request content, credentials, or logs.

## External exporter inventory

Status: **BLOCKED**. No secured, read-only VictoriaMetrics query path or operator-verified sanitized selectors are available in this execution. No `observability/external_metrics.yml` contract or exporter-backed panels have been created. Candidate metric names alone do not establish that a family or label is present in the VictoriaMetrics store; a direct exporter access failure does not prove absence from that store.

Keep measurement levels distinct: node_exporter is host-level, cAdvisor is container-level, and neither reports this process's `VmRSS`/`VmSwap` or Rust heap attribution. External dashboard acceptance remains blocked until stable, non-identifying selectors are confirmed. This does not itself block process-level acceptance, which still requires the controlled Linux baseline and allocation attribution below.

## Controlled comparison

Do not treat the 2026-10-07 production snapshots as a causal baseline: the deployed image has no verified source revision, traffic was not isolated, and no exact-image controlled workload was run. Do not seed a production tenant or issue destructive workload requests to production.

Use a disposable Linux staging deployment with a dedicated database/namespace, explicit staging acknowledgement, remote SurrealDB, and deterministic local/mock embedding provider. Seed and verify the same synthetic dataset for each fresh process: 24 episodes (each <=64 KiB), 129 facts (each <=4 KiB content), and 454 entities. Preserve dataset and configuration hashes, build commit, enabled features and allocator in the evidence metadata. Never save API keys, raw customer data, or unredacted HTTP bodies.

For baseline and candidate, run three fresh-process repetitions under the same 2 CPU limit and identical dataset/config/provider. Measure startup before tenant-ready, health-only, first activation, and first recall. Use five warm-up recall cycles (one identical and one unique recall each), then 20 measured recall cycles (10 identical and 10 unique recalls) against the unchanged dataset. After the recall phase, run exactly one small extraction in its own measured phase and record its added episode/fact counts separately; do not repeat extraction in every cycle. Measure a separate 8-request concurrency case, provider outage, maintenance, subscription eviction, and no-scrape metrics operation. Separate accepted requests from refusals. Use the same sampler and workload. Run allocator profiling only in a separate staging diagnostic build with symbols; correlate live retained objects and peak stacks, and do not infer allocator fragmentation from RSS, swap, or heap mappings alone.

Wait for workload requests and provider in-flight work to reach zero; inspect task, tenant runtime and lifecycle owners where observable. A timer alone does not prove quiescence. If a required owner has no drain signal, mark the idle sample **inconclusive** rather than passing the gate. Keep raw profile artifacts local and sanitized; commit only redacted summary metrics and references/hashes.

## HTTP owner-accounting telemetry

`memory_http_preflight_body_bytes` records only fully collected body bytes,
including malformed JSON; partial, refused, read-error and cancelled bodies are
not observations. The refusal counter uses the closed reasons
`request_capacity`, `aggregate_byte_capacity` and `body_limit`.

The HTTP exposition now provides bounded, no-label resource gauges refreshed by
the existing five-second upkeep worker: preflight reserved requests/bytes,
resident ready-or-draining runtime count, context/query cache-accounted bytes,
and admitted/running/retained-byte values from the process-wide background
embedding coordinator. Cache estimates are summed once per resident runtime;
the shared coordinator is read once, not once per tenant. The preflight byte
gauge is budget reservation, not a measurement of resident body memory.

These values are owner-level diagnostics only. They do not account for all Rust
heap allocations, allocator retention, SDK/database internals, process
`VmRSS`/`VmSwap`, or host/container memory. No controlled Linux A/B or allocation
attribution has been collected, so the gauges provide no evidence of RSS
reduction or target acceptance. Task 15 external exporter inventory remains
blocked as stated above; it is not replaced with zero-valued or guessed series.

## Task 18 acceptance status (2026-10-08)

The implementation and local regression gates do not substitute for the Linux
release measurements. No isolated staging deployment, database, or mock
provider was supplied, and no production workload was run. Consequently the
candidate has not passed or failed the memory targets; they remain unmeasured.

| Gate | Required evidence | Status |
| --- | --- | --- |
| Controlled baseline/candidate | Three fresh-process Linux runs per build under the same 2-CPU limit, dataset, configuration, and deterministic provider | **BLOCKED** — no disposable staging target and controlled dependencies supplied |
| Process footprint | Active `VmRSS+VmSwap` <=200 MiB; post-work warm idle <=100 MiB; `VmHWM` recorded separately; >=4x reduction only when baseline exceeds the applicable target | **NOT MEASURED** — sampler exists, but no controlled process samples were collected |
| Diagnostics overhead | Three paired INFO runs and three paired INFO-vs-DEBUG runs; footprint delta <=5 MiB, CPU/accepted-operation delta <=5%, p95 <=1.15x, constant app series count | **NOT MEASURED** — no paired staging runs or service-only CPU attribution |
| Allocation attribution | Staging profiler live allocations and peak stacks tied to the candidate build | **BLOCKED** — no staging profiler run; owner gauges do not identify allocation roots |
| Host/container dashboard context | Verified read-only VictoriaMetrics access and safe selectors for node_exporter/cAdvisor | **BLOCKED** — no verified query path or selectors; no guessed or zero-valued panels were added |
| Local shipping validation | Full `memory_mcp` tests in default and HTTP feature profiles; format, strict workspace Clippy, toolchain pin, observability checker | **PASSED** — `MEMORY_LOG_COLOR=never cargo test -p memory_mcp --locked`; `MEMORY_LOG_COLOR=never cargo test -p memory_mcp --features streamable-http,mcp-apps,test-fixtures --locked`; `cargo fmt --all --check`; workspace Clippy with `-D warnings`; both xtask checks. Strict Clippy reported no warnings. This is local code validation, not Linux memory acceptance. |
| Release decision | Evidence-backed choice between robustness-only release with RAM target unverified, or a focused owner fix based on allocation stacks | **OPEN** — the plan assigns this choice to the user; neither path is selected here |
| Image and rollback proposal | Immutable old/new image digests and staging-accepted effective configuration | **NOT PREPARED** — no release image was built or staged; no deployment or rollback was attempted |

The accepted-request latency gate remains p95 <=1.15x the controlled baseline;
10–20 MiB idle is a stretch target, not a release gate. Without the controlled
baseline, neither ratio can be evaluated. Owner diagnostics, dashboard panels,
local tests, or host/container series cannot establish process-level target
acceptance or a causal RSS reduction.

Production deployment, restart, config change, and rollback remain outside this
work. A release must not be described as having met its RAM target until the
blocked evidence is collected and the open release decision is made. The local
shipping checks above do not establish RSS, swap, allocation-attribution,
diagnostics-overhead, latency, or resource-plateau acceptance.

## Durable task artifact residual bound

Artifact reconciliation now reads projected rows in keyset pages of at most 64, bounded by the tenant's committed-artifact ID ceiling captured once per pass. A failed pass returns an error; the next pass rescans from the beginning, with completed-task updates remaining idempotent. Artifacts are retained as audit records.

This is a **row-count bound, not a byte bound**: one artifact can still contain a very large `fact_ids` array, a page may therefore still be large, and the database may scan without a pagination index. This change adds no index, migration, or artifact deletion policy. It bounds the reconciliation page materialized at once but does not bound a row's size, a SurrealDB transport frame, total request/process RSS, or query latency. If profiling shows large artifact payloads dominate, design a separate byte/continuation and indexing contract.

Acceptance thresholds are defined in `docs/superpowers/specs/2026-10-07-http-bounded-memory-design.md`. No RAM reduction claim is supported until an identical controlled baseline/candidate comparison and allocation attribution are complete.
