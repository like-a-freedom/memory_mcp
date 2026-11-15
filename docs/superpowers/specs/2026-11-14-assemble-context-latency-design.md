# `assemble_context` latency: design and acceptance contract

Status: approved for local implementation of the corrected retrieval path and its measurements. The approval does not cover commits, branches, production access, deployment, restarts, or tuning.

## Goal and scope

Fix the confirmed positive triple-retrieval defect, measure the corrected unguarded query, and consider an empty-path guard only when the corrected baseline shows a material cost and the guard passes the compatibility checks below. Add stage metrics only when application-level measurements leave a recurring latency question that those metrics can answer.

The store API, schema, temporal visibility, access policy, ranking, search budgets, and response shape remain unchanged. No production rollout, dependency change, migration, generated-code change, MCP tool, or application-facing API is included.

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

Separate SQL probes cannot be added into a historical request profile. Never copy production text, credentials, tenant identifiers, or raw EXPLAIN output into repository artifacts.

The [query-efficiency contract](2026-10-14-surrealdb-query-efficiency.md) supplies semantic and resource invariants, not its old `3.3.2` baseline or unrelated scheduler work. The [storage materialization contract](2026-10-13-storage-materialization-memory-design.md) prohibits hidden candidate caps before global ranking. This focused experiment does not replace either document's release evidence.

## Global constraints

- Rust `1.99.0`; SurrealDB server and SDK `3.3.5`; current `Cargo.lock`; Cargo commands use `--locked`.
- Do not change dependencies, migrations, generated code, MCP tools, or application-facing APIs. Do not edit `Cargo.toml` or `Cargo.lock`.
- A bound client uses one Active Namespace. Do not add request-level namespace, database, or table selection.
- Production remains untouched. Diagnostic SSH access does not authorize deployment, restart, credential use, or tuning. Ordinary recall updates access counters and is not read-only.
- Preserve FTS terms and per-term limits, alias expansion, ANN K/EF, EUCLIDEAN/F32, admission thresholds, reranking, graph budgets, and ordering behavior.
- Preserve temporal and access filtering, provenance, access tracking, query-log policy, cache defaults, and job cadences. Do not delete facts.
- Keep query ownership in `knowledge` and orchestration in `memory`. Add no business logic to transport, `main.rs`, `service/`, or a shared container.
- Preserve `MemoryError` and existing missing-table handling. Never convert syntax, permission, decoding, or transport failures into successful empty results.
- Controlled ports belong in unit tests. Real databases, network, filesystem, and process-global recorders belong in integration tests. Use bounded waits and permits, not sleeps for ordering.
- New telemetry and artifacts exclude raw queries, SQL literals, memory content, credentials, and request, tenant, or record labels. Use synthetic fixtures and allowlisted output.
- Do not commit, create branches, deploy, or modify unrelated user work. Plans, specifications, and ADRs for this work must be in English.

## A. Correct triple retrieval

Keep this public interface unchanged:

```rust
KnowledgeStoreClient::select_facts_by_triple(
    &self, query_text: &str, cutoff: &str, limit: usize,
) -> Result<Vec<serde_json::Value>, MemoryError>
```

Change the nested ID projection from `SELECT source_fact_id` to `SELECT VALUE source_fact_id`. Keep the fact projection, triple predicate, shared `BI_TEMPORAL_WHERE`, bound variables, namespace binding, and final `$limit` unchanged. Apply the corrected projection for every limit, including zero. Do not add a guard in this change.

Tests must derive expected fact IDs from seeded fixture literals, not from the SQL helper. A positive subject, predicate, and object match must return its expected source fact. Duplicate triples must not duplicate returned fact rows. A matching triple with a dangling source must return no fact. Quotes and Unicode remain bound data. Preserve the existing projection's omission of `embedding`, provenance, temporal visibility, and missing-table behavior.

## B. Corrected unguarded baseline

The baseline is the corrected unguarded query from section A. The broken historical SQL is diagnostic evidence only. Do not compare the candidate against it for retrieval quality or speed.

Use a test-local SQL forwarding adapter around a persistent client so both SQL variants pass through `KnowledgeStoreClient` and the real bound-client result conversion. Do not expose crate-private `query_rows` or reconnect for each measurement. The frozen baseline SQL must be a literal copy of the corrected query and its temporal predicate, independent of production query builders.

