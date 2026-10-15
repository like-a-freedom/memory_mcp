# SurrealDB CPU: read-only investigation, 2026-10-14

## Conclusion and confidence

Confirmed: the HTTP application's background scheduler continually creates short-lived tenant-bound SDK sessions. With one ready tenant, three jobs each bind a new client every second. SurrealDB SDK 3.3.2 clones an independent session and replays authentication, so these binds repeatedly verify the root password. Passive production traffic confirms repeated `attach → signin → use → use → detach` activity alongside housekeeping SQL. This continues when there are no MCP requests and no queued tasks.

Confirmed causal effect: on the exact production SurrealDB image, an isolated local experiment with identical simple reads uses substantially more CPU when repeating password signin than when reusing a token. Production HTTP control reads independently demonstrate the authentication cost.

High-confidence conclusion: session/authentication churn is a substantial, avoidable contributor to the constant CPU floor. Per-second housekeeping SQL adds further work. This is a verified mechanism, not a measured attribution of every production CPU cycle. We did not disable production jobs or obtain a symbolized CPU profile; we cannot honestly assign 100% of production CPU to this mechanism or promise a quantified full-service reduction.

## Scope and provenance

- Production: SSH root@82.165.149.36:2288, host has 2 logical CPUs.
- Grafana context `grafana-pushkino`; datasource `victoriametrics`, UID `edr4onptd6hogd`.
- Production container identity: `surrealdb`, cAdvisor job `ionos_eu_vps_cadvisor`, instance `82.165.149.36:9500`, container `e3f490ab037a...`.
- Application metric identity: job/service `memory_mcp`, env `prod`, instance `mcp.like-a-freedom.ru:443`.
- Live samples: approximately 11:55–12:11 UTC on 2026-10-14.
- Historical comparison: 2026-10-12 18:00 through 2026-10-14 12:00 UTC.
- App build metric: version 1.34.2; process started 2026-10-14 08:59:10 UTC.
- SurrealDB: version 3.3.2, Linux x86_64, RocksDB, started 2026-10-12 19:48:58 UTC.
- SurrealDB image ID `sha256:2a94eb283967c7751c88822084d92a496af8071fa99529c807ff165bde53443c`.
- Same local image: `surrealdb/surrealdb@sha256:ced63befc2ba46d97b922fa388aafcd6c81778760df2ba4aaae72bc00266d9ba`, amd64.
- Local source baseline `bb4e9208` (release 1.34.2). The two pre-existing uncommitted CI changes were preserved.

No production configuration, data, deployments, logging levels, or schedules were changed. Diagnostic database statements were SELECT, INFO, EXPLAIN and RETURN. Authentication was required for these reads. Passive packet capture stayed in memory on the host; only aggregate method/table counts were emitted, with no credentials, tokens, memory content or retained raw packet files. The local isolated container used disposable credentials and an empty in-memory database and was removed after the experiment.

## Evidence

### 1. Sustained load, including a no-MCP window

The initial Docker sample showed SurrealDB CPU 9.74%, app CPU 0.26%, and host load average 0.00/0.04/0.13. Historical 10-minute CPU rates were roughly 0.086–0.124 cores at sampled 2-hour points. The 4-hour 5-minute samples around the app deployment were approximately 0.096–0.156 cores. There was no clear new CPU discontinuity at the 1.34.2 process restart.

In the fixed window 11:00–11:30 UTC, application counters showed **zero MCP HTTP requests** and 120 metrics scrapes. SurrealDB's sampled 5-minute rates at 11:00/11:05/.../11:30 were 12.38%, 12.78%, 11.33%, 10.79%, 14.56%, 12.17%, 12.91% of one core. Therefore foreground retrieval is not necessary for the CPU floor.

Docker's approximately 10% means approximately 0.1 CPU core, around 5% of this two-core host's total capacity. Network and block I/O totals in `docker stats` are cumulative, not current bandwidth. The final `31` is the PID/thread count, not query throughput.

### 2. Live workload is small and idle housekeeping is continuous

Production inventory: one ready tenant, 52 episodes, 411 facts, 1,341 change events, zero tenant tasks, zero task artifacts, zero app sessions. Embeddings are enabled at dimension 2048.

Metrics showed task passes at 1/s and embedding backfill at 1/60s. Lifecycle archival/decay were not continuously running. At 11:30 UTC all observed background pass outcomes in the preceding hour were `ok` (43,261 increments); no error outcome series was returned for that window.

The `lease` pass rate was 11/s. This is a misleading aggregate: `run_cycle` emits `record_job_metric("lease", ...)` for **every registered scheduler job**, including jobs whose cadence gate immediately returns. It neither measures eleven lease renewals nor eleven SQL queries.

Passive WS observation first found 180 SQL frames in a nominal 15-second capture, including 75 registry tenant-list queries, 15 app-session queries, 60 task/artifact queries and 30 sequence/event probes. These counts align with 12 SQL requests per scheduler cycle.

