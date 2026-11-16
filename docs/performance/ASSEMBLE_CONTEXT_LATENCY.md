# assemble_context latency runbook

## Current protocol: descriptive v4 baseline

The user approved replacing the calibration experiment with a descriptive baseline and removing additional work that does not decide whether a guard is useful. Spec B is authoritative. Measure empty-table, populated-no-match, and positive-match at limit 40 on a reused connection: five warmups, twenty timed queries. Keep all raw durations, median/min/max, stable row digests, and one SQL call per measurement. No new-connection timing, calibration, confidence intervals, timer-resolution probe, speedup flag, quiescence gate, or host/Engine monitoring is used.

Safety remains unchanged: disposable local disk-backed SurrealDB 3.3.5, immutable image identity, pinned local Docker endpoint, matching loopback binding, RocksDB/resource preflight, bounded commands/setup/queries, create-new reports, generated-database cleanup, and no production mounts or requests. Run docker-default and memory-512m separately. The latter has exactly 536870912 bytes of memory and memory-swap limits; both modes leave CPU unrestricted. Record unavailable modes explicitly rather than silently changing settings.

Required variables are MEMORY_TRIPLE_TEST_URL, MEMORY_TRIPLE_TEST_IMAGE_ID, MEMORY_TRIPLE_TEST_RESOURCE_MODE, and MEMORY_TRIPLE_TEST_REPORT. The ignored test command is:

```sh
cargo test -p memory_mcp --features eval-support --locked --test retrieval_latency_remote remote_latency_capture -- --exact --ignored --test-threads=1
```

Supply all four variables with a freshly inspected disposable endpoint/image and a unique report path. Use the same inputs with remote_triple_compatibility for correctness verification; it does not write a report. Record the actual values and outcomes below. Report v4 contains three records with twenty durations each. A completed descriptive capture is not a calibrated performance result, a speedup, or a production-p99 claim. High spread remains visible and does not fail correct results. Incomplete/incorrect results remain failures.

For an optional candidate, preserve the semantic/read-view gates in Spec C. Compare A/B on one prepared database and reused connection with alternating acquisition order. Require a clear material benefit and no clear positive-case regression; inconclusive results leave the guard out.

## Current results

Task 2 is complete under the user-approved descriptive protocol, with a completed docker-default report and an explicitly unavailable memory-512m mode. Historical v1–v3 reports below retain their original INVALID classifications and are not reinterpreted under v4.

The fresh disk-backed server used SurrealDB 3.3.5 image `sha256:f40e356e40dd2094e5a743ad8b1353f646cf3a660541e92b9f6cce003d3be2dd` on native arm64, with RocksDB at `/tmp/surrealdb`. Its loopback port was 33133. Live preflight verified no explicit memory, swap, or CPU restriction. Compatibility passed in 6.46 seconds; the descriptive capture passed in 20.66 seconds. No host or Engine monitor ran alongside the capture.

| Scenario | Minimum (ms) | Median (ms) | Maximum (ms) |
|---|---:|---:|---:|
| Empty table | 0.345875 | 0.373979 | 0.491167 |
| Populated, no match | 189.194000 | 191.387104 | 202.467666 |
| Positive match | 192.586458 | 194.404000 | 200.016917 |

All three records preserve twenty raw observations and twenty SQL calls, stable row digests, and independently expected result counts. Timings describe this synthetic fixture on a shared local OrbStack host. They do not prove production latency, p99, statistical significance, or a guard speedup. The no-match cost is substantial enough to consider a candidate under Spec C; the empty-table path is already inexpensive.

The separately provisioned memory-512m container used loopback port 33134 with exactly 536870912-byte memory and memory-swap limits and unrestricted CPU settings. Its compatibility test failed during a public triple lookup; generated-database cleanup then exceeded its thirty-second deadline. Docker reported `OOMKilled=true`, zero restarts, and the exact requested limits. The full compatibility test failed after 36.21 seconds. No baseline capture or report was produced for that mode, and no timing or production-capacity conclusion is supported. Limits and fixture were not silently changed and the run was not retried. The disposable container and its own volumes were removed, releasing the failed fixture state.

