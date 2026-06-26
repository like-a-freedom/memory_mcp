# Codebase Refactoring: KISS, DRY, YAGNI, DDD Compliance

> **For agentic workers:** Use `superpowers:subagent-driven-development` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the memory_mcp codebase into compliance with KISS, DRY, YAGNI, and DDD principles by decomposing God Objects, eliminating duplication, removing dead code, enriching the domain model, and establishing a MockDbClient for ergonomic testing.

**Architecture:** Incremental, non-breaking refactoring in 11 sequential phases. `MemoryService` becomes a thin facade over focused domain services (`IngestionService`, `EntityService`, `FactService`, `EmbeddingService`, `ExplanationService`). `MemoryMcp` delegates app logic to per-app modules. `DbClient` trait is preserved but augmented with a `MockDbClient` builder to eliminate test boilerplate. Each phase leaves the codebase compiling and all existing tests passing.

**Tech Stack:** Rust edition 2024, tokio, surrealdb 3.1, rmcp 1.7, thiserror 2.0, serde/serde_json, chrono, async-trait, lru

## Global Constraints

- Edition 2024 — use `let_chains`, `if_let_guard`, other 2024 features consistently
- All existing tests must pass after each commit
- No breaking API changes to `MemoryService` public methods or MCP tool schemas
- `#[must_use]` on all builder/pure-function return types
- `#[allow(clippy::too_many_arguments)]` only where unavoidable during transition; remove in final phases
- Error handling through `MemoryError` enum (thiserror) — no `unwrap()` or `expect()` in production code
- Logging via `StdoutLogger` and `log_event()` helper — never `println!` or `eprintln!`

---

## File Structure

### Files created by this plan

```
src/service/ingestion.rs          — IngestionService (moved from core.rs ingest parts)
src/service/entity.rs             — EntityService (moved from core.rs resolve + lookup)
src/service/fact.rs               — FactService (moved from core.rs add_fact + invalidate)
src/service/explanation.rs        — ExplanationService (moved from core.rs explain + provenance)
src/service/embedding/service.rs  — EmbeddingService (moved from core.rs embedding methods)
src/service/embedding/task_runner.rs — Generic background embedding task runner
src/service/mock_db.rs            — MockDbClient builder for tests
src/mcp/apps/inspector.rs         — Inspector app logic extracted from handlers.rs
src/mcp/apps/diff.rs              — Diff app logic
src/mcp/apps/ingestion_review.rs  — Ingestion review app logic
src/mcp/apps/lifecycle.rs         — Lifecycle dashboard logic
src/mcp/apps/graph.rs             — Graph traversal logic
src/mcp/session.rs                — App session management extracted from handlers.rs
src/mcp/response.rs               — ToolResponse<T> and response constructors
```

### Files heavily modified

```
src/service/core.rs               — Shrinks from 4130 → ~400 lines (facade + helpers)
src/service/core/builder.rs       — Shrinks from 579 → ~200 lines
src/service/core/helpers.rs       — Gains some helper functions from core.rs
src/mcp/handlers.rs               — Shrinks from 3156 → ~400 lines (routing only)
src/service/context.rs            — Shrinks from 5936 → ~200 lines (entry point only)
src/storage/client.rs             — Mostly unchanged; gets MockDbClient in tests
src/models.rs                     — Gains domain methods, loses EpisodeInput
src/service.rs                    — Updated re-exports
src/service/util.rs               — Shrinks; util submodules rehomed
src/service/ingest.rs             — Gains statement_detection from util/
```

---

### Task 1: YAGNI Cleanup — Remove Dead Code and Redundant Abstractions

**Files:**
- Modify: `src/models.rs:1-671`
- Modify: `src/service/core.rs` (resolve_* methods)
- Modify: `src/mcp/handlers.rs` (resolve_* callers)
- Modify: `src/storage/client.rs:937-949`
- Modify: `src/lib.rs:41`

**Interfaces:**
- Consumes: Current `AccessPayload`, `AccessContext`, `IngestRequest`, `EpisodeInput`, resolve_* methods, `render_initial_schema_sql`, `render_migration_sql`
- Produces: Merged `AccessPayload` with `is_scope_allowed()` and `Default`; removed `EpisodeInput`; single `resolve_entity(entity_type: &str, name: &str) -> Result<String, MemoryError>`; merged `render_schema_sql` function

- [ ] **Step 1: Merge AccessPayload and AccessContext**

In `src/models.rs`, add `Default` derive to `AccessPayload`, add `is_scope_allowed`, remove `AccessContext` and its `From` impl. Replace all `AccessContext` references with `AccessPayload`:

```rust
// In src/models.rs, replace AccessPayload + AccessContext with:

/// Access control payload for requests.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct AccessPayload {
    pub allowed_scopes: Option<Vec<String>>,
    pub allowed_tags: Option<Vec<String>>,
    pub caller_id: Option<String>,
    pub session_vars: Option<serde_json::Value>,
    pub transport: Option<String>,
    pub content_type: Option<String>,
    pub cross_scope_allow: Option<Vec<AccessScopeAllow>>,
}

impl AccessPayload {
    /// Creates an access context from an optional payload.
    #[must_use]
    pub fn from_payload(payload: Option<Self>) -> Option<Self> {
        payload
    }

    /// Checks if a scope is allowed.
    #[must_use]
    pub fn is_scope_allowed(&self, scope: &str) -> bool {
        if let Some(scopes) = &self.allowed_scopes
            && !scopes.contains(&scope.to_string())
        {
            return self.cross_scope_allow.as_ref().is_some_and(|cross| {
                cross
                    .iter()
                    .any(|pair| pair.from == "*" && pair.to == scope)
            });
        }
        true
    }
}
```

- [ ] **Step 2: Replace all AccessContext → AccessPayload throughout codebase**

This rename ripples wider than expected. Run `rg "AccessContext" src/ tests/ --files-with-matches` to find ALL files. Critical spots:
- `src/service/core/helpers.rs:11` — `use crate::models::AccessContext;` → `use crate::models::AccessPayload;`
- `src/service/core/helpers.rs:46` — `log_event(..., access: Option<&AccessContext>, ...)` → `Option<&AccessPayload>`
- `src/service/core/helpers.rs:66` — `fn serialize_access(access: &AccessContext)` → `&AccessPayload`
- `src/service/core.rs:1518` — `fn is_scope_allowed(&self, scope: &str, access: &AccessContext)` → DELETE this method; callers should use `access.is_scope_allowed(scope)` directly (the method already exists on AccessPayload from Step 1). Replace all `self.is_scope_allowed(scope, &access)` calls with `access.is_scope_allowed(scope)`.
- `src/service/core.rs:1531` — `fn enforce_rate_limit(&self, access: Option<&AccessContext>)` → `Option<&AccessPayload>`
- `src/mcp/handlers.rs` — every `AccessContext::default()` → `AccessPayload::default()`
- `src/mcp/error.rs` — check for AccessContext references
- All `log_event(...)` callsites: the `access.as_ref()` parameter type changes

Use global find-replace: `AccessContext` → `AccessPayload` across all `.rs` files, then fix any compilation errors with `cargo check`.

- [ ] **Step 3: Remove EpisodeInput struct**

In `src/models.rs`, delete the `EpisodeInput` struct (lines 72-82). It's unused in production. Run `rg "EpisodeInput" src/ tests/` to verify no references exist (should find 0).

- [ ] **Step 4: Collapse 6 resolve_* methods into 1**

In `src/service/core.rs:1231-1258`, replace lines 1231-1258 with:

```rust
    /// Resolves an entity by its type and canonical name.
    pub async fn resolve_entity(
        &self,
        entity_type: &str,
        name: &str,
    ) -> Result<String, MemoryError> {
        self.resolve(
            EntityCandidate {
                entity_type: entity_type.to_string(),
                canonical_name: name.to_string(),
                aliases: Vec::new(),
            },
            None,
        )
        .await
    }
```

Search for callers of `resolve_person`, `resolve_company`, `resolve_location`, `resolve_product`, `resolve_event`, `resolve_concept` in `src/` and `tests/`:

```
rg "resolve_(person|company|location|product|event|concept)" src/ tests/
```

Replace each call. Example: `self.service.resolve_person("Alice")` → `self.service.resolve_entity("person", "Alice")`.

- [ ] **Step 5: Merge duplicate SQL rendering functions**

In `src/storage/client.rs:937-949`, replace both functions with:

```rust
fn render_schema_sql(template: &str, embedding_dimension: usize) -> String {
    template.replace(
        crate::storage::fact_embedding_dimension_placeholder(),
        &embedding_dimension.to_string(),
    )
}
```

Update calls at lines 523 and 573 from `render_initial_schema_sql` and `render_migration_sql` to `render_schema_sql`.

- [ ] **Step 6: Verify all tests pass and commit**

```bash
cargo test --no-fail-fast 2>&1 | tail -20
```

Expected: all tests pass, no warnings about unused imports.

```bash
git add -A && git commit -m "refactor: YAGNI cleanup — merge AccessPayload/AccessContext, remove EpisodeInput, collapse resolve_* methods, deduplicate SQL rendering"
```

---

### Task 2: MockDbClient — Eliminate Test Boilerplate

**Files:**
- Create: `src/service/mock_db.rs`
- Modify: All test files with hand-written `impl DbClient` blocks
- Modify: `src/service.rs` (add `mod mock_db` or `#[cfg(test)] mod`)

**Interfaces:**
- Consumes: `crate::storage::DbClient` trait, `crate::service::MemoryError`, `serde_json::Value`
- Produces: `MockDbClient` struct with `expect_*` methods that replace hand-written mocks

- [ ] **Step 1: Create MockDbClient**

Create `src/service/mock_db.rs`:

