# Embedding Provider Design Specification

**Created:** 2026-03-30  
**Status:** Approved for Implementation  
**Target:** Single-binary MCP with local-first embeddings

---

## 1. Overview

This specification defines the embedding provider architecture for memory_mcp v1. The design favors KISS/YAGNI over fully-consistent online migration, leveraging the existing provider abstraction and accepting a small window of acceptable inconsistency during provider migration for personal low-load scenarios.

---

## 2. Current State

The codebase already contains significant embedding infrastructure:

- **Schema:** `fact.embedding` field exists in `__Initial.surql`, `fact.embedding_next` exists in `__EmbeddingNext.surql`, HNSW index `fact_embedding_hnsw` defined in `__Initial.surql:84`
- **Types:** `EmbeddingSchema` and `EmbeddingStatus` exist in `storage.rs:306-326`
- **Storage:** `DbClient` trait has all embedding migration methods, implemented in `SurrealDbClient`
- **Providers:** `EmbeddingProvider` trait exists with `Disabled`, `OpenAiCompatible`, `Ollama` implementations
- **Migration:** `src/service/migration.rs` implements `run_if_needed()` with backfill + cutover
- **Core:** `core.rs` already spawns embedding migration at startup via `tokio::spawn`

What changes with this spec:

- Change `EmbeddingProvider` trait: `dimension()` returns `usize`, remove `detect_dimension()`
- Add `LocalCandle` variant to `EmbeddingProviderKind`, make it the new default
- Implement `LocalCandleEmbeddingProvider` using CPU-only Candle inference
- Fix bug in `EmbeddingSchema::target_matches_config()`
- Fix backfill pagination (remove offset), fix cutover (3 updates, not 1), remove `hnsw_next`
- Add post-cutover repair pass for facts with `embedding IS NONE`

---

## 3. Design Decisions

### 3.1 Default Provider

- **Current behavior:** `EmbeddingProviderKind::Disabled` when `EMBEDDINGS_ENABLED` is not set or `false`
- **New behavior:** `EmbeddingProviderKind::LocalCandle` when `EMBEDDINGS_ENABLED` is not set
- The `Disabled` variant remains for users who explicitly disable embeddings
- External providers (`openai-compatible`, `ollama`) remain optional

### 3.2 What "Disabled" Means

`EMBEDDINGS_ENABLED=false` disables **runtime embedding logic**: no embedding provider is created, no facts are embedded, no semantic search is performed, no migration is triggered.

It does NOT remove the embedding schema or HNSW index from the database. The `fact.embedding` field, `fact.embedding_next` field, and `fact_embedding_hnsw` index exist regardless of the runtime provider setting — they are part of the database schema applied by versioned migrations. The schema and index are inert when embeddings are disabled.

### 3.3 Single Active Index

- Exactly one active index on `fact.embedding`, named `fact_embedding_hnsw`
- `fact.embedding_next` serves as staging during migration only, **never indexed**
- No `hnsw_next`, no `hnsw_active` — use the existing name `fact_embedding_hnsw`

### 3.4 Simplifications (vs. Full-Featured Design)

| Feature | Decision |
|---------|----------|
| Dual-write during migration | ❌ Not implemented — small inconsistency is acceptable |
| Distributed locking | ❌ Not implemented — single MCP instance per DB is v1 constraint |
| Staging index on `embedding_next` | ❌ Not implemented — never indexed |
| Progress in DB | ❌ Not implemented — progress logged only in process memory |
| Runtime model download | ✅ Implemented — download from HuggingFace with retry/resume |
| Master migration | ❌ Not implemented — only versioned immutable migrations |
| `INFO FOR TABLE` queries | ❌ Not implemented — schema tracked in singleton |
| `EMBEDDINGS_AUTO_MIGRATE` | ❌ Not implemented — migration triggered by schema mismatch |

### 3.5 Schema Representation