A second nominal 10-second capture decoded nine complete cycles: 108 query, 27 attach, 27 signin, 54 use, 27 detach, 2 ping and 2 unclassified control frames. Capture start/flush boundaries mean these exact totals should not be divided blindly by the nominal timeout. The recurring cycle and production scheduler establish approximately **3 password signins and 12 SQL requests per second per one ready tenant**, plus slower jobs and keepalive.

### 3. Code establishes why sessions are recreated

- `crates/memory-mcp/src/http/leases/scheduler.rs:165`: scheduler ticks every second.
- `crates/memory-mcp/src/http/leases/scheduler.rs:236`: all scheduler jobs are counted under `lease`.
- `crates/memory-mcp/src/http/app_sessions/scheduler.rs:32`: lists ready tenants and binds each tenant on every pass, even when the app-session table is empty.
- `crates/memory-mcp/src/http/tasks/scheduler.rs:90`: lists tenants, binds, requeues/reconciles/claims/deletes on every pass, even when the task queue and artifact table are empty.
- `crates/memory-mcp/src/http/subscriptions/scheduler.rs:25`: binds each ready tenant and probes sequence/latest event every pass.
- `crates/memory-mcp/src/http/registry.rs:177`: `(**privileged).clone()` followed by namespace/database selection.
- SDK `surrealdb-3.3.2/src/lib.rs:343`: clone registers a new session identity.
- SDK `surrealdb-3.3.2/src/engine/remote/ws/mod.rs:939`: `handle_session_clone` copies replay commands and replays them for the new session.
- SDK replayable commands include signin. Actual production capture confirms signin is replayed here, not merely JWT authenticate.
- Core `surrealdb-core-3.3.2/src/iam/verify.rs:1229`: password verification uses Argon2.

The bind clone line predates this memory remediation: Git blame attributes it to 2026-08-31, commit `6839ce339`. This does not by itself date the CPU onset; SDK and deployment history could also affect behavior.

### 4. Production authentication control

A single root signin took 45.518 ms wall time. Three pairs of read-only `RETURN 1` requests differed only in authentication (already obtained bearer token versus Basic credentials):

| Pair | Bearer request ms | Basic request ms |
|---|---:|---:|
| 1 | 9.470 | 49.266 |
| 2 | 5.678 | 57.906 |
| 3 | 1.632 | 65.295 |

The SQL execution times were only 0.166–0.367 ms. These wall times include transport/scheduling and are not CPU attribution or an acceptance benchmark. They nevertheless isolate a large extra cost in repeated password authentication.

### 5. Isolated causal A/B, same binary image

Local Docker/OrbStack on ARM ran the production amd64 image through emulation. Empty in-memory engine, disposable diagnostic credentials, no application data. Six consecutive 12-second phases; active phases executed three groups of four `RETURN 1` reads every second (144 reads per phase). In `repeated`, each group first performs password signin. In `retained`, all groups reuse the previously acquired token. CPU was the Docker container cumulative CPU counter delta, not a wall-time estimate.

| Phase | Queries | CPU seconds | CPU % of one core | Median group wall ms |
|---|---:|---:|---:|---:|
| idle | 0 | 0.3713 | 3.094 | — |
| retained 1 | 144 | 0.1553 | 1.294 | 9.518 |
| repeated 1 | 144 | 1.1278 | 9.398 | 33.058 |
| retained 2 | 144 | 0.4198 | 3.497 | 8.965 |
| repeated 2 | 144 | 1.1802 | 9.834 | 33.456 |
| retained 3 | 144 | 0.4870 | 4.058 | 9.740 |

Repeated password auth consistently adds CPU under identical read workload. Adjacent retained controls vary, so use the conservative observation: repeated phases approximately 9.4–9.8%, retained phases 1.3–4.1%. No confidence interval, native production speedup, or full-app RSS reduction is claimed. HTTP/in-memory is a mechanism-isolation experiment, not an exact replay of production WS/RocksDB session attach/detach and housekeeping.

### 6. Query/index hypotheses

- Actual production EXPLAIN for `SELECT event_seq FROM tenant_change_event ORDER BY event_seq DESC LIMIT 1` uses `IndexScan`, `idx_event_seq`, backward direction, limit 1. It is not scanning all 1,341 events for that probe.
- Episode quota aggregation over 52 episodes/125,938 aggregate content units took 2.022 ms SQL time in the sample; its cadence is 60 seconds, not every second.
- Tenant ready/due discovery EXPLAIN uses TableScan. Tenant schema has only a unique namespace-binding index, no status index. With one tenant this is not a convincing explanation for today's CPU floor; it is a scaling concern.
- Empty tasks/app sessions/artifacts do not justify adding indexes as the first CPU fix.
- We did not perform an exhaustive EXPLAIN audit of every query or benchmark foreground vector retrieval across dimensions.

### 7. CPU and I/O attribution limits

A 20-second production counter sample showed 2.270 seconds container CPU, of which the main SurrealDB process accumulated 2.010 seconds at clock tick resolution. Worker threads dominated the short per-thread sample; RocksDB commit/background threads contributed very little CPU during that sample. Thus the approximately 5-second Docker healthcheck is not the principal contributor, and active RocksDB compaction was not dominant in that window.

