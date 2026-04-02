# Embedding Provider Implementation Plan

**Created:** 2026-03-30  
**Depends on:** `docs/superpowers/specs/2026-03-30-embedding-provider-design.md`  
**Goal:** Implement LocalCandle as default embedding provider with migration support

---

## Overview

Most embedding infrastructure already exists in the codebase. This plan focuses on:
1. **Trait change** — `dimension() -> usize`, remove `detect_dimension()`
2. **Config** — Add `LocalCandle` to `EmbeddingProviderKind`, change default behavior
3. **Provider** — Implement `LocalCandleEmbeddingProvider` in `embedding.rs`
4. **Bug fix** — Fix `target_matches_config()` comparison
5. **Migration fixes** — Remove `hnsw_next`, fix backfill, fix cutover, add repair pass
6. **Documentation** — Update README

---

## Pre-existing Code (Not Re-Implemented)

The following already exist and should NOT be duplicated:

| Component | Location | Status |
|-----------|----------|--------|
| `embedding_next` field | `__EmbeddingNext.surql` | ✅ Exists |
| `embedding` field + `fact_embedding_hnsw` index | `__Initial.surql:84` | ✅ Exists |
| `EmbeddingSchema` / `EmbeddingStatus` | `storage.rs:306-326` | ✅ Exists |
| `DbClient` embedding methods | `storage.rs:229-296` | ✅ Exists (trait defaults) |
| `SurrealDbClient` embedding impl | `storage.rs:1749-1884` | ✅ Exists |
| `EmbeddingProvider` trait | `embedding.rs:20-36` | ✅ Exists (needs change) |
| `DisabledEmbeddingProvider` | `embedding.rs:39-75` | ✅ Exists (needs change) |
| `OpenAiCompatibleEmbeddingProvider` | `embedding.rs:77-283` | ✅ Exists (needs change) |
| `OllamaEmbeddingProvider` | `embedding.rs:85-361` | ✅ Exists (needs change) |
| `create_embedding_provider()` | `embedding.rs:92-134` | ✅ Exists (needs change) |
| `run_if_needed()` migration | `migration.rs:12-123` | ✅ Exists (needs fixes) |
| Core startup migration spawn | `core.rs:154-175` | ✅ Exists (verify) |

---

## Phase 1: Config and Bugfix

### Task 1.1: Add LocalCandle to EmbeddingProviderKind

**File:** `src/config.rs`

- [ ] Add `LocalCandle` variant to `EmbeddingProviderKind` enum
- [ ] Add `EMBEDDINGS_MODEL_DIR` parsing (optional, used by LocalCandle)
- [ ] Update `EmbeddingConfig::from_env()` logic:
  - If `EMBEDDINGS_ENABLED` is `"false"` → `Disabled`
  - If `EMBEDDINGS_ENABLED` is not set or `"true"` AND `EMBEDDINGS_PROVIDER` is not set → `LocalCandle`
  - If `EMBEDDINGS_ENABLED` is not set or `"true"` AND `EMBEDDINGS_PROVIDER` is set → parse provider
- [ ] Add `"local-candle"` to provider match arm
- [ ] Add `LocalCandle` to `base_url` match arm (returns `None`)
- [ ] Update existing tests that assert `Disabled` as default — they should now expect `LocalCandle`
- [ ] Add test: no env vars → `LocalCandle` with dim=384
- [ ] Add test: `EMBEDDINGS_ENABLED=false` → `Disabled`
- [ ] Run `cargo check && cargo clippy -- -D warnings`

### Task 1.2: Fix EmbeddingSchema helpers

**File:** `src/storage.rs`

- [ ] Add `"local-candle"` to provider name matching in:
  - `EmbeddingSchema::from_config()` (line 331)
  - `active_matches_config()` (line 350)
  - `target_matches_config()` (line 366)
  - `as_migration_start()` (line 393)
- [ ] **Bug fix** in `target_matches_config()` (line 375):
  ```rust
  // Current (buggy):
  self.target_model.as_deref() == Some(self.model.as_str())
  // Fixed:
  self.target_model.as_deref() == Some(config.model.as_deref().unwrap_or_default())
  ```
- [ ] Run `cargo check && cargo clippy -- -D warnings`

---

## Phase 2: Provider API Change

### Task 2.1: Change EmbeddingProvider trait

**File:** `src/service/embedding.rs`

The current trait (`embedding.rs:20-36`) has:
```rust
fn dimension(&self) -> Option<usize>;
async fn detect_dimension(&self) -> Result<usize, MemoryError>;
```

Change to:
```rust
fn dimension(&self) -> usize;
// detect_dimension() removed
```

