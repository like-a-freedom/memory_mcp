# `assemble_context` latency implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Correct positive triple retrieval, capture a corrected unguarded baseline, and add a guard or stage metrics only when their evidence gates pass.

**Architecture:** Keep triple query ownership in `KnowledgeStoreClient`. First fix the nested ID projection and prove the returned fact against independent fixture IDs. Measure that corrected query before testing an optional guard. Keep database compatibility and performance evidence separate from deterministic contract tests.

**Tech stack:** Rust `1.99.0`, Tokio, SurrealDB SDK/server `3.3.5`, existing Prometheus recorder, Cargo integration tests, eval-harness.

**Spec:** [Design and acceptance contract](../specs/2026-11-14-assemble-context-latency-design.md).

## Authorized execution update (November 16, 2026)

The user resumed Task 2 and approved the descriptive v4 baseline plus a second simplification pass. The current protocol is Spec B; historical calibration restrictions are superseded for new runs only. Keep historical classifications without reclassifying them; raw reports and diagnostics were later removed during merge preparation, with results and recorded SHA-256 identifiers retained in the runbook. Guard/read-view/correctness gates and production safety remain in force. The user subsequently authorized a local feature branch and commit for merge preparation only; no merge or push is authorized.

### Authorized application-only rollout

The approved artifact is built from release tag `v1.38.0` with only the reviewed `SELECT VALUE source_fact_id` production fix. Do not deploy the current `master` checkout. The corrected production target uses SSH port `2288`; all host observations made against the previously supplied address are unverified for this target. Before any production change, verify a backup made from the corrected host, the current application and SurrealDB container/image IDs, Compose service names, host architecture, and configured image reference without printing secrets. Confirm the Compose image reference is a valid Docker image-tag destination in `name[:tag]` form; a digest-only reference is not taggable and blocks rollout. Choose a unique rollback tag and verify that it does not already exist. If any check fails, stop without changing image tags or Compose. Transfer and load the immutable image, then verify its image ID on the production host. Preserve the running application image ID under the verified rollback tag and record the Compose image reference before retagging the candidate image to that reference.

Restart only the `memory_mcp` service with `docker compose -f /opt/memory_mcp/docker-compose.yaml up -d --no-deps --force-recreate --pull never --no-build memory_mcp`. Never run `down`, recreate dependencies, or change the SurrealDB container, volume, or data. Verify the candidate image ID, unchanged SurrealDB container identity, and only the `/memory/health/live` and `/memory/health/ready` endpoints with the configured Host. Keep exact container IDs in the private operational ledger, not repository artifacts. `/memory/health` is UI HTML, not a health endpoint. Do not send production retrieval or mutation requests. If the SurrealDB container identity changes unexpectedly, preserve its volume and stop for investigation; do not attempt to repair or restore the database as part of app rollback.

If an application health check fails while the SurrealDB container ID remains unchanged, retag the preserved previous application image ID to the recorded Compose image reference and repeat the same app-only Compose command. Verify the restored application image ID, unchanged SurrealDB container ID, and both health endpoints. Keep the backup and rollback image. If SSH access, backup integrity, artifact identity, or the rollback image cannot be verified, do not deploy. Record the actual rollout or rollback evidence in the English runbook.

## Global constraints

