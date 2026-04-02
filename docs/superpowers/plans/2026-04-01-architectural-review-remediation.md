# Architectural Review Remediation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close real gaps identified by the 2026-04-01 architectural review, focusing on GLiNER NMS, embedding correctness, and migration registration.

**Architecture:** The review identified issues across three subsystems. After verification, most GLiNER claims were already addressed; remaining work covers IoU-based NMS, embedding time-bombs, and unregistered SurrealDB migrations. This plan addresses verified issues only; many review claims were found already implemented or intentionally designed (see §Validation Notes).

**Tech Stack:** Rust 2024, Candle (tensor inference), SurrealDB, tokenizers, safetensors

---

## Validation Notes — Review Claims vs Actual Code

The following review/addendum findings were **verified against source and found already implemented, inaccurate, or intentionally designed**. These are excluded from the plan:

| Review Claim | Actual State | Verdict |
|---|---|---|
| GLiNER missing sigmoid before threshold | Already implemented at `gliner_entity_extractor.rs:750-751` in `extract_spans`: `let prob = 1.0_f32 / (1.0_f32 + (-score).exp())` applied before `prob >= self.threshold` | ❌ Already done |
| `find_episodes_via_entity` is a stub | Fully implemented at `core.rs:1423-1450` — traverses entity → edge → fact → episode | ❌ Already done |
| BM25 analyzer uses primitive tokenizer | `memory_fts` already uses `TOKENIZERS class FILTERS lowercase, ascii, snowball(english)` in migrations 006 and 009 | ❌ Already done |
| GLiNER sliding window offsets are window-relative | `window_offsets` sliced from absolute `encoding.get_offsets()` at `gliner_entity_extractor.rs:813-814`; `extract_spans` uses absolute byte offsets into original `text` | ❌ Already correct |
| `SpanRepresentationLayer` doesn't match real weights | Code at line 160-163 already fixed to single-prefix path; test `gliner_projection_heads_load_actual_prefixes` confirms keys match `span_rep_layer.project_start.0.weight` (NOT double-prefix). Real `urchade/gliner_multi-v2.1` weights use FFN heads (`project_start`/`project_end`/`out_project`), not `span_reps` parameter. | ❌ Already correct |
| No GLiNER integration tests with real weights exist | Outdated. `tests/gliner_integration.rs` contains ignored end-to-end tests against real `urchade/gliner_multi-v2.1` weights, and `tests/common/mod.rs` wires GLiNER fixtures into eval helpers. Remaining gap is narrower: there is still no regression for long-text window-boundary entities. | ❌ Already done (with narrower follow-up) |
| No LocalCandle integration coverage with real weights exists | Outdated. `tests/eval_retrieval.rs::semantic_retrieval_fires_when_local_provider_enabled` uses `tests/common/mod.rs::make_service_with_local_embeddings()` and a real `tests/fixtures/multilingual-e5-small/` LocalCandle provider. Remaining gap is narrower: there is still no direct model-dimension probe or semantic-similarity regression at the provider boundary. | ❌ Already done (with narrower follow-up) |
| Migration 008 contradicts 006 and should be removed | **Intentional two-pass design.** Tests `versioned_migrations_include_simplified_search_redesign` (line 3029), `versioned_migrations_keep_runtime_upgrade_scripts_in_order` (line 3047, `len() == 5`), and `versioned_migration_008_contains_executable_statements` (line 3098) all hard-assert 008's presence. 006 removes legacy embedding fields; 008 re-adds new semantic embedding support. This is rolling-migration pattern, not a contradiction. | ❌ Intentional |
| Embedding fields in `__Initial.surql` should be removed | Test `connect_in_memory_fresh_install_provisions_full_post_redesign_schema` (line 2696-2697) explicitly asserts `json_contains_text(&fact_json, "embedding")` and `json_contains_text(&fact_json, "fact_embedding_hnsw")`. Fresh-install schema intentionally includes embedding support. | ❌ Intentional |
| `entity_links` needs migration to `array<record<entity>>` | May be desirable for future DB-side pushdown but not blocking — current string-based approach works with existing queries | ⏸ Deferred (not a bug) |
| MCP surface needs simplification | User explicitly excluded from scope | ⏸ Out of scope |
| GLiNER sliding window overlap is sufficient | Verified gap. `extract_inner()` still uses `let step = max_text_tokens.saturating_sub(1).max(1);`, which leaves only a 1-token overlap between windows. This is too small for entity spans that cross chunk boundaries. | ✅ Add follow-up task |
| GLiNER local inference already yields to the async runtime | Verified gap. `EntityExtractor for GlinerEntityExtractor` still calls `self.extract_inner(content)` directly in `async fn extract_candidates`, so CPU-heavy inference runs on the tokio worker thread. | ✅ Add follow-up task |
| LocalCandle E5 prefix accounting is tokenizer-derived | Verified gap. `embed_sync()` still hardcodes `const E5_PREFIX_TOKENS: usize = 2`, so chunk-boundary behavior depends on an assumption about the current tokenizer. | ✅ Add follow-up task |
| LocalCandle validates configured dimension against actual model output at init | Verified gap. `LocalCandleEmbeddingProvider::new()` stores the configured dimension but does not run a probe embedding to assert it matches the real model output dimension. | ✅ Add follow-up task |