Every provider must know its dimension upfront. No auto-detection.

- [ ] Change trait signature: `dimension(&self) -> usize`
- [ ] Remove `detect_dimension()` from trait
- [ ] Update `DisabledEmbeddingProvider`:
  - Constructor takes `dimension: usize` instead of `Option<usize>`
  - `dimension()` returns `self.dimension` (does NOT panic — provider is never asked to embed)
  - Remove `detect_dimension()` impl
- [ ] Update `OpenAiCompatibleEmbeddingProvider`:
  - Store `dimension: usize` (config must provide dimension)
  - `dimension()` returns `self.dimension`
  - Remove `detect_dimension()` impl
- [ ] Update `OllamaEmbeddingProvider`:
  - Store `dimension: usize`
  - `dimension()` returns `self.dimension`
  - Remove `detect_dimension()` impl
- [ ] Update all callers of `dimension()` and `detect_dimension()` in the codebase (check `core.rs`, tests, etc.)
- [ ] Run `cargo check && cargo clippy -- -D warnings`

### Task 2.2: Add Cargo.toml dependencies

**File:** `Cargo.toml`

- [ ] Add `candle-core` (CPU-only features)
- [ ] Add `candle-nn`
- [ ] Add `candle-transformers` (required for BERT encoder stack)
- [ ] Add `tokenizers` (single tokenizer path)
- [ ] Add `hf-hub` (HuggingFace Hub client for model download)
- [ ] Run `cargo check` to verify dependency resolution

### Task 2.3: Model download module

**File:** `src/service/embedding/model_loader.rs` (new)

- [ ] Create model download/cache module
- [ ] Implement `ensure_model_cached(model_name, cache_dir)`:
  1. Check if `<cache_dir>/<model_name>/` contains all required files
  2. If all files present → return (skip download)
  3. If missing → download from HuggingFace using `hf-hub`
  4. Retry with exponential backoff on network errors (max 3 retries)
  5. If interrupted (partial file) → clean up `.tmp` files, retry
  6. If all retries fail → return error (caller decides what to do)
- [ ] Required files: `tokenizer.json`, `config.json`, `model.safetensors`
- [ ] Use `reqwest` with timeout for HTTP downloads (already in project dependencies)
- [ ] Log download progress

---

## Phase 3: LocalCandle Provider

### Task 3.1: LocalCandleEmbeddingProvider

**File:** `src/service/embedding.rs`

- [ ] Add `struct LocalCandleEmbeddingProvider` with fields for model name, dimension, tokenizer, model
- [ ] Implement `EmbeddingProvider` trait:
  - `is_enabled()` → `true`
  - `provider_name()` → `"local-candle"`
  - `dimension()` → `self.dimension` (returns `usize`)
  - `embed(input)` → run pipeline, return `Vec<f64>`
- [ ] Embedding pipeline (standard sentence-transformers):
  1. Tokenize input via `tokenizers` crate
  2. Transformer forward pass via `candle-transformers`
  3. Mean pooling over token embeddings
  4. L2 normalization
  5. Convert `Vec<f32>` → `Vec<f64>` at boundary
- [ ] Load tokenizer from `<cache_dir>/tokenizer.json` (file on disk, loaded at init)
- [ ] Load config from `<cache_dir>/config.json` (file on disk, loaded at init)
- [ ] Load model from `<cache_dir>/model.safetensors` (file on disk, loaded at init)
- [ ] Constructor calls `model_loader::ensure_model_cached()` before loading files
- [ ] If download fails: return error → caller (`create_embedding_provider`) falls back to `Disabled`

### Task 3.2: Update create_embedding_provider()

**File:** `src/service/embedding.rs`

- [ ] Add `EmbeddingProviderKind::LocalCandle` match arm in `create_embedding_provider()` (line 102)
- [ ] Create `LocalCandleEmbeddingProvider` with model/dimension from config
- [ ] Run `cargo check && cargo clippy -- -D warnings`

### Task 3.3: Provider tests

- [ ] Test: `provider_name()` returns `"local-candle"`
- [ ] Test: `dimension()` returns configured dimension (as `usize`)
- [ ] Test: `embed()` returns vector of expected length
- [ ] Test: output vectors are L2-normalized
- [ ] Test: model is cached after first download — no re-download on second init
- [ ] Test: missing model files trigger download from HuggingFace

---

## Phase 4: Migration Fixes

### Task 4.1: Remove `hnsw_next` staging index

**File:** `src/service/migration.rs`

