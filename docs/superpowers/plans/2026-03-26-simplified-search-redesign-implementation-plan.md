# Simplified Search Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove embedding-based search scaffolding and ship a deterministic BM25-first + graph-expansion retrieval pipeline with native Surreal relation endpoints.

**Architecture:** Execute the redesign in five waves. Wave 1 locks the breaking schema/config contract for fresh and existing databases. Wave 2 removes embedding plumbing from the service and storage layers. Wave 3 upgrades graph traversal and lexical search primitives. Wave 4 rewrites context assembly around multilabel query flags, bounded graph expansion, and RRF fusion. Wave 5 cleans up docs and obsolete tests so the repository describes only the simplified runtime.

**Tech Stack:** Rust 2024, rmcp, SurrealDB 3.x, chrono, serde/serde_json, existing integration + acceptance tests

## Status snapshot (updated after actual execution)

- **Wave 1 — mostly complete:** fresh schema/config/model contract locked; startup migration file exists and is registered; the numbered migration still needs a fuller existing-DB upgrade script instead of the current scaffold.
- **Wave 2 — implemented:** embedding service/config/persistence plumbing removed from runtime and tests.
- **Wave 3 — implemented:** native `in` / `out` graph traversal and stronger lexical full-text ordering are in place and verified.
- **Wave 4 — partial:** semantic lookup removal, deterministic fusion, and rationale improvements are implemented; multilabel query flags and bounded graph expansion from the original plan are still not fully landed.
- **Wave 5 — mostly complete:** obsolete tests/comments/docs were cleaned up and the repo-wide verification suite passes; commit-specific checklist items remain intentionally unchecked because no commit was created in this session.

---

## Implementation decisions locked by this plan

- Existing databases migrate through **one new breaking startup migration**: `src/migrations/006_simplified_search_redesign.surql`.
- Fresh databases are created directly from the updated `src/migrations/__Initial.surql` without embedding fields or HNSW indexes.
- `edge` traversal will move to native Surreal relation endpoints (`in` / `out`) while retaining explicit edge metadata fields such as `edge_id`, `relation`, `confidence`, and temporal columns.
- `fact.entity_links` will use the spec-approved interim contract for this wave: `array<string>` containing canonical entity record IDs. This keeps ingestion and retrieval changes focused while still eliminating untyped payloads.
- Query-mode classification will be implemented as a deterministic multilabel flags struct internal to `src/service/context.rs`.

## Wave mapping

- **Wave 1 — lock the breaking schema contract:** Tasks 1-2
- **Wave 2 — remove embedding plumbing:** Tasks 3-4
- **Wave 3 — upgrade lexical and graph primitives:** Tasks 5-6
- **Wave 4 — rewrite context assembly:** Tasks 7-8
- **Wave 5 — documentation and final verification:** Tasks 9-10

### Task 1: Freeze the new schema contract in tests

**Files:**
- Modify: `tests/embedded_fts_search.rs`
- Modify: `tests/service_integration.rs`
- Modify: `src/config.rs`
- Modify: `src/models.rs`

- [x] **Step 1: Write a failing schema test asserting fresh schema no longer contains `embedding` fields or `*_embedding_hnsw` indexes**
- [x] **Step 2: Write a failing config test asserting `SurrealConfig` no longer exposes `embedding_dimension` or reads `SURREALDB_EMBEDDING_DIMENSION`**
- [x] **Step 3: Write a failing model test asserting `Episode`, `Entity`, and `Fact` no longer serialize an `embedding` field**
- [x] **Step 4: Run the focused tests and verify they fail for the expected reasons**
- [ ] **Step 5: Commit the red tests**

### Task 2: Apply the breaking schema migration for fresh and existing databases

**Files:**
- Modify: `src/migrations/__Initial.surql`
- Create: `src/migrations/006_simplified_search_redesign.surql`
- Modify: `src/storage.rs`
- Modify: `tests/common/mod.rs`
- Modify: `tests/embedded_fts_search.rs`

