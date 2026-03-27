# Adaptive Memory Alignment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** add benchmark-driven adaptive memory improvements to `memory_mcp` while preserving the current lexical/BM25 + graph retrieval direction and the intentionally small MCP tool surface.

**Architecture:** Implement the work in five focused tasks. First add observability with a LongMemEval-style acceptance harness. Then enrich fact indexing at write time, make lifecycle policies heat-aware, and add a timeline retrieval mode under `assemble_context`. Delegate to SurrealDB wherever possible (atomic updates, FTS, temporal filtering); implement Rust-side logic only as a last resort.

**Tech Stack:** Rust 2024, rmcp, SurrealDB 3.x, chrono, serde/serde_json, existing integration + acceptance test harness, markdown documentation

---

## Review notes (2026-03-27)

This plan was revised after a double review of all specification documents against KISS, YAGNI, DRY, and DDD criteria. The following items from the original plan were **removed or deferred:**

| Removed item | Reason |
|---|---|
| `usage_event` table (old Task 5) | YAGNI — `access_count` + `last_accessed` on fact suffice. SurrealDB atomic `UPDATE fact SET access_count += 1` eliminates need for a separate table. |
| `related_fact_ids` field + metadata evolution (old Task 4) | YAGNI — related-ness already captured by shared `entity_links` and community membership. New queries, merge logic, and field add complexity without measurable benefit for a personal memory corpus. |
| `FactType` enum (old Task 6 Step 3) | YAGNI — current `fact_type: String` is already flexible. Summary facts can use `fact_type = "summary"` without an enum. |
| Session-summary fact generation (old Task 6 Step 3) | Requires LLM — current extraction is regex-based. Contradicts single-binary/no-external-dependency constraint. |
| `temporal_query_variants` read-time function (old Task 2 Step 4) | KISS — temporal markers belong in `index_keys` at write time. BM25 FTS matches naturally without Rust-side query expansion. |
| `assemble_context_with_view()` separate method (old Task 6 Step 1) | DRY — extend `AssembleContextRequest` with optional fields instead of a parallel method. |

---

## Scope guardrails

- Keep `docs/MEMORY_SYSTEM_SPEC.md` as the current-state runtime contract.
- Keep `docs/SIMPLIFIED_SEARCH_REDESIGN_SPEC.md` as the retrieval-specific target.
- Implement this plan against `docs/superpowers/specs/2026-03-27-sota-memory-alignment-design.md`.
- Do **not** reintroduce embedding fields, HNSW indexes, or runtime dependency on external embedding providers.
- Do **not** add new public MCP tools.
- Delegate to SurrealDB: atomic field updates, FTS indexing, temporal filtering — Rust-side logic only when SurrealDB cannot handle it.

## File map

### Likely modified application files

- `src/models.rs` — add `index_keys`, `access_count`, `last_accessed` fields to `Fact`; extend `AssembleContextRequest` with optional `view_mode`, `window_start`, `window_end`
- `src/storage.rs` — migration wiring, FTS query update to search `index_keys`, atomic access-heat update helper
- `src/service/context.rs` — access-heat recording after retrieval, timeline mode sorting, explain-driven heat boost
- `src/service/core.rs` — increment `access_count` by larger delta in `explain()`
- `src/service/episode.rs` — populate `index_keys` with entity names, aliases, and temporal markers at ingest time
- `src/service/lifecycle/archival.rs` — heat-aware archival skip logic
- `src/service/lifecycle/decay.rs` — heat-aware decay attenuation
- `src/mcp/params.rs` — optional `view_mode`, `window_start`, `window_end` in `AssembleContextParams`
- `src/mcp/handlers.rs` — pass new optional params through to service
- `src/migrations/009_adaptive_memory_alignment.surql` — schema additions for new fact fields + FTS index on `index_keys`

### Likely modified test files