- Rust `1.99.0`; SurrealDB server and SDK `3.3.5`; current `Cargo.lock`; Cargo commands use `--locked`.
- Do not change dependencies, migrations, generated code, MCP tools, or application-facing APIs. Do not edit `Cargo.toml` or `Cargo.lock`.
- A bound client uses one Active Namespace. Do not add request-level namespace, database, or table selection.
- Preserve production data. Use a checked backup and isolated restored data for experiments. Do not expose credentials or run unbounded production load.
- Resolve one local Unix Docker endpoint before daemon inspection. Reject simultaneous `DOCKER_HOST` and `DOCKER_CONTEXT` overrides and remote endpoints; clear both variables from subprocesses and pin Docker CLI inspection to the resolved endpoint.
- Preserve FTS terms and per-term limits, alias expansion, ANN K/EF, EUCLIDEAN/F32, admission thresholds, reranking, graph budgets, and ordering behavior.
- Preserve temporal and access filtering, provenance, access tracking, query-log policy, cache defaults, and job cadences. Do not delete facts.
- Keep query ownership in `knowledge` and orchestration in `memory`. Add no business logic to transport, `main.rs`, `service/`, or a shared container.
- Preserve `MemoryError` and existing missing-table handling. Never convert syntax, permission, decoding, or transport failures into successful empty results.
- Use controlled ports for unit tests and real databases, network, filesystem, and process-global recorders for integration tests. Use bounded waits and permits, not sleeps for ordering.
- New telemetry and artifacts exclude raw queries, SQL literals, memory content, credentials, and request, tenant, or record labels.
- Do not modify unrelated user work. Deployment requires the verification and rollback procedure above. Plans, specifications, and ADRs for this work must be in English. The later user request explicitly authorizes creating a local feature branch and commit for merge preparation; it does not authorize a merge or push.

## Review focus

1. A positive subject, predicate, or object match, duplicate source, dangling source, quoted text, or Unicode input returns only eligible fact rows. Task 1 pins each case to independent IDs.
2. `limit=0`, `1`, `3`, and `40` preserve the output limit without relying on unspecified row order. Task 1 covers each limit.
3. Temporal cutoff and invalidation behavior remain unchanged. Task 1 tests visibility at the cutoff, excludes facts valid or ingested after it, and checks invalidation recorded at or after the cutoff; existing semantic-quality and access tests cover downstream policy.
4. A guard distinguishes a real empty array from malformed output and preserves entered-branch errors. Task 3 checks both branches through the real result conversion.
5. Docker image/backend identity, URL-to-published-loopback host correspondence, Docker CLI timeouts, read-view semantics, timing spread, and missing resource measurements do not become unsupported claims. Tasks 2 and 3 record these gates and leave unresolved claims blocked.

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

- [x] **Step 1: Prove the positive subject-match failure through the public store.** Ensure `subject_match_returns_the_independently_expected_fact` asserts the literal ID `fact:triple-fixture-0000` and object provenance from `KnowledgeStoreClient::select_facts_by_triple`. Keep the test expectation independent of the SQL helper.

- [x] **Step 2: Run the focused test and observe the expected RED.**

  Run: `cargo test -p memory_mcp --features eval-support --locked --test triple_retrieval_contract subject_match_returns_the_independently_expected_fact -- --exact --nocapture`

  Expected: the store returns no row where the test expects `fact:triple-fixture-0000`. A setup, compile, or unrelated error is not the expected RED.

- [x] **Step 3: Freeze the corrected unguarded SQL and complete the independent contract cases.** In the test helper, change only the nested projection to `SELECT VALUE source_fact_id`; rename `legacy_*` helpers to `corrected_unguarded_*` or `corrected_*`. Update tests to compare the public-store result to this frozen corrected SQL and independently assert subject, predicate, and object IDs. Fix the dangling-source query to use the seeded subject `quote 'snow' 日本語`. Keep duplicate, missing-table, quoted/Unicode, provenance, vector omission, and limits `0`, `1`, `3`, and `40` as named cases. Add public-store temporal cases using the seeded subject fact: `fact_valid_at_cutoff_is_included`, `fact_valid_after_cutoff_is_excluded`, `fact_ingested_after_cutoff_is_excluded`, `fact_invalidated_at_cutoff_is_excluded`, and `fact_invalidation_recorded_after_cutoff_remains_visible`. Set temporal fields in the isolated fixture with bound parameters and assert the literal fact ID or an empty result. For `limit=1`, assert only membership in the eligible IDs.

- [x] **Step 4: Run the positive tests against the corrected oracle and confirm the store still fails.**

  Run: `cargo test -p memory_mcp --features eval-support --locked --test triple_retrieval_contract subject_match_returns_the_independently_expected_fact -- --exact --nocapture`

  Expected: RED remains at the public-store result comparison or the literal expected-ID assertion. The corrected oracle independently returns `fact:triple-fixture-0000`.