```rust
//! Mock database client for tests, eliminating boilerplate from hand-written mocks.
//!
//! Usage in tests:
//! ```rust,no_run
//! let db = MockDbClient::new()
//!     .expect_select_one("episode:test", Some(json!({"episode_id": "episode:test", "content": "hello"})))
//!     .expect_create("fact:1", json!({"status": "ok"}));
//! let service = MemoryService::new(Arc::new(db), vec!["org".into()], "warn".into(), 50, 100).unwrap();
//! ```

use std::collections::HashMap;
use std::sync::Mutex;
use async_trait::async_trait;
use serde_json::Value;

use crate::service::MemoryError;
use crate::storage::{DbClient, GraphDirection};

type SelectOneFn = dyn Fn(&str) -> Result<Option<Value>, MemoryError> + Send + Sync;
type SelectTableFn = dyn Fn() -> Result<Vec<Value>, MemoryError> + Send + Sync;
type QueryFn = dyn Fn() -> Result<Value, MemoryError> + Send + Sync;
type CreateFn = dyn Fn() -> Result<Value, MemoryError> + Send + Sync;
type UpdateFn = dyn Fn() -> Result<Value, MemoryError> + Send + Sync;
type EdgeNeighborsFn = dyn Fn() -> Result<Vec<Value>, MemoryError> + Send + Sync;

/// Configurable mock database client for tests.
///
/// By default, every method returns `Ok(vec![])` or `Ok(None)`.
/// Use the `expect_*` builder methods to override specific calls.
pub struct MockDbClient {
    select_one_responses: Mutex<HashMap<String, Result<Option<Value>, MemoryError>>>,
    select_table_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    facts_filtered_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    facts_entity_links_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    facts_ann_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    edges_filtered_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    edge_neighbors_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    entity_lookup_responses: Mutex<HashMap<String, Result<Option<Value>, MemoryError>>>,
    entities_batch_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    entities_by_ids_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    edges_for_triple_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    active_facts_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    episodes_for_archival_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    active_facts_by_episode_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    episodes_by_content_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    communities_by_members_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    communities_matching_summary_responses: Mutex<HashMap<String, Result<Vec<Value>, MemoryError>>>,
    relate_edge_responses: Mutex<HashMap<String, Result<Value, MemoryError>>>,
    create_responses: Mutex<HashMap<String, Result<Value, MemoryError>>>,
    update_responses: Mutex<HashMap<String, Result<Value, MemoryError>>>,
    query_responses: Mutex<HashMap<String, Result<Value, MemoryError>>>,
    migration_result: Mutex<Result<(), MemoryError>>,
    fallback_select_one: Mutex<Box<Option<SelectOneFn>>>,
    fallback_select_table: Mutex<Box<Option<SelectTableFn>>>,
    fallback_query: Mutex<Box<Option<QueryFn>>>,
    fallback_create: Mutex<Box<Option<CreateFn>>>,
    fallback_update: Mutex<Box<Option<UpdateFn>>>,
    fallback_edges_filtered: Mutex<Box<Option<SelectTableFn>>>,
    fallback_edge_neighbors: Mutex<Box<Option<EdgeNeighborsFn>>>,
    fallback_facts_filtered: Mutex<Box<Option<SelectTableFn>>>,
    fallback_facts_by_entity_links: Mutex<Box<Option<SelectTableFn>>>,
    fallback_facts_ann: Mutex<Box<Option<SelectTableFn>>>,
    fallback_entity_lookup: Mutex<Box<Option<SelectOneFn>>>,
    fallback_entities_batch: Mutex<Box<Option<SelectTableFn>>>,
    fallback_active_facts: Mutex<Box<Option<SelectTableFn>>>,
    fallback_episodes_for_archival: Mutex<Box<Option<SelectTableFn>>>,
    fallback_active_facts_by_episode: Mutex<Box<Option<SelectTableFn>>>,
    fallback_episodes_by_content: Mutex<Box<Option<SelectTableFn>>>,
}

impl MockDbClient {
    pub fn new() -> Self {
        Self {
            select_one_responses: Mutex::new(HashMap::new()),
            select_table_responses: Mutex::new(HashMap::new()),
            facts_filtered_responses: Mutex::new(HashMap::new()),
            facts_entity_links_responses: Mutex::new(HashMap::new()),
            facts_ann_responses: Mutex::new(HashMap::new()),
            edges_filtered_responses: Mutex::new(HashMap::new()),
            edge_neighbors_responses: Mutex::new(HashMap::new()),
            entity_lookup_responses: Mutex::new(HashMap::new()),
            entities_batch_responses: Mutex::new(HashMap::new()),
            entities_by_ids_responses: Mutex::new(HashMap::new()),
            edges_for_triple_responses: Mutex::new(HashMap::new()),
            active_facts_responses: Mutex::new(HashMap::new()),
            episodes_for_archival_responses: Mutex::new(HashMap::new()),
            active_facts_by_episode_responses: Mutex::new(HashMap::new()),
            episodes_by_content_responses: Mutex::new(HashMap::new()),
            communities_by_members_responses: Mutex::new(HashMap::new()),
            communities_matching_summary_responses: Mutex::new(HashMap::new()),
            relate_edge_responses: Mutex::new(HashMap::new()),
            create_responses: Mutex::new(HashMap::new()),
            update_responses: Mutex::new(HashMap::new()),
            query_responses: Mutex::new(HashMap::new()),
            migration_result: Mutex::new(Ok(())),
            fallback_select_one: Mutex::new(Box::new(None)),
            fallback_select_table: Mutex::new(Box::new(None)),
            fallback_query: Mutex::new(Box::new(None)),
            fallback_create: Mutex::new(Box::new(None)),
            fallback_update: Mutex::new(Box::new(None)),
            fallback_edges_filtered: Mutex::new(Box::new(None)),
            fallback_edge_neighbors: Mutex::new(Box::new(None)),
            fallback_facts_filtered: Mutex::new(Box::new(None)),
            fallback_facts_by_entity_links: Mutex::new(Box::new(None)),
            fallback_facts_ann: Mutex::new(Box::new(None)),
            fallback_entity_lookup: Mutex::new(Box::new(None)),
            fallback_entities_batch: Mutex::new(Box::new(None)),
            fallback_active_facts: Mutex::new(Box::new(None)),
            fallback_episodes_for_archival: Mutex::new(Box::new(None)),
            fallback_active_facts_by_episode: Mutex::new(Box::new(None)),
            fallback_episodes_by_content: Mutex::new(Box::new(None)),
        }
    }

    pub fn expect_select_one(mut self, record_id: &str, result: Option<Value>) -> Self {
        self.select_one_responses.lock().unwrap().insert(
            record_id.to_string(),
            Ok(result),
        );
        self
    }

    pub fn expect_select_one_with(mut self, f: impl Fn(&str) -> Result<Option<Value>, MemoryError> + Send + Sync + 'static) -> Self {
        self.fallback_select_one = Mutex::new(Box::new(Some(Box::new(f))));
        self
    }

    pub fn expect_create(mut self, record_id: &str, result: Value) -> Self {
        self.create_responses.lock().unwrap().insert(
            record_id.to_string(),
            Ok(result),
        );
        self
    }

    pub fn expect_update(mut self, record_id: &str, result: Value) -> Self {
        self.update_responses.lock().unwrap().insert(
            record_id.to_string(),
            Ok(result),
        );
        self
    }

    pub fn expect_select_table(mut self, table: &str, rows: Vec<Value>) -> Self {
        self.select_table_responses.lock().unwrap().insert(
            table.to_string(),
            Ok(rows),
        );
        self
    }

    pub fn expect_entity_lookup(mut self, normalized_name: &str, result: Option<Value>) -> Self {
        self.entity_lookup_responses.lock().unwrap().insert(
            normalized_name.to_string(),
            Ok(result),
        );
        self
    }

    pub fn expect_edge_neighbors(mut self, node_id: &str, neighbors: Vec<Value>) -> Self {
        self.edge_neighbors_responses.lock().unwrap().insert(
            node_id.to_string(),
            Ok(neighbors),
        );
        self
    }

    pub fn expect_edge_neighbors_with(
        mut self,
        f: impl Fn() -> Result<Vec<Value>, MemoryError> + Send + Sync + 'static,
    ) -> Self {
        self.fallback_edge_neighbors = Mutex::new(Box::new(Some(Box::new(f))));
        self
    }

    pub fn expect_query(mut self, sql_prefix: &str, result: Value) -> Self {
        self.query_responses.lock().unwrap().insert(
            sql_prefix.to_string(),
            Ok(result),
        );
        self
    }
}

impl Default for MockDbClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DbClient for MockDbClient {
    async fn select_one(&self, record_id: &str, _namespace: &str) -> Result<Option<Value>, MemoryError> {
        if let Some(resp) = self.select_one_responses.lock().unwrap().get(record_id) {
            return resp.clone();
        }
        if let Some(ref f) = *self.fallback_select_one.lock().unwrap() {
            return f(record_id);
        }
        Ok(None)
    }

    async fn select_table(&self, table: &str, _namespace: &str) -> Result<Vec<Value>, MemoryError> {
        if let Some(resp) = self.select_table_responses.lock().unwrap().get(table) {
            return resp.clone();
        }
        if let Some(ref f) = *self.fallback_select_table.lock().unwrap() {
            return f();
        }
        Ok(vec![])
    }

    async fn select_facts_filtered(&self, _namespace: &str, scope: &str, _cutoff: &str, query_contains: Option<&str>, _limit: i32) -> Result<Vec<Value>, MemoryError> {
        let key = format!("{}/{}", scope, query_contains.unwrap_or(""));
        if let Some(resp) = self.facts_filtered_responses.lock().unwrap().get(&key) {
            return resp.clone();
        }
        if let Some(ref f) = *self.fallback_facts_filtered.lock().unwrap() {
            return f();
        }
        Ok(vec![])
    }

