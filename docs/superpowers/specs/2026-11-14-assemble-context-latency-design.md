# `assemble_context` latency: design and acceptance contract

Status (November 16, 2026): the user approved a descriptive baseline and a second KISS/YAGNI simplification pass. Section B is the current v4 protocol. Calibration, confidence intervals, new-connection timing, and host/Engine monitoring are retired. Historical v1–v3 results remain INVALID under their original protocols; they are not reclassified. Raw experiment reports and diagnostics were removed during merge preparation; recorded summaries and SHA-256 identifiers remain in the runbook, but missing raw files cannot be independently rechecked. No new production deployment, retrieval request, or mutation is part of this work.

## Completion and conditional follow-up — 2026-11-24

The triple-projection correction and descriptive v4 baseline are complete. The [runbook](../../performance/ASSEMBLE_CONTEXT_LATENCY.md) owns the results and limitations. The measured `docker-default` mode completed. The 512 MiB mode was OOM-killed and remains unavailable. Raw experiment reports were removed during historical merge preparation, leaving narrative summaries and digest identifiers.

The November 16 guard trial was inconclusive. The November 24 native
run with SDK 4.0.1 reproduced a 210 ms populated no-match median,
qualified the guard, and verified the production path against the frozen
unguarded SQL. The short-circuit is implemented and its scope is limited
to the validated query and storage path. Stage instrumentation remains
triggered by an application-level observation under Section D.
[Raw samples and current results](../../performance/ASSEMBLE_CONTEXT_LATENCY.md#sdk-401-follow-up-on-apple-silicon--2026-11-24)

## Goal and scope

Fix the confirmed positive triple-retrieval defect, measure the corrected unguarded query, and consider an empty-path guard only when the corrected baseline shows a material cost and the guard passes the compatibility checks below. Add stage metrics only when application-level measurements leave a recurring latency question that those metrics can answer.

The store API, schema, temporal visibility, access policy, ranking, search budgets, and response shape remain unchanged. Production verification requires a checked backup and an explicit rollback procedure before deployment. No dependency change, migration, generated-code change, MCP tool, or application-facing API change is included.

## Evidence and limits

Investigation used production observations from November 13, 2026 UTC. The application was `v1.38.0` and the reported SurrealDB server version was `3.3.5`. The inspected application source matched that tag.

| Observation | Evidence and limit |
|---|---|
| Slow calls | Calls began at 15:52:35, 17:35:06, 17:36:29, and 17:44:52 UTC. Durations were 9501, 5632, 9347, and 4778 ms; budgets were 10, 8, 15, and 8; result counts were 10, 8, 3, and 8. Four calls do not establish p99. |
| Serialization and query-log writes | `assemble_context.done` preceded serialization and nearly matched operation duration. Runtime `QUERY_LOGGING_ENABLED=false`. |
| Database work | The first call's two-minute window showed DB CPU +6.83 seconds and application CPU +0.18 seconds. These are window aggregates, not a request trace. |
| Empty triple lookup | The inner lookup returned no rows in 1.38 ms. The nested fact lookup returned no rows in 1516 ms with `IndexScan fact_t_valid` followed by filtering. A known-empty ID array used `EmptyScan` in 0.65 ms. |
| Positive triple defect | On the inspected SurrealDB 3.3.5 memory-engine fixture, the current nested `SELECT source_fact_id` projection returned no matching fact. Replacing only that projection with `SELECT VALUE source_fact_id` returned the independently expected fact. This establishes the defect on that fixture, not on every production backend. |
| Other reads | Materializing 2151 visible facts took 1.23 s and returned 3.27 MB without vectors. A graph lookup with original seed IDs reached a 2 s timeout; this was not a full graph-stage replay. |
| Lexical fan-out | Reconstruction of the first saved query produced 16 terms, 44 fallback queries, and one alias variant. The first set took about 1.1 s and returned 2.4 MB before deduplication. |
| ANN over-fetch | Budget 10 became channel limit 40, then K160/EF640. A representative stored vector took 1.1 to 1.6 s at K160 and 68 ms at K5/EF20. This was not the original embedding or a relevance comparison. A forced index hint changed results. |
| Memory pressure | The DB limit was 512 MiB, with approximately 208 to 216 MiB swap during calls and thousands of major faults. A later cgroup OOM at 21:13 UTC does not prove the cause of each earlier spike. |
| Disposable remote compatibility | On November 15, 2026 UTC, `remote_triple_compatibility` passed against the loopback SurrealDB 3.3.5 image after fixing SDK URL handling and configuring the eval-only bound client for the fixture's 2048-dimensional vectors. The harness records the server version from the validated Docker image tag and immutable image ID; SurrealDB 3.3.5 did not provide a usable runtime version through the attempted database-info query. This verifies the expected image identity, not the binary's version through a server API. |
| Corrected unguarded baseline, `v1` | The `docker-default` report at `/tmp/memory-triple-latency-docker-default-20261115.json` contains 24 calibration records. Four met the 3% limit; 20 exceeded it. Maximum noise was 0.4195653025770974. The report also contains 24 sample records with five elapsed values and five SQL calls each; timer resolution was 41 ns. Timing values are diagnostic only. |
| Corrected unguarded baseline, `v2` | The revised `docker-default` report at `/tmp/memory-triple-latency-docker-default-20261115-connection-modes-01.json` contains 24 calibration records. Eight met the 3% limit; 16 exceeded it. Maximum noise was 0.3192349170251424 for `empty_table`, limit 0, `new_connection`; timer resolution was 83 ns. It contains 24 sample records with five elapsed values and five SQL calls each. Host `iostat` peaked at 53,331 transactions/s and 321.26 MB/s; Docker's cumulative block I/O increased by about 0.4 GB. The available logs are not timestamp-aligned with individual query samples, and the process snapshot did not identify the I/O source. These observations show concurrent activity, not its cause or effect on query timings. Timing values are diagnostic only. |
| Container quiescence diagnostic | A later compatibility-only run added a test-local gate that samples Docker Engine counters over the local Unix socket. It passed five consecutive intervals of about 2.0 seconds each at or below 1% container CPU and 1 MiB/s combined block I/O before the first query. CPU deltas ranged from 0.7 to 4.0 ms, reads were zero, and writes ranged from 0 to 24 KiB. Host `iostat` reported 430.32 MB/s during fixture setup and 50.94 MB/s at gate completion. The gate covers container activity only. These host samples do not identify the I/O source or establish that it caused the calibration noise. |

Separate SQL probes cannot be added into a historical request profile. Keep the historical summaries and their limitations, but do not describe the raw reports as preserved: those files and the private plan workspace were removed during merge preparation. The runbook retains the reported SHA-256 identifiers, but the missing files cannot be independently verified. The failed calibration motivated the user-approved change of protocol; v4 does not claim calibrated precision, production latency, or p99.

Before daemon inspection or SDK setup, resolve one local Unix Docker endpoint, rejecting conflicting DOCKER_HOST/DOCKER_CONTEXT overrides and remote endpoints. Clear both overrides in Docker subprocesses and pass the resolved endpoint explicitly with --host. This safety policy remains; Docker Engine stats are no longer collected.

The [query-efficiency contract](2026-10-14-surrealdb-query-efficiency.md) supplies semantic and resource invariants, not its old `3.3.2` baseline or unrelated scheduler work. The [storage materialization contract](2026-10-13-storage-materialization-memory-design.md) prohibits hidden candidate caps before global ranking. This focused experiment does not replace either document's release evidence.

## Global constraints

- Rust `1.99.0`; SurrealDB server and SDK `3.3.5`; current `Cargo.lock`; Cargo commands use `--locked`.
- Do not change dependencies, migrations, generated code, MCP tools, or application-facing APIs. Do not edit `Cargo.toml` or `Cargo.lock`.
- A bound client uses one Active Namespace. Do not add request-level namespace, database, or table selection.
- Production operations are authorized only while preserving data and retaining a checked backup. Use an isolated restored copy for experiments. Ordinary recall updates access counters and is not read-only. Do not infer permission to change schema, delete data, or expose credentials.
- Resolve one local Unix Docker endpoint before daemon inspection. Reject conflicting `DOCKER_HOST`/`DOCKER_CONTEXT` overrides and remote endpoints; pin subsequent Docker CLI calls to that endpoint.
- Preserve FTS terms and per-term limits, alias expansion, ANN K/EF, EUCLIDEAN/F32, admission thresholds, reranking, graph budgets, and ordering behavior.
- Preserve temporal and access filtering, provenance, access tracking, query-log policy, cache defaults, and job cadences. Do not delete facts.
- Keep query ownership in `knowledge` and orchestration in `memory`. Add no business logic to transport, `main.rs`, `service/`, or a shared container.
- Preserve `MemoryError` and existing missing-table handling. Never convert syntax, permission, decoding, or transport failures into successful empty results.
- Controlled ports belong in unit tests. Real databases, network, filesystem, and process-global recorders belong in integration tests. Use bounded waits and permits, not sleeps for ordering.
- New telemetry and artifacts exclude raw queries, SQL literals, memory content, credentials, and request, tenant, or record labels. Use synthetic fixtures and allowlisted output.
- Do not commit, create branches, or modify unrelated user work. Deployment requires a verified artifact, passing functional checks, and a recorded rollback procedure. Plans, specifications, and ADRs for this work must be in English.

## A. Correct triple retrieval

Keep this public interface unchanged:

```rust
KnowledgeStoreClient::select_facts_by_triple(
    &self, query_text: &str, cutoff: &str, limit: usize,
) -> Result<Vec<serde_json::Value>, MemoryError>
```

Change the nested ID projection from `SELECT source_fact_id` to `SELECT VALUE source_fact_id`. Keep the fact projection, triple predicate, shared `BI_TEMPORAL_WHERE`, bound variables, namespace binding, and final `$limit` unchanged. Apply the corrected projection for every limit, including zero. Do not add a guard in this change.

Tests must derive expected fact IDs from seeded fixture literals, not from the SQL helper. A positive subject, predicate, and object match must return its expected source fact. Duplicate triples must not duplicate returned fact rows. A matching triple with a dangling source must return no fact. Quotes and Unicode remain bound data. Preserve the existing projection's omission of `embedding`, provenance, temporal visibility, and missing-table behavior.

## B. Descriptive corrected baseline

Purpose: determine whether the corrected unguarded triple lookup has enough avoidable cost to justify testing a guard. Correctness is established separately by contract tests, including limits 0, 1, 3, and 40. This experiment makes no precision, significance, or tail-latency claim.

Use the frozen corrected SQL through the test-local forwarding DbClient, public KnowledgeStoreClient, and real result conversion. Keep the deterministic fixture: 2191 facts, 2048-dimensional vectors, five triples, and cutoff `2026-11-13T15:52:35.269Z`. Use one disposable database per case and seed it once. Cases are empty table, populated table without a match, and a positive match. Measure only limit 40, on one reused bound connection per case. Run five untimed warmups followed by exactly twenty timed calls. Exclude connection setup, migrations, seeding, serialization, and cleanup. Each timed call must issue one SQL query, return independently expected fact IDs without embeddings, and match the condition's normalized row count/digest.

Keep all twenty durations in acquisition order. Report minimum, median, and maximum; the median is the midpoint of the two middle sorted durations, rounded down to whole nanoseconds. Reject incomplete captures or changed results. Timing variation and large outliers remain visible and do not fail an otherwise correct descriptive capture. No calibration, confidence interval, new-connection mode, timer-resolution probe, speedup flag, monitoring process, or quiescence threshold is required.

The ignored tests read four required variables: MEMORY_TRIPLE_TEST_URL, MEMORY_TRIPLE_TEST_IMAGE_ID, MEMORY_TRIPLE_TEST_RESOURCE_MODE, and MEMORY_TRIPLE_TEST_REPORT. Require a ws URL with explicit port, localhost or a literal loopback IP, and no credentials, query, fragment, or non-root path. Require a full lowercase sha256 image ID and a new report path. The former ASSERT_SPEEDUP variable is no longer read and enables nothing.

Resolve surrealdb/surrealdb:v3.3.5 to an immutable image ID. Before SDK setup, inspect exactly one running container publishing the configured port and verify image identity, matching loopback binding, RocksDB startup, and resource limits. For a literal URL IP require the same published HostIp; localhost accepts a loopback binding. Docker commands have a ten-second timeout and terminate/reap their child on expiry. SDK setup has a thirty-second deadline and each query has a ten-second deadline. Report creation is create-new. Delete only generated fixture databases; preserve cleanup errors. Root/root credentials belong only to the disposable local server.

Run docker-default and memory-512m separately, with fresh disk-backed containers, unique report paths, and no production mounts. The default has no explicit memory/swap limits. The constrained mode sets both to exactly 536870912 bytes. Both require CpuQuota=0, CpuPeriod=0, NanoCpus=0, and empty CpusetCpus; missing or malformed evidence fails closed. Record host/image architecture and emulation. Report the server version as the Docker-validated tag version, not runtime introspection. If a mode cannot complete, retain its failure evidence and record it as unavailable; never fabricate timings or silently change resources. A completed mode supports only its own resource-specific conclusion.

Report schema memory-triple-latency/v4 allows: schema, server_version, sdk_version, image_id, storage_engine, host_architecture, image_architecture, emulation, resource_mode, effective_resources (memory_limit_bytes, memory_swap_limit_bytes, cpu_quota_micros, cpu_period_micros), fixture (fact_count, embedding_dimension, triple_count, cutoff), connection_mode=reused_connection, limit=40, warmup_count=5, sample_count=20, and samples. Each sample record contains case, elapsed_ns, min_ns, median_ns, max_ns, row_count, sha256, and query_call_count=20. Preserve normalized-row SHA-256 digests without raw rows, SQL, bound values, fact IDs, credentials, or tenant/request identifiers.

Task 2 is complete when functional checks pass, available modes have complete descriptive reports, unavailable modes have explicit bounded failure evidence, and the runbook records environment, limitations, and cleanup. Completion does not imply a speedup or authorize a guard.

## C. Optional empty-path guard

Consider a candidate only if the descriptive baseline shows a material no-match cost. If the baseline is already cheap or a comparison would be inconclusive, record guard not justified and leave production code unchanged.

Keep the semantic gates: inspect exact SurrealDB 3.3.5 backend/block transaction sources, cite immutable links, and establish the actual backend's required shared read view. A single RPC or the memory engine's per-transaction snapshot does not prove block consistency. Unknown semantics block the guard.

Before production edits, test the candidate through the same forwarding adapter, public store, fixture, and real decoder. Verify flat empty and positive arrays; distinguish malformed non-array output from an empty result. A deliberately failing fact branch must be skipped when no triples match and propagate its error when entered. Preserve missing-table behavior and all result/provenance/temporal contracts. No shared-decoder changes or two-call fallback.

For performance, compare corrected A with candidate B on the same prepared database and reused connection. Use twenty pairs, alternating AB and BA order, with all raw times and result checks retained. The existing material-improvement target is median(B)/median(A) <= 0.50 for a costly empty/no-match case, supported in both acquisition orders. Inspect the positive-case distributions for a consistent regression beyond baseline variation. If results overlap enough to make the claimed benefit or non-regression unclear, omit the guard. No arbitrary 3% gate, formal significance, or p99 claim is needed. Unavailable resource modes remain unsupported; do not generalize beyond completed modes.

Only after these gates pass, write the deterministic query-policy regression, observe RED, and implement the minimal guard in KnowledgeStoreClient::select_facts_by_triple. Preserve one query, namespace, bindings, corrected projection, errors, and the unguarded limit=0 path. Run contract and remote compatibility tests and repeat the paired comparison after the change. No new configuration flag is needed before a candidate exists.

**Task 3 decision (November 16, 2026):** The read-view and compatibility gates passed for SurrealDB 3.3.5 RocksDB, and the candidate exceeded the no-match speedup target in both acquisition orders. Positive-case candidate medians were 3–6% slower, with overlapping, variable paired distributions; non-regression was inconclusive. Therefore the guard gate did not pass and no production guard or policy test was added. Only `docker-default` was measured; `memory-512m` is unavailable. See the runbook for immutable source links, raw artifact, and detailed results.

## D. Optional stage metrics

Add stage metrics only if the corrected baseline and application-level performance evidence show recurring `assemble_context` latency that current metrics cannot localize. Record the evidence and the diagnostic question before changing instrumentation. If the evidence does not justify the metrics, omit this section's code changes and report the decision.

If justified, reuse `StageTimer` and `memory_pipeline_stage_duration_seconds` with `operation="assemble_context"`. Add only the fixed stages needed to answer the recorded question. The prior proposed vocabulary is:

| Stage | Boundary |
|---|---|
| `lexical_retrieval` | Initial `select_fact_records_for_query` await in `retrieve_fact_channels`. |
| `alias_retrieval` | Alias expansion through its lexical/filter loop, ending before `base_direct_ids`. |
| `triple_retrieval` | `collect_triple_facts`, including decoding. |
| `graph_retrieval` | `collect_graph_facts`. |
| `community_retrieval` | Community collection, including origin-factor reads. |
| `default_postprocessing` | After channel retrieval through `append_source_answers`. |
| `reconciliation` | `attach_reconciliation`. |
| `access_tracking` | Entire `track_fact_accesses` helper, including cache hits. |
| `response_serialization` | `serde_json::to_value(&results)` inside the existing compact guard. |

Retain `query_embedding` and `ann_search` scopes. A timer records once when an entered scope exits by success, error, or future drop. Skipped scopes emit no observation. The selected scopes do not partition the whole operation. No new metric family, dynamic label, payload copy, SQL query, or await is needed.

## E. Evidence and acceptance

Keep these evidence types separate:

- Deterministic contract tests establish the public store result and query-call contract.
- Embedded and remote database tests establish result shape, temporal behavior, missing-table behavior, and error compatibility.
- The descriptive baseline or paired comparison supports only the measured SQL claim on its recorded engine, fixture, and resources. Its report schema and digest rules are defined in section B.
- Existing retrieval-quality tests protect downstream access/type policy and ranking. They do not prove remote latency.
- Existing `memory-perf` is a broad smoke test with a 1.25 p95 ratio threshold. It does not replace the focused SQL experiment or establish production p99.

The core fix is accepted when positive triple lookups return independently expected facts, no-match and dangling-source cases remain empty, the projection and temporal behavior stay intact, and focused and adjacent contracts pass. Complete Task 2 under section B before evaluating the optional guard. Do not reinterpret historical INVALID reports as successful v4 captures.

A guard is accepted only if its backend read-view and decoder/error compatibility are established and the A/B thresholds pass. Stage metrics are accepted only when the runbook records the evidence that justified them and tests prove entered scopes, error/drop behavior, and unchanged response shape.

Runbook `docs/performance/ASSEMBLE_CONTEXT_LATENCY.md` records commands, versions, source references, fixture identity, results, artifacts, cleanup, and unresolved gates. No ADR is needed while the work stays inside the existing knowledge and retrieval boundaries. Any architectural change requires a separate decision.

No production latency improvement or production p99 claim follows from synthetic SQL timings. The user has specifically authorized an application-only rollout of the reviewed fix from release tag `v1.38.0`; this does not authorize a latency experiment, production retrieval or mutation requests, runtime tuning, a database change, or further work on graph indexes, ANN expansion, FTS/alias fan-out, or memory policy.

Before rollout, verify the checked backup, previous application image ID, SurrealDB container identity, Compose service and image reference, target architecture, and staged immutable image ID without exposing secrets. Confirm the Compose image reference is a valid Docker image-tag destination in `name[:tag]` form; digest-only references block this retag-based procedure. Choose a rollback tag that does not already exist and verify it resolves to the previous application image ID after tagging. If the Compose reference is not taggable or the rollback tag is occupied, stop without changing image tags or Compose. Restart only the `memory_mcp` service with `docker compose -f /opt/memory_mcp/docker-compose.yaml up -d --no-deps --force-recreate --pull never --no-build memory_mcp`; do not recreate SurrealDB or touch its volume. Accept health only from `/memory/health/live` and `/memory/health/ready` with the configured Host; `/memory/health` serves UI HTML. If an application health check fails while the SurrealDB container ID remains unchanged, restore the previous application image reference, restart only `memory_mcp`, and verify the prior image ID and unchanged SurrealDB container ID. If the SurrealDB container ID changes unexpectedly, preserve its volume and stop for investigation; do not attempt to repair or restore the database as part of app rollback. If backup, artifact, rollback image, or host access cannot be verified, do not deploy. Record actual evidence in the runbook.
