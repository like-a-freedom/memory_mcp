# Review Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the implementation in line with the validated review findings by fixing correctness gaps first, then upgrading graph, retrieval, and lifecycle capabilities.

**Architecture:** Execute the work in four waves. Wave 1 restores correctness and observability without changing public MCP APIs; Wave 2 upgrades graph persistence/traversal; Wave 3 adds semantic retrieval primitives; Wave 4 completes lifecycle, communities, and migration discipline. Keep the current layered boundary `mcp -> service -> storage -> SurrealDB` intact while tightening query pushdown and schema fidelity.

**Tech Stack:** Rust 2024, rmcp, SurrealDB 3.x, chrono, serde/serde_json, existing integration + acceptance tests

---

## Wave mapping

- **Wave 1 — correctness and observability:** Tasks 1-2
- **Wave 2 — graph and performance:** Tasks 3-5
- **Wave 3 — semantic retrieval:** Task 6
- **Wave 4 — lifecycle, migrations, and documentation:** Tasks 7-8
- **Wave 5 — hardening and governance:** Task 9

### Task 1: Restore temporal correctness and query pushdown

**Files:**
- Modify: `src/migrations/__Initial.surql`
- Modify: `src/storage.rs`
- Modify: `src/service/context.rs`
- Test: `tests/embedded_fts_search.rs`
- Test: `tests/service_acceptance.rs`
- Test: `tests/service_integration.rs`

- [ ] **Step 1: Add a failing integration test for `as_of` + text query**
- [ ] **Step 2: Add a failing migration/schema assertion for `datetime` temporal fields**
- [ ] **Step 3: Introduce a versioned migration plan (`002_datetime_types.surql`) in documentation and code scaffolding**
- [ ] **Step 4: Change temporal fields from `string` to `datetime` / `option<datetime>`**
- [ ] **Step 5: Rewrite `select_facts_filtered()` to use one DB query for scope + temporal visibility + text search**
- [ ] **Step 6: Re-run focused FTS and acceptance tests**
- [ ] **Step 7: Run full verification (`cargo check`, `cargo clippy -- -D warnings`, `cargo test`)**

### Task 2: Preserve provenance and make `explain` real

**Files:**
- Modify: `src/service/core.rs`
- Modify: `src/service/episode.rs`
- Modify: `src/models.rs`
- Modify: `src/mcp/handlers.rs`
- Test: `tests/service_integration.rs`
- Test: `tests/tools_e2e.rs`

- [ ] **Step 1: Add a failing test that `add_fact()` persists supplied provenance**
- [ ] **Step 2: Add a failing test that `store_edge()` persists supplied provenance**
- [ ] **Step 3: Remove `_provenance` discard path and persist the provided payload verbatim**
- [ ] **Step 4: Add a failing `explain()` test requiring episode lookup and returned provenance context**
- [ ] **Step 5: Implement the minimum viable `explain()` flow: load source episode, include scope + timestamps + citation context**
- [ ] **Step 6: Re-run explain and provenance tests, then the full suite**

### Task 3: Remove avoidable scans and lock contention

**Files:**
- Modify: `src/storage.rs`
- Modify: `src/service/core.rs`
- Modify: `src/migrations/__Initial.surql`
- Test: `tests/service_integration.rs`
- Test: `tests/embedded_resolve_alias.rs`

- [ ] **Step 1: Add a failing test for indexed entity lookup by canonical name / alias**
- [ ] **Step 2: Add `edge.from_id` and `edge.to_id` indexes to schema/migrations**
- [ ] **Step 3: Replace `find_entity_record()` table scan with a parameterized `SELECT ... WHERE canonical_name = $name LIMIT 1` path**
- [ ] **Step 4: Decide whether alias lookup needs a separate normalized alias index or a follow-up migration**
- [ ] **Step 5: Verify SurrealDB Rust SDK client-sharing constraints and document whether outer `Mutex<Surreal<_>>` can be safely removed**
- [ ] **Step 6: If validation succeeds, replace outer `Mutex<Surreal<_>>` with the approved shared-client strategy; otherwise record the constraint explicitly**
- [ ] **Step 7: Re-run entity-resolution and graph traversal tests**

### Task 4: Fix edge invalidation before changing graph storage model

**Files:**
- Modify: `src/service/episode.rs`
- Modify: `src/models.rs`
- Modify: `src/storage.rs`
- Test: `tests/embedded_invalidate.rs`
- Test: `tests/service_acceptance.rs`

- [ ] **Step 1: Add a failing test for conflicting edge insertion**
- [ ] **Step 2: Define the edge conflict rule precisely: same `(from_id, relation, to_id)` vs broader semantic conflict**
- [ ] **Step 3: Implement invalidation of active conflicting edges before inserting the new edge**
- [ ] **Step 4: Verify `t_invalid` and `t_invalid_ingested` behavior under repeated writes**
- [ ] **Step 5: Re-run acceptance coverage for graph invalidation semantics**

