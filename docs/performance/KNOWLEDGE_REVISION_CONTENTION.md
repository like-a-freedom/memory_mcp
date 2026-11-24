# Knowledge revision write-path contention check

**Date:** 2026-11-21

**Purpose:** Compare the complete local ingest/extract path before and after
the revision-ledger candidate, and check whether concurrent writes show an
obvious latency regression.

## Setup

- Machine: macOS 27.0.1, `arm64`.
- Rust: `rustc 1.100.0 (0ef1bc386 2026-11-09)`.
- Starting implementation: frozen revision `1ae1da4a4e3120f2de94753fc1a949caee3aced7`, built from a `git archive` extraction without creating or switching a branch.
- Candidate: the user-authorized `master` worktree at the same `HEAD`, with its uncommitted temporal-memory changes.
- Both builds used the same shared Cargo target, locked dependencies, optimized bench profile with LTO disabled and 16 codegen units. The benchmark source is the existing `eval-harness/benches/contention.rs`.
- Each measured operation is one production `ingest_probe` followed by `ExtractCapability::extract_from_service`. Two- and four-client cases run three such operations per client against one shared in-memory SurrealDB service. The operation timer excludes fixture creation and database migrations; Criterion's outer iteration timer includes them.
- Each cell used a 0.5 s warm-up and ten measured samples. This is a local smoke comparison, not a statistically powered performance study.

## Results

The values below are the median and range of the benchmark's per-operation observations from the ten-sample collection window. One operation means the full ingest-plus-extract pair.

| Concurrent clients | Starting revision | Temporal candidate | Interpretation |
| ---: | ---: | ---: | --- |
| 2 | 1.420 ms (1.388–1.509 ms) | 1.421 ms (1.382–1.544 ms) | Distributions overlap; no clear change at this sample size. |
| 4 | 1.113 ms (1.032–1.198 ms) | 1.090 ms (1.015–1.144 ms) | Distributions overlap; no clear change at this sample size. |

The full Criterion iteration estimate, which includes creation of the test service and migrations, was 121.90 ms for the starting revision versus 121.20 ms for the candidate at two clients, and 126.82 ms versus 126.89 ms at four clients. Fixture setup dominates that number; it is not a request-latency estimate.

## Limits

The before/after comparison above measures the production API path under
in-memory SurrealDB and includes extraction and whatever claim
projection/reconciliation work the fixture produces. It does not isolate
revision-clock wait or transaction time, and it does not assert that each
operation creates a relation. Ten samples on one machine cannot rule out a
small or workload-specific regression. The separate durable characterization
below isolates two local timing components, but it is not a frozen-revision
comparison and does not model remote SurrealDB, multiple server processes,
SaaS tenant concurrency or production traffic. Neither result supports a
capacity claim.

## Durable revision-clock characterization

After the in-memory comparison, the candidate was also measured against one
local, on-disk SurrealKV database. This is a characterization of the new
revision path, not a before/after comparison: the frozen starting revision has
no equivalent revision-clock write to compare.

- Four concurrent `MemoryService` writers share one embedded SurrealKV client
  and database. Each writer creates one production fact per batch.
- Production migrations run before measurement. Sixteen four-writer batches
  warm up the store; then 100 batches contribute exactly 400 successful
  revision-transaction observations.
- The eval-only observer separates time waiting for this process's revision
  mutex from elapsed transaction work (including a successful retry, if any).
  It does not observe storage-engine lock internals or time waiting on remote
  database resources.
- Criterion measured 10 batches after a one-second warm-up. Its batch estimate
  was 35.073 ms (34.280–35.928 ms); this is the full four-write batch wall time.
- Across the separately collected 400 transaction observations, lock-wait
  p50/p95 were 11.900/23.460 ms and transaction-duration p50/p95 were
  7.669/8.994 ms.

These figures show measurable local serialization under four writers and are
useful for future comparisons. They do not establish a production capacity
limit or a regression: this is one machine, one process, one durable local
engine and one small write shape. Remote SurrealDB, multiple server processes,
tenant concurrency, failure/retry distributions and production traffic remain
unmeasured. Do not promote based on this benchmark alone.

## Separate-process WebSocket characterization

