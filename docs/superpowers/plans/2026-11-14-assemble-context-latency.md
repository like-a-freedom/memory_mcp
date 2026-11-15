# `assemble_context` latency implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Correct positive triple retrieval, capture a corrected unguarded baseline, and add a guard or stage metrics only when their evidence gates pass.

**Architecture:** Keep triple query ownership in `KnowledgeStoreClient`. First fix the nested ID projection and prove the returned fact against independent fixture IDs. Measure that corrected query before testing an optional guard. Keep database compatibility and performance evidence separate from deterministic contract tests.

**Tech stack:** Rust `1.99.0`, Tokio, SurrealDB SDK/server `3.3.5`, existing Prometheus recorder, Cargo integration tests, eval-harness.

**Spec:** [Design and acceptance contract](../specs/2026-11-14-assemble-context-latency-design.md).

## Global constraints

- Rust `1.99.0`; SurrealDB server and SDK `3.3.5`; current `Cargo.lock`; Cargo commands use `--locked`.
- Do not change dependencies, migrations, generated code, MCP tools, or application-facing APIs. Do not edit `Cargo.toml` or `Cargo.lock`.
- A bound client uses one Active Namespace. Do not add request-level namespace, database, or table selection.
- Production remains untouched. Diagnostic SSH access does not authorize deployment, restart, credential use, or tuning.
- Preserve FTS terms and per-term limits, alias expansion, ANN K/EF, EUCLIDEAN/F32, admission thresholds, reranking, graph budgets, and ordering behavior.
- Preserve temporal and access filtering, provenance, access tracking, query-log policy, cache defaults, and job cadences. Do not delete facts.
- Keep query ownership in `knowledge` and orchestration in `memory`. Add no business logic to transport, `main.rs`, `service/`, or a shared container.
- Preserve `MemoryError` and existing missing-table handling. Never convert syntax, permission, decoding, or transport failures into successful empty results.
- Use controlled ports for unit tests and real databases, network, filesystem, and process-global recorders for integration tests. Use bounded waits and permits, not sleeps for ordering.
- New telemetry and artifacts exclude raw queries, SQL literals, memory content, credentials, and request, tenant, or record labels.
- Do not commit, create branches, deploy, or modify unrelated user work. Plans, specifications, and ADRs for this work must be in English.

## Review focus

1. A positive subject, predicate, or object match, duplicate source, dangling source, quoted text, or Unicode input returns only eligible fact rows. Task 1 pins each case to independent IDs.
2. `limit=0`, `1`, `3`, and `40` preserve the output limit without relying on unspecified row order. Task 1 covers each limit.
3. Temporal cutoff and invalidation behavior remain unchanged. Task 1 tests visibility at the cutoff, excludes facts valid or ingested after it, and checks invalidation recorded at or after the cutoff; existing semantic-quality and access tests cover downstream policy.
4. A guard distinguishes a real empty array from malformed output and preserves entered-branch errors. Task 3 checks both branches through the real result conversion.
5. Docker image/backend identity, URL-to-published-loopback host correspondence, Docker CLI timeouts, read-view semantics, calibration noise, and missing resource measurements do not become unsupported claims. Tasks 2 and 3 record these gates and leave unresolved claims blocked.

## File map

All paths are workspace-relative. Line numbers are navigation hints.