- [x] **Step 5: Implement the minimal production fix.** In `KnowledgeStoreClient::select_facts_by_triple`, change only the nested projection to `SELECT VALUE source_fact_id`. Keep the public signature, fact projection, `BI_TEMPORAL_WHERE`, variables, `$limit`, and error handling unchanged. Do not add a guard.

- [x] **Step 6: Run contract and adjacent query tests.**

  ```bash
  cargo test -p memory_mcp --features eval-support --locked --test triple_retrieval_contract
  cargo test -p memory_mcp --features eval-support --locked --test retrieval_query_projection --test retrieval_query_batching --test graph_batch_query_regression --test query_scaling_regressions
  ```

  Expected: independently expected positive IDs, all named temporal-boundary cases, and adjacent query contracts pass. `limit=0` returns no rows; the store SQL still uses the corrected projection. No new dependency, API, or schema change appears.

## Task 2: Capture a descriptive corrected baseline

**Files:** tests/retrieval_latency_remote.rs and tests/support/retrieval_latency_experiment.rs under crates/memory-mcp; the runbook and this spec/plan. No production query change.

**Interfaces:** Keep verify_remote_triple_compatibility(config) and capture_remote_latency(config). Use corrected_unguarded_triple_sql(), the unchanged public store method, and the current deterministic fixture. New reports use memory-triple-latency/v4.

- [x] Add RED regressions for median/min/max with raw acquisition order preserved, incomplete captures, changed rows, and visible outliers. Add a RED configuration test requiring only the four endpoint/image/resource/report values.
- [x] Replace calibration and connection-mode branches with three cases at limit 40, five warmups, and twenty timed queries on a reused connection. Keep row/digest/one-call checks and create-new artifact handling.
- [x] Remove calibration/CI functions and tests, new-connection timing, quiescence/stats polling, timestamp diagnostics, lifecycle probes, timer-resolution probing, and the unused ASSERT_SPEEDUP input. Keep loopback/image/engine/resource preflight, pinned Docker endpoint, bounded commands, setup/query deadlines, and generated-database cleanup.
- [x] Run ordinary harness and triple contracts, full crate tests, formatting, and required workspace Clippy. Expected: ordinary tests pass; external measurements remain ignored by default.
- [x] Run live compatibility and one bounded descriptive capture per resource mode on fresh disposable disk-backed SurrealDB 3.3.5 containers. Use unique artifacts and no production data. Expected: three records per completed mode, twenty samples and twenty query calls per record, correct rows/digests, and honest summary statistics. Record a bounded failure as unavailable, not as fabricated timing.
- [x] Record reports, resource evidence, limits, cleanup, and validation in the runbook. Keep historical v1–v3 reports INVALID under their original rules. Mark this task complete only under the approved v4 completion contract.

Post-fix application checks remain separate evidence: the previous retrieval-quality run was INVALID because its real provider was unavailable for all 37 cases; the weekly memory-perf smoke passed. These do not establish SQL or production latency. Do not rerun unrelated model-dependent evaluations merely to complete the descriptive SQL baseline.

Task 2 outcome (November 16, 2026): complete under v4. The docker-default report contains all three cases and twenty observations each. The memory-512m compatibility run was OOM-killed and cleanup reached its deadline; that mode is unavailable and has no timing report. Both disposable containers were removed. See the runbook for measurements and validation. Task 3 remains optional and unimplemented.

## Task 3: Evaluate the optional guard

**Files:** knowledge_store.rs, triple contracts, the test-local remote experiment, and runbook only if justified.

**Interfaces:** Same corrected A, proposed B, DbClient query boundary, public store signature, and fixture. Section C of the spec owns all acceptance rules.