The ignored remote experiment accepts only a disposable loopback SurrealDB endpoint. It reads exactly `MEMORY_TRIPLE_TEST_URL`, `MEMORY_TRIPLE_TEST_IMAGE_ID`, `MEMORY_TRIPLE_TEST_RESOURCE_MODE`, `MEMORY_TRIPLE_TEST_ASSERT_SPEEDUP`, and `MEMORY_TRIPLE_TEST_REPORT`; missing or invalid values fail before connection. The URL must use `ws`, include an explicit port, contain no credentials, query, or fragment, and use `localhost` or a literal loopback IP. `MEMORY_TRIPLE_TEST_IMAGE_ID` must be a full `sha256:` image ID. `MEMORY_TRIPLE_TEST_RESOURCE_MODE` must be `docker-default` or `memory-512m`. The baseline harness accepts only `MEMORY_TRIPLE_TEST_ASSERT_SPEEDUP=0`; it rejects `1` until Task 3 implements candidate-query support. The candidate harness may accept `1` only while capturing the implemented candidate against the corrected baseline. Other values are invalid. The report path must not already exist; report creation uses create-new semantics. Test credentials may be used only for the disposable local server.

Resolve `surrealdb/surrealdb:v3.3.5` to an immutable local image ID before startup. Run an isolated RocksDB server bound to `127.0.0.1`; use the same `root`/`root` test credentials only for this disposable container. `docker-default` applies no explicit container resource limits. `memory-512m` sets both memory and memory-swap limits to exactly 536870912 bytes and leaves CPU unrestricted. For both modes, preflight requires `CpuQuota=0`, `CpuPeriod=0`, `NanoCpus=0`, and an empty `CpusetCpus`; unavailable or incorrectly typed inspection fields fail preflight. Before connecting, use the Docker CLI to locate exactly one running container publishing the URL's host port and verify its image ID, loopback host binding, RocksDB startup path, and resource settings against the selected mode. For a literal IP in the URL, require Docker's published `HostIp` to equal that IP. For `localhost`, accept any published loopback IP. Give each Docker CLI invocation a 10-second timeout; on timeout, terminate and reap the child process, then fail preflight. Record effective limits from `docker inspect`, not from the mode label alone. The synthetic inspection fixtures test only the test-local evidence validator's acceptance policy; they do not test Docker behavior or live container state. The ignored experiment must validate fresh CLI inspection output before connecting. Record server and SDK versions, image identity, storage engine, Docker host and image architectures, emulation status, and effective resource limits. Use an empty disposable database and seed 2191 facts with deterministic 2048-dimensional vectors and five triples at fixed cutoff `2026-11-13T15:52:35.269Z`. Fact IDs are `fact:triple-fixture-0000` through `fact:triple-fixture-2190`; vector component `(i, j)` is `((i * 31 + j * 17) % 101) / 100.0` as `f32`. The five triple sources are fact 0000 for `subject-match`, fact 0001 for `predicate-match`, fact 0002 for `object-match`, a duplicate source to fact 0000 for `duplicate-match`, and the missing ID `fact:triple-fixture-missing` for subject `quote 'snow' 日本語`. Use synthetic data only.

Run the experiment once for each resource mode because `MEMORY_TRIPLE_TEST_RESOURCE_MODE` selects exactly one mode per process. Measure empty-table, populated-no-match, and positive-match cases at limits 0, 1, 3, and 40 in that mode. Take five timed samples for every case, limit, resource mode, and cache mode. A fresh sample uses a newly created database and a new bound connection, seeds the fixture, issues one timed query, then closes the connection and removes only that database. Warm mode uses one seeded database and persistent connection, runs five untimed calls, then takes five timed samples. Compare rows as normalized multisets; do not require an unspecified tie order when a fixture exceeds the limit. Exclude connection setup, authentication, database creation, migrations, seeding, report serialization, and cleanup from query timing.

For each case, limit, selected resource mode, and cache mode, run five A/A calibration pairs against the same corrected unguarded query. Calibration noise is the maximum of `abs(a1-a2)/max(a1,a2)` across those pairs. A pair with two zero-duration samples is invalid. Noise above 3% makes timing evidence invalid. Record each sample and the smallest non-zero `Instant` delta measured in the test process. Each run writes one report for its selected resource mode; run the second mode with a distinct report path. Do not claim a useful empty-path optimization when the baseline signal is at or below the calibrated measurement noise.