| File | Responsibility |
|---|---|
| `crates/memory-mcp/src/knowledge/knowledge_store.rs:117–143` | Correct the nested triple ID projection; add the guard only if Task 3 passes. |
| `crates/memory-mcp/tests/support/triple_retrieval_oracle.rs` | Hold the independent corrected unguarded SQL, fixed variables, deterministic fixture, and normalized-row helper. |
| `crates/memory-mcp/tests/triple_retrieval_contract.rs` | Test public-store results, positive IDs, limits, duplicate/dangling sources, and query data binding. |
| `crates/memory-mcp/tests/retrieval_latency_remote.rs` | Create only for Task 2; run ignored, loopback-only compatibility and timing experiments through the public store. |
| `crates/memory-mcp/tests/support/retrieval_latency_experiment.rs` | Hold the test-local remote harness, bounded Docker CLI runner, and latency measurement support. |
| `docs/performance/ASSEMBLE_CONTEXT_LATENCY.md` | Record exact setup/cleanup commands, baseline, decisions, measurements, source references, and remaining gates. |
| `crates/memory-mcp/src/memory/retrieval/pipeline.rs` | Modify only if Task 4 justifies stage metrics. |
| `crates/memory-mcp/src/memory/retrieval.rs` | Modify only if Task 4 justifies access/reconciliation metrics. |
| `crates/memory-mcp/src/tools/assemble_context.rs` | Modify only if Task 4 justifies serialization metrics. |
| `crates/memory-mcp/src/shared/observability.rs` | Extend its test-only stage vocabulary only for metrics selected in Task 4. |
| `crates/memory-mcp/tests/assemble_context_stage_metrics.rs` | Create only if Task 4 justifies stage metrics. |

Do not modify `tests/common/storage_memory.rs`, shared decoding, migrations, ANN/FTS builders, or evaluation artifact schemas. Its counting allocator and reconnecting helper are unsuitable for this latency target. Do not add an ADR unless investigation requires an architectural change.

## Task 1: Correct positive triple retrieval

**Files:** Modify `knowledge_store.rs`, `tests/support/triple_retrieval_oracle.rs`, and `tests/triple_retrieval_contract.rs`.

**Interfaces:** Keep `KnowledgeStoreClient::select_facts_by_triple(&self, query_text: &str, cutoff: &str, limit: usize) -> Result<Vec<Value>, MemoryError>` unchanged. The test oracle exports `corrected_unguarded_triple_sql() -> &'static str`, `corrected_triple_vars(query: &str, cutoff: &str, limit: usize) -> Value`, `async fn seed_triple_fixture(db: &dyn DbClient, namespace: &str, fact_count: usize, dimension: usize) -> Result<(), MemoryError>`, and `normalized_rows(rows: &[Value]) -> Vec<String>`.

- [ ] **Step 1: Prove the positive subject-match failure through the public store.** Ensure `subject_match_returns_the_independently_expected_fact` asserts the literal ID `fact:triple-fixture-0000` and object provenance from `KnowledgeStoreClient::select_facts_by_triple`. Keep the test expectation independent of the SQL helper.

- [ ] **Step 2: Run the focused test and observe the expected RED.**

  Run: `cargo test -p memory_mcp --features eval-support --locked --test triple_retrieval_contract subject_match_returns_the_independently_expected_fact -- --exact --nocapture`

  Expected: the store returns no row where the test expects `fact:triple-fixture-0000`. A setup, compile, or unrelated error is not the expected RED.

- [ ] **Step 3: Freeze the corrected unguarded SQL and complete the independent contract cases.** In the test helper, change only the nested projection to `SELECT VALUE source_fact_id`; rename `legacy_*` helpers to `corrected_unguarded_*` or `corrected_*`. Update tests to compare the public-store result to this frozen corrected SQL and independently assert subject, predicate, and object IDs. Fix the dangling-source query to use the seeded subject `quote 'snow' 日本語`. Keep duplicate, missing-table, quoted/Unicode, provenance, vector omission, and limits `0`, `1`, `3`, and `40` as named cases. Add public-store temporal cases using the seeded subject fact: `fact_valid_at_cutoff_is_included`, `fact_valid_after_cutoff_is_excluded`, `fact_ingested_after_cutoff_is_excluded`, `fact_invalidated_at_cutoff_is_excluded`, and `fact_invalidation_recorded_after_cutoff_remains_visible`. Set temporal fields in the isolated fixture with bound parameters and assert the literal fact ID or an empty result. For `limit=1`, assert only membership in the eligible IDs.

