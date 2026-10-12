# SurrealDB Query Performance (explain timeouts + idle churn) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `explain` stop timing out and stop driving the SurrealDB container to 100%+ CPU / heavy disk reads by making record-equality filters index-safe (`type::record` bindings), splitting one nested-subquery join into bounded indexed lookups, adding two missing lookup indexes, capping blind timeout retries, and documenting native runtime guardrails.

**Architecture:** Query-shape fixes live in the owning contexts (`knowledge/`, `memory/`) since SurrealDB 3.3.0's planner folds `<record> $param` casts too late for index selection — the fix is to bind table and key as two separate strings and construct the record in-query via `type::record($table, $key)` (proven by EXPLAIN matrix, Appendix A). One migration adds the two missing lookup indexes (`episode.episode_id`, `fact.source_episode`); `fact.fact_id` already carries the `fact_claim_backfill_cursor_idx` index from migration 029. The platform layer decouples timeout retries from transient-conflict retries so an overloaded query fails fast instead of amplifying load. Ops guardrails (native `--query-timeout`, background-interval relaxation, port unexposure, credential rotation) are delivered as an operator runbook — prod changes are documented, not executed, by this plan.

**Tech Stack:** Rust 2021, SurrealDB 3.3.0 CE (embedded `mem://` in tests; Docker/RocksDB in prod), tokio, serde_json, thiserror.

**Spec:** `docs/BACKLOG.md:80-97` (the report + explicit ask: analyze, find root cause, produce a fix plan). There is no separate design doc; Appendices A and B of this plan embed the verified root-cause evidence and the native-feature research this plan argues from and are the binding context.

## Global Constraints

- Workspace `rust-version` 1.99.0 — no toolchain, dependency, `Cargo.toml`, Dockerfile or CI-pin changes (AGENTS.md).
- No new MCP tools (8-tool surface frozen); no changes needed in `src/mcp/`, `src/http/`, `src/control/`, `main.rs`, or `src/service/`.
- Shared helpers live in `src/shared/` (alongside `temporal.rs`); no business logic in the storage platform; no caller-supplied table names (keep using `KnowledgeTables`/`MemoryTables` owner scopes where the code already does).
- Never delete facts; only reads and additive schema change in this plan.
- No `unwrap()` in production code; `MemoryError`-based errors; logging through existing facades with existing `db.<op>.retry|timeout` op names.
- Bi-temporal semantics unchanged: `BI_TEMPORAL_WHERE`, the `relation = 'involved_in'` filter, cutoff handling and `ORDER BY t_ref DESC LIMIT 10` semantics must be preserved exactly.
- Record ids split on the **first** colon only; never interpolate record ids into SQL text (Tasks 1-2 replace interpolation-adjacent casts with bound parts).
- Tests: run the narrowest set — `cargo test -p memory_mcp <name-filter>` or `cargo test -p memory_mcp --test <target> [name]`. Full `-p memory_mcp` run only in Task 7. Embedded-SurrealDB integration tests are flaky in parallel (LOCK file contention): run new integration targets with `-- --test-threads=1` (precedent: header note in `tests/explain_provenance.rs`).
- Before shipping: `cargo fmt --all --check` (zero diff) and `cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings` (zero warnings). The user explicitly required zero warnings (e.g. the `query_cache.rs` dead-code warning class — do not add any new unused public methods).
- Repo hygiene: do NOT push; do NOT stage or commit the untracked `docs/superpowers/plans/2026-10-08-logging-quality-correlation-and-coverage.md`; never commit `observability/.tmp-trackly/`.
- Prod server (`ssh -p 2288 root@82.165.149.36`) is read-only evidence-gathering for the runbook author only; never echo DB passwords or API keys anywhere.

## Review Focus

Input classes the spec implies but task tests under-capture — each is pinned by the named task test:

1. **Odd record keys** — a record id whose key itself contains further colons or non-ASCII characters (`entity:odd:key`) must resolve to exactly the same rows with type::record($table, $key) as the old `<record>` cast did — Task 1 unit `split_record_id_splits_on_first_colon_and_keeps_remainder`.
2. **Malformed ids** (no colon, empty table or key) — the rewrite must leave these on the legacy `<record>` cast path unchanged: the builders still emit the cast (Tasks 1-2) and the Task 3 runtime path returns exactly what the old single query returned — Task 1 unit `neighbors_query_falls_back_to_cast_for_unsplit_id`, Task 2 unit `triple_lookup_falls_back_to_cast_for_unsplit_ids`, Task 3 unit `unsplit_entity_id_returns_legacy_single_query`.
3. **Empty graph paths** — entity with no `involved_in` edges must return no episodes without issuing the episode-stage query — Task 3 units `entity_without_edges_returns_empty` / `fact_without_source_episode_returns_empty`.
4. **Failure-surfacing behavior change** — a truly stalled read now returns after 2 timeout attempts (~60 s + 200 ms) instead of 3 (~90 s), and the storage error text keeps the `timed out` marker callers/logs match on — Task 5 units `stalled_attempt_fails_after_two_timeout_attempts`, `timeout_then_success_returns_value`.
5. **Index backfill on populated namespaces** — the new indexes must exist after startup on a namespace that already holds rows (migrations are replayed and `DEFINE INDEX` builds over existing data before the store serves; prod's current dataset is ~30 episodes / 170 facts so backfill is immediate, but larger namespaces pay the build time once at startup — the runbook notes this) — Task 4 integration `context_lookup_indexes_registered_by_migrations` asserts `INFO FOR TABLE` after `apply_migrations`.

---
### Task 1: Shared `split_record_id` helper + index-safe neighbor builders

**Files:**
- Create: `crates/memory-mcp/src/shared/record.rs`
- Modify: `crates/memory-mcp/src/shared.rs` (register the module next to `pub mod temporal;`)
- Modify: `crates/memory-mcp/src/knowledge/queries.rs` (`build_select_edge_neighbors_query` L213-231, `build_select_graph_edge_neighbors_query` L235-251; unit tests go in the existing `mod tests`)
- Test: `crates/memory-mcp/tests/graph_neighbors_record_binding.rs` (new)

**Interfaces:**
- Consumes: existing `(String, Value)` builder convention; `GraphDirection` (re-exported at `memory_mcp::storage`); `BI_TEMPORAL_WHERE` (its `$cutoff` var stays).
- Produces: `pub(crate) fn split_record_id(record_id: &str) -> Option<(&str, &str)>` in `crate::shared::record` — the only splitting rule in this plan; Tasks 2 and 3 consume it by path. Builder signatures unchanged. When the id splits, vars use `node_table`/`node_key`; when it does not, vars use the legacy `node_id` and the SQL keeps the `<record>` cast.

Context for the implementer (already decided, do not re-litigate): on SurrealDB 3.3.0 a `<record> $param` cast folds to a literal *after* index selection, so the planner emits TableScan for `edge`; `type::record($table, $key)` with two string params produces IndexScan at plan time and matches the same rows at runtime (Appendix A matrix, verified on the v3.3.0 image). `WITH INDEX` parses but is ignored; `USE`/`FORCE INDEX` are parse errors — hints are a dead end. `type::thing` does not exist on 3.3.0 (the parser suggests `type::record`).


- [ ] **Step 1: Write failing unit tests for the two neighbor builders**

In each block below, match the existing `tests`-mod import style of the file.

```rust
#[test]
fn edge_neighbors_binds_record_parts_not_cast() {
    let (sql, vars) = build_select_edge_neighbors_query(
        "entity:abc-123", "2026-01-01T00:00:00Z", GraphDirection::Outgoing);
    assert!(sql.contains("= type::record($node_table, $node_key)"), "{sql}");
    assert!(!sql.contains("<record>"), "cast must be gone: {sql}");
    assert_eq!(vars["node_table"], "entity");
    assert_eq!(vars["node_key"], "abc-123");
}
```

```rust
#[test]
fn graph_edge_neighbors_binds_record_parts() {
    let (sql, vars) = build_select_graph_edge_neighbors_query(
        "fact:x9", "2026-01-01T00:00:00Z", GraphDirection::Incoming);
    assert!(sql.contains("= type::record($node_table, $node_key)"), "{sql}");
    assert!(!sql.contains("<record>"), "cast must be gone: {sql}");
    assert_eq!(vars["node_table"], "fact");
    assert_eq!(vars["node_key"], "x9");
}
```

```rust
#[test]
fn neighbors_query_falls_back_to_cast_for_unsplit_id() {
    let (sql, vars) = build_select_edge_neighbors_query(
        "noid", "2026-01-01T00:00:00Z", GraphDirection::Incoming);
    assert!(sql.contains("<record> $node_id"), "{sql}");
    assert_eq!(vars["node_id"], "noid");
}
```

The first two tests pin the new contract; the third pins the Review Focus 2 fallback.

- [ ] **Step 2: Run the unit tests to verify they fail**

Run: cargo test -p memory_mcp --lib knowledge::queries
Expected: the two cast-absence tests FAIL on their assertions; the fallback test already passes (it pins the shape that stays as fallback). That is the RED signal.

- [ ] **Step 3: Create the `split_record_id` helper**

Create `crates/memory-mcp/src/shared/record.rs`, whose doc comment states the invariant (split on the FIRST colon; empty parts and a missing colon are not record ids) and `pub(crate) fn split_record_id(record_id: &str) -> Option<(&str, &str)>`, plus its own `#[cfg(test)] mod tests` with `split_record_id_splits_on_first_colon_and_keeps_remainder` (Review Focus 1): asserts `("entity","odd:key")` from `entity:odd:key`, `("t","ab")` from `t:ab`, and `None` from `bare`, `:x`, `x:`. Register `pub mod record;` next to `pub mod temporal;` in `shared.rs`.

- [ ] **Step 4: Rewrite the two neighbor builders in `knowledge/queries.rs`**

In `build_select_edge_neighbors_query` and `build_select_graph_edge_neighbors_query`, replace the `{node_field} = <record> $node_id` predicate: split `node_id` with `crate::shared::record::split_record_id`; when it splits, emit `{node_field} = type::record($node_table, $node_key)` and bind `node_table`/`node_key`; when it does not, keep the current `{node_field} = <record> $node_id` and bind `node_id` (fallback). The `BI_TEMPORAL_WHERE` clause, SELECT projection, node_field direction match and ORDER BY stay byte-identical.

- [ ] **Step 5: Run the unit tests to verify they pass**

Run: cargo test -p memory_mcp --lib knowledge::queries
Expected: PASS for the three new tests and all pre-existing builder tests.

- [ ] **Step 6: Write the index-plan integration test**

Create `crates/memory-mcp/tests/graph_neighbors_record_binding.rs`:

```rust
mod common;

use chrono::{Duration, Utc};
use memory_mcp::knowledge::queries::build_select_edge_neighbors_query;
use memory_mcp::models::EdgeAttributes;
use memory_mcp::storage::{DbClient, GraphDirection};
```

```rust
#[tokio::test]
async fn neighbors_resolve_through_type_record_binding() {
    let (service, db_client) = common::make_service_with_client().await;
    let alice = memory_mcp::service::deterministic_entity_id("person", "Alice Ann");
    let bob = memory_mcp::service::deterministic_entity_id("person", "Bob Ben");
    common::seed_entity(&db_client, "org", &alice, "person", "Alice Ann", &[]).await;
    common::seed_entity(&db_client, "org", &bob, "person", "Bob Ben", &[]).await;
    service
        .relate(&alice, "knows", &bob, EdgeAttributes::inferred())
        .await
        .expect("relate alice to bob");
    let cutoff = (Utc::now() + Duration::hours(1)).to_rfc3339();
    let store = memory_mcp::knowledge::KnowledgeGraphStore::new(db_client, "org");
    let rows = store
        .select_edge_neighbors(&alice, &cutoff, GraphDirection::Outgoing)
        .await
        .expect("outgoing neighbors");
    assert!(rows
        .iter().any(|r| r["out"].to_string().contains(&bob)), "{rows:?}");
}
```

```rust
#[tokio::test]
async fn neighbor_query_plan_is_index_scan_on_edge_in() {
    let (_service, db_client) = common::make_service_with_client().await;
    let cutoff = (Utc::now() + Duration::hours(1)).to_rfc3339();
    let (sql, vars) = build_select_edge_neighbors_query(
        "entity:planner-probe", &cutoff, GraphDirection::Outgoing);
    let plan = db_client
        .query(&format!("EXPLAIN {sql}"), Some(vars), "org")
        .await
        .expect("explain");
    let plan = plan.to_string();
    // Ruling: the plan tree is asserted as substrings of the serialized value
    // because its exact shape is version-specific; this pair is the
    // observable index-usage signal (verified on the v3.3.0 image).
    assert!(plan.contains("IndexScan"), "{plan}");
    assert!(plan.contains("edge_in"), "{plan}");
}
```

- [ ] **Step 7: Run the integration test**

Run: cargo test -p memory_mcp --test graph_neighbors_record_binding -- --test-threads=1
Expected: PASS (both tests). If the EXPLAIN test reports TableScan, the binding rewrite silently regressed - fix the builder, do not weaken the assertion.

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git add crates/memory-mcp/src/shared.rs crates/memory-mcp/src/shared/record.rs \
  crates/memory-mcp/src/knowledge/queries.rs \
  crates/memory-mcp/tests/graph_neighbors_record_binding.rs
git commit -m "fix(knowledge): bind neighbor lookups via type::record to keep index scans"
```

---

### Task 2: Triple-dedup lookup binds record parts instead of casts

**Files:**
- Modify: crates/memory-mcp/src/knowledge/queries.rs (new builder + unit tests)
- Modify: crates/memory-mcp/src/knowledge/graph_store.rs (`select_edges_for_triple` L226-236 -> delegate)
- Test: crates/memory-mcp/tests/graph_neighbors_record_binding.rs (append one runtime test)

**Interfaces:**
- Consumes: `split_record_id` from Task 1 (path `crate::shared::record::split_record_id`).
- Produces: `pub fn build_select_edges_for_triple_query(in_id: &str, relation: &str, out_id: &str) -> (String, Value)` in `knowledge/queries.rs`; `KnowledgeGraphStore::select_edges_for_triple(&self, in_id, relation, out_id) -> Result<Vec<Value>, MemoryError>` signature unchanged; existing callers keep calling it untouched.

Same planner pathology as Task 1, at the highest-frequency call site: every
fact write probes for an existing exactly-identical edge, twice per write
through the same casts, each answered with a TableScan of `edge`.

- [ ] **Step 1: Write the failing unit tests**

Append to the same `mod tests`:

```rust
#[test]
fn triple_lookup_binds_record_parts_for_both_ends() {
    let (sql, vars) = build_select_edges_for_triple_query(
        "entity:trip-a", "knows", "entity:trip-c");
    assert!(sql.contains("in = type::record($in_table, $in_key)"), "{sql}");
    assert!(sql.contains("out = type::record($out_table, $out_key)"), "{sql}");
    assert!(!sql.contains("<record>"), "{sql}");
    assert_eq!(vars["relation"], "knows");
    assert_eq!(vars["out_key"], "trip-c");
}
```

```rust
#[test]
fn triple_lookup_falls_back_to_cast_for_unsplit_ids() {
    let (sql, vars) = build_select_edges_for_triple_query(
        "noid", "knows", "entity:trip-c");
    assert!(sql.contains("in = <record> $in_id"), "{sql}");
    assert_eq!(vars["in_id"], "noid");
}

```

- [ ] **Step 2: Run the unit tests to verify they fail**

Run: cargo test -p memory_mcp --lib knowledge::queries
Expected: COMPILE ERROR - `build_select_edges_for_triple_query` not found. That is the RED signal.

- [ ] **Step 3: Implement the builder and delegate from graph_store**

Add `pub fn build_select_edges_for_triple_query(in_id: &str, relation: &str, out_id: &str) -> (String, Value)` to `knowledge/queries.rs`: when both ids split, bind `in_table`/`in_key`/`relation`/`out_table`/`out_key` and emit `SELECT * FROM edge WHERE in = type::record($in_table, $in_key) AND relation = $relation AND out = type::record($out_table, $out_key)`; when either side is unsplit, return today's two-cast SQL verbatim.

In `graph_store.rs`, `select_edges_for_triple` becomes three lines: call the builder, then `self.db.query_rows(&sql, Some(vars)).await`. The doc comment stays.

- [ ] **Step 4: Run the unit tests to verify they pass**

Run: cargo test -p memory_mcp --lib knowledge::queries
Expected: PASS (two new triple tests + the neighbor builder tests from Task 1, which share this file).

- [ ] **Step 5: Append the Runtime regression test to the integration target**

Append to `crates/memory-mcp/tests/graph_neighbors_record_binding.rs`:

```rust
#[tokio::test]
async fn triple_lookup_finds_only_the_matching_edge() {
    let (service, db_client) = common::make_service_with_client().await;
    common::seed_entity(&db_client, "org", "entity:trip-a", "person", "Trip Ada", &[]).await;
    common::seed_entity(&db_client, "org", "entity:trip-c", "person", "Trip Cal", &[]).await;
    service.relate("entity:trip-a", "knows", "entity:trip-c", EdgeAttributes::inferred()).await.expect("knows edge");
    service.relate("entity:trip-a", "owns", "entity:trip-c", EdgeAttributes::inferred()).await.expect("owns edge");
    let store = memory_mcp::knowledge::KnowledgeGraphStore::new(db_client, "org");
    let hits = store.select_edges_for_triple("entity:trip-a", "knows", "entity:trip-c").await.expect("dedup lookup");
    assert_eq!(hits.len(), 1, "dedup lookup must return only the matching edge: {hits:?}");
    assert_eq!(hits[0]["relation"], "knows");
}
```

Run cargo test -p memory_mcp --test graph_neighbors_record_binding -- --test-threads=1
Expected: PASS (all tests in the file). If the count moved, the append broke compilation.

- [ ] **Step 6: Format, lint the touched files, commit**

```bash
cargo fmt --all
git add crates/memory-mcp/src/knowledge/queries.rs crates/memory-mcp/src/knowledge/graph_store.rs crates/memory-mcp/tests/graph_neighbors_record_binding.rs
git commit -m "fix(knowledge): bind triple dedup lookup via type::record to keep index scans"
```

---

### Task 3: Split the episode-via-entity join into three bounded indexed lookups

**Files:**
- Modify: crates/memory-mcp/src/memory/queries.rs (three new builders + unit tests)
- Modify: crates/memory-mcp/src/memory/episode_context_store.rs (`select_episodes_via_entity` L61-73)
- Test: crates/memory-mcp/tests/embedded_episode_via_entity.rs (new)

**Interfaces:**
- Consumes: `crate::shared::record::split_record_id` (Task 1).
- Produces: `select_episodes_via_entity(&self, entity_id: &str) -> Result<Vec<Value>, MemoryError>` unchanged (full episode rows, t_ref DESC, LIMIT 10; caller `find_episodes_via_entity` untouched). New `pub fn` builders in `memory/queries.rs` (the `tests` mod is created here; `memory/queries.rs` has none yet):
  - `build_select_fact_ids_via_entity_query(entity_table: &str, entity_key: &str) -> (String, Value)`: SELECT type::string(out) AS fact_id FROM edge WHERE in = type::record($entity_table, $entity_key) AND relation = 'involved_in'
  - `build_select_source_episodes_via_facts_query(fact_ids: &[String]) -> (String, Value)`: SELECT source_episode FROM fact WHERE fact_id INSIDE $fact_ids
  - `build_select_episodes_by_ids_query(episode_ids: &[Value]) -> (String, Value)`: SELECT * FROM episode WHERE episode_id INSIDE $episode_ids ORDER BY t_ref DESC LIMIT 10
- The `EpisodeContextStore` re-export already exists at `src/memory.rs:39`; integration tests build `EpisodeContextStore::new(db_client, "org")`.

The current method is one query nesting two `IN (subquery)` layers (Appendix A: the v3.3.0 planner answers nested IN with a TableScan of the outer table even when inner predicates are indexable) and a `<record>` cast inside (same pathology as Task 1). New shape: stage 1 resolves fact ids from edge, stage 2 resolves episode ids from fact, stage 3 reads episodes by id list. Stages 2-3 bind an array param and use INSIDE; stage 2 is already index-backed by `fact_claim_backfill_cursor_idx`, stage 3 gets its `episode.episode_id` index in Task 4.

- [ ] **Step 1: Freeze current behavior with characterization guards**

This task preserves behavior, so its integration tests characterize the contract and must PASS against the current impl before any change (recorded baseline). Create `crates/memory-mcp/tests/embedded_episode_via_entity.rs`:

```rust
mod common;

use chrono::{Duration, Utc};
use memory_mcp::memory::EpisodeContextStore;
use memory_mcp::models::{IngestRequest, Provenance};
use memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability;
use memory_mcp::storage::DbClient;
```

```rust
async fn seed_linked_episode(
    service: &memory_mcp::service::MemoryService,
    entity: &str,
    source: &str,
    text: &str,
    t: chrono::DateTime<Utc>,
) -> String {
    let episode_id = IngestCapability::ingest_from_service(
        service,
        IngestRequest { source_type: "email".into(), source_id: source.into(), content: text.into(), t_ref: t, t_ingested: None, policy_tags: vec![] },
        None,
    ).await.expect("seed episode");
    service.add_fact("metric", text, text, &episode_id, t, 0.9,
        vec![entity.to_string()], vec![], Provenance::agent_observation(&episode_id))
        .await.expect("seed linked fact");
    episode_id
}
```

```rust
#[tokio::test]
async fn linked_episode_returns_via_shared_entity() {
    let (service, db_client) = common::make_service_with_client().await;
    let entity = memory_mcp::service::deterministic_entity_id("person", "Alice Smith");
    let ep = seed_linked_episode(&service, &entity, "ep-a", "Alice closed a deal", Utc::now()).await;
    let store = EpisodeContextStore::new(db_client, "org");
    let rows = store.select_episodes_via_entity(&entity).await.expect("entity lookup");
    let ids: Vec<&str> = rows.iter().filter_map(|r| r["episode_id"].as_str()).collect();
    assert!(ids.contains(&ep.as_str()), "linked episode must appear: {ids:?}");
}
```

```rust
#[tokio::test]
async fn entity_without_edges_returns_empty() {
    let (service, db_client) = common::make_service_with_client().await;
    common::seed_entity(&db_client, "org", "entity:ghost-99", "person", "Ghost Person", &[]).await;
    let store = EpisodeContextStore::new(db_client, "org");
    let rows = store.select_episodes_via_entity("entity:ghost-99").await.expect("empty is not an error");
    assert!(rows.is_empty(), "no edges must yield no rows: {rows:?}");
}
```

```rust
#[tokio::test]
async fn recency_order_and_limit_preserved() {
    let (service, db_client) = common::make_service_with_client().await;
    let entity = memory_mcp::service::deterministic_entity_id("person", "Busy Bob");
    let base = Utc::now() - Duration::hours(24);
    let mut wanted = Vec::new();
    for i in 0..12 {
        let t = base + Duration::minutes(i);
        wanted.push(seed_linked_episode(&service, &entity, &format!("ep-busy-{i}"), &format!("note {i}"), t).await);
    }
    let store = EpisodeContextStore::new(db_client, "org");
    let rows = store.select_episodes_via_entity(&entity).await.expect("recency query");
    let ids: Vec<&str> = rows.iter().filter_map(|r| r["episode_id"].as_str()).collect();
    assert_eq!(ids.len(), 10, "LIMIT 10 must hold");
    assert_eq!(ids, wanted[2..].iter().rev().map(String::as_str).collect::<Vec<_>>(), "newest-first order");
}
```

```rust
#[tokio::test]
async fn fact_without_source_episode_returns_empty() {
    let (service, db_client) = common::make_service_with_client().await;
    let entity = memory_mcp::service::deterministic_entity_id("person", "Orphan Fact");
    seed_linked_episode(&service, &entity, "ep-orphan", "orphan note", Utc::now()).await;
    let n = db_client.query("UPDATE fact SET source_episode = NONE", None, "org").await.expect("strip");
    drop(n);
    let store = EpisodeContextStore::new(db_client, "org");
    let rows = store.select_episodes_via_entity(&entity).await.expect("empty is not an error");
    assert!(rows.is_empty(), "facts without source_episode must yield no episodes: {rows:?}");
}
```

```rust
#[tokio::test]
async fn unsplit_entity_id_returns_legacy_single_query() {
    let (_service, db_client) = common::make_service_with_client().await;
    let store = EpisodeContextStore::new(db_client, "org");
    let result = store.select_episodes_via_entity("alice-without-colon").await;
    assert!(result.is_err(), "unsplit id keeps today's cast error");
}
```

Run: cargo test -p memory_mcp --test embedded_episode_via_entity -- --test-threads=1
Expected: PASS, 5 tests, against the CURRENT single-query implementation. If any test fails now, stop and fix the test to capture actual current behavior before continuing.

- [ ] **Step 2: Write failing unit tests for the three stage builders**

Add a `#[cfg(test)] mod tests` to `crates/memory-mcp/src/memory/queries.rs`:

```rust
#[test]
fn fact_ids_via_entity_binds_type_record_parts() {
    let (sql, vars) = build_select_fact_ids_via_entity_query("entity", "abc-123");
    assert!(sql.contains("in = type::record($entity_table, $entity_key)"), "{sql}");
    assert!(sql.contains("relation = 'involved_in'"), "{sql}");
    assert!(!sql.contains("<record>"), "{sql}");
    assert_eq!(vars["entity_key"], "abc-123");
}
```

```rust
#[test]
fn source_episodes_via_facts_binds_id_array() {
    let ids = vec!["fact:a".to_string(), "fact:b".to_string()];
    let (sql, vars) = build_select_source_episodes_via_facts_query(&ids);
    assert!(sql.contains("fact_id INSIDE $fact_ids"), "{sql}");
    assert_eq!(vars["fact_ids"][1], "fact:b");
}

#[test]
fn episodes_by_ids_binds_raw_values_and_keeps_order_clause() {
    let ids = vec![serde_json::json!("episode:1"), serde_json::json!("episode:2")];
    let (sql, vars) = build_select_episodes_by_ids_query(&ids);
    assert!(sql.contains("WHERE episode_id INSIDE $episode_ids"), "{sql}");
    assert!(sql.contains("ORDER BY t_ref DESC"), "{sql}");
    assert!(sql.contains("LIMIT 10"), "{sql}");
    assert_eq!(vars["episode_ids"], serde_json::json!(["episode:1", "episode:2"]));
}
```

The `mod tests` body starts with `use super::*;`.

Run: cargo test -p memory_mcp --lib memory::queries::tests
Expected: COMPILE ERROR - three builders not found. That is the RED signal.

- [ ] **Step 3: Implement the three builders**

Same file, `pub fn` returning `(String, Value)` with `json!` vars exactly per the Interfaces block; no interpolation; plain static SQL strings.

- [ ] **Step 4: Run the unit tests to verify they pass**

Run: cargo test -p memory_mcp --lib memory::queries::tests
Expected: PASS (3 tests).

- [ ] **Step 5: Rewrite `select_episodes_via_entity` as the staged flow**

In `episode_context_store.rs`, keep the public signature. Body: if `split_record_id(entity_id)` is None, run the current legacy SQL string verbatim in that branch (comment: "legacy path for unsplit ids; callers pass record ids"). Otherwise: stage 1 query_rows, collect strings from `row["fact_id"]`, return Ok(vec![]) when empty; stage 2 query_rows, collect raw `Value`s from `row["source_episode"]` skipping missing/null, return Ok(vec![]) when empty; stage 3 returns `self.db.query_rows(...)` unchanged. Keep the method doc comment; extend it with one line explaining the staged shape and citing Appendix A.

- [ ] **Step 6: Run the characterization suite again**

Run: cargo test -p memory_mcp --test embedded_episode_via_entity -- --test-threads=1
Expected: PASS, 5 tests - identical results to the Step 1 baseline. If an assertion flips, the rewrite changed behavior: fix the rewrite, not the test.

- [ ] **Step 7: Format, lint the touched files, commit**

```bash
cargo fmt --all
cargo clippy -p memory_mcp --lib --locked -- -D warnings
git add crates/memory-mcp/src/memory/queries.rs crates/memory-mcp/src/memory/episode_context_store.rs crates/memory-mcp/tests/embedded_episode_via_entity.rs
git commit -m "perf(memory): split episode-via-entity join into bounded indexed lookups"
```

---

### Task 4: Add the two context-lookup indexes as migration 053

**Files:**
- Create: crates/memory-mcp/migrations/053_context_lookup_indexes.surql
- Modify: crates/memory-mcp/src/storage/migrations.rs (append one MigrationScript entry at the end of the versioned_migrations() array, after 039_filesystem_ingestion)
- Test: crates/memory-mcp/tests/context_lookup_indexes.rs (new)

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: indexes `episode_episode_id` (episode.episode_id) and `fact_source_episode` (fact.source_episode) present in every namespace after startup. `fact.fact_id` needs no new index: migration 029 already defines `fact_claim_backfill_cursor_idx ON fact COLUMNS fact_id`, so Task 3 stage 2 is index-backed today. One registration point is enough: the HTTP tenant runner (http/leases/migration.rs L206-221) runs the base `apply_migrations_impl` (which replays `versioned_migrations()`) before its own 040-045 scripts, so both the local stdio profile and HTTP tenants get the indexes from the storage catalog entry; `CURRENT_SCHEMA_VERSION` is a label returned unchanged and needs no bump.

Why these two: stage 3 of Task 3 answers `episode_id INSIDE $episode_ids` and needs the `episode.episode_id` index, and stage 2's `fact_id INSIDE $fact_ids` is already covered by `fact_claim_backfill_cursor_idx` (migration 029). `fact.source_episode` carries the fact-to-episode provenance link every explain follows item-by-item; indexing it removes the per-item fact scans named in Appendix A RC3.

- [ ] **Step 1: Write the failing integration test**

Create `crates/memory-mcp/tests/context_lookup_indexes.rs`:

```rust
mod common;

use memory_mcp::storage::DbClient;

#[tokio::test]
async fn context_lookup_indexes_registered_by_migrations() {
    let (_service, db) = common::make_service_with_client().await;
    let episode = db.query("INFO FOR TABLE episode", None, "org").await.expect("info episode").to_string();
    let fact = db.query("INFO FOR TABLE fact", None, "org").await.expect("info fact").to_string();
    assert!(episode.contains("episode_episode_id"), "{episode}");
    assert!(fact.contains("fact_source_episode"), "{fact}");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: cargo test -p memory_mcp --test context_lookup_indexes -- --test-threads=1
Expected: FAIL - neither index name is present. That is the RED signal.

- [ ] **Step 3: Write the migration script**

Create `crates/memory-mcp/migrations/053_context_lookup_indexes.surql`:

```surql
-- Context-lookup indexes for the plan 2026-10-09 query-performance work.
DEFINE INDEX IF NOT EXISTS episode_episode_id ON TABLE episode COLUMNS episode_id;
DEFINE INDEX IF NOT EXISTS fact_source_episode ON TABLE fact COLUMNS source_episode;
```

- [ ] **Step 4: Register the script in the storage catalog**

In `crates/memory-mcp/src/storage/migrations.rs`, append after the `039_filesystem_ingestion.surql` entry of `versioned_migrations()` (same two-field shape as the 039 entry: no version field exists):

```rust
MigrationScript {
    file_name: "053_context_lookup_indexes.surql",
    sql: include_str!("../../migrations/053_context_lookup_indexes.surql"),
},
```

- [ ] **Step 5: Run the test and the catalog checks**

Run: cargo test -p memory_mcp --test context_lookup_indexes -- --test-threads=1
Expected: PASS.

Run: cargo test -p memory_mcp --lib storage::migrations
Expected: PASS (duplicate-name and ordering checks cover the new entry).

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/memory-mcp/migrations/053_context_lookup_indexes.surql crates/memory-mcp/src/storage/migrations.rs crates/memory-mcp/tests/context_lookup_indexes.rs
git commit -m "feat(storage): index episode/fact columns used by explain lookups"
```

---

### Task 5: Stop retrying stalled query timeouts three times

**Files:**
- Modify: crates/memory-mcp/src/platform/persistence/transactions.rs (constants, retry loop, its `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: `pub(crate) struct DbRetryPolicy { pub(crate) attempts: u32, pub(crate) timeout_attempts: u32, pub(crate) initial_delay: Duration, pub(crate) per_attempt_timeout: Duration }` with `Default` = 3 / 2 / 200 ms / 30 s; `pub(crate) async fn with_db_retry_policy<T, F, Fut>(op_name: &str, policy: &DbRetryPolicy, logger: &StdoutLogger, f: F) -> Result<T, MemoryError>`; `with_db_retry(op_name, logger, f)` keeps its exact signature and delegates with the default policy, so no call site outside this file changes.

The prod error text is this loop (L85-92): every attempt re-runs a stalled 30 s query, and the timeout branch retries it like a transient conflict - up to 3 full timeouts (~90 s + backoffs) against an overloaded server. A timeout is not a conflict: two attempts cover a single WebSocket hang (the original motivation for the per-attempt timeout) while halving the retry-storm amplification documented in Appendix A RC2. Non-timeout transient conflicts keep 3 attempts.

- [ ] **Step 1: Write the failing unit tests**

In the existing `#[cfg(test)] mod tests` add a helper and three tests. `StdoutLogger` and the `AtomicU32` counter come from the imports the existing tests already use.

```rust
fn fast_policy(timeout_attempts: u32) -> DbRetryPolicy {
    DbRetryPolicy {
        attempts: 3,
        timeout_attempts,
        initial_delay: Duration::from_millis(1),
        per_attempt_timeout: Duration::from_millis(50),
    }
}
```

```rust
#[tokio::test]
async fn stalled_attempt_fails_after_two_timeout_attempts() {
    let calls = Arc::new(AtomicU32::new(0));
    let counter = calls.clone();
    let result: Result<i32, MemoryError> = with_db_retry_policy(
        "test_op", &fast_policy(2), &logger(),
        || { let counter = counter.clone(); async move {
            counter.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(0)
        } },
    ).await;
    let message = match result {
        Err(MemoryError::Storage(message)) => message,
        other => panic!("expected a timeout storage error, got {other:?}"),
    };
    assert!(message.contains("timed out"), "{message}");
    assert_eq!(calls.load(Ordering::SeqCst), 2, "timeout must not get a third attempt");
}
```

```rust
#[tokio::test]
async fn timeout_then_success_returns_value() {
    let calls = Arc::new(AtomicU32::new(0));
    let counter = calls.clone();
    let result = with_db_retry_policy("test_op", &fast_policy(2), &logger(), || {
        let counter = counter.clone();
        async move {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                tokio::time::sleep(Duration::from_millis(500)).await;
                Ok(0)
            } else {
                Ok(7)
            }
        }
    }).await;
    assert_eq!(result.unwrap(), 7);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
```

```rust
#[tokio::test]
async fn transient_conflicts_still_get_three_attempts() {
    let calls = Arc::new(AtomicU32::new(0));
    let counter = calls.clone();
    let policy = DbRetryPolicy { attempts: 3, timeout_attempts: 1,
        initial_delay: Duration::from_millis(1), per_attempt_timeout: Duration::from_secs(30) };
    let result: Result<i32, MemoryError> = with_db_retry_policy(
        "test_op", &policy, &logger(), || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(1)).await;
                Err(MemoryError::Storage("Resource busy".into()))
            }
        }).await;
    match result {
        Err(MemoryError::Storage(message)) => assert_eq!(message, "Resource busy"),
        other => panic!("expected the last storage error, got {other:?}"),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}
```

- [ ] **Step 2: Run the unit tests to verify they fail**

Run: `cargo test -p memory_mcp --lib platform::persistence::transactions`
Expected: COMPILE ERROR — `DbRetryPolicy` and `with_db_retry_policy` not found in this scope.

- [ ] **Step 3: Implement `DbRetryPolicy` and `with_db_retry_policy` in `transactions.rs`**

Add `#[derive(Debug, Clone)] pub(crate) struct DbRetryPolicy` with fields `attempts: u32`, `timeout_attempts: u32`, `initial_delay: Duration`, `per_attempt_timeout: Duration` (all `pub(crate)`), and `impl Default` seeding from the existing consts: `attempts: DEFAULT_DB_RETRY_ATTEMPTS`, `timeout_attempts: 2`, `initial_delay: Duration::from_millis(DEFAULT_DB_RETRY_INITIAL_DELAY_MS)`, `per_attempt_timeout: Duration::from_secs(DEFAULT_DB_QUERY_TIMEOUT_SECS)`. Move the current `with_db_retry` loop into `pub(crate) async fn with_db_retry_policy<T, F, Fut>(op_name: &str, policy: &DbRetryPolicy, logger: &StdoutLogger, f: F) -> Result<T, MemoryError>` with the same `F: Fn() -> Fut` / `Fut: Future<Output = Result<T, MemoryError>>` bounds.

Changes inside the loop; everything else (log fields, op names `db.{op}.retry` / `db.{op}.timeout`) stays identical:
- per-attempt guard: `tokio::time::timeout(policy.per_attempt_timeout, f())`;
- transient branch threshold: `attempt >= policy.attempts` (the default keeps today's 3);
- timeout branch threshold: `attempt >= policy.timeout_attempts` — the actual fix; today a stalled attempt re-runs 3 times (~90 s of SurrealDB burn before the user sees the failure);
- timeout error keeps the shape `db.{op_name}: timed out after {secs}s ({n} attempts)` but reports what actually ran: `secs = policy.per_attempt_timeout.as_secs()`, `n = attempt` (today L88-92 hardcodes the total constant 3 regardless of attempts run);
- backoff delay in both branches: `policy.initial_delay.saturating_mul(1 << attempt.saturating_sub(1).min(6))`;
- the `max_attempts` log field takes the branch's own threshold value from the policy.

Then `with_db_retry` becomes a thin wrapper: `with_db_retry_policy(op_name, &DbRetryPolicy::default(), logger, f).await`. All existing callers and the four existing unit tests stay unchanged.

- [ ] **Step 4: Run the unit tests to verify they pass**

Run: `cargo test -p memory_mcp --lib platform::persistence::transactions`
Expected: 7 passed (3 new + 4 existing), 0 failed.

- [ ] **Step 5: Format and commit**

```bash
cargo fmt --all
git add crates/memory-mcp/src/platform/persistence/transactions.rs
git commit -m "fix(persistence): cap query timeout retries at two attempts"
```

---

### Task 6: Preload communities once per explain insight batch

**Files:**
- Modify: `crates/memory-mcp/src/memory/explanation.rs` (`build_graph_insights_batched`; add a test to `mod tests`)
- Modify: `crates/memory-mcp/src/memory/retrieval/graph_surprising.rs` (`find_surprising_connections`; `find_surprising_connections_honors_neighbor_query_budget`)

**Interfaces:**
- Consumes: `GraphCommunity` (`graph_reads.rs` L31-36) and `graph_community_from_value` (`graph_reads.rs` L57); `KnowledgeGraphStore::select_communities() -> Result<Vec<Value>, MemoryError>`; `ExplanationService` already implements `GraphContext` (it passes `self` into graph reads today).
- Produces: `pub(crate) async fn find_surprising_connections(ctx: &impl GraphContext, source_entity: &str, max_depth: i32, budget: GraphTraversalBudget, communities: &[GraphCommunity]) -> Result<Vec<SurprisingConnection>, MemoryError>` — new final parameter; after this task the function itself never reads the `community` table.

Why this belongs in the performance plan: `find_surprising_connections` currently loads and parses the whole `community` table on every call (`graph_surprising.rs` L49-L55), and `build_graph_insights_batched` calls it once per linked entity (`explanation.rs` L398-L420, up to 8 entities) — up to 8 full `community` scans inside a single explain request. One hoisted fetch makes it one.

- [ ] **Step 1: Write the failing batch test in `explanation.rs` `mod tests`**

Add two items to the tests module. First, a counting fake that mirrors the six-method `DbClient` surface of the fake at `graph_surprising.rs` L262-L360 and counts every read of the `community` table:

```rust
#[derive(Default)]
struct CountingDbClient {
    community_selects: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl crate::storage::client::DbClient for CountingDbClient {
    async fn select_one(&self, record_id: &str, _ns: &str) -> Result<Option<Value>, MemoryError> {
        Ok(Some(json!({"entity_id": record_id, "canonical_name": record_id})))
    }
    async fn select_table(&self, table: crate::storage::table_scope::OwnedTable, _ns: &str) -> Result<Vec<Value>, MemoryError> {
        if table.as_str() == "community" {
            self.community_selects.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(vec![])
    }
    async fn create(&self, _: &str, _: Value, _: &str, _: &[&str]) -> Result<Value, MemoryError> { Ok(Value::Null) }
    async fn update(&self, _: &str, _: Value, _: &str, _: &[&str]) -> Result<Value, MemoryError> { Ok(Value::Null) }
    async fn query(&self, sql: &str, _vars: Option<Value>, _ns: &str) -> Result<Value, MemoryError> {
        if sql.contains("FROM edge") { Ok(Value::Array(Vec::new())) } else { Ok(Value::Null) }
    }
    async fn apply_migrations(&self, _ns: &str) -> Result<(), MemoryError> { Ok(()) }
}
```

Second, the test. All names (`Arc`, `Value`, `json!`, `MemoryError`, `StdoutLogger`, `ExplanationService`) are already in the tests module's scope via `use super::*`; the private `build_graph_insights_batched` is callable from the module's own tests:

```rust
#[tokio::test]
async fn explain_batch_fetches_communities_once() {
    let db = Arc::new(CountingDbClient::default());
    let svc = ExplanationService::new(db.clone(), StdoutLogger::new("warn"), "org".to_string());
    let insights = svc
        .build_graph_insights_batched(&["entity:a".to_string(), "entity:b".to_string(), "entity:c".to_string()])
        .await
        .expect("insights")
        .expect("batch has linked entities");
    assert!(insights.hub_entities.is_empty() && insights.surprising_connections.is_empty());
    assert_eq!(
        db.community_selects.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "one community load per batch, not one per entity"
    );
}
```

- [ ] **Step 2: Run the batch test to verify it fails**

Run: `cargo test -p memory_mcp --lib memory::explanation::tests::explain_batch_fetches_communities_once`
Expected: FAIL on the `assert_eq!` with `left: 3, right: 1` (the test itself compiles — the failure is the behavior, not plumbing): today every linked entity triggers its own `community` fetch.

- [ ] **Step 3: Hoist the community fetch and pass it down as a slice**

In `explanation.rs`, immediately after the `explain.graph_insights.start` log and before the `find_hub_entities` call, add one hoisted fetch:

```rust
let communities = self
    .knowledge_graph_store()
    .select_communities()
    .await?
    .into_iter()
    .filter_map(|record| crate::memory::retrieval::graph_reads::graph_community_from_value(&record))
    .collect::<Vec<_>>();
```

Then pass it into the loop call: `find_surprising_connections(self, &entity_id, 3, budget, &communities)`.

In `graph_surprising.rs`, add the final parameter `communities: &[GraphCommunity]` to the `find_surprising_connections` signature and delete the internal fetch statement (L49-L55, from `let communities = ctx.knowledge_graph_store()` through its `.collect::<Vec<_>>();`). The `let cutoff_iso = ...` line above it and everything below it (`let source_community_ids = community_ids_for_member(&communities, source_entity);` and the rest of the body) stay exactly as they are, now reading the parameter. `GraphCommunity` is already imported at L6.

- [ ] **Step 4: Update the budget test to the new signature**

In `find_surprising_connections_honors_neighbor_query_budget`: add a `community_selects: AtomicUsize` field to `BudgetedGraphDbClient` and increment it inside the fake's existing `community` branch (the branch keeps returning its 256 fixture rows). Pass the same 256 rows pre-parsed into the call, and assert the scan never reads the table itself:

```rust
let communities = (0..256)
    .map(|idx| json!({
        "community_id": format!("community:{idx}"),
        "summary": format!("Community {idx}"),
        "member_entities": [format!("entity:{idx}")],
        "updated_at": "2026-04-15T00:00:00Z",
    }))
    .filter_map(|record| graph_community_from_value(&record))
    .collect::<Vec<_>>();
```
```rust
let connections = find_surprising_connections(
    &service, "entity:0", 32, GraphTraversalBudget::FULL, &communities,
)
.await
.expect("connections");
assert_eq!(
    db.community_selects.load(Ordering::Relaxed),
    0,
    "the scan must not read the community table itself"
);
```

The two existing budget assertions (`neighbor_queries <= max_neighbor_queries`, `connections.len() <= max_results`) stay unchanged. `json!`, `graph_community_from_value`, and `Ordering` are already imported in that tests module.

- [ ] **Step 5: Run both test targets, then commit**

Run: `cargo test -p memory_mcp --lib memory::retrieval::graph_surprising`
Run: `cargo test -p memory_mcp --lib memory::explanation`
Expected: both PASS — the batch test sees exactly 1 community fetch; the budget test sees 0 self-fetches and still holds its budget bounds.

```bash
git add crates/memory-mcp/src/memory/explanation.rs crates/memory-mcp/src/memory/retrieval/graph_surprising.rs
git commit -m "perf(memory): preload communities once per explain insight batch"
```

---

### Task 7: Kill the query-cache dead-code warning and pass the repo gates

**Files:**
- Modify: `crates/memory-mcp/src/embedding/query_cache.rs` (the dead accessor on `QueryEmbeddingCacheState`)

**Interfaces:**
- Consumes: the branch as shipped by Tasks 1-6.
- Produces: a branch where `cargo check -p memory_mcp` on default features prints zero warnings, and fmt / clippy / the full suite all pass.

Context for the exact warning the operator reported: `warning: methods accounted_bytes, len, and is_empty are never used` in `crates/memory-mcp/src/embedding/query_cache.rs` when building `memory_mcp` (lib) on the default feature set. In the current tree `len`/`is_empty` are already `#[cfg(test)]` and used by unit tests; the accessor that is genuinely dead on default features is `accounted_bytes`: every production caller of it (`MemoryService::retained_cache_bytes` in `src/service/core.rs`, `http/runtime/memory_snapshot.rs`, `http/runtime/storage.rs`) exists only under the `streamable-http` feature. Gate the accessor at its only consumer instead of deleting it — the SaaS build reads it for the Prometheus byte-retention gauges.

- [ ] **Step 1: Reproduce the warning**

Run: `cargo check -p memory_mcp 2>&1 | grep -n "never used"`
Expected: at least one dead-code warning naming `accounted_bytes` on `QueryEmbeddingCacheState` (report the full list; handle every name it prints the same way in Step 2).

- [ ] **Step 2: Gate `accounted_bytes` behind its consumers**

In `crates/memory-mcp/src/embedding/query_cache.rs`, the accessor at line 113 is reached from two communities: the module's own unit tests (lines 190, 197, 272) and the `streamable-http` production path (`MemoryService::retained_cache_bytes` in `src/service/core.rs`, the HTTP byte-retention gauges). Gate it for exactly those two, not for `streamable-http` alone: a bare `#[cfg(feature = "streamable-http")]` would strip it from the default unit-test build and break those tests. Insert `#[cfg(any(test, feature = "streamable-http"))]` immediately above the existing `#[must_use]`:

```rust
    #[cfg(any(test, feature = "streamable-http"))]
    #[must_use]
    pub(crate) fn accounted_bytes(&self) -> usize {
        self.accounted_bytes
    }
```

`len` and `is_empty` (lines 119, 125) are already `#[cfg(test)]`, so they are absent from the default non-test lib and cannot carry the warning; leave them untouched. Never reach for `#[allow(dead_code)]` — the repo forbids it.

- [ ] **Step 3: Confirm the warning is gone**

Run: `cargo check -p memory_mcp 2>&1 | grep -n "never used"`
Expected: no output. `cargo check -p memory_mcp` itself must exit clean with zero warnings — the accessor is now compiled only where something reads it.

- [ ] **Step 4: Format check**

Run: `cargo fmt --all --check`
Expected: no diff. If it reports one, run `cargo fmt --all` and re-check.

- [ ] **Step 5: Workspace clippy**

Run: `cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings`
Expected: exit 0. This profile compiles `streamable-http`, so the accessor stays live there too — both profiles are warning-free, not just the default one.

- [ ] **Step 6: Full suite**

Run: `cargo test -p memory_mcp`
Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/memory-mcp/src/embedding/query_cache.rs
git commit -m "fix(embedding): gate query-cache byte accounting behind its only consumer"
```

---

## Operator runbook: native guardrails (documented, not executed)

This plan changes no production setting. These are the operator-side levers that complement the code fixes, in the order to try them.

1. **Bound runaway queries server-side** — start the container with `--query-timeout <duration>` so a pathologically slow query is killed by the server rather than held by the client; mirror the flag in the compose file or unit that starts SurrealDB. This is the native complement to Task 5's two-attempt client cap.
2. **Relax background intervals** — raise the schedule of the node-membership refresh, index compaction and change-feed GC tasks so the idle CPU/disk churn in RC3 drops; none is on a request's critical path.
3. **Do not publish the SurrealDB port** — keep the database port off the host's public interface and reach it only over the internal Docker network.
4. **Rotate credentials** — supply `SURREALDB_USERNAME` / `SURREALDB_PASSWORD` from a secret store rather than the compose file, and rotate on the normal cadence.
5. **Watch the right signals** — the code emits `db.<op>.timeout` and `db.<op>.retry` ops; alert on their rate and on the container's CPU and block-read (docker stats / Grafana). After this plan the timeout op fires at most once per stalled call (Task 5), and the index fixes make the retry op rarer.
6. **Index backfill is a one-time startup cost** — the two indexes from Task 4 build when migrations replay at startup; on the current prod dataset (~30 episodes / ~170 facts) it is immediate, but a larger namespace pays that build once on the first start after deploy (Review Focus 5).

---

## Appendix A: Root-cause evidence (EXPLAIN on SurrealDB 3.3.0 CE)

The plan hinges on a fact that is invisible in the logs: on SurrealDB 3.3.0 the planner refuses these record predicates as index bounds and falls back to a full scan. Each row below is an `EXPLAIN` run against an embedded `mem://` namespace seeded with representative rows; the predicate is the exact shape the code emits today.

### A.1 RC1 - record-cast predicates defeat index selection

| Predicate shape (edge/fact/episode) | Planner decision | Index used |
|---|---|---|
| `in = <record> $param` | TableScan on the edge table | none — the cast is folded after index selection |
| `out = <record> $param` | TableScan on the edge table | none — same fold-order problem |
| `in = type::record($table, $key)` | IndexScan | `edge_in` |
| `out = type::record($table, $key)` | IndexScan | `edge_out` |
| `fact_id = $param` | IndexScan | `fact_claim_backfill_cursor_idx` (migration 029, `ON fact COLUMNS fact_id`) |
| `episode_id = $param` (no index on `episode.episode_id`) | TableScan | none |
| `source_episode = $param` (no index on `fact.source_episode`) | TableScan | none |
| `x IN (SELECT ... FROM ...)` nested subquery | TableScan on the outer table too | none — the subquery result is not a pushdown bound |
| `... WITH INDEX edge_in` | parses, no effect (still TableScan) | none |
| `... USE INDEX` / `... FORCE INDEX` | syntax error in 3.3.0 | n/a |

Two consequences the code must absorb:

- **The index follows the column, not the direction.** `node_field` already maps `Direction::Incoming -> "out"` and `Direction::Outgoing -> "in"` (`knowledge/queries.rs` L218-222, L240-242); the fix changes only the right-hand side of the comparison, so an Outgoing lookup keeps resolving through `edge_in` and an Incoming one through `edge_out`. The Task 1 EXPLAIN test pins exactly this.
- **There is no hint escape hatch.** `WITH INDEX` is a no-op here and `USE`/`FORCE INDEX` do not parse, so the only lever is the predicate shape — which is why the fix is in the query builders, not in storage configuration.

### A.2 RC2 - a timeout is retried like a conflict, tripling the stall

The operator observed `explain` failing with **`storage error: db.execute_query: timed out after 30s (3 attempts)`**. That string is produced verbatim at `src/platform/persistence/transactions.rs` L88-91: `db.{op_name}: timed out after {DEFAULT_DB_QUERY_TIMEOUT_SECS}s ({DEFAULT_DB_RETRY_ATTEMPTS} attempts)`, with `DEFAULT_DB_QUERY_TIMEOUT_SECS = 30` (L26) and `DEFAULT_DB_RETRY_ATTEMPTS = 3` (L22).

The retry loop (L43-84) treats a wall-clock timeout the same as a transient conflict: the timeout branch (L85-92) re-runs the same query. Against an already overloaded container the unindexed scans from A.1 push a single query past 30 s, so a failed `explain` costs **three full 30 s stalls (~90 s) of SurrealDB CPU**, which is exactly the 200 % container load reported while `explain` was hung — each retry re-reads the same data it just failed to finish reading. Task 5 decouples the two: timeouts cap at two attempts, transient conflicts keep three.

### A.3 RC3 - idle CPU and disk churn

The operator's steady-state `docker stats` for the container —
`9.98%` CPU, `2.51TB / 58.6GB` block-read — is a server that is never fully quiet. Three contributors stack:

1. **RocksDB background work** (compaction, WAL, blob rewrites) runs against the mounted volume whether or not a query arrives; the `2.51TB` figure is cumulative read since container start, not a single burst.
2. **Periodic interval tasks** the server schedules (node-membership refresh, index compaction, change-feed GC) tick on their own clocks.
3. **Cumulative unindexed full scans** (RC1): every explain path that falls back to a TableScan reads whole tables (`fact`, `episode`, `edge`), which keeps the page cache and compaction pipeline busy well after the request returns.

There is no cheap native probe for this on the Community edition: slow-query logging is Enterprise-only in 3.3.0, so `EXPLAIN` (A.1) is the diagnostic of record, and the runbook's `--query-timeout` plus background-interval relaxation are the operational knobs.

## Appendix B: Native SurrealDB features and community practice

Primary reference: the SurrealDB query documentation the operator supplied, <https://surrealdb.com/docs/learn/querying>, plus the function and index pages it links (`type::record`, `DEFINE INDEX`, `INSIDE`).

### B.1 Native features this plan uses

- **`type::record($table, $key)`** (Task 1, Task 2, Task 3 stage 1) — constructs a record id from two bound strings in-query, which the 3.3.0 planner accepts as an index bound where a literal cast does not (RC1).
- **Parameterized `INSIDE $param`** (Task 3 stages 2-3) — array membership as a bound parameter, the documented way to pass an id list without string interpolation.
- **Migration-registered secondary indexes** (Task 4) — `DEFINE INDEX` replayed from the versioned catalog; on startup the index builds over existing rows before the store serves.
- **Batched reads hoisted to the caller** (Task 6) — one communities read per explain batch instead of one per entity, using the existing repository read rather than a new query shape.

### B.2 Native options considered and rejected

- **`WITH INDEX <name>`** — parses on 3.3.0 but does not change the plan; the query still TableScans (RC1).
- **`USE INDEX` / `FORCE INDEX`** — do not parse on 3.3.0 (syntax error); there is no index-hint escape hatch.
- **`type::thing(...)`** — absent on 3.3.0; the parser itself suggests `type::record`.
- **Record-link graph traversal** (`->edge->node`) — would need schema-level record-link typing the codebase does not declare; the plan keeps the explicit edge queries and fixes their predicates instead.
- **Enterprise slow-query logging** — the only native way to surface slow queries, and it is license-gated off in CE.

### B.3 Community practice applied

- Keep **bare record values** (not computed expressions) in predicates on indexed columns; a computation on the indexed side defeats index selection.
- Give **every column a lookup predicate binds its own index** — this is what makes adding `episode.episode_id` and `fact.source_episode` the correct complement to the query-shape fixes (`fact.fact_id` already carries one).
- Prefer **parameterized array membership** over interpolated `IN (...)` lists, both for index use and to avoid re-parsing a fresh query string per call.