    async fn select_facts_by_entity_links(&self, _namespace: &str, _scope: &str, _cutoff: &str, entity_links: &[String], _limit: i32) -> Result<Vec<Value>, MemoryError> {
        let key = entity_links.join(",");
        if let Some(resp) = self.facts_entity_links_responses.lock().unwrap().get(&key) {
            return resp.clone();
        }
        if let Some(ref f) = *self.fallback_facts_by_entity_links.lock().unwrap() {
            return f();
        }
        Ok(vec![])
    }

    async fn select_facts_ann(&self, _namespace: &str, _scope: &str, _cutoff: &str, _query_vec: &[f64], _limit: i32) -> Result<Vec<Value>, MemoryError> {
        if let Some(ref f) = *self.fallback_facts_ann.lock().unwrap() {
            return f();
        }
        Ok(vec![])
    }

    async fn select_edges_filtered(&self, _namespace: &str, _cutoff: &str) -> Result<Vec<Value>, MemoryError> {
        if let Some(ref f) = *self.fallback_edges_filtered.lock().unwrap() {
            return f();
        }
        Ok(vec![])
    }

    async fn select_edge_neighbors(&self, _namespace: &str, node_id: &str, _cutoff: &str, _direction: GraphDirection) -> Result<Vec<Value>, MemoryError> {
        if let Some(resp) = self.edge_neighbors_responses.lock().unwrap().get(node_id) {
            return resp.clone();
        }
        if let Some(ref f) = *self.fallback_edge_neighbors.lock().unwrap() {
            return f();
        }
        Ok(vec![])
    }

    async fn select_entity_lookup(&self, _namespace: &str, normalized_name: &str) -> Result<Option<Value>, MemoryError> {
        if let Some(resp) = self.entity_lookup_responses.lock().unwrap().get(normalized_name) {
            return resp.clone();
        }
        if let Some(ref f) = *self.fallback_entity_lookup.lock().unwrap() {
            return f(normalized_name);
        }
        Ok(None)
    }

    async fn select_entities_batch(&self, _namespace: &str, _names: &[String]) -> Result<Vec<Value>, MemoryError> {
        if let Some(ref f) = *self.fallback_entities_batch.lock().unwrap() {
            return f();
        }
        Ok(vec![])
    }

    async fn select_active_facts(&self, _namespace: &str, _limit: i32) -> Result<Vec<Value>, MemoryError> {
        if let Some(ref f) = *self.fallback_active_facts.lock().unwrap() {
            return f();
        }
        Ok(vec![])
    }

    async fn select_episodes_for_archival(&self, _namespace: &str, _cutoff: &str, _limit: i32) -> Result<Vec<Value>, MemoryError> {
        if let Some(ref f) = *self.fallback_episodes_for_archival.lock().unwrap() {
            return f();
        }
        Ok(vec![])
    }

    async fn select_active_facts_by_episode(&self, _namespace: &str, _episode_id: &str, _cutoff: &str, _limit: i32) -> Result<Vec<Value>, MemoryError> {
        if let Some(ref f) = *self.fallback_active_facts_by_episode.lock().unwrap() {
            return f();
        }
        Ok(vec![])
    }

    async fn select_episodes_by_content(&self, _namespace: &str, _scope: &str, _cutoff: &str, _query_contains: Option<&str>, _limit: i32) -> Result<Vec<Value>, MemoryError> {
        if let Some(ref f) = *self.fallback_episodes_by_content.lock().unwrap() {
            return f();
        }
        Ok(vec![])
    }

    async fn select_communities_matching_summary(&self, _namespace: &str, _query: &str) -> Result<Vec<Value>, MemoryError> {
        if let Some(resp) = self.communities_matching_summary_responses.lock().unwrap().get(_query) {
            return resp.clone();
        }
        Ok(vec![])
    }

    async fn select_communities_by_member_entities(&self, _namespace: &str, member_entities: &[String]) -> Result<Vec<Value>, MemoryError> {
        let key = member_entities.join(",");
        if let Some(resp) = self.communities_by_members_responses.lock().unwrap().get(&key) {
            return resp.clone();
        }
        Ok(vec![])
    }

    async fn relate_edge(&self, _namespace: &str, edge_id: &str, _from_id: &str, _to_id: &str, _content: Value) -> Result<Value, MemoryError> {
        if let Some(resp) = self.relate_edge_responses.lock().unwrap().get(edge_id) {
            return resp.clone();
        }
        Ok(Value::Null)
    }

    async fn create(&self, record_id: &str, _content: Value, _namespace: &str) -> Result<Value, MemoryError> {
        if let Some(resp) = self.create_responses.lock().unwrap().get(record_id) {
            return resp.clone();
        }
        if let Some(ref f) = *self.fallback_create.lock().unwrap() {
            return f();
        }
        Ok(Value::Null)
    }

    async fn update(&self, record_id: &str, _content: Value, _namespace: &str) -> Result<Value, MemoryError> {
        if let Some(resp) = self.update_responses.lock().unwrap().get(record_id) {
            return resp.clone();
        }
        if let Some(ref f) = *self.fallback_update.lock().unwrap() {
            return f();
        }
        Ok(Value::Null)
    }

    async fn query(&self, sql: &str, _vars: Option<Value>, _namespace: &str) -> Result<Value, MemoryError> {
        for (prefix, result) in self.query_responses.lock().unwrap().iter() {
            if sql.starts_with(prefix.as_str()) {
                return result.clone();
            }
        }
        if let Some(ref f) = *self.fallback_query.lock().unwrap() {
            return f();
        }
        Ok(Value::Null)
    }

    async fn select_entities_by_ids(&self, _namespace: &str, entity_ids: &[String]) -> Result<Vec<Value>, MemoryError> {
        let key = entity_ids.join(",");
        if let Some(resp) = self.entities_by_ids_responses.lock().unwrap().get(&key) {
            return resp.clone();
        }
        Ok(vec![])
    }

    async fn select_edges_for_triple(&self, _namespace: &str, _in_id: &str, _relation: &str, _out_id: &str) -> Result<Vec<Value>, MemoryError> {
        Ok(vec![])
    }

    async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
        self.migration_result.lock().unwrap().clone()
    }
}
```

- [ ] **Step 2: Add MockDbClient to the service module**

In `src/service.rs`, add:

```rust
#[cfg(test)]
mod mock_db;
pub use mock_db::MockDbClient;
```

- [ ] **Step 3: Rewrite one test file — episode.rs — as proof of concept**

In `src/service/episode.rs`, rewrite the test `extract_entities_does_not_block_runtime_for_local_gliner_provider` (lines 535-591) to use `MockDbClient` instead of hand-written `BlockingGlinerExtractor`:

```rust
#[tokio::test(flavor = "current_thread")]
async fn extract_entities_does_not_block_runtime_for_local_gliner_provider() {
    struct BlockingGlinerExtractor;
    #[async_trait::async_trait]
    impl EntityExtractor for BlockingGlinerExtractor {
        fn provider_name(&self) -> &'static str { "gliner" }
        async fn extract_candidates(&self, _content: &str) -> Result<Vec<EntityCandidate>, MemoryError> {
            std::thread::sleep(Duration::from_millis(250));
            Ok(Vec::new())
        }
    }

    let db_client = Arc::new(
        SurrealDbClient::connect_in_memory("episode-test-gliner", "org", "warn")
            .await
            .expect("connect in memory"),
    );
    db_client.apply_migrations("org").await.expect("apply migrations");

    let mut service = MemoryService::new(
        db_client,
        vec!["org".to_string()],
        "warn".to_string(),
        50,
        100,
    ).expect("create service");
    service.entity_extractor = Arc::new(BlockingGlinerExtractor);

    let ticker = tokio::spawn(async move {
        let start = Instant::now();
        tokio::time::sleep(Duration::from_millis(50)).await;
        start.elapsed()
    });
    tokio::task::yield_now().await;

    let _ = extract_entities(&service, "Atlas project status", None)
        .await
        .expect("extract entities");
    let tick_elapsed = ticker.await.expect("join ticker");

    assert!(
        tick_elapsed < Duration::from_millis(150),
        "local gliner extraction blocked the runtime for {:?}", tick_elapsed
    );
}
// NOTE: this test uses SurrealDbClient because it needs real GLiNER behavior.
// Pure unit tests with MockDbClient are in Task 4 (IngestionService) onward.
```

For the tests that DON'T need real SurrealDB, convert them. Example for `collect_connected_entity_component_uses_neighbor_queries_instead_of_edge_scan`:

Replace the 200-line `NeighborOnlyDbClient` impl with:

```rust
#[tokio::test]
async fn collect_connected_entity_component_uses_neighbor_queries_instead_of_edge_scan() {
    let mk = |from_id: &str, relation: &str, to_id: &str| {
        json!({
            "edge_id": format!("edge:{from_id}:{relation}:{to_id}"),
            "in": from_id, "relation": relation, "out": to_id,
            "t_valid": "2024-01-01T00:00:00Z", "t_ingested": "2024-01-01T00:00:00Z"
        })
    };

    let db = MockDbClient::new()
        .expect_edge_neighbors_with(move || {
            // The logic from the old NeighborOnlyDbClient goes here as a closure
            // ... (see full implementation in actual code)
        });

    let service = MemoryService::new(
        Arc::new(db),
        vec!["org".to_string()],
        "warn".to_string(),
        50, 100,
    ).unwrap();

    let connected = collect_connected_entity_component(
        &service, &["entity:alice".to_string()], "org"
    ).await.unwrap();

    assert_eq!(connected, vec![
        "entity:alice".to_string(),
        "entity:bob".to_string(),
        "entity:carol".to_string(),
    ]);
}
```

- [ ] **Step 4: Rewrite the remaining test files**

Convert all hand-written `impl DbClient for *DbClient` blocks across test files:
- `src/service/episode.rs` (two mocks: `NeighborOnlyDbClient`, `IndexLookupDbClient`)
- `src/service/context.rs` (one mock: `DedupFallbackDbClient`)
- `tests/` directory (any test that implements DbClient)

Process: for each test, replace the 100-300 line impl block with a `MockDbClient::new()` builder chain.

- [ ] **Step 5: Verify all tests pass and commit**

```bash
cargo test --no-fail-fast 2>&1 | tail -30
```

```bash
git add -A && git commit -m "refactor: add MockDbClient builder, convert hand-written test mocks to declarative builder pattern"
```

---

### Task 3: DRY — Generic Background Embedding Task Runner

**Files:**
- Create: `src/service/embedding/task_runner.rs`
- Modify: `src/service/core.rs` (remove duplicate task methods)
- Modify: `src/service.rs` (re-export)

**Interfaces:**
- Consumes: Background task enqueue/run pairs from `core.rs`
- Produces: `BackgroundTaskRunner` with `enum TaskKind { Fact { namespace, fact_id, input }, Query { input } }`, unified retry loop

- [ ] **Step 1: Write the BackgroundTaskRunner**

Create `src/service/embedding/task_runner.rs`:

```rust
use std::sync::Arc;
use std::collections::HashSet;