The existing `EmbeddingSchema` uses separate fields (`provider`, `model`, `dimension`) rather than a composite fingerprint. This is kept as-is. The `active_matches_config()` and `target_matches_config()` methods compare fields individually.

---

## 4. Configuration

### 4.1 Environment Variables

| Variable | Values | Default | Change |
|----------|--------|---------|--------|
| `EMBEDDINGS_ENABLED` | `true`/`false`/not set | not set (treated as enabled for LocalCandle) | **Changed:** default behavior switches from Disabled to LocalCandle |
| `EMBEDDINGS_PROVIDER` | `local-candle`, `openai-compatible`, `ollama` | `local-candle` (when enabled) | **New:** `local-candle` variant |

**Behavior matrix:**

| `EMBEDDINGS_ENABLED` | `EMBEDDINGS_PROVIDER` | Result |
|---------------------|----------------------|--------|
| not set | not set | `LocalCandle` with defaults |
| `true` | not set | `LocalCandle` with defaults |
| `false` | any | `Disabled` |
| `true` | `local-candle` | `LocalCandle` with defaults |
| `true` | `openai-compatible` | `OpenAiCompatible` (requires model/base_url/api_key) |
| `true` | `ollama` | `Ollama` |

### 4.2 Provider-Specific Config

**LocalCandle (default):**
- `EMBEDDINGS_MODEL` — default: `intfloat/multilingual-e5-small`
- `EMBEDDINGS_DIMENSION` — default: `384`
- `EMBEDDINGS_MODEL_DIR` — optional override for model directory path
- `BASE_URL` — not required
- `API_KEY` — not required

**OpenAI-Compatible:**
- `EMBEDDINGS_MODEL` — required
- `EMBEDDINGS_DIMENSION` — required
- `EMBEDDINGS_BASE_URL` — default: `https://api.openai.com/v1`
- `EMBEDDINGS_API_KEY` — required

**Ollama:**
- `EMBEDDINGS_MODEL` — required
- `EMBEDDINGS_DIMENSION` — required
- `EMBEDDINGS_BASE_URL` — default: `http://127.0.0.1:11434`
- `EMBEDDINGS_API_KEY` — optional

### 4.3 Model Assets — Runtime Download

LocalCandle downloads model files from HuggingFace on first launch. Binary stays small. No manual deployment.

**Download target:** HuggingFace Hub (via `hf-hub` crate).

**Cache directory resolution:**
1. If `EMBEDDINGS_MODEL_DIR` is set → use that path
2. Otherwise → `<data_dir>/models/<EMBEDDINGS_MODEL>/`

Where `<data_dir>` is the SurrealDB data directory (from `SurrealConfig::data_dir_or_default()`).

**Required files (downloaded automatically):**
- `tokenizer.json`
- `config.json`
- `model.safetensors`

**Default download:** `intfloat/multilingual-e5-small` from HuggingFace.

**Download resilience:**
- Retry with exponential backoff on network errors (max 3 retries per launch)
- If download is interrupted (partial file), clean up and retry
- If all retries exhausted on one launch, retry on next launch
- Model is cached on disk after successful download — downloaded only once
- If model already exists on disk, skip download entirely
- Download happens during provider initialization, before MCP starts serving
- If download ultimately fails: log warning, fall back to `Disabled` for this launch, retry next launch

**Example cache layout:**
```
<binary_dir>/data/surrealdb/
└── models/
    └── intfloat/multilingual-e5-small/
        ├── tokenizer.json
        ├── config.json
        └── model.safetensors
```

---

## 5. Database Schema

### 5.1 Existing Fields

The following fields and indexes already exist and are NOT new:

```sql
-- Already in __Initial.surql
DEFINE FIELD embedding ON fact TYPE option<array<float>>;
DEFINE INDEX fact_embedding_hnsw ON TABLE fact FIELDS embedding HNSW DIMENSION __FACT_EMBEDDING_DIMENSION__;

-- Already in __EmbeddingNext.surql
DEFINE FIELD embedding_next ON TABLE fact TYPE option<array<float>>;
```