- [ ] **Step 4: Run the positive tests against the corrected oracle and confirm the store still fails.**

  Run: `cargo test -p memory_mcp --features eval-support --locked --test triple_retrieval_contract subject_match_returns_the_independently_expected_fact -- --exact --nocapture`

  Expected: RED remains at the public-store result comparison or the literal expected-ID assertion. The corrected oracle independently returns `fact:triple-fixture-0000`.

- [ ] **Step 5: Implement the minimal production fix.** In `KnowledgeStoreClient::select_facts_by_triple`, change only the nested projection to `SELECT VALUE source_fact_id`. Keep the public signature, fact projection, `BI_TEMPORAL_WHERE`, variables, `$limit`, and error handling unchanged. Do not add a guard.

- [ ] **Step 6: Run contract and adjacent query tests.**

  ```bash
  cargo test -p memory_mcp --features eval-support --locked --test triple_retrieval_contract
  cargo test -p memory_mcp --features eval-support --locked --test retrieval_query_projection --test retrieval_query_batching --test graph_batch_query_regression --test query_scaling_regressions
  ```

  Expected: independently expected positive IDs, all named temporal-boundary cases, and adjacent query contracts pass. `limit=0` returns no rows; the store SQL still uses the corrected projection. No new dependency, API, or schema change appears.

## Task 2: Capture the corrected unguarded baseline

**Files:** Create `retrieval_latency_remote.rs` and the runbook. Modify `tests/support/retrieval_latency_experiment.rs` for the test-local remote harness and bounded Docker CLI runner. Extend the test oracle only for remote fixture setup or report encoding required by the spec.

**Interfaces:** Variant A is `corrected_unguarded_triple_sql()` from Task 1. Use one test-local forwarding `DbClient` around one persistent bound client so the public store and real `query_rows` conversion remain in the measurement path. The ignored tests are `remote_triple_compatibility` and `remote_latency_capture`.

- [ ] **Step 1: Add loopback configuration and fixture tests before the ignored experiment.** Test each required variable missing in turn, invalid speedup/resource mode/image ID, and non-loopback or malformed URL rejection before connection. Test deterministic fixture metadata: 2191 ordered fact IDs, 2048 values per vector, and the five exact triple source tuples. Test that row digests remain stable under row and object-key reordering. Exercise the test-local container-evidence validator with synthetic inspection JSON, including a URL-host mismatch, nonzero `NanoCpus`, and nonempty `CpusetCpus`, and assert each policy failure category separately. Add a fake-executable test for Docker CLI timeout and child termination. These tests cover harness policy and subprocess management, not Docker daemon behavior or live container state; the ignored run must verify live CLI inspection output. Keep helpers inside the integration test target.

- [ ] **Step 2: Run the focused test-target checks and observe RED for missing configuration/fixture behavior.**

  Run: `cargo test -p memory_mcp --features eval-support --locked --test retrieval_latency_remote`

  Expected: new configuration/fixture contract assertions fail for the missing behavior, not because an external server is unavailable. Remote measurements remain ignored in ordinary test runs.

- [ ] **Step 3: Implement the minimal loopback-only test harness.** Read only the five `MEMORY_TRIPLE_TEST_*` variables defined in Spec B. Parse and validate all values before opening a connection; accept only `ws` URLs on `localhost` or literal loopback IPs, an image ID matching `sha256:` plus 64 lowercase hex digits, resource modes `docker-default` or `memory-512m`, and `MEMORY_TRIPLE_TEST_ASSERT_SPEEDUP=0`. Reject `1` until Task 3 implements candidate-query support; reject any other value. Require Docker's published `HostIp` to equal a literal URL IP, or any loopback IP for `localhost`. Verify `CpuQuota=0`, `CpuPeriod=0`, `NanoCpus=0`, and empty `CpusetCpus` for both resource modes; missing or malformed values fail preflight. Give each Docker CLI invocation a 10-second timeout and terminate and reap its child on timeout. Create the report with `create_new` so an existing artifact is never overwritten. Set a 30-second SDK setup deadline and 10-second query timeout. Use generated unique databases and remove only those databases during cleanup.