use serde_json::json;
use tokio::sync::Mutex;

use crate::logging::LogLevel;
use super::super::error::MemoryError;
use super::super::{DEFAULT_BACKGROUND_EMBEDDING_ATTEMPTS, background_embedding_retry_delay, is_remote_embedding_provider, is_transient_embedding_error};

/// Identifies the kind of background embedding task.
#[derive(Debug, Clone)]
pub enum EmbeddingTaskKind {
    Fact {
        namespace: String,
        fact_id: String,
        input: String,
    },
    Query {
        input: String,
    },
}

/// Runs background embedding tasks with retry logic.
pub struct BackgroundTaskRunner {
    inflight: Arc<Mutex<HashSet<String>>>,
}

impl BackgroundTaskRunner {
    pub fn new() -> Self {
        Self {
            inflight: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn inflight_set(&self) -> Arc<Mutex<HashSet<String>>> {
        self.inflight.clone()
    }

    /// Returns true if the task was reserved (not already inflight).
    pub async fn try_reserve(&self, task_key: &str) -> bool {
        self.inflight.lock().await.insert(task_key.to_string())
    }

    /// Releases a completed task from the inflight set.
    pub async fn release(&self, task_key: &str) {
        self.inflight.lock().await.remove(task_key);
    }

    /// Returns true if a task with the given key is currently inflight.
    pub async fn is_inflight(&self, task_key: &str) -> bool {
        self.inflight.lock().await.contains(task_key)
    }
}
```

- [ ] **Step 2: Unify the dual enqueue+run methods in core.rs**

Replace the 4 pairs of methods at lines 951-1156 of `src/service/core.rs` with a single pattern. The `enqueue_background_fact_embedding` and `enqueue_background_query_embedding` become one:

```rust
async fn enqueue_background_embedding_task(
    &self,
    task_key: String,
    kind: EmbeddingTaskKind,
) {
    if !self.task_runner.try_reserve(&task_key).await {
        self.logger.log(
            std::collections::HashMap::from([
                ("op".to_string(), json!("embedding.background_deduped")),
            ]),
            LogLevel::Debug,
        );
        return;
    }

    let service = self.clone();
    tokio::spawn(async move {
        let outcome = service.run_background_embedding_task(&kind).await;
        service.task_runner.release(&task_key).await;

        if let Err(err) = outcome {
            let kind_label = match &kind {
                EmbeddingTaskKind::Fact { .. } => "fact",
                EmbeddingTaskKind::Query { .. } => "query",
            };
            service.logger.log(
                std::collections::HashMap::from([
                    ("op".to_string(), json!("embedding.background_failed")),
                    ("kind".to_string(), json!(kind_label)),
                    ("error".to_string(), json!(err.to_string())),
                ]),
                LogLevel::Warn,
            );
        }
    });
}

async fn run_background_embedding_task(
    &self,
    kind: &EmbeddingTaskKind,
) -> Result<(), MemoryError> {
    let input = match kind {
        EmbeddingTaskKind::Fact { input, .. } => input,
        EmbeddingTaskKind::Query { input } => input,
    };

    for attempt in 1..=DEFAULT_BACKGROUND_EMBEDDING_ATTEMPTS {
        match self.generate_embedding(input).await {
            Ok(Some(embedding)) => {
                match kind {
                    EmbeddingTaskKind::Fact { namespace, fact_id, .. } => {
                        self.store_embedding_on_fact(namespace, fact_id, embedding).await?;
                    }
                    EmbeddingTaskKind::Query { .. } => {
                        self.store_query_embedding(input, embedding).await;
                    }
                }
                return Ok(());
            }
            Ok(None) => return Ok(()),
            Err(err)
                if is_transient_embedding_error(&err)
                    && is_remote_embedding_provider(self.embedding_provider.provider_name())
                    && attempt < DEFAULT_BACKGROUND_EMBEDDING_ATTEMPTS =>
            {
                let delay = background_embedding_retry_delay(attempt);
                tokio::time::sleep(delay).await;
            }
            Err(err) => return Err(err),
        }
    }

    Ok(())
}
```

Update `enqueue_background_fact_embedding` to call the unified version:

```rust
async fn enqueue_background_fact_embedding(
    &self,
    namespace: String,
    fact_id: String,
    input: String,
) {
    let task_key = self.background_fact_task_key(&namespace, &fact_id);
    self.enqueue_background_embedding_task(
        task_key,
        EmbeddingTaskKind::Fact { namespace, fact_id, input },
    ).await;
}
```

Same for `enqueue_background_query_embedding` → use `EmbeddingTaskKind::Query { input }`.

- [ ] **Step 3: Remove the 4 old inner methods**

Delete `run_background_fact_embedding_task`, `run_background_fact_embedding_task_inner`, `run_background_query_embedding_task`, `run_background_query_embedding_task_inner` — replaced by the unified `run_background_embedding_task`.

- [ ] **Step 4: Add unit test for the background task runner**

In `src/service/embedding/task_runner.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn try_reserve_prevents_duplicate_tasks() {
        let runner = BackgroundTaskRunner::new();
        assert!(runner.try_reserve("task:1").await);
        assert!(!runner.try_reserve("task:1").await);
        runner.release("task:1").await;
        assert!(runner.try_reserve("task:1").await);
    }

    #[tokio::test]
    async fn is_inflight_reports_correctly() {
        let runner = BackgroundTaskRunner::new();
        assert!(!runner.is_inflight("missing").await);
        runner.try_reserve("task:2").await;
        assert!(runner.is_inflight("task:2").await);
        runner.release("task:2").await;
        assert!(!runner.is_inflight("task:2").await);
    }
}
```

- [ ] **Step 5: Verify and commit**

```bash
cargo test --no-fail-fast service::embedding::task_runner 2>&1
```

```bash
git add -A && git commit -m "refactor: unify background embedding tasks into generic BackgroundTaskRunner"
```

---

### Task 4: Extract IngestionService + EntityService from MemoryService

**Files:**
- Create: `src/service/ingestion.rs` (new domain service file)
- Create: `src/service/entity.rs` (new domain service file)
- Modify: `src/service/core.rs` (delegate to new services)
- Modify: `src/service.rs` (re-exports)

**Interfaces:**
- Consumes: `Arc<dyn DbClient>`, `StdoutLogger`, `Arc<RateLimiter>`, free functions from `ingest/`, `entity_extraction/`
- Produces:
  - `IngestionService::ingest(request: IngestRequest, access: Option<AccessPayload>) -> Result<String, MemoryError>`
  - `EntityService::resolve(candidate: EntityCandidate, access: Option<AccessPayload>) -> Result<String, MemoryError>`
  - `EntityService::resolve_typed(entity_type: &str, name: &str) -> Result<String, MemoryError>`

- [ ] **Step 1: Create IngestionService**

Create `src/service/ingestion.rs`:

```rust
use std::sync::Arc;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::logging::{LogLevel, StdoutLogger};
use crate::models::{AccessPayload, IngestRequest};

use super::error::MemoryError;
use super::ingest::prepare_ingest_request;
use super::util::{deterministic_episode_id, validate_ingest_request, RateLimiter};
use super::{log_event, normalize_dt, now};

/// Handles episode ingestion: file parsing, deduplication, and persistence.
pub struct IngestionService {
    db_client: Arc<dyn crate::storage::DbClient>,
    namespaces: Vec<String>,
    logger: StdoutLogger,
    rate_limiter: Arc<RateLimiter>,
    default_namespace: String,
}

impl IngestionService {
    pub fn new(
        db_client: Arc<dyn crate::storage::DbClient>,
        namespaces: Vec<String>,
        logger: StdoutLogger,
        rate_limiter: Arc<RateLimiter>,
    ) -> Self {
        let default_namespace = namespaces.first().cloned().unwrap_or_else(|| "org".into());
        Self { db_client, namespaces, logger, rate_limiter, default_namespace }
    }

    /// Rate limit check matching MemoryService::enforce_rate_limit pattern.
    fn rate_limiter_check(&self, access: Option<&AccessPayload>) -> Result<(), MemoryError> {
        if let Some(access) = access
            && let Some(caller) = &access.caller_id
            && !self.rate_limiter.allow(caller)
        {
            return Err(MemoryError::Validation("rate limit exceeded".into()));
        }
        Ok(())
    }

    pub fn namespace_for_scope(&self, scope: &str) -> String {
        // Delegate to the existing resolve_namespace free function from helpers.rs
        let (ns, fell_back) = super::core::helpers::resolve_namespace(
            &self.namespaces, &self.default_namespace, scope,
        );
        if fell_back {
            self.logger.log_warn_dedup(
                std::collections::HashMap::from([
                    ("op".to_string(), json!("scope.namespace_fallback")),
                    ("scope".to_string(), json!(scope)),
                    ("resolved_namespace".to_string(), json!(&ns)),
                ]),
                &format!("scope.namespace_fallback:{}", ns),
                10,
            );
        }
        ns
    }