- `tests/common/mod.rs` — `ingest_episode()`, `seed_fact_at()` helpers
- `tests/service_integration.rs` — index_keys persistence, FTS on index_keys, timeline mode
- `tests/service_acceptance.rs` — existing acceptance coverage
- `tests/tools_e2e.rs` — timeline mode through MCP surface
- `tests/lifecycle_archival.rs` — hot-fact skip
- `tests/lifecycle_decay.rs` — hot-fact decay attenuation
- `tests/explain_provenance.rs` — access_count boost on explain
- `tests/embedded_fts_search.rs` — BM25 matching via index_keys
- Create: `tests/longmem_acceptance.rs` — 5 benchmark categories

### Likely modified docs

- `docs/MEMORY_SYSTEM_SPEC.md`
- `README.md`
- `docs/LIFECYCLE_BACKGROUND_JOBS.md`

---

### Task 1: Add a LongMemEval-style acceptance harness

**Files:**
- Create: `tests/longmem_acceptance.rs`
- Modify: `tests/common/mod.rs`

- [ ] **Step 1: Write the failing acceptance tests for the five benchmark categories**

```rust
#[tokio::test]
async fn assemble_context_when_fact_is_needed_across_sessions_then_returns_evidence() {
    let service = make_service().await;

    ingest_episode(&service, "sess-1", "Alice promised to send the Atlas deck by Friday").await;
    ingest_episode(&service, "sess-2", "We discussed unrelated travel plans").await;
    ingest_episode(&service, "sess-3", "Reminder: Atlas launch is still on track").await;

    let items = service
        .assemble_context(AssembleContextRequest {
            query: "what did Alice promise about Atlas".into(),
            scope: "personal".into(),
            ..Default::default()
        })
        .await
        .expect("context should assemble");

    assert!(items.iter().any(|item| item.content.contains("send the Atlas deck")));
}

#[tokio::test]
async fn assemble_context_when_question_is_unanswerable_then_returns_empty() {
    let service = make_service().await;

    let items = service
        .assemble_context(AssembleContextRequest {
            query: "what is Bob's passport number".into(),
            scope: "personal".into(),
            ..Default::default()
        })
        .await
        .expect("context should assemble");

    assert!(items.is_empty());
}
```

- [ ] **Step 2: Add helper builders in `tests/common/mod.rs` for multi-session ingest fixtures**

Use the existing `make_service()` pattern. Add minimal helpers:

```rust
/// Ingest an episode and extract entities/facts in one step.
pub async fn ingest_episode(service: &MemoryService, source_id: &str, content: &str) {
    let request = IngestRequest {
        source_type: "chat".into(),
        source_id: source_id.into(),
        content: content.into(),
        t_ref: "2026-03-01T10:00:00Z".parse().unwrap(),
        scope: "personal".into(),
        ..Default::default()
    };
    let episode_id = service.ingest(request, None).await.expect("ingest should succeed");
    service.extract(ExtractRequest { episode_id: episode_id.0 }, None).await.expect("extract should succeed");
}
```

- [ ] **Step 3: Run the new acceptance test file and verify failures are meaningful**

Run: `cargo test --test longmem_acceptance -- --test-threads=1`

Expected: some tests may already pass (direct fact lookup); others fail (temporal, abstention).

- [ ] **Step 4: Add the remaining benchmark categories as explicit tests**

Include at minimum:
- temporal reasoning (`as_of` with facts before and after a cutoff)
- knowledge update (invalidated old fact should not appear)
- multi-session reasoning (facts from different episodes compose)
- abstention (no matching facts → empty result)
- evidence retrieval quality for direct fact lookup

- [ ] **Step 5: Commit**

```bash
git add tests/common/mod.rs tests/longmem_acceptance.rs
git commit -m "test: add longmem-style acceptance harness"
```

### Task 2: Add fact-augmented index keys with temporal markers

**Files:**
- Modify: `src/models.rs`
- Modify: `src/service/episode.rs`
- Modify: `src/storage.rs`
- Create: `src/migrations/009_adaptive_memory_alignment.surql`
- Test: `tests/service_integration.rs`
- Test: `tests/embedded_fts_search.rs`