- [ ] Remove `create_hnsw_index("embedding_next", "hnsw_next", ...)` call (line 55)
- [ ] Remove `drop_hnsw_index("hnsw_next", ...)` call (line 117)
- [ ] `embedding_next` remains a field but is never indexed

### Task 4.2: Fix backfill — remove offset

The offset-based backfill has a bug: after processing a batch at offset 0, those records no longer match `WHERE embedding_next IS NONE`, so subsequent offset values skip records.

**File:** `src/storage.rs`

- [ ] Change `get_facts_pending_reembed()` signature: remove `offset` parameter
- [ ] Change SQL from `LIMIT {} START {}` to just `LIMIT {}`
- [ ] Update `DbClient` trait default signature
- [ ] Update `SurrealDbClient` implementation

**File:** `src/service/migration.rs`

- [ ] Remove `offset` variable and its increment
- [ ] Change loop to call `db.get_facts_pending_reembed(batch_size, ns)` (no offset)

### Task 4.3: Fix cutover — 3 updates instead of 1

**File:** `src/storage.rs`

Current `apply_cutover()` (line 1862) only does:
```sql
UPDATE fact SET embedding = embedding_next, embedding_next = NONE WHERE embedding_next IS NOT NONE
```

This leaves facts without `embedding_next` with old embeddings from previous provider — mixing embedding spaces.

**Fix — three sequential updates:**
- [ ] Update `apply_cutover()` to:
  ```sql
  UPDATE fact SET embedding = NONE WHERE embedding_next IS NONE;
  UPDATE fact SET embedding = embedding_next WHERE embedding_next IS NOT NONE;
  UPDATE fact SET embedding_next = NONE WHERE embedding_next IS NOT NONE;
  ```

**File:** `src/service/migration.rs`

- [ ] Update cutover sequence to use existing index name `fact_embedding_hnsw`:
  ```rust
  db.drop_hnsw_index("fact_embedding_hnsw", ns).await?;
  db.apply_cutover(ns).await?;
  db.create_hnsw_index("embedding", "fact_embedding_hnsw", target_dim, ns).await?;
  ```
- [ ] Replace all `"hnsw_active"` references with `"fact_embedding_hnsw"`

### Task 4.4: Add post-cutover repair pass

**File:** `src/service/migration.rs`

After cutover sets `status=ready`, some facts may have `embedding = NONE` (created/updated during migration). They will NOT auto-fix on next startup because `status=ready` + schema match = no migration triggered.

- [ ] After cutover completes, run repair loop:
  ```rust
  loop {
      let batch = db.get_facts_without_embedding(batch_size, ns).await?;
      if batch.is_empty() { break; }
      for (id, content) in &batch {
          if let Ok(vec) = provider.embed(content).await {
              db.set_fact_embedding(id, vec, ns).await?;
          }
      }
  }
  ```

**File:** `src/storage.rs`

- [ ] Add `get_facts_without_embedding(limit, ns)` method:
  ```sql
  SELECT id, content FROM fact WHERE embedding IS NONE LIMIT $batch_size
  ```
- [ ] Add `set_fact_embedding(id, vec, ns)` method:
  ```sql
  UPDATE $id SET embedding = $vec
  ```
- [ ] Add to `DbClient` trait and `SurrealDbClient` impl

### Task 4.5: Migration tests

- [ ] Test: backfill without offset processes all facts
- [ ] Test: cutover zeros out `embedding` for facts without `embedding_next`
- [ ] Test: repair pass fills in `embedding IS NONE` facts after cutover
- [ ] Test: only `fact_embedding_hnsw` index exists (no `hnsw_next`, no `hnsw_active`)

---

## Phase 5: Integration and Verification

### Task 5.1: Core integration

**File:** `src/service/core.rs`

The trait change (`dimension() -> usize`, no `detect_dimension()`) may affect callers in `core.rs`.

- [ ] Verify that `create_embedding_provider()` now handles `LocalCandle`
- [ ] Verify fallback behavior: if LocalCandle init (including download) fails → fall back to `Disabled`
- [ ] Search for all `detect_dimension()` calls and remove/replace with `dimension()`
- [ ] Search for all `dimension()` callers that expect `Option<usize>` and update to `usize`
- [ ] Run `cargo check && cargo clippy -- -D warnings`

### Task 5.2: End-to-End Tests

- [ ] Test: fresh DB starts with LocalCandle, creates `fact_embedding_hnsw` index, embeds facts correctly
- [ ] Test: semantic search works with LocalCandle embeddings
- [ ] Test: migration from LocalCandle → openai-compatible (mock provider)
- [ ] Test: migration from openai-compatible → LocalCandle (mock provider)
- [ ] Test: `target_matches_config()` correctly identifies schema mismatch (bug fix)
- [ ] Test: after cutover, no mixed embedding spaces exist
- [ ] Test: repair pass fills all `embedding IS NONE` after cutover