- [ ] **Step 4: Add corrected-baseline capture with deterministic synthetic data.** Seed 2191 facts, 2048-dimensional deterministic vectors, and five triples at cutoff `2026-11-13T15:52:35.269Z`. For each of the three cases, four limits, the selected resource mode, and two cache modes, collect five timing samples and five A/A pairs. Fresh mode creates and removes a database and bound connection per timed sample; warm mode runs five untimed calls before its five samples on one connection. Normalize result rows as multisets and compute the Spec B SHA-256 digest. Exclude connection setup, authentication, database creation, migrations, seeding, report serialization, and cleanup from timing. Write only the Spec B allowlisted fields. Run once per resource mode and use a distinct report path for each run.

- [ ] **Step 5: Run local configuration and embedded/remote compatibility tests.**

  ```bash
  cargo test -p memory_mcp --features eval-support --locked --test retrieval_latency_remote
  cargo test -p memory_mcp --features eval-support --locked --test triple_retrieval_contract
  ```

  Expected: test-target unit checks and local database contracts pass. If Docker or an isolated server is unavailable, mark the remote baseline `BLOCKED`; do not substitute the embedded memory engine and claim remote-server evidence.

- [ ] **Step 6: Capture the unguarded baseline before any guard edit.** Provision only a disposable loopback 3.3.5 server with image tag `surrealdb/surrealdb:v3.3.5`, resolved to an immutable image ID. Follow the two resource modes in Spec B. Run the ignored test once for `docker-default` and once for `memory-512m`, setting `MEMORY_TRIPLE_TEST_RESOURCE_MODE` and a distinct `MEMORY_TRIPLE_TEST_REPORT` path for each invocation. Each invocation runs five A/A calibration pairs for every case/limit/cache combination with `MEMORY_TRIPLE_TEST_ASSERT_SPEEDUP=0` and `--ignored --test-threads=1`. Record every sample, engine and image identity, noise, effective resource measurements, and cleanup in the runbook.

  Run the following command once per resource mode. Each command supplies all five required environment variables and writes to a separate, create-new report path:

  ```bash
  MEMORY_TRIPLE_TEST_URL=ws://127.0.0.1:33127 \
  MEMORY_TRIPLE_TEST_IMAGE_ID=sha256:f40e356e40dd2094e5a743ad8b1353f646cf3a660541e92b9f6cce003d3be2dd \
  MEMORY_TRIPLE_TEST_RESOURCE_MODE=docker-default \
  MEMORY_TRIPLE_TEST_ASSERT_SPEEDUP=0 \
  MEMORY_TRIPLE_TEST_REPORT=/tmp/memory-triple-latency-docker-default-20261114.json \
  cargo test -p memory_mcp --features eval-support --locked --test retrieval_latency_remote remote_latency_capture -- --exact --ignored --test-threads=1

  MEMORY_TRIPLE_TEST_URL=ws://127.0.0.1:33127 \
  MEMORY_TRIPLE_TEST_IMAGE_ID=sha256:f40e356e40dd2094e5a743ad8b1353f646cf3a660541e92b9f6cce003d3be2dd \
  MEMORY_TRIPLE_TEST_RESOURCE_MODE=memory-512m \
  MEMORY_TRIPLE_TEST_ASSERT_SPEEDUP=0 \
  MEMORY_TRIPLE_TEST_REPORT=/tmp/memory-triple-latency-memory-512m-20261114.json \
  cargo test -p memory_mcp --features eval-support --locked --test retrieval_latency_remote remote_latency_capture -- --exact --ignored --test-threads=1
  ```

  Expected: complete corrected-unguarded samples for both resource modes, row digests, one query call per measured sample, and maximum A/A calibration noise at or below 3%. Missing server/resource evidence is `BLOCKED`; noise above 3% is `INVALID` and requires investigation.