---

## File Map

| File | Role in this plan |
|---|---|
| `src/service/gliner_entity_extractor.rs` | Task 1: IoU NMS |
| `src/service/embedding.rs` | Tasks 2–4: L2 norm, E5 prefix guard, block_in_place |
| `src/storage.rs` | Task 5: Register 011/012 + update test assertions |
| `src/migrations/012_app_sessions.surql` | Task 5: Verify exists and is registered |
| `tests/gliner_integration.rs` | Verified existing real-model GLiNER integration coverage; extend with long-text boundary regression |
| `tests/eval_retrieval.rs` | Verified existing LocalCandle real-model integration coverage; extend with direct provider-level expectations |

---

## Wave 1 — GLiNER NMS (P1)

### Task 1: Replace any-overlap NMS with IoU-based NMS

**Problem:** Current NMS at `gliner_entity_extractor.rs:767-783` suppresses any partial overlap within the same label. This aggressively suppresses valid nested entities — e.g., if "New York" (score 0.8) is kept first, "New York City" (score 0.7) gets suppressed despite being a distinct, longer entity. Standard NER NMS uses IoU threshold (typically 0.5), which allows spans with low overlap to coexist.

**Note:** Current code already correctly includes `k.label == span.label` check (label-aware). The fix is only in the overlap criterion.

**Files:**
- Modify: `src/service/gliner_entity_extractor.rs:767-783`

- [x] **Step 1: Rewrite `apply_nms` with IoU threshold**

```rust
fn apply_nms(&self, mut spans: Vec<ScoredSpan>) -> Vec<ScoredSpan> {
    const IOU_THRESHOLD: f32 = 0.5;

    spans.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

    let mut kept: Vec<ScoredSpan> = Vec::new();

    for span in spans {
        let dominated = kept.iter().any(|k| {
            if k.label != span.label {
                return false;
            }
            let inter_start = span.start.max(k.start);
            let inter_end = span.end.min(k.end);
            if inter_start >= inter_end {
                return false;
            }
            let intersection = (inter_end - inter_start) as f32;
            let union = (span.end - span.start + k.end - k.start) as f32 - intersection;
            intersection / union > IOU_THRESHOLD
        });

        if !dominated {
            kept.push(span);
        }
    }

    kept
}
```

- [x] **Step 2: Run `cargo check` and `cargo clippy`**

- [x] **Step 3: Run tests**

- [x] **Step 4: Commit**

```bash
git add src/service/gliner_entity_extractor.rs
git commit -m "fix(gliner): use IoU-based NMS instead of any-overlap suppression"
```

---

## Wave 2 — Embedding Correctness Fixes (P2/P3)

### Task 2: Fix L2 normalization to per-sample (time-bomb fix, P2)

**Problem:** `embed_inner` at `embedding.rs:424-434` uses `sum_all()` which computes a single scalar norm across the entire batch tensor. Today `pooled` always has shape `[1, hidden_size]` (batch_size=1, from `unsqueeze(0)` at line 384), so `sum_all()` produces the same result as per-sample sum. **This is a latent time-bomb** — if batch inference is ever introduced, L2 normalization will silently produce wrong vectors. Not a current production defect.

**Severity:** P2 (time-bomb, not active bug). Fix now while the code is fresh.

**Files:**
- Modify: `src/service/embedding.rs:424-434`

- [x] **Step 1: Replace `sum_all()` with per-dimension sum**

- [x] **Step 2: Verify with existing test**

- [x] **Step 3: Commit**

```bash
git add src/service/embedding.rs
git commit -m "fix(embedding): use per-sample L2 normalization instead of batch-global"
```

---

### Task 3: Fix max_tokens guard to account for E5 prefix (P3)

**Problem:** `embed()` at `embedding.rs:464-472` checks `token_count <= self.max_tokens` on the raw input. Then `embed_inner` prepends `"query: "` (line 371) and re-tokenizes. The prefix adds ~2 tokens, so texts at the boundary (`token_count == max_tokens`) will have `max_tokens + 2` tokens after prefix in `embed_inner`, silently exceeding the limit.