The v4 baseline report, candidate A/B report, historical v3 and tmpfs reports, transport probes, diagnostic logs, and private plan scratch were removed during merge preparation on November 16, 2026. `target/` build output was also removed. The narrative results above remain, but the raw artifacts can no longer be independently rechecked. Their SHA-256 values, calculated before deletion, are retained below as identifiers only; a digest cannot reproduce or independently validate a missing file. Both fresh containers had already been removed; this cleanup did not touch Docker containers, production data, or the separate `.tmp-investigate/` rollback artifacts.

| Removed artifact | SHA-256 |
|---|---|
| `baseline-v4-docker-default-20261116-01.json` | `61513631f7324e6d37b8a98adbb30ca0b9401708de02dbc4981d147f58ed7689` |
| `candidate-ab-docker-default-20261116-01.json` | `ecef5346daab5a7c7fbde981968eb8a0328879db13650f9385c7e86b6d9dfa57` |
| `baseline-v3-docker-default-20261116-01.json` | `ec9ca0adef7b25ca6e919000f5eb4eaf449db4fb355db7a6d4d0b3f63d6a3c72` |
| `baseline-tmpfs-default-20261116-01.json` | `10ef505ce86f5e2b5edc62d6f88bde6b2ed431fc0852e57401c76945678c5784` |
| `ws-probe-host-20261116-01.json` | `21fb66542e33835a4bad430d4f0d7938b2d9b0a34e35d5fa4d7aaa3164d91411` |
| `ws-probe-linux-20261116-01.json` | `263eadbf71c2d14eb1e804601d30ca1c38dd844fd5523f4076a77baa85694a03` |
| `transport-probe-20261116-01.json` | `bb56ebdd176a76298a12859e33d22b76d556cb3b986aaac89ffffc6478739fe0` |
| `baseline-v3-docker-default-20261116-01-host.jsonl` | `a00e7d8a0475844d64535ec60f80fcc27eaa5e33e18026e82be810f1d7070453` |
| `baseline-v3-docker-default-20261116-01-engine.jsonl` | `f3cda83183977eaf24be7c945f6baa6535c506db730725529eec7f39d59999e5` |
| `host-io-tmpfs-default-20261116-01.jsonl` | `d14f1d5a704c72e00219187fc2a45cabecda3a43f283dc72aedb3a3c5eb5e1fc` |
| `engine-io-tmpfs-default-20261116-01.jsonl` | `1ac9d35c069e83cadb33016ee2162109e03b86e8a1fd7973a86b57363edbe4a6` |

The older v1/v2 report paths named later in this historical runbook are also no longer retained. Their recorded summaries and INVALID classifications remain historical only.

Validation: new descriptive-sample and four-variable configuration tests were observed RED before implementation, then GREEN. The ordinary harness target passed 35 tests with two live tests ignored; triple contracts passed 21 tests. `cargo test -p memory_mcp --locked`, required workspace Clippy with `fs-watch,mcp-apps,streamable-http,test-fixtures,eval-support` and `-D warnings`, formatting, and diff whitespace checks passed. Full crate output was recorded in `crate-tests-v4.log`, which was removed during merge preparation; only this summary remains. The failed memory-512m compatibility test is an unavailable-resource result, not a passing live test.

## Optional guard evaluation (November 16, 2026)