### 5.2 Singleton: `embedding_schema`

Uses a fixed record ID `embedding_schema:embedding` (singleton pattern). Implemented in `storage.rs:1754`.

```rust
EmbeddingSchema {
    provider: String,       // e.g., "local-candle", "openai-compatible"
    model: String,          // e.g., "intfloat/multilingual-e5-small"
    dimension: usize,       // e.g., 384
    status: EmbeddingStatus, // Ready | Migrating | Cutover
    target_provider: Option<String>,
    target_model: Option<String>,
    target_dimension: Option<usize>,
}
```

No UNIQUE INDEX on schema_id — singleton is enforced by fixed record ID.

---

## 6. Provider API

### 6.1 Trait Contract

The `EmbeddingProvider` trait requires:

```rust
trait EmbeddingProvider: Send + Sync {
    fn is_enabled(&self) -> bool;
    fn provider_name(&self) -> &'static str;
    fn dimension(&self) -> usize;
    async fn embed(&self, input: &str) -> Result<Vec<f64>, MemoryError>;
}
```

`dimension()` returns `usize` — every provider must know its dimension upfront. `detect_dimension()` is removed from the trait.

### 6.2 LocalCandle Provider

`LocalCandleEmbeddingProvider` is added to `src/service/embedding.rs`, implementing the current trait contract.

**Implementation details:**

| Method | Return |
|--------|--------|
| `is_enabled()` | `true` |
| `provider_name()` | `"local-candle"` |
| `dimension()` | `self.dimension` (configured, e.g. 384) |
| `embed(input)` | `Vec<f64>` embedding |

**Embedding pipeline (standard sentence-transformers):**
1. Tokenize input via `tokenizers` crate (single tokenizer path)
2. Transformer forward pass via `candle-transformers`
3. Mean pooling over token embeddings
4. L2 normalization
5. Convert internal `Vec<f32>` to `Vec<f64>` (widening, no precision loss)

### 6.3 Disabled Provider

`DisabledEmbeddingProvider`:
- `is_enabled()` → `false`
- `provider_name()` → `"disabled"`
- `dimension()` → returns stored `dimension: usize` (does NOT panic — provider is never asked to embed)
- `embed()` → returns error

### 6.4 External Providers

`OpenAiCompatible` and `Ollama`:
- `dimension()` returns configured `dimension: usize`
- `embed()` makes HTTP request to provider endpoint
- `detect_dimension()` is removed — dimension must be configured explicitly

---

## 7. Runtime Behavior

### 7.1 First Launch

1. Apply versioned migrations (existing: `006` through `010`)
2. Create provider from runtime config
3. If embeddings disabled (`EMBEDDINGS_ENABLED=false`): skip embedding initialization entirely, do not create `embedding_schema`, do not trigger migration
4. If `embedding_schema` record does not exist:
   - The active HNSW index `fact_embedding_hnsw` already exists from `__Initial.surql` migrations
   - Insert `embedding_schema` record with current config and `status=ready`

### 7.2 Normal Launch

| Condition | Behavior |
|-----------|----------|
| `status=ready` and `active_matches_config(config)` | No-op, continue as normal |
| `status=ready` and NOT `active_matches_config(config)` | Start background migration |
| `status=migrating` and `target_matches_config(config)` | Resume migration |
| `status=migrating` and NOT `target_matches_config(config)` | Clear `embedding_next`, reset target, restart migration |
| `status=cutover` after crash | Complete cutover before normal operation |

### 7.3 Search Behavior

- Semantic search always uses `fact.embedding` with the active HNSW index `fact_embedding_hnsw`
- `embedding_next` is never used for search and is never indexed
- During migration, search continues with old embeddings from `fact.embedding`

---

## 8. Migration Algorithm

### 8.1 Phase 1: Backfill

Backfill uses a cursorless loop — same query each iteration, no offset:

```sql
SELECT id, content FROM fact WHERE embedding_next IS NONE LIMIT $batch_size
```

Offset-based pagination is intentionally NOT used because:
- After processing a batch, those records no longer satisfy `WHERE embedding_next IS NONE`
- The next batch naturally picks up the remaining records
- Offset would skip records that shifted into earlier positions

Algorithm:
1. Compute target schema from current provider config
2. Update `embedding_schema` status to `migrating` (via `as_migration_start()`)
3. If new migration (not resume): clear all `embedding_next` via `clear_next_embeddings()`
4. Loop:
   ```sql
   SELECT id, content FROM fact WHERE embedding_next IS NONE LIMIT $batch_size
   ```
5. For each fact: embed `content` field, save to `embedding_next`
6. When batch is empty → trigger cutover

### 8.2 Phase 2: Cutover

Triggered when `get_facts_pending_reembed()` returns empty batch.

**Critical:** Without dual-write, facts that were not re-embedded during migration (new/modified during `migrating`) must NOT keep old embeddings in `fact.embedding`. Otherwise the active index contains vectors from two different embedding spaces, corrupting all semantic search results.

Cutover sequence:
1. Update `embedding_schema` status to `cutover`
2. Drop active HNSW index `fact_embedding_hnsw`
3. Three sequential updates:
   ```sql
   -- Clear old embeddings for facts that were not re-embedded
   UPDATE fact SET embedding = NONE WHERE embedding_next IS NONE;
   -- Copy new embeddings for re-embedded facts
   UPDATE fact SET embedding = embedding_next WHERE embedding_next IS NOT NONE;
   -- Clear staging field
   UPDATE fact SET embedding_next = NONE WHERE embedding_next IS NOT NONE;
   ```
4. Create new `fact_embedding_hnsw` index with target dimension
5. Update `embedding_schema`: set provider/model/dimension to target values, clear targets, status = `ready`

### 8.3 Post-Cutover Repair

After cutover, some facts may have `embedding = NONE` (created/updated during migration, not in backfill batch). These facts will NOT be auto-reembedded on the next startup because `status=ready` and schema matches — migration check is skipped.

Therefore, after cutover completes and `status=ready`, a repair pass runs:

```sql
SELECT id, content FROM fact WHERE embedding IS NONE LIMIT $batch_size
```

For each such fact:
- Compute embedding via the now-active provider
- Write directly to `embedding` (not `embedding_next`)

This loop repeats until no facts with `embedding IS NONE` remain. It closes the logical gap without requiring dual-write.

### 8.4 Inconsistency Window

During `migrating` status:
- New/modified facts may not have `embedding_next` populated
- Search continues using old `fact.embedding` (acceptable for personal low-load)

During cutover (brief window):
- Active index is dropped and rebuilt
- Semantic search unavailable for seconds

After cutover (brief window):
- Facts that were not re-embedded have `embedding = NONE`
- These facts will be absent from semantic search until repair pass completes

### 8.5 Vector Type

- `EmbeddingProvider` trait returns `Vec<f64>`
- Candle native output is `Vec<f32>`
- `LocalCandleEmbeddingProvider` converts `f32 → f64` at the boundary (widening, no precision loss)
- SurrealDB stores as `ARRAY<FLOAT>` (f64)

---

## 9. Bug Fix: `target_matches_config()`

Current implementation (`storage.rs:375`) has a bug:

```rust
// BUG: compares target_model against self.model instead of config model
self.target_model.as_deref() == Some(self.model.as_str())
```

Fix: compare against config values:

```rust
self.target_model.as_deref() == Some(config.model.as_deref().unwrap_or_default())
```

This ensures `ready + different schema` correctly triggers migration instead of always failing the target match.

---

## 10. Module Structure

