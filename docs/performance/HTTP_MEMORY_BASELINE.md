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

## Controlled comparison

Do not treat the 2026-10-07 production snapshots as a causal baseline: the deployed image has no verified source revision, traffic was not isolated, and no exact-image controlled workload was run. Do not seed a production tenant or issue destructive workload requests to production.

Use a disposable Linux staging deployment with a dedicated database/namespace, explicit staging acknowledgement, remote SurrealDB, and deterministic local/mock embedding provider. Seed and verify the same synthetic dataset for each fresh process: 24 episodes (each <=64 KiB), 129 facts (each <=4 KiB content), and 454 entities. Preserve dataset and configuration hashes, build commit, enabled features and allocator in the evidence metadata. Never save API keys, raw customer data, or unredacted HTTP bodies.

For baseline and candidate, run three fresh-process repetitions under the same 2 CPU limit and identical dataset/config/provider. Measure startup before tenant-ready, health-only, first activation, first recall/extract, 20 cycles (10 identical recalls, 10 unique recalls, one small extraction each), a separate 8-request concurrency case, provider outage, maintenance, subscription eviction, and no-scrape metrics operation. Separate accepted requests from refusals. Use the same sampler and workload. Run allocator profiling only in a separate staging diagnostic build with symbols; correlate live retained objects and peak stacks, and do not infer allocator fragmentation from RSS, swap, or heap mappings alone.

Wait for workload requests and provider in-flight work to reach zero; inspect task, tenant runtime and lifecycle owners where observable. A timer alone does not prove quiescence. If a required owner has no drain signal, mark the idle sample **inconclusive** rather than passing the gate. Keep raw profile artifacts local and sanitized; commit only redacted summary metrics and references/hashes.

Acceptance thresholds are defined in `docs/superpowers/specs/2026-10-07-http-bounded-memory-design.md`. No RAM reduction claim is supported until an identical controlled baseline/candidate comparison and allocation attribution are complete.