### Task 5.3: Documentation Updates

**File:** `README.md`

- [ ] Document: LocalCandle as default embedded provider
- [ ] Document: model auto-downloads from HuggingFace on first launch
- [ ] Document: download retry behavior (retries with backoff, resumes next launch if failed)
- [ ] Document: model cache location and `EMBEDDINGS_MODEL_DIR` override
- [ ] Document: how to disable embeddings (`EMBEDDINGS_ENABLED=false`)
- [ ] Document: what "disabled" means — runtime logic skipped, DB schema/index remain inert
- [ ] Document: how to configure external providers (openai-compatible, ollama)
- [ ] Document: migration flow and acceptable inconsistency during migration
- [ ] Document: v1 limitation (single MCP instance per DB)

### Task 5.4: Final Verification

- [ ] `cargo fmt`
- [ ] `cargo clippy -- -D warnings`
- [ ] `cargo test`
- [ ] `cargo doc --no-deps`

---

## Dependencies

| Task | Dependencies |
|------|--------------|
| Config (1.1) | None |
| Schema helpers (1.2) | None |
| Trait change (2.1) | None |
| Cargo.toml (2.2) | None |
| Model assets (2.3) | None |
| LocalCandle provider (3.1) | Trait change (2.1), Cargo.toml (2.2), Model download module (2.3) |
| create_embedding_provider (3.2) | LocalCandle provider (3.1) |
| Migration fixes (4.1-4.3) | None (can be done in parallel with Phase 2-3) |
| Repair pass (4.4) | Cutover fix (4.3) |
| Core integration (5.1) | Config (1.1), create_embedding_provider (3.2), Trait change (2.1) |
| E2E tests (5.2) | All of above |

---

## Files to Modify

| File | Changes |
|------|---------|
| `src/config.rs` | Add `LocalCandle` to enum, update `from_env()` logic, change default behavior |
| `src/storage.rs` | Add `LocalCandle` to provider matching in 4 methods, fix `target_matches_config()` bug, fix `apply_cutover()` to 3 updates, add `get_facts_without_embedding()` and `set_fact_embedding()`, remove `offset` from `get_facts_pending_reembed()` |
| `src/service/embedding.rs` | Change trait (`dimension() -> usize`, remove `detect_dimension()`), update all impls, add `LocalCandleEmbeddingProvider`, update `create_embedding_provider()` |
| `src/service/embedding/model_loader.rs` | New: model download/cache module with retry/resume |
| `src/service/migration.rs` | Remove `hnsw_next` creation/dropping, fix backfill (remove offset), fix cutover sequence, unify index name to `fact_embedding_hnsw`, add repair pass |
| `src/service/core.rs` | Verify/modify if needed — trait change may require updating callers of `dimension()` and `detect_dimension()` |
| `Cargo.toml` | Add `candle-core`, `candle-nn`, `candle-transformers`, `tokenizers`, `hf-hub` |
| `README.md` | Document LocalCandle, config, disabled semantics, migration flow, auto-download

## Files to Add

| File | Purpose |
|------|---------|
| `src/service/embedding/model_loader.rs` | Model download/cache module with retry/resume |

Model files are NOT in the repository — downloaded from HuggingFace at runtime and cached on disk.

## Files NOT to Modify

| File | Reason |
|------|--------|
| `src/migrations/*.surql` | No new migrations needed |
| `src/service/mod.rs` | Exports already correct |

---

## Notes

- Model files (~80MB for intfloat/multilingual-e5-small) downloaded from HuggingFace on first launch — binary stays small
- Download is resilient: retry with backoff, resume on interruption, fallback to `Disabled` if exhausted
- On next launch, download retry resumes until successful
- Model cached on disk after successful download — no re-download
- CPU inference may be slow for large fact bases — acceptable for personal/low-load usage
- `f32 → f64` conversion at provider boundary is widening (no precision loss)
- `dimension()` changes from `Option<usize>` to `usize` — all providers must know dimension upfront
- `detect_dimension()` is removed — dimension must be configured explicitly for all providers
- Repair pass is lightweight: only runs for `embedding IS NONE` facts, typically small count
- `DisabledEmbeddingProvider::dimension()` returns stored value, does NOT panic
- `EMBEDDINGS_ENABLED=false` means: skip runtime embedding logic; DB schema and index remain inert
- `hf-hub` crate provides HuggingFace Hub client; `reqwest` (already in deps) handles HTTP