- [x] Task 2's populated no-match median was 191.387 ms, so a candidate was worth evaluating; the empty-table median was only 0.374 ms.
- [x] Verified SurrealDB 3.3.5's statement transaction, deferred IF/ELSE branch execution, and RocksDB snapshot behavior against immutable commit `5c3383fe1841e10f7fc67fdf41b6d0671cbc1b13`; links and source paths are recorded in the runbook.
- [x] Added candidate compatibility cases and ran them against the disposable server. The first RED was an inconsistent fixture expectation, corrected to `predicate-match` / `fact:triple-fixture-0001`; the live rerun passed.
- [x] Compared A/B with five warmups and twenty alternating-order pairs per case on one prepared database/reused client. The raw A/B report was captured during evaluation and then removed during merge preparation. Its SHA-256 is recorded in the runbook; the summarized results remain, but the missing report cannot be independently rechecked. No-match improvement exceeded 2x in both orders; positive-case medians were 3–6% slower with overlapping, variable paired distributions.
- [x] **Decision: do not add the production guard.** Positive-case non-regression is inconclusive under Section C, so the guard's acceptance gate is not met. The deterministic policy test and production query change are correctly skipped; no public contract changed. Only `docker-default` was measured; `memory-512m` remains unavailable.
- [x] Candidate compatibility and paired comparison passed for the measured local mode. No post-implementation rerun applies because no production guard was implemented; the runbook records the mode-specific result and limitation.

## Task 4: Add stage metrics only when measurements justify them

**Files:** Modify pipeline/recall/serialization call sites, the test-only stage registry, a dedicated integration test, and the runbook only if the decision gate passes.

**Interfaces:** Reuse `StageTimer::new(&'static str, &'static str)` and its existing Drop behavior. Keep the recorder and registry visibility unchanged. If selected, use fixed stage names from Spec D only.

- [x] **Step 1: Record the decision in the runbook.** No recurring application-level latency result remains to localize: the retrieval-quality evaluation was invalid because its provider was unavailable, while the weekly perf smoke does not establish a recurring assemble-context issue. Record `stage metrics not justified`.

- [x] **Steps 2–5 skipped:** no metric scopes were selected, so no instrumentation, metric test target, or observability check was added for this task.

- [x] **Step 3 skipped:** no selected metric test exists because Task 4 was not justified.

  Run: `cargo test -p memory_mcp --features fs-watch,mcp-apps,streamable-http,eval-support --locked --test assemble_context_stage_metrics`

  Expected: the selected new metric deltas are missing before instrumentation. If no metrics were justified, this target is not created and this step is skipped.

- [x] **Step 4 skipped:** no block-scoped timers or metric names were selected because Task 4 was not justified.

- [x] **Step 5 skipped:** no metric instrumentation was added, so the conditional metric test and observability validation were not run.

  ```bash
  cargo test -p memory_mcp --features fs-watch,mcp-apps,streamable-http,eval-support --locked --test assemble_context_stage_metrics
  cargo test -p memory_mcp --features eval-support --locked --test assemble_context_wire_contract --test embedded_context_cache --test assemble_context_semantic_quality
  cargo run -p xtask --locked -- check-observability
  ```

  Expected: selected observation deltas and existing wire/cache/quality contracts pass; observability checks pass. Do not edit unrelated generated dashboards to silence failures.

## Final validation

- [x] Run `cargo fmt --all --check`.
- [x] Run applicable triple retrieval and existing query/quality tests from Tasks 1 and 3.
- [x] Metrics test not run; Task 4 was not justified.
- [x] Run `cargo test -p memory_mcp --locked`.
- [x] Run `cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http,test-fixtures,eval-support --locked -- -D warnings`.
- [x] Update the English runbook with actual test output, baseline/candidate identities, RED/GREEN results, guard decision, any metrics decision, artifacts, cleanup, and unresolved gates.
- [x] Preserve all user WIP. Do not deploy without the verification and rollback procedure above. Report any unavailable server, native resource, model, or permission as blocked rather than passing. The later merge-preparation request authorizes a local branch and commit only; no merge or push is part of this plan.