On 2026-11-21, the isolated revision benchmark was repeated against the
official SurrealDB 4.0.1 server in a separate local process. The server listened
only on `127.0.0.1:8765` and used a temporary, on-disk SurrealKV directory. Four
independent SDK WebSocket clients, each with its own in-process revision mutex,
shared one generated namespace and each issued one production fact write per
batch. Sixteen four-writer batches warmed the store; 100 batches supplied 400
successful revision-transaction observations. Criterion measured ten batches
after a one-second warm-up.

The first attempt with the original three-attempt database retry policy failed
during warm-up with `Transaction conflict`. Increasing the general retry count
to four made the writes succeed, but the 200/400/800 ms backoff delays dominated
the observed remote p95. The final implementation keeps the general query and
embedded-storage policies intact and uses four attempts with a 10 ms initial
backoff for the remote atomic-transaction path.

| Remote transaction policy | Criterion four-write batch | Successful transaction duration p50/p95 | Outcome |
| --- | ---: | ---: | --- |
| Four attempts, 200 ms initial delay | 1.5234–1.5318 s | 669.405/1,526.952 ms | 400/400 writes succeeded; retry delays dominated the tail. |
| Four attempts, 10 ms initial delay | 153.05–155.24 ms | 55.526/149.708 ms | 400/400 writes succeeded; Criterion reported an 89.913% median reduction against the prior run. |

The second run also completed the durable local characterization under the
unchanged embedded policy: four writers produced 400 observations with lock
wait p50/p95 12.600/23.841 ms and transaction-duration p50/p95 7.746/9.192 ms.
This confirms that the faster remote policy did not replace the embedded
policy. The remote observer's lock-wait values are zero because each writer
uses a different client mutex; its transaction duration includes the remote
conflict retries and backoff.

This is a loopback wire-protocol and separate-server-process characterization,
not a WAN, multi-host, SaaS tenant-load or production-traffic result. The
standalone server used SurrealDB 4.0.1, whose official release and installation
instructions are documented at [SurrealDB 4.0 releases](https://surrealdb.com/releases/4.0)
and [SurrealDB installation](https://surrealdb.com/surrealdb/install). Use a
disposable dedicated database and temporary data directory: the harness
creates a generated namespace and does not drop it automatically.

## Reproduction

Run each command once from the corresponding source tree. The starting revision can be extracted with `git archive` into a temporary directory; no branch is required.

```sh
CARGO_PROFILE_BENCH_LTO=false CARGO_PROFILE_BENCH_CODEGEN_UNITS=16 \
  cargo bench -p eval-harness --features bench --bench contention -- \
  'contention/clients_2' --noplot --warm-up-time 0.5 --measurement-time 1 --sample-size 10

CARGO_PROFILE_BENCH_LTO=false CARGO_PROFILE_BENCH_CODEGEN_UNITS=16 \
  cargo bench -p eval-harness --features bench --bench contention -- \
  'contention/clients_4' --noplot --warm-up-time 0.5 --measurement-time 1 --sample-size 10
```

Run the durable characterization against the candidate worktree with:

```sh
CARGO_PROFILE_BENCH_LTO=false CARGO_PROFILE_BENCH_CODEGEN_UNITS=16 \
  cargo bench -p eval-harness --features bench --bench contention -- \
  'revision_clock_durable_surreal_kv_4_writers' --noplot \
  --warm-up-time 1 --measurement-time 3 --sample-size 10
```

For the separate-process WebSocket case, start the pinned SurrealDB 4.0.1
binary against a temporary SurrealKV directory, bind only to loopback, and use
temporary credentials. Then run:

```sh
MEMORY_REVISION_REMOTE_BENCH_URL=ws://127.0.0.1:8765 \
MEMORY_REVISION_REMOTE_BENCH_ACK=dedicated-loopback-database \
MEMORY_REVISION_REMOTE_BENCH_USERNAME="$LOCAL_DB_USER" \
MEMORY_REVISION_REMOTE_BENCH_PASSWORD="$LOCAL_DB_PASSWORD" \
CARGO_PROFILE_BENCH_LTO=false CARGO_PROFILE_BENCH_CODEGEN_UNITS=16 \
  cargo bench -p eval-harness --features bench --bench contention -- \
  revision_clock_remote_ws_4_clients --noplot --warm-up-time 1 \
  --measurement-time 3 --sample-size 10
```
