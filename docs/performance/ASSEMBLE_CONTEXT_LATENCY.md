# `assemble_context` latency runbook

## Status

The corrected triple query passes the local retrieval contract. The remote compatibility gate is `BLOCKED`: Docker inspection and direct SQL shell checks pass, but the Rust SDK fails while opening its WebSocket connection. No remote latency report was produced. Do not add the optional empty-path guard or stage metrics without a valid corrected baseline.

## Scope and evidence

The experiment uses synthetic data on a disposable local SurrealDB server. It does not measure production latency or establish production p99. Production systems were not accessed, changed, or tuned.

The deterministic container-evidence tests in `crates/memory-mcp/tests/retrieval_latency_remote.rs` feed synthetic Docker inspection JSON to a test-local validator. They test its acceptance policy for image identity, URL-to-loopback binding correspondence, RocksDB startup, and resource limits, including rejection of nonzero `NanoCpus` and nonempty `CpusetCpus`. They do not invoke the Docker daemon or prove a live container's state. A separate fake-executable test checks Docker CLI timeout and child cleanup. The ignored remote tests run Docker CLI inspection before connecting and cover the live-container boundary.

## Local test results

- `cargo test -p memory_mcp --features eval-support --locked --test retrieval_latency_remote`: 29 passed, 2 ignored. This includes the baseline candidate-mode rejection, CPU resource-policy checks, and fake-Docker timeout/child-reaping regression.
- `cargo test -p memory_mcp --features eval-support --locked --test triple_retrieval_contract`: 20 passed.
- `cargo fmt --all --check`: passed.
- `cargo test -p memory_mcp --locked`: passed.
- `cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http,test-fixtures,eval-support --locked -- -D warnings`: passed.
- `cargo run -p eval-harness --locked --bin memory-eval -- run --profile evals/profiles/retrieval_quality.json --artifact target/evals/assemble-context-retrieval-quality.json`: `INVALID`. All 37 cases could not initialize a real embedding provider. The artifact is local to `target/evals/` and does not support a retrieval-quality conclusion.
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

The ignored harness performs its own preflight. It requires exactly one running container publishing the configured port and checks the image ID, RocksDB arguments, and resource settings before opening the SDK connection. Docker's published `HostIp` must equal a literal IP in the configured URL; when the URL uses `localhost`, any loopback `HostIp` is accepted. Both resource modes require `CpuQuota=0`, `CpuPeriod=0`, `NanoCpus=0`, and empty `CpusetCpus`; missing or malformed fields fail preflight. Each Docker CLI invocation has a 10-second timeout. On timeout, the harness terminates and reaps the child, then fails preflight. The live Docker inspection recorded above predates the explicit `NanoCpus` and `CpusetCpus` checks; verify those fields on a fresh inspection before claiming the updated resource gate passed.

## Remote compatibility attempt

The following checks succeeded:

- The server health endpoint returned HTTP 200.
- The image's SQL shell returned `[1]` when connected to the server over its container-local WebSocket endpoint.
- The same SQL shell returned `[1]` through the published host route.
- A raw WebSocket upgrade to the published `/rpc` endpoint returned `HTTP/1.1 101 Switching Protocols`.
- The harness's live Docker inspection accepted the selected container before each SDK attempt.

`remote_triple_compatibility` still failed at the Rust SDK `connect` stage with both `ws://127.0.0.1:33127` and `ws://localhost:33127`. The process had no proxy environment variables, and retrying with proxy variables unset did not change the result. The SDK did not reach signin, namespace/database selection, migration, or fixture setup. The failure message exposes only a safe stage label; raw SDK error text is not recorded.

The live cause remains unknown. Do not treat CLI success or a raw WebSocket upgrade as proof that the Rust SDK client can use this endpoint. No compatibility database was created by the failed attempts.

## Remote latency capture

`remote_latency_capture` was not run. Both planned report paths were absent and remain unused:

- `/tmp/memory-triple-latency-docker-default-20261114.json`
- `/tmp/memory-triple-latency-memory-512m-20261114.json`

Do not reduce the prescribed sample count to work around setup cost. Fresh mode requires about 180 database setups per resource-mode run. Resume only after the Rust SDK compatibility gate passes and the effective Docker resource settings are verified for each mode. Run each mode separately with all five `MEMORY_TRIPLE_TEST_*` variables and a distinct report path, as shown in `docs/superpowers/plans/2026-11-14-assemble-context-latency.md`.

The baseline harness accepts only `MEMORY_TRIPLE_TEST_ASSERT_SPEEDUP=0`. It rejects `1` until Task 3 implements candidate-query capture. The report schema is `memory-triple-latency/v1`; it permits only the fields defined in Spec B. Do not add SQL, bound values, fact IDs, raw rows, credentials, tenant identifiers, or request labels.

## Cleanup

The disposable container and its anonymous Docker volumes can be removed with:

```bash
docker rm --force --volumes d4dec2ff13a43a35689eb256ddf6dabb9b875917733f16a271022356a1e332a5
```

Do not remove any container unless its full ID and image have been re-verified. The unrelated service previously observed on port `18081` is out of scope; never connect to it.