- [x] **Step 1: Write a failing migration test asserting the startup migration registry includes `006_simplified_search_redesign.surql`**
- [x] **Step 2: Write a failing embedded-schema test asserting the analyzer definition uses `memory_fts` with punctuation-aware tokenization**
- [x] **Step 3: Update `__Initial.surql` to remove embedding fields/indexes, define `memory_fts`, and define edge endpoint/index expectations for fresh databases**
- [ ] **Step 4: Add `006_simplified_search_redesign.surql` to drop embedding fields/indexes, replace the analyzer/indexes, and move edge endpoint indexing to `in` / `out` for upgraded databases**
- [x] **Step 5: Remove `embedding_dimension` rendering from `src/storage.rs` and simplify in-memory client constructors accordingly**
- [x] **Step 6: Update `tests/common/mod.rs` helpers to use the simplified in-memory constructor path**
- [x] **Step 7: Run the focused schema/migration tests and verify they pass**
- [ ] **Step 8: Commit the migration wave**

### Task 3: Remove embedding abstractions and config from the service surface

**Files:**
- Modify: `src/config.rs`
- Modify: `src/service/mod.rs`
- Modify: `src/service/core.rs`
- Delete: `src/service/embedding.rs`
- Modify: `tests/service_integration.rs`

- [x] **Step 1: Write a failing compile-targeted test update removing `NullEmbedder` / `EmbeddingProvider` expectations from `tests/service_integration.rs`**
- [x] **Step 2: Remove `embedding_dimension` from `SurrealConfig` and `SurrealConfigBuilder`**
- [x] **Step 3: Remove `EmbeddingProvider`, `NullEmbedder`, and the `embedder` field from `MemoryService`**
- [x] **Step 4: Delete `src/service/embedding.rs` and stop re-exporting embedding types from `src/service/mod.rs`**
- [x] **Step 5: Run `cargo test --test service_integration` and verify the embedding-scaffolding contract is gone**
- [ ] **Step 6: Commit the service-surface cleanup**

### Task 4: Remove embedding persistence and parsing paths

**Files:**
- Modify: `src/service/core.rs`
- Modify: `src/service/episode.rs`
- Modify: `src/models.rs`
- Modify: `src/service/query.rs`
- Test: `tests/service_integration.rs`

- [x] **Step 1: Write a failing integration test asserting ingest/resolve/add_fact persist records without any `embedding` field in the payload**
- [x] **Step 2: Remove embedding writes from `ingest()`, `resolve()`, and `add_fact()`**
- [x] **Step 3: Remove embedding parsing helpers and fields from `episode_from_record()` / `fact_from_record()` / models**
- [x] **Step 4: Replace test fixtures that still construct `embedding: None` with the simplified structs**
- [x] **Step 5: Run focused unit + integration tests for ingest/extract/add_fact paths**
- [ ] **Step 6: Commit the persistence cleanup**

### Task 5: Upgrade edge storage and traversal to native relation endpoints

**Files:**
- Modify: `src/models.rs`
- Modify: `src/service/core.rs`
- Modify: `src/service/episode.rs`
- Modify: `src/storage.rs`
- Test: `tests/service_integration.rs`
- Test: `tests/service_acceptance.rs`

- [x] **Step 1: Write a failing traversal test asserting neighbor queries read native `in` / `out` endpoints rather than `from_id` / `to_id` lookups**
- [x] **Step 2: Write a failing edge-storage test asserting `relate_edge()` persists relation endpoints plus metadata in one operation**
- [x] **Step 3: Update the `Edge` model and parsing helpers to treat relation endpoints as the primary graph identity while keeping `edge_id` / `relation` / temporal metadata explicit**
- [x] **Step 4: Rewrite `build_select_edge_neighbors_query()` and related parsing in `src/storage.rs` to query/order by `in` / `out`**
- [x] **Step 5: Rewrite `store_edge()`, conflict invalidation, and intro-chain/community traversal helpers around native relation endpoints**
- [x] **Step 6: Run focused graph tests for `find_intro_chain`, extraction, and community maintenance**
- [ ] **Step 7: Commit the graph migration**

### Task 6: Strengthen lexical search primitives

**Files:**
- Modify: `src/storage.rs`
- Modify: `tests/embedded_fts_search.rs`
- Modify: `tests/service_acceptance.rs`