### Task 5: Upgrade from flat edges to native graph relations

**Files:**
- Modify: `src/migrations/__Initial.surql`
- Modify: `src/service/episode.rs`
- Modify: `src/service/core.rs`
- Modify: `src/storage.rs`
- Test: `tests/embedded_context_cache.rs`
- Test: `tests/service_acceptance.rs`

- [ ] **Step 1: Write a design note for relation-table naming (`mentions`, `involved_in`, etc.) and compatibility strategy**
- [ ] **Step 2: Add failing traversal tests that must pass through DB-side relation traversal rather than loading all edges**
- [ ] **Step 3: Introduce `TYPE RELATION` tables / migration path**
- [ ] **Step 4: Rewrite `store_edge()` / `relate()` around `RELATE` semantics**
- [ ] **Step 5: Rewrite `find_intro_chain()` to query the graph instead of materializing all edges in memory**
- [ ] **Step 6: Re-run graph traversal and cache tests**

### Task 6: Add semantic retrieval and extraction scaffolding

**Files:**
- Modify: `src/models.rs`
- Modify: `src/service/core.rs`
- Modify: `src/service/episode.rs`
- Modify: `src/migrations/__Initial.surql`
- Create: `src/service/embedding.rs`
- Create: `src/service/entity_extraction.rs`
- Test: `tests/service_integration.rs`

- [ ] **Step 1: Add failing tests for missing embedding fields / provider abstraction**
- [ ] **Step 2: Introduce `EmbeddingProvider` with a `NullEmbedder` test implementation**
- [ ] **Step 3: Add `embedding` fields and vector indexes in schema using SurrealDB 3.x-compatible index types**
- [ ] **Step 4: Split entity extraction behind a trait so regex fallback is no longer the only implementation path**
- [ ] **Step 5: Keep the existing regex extractor as deterministic fallback for tests**
- [ ] **Step 6: Document hybrid retrieval ranking inputs (FTS + graph + embeddings) before enabling them by default**

### Task 7: Finish communities, lifecycle, and migration discipline

**Files:**
- Modify: `src/service/episode.rs`
- Modify: `src/service/context.rs`
- Modify: `src/storage.rs`
- Modify: `src/migrations/__Initial.surql`
- Create: `src/migrations/002_datetime_types.surql`
- Create: `src/migrations/003_edge_indexes.surql`
- Create: `src/migrations/004_migration_checksums.surql`
- Test: `tests/service_acceptance.rs`
- Test: `tests/service_integration.rs`

- [ ] **Step 1: Add a failing test proving `assemble_context()` currently ignores communities**
- [ ] **Step 2: Replace per-episode community grouping with a graph-based connected-components baseline**
- [ ] **Step 3: Feed community summaries into retrieval only after correctness tests pass**
- [ ] **Step 4: Introduce migration bookkeeping with file name + checksum + executed_at validation**
- [ ] **Step 5: Add startup checks that reject modified-applied migrations**
- [ ] **Step 6: Document a follow-up decay/consolidation background job instead of mixing it into the correctness wave**

### Task 8: Close the documentation loop after code changes

**Files:**
- Modify: `docs/MEMORY_SYSTEM_SPEC.md`
- Modify: `README.md`
- Modify: `docs/REVIEW_ALIGNMENT_2026-03-25.md`

- [ ] **Step 1: Remove any stale `✅ Done` claims invalidated by implementation work**
- [ ] **Step 2: Promote completed roadmap items from “planned” to “implemented” with evidence**
- [ ] **Step 3: Record verification commands and exact results in the changelog**
- [ ] **Step 4: Harmonize `.vscode/mcp.json` examples and any other MCP host configuration snippets with the actual binary/run path**
- [ ] **Step 5: Document Rust build, stdio startup, and environment configuration for local operation**
- [ ] **Step 6: Run a final documentation review for consistency with public MCP behavior**

### Task 9: Security hardening and risk assessment

**Files:**
- Modify: `src/storage.rs`
- Modify: `src/config.rs`
- Modify: `docs/MEMORY_SYSTEM_SPEC.md`
- Modify: `README.md`
- Create: `docs/security-hardening-roadmap.md`

- [ ] **Step 1: Inventory all non-parameterized query paths and classify them by risk**
- [ ] **Step 2: Convert the highest-risk paths to parameterized queries first**
- [ ] **Step 3: Define the minimum RBAC / capabilities-lockdown model needed for local vs remote deployment**
- [ ] **Step 4: Write a repository risk-assessment note covering license, migration drift, compatibility, and operational assumptions**
- [ ] **Step 5: Reconcile the hardening status in README and `docs/MEMORY_SYSTEM_SPEC.md`**