**Severity:** P3. The edge case only triggers on texts with exactly `max_tokens` tokens (typically 512) — rare in practice, but a correctness gap.

**Files:**
- Modify: `src/service/embedding.rs:463-476`

- [x] **Step 1: Adjust the threshold check**

- [x] **Step 2: Run `cargo test --lib embedding`**

- [x] **Step 3: Commit**

```bash
git add src/service/embedding.rs
git commit -m "fix(embedding): account for E5 prefix tokens in max_tokens guard"
```

---

### Task 4: Move sync BERT inference off async runtime (P2)

**Problem:** `embed_inner` is a sync function doing CPU-heavy BERT inference (tokenize → forward → pool → normalize), called directly from `async fn embed`. This blocks the tokio worker thread for the duration of inference (potentially hundreds of ms), reducing throughput for concurrent MCP requests.

**Constraint:** `EmbeddingProvider::embed` takes `&self` (trait contract). `tokio::task::spawn_blocking` requires `'static` closure, so `&self` cannot be moved in. Two approaches:

- **Approach A (minimal, this task):** Use `tokio::task::block_in_place` which allows non-`'static` borrows. Requires multi-threaded runtime (default for `#[tokio::main]`; verify no `flavor = "current_thread"` in `main.rs`).
- **Approach B (future, if trait is refactored):** Change `EmbeddingProvider::embed` to take `self: Arc<Self>`, enabling `spawn_blocking`. Larger refactor, separate task.

**Files:**
- Modify: `src/service/embedding.rs:463-492` (the `EmbeddingProvider` impl for `LocalCandleEmbeddingProvider`)

- [x] **Step 1: Verify runtime flavor in `main.rs`**

- [x] **Step 2: Extract `embed_sync` private method**

- [x] **Step 3: Run `cargo check` and `cargo test`**

- [x] **Step 4: Commit**

```bash
git add src/service/embedding.rs
git commit -m "fix(embedding): move sync BERT inference off async runtime via block_in_place"
```

---

## Wave 3 — Migration Registration (P0)

### Task 5: Register migrations 011 and 012 in versioned list

**Problem:** `011_ingestion_draft.surql` (APP-03: draft_ingestion + draft_item tables) and `012_app_sessions.surql` (app_session table) exist as files but are NOT listed in `versioned_migrations()` at `storage.rs:852-875`. These migrations will never be applied on startup for existing databases. APP-03 and APP-04 features depend on these tables.

**Cascade:** Adding 011/012 changes `versioned_migrations()` length from 5 to 7. Two existing tests hard-assert on the current state and will break without updates:
- `versioned_migrations_keep_runtime_upgrade_scripts_in_order` (line 3047): asserts `migrations.len() == 5` and index-specific file names
- `versioned_migrations_include_simplified_search_redesign` (line 3015): asserts presence of 006-010 but does not assert 011/012

Both tests must be updated as part of this task.

**Files:**
- Modify: `src/storage.rs:852-875` (`versioned_migrations()`)
- Modify: `src/storage.rs:3044-3069` (`versioned_migrations_keep_runtime_upgrade_scripts_in_order`)
- Modify: `src/storage.rs:3015-3041` (`versioned_migrations_include_simplified_search_redesign`)
- Verify: `src/migrations/012_app_sessions.surql` exists

- [x] **Step 1: Verify both migration files exist**

- [x] **Step 2: Add 011 and 012 to `versioned_migrations()`**

- [x] **Step 3: Update `versioned_migrations_keep_runtime_upgrade_scripts_in_order`**

- [x] **Step 4: Update `versioned_migrations_include_simplified_search_redesign`**

- [x] **Step 5: Run `cargo check`**

- [x] **Step 6: Run full test suite**

- [x] **Step 7: Commit**

```bash
git add src/storage.rs
git commit -m "fix(storage): register migrations 011 and 012 and update test assertions"
```

---

## Wave 4 — GLiNER Integration Test (P0)

### Task 6: Add end-to-end integration test with real model weights

**Problem:** No test verifies GLiNER works with actual `urchade/gliner_multi-v2.1` weights. The existing unit tests (`gliner_projection_heads_load_actual_prefixes`, `infers_backbone_config_from_actual_gliner_weight_layout`, `parses_gliner_runtime_config_with_model_name_fallback`) only cover config parsing and synthetic weight loading — they don't run a forward pass or verify extraction quality. Without this test, NMS fix (Task 1) cannot be validated against real model behavior.

**Files:**
- Create: `tests/gliner_integration.rs`

