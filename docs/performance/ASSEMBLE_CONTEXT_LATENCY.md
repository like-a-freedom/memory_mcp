# `assemble_context` latency runbook

## Authorized continuation and recovery check (November 15, 2026)

The user deferred Task 2 and authorized production inspection, backup, restarts, and verification while preserving production data. The guard and stage metrics remain deferred; neither has acceptance evidence. The historical timing restrictions below apply to any resumed benchmark, not to correctness checks.

Operations before the address correction inspected a host that the user later identified as the wrong address. Its container topology, data mount, and resource observations are historical only; they are not verified facts about actual production.

A cold backup at `/opt/memory_mcp-backups/pre-latency-20261115T182857Z/memory_mcp.tar` was previously reported as created after stopping the application and then the database. Its recorded SHA-256 is `0a29386fbb840387165868a0c586b5022a9f2d8d3be4644012da2bad064712b4`; extraction and all 33 file digests matched, and the directory and archive modes were reported as `700` and `600`. Because the address used for those operations has since been identified as incorrect, the archive's association with actual production is unverified. A fresh cold backup was therefore made on the corrected production host over SSH port `2288`, at `/opt/memory_mcp-backups/pre-hotfix-v1.38.0-20261116T074844Z/memory_mcp.tar`. Its SHA-256 is `4f3c500d1acce880a1673d8b0d01487b9a3b0d23771710ef97188ff8a064688a`; it is 154060800 bytes, with archive mode `600` and directory mode `700`. `tar --compare` verified it against the stopped `/opt/memory_mcp/data` tree. Both services were restarted and SurrealDB returned to `healthy`; exact container IDs are retained in the private ledger, not repository artifacts. Keep container metadata private.

A copy of the previously reported backup data, without configuration or credentials, was opened by local SurrealDB 3.3.5 in a network-disabled container limited to one CPU and 768 MiB RAM/swap. `isready` returned `OK`, with no OOM; the container was then stopped. This verifies only that the copied database opens; it does not establish that the copy came from actual production, or prove retrieval results, tenant isolation, or latency. The first archive transfer timed out and is incomplete; do not use that local `data.tar`. The subsequent directory transfer supplied the data used by the successful startup check.

Health checks against the previously used address required its configured Host and `/memory` base path; those results are historical and do not establish health on actual production. `/memory/health` serves UI HTML and is not a health endpoint. Current production deployment and health results are recorded in the Status section below.

## Application-only rollout and rollback

The authorized change is limited to the application image built from release tag `v1.38.0` plus the reviewed `SELECT VALUE source_fact_id` fix. Do not deploy the current `master` checkout. The fix changes no schema or migration, so rollback is an application-image rollback; do not restore or restart SurrealDB as part of the rollback.

The candidate is `memory_mcp:hotfix-v1.38.0-triple-select-value-20261115`, image ID `sha256:95fad476c179b40e2102cfe41b9a0d39cd1c3fb853d144c574ff661c025f81cc`, platform `linux/amd64`. Its local archive `.tmp-investigate/hotfix-artifacts/memory_mcp-hotfix-v1.38.0-triple-select-value-20261115.tar` has SHA-256 `ab2506e59c493d85b908a1d1275888d6a4864d8500b0f070fc7e793470d43b7a`. The 230239232-byte archive was transferred to `/opt/memory_mcp-backups/candidate-memory_mcp-hotfix-v1.38.0-triple-select-value-20261116T074844Z.tar`; the remote SHA-256 matched, and Docker loaded the same candidate image ID and platform.

Before rollout, connect to the corrected production host over SSH port `2288` and verify the fresh cold-backup path and SHA-256 above. Then verify the running application and SurrealDB image IDs, Compose service names, architecture, and Compose image reference. Keep container IDs in the private operational ledger; do not put them in repository artifacts. Confirm that the app reference is a valid Docker image-tag destination in `name[:tag]` form; a digest-only reference cannot be used by this retag-based procedure. Choose a unique rollback tag and confirm it does not already exist. If these checks fail, do not change image tags or Compose. Verify that the staged image ID and architecture match the locally validated artifact. Keep secrets out of command output and logs.

Transfer the image using a new, uniquely named local archive under `.tmp-investigate/`, then load it on the production host. Verify the loaded image ID and target architecture before tagging. Tag the old application image ID with the verified unique rollback tag, then verify that the tag resolves to the old image ID. Record the image reference configured for the `memory_mcp` service, then tag the staged image ID to that reference. Deploy only the application service:

```bash
docker compose -f /opt/memory_mcp/docker-compose.yaml up -d --no-deps --force-recreate --pull never --no-build memory_mcp
```

Do not run `down`, recreate dependencies, change the Compose project, or operate on the SurrealDB service or its volume. Verify that the replacement application uses the staged image ID and that the SurrealDB container ID is unchanged. Probe only `/memory/health/live` and `/memory/health/ready` with the configured Host. Do not send retrieval or mutation requests to production. If the SurrealDB container ID changes unexpectedly, do not attempt to recreate or restore it; preserve its volume and stop for investigation.

If an application health check fails while the SurrealDB identity remains unchanged, restore the recorded Compose image reference with the preserved ID and run the same app-only Compose command:

```bash
docker image tag <previous-application-image-id> <recorded-compose-image-reference>
docker compose -f /opt/memory_mcp/docker-compose.yaml up -d --no-deps --force-recreate --pull never --no-build memory_mcp
```

Verify the old application image ID, unchanged SurrealDB container identity, and both health endpoints. Leave the verified backup and rollback image in place. Record the actual commands, image IDs, health results, and outcome below. Retain container IDs in the private ledger rather than repository artifacts. Do not claim deployment until the checks complete.

## Status

The corrected triple query passes its local retrieval contract, and the ignored remote compatibility test passed twice against the disposable SurrealDB 3.3.5 image. The user deferred Task 2; this does not validate its baseline gate. Both baseline reports are invalid. The `v1` run exceeded the 3% limit in 20 of 24 A/A records, with maximum noise 0.4195653025770974. The revised `v2` run exceeded it in 16 of 24 records, with maximum noise 0.3192349170251424. The v2 run used one prepared database per case and seeded each fixture once. Host disk activity coincided with the run, but the logs do not identify its source or link it to individual query samples. Task 2 remains incomplete. Do not run another latency capture until the variability source is identified and removed or the environment is isolated to control it. The next report path is reserved. A later capture must use timestamped host and Docker I/O logs aligned with per-query sample intervals, outside the report allowlist. Do not run `memory-512m`, evaluate the guard, add stage metrics, or lower the 3% threshold while the baseline is invalid.

Application-only rollout completed on November 16, 2026, over SSH port `2288` on the corrected production host; the previously used address was incorrect. The host is Docker `linux/x86_64`, with Compose services `memory_mcp` and `surrealdb` and app image reference `ghcr.io/like-a-freedom/memory_mcp:latest`. The database bind mount is `/opt/memory_mcp/data` to `/data`. A fresh cold backup from this host is recorded above. The previous app image was preserved under `ghcr.io/like-a-freedom/memory_mcp:rollback-pre-triple-select-20261116`, and the rollback tag was verified to resolve to the prior image. The Compose image reference was retagged to candidate image `sha256:95fad476c179b40e2102cfe41b9a0d39cd1c3fb853d144c574ff661c025f81cc` before running the app-only `up -d --no-deps --force-recreate --pull never --no-build memory_mcp` command. After deployment, the app uses the candidate image; SurrealDB's container and image identities are unchanged, and it remains healthy. `/memory/health/live` and `/memory/health/ready` with the configured Host returned HTTP 200 before and after deployment. Exact container IDs are retained in the private ledger. No rollback was needed. No production retrieval or mutation request was sent; this confirms deployment and health, not live retrieval behavior.

## Container quiescence diagnostic

The compatibility-only diagnostic run started fixture setup at 18:10:20.568 UTC and completed it at 18:10:25.217 UTC, about 4.65 seconds later. Before the first query, the harness sampled exact Docker Engine CPU and block-I/O counters through the local Unix socket. It requires five consecutive intervals at or below 1% container CPU and 1 MiB/s combined container I/O, samples at least 500 ms apart, and fails after 60 seconds. The live check passed five quiet intervals of about 2.0 seconds each. Container CPU deltas ranged from 0.7 to 4.0 ms; reads were zero and writes ranged from 0 to 24 KiB.