- [x] **Step 1: Write a failing embedded test for punctuation/separator equivalence such as `atlas_launch` vs `atlas launch`**
- [x] **Step 2: Write a failing query-builder test asserting lexical search orders by full-text score before deterministic tie-breaks**
- [x] **Step 3: Rewrite `build_select_facts_filtered_query()` to use the new analyzer/index contract and explicit full-text score ordering**
- [x] **Step 4: Remove the per-term fallback assumption from tests that currently depend on whitespace splitting rather than analyzer quality**
- [x] **Step 5: Re-run embedded FTS and acceptance tests**
- [ ] **Step 6: Commit the lexical search upgrade**

### Task 7: Redesign context assembly around multilabel flags and bounded graph expansion

**Files:**
- Modify: `src/service/context.rs`
- Modify: `src/storage.rs`
- Modify: `src/service/core.rs`
- Test: `tests/service_integration.rs`
- Test: `tests/embedded_context_cache.rs`

- [ ] **Step 1: Write a failing unit test for multilabel query-mode detection (`entity-centric + time-scoped`, `relationship + entity-centric`, etc.)**
- [x] **Step 2: Write a failing integration test asserting `assemble_context()` no longer calls `select_facts_by_embedding()`**
- [ ] **Step 3: Introduce a small internal query-flags type plus deterministic query normalization helpers**
- [x] **Step 4: Remove `collect_semantic_facts()` and all semantic-lookup branches from `assemble_context()`**
- [ ] **Step 5: Rework anchor resolution to use canonical-name/alias matches plus `entity_links` from lexical top-N facts**
- [ ] **Step 6: Keep graph expansion bounded by query flags (1 hop for entity-centric, 2 hops for relationship/path)**
- [x] **Step 7: Re-run context, cache, and service integration tests**
- [ ] **Step 8: Commit the context-assembly rewrite**

### Task 8: Implement deterministic fusion and explainable rationales

**Files:**
- Modify: `src/service/context.rs`
- Modify: `src/models.rs`
- Test: `tests/service_integration.rs`
- Test: `tests/tools_e2e.rs`

- [ ] **Step 1: Write a failing test for deterministic RRF ordering with tie-breaks on `t_valid`, confidence, and ID**
- [ ] **Step 2: Write a failing test that each assembled item reports whether it came from lexical retrieval, graph expansion, or both**
- [x] **Step 3: Implement minimal RRF fusion for lexical and graph candidate lists**
- [x] **Step 4: Replace recency-only rationale strings with structured, deterministic explanation strings derived from match source + anchor path**
- [x] **Step 5: Run focused retrieval/explainability tests and then the MCP-level end-to-end tests**
- [ ] **Step 6: Commit the ranking/explainability wave**

### Task 9: Remove obsolete semantic/community assumptions from docs and tests

**Files:**
- Modify: `tests/service_integration.rs`
- Modify: `tests/embedded_fts_search.rs`
- Modify: `README.md`
- Modify: `docs/MEMORY_SYSTEM_SPEC.md`
- Modify: `docs/SEMANTIC_RETRIEVAL_RANKING.md`
- Modify: `docs/SIMPLIFIED_SEARCH_REDESIGN_SPEC.md`

- [x] **Step 1: Delete or rewrite tests that explicitly require semantic embedding scaffolding to exist**
- [x] **Step 2: Rewrite stale comments mentioning whitespace fallback or dormant vector search as intended runtime behavior**
- [x] **Step 3: Update docs to reflect the implemented BM25 + graph runtime rather than the retired semantic scaffolding**
- [x] **Step 4: Run markdown/error validation on touched docs**
- [ ] **Step 5: Commit the cleanup wave**

### Task 10: Final repository verification

**Files:**
- Verify only: workspace-wide

- [x] **Step 1: Run `cargo fmt --all`**
- [x] **Step 2: Run `cargo check`**
- [x] **Step 3: Run `cargo clippy --all-targets -- -D warnings`**
- [x] **Step 4: Run `cargo test`**
- [x] **Step 5: Inspect `git diff --stat` and verify only intended files changed**
- [x] **Step 6: Prepare a concise summary of breaking changes, verification evidence, and follow-up risks**