- [x] **Step 1: Create integration test file**

- [x] **Step 2: Create model fixture directory**

- [x] **Step 3: Run ignored test (expect skip if no model)**

- [x] **Step 4: Commit**

```bash
git add tests/gliner_integration.rs
git commit -m "test(gliner): add end-to-end integration tests with real model weights"
```

---

## Wave 4b — Independent Review Follow-up (verified gaps only)

### Task 7: Make LocalCandle initialization self-validating

**Problem:** The 2026-04-02 independent review correctly identified two still-open LocalCandle correctness gaps:

1. `embed_sync()` still hardcodes `const E5_PREFIX_TOKENS: usize = 2`, so the chunking guard assumes the current E5 tokenizer layout instead of deriving it from the loaded tokenizer.
2. `LocalCandleEmbeddingProvider::new()` still trusts the configured dimension and never probes the loaded model output. If config and model drift apart, semantic search degrades later via silent dimension mismatch handling instead of failing fast during startup.

**Files:**
- Modify: `src/service/embedding.rs`
- Modify: `tests/eval_retrieval.rs`

- [ ] **Step 1: Replace hardcoded E5 prefix accounting with tokenizer-derived count**

- [ ] **Step 2: Add a startup probe in `LocalCandleEmbeddingProvider::new()` and fail fast on dimension mismatch**

- [ ] **Step 3: Add or extend a regression test that exercises the real local fixture and verifies the provider starts cleanly with the expected output dimension**

- [ ] **Step 4: Run targeted tests plus the full verification pipeline**

### Task 8: Make GLiNER chunking and runtime behavior production-safe

**Problem:** The same review also identified two verified GLiNER runtime gaps that are not covered elsewhere in this plan:

1. `extract_inner()` still uses 1-token overlap between windows, which is too small for entities split near the chunk boundary.
2. `extract_candidates()` still runs CPU-heavy local inference inline on the tokio worker thread instead of yielding via a blocking boundary.

**Files:**
- Modify: `src/service/gliner_entity_extractor.rs`
- Modify: `tests/gliner_integration.rs`

- [ ] **Step 1: Replace the current 1-token overlap with a deliberate overlap policy and keep it configurable in code**

- [ ] **Step 2: Offload local GLiNER inference from the async runtime using the same general pattern already used by LocalCandle**

- [ ] **Step 3: Add a long-text ignored integration regression where a known entity lands across a window boundary and must still be recovered**

- [ ] **Step 4: Run targeted GLiNER tests plus the full verification pipeline**

### Task 9: Tighten real-model regression coverage instead of duplicating smoke tests

**Problem:** The review's broad claim that real-model integration tests are missing is no longer accurate, but it still exposed a narrower planning gap: current real-model tests are mostly smoke tests. They do not explicitly guard the two failure modes above (provider self-validation and window-boundary recall).

**Files:**
- Modify: `tests/gliner_integration.rs`
- Modify: `tests/eval_retrieval.rs`
- Modify: `docs/GLINER_NER_IMPLEMENTATION_PLAN.md`

- [ ] **Step 1: Document that real-model smoke coverage already exists and narrow the remaining gap to boundary and probe regressions**

- [ ] **Step 2: Extend the ignored integration suite with one LocalCandle-oriented assertion and one GLiNER boundary-window assertion**

- [ ] **Step 3: Re-run the ignored integration tests when fixtures are present, then run the full verification pipeline**

## Wave 5 — Deferred Items (Not in This Sprint)

These are noted for future work but excluded from this plan:

| Item | Reason for Deferral |
|---|---|
| Embedding removal from `__Initial.surql` and migration 008 | Intentional two-pass design verified by 3 tests; removing requires coordinated test + schema refactor |
| `entity_links` → `array<record<entity>>` migration | Works today with string IDs; DB-side graph pushdown is a future optimization, not a bug |
| `Arc<Surreal<...>>` connection pool for remote mode | Only matters for multi-agent/shared-session scenarios |
| Community detection: node centrality + bridge edges | Current connected-component approach is functional; enhancement, not correctness fix |
| LRU embedding cache | Performance optimization, not correctness |
| `EmbeddingProvider::embed` taking `Arc<Self>` for `spawn_blocking` | Requires trait refactor; `block_in_place` (Task 4) is sufficient for single-binary stdio server |
| `open_app` per-app param tables / typed launchers | MCP surface excluded from scope |

---

## Verification Pipeline

After completing all waves, run the full repository pipeline:

```bash
cargo fmt --all
cargo check
cargo clippy --all-targets -- -D warnings
cargo test
cargo doc --no-deps
```

All five commands must pass before the work is considered complete.