- [ ] **Step 1: Write the failing tests for `index_keys` persistence and FTS matching**

```rust
#[tokio::test]
async fn add_fact_when_entities_are_present_then_index_keys_include_canonical_names() {
    let (service, db) = make_service_with_client().await;
    ingest_episode(&service, "ep-1", "Alice from Atlas promised the deck in March 2026").await;

    // Query fact directly from DB
    let facts = db.select_table("fact", "personal").await.unwrap();
    let fact = &facts[0];
    let index_keys = fact.get("index_keys").and_then(|v| v.as_array()).unwrap();
    assert!(index_keys.iter().any(|k| k.as_str() == Some("alice")));
    assert!(index_keys.iter().any(|k| k.as_str() == Some("atlas")));
}

#[tokio::test]
async fn assemble_context_when_query_matches_index_key_then_fact_is_returned() {
    let service = make_service().await;
    ingest_episode(&service, "ep-1", "Alice from Atlas promised the deck in March 2026").await;

    // Search by entity name that appears in index_keys but may not be in content
    let items = service
        .assemble_context(AssembleContextRequest {
            query: "alice atlas".into(),
            scope: "personal".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    assert!(!items.is_empty());
}
```

- [ ] **Step 2: Add schema fields to `src/models.rs` and the migration**

Add to `Fact` struct:

```rust
#[serde(default)]
pub index_keys: Vec<String>,
#[serde(default)]
pub access_count: i64,
pub last_accessed: Option<DateTime<Utc>>,
```

Migration `009_adaptive_memory_alignment.surql`:

```sql
-- Fact-augmented index keys for enriched FTS retrieval
DEFINE FIELD index_keys ON fact TYPE array<string> VALUE $value OR [];
DEFINE FIELD access_count ON fact TYPE int VALUE $value OR 0;
DEFINE FIELD last_accessed ON fact TYPE option<datetime>;

-- FTS index on index_keys for BM25 matching alongside fact.content
DEFINE INDEX fact_index_keys_fts ON fact FIELDS index_keys SEARCH ANALYZER memory_fts BM25;
```

Update `fact_from_record()` in `src/service/episode.rs` to parse the new fields with `#[serde(default)]`.

- [ ] **Step 3: Populate `index_keys` during extraction**

In the extraction path (after entity resolution), build index keys from:
- canonical entity names (lowercased),
- entity aliases (lowercased),
- extracted temporal markers from fact content (regex: month names, `YYYY-MM`, explicit date phrases).

```rust
fn build_index_keys(entities: &[ExtractedEntity], t_valid: DateTime<Utc>) -> Vec<String> {
    let mut keys: Vec<String> = entities
        .iter()
        .map(|e| e.canonical_name.to_lowercase())
        .collect();

    // Add temporal markers from the fact's valid time
    keys.push(t_valid.format("%B %Y").to_string().to_lowercase()); // "march 2026"
    keys.push(t_valid.format("%Y-%m").to_string());                 // "2026-03"

    keys.sort();
    keys.dedup();
    keys
}
```

Temporal markers go into `index_keys` at write time — no read-time query expansion needed.

- [ ] **Step 4: Update FTS query to search both `content` and `index_keys`**

Modify `build_select_facts_filtered_query` in `src/storage.rs`:

```sql
SELECT *, search::score(1) AS ft_score
FROM fact
WHERE scope = $scope
  AND t_valid <= type::datetime($cutoff)
  AND (t_ingested IS NONE OR t_ingested <= type::datetime($cutoff))
  AND (t_invalid IS NONE OR t_invalid > type::datetime($cutoff)
       OR t_invalid_ingested > type::datetime($cutoff))
  AND (content @1@ $query OR index_keys @1@ $query)
ORDER BY ft_score DESC, t_valid DESC, fact_id ASC
LIMIT $limit
```

> Note: SurrealDB FTS `@N@` operator on array fields searches all elements. Both `content` and `index_keys` share the same score slot `@1@` — SurrealDB merges scores from both fields. Verify in integration test.