In the same 20-second interval, physical read bytes did not increase; physical write bytes increased by 1,880,064 (approximately 94 kB/s). Logical/process read counters increased substantially; these include cached and procfs reads and cannot be interpreted as disk scan throughput. This investigation does not fully attribute those logical reads or the physical write baseline.

Host memory and I/O pressure avg10 were zero in a later sample. The host had existing swap usage, and the SurrealDB process had roughly 34 MiB swapped; this alone does not prove ongoing swap churn. No production symbolized CPU profile was available (perf absent, binary stripped). Internal maintenance contributes an unmeasured residual.

## Prioritized mitigation direction — no implementation yet

1. **Remove authentication/session churn first.** Reuse a tenant-bound maintenance client through a bounded cache or bounded pool with clear ownership, eviction and shutdown behavior. Clone the outer Arc for shared ownership; do not assume `Surreal::clone` is cheap or shares the same session. Preserve a permanently bound namespace/database for each session; do not introduce concurrent rebinding of one global client. Reconnect/auth expiration must invalidate/rebuild safely. A shared unbounded map would reintroduce the memory problem.
2. **Reduce redundant housekeeping after the first change is measured.** Separate prompt task dispatch/lease recovery from expiry deletion, artifact reconciliation and outbox invariant probes. Give housekeeping explicit bounded cadences/backoff. Preserve prompt task claim behavior and contractual recovery/notification deadlines. Verify expiry is enforced on read before delaying cleanup; do not change a security deadline indirectly. Coalesce repeated ready-tenant discovery with bounded paging and fairness, rather than permanently serving only the first 100 tenants.
3. **Make attribution trustworthy.** Record bounded per-job identities instead of labeling all jobs `lease`; distinguish scheduled/skipped/executed and actual DB work. Add bounded session-create/reuse/re-auth and SQL-category counters if existing signals cannot validate the change. No raw SQL parameters or content. Metrics overhead must remain measured and bounded.
4. **Index only measured scaling bottlenecks.** Benchmark ready/due discovery and task predicates at realistic registry/task sizes and inspect full production query shapes. Add the right compound indexes only after controlled EXPLAIN and performance evidence. This is not the first fix for a one-tenant idle CPU floor.

## Acceptance gates for a future implementation

- Red regression reproduces repeated SDK remote sessions/signins under an idle scheduler; green proves reuse over multiple ticks without stopping required jobs.
- Real remote SurrealDB integration tests (same pinned version) verify two tenants with interleaved concurrent work, namespace isolation, cancellation, cache eviction, credential/session expiration, reconnect and shutdown. Use barriers and bounded waits.
- Bounded resource tests: clients/sessions/retained bytes do not grow with ticks or tenant churn; eviction/release is observable; tenant count above page size remains fair.
- Fencing, lease renewal, cancellation, task recovery, artifact reconciliation, outbox notifications and expiry admission behavior retain their deadlines and durable semantics.
- Native Linux A/A and A/B with immutable parent/candidate images, identical corpus, authentication and resources; idle, ingest, retrieve, task/reembed and reconnect scenarios. Measure DB and app CPU, SQL/signin rates, p50/p95/p99 latency, throughput, errors, physical I/O, RSS/HWM/cgroup memory and swap.
- Embedding dimensions 17/384/768/1536/2048/3072, dimension/model transition and reembed, and embeddings disabled. Session reuse must not cache stale embedding policy or namespace binding.
- Retrieval quality, outputs and durability must remain equivalent; use the real provider/corpus path, not fixture-only validators. No worsening of performance or memory accepted merely in exchange for CPU savings.
- Proposed target: steady idle signin should cease scaling with scheduler ticks and remain near zero after warmup (except explicit reconnect/token renewal). Fix a numerical CPU/latency/memory acceptance budget from native A/A noise before implementation; do not convert this emulated mechanism test into a production speedup promise.
- Small relevant suites first; before shipping full applicable tests, strict workspace Clippy with `-D warnings`, fmt check and actual hosted gates.

## Reproducible telemetry queries

Use fixed UTC bounds and the verified context/datasource above.

```promql
sum by (name) (rate(container_cpu_usage_seconds_total{name="surrealdb",instance="82.165.149.36:9500"}[5m]))
sum by (route) (increase(memory_http_requests_total{job="memory_mcp"}[30m]))
sum by (pass) (rate(memory_background_jobs_total{job="memory_mcp"}[10m]))
sum by (pass) (rate(memory_background_job_duration_seconds_sum{job="memory_mcp"}[10m]))
  / sum by (pass) (rate(memory_background_jobs_total{job="memory_mcp"}[10m]))
```

Example:

```sh
rtk proxy gcx metrics query --context grafana-pushkino -d edr4onptd6hogd \
  'sum by(name)(rate(container_cpu_usage_seconds_total{name="surrealdb",instance="82.165.149.36:9500"}[5m]))' \
  --from 2026-10-14T11:00:00Z --to 2026-10-14T11:30:00Z --step 5m --agent
```

Keep SQL credentials inside the diagnostic process. Do not copy full Docker environment/configuration or raw packets into analysis outputs.