The gate does not observe host-wide activity. Host `iostat` recorded 430.32 MB/s during fixture setup at 18:10:21.678 UTC and 50.94 MB/s at 18:10:36.803 UTC, around gate completion. The source remains unidentified, and these observations do not establish that host I/O caused the invalid A/A calibration. The gate is necessary container evidence, not proof of a quiet or isolated host. Do not run another latency capture until host variability is attributed and controlled or the benchmark runs in an environment that isolates it. The timestamped diagnostic logs are `.superpowers/sdd/2026-11-14-assemble-context-latency/compatibility-diagnostic-cargo-20261115T181016Z.log`, `.superpowers/sdd/2026-11-14-assemble-context-latency/compatibility-diagnostic-docker-stats-20261115T181016Z.log`, and `.superpowers/sdd/2026-11-14-assemble-context-latency/compatibility-diagnostic-iostat-20261115T181016Z.log`.

## Scope and evidence

The experiment uses synthetic data on a disposable local SurrealDB server. It does not measure production latency or establish production p99. The historical synthetic experiment did not access production. The later authorized backup and recovery checks are recorded above; they provide no latency evidence.

The deterministic container-evidence tests in `crates/memory-mcp/tests/retrieval_latency_remote.rs` feed synthetic Docker inspection JSON to a test-local validator. They test its acceptance policy for image identity, URL-to-loopback binding correspondence, RocksDB startup, and resource limits, including rejection of nonzero `NanoCpus` and nonempty `CpusetCpus`. They do not invoke the Docker daemon or prove a live container's state. A separate fake-executable test checks Docker CLI timeout and child cleanup. The ignored remote tests run Docker CLI inspection before connecting and cover the live-container boundary.

## Local test results

- `cargo test -p memory_mcp --features eval-support --locked --test retrieval_latency_remote`: 34 passed, 2 ignored. This includes the baseline candidate-mode rejection, CPU resource-policy checks, connection-mode report fields, fake-Docker timeout/child-reaping regression, and container-quiescence parser and threshold tests.
- `cargo test -p memory_mcp --features eval-support --locked --test triple_retrieval_contract`: 21 passed. Follow-up for `limit=1`: the prior test failed when independent valid queries selected different eligible facts; the revised test passed after asserting eligibility and result counts without relying on row order.
- `cargo test -p memory_mcp --features eval-support --locked --test retrieval_query_projection --test retrieval_query_batching --test graph_batch_query_regression --test query_scaling_regressions`: 22 passed; `query_scaling_regressions` contains no tests.
- `cargo fmt --all --check`: passed.

- `cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http,test-fixtures --locked -- -D warnings`: passed.
- The same workspace Clippy command with `eval-support` added to `--features`: passed.
- `cargo test -p memory_mcp --locked`: passed; the library target reported 1,968 passed, 0 failed, and 1 ignored, and the remaining test targets and doc tests passed.
- `cargo run -p eval-harness --locked --bin memory-eval -- run --profile evals/profiles/retrieval_quality.json --artifact target/evals/assemble-context-retrieval-quality.json`: `INVALID`. A real embedding provider was unavailable for all 37 cases. The artifact is local to `target/evals/` and does not support a retrieval-quality conclusion.
- `cargo run --release -p eval-harness --locked --bin memory-perf -- run --profile evals/perf/profiles/perf-weekly.json --artifact target/perf/assemble-context-perf.json --baseline evals/perf/baselines/perf-weekly-local.json --host-tag local`: passed. This broad perf smoke test does not replace the focused SQL experiment.

The first evaluation attempt failed before writing because `target/evals/` did not exist. After creating that output directory, the command completed and wrote the invalid-result artifact. The perf output directory was also created before its run.

## Disposable Docker server

The server used SurrealDB `3.3.5` and the matching SDK. Docker Engine was `29.8.2` on `linux/aarch64`; the image targeted `linux/arm64`, with no architecture emulation. The image tag `surrealdb/surrealdb:v3.3.5` resolved to `sha256:f40e356e40dd2094e5a743ad8b1353f646cf3a660541e92b9f6cce003d3be2dd`.

The verified container was `memory-triple-latency-20261114-default`, ID `d4dec2ff13a43a35689eb256ddf6dabb9b875917733f16a271022356a1e332a5`. It published container port `8000/tcp` on `127.0.0.1:33127`, used `rocksdb:///tmp/surrealdb`, and had no explicit memory or CPU limits. Docker inspection reported memory `0`, memory swap `0`, CPU quota `0`, and CPU period `0`. This matches `docker-default`; `memory-512m` was not provisioned.

Equivalent command for this verified configuration (Docker assigned host port `33127` dynamically):