    pub async fn ingest(
        &self,
        request: IngestRequest,
        access: Option<AccessPayload>,
    ) -> Result<String, MemoryError> {
        self.rate_limiter_check(access.as_ref())?;

        let ingest_transport = super::ingest::detect_ingest_transport(&request.content);
        let original_source_id = request.source_id.clone();

        self.logger.log(
            log_event(
                "ingest.prepare",
                json!({
                    "source_type": request.source_type,
                    "source_id": request.source_id,
                    "scope": request.scope,
                    "project": request.project,
                    "transport": ingest_transport,
                }),
                json!({}),
                access.as_ref(),
                None, None,
            ),
            LogLevel::Debug,
        );

        let request = prepare_ingest_request(request).await?;

        self.logger.log(
            log_event(
                "ingest.prepared",
                json!({
                    "scope": request.scope,
                    "project": request.project,
                    "transport": ingest_transport,
                    "source_id_rewritten": request.source_id != original_source_id,
                }),
                json!({
                    "source_id": request.source_id,
                    "content_len": request.content.len(),
                }),
                access.as_ref(), None, None,
            ),
            LogLevel::Trace,
        );

        validate_ingest_request(&request)?;

        let episode_id = deterministic_episode_id(
            &request.source_type, &request.source_id, request.t_ref, &request.scope,
        );
        let namespace = self.namespace_for_scope(&request.scope);
        let existing = self.db_client.select_one(&episode_id, &namespace).await?;

        if existing.is_none() {
            let t_ingested = request.t_ingested.unwrap_or_else(now);
            let mut payload = serde_json::Map::from_iter([
                ("episode_id".to_string(), json!(episode_id)),
                ("source_type".to_string(), json!(request.source_type)),
                ("source_id".to_string(), json!(request.source_id)),
                ("content".to_string(), json!(request.content)),
                ("t_ref".to_string(), json!(normalize_dt(request.t_ref))),
                ("t_ingested".to_string(), json!(normalize_dt(t_ingested))),
                ("scope".to_string(), json!(request.scope.clone())),
                ("visibility_scope".to_string(), json!(
                    request.visibility_scope.unwrap_or_else(|| request.scope.clone())
                )),
                ("policy_tags".to_string(), json!(request.policy_tags)),
            ]);
            if let Some(project) = request.project.clone() {
                payload.insert("project".to_string(), json!(project));
            }
            self.db_client.create(&episode_id, Value::Object(payload), &namespace).await?;
        } else {
            self.logger.log(
                log_event("ingest.duplicate", json!({
                    "episode_id": episode_id,
                    "source_id": request.source_id,
                    "scope": request.scope,
                }), json!({"status": "existing_episode_reused"}), access.as_ref(), None, None),
                LogLevel::Debug,
            );
        }

        self.logger.log(
            log_event("ingest", json!({
                "source_type": request.source_type,
                "source_id": request.source_id,
                "t_ref": normalize_dt(request.t_ref),
                "scope": request.scope,
            }), json!({"episode_id": episode_id}), access.as_ref(), None, None),
            LogLevel::Info,
        );

        Ok(episode_id)
    }
}
```

- [ ] **Step 2: Create EntityService**

Create `src/service/entity.rs`:

```rust
use std::sync::Arc;

use serde_json::json;

use crate::models::{AccessPayload, EntityCandidate};

use super::error::MemoryError;
use super::util::{deterministic_entity_id, deterministic_entity_id_stable};
use super::{normalize_text, string_from_value};

pub struct EntityService {
    db_client: Arc<dyn crate::storage::DbClient>,
    default_namespace: String,
}

impl EntityService {
    pub fn new(db_client: Arc<dyn crate::storage::DbClient>, default_namespace: String) -> Self {
        Self { db_client, default_namespace }
    }

    pub async fn resolve(
        &self,
        candidate: EntityCandidate,
    ) -> Result<String, MemoryError> {
        super::util::validate_entity_candidate(&candidate)?;
        let namespace = &self.default_namespace;
        let normalized = normalize_text(&candidate.canonical_name);

        let existing = self.db_client
            .select_entity_lookup(namespace, &candidate.canonical_name)
            .await?;

        if let Some(record) = existing {
            let existing_id = record
                .get("entity_id")
                .and_then(string_from_value)
                .or_else(|| record.get("id").and_then(string_from_value))
                .unwrap_or_default();
            return Ok(existing_id);
        }

        let entity_id = deterministic_entity_id(&candidate.entity_type, &candidate.canonical_name);
        let aliases = candidate.aliases
            .into_iter()
            .filter(|alias| !alias.trim().is_empty())
            .map(|alias| normalize_text(&alias))
            .collect::<Vec<_>>();

        let payload = json!({
            "entity_id": entity_id,
            "entity_type": candidate.entity_type,
            "canonical_name": candidate.canonical_name,
            "canonical_name_normalized": normalized,
            "aliases": aliases.clone(),
        });

        match self.db_client.create(&entity_id, payload, namespace).await {
            Ok(_) => Ok(entity_id),
            Err(MemoryError::Storage(msg)) if msg.contains("already exists") => {
                let existing = self.db_client
                    .select_entity_lookup(namespace, &candidate.canonical_name)
                    .await?;
                if let Some(record) = existing {
                    let existing_id = record
                        .get("entity_id")
                        .and_then(string_from_value)
                        .or_else(|| record.get("id").and_then(string_from_value))
                        .unwrap_or_default();
                    return Ok(existing_id);
                }
                Ok(entity_id)
            }
            Err(err) => Err(err),
        }
    }