- [ ] **Step 5: Run focused tests**

Run: `cargo test --test service_integration --test embedded_fts_search -- --test-threads=1`

Expected: PASS with new coverage for `index_keys` and temporal marker retrieval.

- [ ] **Step 6: Commit**

```bash
git add src/models.rs src/service/episode.rs src/storage.rs \
  src/migrations/009_adaptive_memory_alignment.surql \
  tests/service_integration.rs tests/embedded_fts_search.rs
git commit -m "feat: add fact-augmented index keys with temporal markers"
```

### Task 3: Make lifecycle policies heat-aware

**Files:**
- Modify: `src/service/context.rs`
- Modify: `src/service/core.rs`
- Modify: `src/service/lifecycle/archival.rs`
- Modify: `src/service/lifecycle/decay.rs`
- Modify: `src/storage.rs`
- Test: `tests/lifecycle_archival.rs`
- Test: `tests/lifecycle_decay.rs`
- Test: `tests/explain_provenance.rs`

- [ ] **Step 1: Write failing tests showing recently accessed facts decay/archive differently**

```rust
#[tokio::test]
async fn decay_worker_when_fact_was_recently_accessed_then_decay_is_attenuated() {
    let (service, db) = make_service_with_client().await;
    let fact_id = seed_old_fact(&service, "Atlas deck draft", "2025-01-01T00:00:00Z").await;

    // Simulate recent access via DB update
    db.update(&fact_id, json!({"access_count": 5, "last_accessed": "2026-03-26T00:00:00Z"}), "personal")
        .await.unwrap();

    run_decay_pass(&service, 0.1, 90.0).await.unwrap();

    // Fact should still be active because it was recently accessed
    let record = db.select_one(&fact_id, "personal").await.unwrap().unwrap();
    assert!(record.get("t_invalid").unwrap().is_null());
}

#[tokio::test]
async fn explain_when_fact_is_cited_then_access_count_increases() {
    let (service, db) = make_service_with_client().await;
    // ... set up fact, call explain, assert access_count > 0
}
```

- [ ] **Step 2: Add atomic access-heat recording in storage layer**

SurrealDB can do this in one atomic update:

```rust
/// Record a fact access by incrementing access_count and updating last_accessed.
/// `boost` controls the increment: 1 for retrieval, 3 for explain.
pub async fn record_fact_access(
    &self,
    fact_id: &str,
    namespace: &str,
    boost: i64,
) -> Result<(), MemoryError> {
    let sql = "UPDATE type::thing('fact', $id) SET access_count += $boost, last_accessed = time::now()";
    // ...
}
```

- [ ] **Step 3: Call `record_fact_access` from `assemble_context` and `explain`**

In `assemble_context`, after building the final result set:

```rust
// Record access heat for returned facts (fire-and-forget, don't fail the response)
for item in &results {
    let _ = service.db_client.record_fact_access(&item.fact_id, &namespace, 1).await;
}
```

In `explain`, for each successfully explained item:

```rust
// Stronger signal: fact was useful enough to be cited
let _ = self.db_client.record_fact_access(&item.fact_id, &namespace, 3).await;
```

- [ ] **Step 4: Update decay to consider access heat**

In `run_decay_pass`, after computing `decayed < threshold`, add a heat check:

```rust
let access_count = record.get("access_count").and_then(json_i64).unwrap_or(0);
let last_accessed = record.get("last_accessed")
    .and_then(|v| v.as_str())
    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
    .map(|dt| dt.with_timezone(&Utc));

// Skip invalidation if fact was accessed recently (within half_life_days)
let is_hot = last_accessed
    .is_some_and(|la| (now - la).num_days() as f64 <= half_life_days);

if decayed < threshold && !is_hot {
    // invalidate fact
}
```

Archival similarly checks `last_accessed` on facts linked to the episode.

- [ ] **Step 5: Run lifecycle tests**

Run: `cargo test --test lifecycle_archival --test lifecycle_decay --test explain_provenance -- --test-threads=1`