The report is JSON with schema `memory-triple-latency/v1`. Its allowlist is: `schema`, `server_version`, `sdk_version`, `image_id`, `storage_engine`, `host_architecture`, `image_architecture`, `emulation`, `resource_mode`, `effective_resources` (`memory_limit_bytes`, `memory_swap_limit_bytes`, `cpu_quota_micros`, `cpu_period_micros`), `fixture` (`fact_count`, `embedding_dimension`, `triple_count`, `cutoff`), `measurement_resolution_ns`, `calibration` (case, limit, resource mode, cache mode, five sample pairs, and maximum noise fraction), and `samples` (case, limit, resource mode, cache mode, five elapsed nanosecond values, row count, SHA-256 digest of the JSON encoding of normalized rows, and query-call count). Do not include SQL, bound values, fact IDs, raw rows, credentials, tenant identifiers, or request labels.

## C. Optional empty-path guard

Do not implement or benchmark a guard until section B has produced a valid corrected baseline. Consider a guard only when corrected empty-table or populated-no-match calls have a repeatable cost above measurement noise.

Before product implementation, test the candidate SQL as variant B in the remote harness. Variant A uses the corrected unguarded SQL. Both variants use the same forwarding adapter type, public store method, client, fixture, and row conversion. Run A/A calibration and five rounds of AB then BA comparisons. The only eligible speedup cases are empty-table and populated-no-match with `limit > 0`. The target is median(B)/median(A) <= 0.50 in both fresh and warm modes. Positive-match and zero-limit cases must not regress beyond `max(3%, observed calibration noise)`. Invalid noise, incomplete samples, or missing speedup leaves the guard out.

A single block or RPC does not prove that the two reads share the required read view. Before accepting the guard, inspect the exact SurrealDB 3.3.5 engine and block transaction code, cite immutable source paths and line anchors, and establish behavior for the production backend. The memory fixture's per-transaction snapshot isolation does not establish read-view sharing within a block. If the actual backend or block read-view behavior cannot be established, do not add the guard.

On the isolated 3.3.5 server, assert flat array results for empty and positive branches. Verify that an intentionally failing fact branch is skipped when no triples match and that the same error propagates when the branch is entered. Compare rows and supported missing-table/error behavior with the corrected baseline. Distinguish a real empty array from a malformed non-array result, because `BoundDbClient::query_rows` maps non-arrays to empty rows. Preserve errors; do not change the shared decoder or add a two-call fallback.

If all evidence passes, add a deterministic recording-`DbClient` test for one guarded call, unchanged namespace and bindings, and the corrected fact SELECT. Keep `limit=0` on the corrected unguarded query. Implement the guard only in `KnowledgeStoreClient::select_facts_by_triple`.

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
- The remote A/A or A/B experiment supports only the measured SQL claim on its recorded engine, fixture, and resources. Its report schema and digest rules are defined in section B.
- Existing retrieval-quality tests protect downstream access/type policy and ranking. They do not prove remote latency.
- Existing `memory-perf` is a broad smoke test with a 1.25 p95 ratio threshold. It does not replace the focused SQL experiment or establish production p99.

The core fix is accepted when positive triple lookups return their independently expected facts, no-match and dangling-source cases remain empty, the existing projection and temporal behavior remain intact, and the focused and adjacent regression tests pass. The unguarded baseline must be attempted after the fix and before any guard. Report it as `BLOCKED` if an isolated 3.3.5 server or required measurements are unavailable.

A guard is accepted only if its backend read-view and decoder/error compatibility are established and the A/B thresholds pass. Stage metrics are accepted only when the runbook records the evidence that justified them and tests prove entered scopes, error/drop behavior, and unchanged response shape.

Runbook `docs/performance/ASSEMBLE_CONTEXT_LATENCY.md` records commands, versions, source references, fixture identity, results, artifacts, cleanup, and unresolved gates. No ADR is needed while the work stays inside the existing knowledge and retrieval boundaries. Any architectural change requires a separate decision.

No production latency improvement or production p99 claim follows from synthetic SQL timings. Production rollout, runtime tuning, and further work on graph indexes, ANN expansion, FTS/alias fan-out, or memory policy require separate approval.