The optional guard candidate was evaluated only against the disposable local SurrealDB 3.3.5 RocksDB server. Its read-view semantics were checked against immutable source commit [`5c3383fe1841e10f7fc67fdf41b6d0671cbc1b13`](https://github.com/surrealdb/surrealdb/tree/5c3383fe1841e10f7fc67fdf41b6d0671cbc1b13): [the read-only statement transaction](https://github.com/surrealdb/surrealdb/blob/5c3383fe1841e10f7fc67fdf41b6d0671cbc1b13/surrealdb/core/src/dbs/executor.rs#L1554-L1607), [deferred IF/ELSE branch evaluation in the same execution context](https://github.com/surrealdb/surrealdb/blob/5c3383fe1841e10f7fc67fdf41b6d0671cbc1b13/surrealdb/core/src/exec/operators/ifelse.rs#L109-L160), and [RocksDB snapshot-backed transactions/read options](https://github.com/surrealdb/surrealdb/blob/5c3383fe1841e10f7fc67fdf41b6d0671cbc1b13/surrealdb/kvs-rocksdb/src/lib.rs#L796-L850). Both published crates' `.cargo_vcs_info.json` files identify this same commit and their `path_in_vcs` locations. The source path shows that a statement opens one read transaction; `IF` evaluates the condition and only the selected branch in the same cloned execution context; RocksDB enables a snapshot for that transaction. This supports a shared statement read view for this exact engine/version, not for other backends or versions.

Candidate compatibility first returned a RED result because one fixture case had an inconsistent expected fact-ID list; the test oracle was corrected to use `predicate-match` / `fact:triple-fixture-0001`. The rerun passed against the real disposable server: 1 passed in 7.44 seconds. It checked raw and public flat-array results, malformed output rejection, skipped and entered failing branches, missing-table behavior, unchanged eligible rows/projection, and exactly one public SQL call. This was a test-expectation correction, not a production SQL defect.

The paired A/B capture used the same prepared database and `SurrealDbClient` for both queries, five warmups per case, and twenty alternating AB/BA pairs per case at limit 40. The disposable endpoint was `127.0.0.1:33362`, image `sha256:f40e356e40dd2094e5a743ad8b1353f646cf3a660541e92b9f6cce003d3be2dd`, resource mode `docker-default`. All 80 timed lookups returned the independently expected rows and issued one SQL call each. The raw report was removed during merge preparation; its recorded SHA-256 is `ecef5346daab5a7c7fbde981968eb8a0328879db13650f9385c7e86b6d9dfa57`, but its contents can no longer be independently rechecked.

| Case / order | Baseline median (ms) | Candidate median (ms) | Candidate / baseline |
|---|---:|---:|---:|
| Populated no-match, AB | 199.996 | 0.807 | 0.0040 |
| Populated no-match, BA | 200.564 | 0.569 | 0.0028 |
| Positive match, AB | 210.100 | 222.929 | 1.0611 |
| Positive match, BA | 213.843 | 221.025 | 1.0336 |

The candidate clearly exceeds the no-match improvement target in both orders. For positive matches, candidate medians are 3–6% higher in both orders, while paired distributions overlap and are noisy (median of the per-pair B/A ratios—not the ratio of the medians above—was 1.0426 AB and 1.0132 BA; 8/10 and 7/10 pairs respectively were slower). The data therefore does not establish either a material regression beyond baseline variation or non-regression. Under Spec C's conservative rule, the comparison is inconclusive and **the production guard is not added**. The comparison covers only `docker-default`; `memory-512m` remains unavailable after its OOM-killed compatibility run, and no result is generalized to it. No production retrieval/mutation request, deployment, or production-data operation occurred during this evaluation.

Task 4 is not justified: there is no recurring application-level latency result to localize. The retrieval-quality evaluation was invalid because its real provider was unavailable; the weekly perf smoke is not evidence of a recurring assemble-context latency problem. No stage metrics or metrics tests were added.

## Final local validation (November 16, 2026)

After keeping the optional SQL candidate in test-only code, strict Clippy found that its query builders and triple-count constant lived in a support module also compiled by the independent contract-test target. They were moved into the remote experiment module that uses them; no production code changed. The ignored candidate-compatibility test passed again against the disposable server: 1 passed in 7.99 seconds.

`cargo fmt --all --check` and `git diff --check` passed. Focused validation passed: `retrieval_latency_remote` 35 passed / 4 ignored and `triple_retrieval_contract` 21 passed. `cargo test -p memory_mcp --locked` passed across 155 result groups (2,607 passed, 0 failed, 42 ignored); all eight doctests passed. Required workspace Clippy with `fs-watch,mcp-apps,streamable-http,test-fixtures,eval-support` and `-D warnings` passed. No production store query, deployment, or production data was changed by the optional-guard investigation.

## Historical audit (superseded protocols)

Everything below is retained as historical evidence, including its then-current instructions and status labels. Those commands, calibration gates, diagnostic requirements, and completion restrictions are not the current protocol. Only the v4 section above and Spec B govern new baseline runs. Production backup and rollout records remain historical facts with their stated provenance and limits; this simplification performs no production operation.


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

**Latest, November 16, 2026:** Task 2 is resumed. The user approved the fixed-101-pair v3 interval criterion below. The first disk-backed v3 docker-default report is INVALID: 20/24 condition intervals pass, but all four empty-table/new-connection intervals fail. The calibration implementation is verified; the full baseline acceptance gate remains unsatisfied. Historical v1/v2 reports keep their original INVALID status. No memory-512m or candidate run followed.

The corrected triple query passes its local retrieval contract, and the ignored remote compatibility test passed twice against the disposable SurrealDB 3.3.5 image. The user deferred Task 2; this does not validate its baseline gate. Both baseline reports are invalid. The `v1` run exceeded the 3% limit in 20 of 24 A/A records, with maximum noise 0.4195653025770974. The revised `v2` run exceeded it in 16 of 24 records, with maximum noise 0.3192349170251424. The v2 run used one prepared database per case and seeded each fixture once. Host disk activity coincided with the run, but the logs do not identify its source or link it to individual query samples. Task 2 remains incomplete. Do not run another latency capture until the variability source is identified and removed or the environment is isolated to control it. The next report path is reserved. A later capture must use timestamped host and Docker I/O logs aligned with per-query sample intervals, outside the report allowlist. Do not run `memory-512m`, evaluate the guard, add stage metrics, or lower the 3% threshold while the baseline is invalid.

Application-only rollout completed on November 16, 2026, over SSH port `2288` on the corrected production host; the previously used address was incorrect. The host is Docker `linux/x86_64`, with Compose services `memory_mcp` and `surrealdb` and app image reference `ghcr.io/like-a-freedom/memory_mcp:latest`. The database bind mount is `/opt/memory_mcp/data` to `/data`. A fresh cold backup from this host is recorded above. The previous app image was preserved under `ghcr.io/like-a-freedom/memory_mcp:rollback-pre-triple-select-20261116`, and the rollback tag was verified to resolve to the prior image. The Compose image reference was retagged to candidate image `sha256:95fad476c179b40e2102cfe41b9a0d39cd1c3fb853d144c574ff661c025f81cc` before running the app-only `up -d --no-deps --force-recreate --pull never --no-build memory_mcp` command. After deployment, the app uses the candidate image; SurrealDB's container and image identities are unchanged, and it remains healthy. `/memory/health/live` and `/memory/health/ready` with the configured Host returned HTTP 200 before and after deployment. Exact container IDs are retained in the private ledger. No rollback was needed. No production retrieval or mutation request was sent; this confirms deployment and health, not live retrieval behavior.

## Resumed Task 2 diagnostic (November 16, 2026)

The user requested completion of Task 2, superseding its deferral. A new disposable loopback container used the same immutable arm64 SurrealDB 3.3.5 image and RocksDB engine, with unrestricted container resources. Its database path was mounted on an 8 GiB `tmpfs` to remove the database's disk-write path as a source of variation. This is a diagnostic storage configuration, not production-disk performance evidence. Existing containers and production data were untouched.

The capture ran with timestamped host `iostat`, Docker Engine CPU/memory/block-I/O samples, and per-query timing markers. It completed all 24 sample records and all 24 calibration records, but 18 calibration records exceeded 3%; maximum noise was 0.33715632965165676. The ignored `remote_latency_capture` test correctly exited with failure. The report is `INVALID` and does not satisfy the baseline gate. Sample latency ranges were 0.290–0.731 ms for empty tables, 181.139–216.499 ms for populated no-match, and 148.159–198.509 ms for positive matches; these ranges are diagnostic only.

The logs contain 420 query timing events and 45 Engine samples. Container block writes remained zero; cumulative block reads increased during parts of the run. Nine noisy A/A pairs were bracketed by unchanged Engine block-I/O counters. One populated no-match, limit-3, reused-connection pair took 208959166 ns and 191381542 ns, yielding 0.08411989929171138 noise with unchanged bracketing block counters. This establishes that additional database block I/O is not necessary for the observed variation. It does not identify whether scheduling, virtualization, network transport, runtime work, or another factor caused that variation. Peak reported container memory was 1322520576 bytes; this was not a `memory-512m` run.

The report `baseline-tmpfs-default-20261116-01.json`, capture log, host/Engine JSONL diagnostics, and bounded `capture-diagnostic.py` driver were removed during merge preparation. The report's pre-deletion SHA-256 is recorded in the current results table; the report used create-new semantics. The new container was stopped and removed with its anonymous volumes after the capture. This v3-era statement that no candidate experiment followed the invalid result is historical and is superseded by the later v4 and optional-guard sections.

Task 2 is resumed but incomplete. A controlled environment must pass the existing calibration gate before Tasks 3 and 4 can consume the timing evidence. Repeated attempts until one passes, discarding outliers, or relaxing the 3% threshold would not satisfy the current contract. The ordinary harness target still passes 39 tests with two live tests ignored.

### Transport and server-time probes

A subsequent bounded diagnostic used a separate disposable SurrealDB 3.3.5 container and the trivial `RETURN 1` query. It did not use the Rust SDK, public store, or baseline fixture and cannot replace the baseline. The identical JSON WebSocket probe ran from macOS through the published port and from an arm64 Python container sharing the server's Linux network namespace. Each probe ran 210 queries on one authenticated connection, with the first ten classified as warmup and every remaining observation retained.

| Path | Median elapsed time | Pairs above 3% | Maximum pair noise |
|---|---:|---:|---:|
| macOS published port | 161875 ns | 75/100 | 0.18833569739952719 |
| Linux network namespace | 34916.5 ns | 68/100 | 0.25363007192292036 |

SurrealDB-reported SQL time also varied: median 10021.5 ns on the macOS probe, with 88/100 server-time pairs above 3%; median 5834 ns on the Linux probe, with 32/100 pairs above 3%. Thus bypassing the published-port path reduces typical round-trip cost but does not make every short-query pair satisfy 3%. The measurements do not establish the cause of the full fixture's 150–200 ms variability. They show that relocating the client alone is insufficient to guarantee the current maximum-single-pair criterion.

The WebSocket and HTTP probe outputs and their helper scripts were removed during merge preparation; their report hashes are listed in the current results table. The HTTP timings are not interchangeable with WebSocket timings. The Python helper image was `python:3.13-alpine`, resolved to manifest digest `sha256:9ef1a599f863231c3d91e941968eb8a0328879db13650f9385c7e86b6d9dfa57`, explicitly run as arm64. The probe container and its anonymous volumes were removed. No production access, SDK change, calibration-threshold change, or additional baseline capture occurred.

## Approved v3 calibration and first result

The user approved a fixed-sample equivalence protocol after reviewing the transport diagnostics. Each condition retains exactly 101 A/A pairs; acquisition alternates A/B then B/A, while report pairs remain `[A,B]`. Sort the positive B/A ratios and take order statistics 41 and 61 (zero-based indices 40 and 60). Under independent identically distributed continuous pairs, this is an exact 95.395593% confidence interval for the median ratio; ties are conservative. Accept only a complete interval inside `[0.97, 1/0.97]`. The 3% margin remains unchanged; maximum single-pair noise is preserved as a descriptive statistic. Zero durations and incomplete pair sets are invalid. There is no adaptive pair count, outlier filtering, or automatic retry. Serial dependence and host drift remain limitations of the statistical interpretation.

The first v3 capture used a new disk-backed local arm64 SurrealDB 3.3.5 container, immutable image `sha256:f40e356e40dd2094e5a743ad8b1353f646cf3a660541e92b9f6cce003d3be2dd`, RocksDB under `/tmp/surrealdb`, and loopback port 33131. Preflight verified zero memory/swap/CPU limits and an empty CPU set. Timestamped host and Engine diagnostics ran alongside the test-local query markers. No compilation, crate test suite, or Clippy job ran concurrently with the capture. This is a shared local OrbStack host, not a dedicated machine or production timing environment.

The ignored capture ran for 701.25 seconds and exited 101 because four interval gates failed. Its raw report recorded all 2424 calibration pairs and 120 descriptive samples; that report and the associated timing logs were removed during merge preparation. Every record had 101 pairs; every sample record had five values and five query calls. The 4848 calibration timing events independently confirmed alternating acquisition order for every condition. Twenty intervals passed, including all populated-no-match and positive-match conditions in both connection modes, and all reused-connection empty-table conditions. The failing intervals were:

| Case | Limit | Connection mode | Median B/A interval |
|---|---:|---|---|
| empty_table | 0 | new_connection | [0.9476911785298676, 1.0125797221646278] |
| empty_table | 1 | new_connection | [0.9686566285313766, 1.0231057785570066] |
| empty_table | 3 | new_connection | [0.9685234822331849, 1.0287707971712716] |
| empty_table | 40 | new_connection | [0.9668738379814077, 1.0092882384393391] |

Each failing interval includes one and extends below 0.97; these are imprecise equivalence estimates, not proof of directional bias. The report remains INVALID overall. The descriptive sample ranges were 0.299–0.796 ms (empty table), 180.548–198.769 ms (populated no-match), and 152.551–202.280 ms (positive match). Those ranges do not establish production performance or p99. Do not treat 20 passing cells as permission to bypass the four failed cells.

The v3 report, its `.log`, host/Engine JSONL diagnostics, and bounded `capture-v3.py` driver were removed during merge preparation; the report and diagnostic hashes are listed in the current results table. The capture used create-new paths and an 1800-second process-group deadline. The container had no OOM or restart and was stopped and removed with its anonymous volumes after the capture. Other containers and production data were untouched. The memory-512m mode was not started.

The implementation passed RED→GREEN calibration regressions, the ordinary harness target (45 passed, two ignored), triple contracts (21 passed), the full production-crate suite, required workspace Clippy with eval-support and `-D warnings`, formatting, and diff whitespace checks. A fresh subscribed-model reviewer independently recomputed all intervals and descriptive maxima from the raw pairs and found no actionable findings. The review confirmed the overall INVALID classification. The remaining work is obtaining precise empty-table/new-connection measurements under a controlled environment change, not changing the acceptance rule after seeing this result.

## Docker endpoint consistency

Before daemon inspection or SDK/database setup, the harness resolves one local Unix Docker socket from `DOCKER_HOST`, an explicit `DOCKER_CONTEXT`, or the Docker CLI's selected context. It rejects simultaneous `DOCKER_HOST` and `DOCKER_CONTEXT` overrides and rejects remote endpoints before inspecting the daemon. The context lookup clears both override variables. Every later Docker CLI call clears both variables and passes `--host` for the resolved endpoint; the stats request uses the socket path derived from that same endpoint. This prevents the image/container preflight and stats monitor from querying different daemons.

Fake-CLI regression tests verify rejection and endpoint pinning. A read-only check of the available Docker CLI resolved its selected context to `unix:///var/run/docker.sock`. These checks establish endpoint selection only. They do not validate timing or unblock the deferred baseline capture.

## Container quiescence diagnostic

The compatibility-only diagnostic run started fixture setup at 18:10:20.568 UTC and completed it at 18:10:25.217 UTC, about 4.65 seconds later. Before the first query, the harness sampled exact Docker Engine CPU and block-I/O counters through the local Unix socket. It requires five consecutive intervals at or below 1% container CPU and 1 MiB/s combined container I/O, samples at least 500 ms apart, and fails after 60 seconds. The live check passed five quiet intervals of about 2.0 seconds each. Container CPU deltas ranged from 0.7 to 4.0 ms; reads were zero and writes ranged from 0 to 24 KiB.

The gate did not observe host-wide activity. Host `iostat` recorded 430.32 MB/s during fixture setup at 18:10:21.678 UTC and 50.94 MB/s at 18:10:36.803 UTC, around gate completion. The source remains unidentified, and these observations do not establish that host I/O caused the invalid A/A calibration. The gate was container-only evidence, not proof of a quiet or isolated host. Its timestamped diagnostic logs were removed during merge preparation; this summary is no longer backed by locally retained raw logs.

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

The original `remote_latency_capture` wrote `/tmp/memory-triple-latency-docker-default-20261115.json` and completed 24 calibration records plus 24 sample records. Twenty calibration records exceeded the 3% limit; four passed: `populated_no_match` limits 1 and 40 in the old `warm` mode, and `positive_match` limits 1 and 40 in the old `warm` mode. Maximum noise was 0.4195653025770974. This run is `INVALID`; its raw report is no longer retained, so the recorded summary cannot be independently rechecked.

Each old sample contained five elapsed values and five SQL calls. Timer resolution was 41 ns. Query durations ranged from 0.368 to 1.060 ms for `empty_table`, 196.402 to 279.641 ms for `populated_no_match`, and 162.158 to 274.459 ms for `positive_match`. These values do not support timing conclusions. After the 866.29-second run, Docker reported approximately 5.3 GiB of memory use and 105 GB of cumulative block I/O. The old harness created and seeded 10 databases for A/A pairs and five more for timed samples for each fixture-backed case/limit. This could have caused RocksDB I/O interference, but no concurrent I/O or compaction trace was captured. Warm-mode calibration also failed, so the cause remains unconfirmed.

### Revised `v2` capture

The revised harness prepares one database per case, seeds each fixture once, and performs no database creation, fixture seeding, or cleanup between measured queries. `new_connection` opens a fresh SDK connection to the prepared database before each query; connection setup is excluded from the query timer. `reused_connection` uses one connection and performs five untimed calls before calibration and timed samples. These names describe connection handling only. Neither mode establishes cold or warm server-cache state. Reports use schema `memory-triple-latency/v2` and the allowlisted `connection_mode` field. The complete schema is specified in the design contract.

The revised `docker-default` capture wrote `/tmp/memory-triple-latency-docker-default-20261115-connection-modes-01.json`. It completed 24 calibration records and 24 sample records. Sixteen calibration records exceeded 3%; eight passed. Every `empty_table` calibration exceeded the gate. Maximum noise was 0.3192349170251424 for `empty_table`, limit 0, `new_connection`. The timer resolution was 83 ns. Timed samples returned stable row digests and each sample made five SQL calls. Timing values are diagnostic only because calibration failed.

The monitor recorded Docker stats from approximately 113 GB read / 92.5 GB written at start to 113 GB read / 92.9 GB written near the end, with memory briefly reaching about 12.05 GiB and CPU near 100% during parts of the run. Host `iostat` recorded up to 53,331 transactions/s and 321.26 MB/s during the capture. A post-run sample ranged from 0.67 to 9.20 MB/s over six intervals. The process snapshot did not identify the disk-I/O source. The monitoring logs lack timestamps aligned to individual query samples, so the evidence shows concurrent activity but does not establish causality. The run lasted 80.92 seconds, compared with 866.29 seconds for the `v1` capture. Docker's rounded cumulative write counter increased by about 0.4 GB rather than 105 GB. Preparing one database per case removed repeated fixture seeding between observations, but did not produce valid calibration.

The historical disposable container was `d4dec2ff13a43a35689eb256ddf6dabb9b875917733f16a271022356a1e332a5`, running image ID `sha256:f40e356e40dd2094e5a743ad8b1353f646cf3a660541e92b9f6cce003d3be2dd`, publishing `127.0.0.1:33127`, with RocksDB at `/tmp/surrealdb`; inspection confirmed no explicit memory, swap, or CPU limits. The v1/v2 reports and their raw files are no longer retained. The report path `/tmp/memory-triple-latency-docker-default-20261115-connection-modes-02.json` was reserved historically and was not used. The calibration procedure and restrictions in this subsection are superseded by the current v4 protocol above.

Do not reduce the sample count or raise the 3% threshold. The baseline harness accepts only `MEMORY_TRIPLE_TEST_ASSERT_SPEEDUP=0`; it rejects `1` until Task 3 implements candidate-query capture. Do not add SQL, bound values, fact IDs, raw rows, credentials, tenant identifiers, or request labels to the report.

## Cleanup

The current disposable container must remain running; do not remove it as part of this work. For a future explicitly approved cleanup, the container and its anonymous Docker volumes can be removed with:

```bash
docker rm --force --volumes d4dec2ff13a43a35689eb256ddf6dabb9b875917733f16a271022356a1e332a5
```

Do not remove any container unless its full ID and image have been re-verified. The unrelated service previously observed on port `18081` is out of scope; never connect to it.