```bash
docker run --detach \
  --name memory-triple-latency-20261114-default \
  --publish 127.0.0.1::8000 \
  surrealdb/surrealdb:v3.3.5 \
  start --log warn --user root --pass root \
  --bind 0.0.0.0:8000 rocksdb:///tmp/surrealdb
```

Use `root` credentials only on the disposable local server. Do not copy them into latency reports or reuse them elsewhere. An earlier attempt to start RocksDB at `/var/lib/surrealdb` failed because the image runtime user could not write there. `/tmp/surrealdb` worked without elevating privileges. The container emitted `onnxruntime cpuid_info warning: Unknown CPU vendor. cpuinfo_vendor value: 0`; no effect on SQL timing is inferred from that warning.

Useful checks for a new run:

```bash
docker inspect --format '{{.Id}}|{{.Config.Image}}|{{.Image}}|{{.State.Running}}|{{json .HostConfig.PortBindings}}' <container-id>
docker port <container-id> 8000/tcp
docker info --format '{{.ServerVersion}}|{{.OSType}}|{{.Architecture}}'
```

The ignored harness performs its own preflight. It requires exactly one running container publishing the configured port and checks the image ID, RocksDB arguments, and resource settings before opening the SDK connection. Docker's published `HostIp` must equal a literal IP in the configured URL; when the URL uses `localhost`, any loopback `HostIp` is accepted. Both resource modes require `CpuQuota=0`, `CpuPeriod=0`, `NanoCpus=0`, and empty `CpusetCpus`; missing or malformed fields fail preflight. Each Docker CLI invocation has a 10-second timeout. On timeout, the harness terminates and reaps the child, then fails preflight. A fresh inspection on November 15, 2026 confirmed the current container has `NanoCpus=0` and an empty `CpusetCpus`; the live compatibility test then passed the harness preflight.

## Remote compatibility attempt

The following checks succeeded:

- The server health endpoint returned HTTP 200.
- The image's SQL shell returned `[1]` when connected to the server over its container-local WebSocket endpoint.
- The same SQL shell returned `[1]` through the published host route.
- A raw WebSocket upgrade to the published `/rpc` endpoint returned `HTTP/1.1 101 Switching Protocols`.
- The harness's live Docker inspection accepted the selected container before each SDK attempt.

The passing compatibility run used the following environment and test command. The configured report path was unused because this test does not write a latency report:

```bash
MEMORY_TRIPLE_TEST_URL=ws://127.0.0.1:33127 \
MEMORY_TRIPLE_TEST_IMAGE_ID=sha256:f40e356e40dd2094e5a743ad8b1353f646cf3a660541e92b9f6cce003d3be2dd \
MEMORY_TRIPLE_TEST_RESOURCE_MODE=docker-default \
MEMORY_TRIPLE_TEST_ASSERT_SPEEDUP=0 \
MEMORY_TRIPLE_TEST_REPORT=/tmp/memory-triple-latency-remote-compat-20261115-01.json \
cargo test -p memory_mcp --features eval-support --locked --test retrieval_latency_remote remote_triple_compatibility -- --exact --ignored --test-threads=1
```

Initial SDK attempts failed at `connect` until the WebSocket URL normalization was fixed. The next live run reached fixture seeding but failed because the schema used the default 1536-dimensional vectors while the fixture contains 2048-dimensional vectors. After adding the eval-only dimension-aware bound constructor, migration and fixture seeding passed, but the runtime version check failed. `INFO FOR DB` does not report a version, so the harness now derives its declared server version from the Docker-verified image tag and immutable image ID. The final `remote_triple_compatibility` run passed on November 15, 2026: `1 passed` in 7.66 seconds after fresh Docker inspection. It exercised migration, fixture seeding, and the public triple lookup against the published loopback endpoint. After the production backup and local restore check, the same ignored compatibility test passed again against the still-running local disposable container: `1 passed` in 17.81 seconds. The container quiescence gate passed before its queries. This is functional compatibility evidence only; it is not a calibrated latency result and it did not connect to production.

The server version recorded by the harness comes from the configured `surrealdb/surrealdb:v3.3.5` tag after Docker confirms that the tag resolves to the configured immutable image ID. This is image identity evidence, not runtime version introspection. `INFO FOR DB` does not report the version, and `RETURN version()` is unsupported by this server. The SDK version comes from the build.

## Remote latency capture

### Historical `v1` capture