- [ ] **Step 7: Capture the post-fix, pre-guard application baselines.** Run `cargo run -p eval-harness --locked --bin memory-eval -- run --profile evals/profiles/retrieval_quality.json --artifact target/evals/assemble-context-retrieval-quality.json` and `cargo run --release -p eval-harness --locked --bin memory-perf -- run --profile evals/perf/profiles/perf-weekly.json --artifact target/perf/assemble-context-perf.json --baseline evals/perf/baselines/perf-weekly-local.json --host-tag local`. Check that the artifact paths do not already exist before running. Record unavailable model files, invalid denominators, or host/profile mismatches without changing profiles or baselines.

## Task 3: Evaluate and, only if justified, add the empty-path guard

**Files:** Modify `knowledge_store.rs`, the triple contract tests, the remote experiment, and the runbook only if the guard passes every gate.

**Interfaces:** Consume the corrected unguarded SQL and baseline from Tasks 1 and 2. The public method signature and `DbClient::query` boundary remain unchanged. The test harness compares A (corrected unguarded SQL) with B (guard candidate SQL) through the same public store and client.

- [ ] **Step 1: Decide whether an A/B comparison is justified.** Review Task 2's calibrated empty-table and no-match samples. Proceed only when the empty-path duration is repeatable and above the A/A noise and timer resolution. Otherwise record `guard not justified` with the measured values and skip the rest of Task 3.

- [ ] **Step 2: Verify SurrealDB 3.3.5 syntax and read-view semantics before adopting the candidate.** Inspect exact versioned engine and block transaction sources. Record immutable source links with line anchors and the actual production backend identity if established by authorized evidence. The memory-engine transaction snapshot is not proof that block reads share a read view. If the backend or snapshot behavior remains unknown, record `guard BLOCKED` and stop this task without changing production code.

- [ ] **Step 3: Add test-only candidate compatibility and timing cases.** Use the proposed block in Spec C as B, with the corrected unguarded query as A. Introduce candidate mode only with this implemented B variant; `MEMORY_TRIPLE_TEST_ASSERT_SPEEDUP=1` remains rejected unless the candidate is selected. Assert exact flat-array empty and positive results through `BoundDbClient`; distinguish non-array results from empty arrays. Inject a failing fact branch to prove it is skipped for empty triples and propagates when entered. Compare missing-table and supported error behavior.

- [ ] **Step 4: Run compatibility against the isolated 3.3.5 backend.**

  Run: `cargo test -p memory_mcp --features eval-support --locked --test retrieval_latency_remote remote_triple_compatibility -- --exact --ignored --test-threads=1`

  Expected: both branches return the expected flat arrays; skipped branches do not run; entered-branch errors propagate; corrected positive facts, missing-table handling, and result shape match. Any malformed result, error mismatch, or unproven read view blocks the guard.

- [ ] **Step 5: Measure the candidate before production implementation.** For each resource mode captured in Task 2, run the ignored candidate test with that mode selected and a distinct report path. Run five rounds of A/A plus AB and BA pairs in both cache modes. Apply Spec C thresholds: empty/no-match positive-limit median(B)/median(A) <= `0.50`; positive and zero-limit cases do not regress beyond `max(3%, observed calibration noise)`; A/A noise <= `3%`. Preserve all samples and unavailable resource reasons.

  Run once per resource mode, setting `MEMORY_TRIPLE_TEST_RESOURCE_MODE` and a unique `MEMORY_TRIPLE_TEST_REPORT` for each invocation: `MEMORY_TRIPLE_TEST_ASSERT_SPEEDUP=1 cargo test -p memory_mcp --features eval-support --locked --test retrieval_latency_remote remote_latency_capture -- --exact --ignored --test-threads=1`

  Expected: every gate passes. If the speedup target fails, the corrected baseline is too noisy, or resources regress beyond calibrated noise, record the result and do not implement the guard.

- [ ] **Step 6: Add a deterministic query-policy regression and observe RED.** Only after Step 5 passes, test `empty_triple_lookup_submits_guarded_corrected_query` through a recording `DbClient`. Assert one call, the bound namespace, unchanged `query/cutoff/limit` variables, the triple existence guard before the corrected fact SELECT, and flat empty output. Test that `limit=0` submits the corrected unguarded query. Keep a quoted sentinel only in variables.

  Run: `cargo test -p memory_mcp --features eval-support --locked --test triple_retrieval_contract empty_triple_lookup_submits_guarded_corrected_query -- --exact`

  Expected: RED because the production query does not yet contain a guard. A compiler or setup error is not the expected RED.