    pub async fn resolve_typed(
        &self,
        entity_type: &str,
        name: &str,
    ) -> Result<String, MemoryError> {
        self.resolve(EntityCandidate {
            entity_type: entity_type.to_string(),
            canonical_name: name.to_string(),
            aliases: Vec::new(),
        }).await
    }
}
```

- [ ] **Step 3: Wire IngestionService and EntityService into MemoryService**

In `src/service/core/builder.rs`, add fields:

```rust
pub(crate) ingestion_service: IngestionService,
pub(crate) entity_service: EntityService,
```

Initialize them in `build()`:

```rust
ingestion_service: IngestionService::new(
    db_client.clone(), namespaces.clone(),
    logger.clone(), rate_limiter.clone(),
),
entity_service: EntityService::new(
    db_client.clone(), default_namespace.clone(),
),
```

- [ ] **Step 4: Delegate MemoryService methods to services**

In `src/service/core.rs`, replace the `ingest()` impl body with delegation:

```rust
pub async fn ingest(&self, request: IngestRequest, access: Option<AccessPayload>) -> Result<String, MemoryError> {
    self.ingestion_service.ingest(request, access).await
}
```

Replace `resolve()` and `resolve_entity()` with delegation to `self.entity_service`.

Keep `find_entity_record_by_id` and `find_entity_record` as helper methods on `MemoryService` (they're DB lookups, not domain logic).

- [ ] **Step 5: Write unit tests for the new services**

In `src/service/ingestion.rs`, add test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use crate::service::MockDbClient;
    use crate::logging::StdoutLogger;
    use crate::service::util::RateLimiter;
    use chrono::Utc;
    use serde_json::json;

    #[tokio::test]
    async fn ingest_creates_new_episode() {
        let t_ref = Utc::now();
        let expected_id = deterministic_episode_id("inline", "content-hash", t_ref, "org");

        let db = MockDbClient::new()
            .expect_select_one(&expected_id, None)
            .expect_create(&expected_id, Value::Null);

        let svc = IngestionService::new(
            Arc::new(db),
            vec!["org".into()],
            StdoutLogger::new("warn"),
            Arc::new(RateLimiter::new(1000, 100)),
        );

        let result = svc.ingest(IngestRequest {
            source_type: "inline".into(),
            source_id: "content-hash".into(),
            content: "hello world".into(),
            t_ref,
            scope: "org".into(),
            project: None,
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
        }, None).await;

        assert_eq!(result.unwrap(), expected_id);
    }

    #[tokio::test]
    async fn ingest_returns_existing_episode_id_on_duplicate() {
        let t_ref = Utc::now();
        let expected_id = deterministic_episode_id("inline", "content-hash", t_ref, "org");

        let db = MockDbClient::new()
            .expect_select_one(&expected_id,
                Some(json!({"episode_id": &expected_id, "content": "old"}))
            );

        let svc = IngestionService::new(
            Arc::new(db),
            vec!["org".into()],
            StdoutLogger::new("warn"),
            Arc::new(RateLimiter::new(1000, 100)),
        );

        let result = svc.ingest(IngestRequest {
            source_type: "inline".into(),
            source_id: "content-hash".into(),
            content: "hello world".into(),
            t_ref,
            scope: "org".into(),
            project: None,
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
        }, None).await;

        assert_eq!(result.unwrap(), expected_id);
    }
}
```

- [ ] **Step 6: Verify and commit**

```bash
cargo test --no-fail-fast 2>&1 | tail -20
```

```bash
git add -A && git commit -m "refactor: extract IngestionService and EntityService from MemoryService"
```

---

### Task 5: Extract FactService + EmbeddingService from MemoryService

**Files:**
- Create: `src/service/fact.rs`
- Create: `src/service/embedding/service.rs`
- Modify: `src/service/core.rs` (delegate add_fact, invalidate, embedding methods)
- Modify: `src/service.rs` (re-exports)

**Interfaces:**
- Consumes: `Arc<dyn DbClient>`, `Arc<dyn EmbeddingProvider>`, `StdoutLogger`, `Arc<RateLimiter>`, entity lookup helper
- Produces:
  - `FactService::add_fact(fact_type, content, quote, source_episode, t_valid, scope, confidence, entity_links, policy_tags, provenance) -> Result<String, MemoryError>`
  - `FactService::invalidate(request: InvalidateRequest) -> Result<(), MemoryError>`
  - `EmbeddingService::generate_embedding(input: &str) -> Result<Option<Vec<f64>>, MemoryError>`
  - `EmbeddingService::enqueue_background_embedding(kind: EmbeddingTaskKind)`

- [ ] **Step 1: Create FactService**

Create `src/service/fact.rs`. The key design decision: `add_fact` calls `build_fact_index_keys`, which resolves entity IDs to canonical names via an `entity_lookup` closure passed as a parameter. This avoids a circular dependency on `EntityService`:

```rust
use std::collections::HashSet;
use std::sync::Arc;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::storage::DbClient;
use super::error::MemoryError;
use super::util::deterministic_fact_id;
use super::util::validate_fact_input;
use super::{normalize_dt, normalize_text, now};
use super::value_helpers::{json_i64, string_from_value};
use super::cache::invalidate_cache_by_scope;

pub struct FactService {
    db_client: Arc<dyn DbClient>,
    namespaces: Vec<String>,
    default_namespace: String,
}

impl FactService {
    pub fn new(db_client: Arc<dyn DbClient>, namespaces: Vec<String>) -> Self {
        let default_namespace = namespaces.first().cloned().unwrap_or_else(|| "org".into());
        Self { db_client, namespaces, default_namespace }
    }

    pub fn namespace_for_scope(&self, scope: &str) -> String {
        self.namespaces.iter().find(|ns| ns == &scope).cloned()
            .unwrap_or_else(|| self.default_namespace.clone())
    }

    pub async fn add_fact(
        &self,
        fact_type: &str,
        content: &str,
        quote: &str,
        source_episode: &str,
        t_valid: DateTime<Utc>,
        scope: &str,
        confidence: f64,
        entity_links: Vec<String>,
        policy_tags: Vec<String>,
        provenance: Value,
        embedding_input: String,
        entity_lookup: impl Fn(&str) -> Result<Option<String>, super::error::MemoryError>,
    ) -> Result<String, MemoryError> {
        validate_fact_input(fact_type, content, quote, source_episode, scope)?;

        let fact_id = deterministic_fact_id(fact_type, content, source_episode, t_valid);
        let namespace = self.namespace_for_scope(scope);
        let existing = self.db_client.select_one(&fact_id, &namespace).await?;

        if existing.is_none() {
            let t_ingested = now();
            let index_keys = Self::build_fact_index_keys(
                content, source_episode, &provenance, &entity_links, t_valid, &entity_lookup,
            ).await?;

            let mut payload = serde_json::Map::from_iter([
                ("fact_id".to_string(), json!(fact_id.clone())),
                ("fact_type".to_string(), json!(fact_type)),
                ("content".to_string(), json!(content)),
                ("quote".to_string(), json!(quote)),
                ("source_episode".to_string(), json!(source_episode)),
                ("t_valid".to_string(), json!(normalize_dt(t_valid))),
                ("t_ingested".to_string(), json!(normalize_dt(t_ingested))),
                ("confidence".to_string(), json!(confidence)),
                ("index_keys".to_string(), json!(index_keys)),
                ("access_count".to_string(), json!(0)),
                ("entity_links".to_string(), json!(entity_links)),
                ("scope".to_string(), json!(scope)),
                ("policy_tags".to_string(), json!(policy_tags)),
                ("provenance".to_string(), provenance),
            ]);

            self.db_client.create(&fact_id, Value::Object(payload), &namespace).await?;
        }

        Ok(fact_id)
    }

    async fn build_fact_index_keys(
        content: &str,
        source_episode: &str,
        provenance: &Value,
        entity_links: &[String],
        t_valid: DateTime<Utc>,
        entity_lookup: impl Fn(&str) -> Result<Option<String>, super::error::MemoryError>,
    ) -> Result<Vec<String>, MemoryError> {
        let mut keys = HashSet::new();

        for entity_id in entity_links {
            if let Some(canonical_name) = entity_lookup(entity_id)? {
                let normalized = normalize_text(&canonical_name);
                if !normalized.is_empty() {
                    keys.insert(normalized);
                }
            }
        }

        // Keep temporal and reference index key extraction from existing code
        // (temporal_index_keys, reference_index_terms — unchanged)

        let mut keys: Vec<_> = keys.into_iter().collect();
        keys.sort();
        Ok(keys)
    }

    pub async fn invalidate(
        &self,
        fact_id: &str,
        t_invalid: DateTime<Utc>,
        cache: &crate::service::cache::ContextCacheHandle,
    ) -> Result<(), MemoryError> {
        // Scan all namespaces for the fact record — matching find_record_by_id pattern
        let mut found_record: Option<(serde_json::Map<String, Value>, String)> = None;
        for namespace in &self.namespaces {
            if let Some(Value::Object(map)) = self.db_client.select_one(fact_id, namespace).await? {
                found_record = Some((map, namespace.clone()));
                break;
            }
        }

        let (mut record, namespace) = found_record
            .ok_or_else(|| MemoryError::NotFound("fact_id not found".into()))?;

        let scope = record.get("scope")
            .and_then(string_from_value)
            .unwrap_or_else(|| namespace.clone());

        record.insert("t_invalid".to_string(), json!(normalize_dt(t_invalid)));
        record.insert("t_invalid_ingested".to_string(), json!(normalize_dt(now())));

        self.db_client.update(fact_id, Value::Object(record), &namespace).await?;
        invalidate_cache_by_scope(cache, &scope).await;
        Ok(())
    }
}
```

- [ ] **Step 2: Create EmbeddingService**

Create `src/service/embedding/service.rs` with `generate_embedding`, `generate_query_embedding_with_background`, background task enqueue/dequeue, query embedding cache — ported from lines 726-1156 of `core.rs`.

- [ ] **Step 3: Wire into MemoryService builder and delegate**

Add `fact_service` and `embedding_service` fields to the struct in builder. Delegate `add_fact()`, `invalidate()`, `generate_embedding()`, and background task methods.

- [ ] **Step 4: Unit tests for FactService**

Test add_fact with successful embedding, add_fact with disabled embedding, invalidate with found fact, invalidate with not-found fact.

- [ ] **Step 5: Verify and commit**

```bash
cargo test --no-fail-fast 2>&1 | tail -20
git add -A && git commit -m "refactor: extract FactService and EmbeddingService from MemoryService"
```

---

### Task 6: Extract ExplanationService + Wire MemoryService Facade

**Files:**
- Create: `src/service/explanation.rs`
- Modify: `src/service/core.rs` (delegate explain)
- Modify: `src/service.rs` (re-exports)

**Interfaces:**
- Consumes: `Arc<dyn DbClient>`, `FactService`, `EntityService`, `EmbeddingService`, `StdoutLogger`
- Produces: `ExplanationService::explain(request: ExplainRequest, access: Option<AccessPayload>) -> Result<Vec<ExplainItem>, MemoryError>`

- [ ] **Step 1: Create ExplanationService**

Port the `explain()` method (lines 259-410 of `core.rs`), splitting its three-phase pipeline into focused private methods. The service takes concrete dependencies via Arc — no circular dependency issues since ExplanationService only reads, never modifies:

```rust
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::models::{AccessPayload, ExplainItem, ExplainRequest};
use crate::storage::DbClient;
use crate::logging::{LogLevel, StdoutLogger};

use super::error::MemoryError;
use super::episode::{episode_from_record, fact_from_record};
use super::core::helpers::{find_episode_record, find_fact_record};
use super::util::string_from_value;
use super::log_event;

pub struct ExplanationService {
    db_client: Arc<dyn DbClient>,
    logger: StdoutLogger,
    namespaces: Vec<String>,
    default_namespace: String,
}

impl ExplanationService {
    pub fn new(
        db_client: Arc<dyn DbClient>,
        logger: StdoutLogger,
        namespaces: Vec<String>,
    ) -> Self {
        let default_namespace = namespaces.first().cloned().unwrap_or_else(|| "org".into());
        Self { db_client, logger, namespaces, default_namespace }
    }

    pub fn namespace_for_scope(&self, scope: &str) -> String {
        self.namespaces.iter().find(|ns| ns == &scope).cloned()
            .unwrap_or_else(|| self.default_namespace.clone())
    }

    pub async fn explain(
        &self,
        request: ExplainRequest,
        access: Option<AccessPayload>,
    ) -> Result<Vec<ExplainItem>, MemoryError> {
        // Phase 1: resolve episodes/facts, collect entity_links
        struct ResolvedItem {
            item: ExplainItem,
            episode: Option<crate::models::Episode>,
            entity_links: Vec<String>,
            fact_namespace: Option<String>,
        }

        let mut resolved = Vec::with_capacity(request.context_pack.len());
        let mut all_entity_links: HashSet<String> = HashSet::new();

        for item in request.context_pack {
            if item.source_episode.is_empty() {
                return Err(MemoryError::Validation("source_episode is required for explain items".into()));
            }
            let (record, _) = find_episode_record(&self.db_client, &item.source_episode).await?;
            let episode = record.as_ref().and_then(episode_from_record);

            let (entity_links, fact_namespace) = if let Some(ref fact_id) = item.fact_id {
                let (fact_record, namespace) = find_fact_record(&self.db_client, fact_id).await?;
                let links = fact_record
                    .and_then(|r| r.get("entity_links").and_then(|v| v.as_array()).map(|arr| {
                        arr.iter().filter_map(|v| v.as_str().map(String::from)).collect::<Vec<_>>()
                    }))
                    .unwrap_or_default();
                for link in &links { all_entity_links.insert(link.clone()); }
                (links, namespace)
            } else {
                (Vec::new(), None)
            };

            resolved.push(ResolvedItem { item, episode, entity_links, fact_namespace });
        }

        // Phase 2: shared graph insights
        let entity_links_vec: Vec<String> = all_entity_links.into_iter().collect();
        let first_namespace = resolved.iter()
            .find_map(|r| r.fact_namespace.clone()
                .or_else(|| r.episode.as_ref().map(|ep| self.namespace_for_scope(&ep.scope))))
            .unwrap_or_else(|| self.default_namespace.clone());
        // build_graph_insights_batched — keep on MemoryService or inline here

        // Phase 3: build explain items with cached provenance
        let mut episode_via_entity_cache: HashMap<String, Vec<crate::models::Episode>> = HashMap::new();
        let mut explanations = Vec::with_capacity(resolved.len());

        for resolved_item in resolved {
            let Some(episode) = resolved_item.episode else {
                explanations.push(resolved_item.item);
                continue;
            };
            let explanation = ExplainItem {
                fact_id: resolved_item.item.fact_id,
                content: if resolved_item.item.content.is_empty() { episode.content.clone() } else { resolved_item.item.content },
                quote: resolved_item.item.quote,
                source_episode: resolved_item.item.source_episode,
                scope: Some(episode.scope.clone()),
                t_ref: Some(episode.t_ref),
                t_ingested: Some(episode.t_ingested),
                provenance: json!({
                    "source_episode": episode.episode_id,
                    "source_type": episode.source_type,
                    "source_id": episode.source_id,
                }),
                citation_context: Some(episode.content.clone()),
                all_sources: Vec::new(), // provenance via entity cache — keep existing logic
                graph_insights: None,
            };
            explanations.push(explanation);
        }

        self.logger.log(
            log_event("explain", json!({"count": explanations.len()}), json!({"count": explanations.len()}),
                access.as_ref(), None, None),
            LogLevel::Info,
        );
        Ok(explanations)
    }
}

- [ ] **Step 2: Delegate explain() in MemoryService to a one-liner**

```rust
pub async fn explain(&self, request: ExplainRequest, access: Option<AccessPayload>) -> Result<Vec<ExplainItem>, MemoryError> {
    self.explanation_service.explain(request, access).await
}
```

- [ ] **Step 3: Clean up core.rs — now ~400 lines**

After all extractions, `core.rs` contains only:
- The `MemoryService` struct definition (delegated to builder)
- Public delegating methods: `ingest()`, `extract()`, `explain()`, `resolve()`, `add_fact()`, `invalidate()`, `assemble_context()`, `relate()`, `episode_count()`, `get_surrealdb_config()`, `find_intro_chain()`
- Helper methods: `find_episode_record()`, `find_fact_record()`, `find_entity_record()`, `find_entity_record_by_id()`, `namespace_for_scope()`, `is_scope_allowed()`, `enforce_rate_limit()`, `project_for_source_episode()`, `build_graph_insights_batched()`

- [ ] **Step 4: Unit tests for ExplanationService**

Test: explain with single context item, explain with missing episode, explain with provenance collection.

- [ ] **Step 5: Verify all tests pass and commit**

```bash
cargo test --no-fail-fast 2>&1 | tail -20
git add -A && git commit -m "refactor: extract ExplanationService, finalize MemoryService as thin facade"
```

---

### Task 7: Decompose MemoryMcp into Focused App Modules

**Files:**
- Create: `src/mcp/apps/inspector.rs`
- Create: `src/mcp/apps/diff.rs`
- Create: `src/mcp/apps/ingestion_review.rs`
- Create: `src/mcp/apps/lifecycle.rs`
- Create: `src/mcp/apps/graph.rs`
- Create: `src/mcp/session.rs`
- Create: `src/mcp/response.rs`
- Modify: `src/mcp/handlers.rs` (shrinks to routing + tool macros)
- Modify: `src/mcp.rs` (add `mod apps`, `mod session`, `mod response`)

**Interfaces:**
- Consumes: App-specific logic currently in `MemoryMcp` impl blocks in `handlers.rs`
- Produces: Pure functions in each app module taking `&MemoryService` and parameters; `SessionManager` struct for session lifecycle; `ToolResponse<T>` in its own file

- [ ] **Step 1: Extract ToolResponse to mcp/response.rs**

Move `ToolResponse<T>`, `OpenAppResult`, `AppCommandResult`, and their constructors (`success_with_guidance`, `complete_list`, etc.) to `src/mcp/response.rs`. Update imports in `handlers.rs`.

- [ ] **Step 2: Extract app modules**

**CRITICAL constraint:** The rmcp `#[tool(description = "...")]` and `#[tool_router]` macros generate code that requires methods to be defined directly on `impl MemoryMcp`. Therefore, ONLY move the helper functions (inspector_payload, diff_payload, lifecycle_dashboard, graph_path_snapshot, etc.) to separate modules. The `#[tool]`-annotated methods MUST stay in `handlers.rs` inside the `#[tool_router] impl MemoryMcp` block — they become thin wrappers that delegate to the extracted functions.

For each app, move the `inspector_payload` / `open_inspector_app` methods from `MemoryMcp` into a module:

`src/mcp/apps/inspector.rs`:
```rust
use crate::service::MemoryService;
use crate::mcp::error::mcp_error;
use rmcp::ErrorData;
use serde_json::{Value, json};

pub async fn inspector_payload(
    service: &MemoryService,
    scope: &str,
    target_type: &str,
    target_id: &str,
    as_of: Option<&str>,
) -> Result<Value, ErrorData> {
    // ... ported method body (lines 529-590 of handlers.rs)
}
```

Repeat for `diff`, `ingestion_review`, `lifecycle`, `graph`. Each module has standalone async functions — no shared state.

- [ ] **Step 3: Extract session management to mcp/session.rs**

Move `AppSessionState`, `next_session_id`, `insert_session`, `session`, `replace_session_payload`, `remove_session`, `create_session`, `enrich_session_payload` into `SessionManager` struct:

```rust
pub struct SessionManager {
    sessions: Arc<tokio::sync::RwLock<HashMap<String, AppSessionState>>>,
    counter: Arc<AtomicU64>,
}
```

`MemoryMcp` holds a `SessionManager` instance and delegates.

- [ ] **Step 4: MemoryMcp handlers.rs → ~400 lines of routing**

After extraction, `handlers.rs` contains:
- `MemoryMcp` struct (service, session_manager, counter, tool_router)
- `ServerHandler` impl (get_info, list_resources, list_resource_templates, read_resource)
- `#[tool_router]` impl with `#[tool]` annotated methods (ingest, extract, explain, resolve, invalidate, assemble_context, open_app, app_command, run_lifecycle_pass)
- Each tool method is ~20 lines: validate → delegate to service → return ToolResponse

- [ ] **Step 5: Verify and commit**

```bash
cargo test --no-fail-fast 2>&1 | tail -20
git add -A && git commit -m "refactor: decompose MemoryMcp into focused app modules, session manager, and response types"
```

---

### Task 8: DDD — Enrich Domain Models with Behavior

**Files:**
- Modify: `src/models.rs` (add methods to Fact, Entity, Episode)
- Modify: `src/service/query.rs` (move decayed_confidence to Fact::decayed_confidence)

**Interfaces:**
- Consumes: Current anemic model structs
- Produces: Models with domain methods; `decayed_confidence` becomes `Fact::decayed_confidence(self, now: DateTime<Utc>) -> f64`

- [ ] **Step 1: Add methods to Fact**

First, move the decay constants from `src/service.rs` (the `constants` inline module at lines 39-51) to `src/models.rs` so they're accessible from the Fact impl block:

```rust
// In src/models.rs, add at top of file after imports:
/// Half-life in days for metric and promise fact confidence decay.
pub const METRIC_HALF_LIFE_DAYS: f64 = 365.0;
/// Half-life in days for general fact confidence decay.
pub const DEFAULT_HALF_LIFE_DAYS: f64 = 180.0;
/// Scaling factor for confidence rounding.
pub const CONFIDENCE_SCALE: f64 = 10000.0;
```

Update `src/service.rs` to re-export from models instead of defining inline:
```rust
pub use crate::models::{METRIC_HALF_LIFE_DAYS, DEFAULT_HALF_LIFE_DAYS, CONFIDENCE_SCALE};
```

Then add `impl Fact` block referencing these constants:

```rust
impl Fact {
    /// Returns true if the fact is active (not invalidated) as of the given timestamp.
    #[must_use]
    pub fn is_active(&self, as_of: DateTime<Utc>) -> bool {
        self.t_invalid.map_or(true, |t| t > as_of)
    }

    /// Calculates confidence decayed by half-life based on fact age.
    #[must_use]
    pub fn decayed_confidence(&self, now: DateTime<Utc>) -> f64 {
        let half_life_days = if self.fact_type == crate::models::FactType::Metric.as_str()
            || self.fact_type == crate::models::FactType::Promise.as_str()
            || self.fact_type == crate::models::FactType::Decision.as_str()
        {
            super::METRIC_HALF_LIFE_DAYS
        } else {
            super::DEFAULT_HALF_LIFE_DAYS
        };
        let delta_days = (now - self.t_valid).num_days().max(0) as f64;
        let decay = 0.5_f64.powf(delta_days / half_life_days);
        (self.confidence * decay * super::CONFIDENCE_SCALE).round() / super::CONFIDENCE_SCALE
    }
}
```

Update callers in `src/service/query.rs` and `src/service/context/` to use `fact.decayed_confidence(now)` instead of `decayed_confidence(&fact, now)`. Keep the free function `decayed_confidence` in `query.rs` as a thin delegator for backward compatibility: `pub fn decayed_confidence(fact: &Fact, now: DateTime<Utc>) -> f64 { fact.decayed_confidence(now) }`.

- [ ] **Step 2: Add methods to Entity**

```rust
impl Entity {
    /// Checks if this entity matches a given name or alias (case-insensitive).
    #[must_use]
    pub fn matches_name(&self, name: &str) -> bool {
        self.canonical_name.to_lowercase() == name.to_lowercase()
            || self.aliases.iter().any(|a| a.to_lowercase() == name.to_lowercase())
    }
}
```

- [ ] **Step 3: Add method to Episode**

```rust
impl Episode {
    /// Returns true if another episode represents the same source material.
    #[must_use]
    pub fn is_duplicate_of(&self, other: &Episode) -> bool {
        self.source_type == other.source_type
            && self.source_id == other.source_id
            && self.scope == other.scope
    }
}
```

- [ ] **Step 4: Verify and commit**

```bash
cargo test --no-fail-fast 2>&1 | tail -20
git add -A && git commit -m "refactor: enrich domain models — Fact::is_active, Fact::decayed_confidence, Entity::matches_name, Episode::is_duplicate_of"
```

---

### Task 9: Structural Cleanup of context.rs

**Files:**
- Modify: `src/service/context.rs` (5936 → ~200 lines)
- Modify: `src/service/context/pipeline.rs` (gain assemble_context wiring from context.rs)

**Interfaces:**
- Consumes: Current `assemble_context()` function and its helper logic
- Produces: `context.rs` as a thin entry point; pipeline logic in `context/pipeline.rs`

- [ ] **Step 1: Move parameter setup and view resolution to pipeline.rs**

Move the large block from `context.rs` lines 59-302 (after `enforce_rate_limit` check through view mode resolution) into `context/pipeline.rs` as a private `prepare_context_params()` function returning a `PreparedContextParams` struct.

- [ ] **Step 2: Move cache logic into a helper function in pipeline.rs**

Move the cache-key construction, cache-hit check, and cache-set logic (lines 144-191 and 365-368) into `check_context_cache()` / `store_context_cache()` helper functions in `context/cache_ops.rs` or inline in pipeline.

- [ ] **Step 3: context.rs becomes an orchestrator**

```rust
pub async fn assemble_context(
    service: &crate::service::MemoryService,
    request: AssembleContextRequest,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let started_at = Instant::now();
    let access = AccessPayload::from_payload(request.access.clone());

    log_context_start(service, &request, access.as_ref());
    service.enforce_rate_limit(access.as_ref())?;
    validate_scope(&request.scope)?;

    let params = pipeline::prepare_context_params(service, &request, access.as_ref()).await?;

    if let Some(cached) = pipeline::check_cache(service, &params).await {
        return Ok(cached);
    }

    let mut results = pipeline::assemble_default_context(service, &params).await?;
    pipeline::maybe_append_experience(service, &mut results, &params).await?;

    pipeline::store_cache(service, &params, &results).await;
    pipeline::log_and_track(service, &request, &results, &params, started_at).await;

    Ok(results)
}
```

- [ ] **Step 4: Verify and commit**

```bash
cargo test --no-fail-fast 2>&1 | tail -20
git add -A && git commit -m "refactor: shrink context.rs from 5936 to ~200 lines via pipeline extraction"
```

---

### Task 10: Simplify builder.rs Startup Logic

**Files:**
- Modify: `src/service/core/builder.rs` (579 → ~200 lines)
- Modify: `src/service/startup.rs` (gain embedding startup decision wiring)

**Interfaces:**
- Consumes: Complex `new_from_env_with_mode()` with nested embedding startup logic and lifecycle spawning
- Produces: Builder delegates to startup.rs for embedding decision, lifecycle.rs for worker spawning

- [ ] **Step 1: Extract embedding startup orchestration to startup.rs**

Move the 80 lines of embedding preflight → target resolution → decision making (lines 133-280 of builder.rs) into `startup::resolve_embedding_startup()`:

```rust
pub async fn resolve_embedding_startup(
    config: &EmbeddingConfig,
    db_client: &Arc<dyn DbClient>,
    namespaces: &[String],
    data_dir: &str,
    mode: EmbeddingActivationMode,
    startup_logger: &StdoutLogger,
) -> Result<(EmbeddingStartupDecision, Option<ResolvedEmbeddingTarget>), MemoryError> {
    // ... all the logic from builder.rs lines 133-280
}
```

- [ ] **Step 2: Extract lifecycle worker spawning**

Move lines 383-428 of builder.rs into `lifecycle::spawn_workers_from_config()`:

```rust
pub fn spawn_workers_from_config(
    service: &MemoryService,
    config: &LifecycleConfig,
) {
    if !config.enabled { return; }
    let _decay_handle = spawn_decay_worker(service.clone(), ...);
    let _archival_handle = spawn_archival_worker(service.clone(), ...);
    let _community_handle = spawn_community_worker(service.clone(), ...);
}
```

- [ ] **Step 3: builder.rs new_from_env_with_mode() → clean pipeline**

```rust
pub(crate) async fn new_from_env_with_mode(mode: EmbeddingActivationMode) -> Result<Self, MemoryError> {
    let config = SurrealConfig::from_env()?;
    let default_namespace = config.default_namespace()
        .ok_or_else(|| MemoryError::ConfigInvalid("namespaces cannot be empty".into()))?;

    log_startup_info(&config);
    let db_client = connect_and_migrate(&config, default_namespace).await?;
    let db_client = Arc::new(db_client);

    let (decision, target) = startup::resolve_embedding_startup(
        &config.embedding, &db_client, &config.namespaces,
        &config.data_dir_or_default(), mode, &startup_logger,
    ).await?;

    let embedding_provider = startup::build_embedding_provider(&config, &target, &decision, mode).await?;
    let entity_extractor = create_entity_extractor(&config.ner, &config.data_dir_or_default(), &startup_logger).await?;

    let mut service = Self::build_service(...)?;

    lifecycle::spawn_workers_from_config(&service, &config.lifecycle);
    service.check_surrealdb_connection().await?;

    Ok(service)
}
```

- [ ] **Step 4: Verify and commit**

```bash
cargo test --no-fail-fast 2>&1 | tail -20
git add -A && git commit -m "refactor: simplify builder.rs — extract embedding startup and lifecycle spawning to domain modules"
```

---

### Task 11: Reorganize util Module by Domain

**Files:**
- Modify: `src/service/util.rs` (shrinks — keep `RateLimiter`, remove re-exports for moved items)
- Move: `util/ids.rs` content → add as `pub(crate)` functions in relevant model files or keep as `service/ids.rs`
- Move: `util/validation.rs` → split into `service/ingestion/validation.rs`, `service/fact/validation.rs`, `service/entity/validation.rs` (or keep consolidated as `service/domain/validation.rs`)
- Move: `util/statement_detection.rs` → `service/ingest/statement_detection.rs`
- Move: `util/rate_limit.rs` → `service/rate_limiter.rs` (rename from `util/rate_limit`)

**Interfaces:**
- Consumes: Current scattered utility functions
- Produces: Utils near their consumers

- [ ] **Step 1: Move statement_detection to ingest module**

Move `src/service/util/statement_detection.rs` → `src/service/ingest/statement_detection.rs`. Update imports in `src/service/episode.rs` (test module imports) and `src/service/ingest.rs`.

- [ ] **Step 2: Elevate rate_limiter to service level**

Rename `src/service/util/rate_limit.rs` → `src/service/rate_limiter.rs`. Update all imports — mostly in `service/core/builder.rs`.

- [ ] **Step 3: Keep ids and validation in util/ with clear docs**

`util/ids.rs` stays — ID generation is cross-cutting and used by multiple services. Add doc comment: "Deterministic ID generation functions used by multiple domain services."

`util/validation.rs` stays but clarify that it contains shared validation for ingest requests, entity candidates, and fact input — used by IngestionService, EntityService, and FactService.

- [ ] **Step 4: Verify and commit**

```bash
cargo test --no-fail-fast 2>&1 | tail -20
git add -A && git commit -m "refactor: reorganize util module — statement_detection to ingest/, rate_limiter to service/"
```

---

## Testing Decisions

- Each domain service (IngestionService, EntityService, FactService, EmbeddingService, ExplanationService) has unit tests in its own file using `MockDbClient`
- Tests test external behavior: given input X, expect output Y or error Z
- Tests do NOT test logging output or internal implementation details
- Existing integration tests in `tests/` directory continue to pass — they test end-to-end flows that remain unchanged
- New tests follow the pattern: create MockDbClient with configured responses → create service → call method → assert result
- Prior art: the existing tests in `src/service/episode.rs` establish the testing pattern used in this codebase (tokio async tests, Arc<dyn DbClient>, serde_json::Value)

## Out of Scope

- **No DbClient trait splitting.** The 25-method trait stays as-is. It's tested, it works, and MockDbClient makes test ergonomics sufficient.
- **No changes to `src/storage/queries.rs` SQL builders.** They are well-structured and self-contained.
- **No changes to `src/storage/migrations.rs` or `migrations/` directory.**
- **No changes to `src/config/` module.** Configuration is clean.
- **No changes to `src/service/reembed.rs` (1690 lines).** This is a maintenance CLI workflow with its own internal structure. It could be a future refactoring target but is out of scope here.
- **No changes to `src/mcp/error.rs`** beyond mechanical AccessContext → AccessPayload renames in Task 1.
- **No changes to `tests/` integration test files** beyond replacing hand-written DbClient mocks with MockDbClient.
- **No new features.** This is a pure refactoring — behavior must be identical before and after.
- **No changes to MCP tool schemas.** External API surface preserved.
- **No changes to `src/service/context/` submodules** beyond the structural cleanup of `context.rs` itself.
- **No async runtime changes.** Tokio configuration unchanged.

## Compilation Constraint

Tasks 4, 5, and 6 all modify `src/service/core/builder.rs` to add service fields. Each task MUST leave the codebase compiling and all tests passing. Execute these tasks strictly in order — do not parallelize. After each task, run `cargo check` and `cargo test --no-fail-fast` before proceeding to the next.