```
src/
├── config.rs                    # Modified: add LocalCandle, EMBEDDINGS_MODEL_DIR, change default
├── service/
│   ├── mod.rs                    # No changes
│   ├── embedding.rs              # Modified: change trait, add LocalCandle, update factory
│   ├── migration.rs              # Modified: remove hnsw_next, fix backfill, fix cutover, add repair
│   └── core.rs                   # Verify/modify if needed (trait change may affect callers)
├── storage.rs                    # Modified: add LocalCandle to helpers, fix bug, fix cutover, add repair methods
├── migrations/                   # No changes to existing files
└── Cargo.toml                    # Modified: add candle-core, candle-nn, candle-transformers, tokenizers
```

Model files are downloaded from HuggingFace on first launch and cached on disk:
```
<data_dir>/models/intfloat/multilingual-e5-small/
├── tokenizer.json
├── config.json
└── model.safetensors
```

---

## 11. What We Deliberately Don't Do

| Reason | Exclusion |
|--------|-----------|
| KISS — user accepts small inconsistency | No dual-write during migration |
| v1 constraint | No distributed locking / multi-instance |
| Simplicity | No staging index on embedding_next |
| Single-instance assumption | No progress persistence in DB |
| Immutable migrations principle | No master migration editable post-hoc |

---

## 12. Acceptance Criteria

- [ ] Without `EMBEDDINGS_ENABLED` env var, system starts with LocalCandle (dimension=384)
- [ ] With `EMBEDDINGS_ENABLED=false`, system uses Disabled provider (no runtime embedding logic)
- [ ] With `EMBEDDINGS_ENABLED=false`, DB schema still contains `fact.embedding` and `fact_embedding_hnsw` (inert)
- [ ] LocalCandle downloads model from HuggingFace on first launch
- [ ] Downloaded model is cached on disk — no re-download on subsequent launches
- [ ] Download retries with backoff on network errors (max 3 retries)
- [ ] If download fails after retries, system falls back to `Disabled` for this launch
- [ ] On next launch, download is retried until successful
- [ ] `EMBEDDINGS_MODEL_DIR` overrides the cache directory path
- [ ] Migration `local-candle → openai-compatible` works via same migration flow
- [ ] Migration `openai-compatible → local-candle` works via same migration flow
- [ ] Changing target during `migrating` clears `embedding_next` and restarts from scratch
- [ ] After cutover: `embedding_next` is NULL, schema updated, `status=ready`
- [ ] After cutover: facts without `embedding_next` have `embedding = NONE` (no mixed spaces)
- [ ] After cutover: repair pass fills in all `embedding IS NONE` facts
- [ ] Backfill loop never skips records (no offset)
- [ ] Only one HNSW index exists at any time (`fact_embedding_hnsw`)
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo test` passes

---

## 13. Dependencies

### Cargo.toml
- `candle-core` — tensor computation
- `candle-nn` — neural network layers
- `candle-transformers` — BERT encoder (required for sentence-transformers pipeline)
- `tokenizers` — tokenizer loading (single tokenizer path)
- `hf-hub` — HuggingFace Hub client for model download

### Runtime Model Assets (downloaded automatically on first launch)
- `<data_dir>/models/intfloat/multilingual-e5-small/tokenizer.json`
- `<data_dir>/models/intfloat/multilingual-e5-small/config.json`
- `<data_dir>/models/intfloat/multilingual-e5-small/model.safetensors`

Downloaded from HuggingFace Hub. Cached on disk. Path overridable via `EMBEDDINGS_MODEL_DIR`.

---

## 14. Documentation Updates

README.md should describe:
- LocalCandle as default embedded provider
- How to disable embeddings (`EMBEDDINGS_ENABLED=false`)
- How to configure external providers (openai-compatible, ollama)
- Migration flow and acceptable inconsistency during migration
- What "disabled" means: runtime embedding logic is skipped, DB schema/index remain inert
- v1 limitation: single MCP instance per DB

---

## 15. Related Documents

- `SIMPLIFIED_SEARCH_REDESIGN_SPEC.md` — Previous design that removed embeddings (now partially reversed)
- `MEMORY_SYSTEM_SPEC.md` — System-level specification