- [ ] **Step 7: Implement the minimal owner-local guard and verify GREEN.** Keep the corrected SELECT as the guarded block's fact branch. Do not change decoder behavior, add a two-call fallback, change the API, or add logging. Keep zero-limit execution on the corrected unguarded SELECT.

  Run:

  ```bash
  cargo test -p memory_mcp --features eval-support --locked --test triple_retrieval_contract
  cargo test -p memory_mcp --features eval-support --locked --test retrieval_latency_remote remote_triple_compatibility -- --exact --ignored --test-threads=1
  ```

  Run `remote_latency_capture` once per resource mode with `MEMORY_TRIPLE_TEST_ASSERT_SPEEDUP=1` and a unique report path for each invocation.

  Expected: contract tests, real-engine compatibility, and the measured A/B gate pass. If Task 3 was skipped, do not run or report a guard candidate.

## Task 4: Add stage metrics only when measurements justify them

**Files:** Modify pipeline/recall/serialization call sites, the test-only stage registry, a dedicated integration test, and the runbook only if the decision gate passes.

**Interfaces:** Reuse `StageTimer::new(&'static str, &'static str)` and its existing Drop behavior. Keep the recorder and registry visibility unchanged. If selected, use fixed stage names from Spec D only.

- [ ] **Step 1: Write the evidence-based decision in the runbook.** Identify a recurring application-level latency result after Task 1 and optional Task 3, the work that current metrics cannot localize, and why selected stage boundaries answer that question. If no such question remains, record `stage metrics not justified` and skip Task 4 code changes.

- [ ] **Step 2: Add tests for only the selected scopes before instrumentation.** Use a dedicated integration target and the public `shared_test_handle()`. Cover successful entered scopes, the selected error/drop boundary, cache-hit behavior when access tracking is selected, and unchanged compact/full response shape when serialization is selected. For an async ordering claim, use permits and bounded waits, not sleeps. Recorder installation failure is a test failure.

- [ ] **Step 3: Run the selected metric test and observe RED.**

  Run: `cargo test -p memory_mcp --features fs-watch,mcp-apps,streamable-http,eval-support --locked --test assemble_context_stage_metrics`

  Expected: the selected new metric deltas are missing before instrumentation. If no metrics were justified, this target is not created and this step is skipped.

- [ ] **Step 4: Add block-scoped timers at the selected ownership boundaries.** Keep existing timer scopes unchanged. Add only fixed names in `DECLARED_STAGES`; do not add a metric family, dynamic labels, awaits, SQL, or payload copies.

- [ ] **Step 5: Run selected metric tests and existing behavioral contracts.**

  ```bash
  cargo test -p memory_mcp --features fs-watch,mcp-apps,streamable-http,eval-support --locked --test assemble_context_stage_metrics
  cargo test -p memory_mcp --features eval-support --locked --test assemble_context_wire_contract --test embedded_context_cache --test assemble_context_semantic_quality
  cargo run -p xtask --locked -- check-observability
  ```

  Expected: selected observation deltas and existing wire/cache/quality contracts pass; observability checks pass. Do not edit unrelated generated dashboards to silence failures.

## Final validation

- [ ] Run `cargo fmt --all --check`.
- [ ] Run applicable triple retrieval and existing query/quality tests from Tasks 1 and 3.
- [ ] Run the metrics test only if Task 4 was justified.
- [ ] Run `cargo test -p memory_mcp --locked`.
- [ ] Run `cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings`.
- [ ] Update the English runbook with actual test output, baseline/candidate identities, RED/GREEN results, guard decision, any metrics decision, artifacts, cleanup, and unresolved gates.
- [ ] Preserve all user WIP. Do not commit or deploy. Report any unavailable server, native resource, model, or permission as blocked rather than passing.