The original `remote_latency_capture` wrote `/tmp/memory-triple-latency-docker-default-20261115.json` and completed 24 calibration records plus 24 sample records. Twenty calibration records exceeded the 3% limit; four passed: `populated_no_match` limits 1 and 40 in the old `warm` mode, and `positive_match` limits 1 and 40 in the old `warm` mode. Maximum noise was 0.4195653025770974. This run is `INVALID`; preserve the report and never reuse its path.

Each old sample contained five elapsed values and five SQL calls. Timer resolution was 41 ns. Query durations ranged from 0.368 to 1.060 ms for `empty_table`, 196.402 to 279.641 ms for `populated_no_match`, and 162.158 to 274.459 ms for `positive_match`. These values do not support timing conclusions. After the 866.29-second run, Docker reported approximately 5.3 GiB of memory use and 105 GB of cumulative block I/O. The old harness created and seeded 10 databases for A/A pairs and five more for timed samples for each fixture-backed case/limit. This could have caused RocksDB I/O interference, but no concurrent I/O or compaction trace was captured. Warm-mode calibration also failed, so the cause remains unconfirmed.

### Revised `v2` capture

The revised harness prepares one database per case, seeds each fixture once, and performs no database creation, fixture seeding, or cleanup between measured queries. `new_connection` opens a fresh SDK connection to the prepared database before each query; connection setup is excluded from the query timer. `reused_connection` uses one connection and performs five untimed calls before calibration and timed samples. These names describe connection handling only. Neither mode establishes cold or warm server-cache state. Reports use schema `memory-triple-latency/v2` and the allowlisted `connection_mode` field. The complete schema is specified in the design contract.

The revised `docker-default` capture wrote `/tmp/memory-triple-latency-docker-default-20261115-connection-modes-01.json`. It completed 24 calibration records and 24 sample records. Sixteen calibration records exceeded 3%; eight passed. Every `empty_table` calibration exceeded the gate. Maximum noise was 0.3192349170251424 for `empty_table`, limit 0, `new_connection`. The timer resolution was 83 ns. Timed samples returned stable row digests and each sample made five SQL calls. Timing values are diagnostic only because calibration failed.

The monitor recorded Docker stats from approximately 113 GB read / 92.5 GB written at start to 113 GB read / 92.9 GB written near the end, with memory briefly reaching about 12.05 GiB and CPU near 100% during parts of the run. Host `iostat` recorded up to 53,331 transactions/s and 321.26 MB/s during the capture. A post-run sample ranged from 0.67 to 9.20 MB/s over six intervals. The process snapshot did not identify the disk-I/O source. The monitoring logs lack timestamps aligned to individual query samples, so the evidence shows concurrent activity but does not establish causality. The run lasted 80.92 seconds, compared with 866.29 seconds for the `v1` capture. Docker's rounded cumulative write counter increased by about 0.4 GB rather than 105 GB. Preparing one database per case removed repeated fixture seeding between observations, but did not produce valid calibration.

The current disposable container is `d4dec2ff13a43a35689eb256ddf6dabb9b875917733f16a271022356a1e332a5`, running image ID `sha256:f40e356e40dd2094e5a743ad8b1353f646cf3a660541e92b9f6cce003d3be2dd`, publishing `127.0.0.1:33127`, with RocksDB at `/tmp/surrealdb`; fresh inspection confirmed no explicit memory, swap, or CPU limits. Preserve both invalid reports. The next report path, `/tmp/memory-triple-latency-docker-default-20261115-connection-modes-02.json`, is reserved. Before any new capture, control the variability source and prepare timestamped host and Docker I/O logs aligned with per-query sample intervals. Do not run `memory-512m` or Tasks 3 and 4 until `docker-default` passes every baseline gate.

Do not reduce the sample count or raise the 3% threshold. The baseline harness accepts only `MEMORY_TRIPLE_TEST_ASSERT_SPEEDUP=0`; it rejects `1` until Task 3 implements candidate-query capture. Do not add SQL, bound values, fact IDs, raw rows, credentials, tenant identifiers, or request labels to the report.

## Cleanup

The current disposable container must remain running; do not remove it as part of this work. For a future explicitly approved cleanup, the container and its anonymous Docker volumes can be removed with:

```bash
docker rm --force --volumes d4dec2ff13a43a35689eb256ddf6dabb9b875917733f16a271022356a1e332a5
```

Do not remove any container unless its full ID and image have been re-verified. The unrelated service previously observed on port `18081` is out of scope; never connect to it.