Expected: PASS with hot-fact protection and explain-boosted access counts.

- [ ] **Step 6: Commit**

```bash
git add src/service/context.rs src/service/core.rs src/storage.rs \
  src/service/lifecycle/archival.rs src/service/lifecycle/decay.rs \
  tests/lifecycle_archival.rs tests/lifecycle_decay.rs tests/explain_provenance.rs
git commit -m "feat: make lifecycle policies heat-aware"
```

### Task 4: Add timeline-oriented retrieval mode under `assemble_context`

**Files:**
- Modify: `src/models.rs`
- Modify: `src/mcp/params.rs`
- Modify: `src/mcp/handlers.rs`
- Modify: `src/service/context.rs`
- Test: `tests/tools_e2e.rs`
- Test: `tests/service_integration.rs`

- [ ] **Step 1: Write failing tests for backwards-compatible timeline mode**

```rust
#[tokio::test]
async fn assemble_context_when_view_mode_is_timeline_then_results_sorted_by_t_valid() {
    let service = make_service().await;
    seed_fact_at(&service, "Atlas planning started", "2026-01-01T00:00:00Z").await;
    seed_fact_at(&service, "Atlas budget increased", "2026-02-01T00:00:00Z").await;
    seed_fact_at(&service, "Atlas launch confirmed", "2026-03-01T00:00:00Z").await;

    let items = service
        .assemble_context(AssembleContextRequest {
            query: "atlas".into(),
            scope: "personal".into(),
            view_mode: Some("timeline".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    // Timeline mode: oldest first
    assert!(items[0].content.contains("planning started"));
    assert!(items[1].content.contains("budget increased"));
    assert!(items[2].content.contains("launch confirmed"));
}

#[tokio::test]
async fn assemble_context_when_window_is_set_then_only_facts_within_window_returned() {
    let service = make_service().await;
    seed_fact_at(&service, "January event", "2026-01-15T00:00:00Z").await;
    seed_fact_at(&service, "February event", "2026-02-15T00:00:00Z").await;
    seed_fact_at(&service, "March event", "2026-03-15T00:00:00Z").await;

    let items = service
        .assemble_context(AssembleContextRequest {
            query: "event".into(),
            scope: "personal".into(),
            view_mode: Some("timeline".into()),
            window_start: Some("2026-02-01T00:00:00Z".parse().unwrap()),
            window_end: Some("2026-02-28T23:59:59Z".parse().unwrap()),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(items.len(), 1);
    assert!(items[0].content.contains("February"));
}

// Verify old callers are unaffected (default view_mode = None → standard behavior)
#[tokio::test]
async fn assemble_context_when_no_view_mode_then_standard_relevance_order() {
    // ... existing behavior unchanged
}
```

- [ ] **Step 2: Extend `AssembleContextRequest` with optional fields**

```rust
pub struct AssembleContextRequest {
    pub query: String,
    pub scope: String,
    pub as_of: Option<DateTime<Utc>>,
    #[serde(default = "default_budget")]
    pub budget: i32,
    /// Optional view mode: "timeline" sorts by t_valid ASC; default is relevance ranking.
    pub view_mode: Option<String>,
    /// Optional time window start (inclusive). Only used when view_mode = "timeline".
    pub window_start: Option<DateTime<Utc>>,
    /// Optional time window end (inclusive). Only used when view_mode = "timeline".
    pub window_end: Option<DateTime<Utc>>,
    #[serde(skip_serializing, default)]
    #[schemars(skip)]
    pub access: Option<AccessPayload>,
}
```

Extend `AssembleContextParams` in `src/mcp/params.rs` similarly with optional `view_mode`, `window_start`, `window_end` string fields.

- [ ] **Step 3: Implement timeline sorting and window filtering in `src/service/context.rs`**

After fusion ranking, before budget truncation:

```rust
let is_timeline = request.view_mode.as_deref() == Some("timeline");

if is_timeline {
    // Apply optional time window filter
    if let (Some(start), Some(end)) = (request.window_start, request.window_end) {
        ranked_facts.retain(|rf| rf.fact.t_valid >= start && rf.fact.t_valid <= end);
    }
    // Sort chronologically (oldest first) with stable tie-break
    ranked_facts.sort_by(|a, b| {
        a.fact.t_valid.cmp(&b.fact.t_valid)
            .then_with(|| a.fact.fact_id.cmp(&b.fact.fact_id))
    });
}
```

- [ ] **Step 4: Run service + MCP-level tests**

Run: `cargo test --test service_integration --test tools_e2e -- --test-threads=1`

Expected: PASS with old callers unchanged and new timeline mode covered.

- [ ] **Step 5: Commit**

```bash
git add src/models.rs src/mcp/params.rs src/mcp/handlers.rs \
  src/service/context.rs \
  tests/tools_e2e.rs tests/service_integration.rs
git commit -m "feat: add timeline retrieval mode in assemble_context"
```

### Task 5: Documentation and full verification

**Files:**
- Modify: `README.md`
- Modify: `docs/MEMORY_SYSTEM_SPEC.md`
- Modify: `docs/LIFECYCLE_BACKGROUND_JOBS.md`
- Verify only: workspace-wide

- [ ] **Step 1: Update MEMORY_SYSTEM_SPEC.md to describe adaptive-memory fields**

Document:
- `fact.index_keys` — populated at ingest with entity names, aliases, temporal markers
- `fact.access_count` / `fact.last_accessed` — updated on retrieval and explain
- heat-aware lifecycle skip for recently-accessed facts
- `assemble_context` optional `view_mode=timeline`, `window_start`, `window_end`
- LongMemEval-style acceptance harness coverage

- [ ] **Step 2: Update LIFECYCLE_BACKGROUND_JOBS.md**

Document heat-aware behavior: decay and archival workers now skip facts with recent `last_accessed`.

- [ ] **Step 3: Add retrieval notes to README**

```md
### Adaptive Memory Features

- Fact-augmented index keys: entity names, aliases, and temporal markers indexed at ingest for enriched BM25 retrieval.
- Heat-aware lifecycle: recently-accessed facts are protected from decay/archival.
- Timeline retrieval: `assemble_context` supports `view_mode=timeline` with optional `window_start`/`window_end`.
- LongMem-style acceptance tests cover information extraction, multi-session reasoning, temporal reasoning, knowledge updates, and abstention.
```

- [ ] **Step 4: Run formatting and repository verification**

```bash
cargo fmt --all
cargo check
cargo clippy --all-targets -- -D warnings
cargo test -- --test-threads=1
```

All must pass.

- [ ] **Step 5: Inspect diff shape before merging**

Run: `git diff --stat`
Expected: only the planned Rust, migration, test, and documentation files are changed.

- [ ] **Step 6: Commit**

```bash
git add README.md docs/MEMORY_SYSTEM_SPEC.md docs/LIFECYCLE_BACKGROUND_JOBS.md src tests
git commit -m "docs: align runtime docs with adaptive memory improvements"
```

---

## Self-review checklist

- [ ] Every requirement from `docs/superpowers/specs/2026-03-27-sota-memory-alignment-design.md` §8 maps to at least one task.
- [ ] No task reintroduces embeddings or HNSW runtime dependencies.
- [ ] No task adds a new public MCP tool.
- [ ] No task adds a separate `usage_event` table — access heat uses fields on `fact`.
- [ ] No task adds a `FactType` enum — `fact_type: String` remains.
- [ ] No task requires an LLM at runtime.
- [ ] Tests exist for all five benchmark categories.
- [ ] Timeline behavior is backwards-compatible and deterministic.
- [ ] Lifecycle behavior is driven by measured `access_count` / `last_accessed`, not only TTL.
- [ ] Temporal markers are indexed at write time, not expanded at read time.

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-03-27-adaptive-memory-alignment-implementation-plan.md`. Two execution options:

**1. Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration

**2. Inline Execution** — execute tasks in this session using executing-plans, batch execution with checkpoints

**Which approach?**
