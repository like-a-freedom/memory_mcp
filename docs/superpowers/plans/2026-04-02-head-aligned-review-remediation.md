# Head-Aligned Review Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the remediation items that still exist in `HEAD` — both the original architectural cleanup work and the retrieval-quality gaps identified from eval runs (LongMemEval, MAB Conflict_Resolution benchmarks).

**Architecture:** This plan keeps the current `mcp -> service -> storage -> SurrealDB` layering intact and avoids breaking public-contract changes. The work is split into eight waves: align the review docs with reality, move entity→episode traversal behind `DbClient`, add a typed `fact.entity_links` migration, remove confirmed dead helpers from `embedding.rs`, polish MCP tool guidance, wire entity-graph expansion into `assemble_context`, expose temporal fields in `AssembledContextItem`, and fix in-memory SurrealDB eval stability.

**Tech Stack:** Rust 2024, rmcp, SurrealDB, serde/serde_json, schemars, tokio, existing integration + MCP E2E tests

---

## Coverage Status — все проблемы из eval и ревью

| # | Проблема | Статус в `HEAD` | Адресовано планом | Wave |
|---|---|---|---|---|
| 1 | **Semantic retrieval** — FTS alone insufficient, нужен embedding-based search | Инфраструктура есть (`collect_semantic_facts`, `select_facts_ann`, `LocalCandleEmbeddingProvider`), но не активна в eval без `EMBEDDINGS_ENABLED=true` | ✅ Wave 6, Task 6a — enable local embeddings в eval setup | 6 |
| 2 | **Graph-based retrieval** — BFS по entity graph нужен для multi-hop | `bfs_path` — `#[cfg(test)]` only, не подключён к `assemble_context`. `select_edge_neighbors` + `select_facts_by_entity_links` в `DbClient` не вызываются в retrieval pipeline | ✅ Wave 6, Task 6b — wire entity-graph expansion в `assemble_context` | 6 |
| 3 | **Entity-aware retrieval** — GLiNER entity должны использоваться для retrieval | `select_entities_batch` используется только для alias expansion, НЕ для entity-graph traversal в retrieval | ✅ Wave 6, Task 6b — entity NER → graph traversal → fact lookup pipeline | 6 |
| 4 | **Temporal computation** — `assemble_context` должен возвращать `t_ref`/`t_valid` | `AssembledContextItem` не имеет timestamp полей; `Fact.t_ref`/`Fact.t_valid` есть в модели | ✅ Wave 7 — добавить `t_ref`/`t_valid` в `AssembledContextItem` | 7 |
| 5 | **Multi-hop reasoning** — "country of citizenship of spouse of author" | Requires LLM-level 3+ hop reasoning; сервер не может resolver reasoning chains | ⚠️ Partial — Wave 6 даёт 1-hop entity expansion; reasoning остаётся на LLM | 6 |
| 6 | **In-memory stability** — SurrealDB деградирует после ~20 eval кейсов | Нет recycling; `longmem_acceptance.rs` создаёт fresh service per test, но eval loop может стать проблемой | ✅ Wave 8 — per-batch recycling + диагностический тест | 8 |
| 7 | **Abstention detection** — система должна говорить "не знаю" | LongMemEval oracle не содержит abstention test cases — dataset artifact, не проблема кода. | ⚠️ Out of scope — dataset issue, не server behavior | — |
| 8 | **Temporal reasoning** — "How many days before X happened?" | `t_ref`/`t_valid` не предоставляются LLM; после Wave 7 — LLM получит данные для вычислений | ✅ Wave 7 — expose timestamps; date math остаётся на LLM | 7 |

> **Легенда:** ✅ Полностью адресовано | ⚠️ Частично / ограничения уровня сервера | ❌ Не адресовано

---

## Validation Notes — claims to exclude from implementation

The following review items were re-checked against `HEAD` and should **not** be reopened in implementation work:

| Review Claim | Actual State in `HEAD` | Verdict |
|---|---|---|
| GLiNER applies threshold directly to logits | `src/service/gliner_entity_extractor.rs` already applies `let prob = 1.0_f32 / (1.0_f32 + (-score).exp())` before comparing to `self.threshold` | ❌ Already fixed |
| `find_episodes_via_entity` is a stub | `src/service/core.rs:1449-1465` already contains a working helper that queries linked episodes | ❌ Already implemented |
| BM25/FTS still uses whitespace-only analyzer | `src/migrations/__Initial.surql` and `src/migrations/006_simplified_search_redesign.surql` already use `FILTERS lowercase, ascii, snowball(english)` | ❌ Already fixed |
| MCP responses need a new `suggested_next_action` field | `src/mcp/handlers.rs:36-51` already exposes `guidance`, and public tests assert it | ❌ Already covered |
| Remote embedding providers are zombie runtime code | `src/service/embedding.rs` still instantiates `OpenAiCompatibleEmbeddingProvider` and `OllamaEmbeddingProvider` from `create_embedding_provider()` | ❌ Not a valid deletion target |

The following **eval-observed problems are out of scope** for the retrieval server because they require LLM-level reasoning that the server cannot perform:

| Problem | Why Out of Scope | How the Server Helps |
|---|---|---|
| Multi-hop chain reasoning ("country of citizenship of spouse of author of X") | Requires 3+ step reasoning chains — the MCP server can only retrieve and surface facts; the agent must reason over them | Wave 6 provides 1-2-hop entity graph expansion, which surfaces relevant entity-linked facts deeper than FTS alone |
| Temporal arithmetic ("how many days before X happened") | Computing "N days" requires knowing WHICH returned facts are X and Y — that is LLM-level reasoning over the retrieved set | Wave 7 exposes `t_ref`/`t_valid` on every `AssembledContextItem` so the LLM has the timestamp data to compute differences |
| Abstention tuning (0% empty-when-irrelevant) | Per eval analysis: LongMemEval oracle has no abstention test cases — this is a dataset filtering artifact, not a server behavior problem | N/A — dataset issue |
| Semantic retrieval "not in hot path" | `collect_semantic_facts` IS wired into `assemble_context:195`. The path is live but requires `EMBEDDINGS_ENABLED=true`. In eval runs without this env var, the semantic path is silently skipped. `LocalCandleEmbeddingProvider` is available via `EMBEDDINGS_PROVIDER=local`. | Enable by default in eval config — see Wave 6 |

Only the residual items below belong in implementation work.

---

## File Map

| File | Role in this plan |
|---|---|
| `docs/REVIEW_ALIGNMENT_2026-03-25.md` | Remove stale claims and align the documented backlog with real `HEAD` |
| `src/service/core.rs` | Replace direct SQL lookup with `DbClient`-backed entity→episode traversal |
| `src/storage.rs` | Extend `DbClient`, implement linked-episode query, register new migration |
| `src/service/test_support.rs` | Keep `MockDb` aligned with the extended `DbClient` trait |
| `src/models.rs` | Add entity-link compatibility deserialization; add `t_ref`/`t_valid` to `AssembledContextItem` |
| `src/migrations/014_fact_entity_links_typed.surql` | Add typed `fact.entity_links` schema migration |
| `src/service/embedding.rs` | Remove confirmed dead helper functions without touching live providers |
| `src/mcp/handlers.rs` | Refine `ingest` / `extract` / `explain` / `open_app` descriptions and app-specific errors |
| `src/mcp/params.rs` | Keep the current public schema, but update tests and comments for clarity |
| `src/mcp/error.rs` | Preserve `guidance` as the canonical next-step hint |
| `src/service/context.rs` | Add entity-graph expansion phase; populate `t_ref`/`t_valid` in result mapping |
| `tests/explain_provenance.rs` | Behavioral coverage for linked episodes through shared entities |
| `tests/tools_e2e.rs` | MCP surface tests for `guidance`, `explain`, `open_app`, and `t_ref` in results |
| `tests/eval_retrieval.rs` | In-memory stability (connection recycling) |
| `tests/embedded_support.rs` | Shared in-memory DB setup used by eval tests |

---

## Wave 1 — Rebase the review backlog on real `HEAD` (P0)

### Task 1: Update the review-alignment document so it stops asking for already-completed work

**Why this task exists:** `docs/REVIEW_ALIGNMENT_2026-03-25.md` still claims that `find_episodes_via_entity` is a stub and that several MCP/FTS issues remain, even though the code no longer matches that description. This creates a false backlog and increases the odds of duplicate work.

**Files:**
- Modify: `docs/REVIEW_ALIGNMENT_2026-03-25.md`

- [ ] **Step 1: Rewrite the stale residual-work section in `docs/REVIEW_ALIGNMENT_2026-03-25.md`**

Replace the stale note about the stub and outdated MCP/FTS gaps with an updated residual-work section like this:

```markdown
## Remaining Work (HEAD-aligned)

| Item | Status | Notes |
| --- | --- | --- |
| Entity → episode traversal bypasses `DbClient` | 📋 Planned | Helper works, but storage logic still lives in `src/service/core.rs` |
| `fact.entity_links` still uses string IDs | 📋 Planned | Migration to typed record refs should preserve compatibility |
| `embedding.rs` contains test-only dead helpers | 📋 Planned | Remove only confirmed dead helpers, keep live providers |
| MCP tool descriptions still have friction points | 📋 Planned | Improve descriptions and invalid-parameter guidance without breaking schema |

### Excluded from implementation

- GLiNER sigmoid before threshold — already fixed in `src/service/gliner_entity_extractor.rs`
- BM25 snowball analyzer — already present in migrations
- `suggested_next_action` — already covered by `guidance`
- `find_episodes_via_entity` stub claim — outdated; helper exists in `src/service/core.rs`
```

- [ ] **Step 2: Sanity-check the doc against the source tree before committing**

Run: `cargo check`
Expected: PASS — docs-only update must not require code changes.

- [ ] **Step 3: Commit**

```bash
git add docs/REVIEW_ALIGNMENT_2026-03-25.md docs/superpowers/plans/2026-04-02-head-aligned-review-remediation.md
git commit -m "docs: align remediation backlog with current head"
```

---

## Wave 2 — Move entity→episode traversal behind `DbClient` (P0)

### Task 2: Replace the direct SQL helper in `src/service/core.rs` with a `DbClient` capability

**Why this task exists:** `src/service/core.rs:1449-1465` contains working SQL for linked episodes, but that SQL bypasses the storage boundary. Per repository rules, DB-specific traversal belongs in `src/storage.rs` behind `DbClient`, not in the service layer.

**Files:**
- Modify: `src/storage.rs:38-2142`
- Modify: `src/service/core.rs:1400-1465`
- Modify: `src/service/test_support.rs:1-167`
- Test: `tests/explain_provenance.rs`

- [ ] **Step 1: Add a failing storage-level test for linked episode lookup**

Add a focused test in `src/storage.rs` near the existing migration/query tests:

```rust
#[tokio::test]
async fn select_episodes_by_entity_returns_linked_episodes() {
    let client = SurrealDbClient::connect_in_memory("testdb", "testns", "warn")
        .await
        .expect("connect");
    client.apply_migrations("testns").await.expect("migrations");

    client
        .create(
            "entity:atlas",
            serde_json::json!({
                "entity_id": "entity:atlas",
                "canonical_name": "Atlas",
                "canonical_name_normalized": "atlas",
                "entity_type": "project",
                "aliases": []
            }),
            "testns",
        )
        .await
        .expect("entity");

    client
        .create(
            "episode:atlas-1",
            serde_json::json!({
                "episode_id": "episode:atlas-1",
                "source_type": "note",
                "source_id": "atlas-1",
                "content": "Atlas kickoff",
                "scope": "org",
                "t_ref": "2026-01-01T00:00:00Z",
                "t_ingested": "2026-01-01T00:00:00Z"
            }),
            "testns",
        )
        .await
        .expect("episode");

    client
        .create(
            "fact:atlas-1",
            serde_json::json!({
                "fact_id": "fact:atlas-1",
                "fact_type": "note",
                "content": "Atlas kickoff",
                "quote": "Atlas kickoff",
                "source_episode": "episode:atlas-1",
                "scope": "org",
                "entity_links": ["entity:atlas"],
                "t_valid": "2026-01-01T00:00:00Z",
                "t_ingested": "2026-01-01T00:00:00Z",
                "confidence": 1.0,
                "access_count": 0,
                "index_keys": []
            }),
            "testns",
        )
        .await
        .expect("fact");

    client
        .relate_edge(
            "testns",
            "edge:atlas-fact",
            "entity:atlas",
            "fact:atlas-1",
            serde_json::json!({
                "relation": "involved_in",
                "t_valid": "2026-01-01T00:00:00Z"
            }),
        )
        .await
        .expect("edge");

    let rows = client
        .select_episodes_by_entity("testns", "entity:atlas", 10)
        .await
        .expect("lookup");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["episode_id"], serde_json::json!("episode:atlas-1"));
}
```

Run: `cargo test select_episodes_by_entity_returns_linked_episodes -- --test-threads=1`
Expected: FAIL with `no method named select_episodes_by_entity`.

- [ ] **Step 2: Extend `DbClient` and `MockDb` with a dedicated lookup method**

Add this trait method to `src/storage.rs`:

```rust
    /// Selects episodes linked to an entity through active relation edges and facts.
    async fn select_episodes_by_entity(
        &self,
        namespace: &str,
        entity_id: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError>;
```

Add the matching closure type and field to `src/service/test_support.rs`:

```rust
type SelectEpisodesByEntityFn =
    dyn Fn(&str, &str, i32) -> Result<Vec<Value>, MemoryError> + Send + Sync;
```

```rust
pub select_episodes_by_entity_fn: Box<SelectEpisodesByEntityFn>,
```

```rust
select_episodes_by_entity_fn: Box::new(|_, _, _| Ok(vec![])),
```

```rust
    async fn select_episodes_by_entity(
        &self,
        namespace: &str,
        entity_id: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        (self.select_episodes_by_entity_fn)(namespace, entity_id, limit)
    }
```

- [ ] **Step 3: Implement the storage query in `src/storage.rs`**

Add a query builder and `SurrealDbClient` implementation using the existing service SQL verbatim:

```rust
fn build_select_episodes_by_entity_query(entity_id: &str, limit: i32) -> (String, Value) {
    (
        "SELECT * FROM episode WHERE episode_id IN (SELECT VALUE source_episode FROM fact WHERE fact_id IN (SELECT VALUE type::string(out) FROM edge WHERE in = <record> $entity_id AND relation = 'involved_in')) ORDER BY t_ref DESC LIMIT $limit".to_string(),
        serde_json::json!({
            "entity_id": entity_id,
            "limit": limit,
        }),
    )
}
```

```rust
    async fn select_episodes_by_entity(
        &self,
        namespace: &str,
        entity_id: &str,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        let (sql, vars) = build_select_episodes_by_entity_query(entity_id, limit);
        let surreal_val = match self.execute_query(&sql, Some(vars), namespace).await {
            Ok(value) => value,
            Err(MemoryError::Storage(message)) if is_missing_table_error(&message) => {
                return Ok(Vec::new());
            }
            Err(err) => return Err(err),
        };
        Ok(extract_records(surreal_to_json(surreal_val)))
    }
```

- [ ] **Step 4: Replace the direct SQL helper in `src/service/core.rs`**

Replace the private helper body with a thin storage call:

```rust
    async fn find_episodes_via_entity(
        &self,
        entity_id: &str,
        namespace: &str,
    ) -> Result<Vec<crate::models::Episode>, MemoryError> {
        let rows = self
            .db_client
            .select_episodes_by_entity(namespace, entity_id, 10)
            .await?;

        rows.into_iter()
            .map(serde_json::from_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| MemoryError::Storage(format!(
                "failed to parse linked episodes: {err}"
            )))
    }
```

- [ ] **Step 5: Re-run the behavior tests that depend on linked provenance**

Run: `cargo test --test explain_provenance explain_includes_linked_episodes_via_shared_entity -- --test-threads=1`
Expected: PASS.

Run: `cargo test select_episodes_by_entity_returns_linked_episodes -- --test-threads=1`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/storage.rs src/service/core.rs src/service/test_support.rs tests/explain_provenance.rs
git commit -m "refactor(storage): route entity episode lookup through db client"
```

---

## Wave 3 — Typed `fact.entity_links` migration with compatibility coverage (P1)

### Task 3: Migrate `fact.entity_links` from string IDs to typed `record<entity>` references

**Why this task exists:** The review note was directionally right but pointed at the wrong table. In `HEAD`, `entity_links` lives on `fact`, not on `entity`. The migration must therefore target `fact.entity_links`, while preserving current Rust/API behavior and avoiding any change to the public MCP contract.

**Files:**
- Create: `src/migrations/014_fact_entity_links_typed.surql`
- Modify: `src/storage.rs:852-875`
- Modify: `src/models.rs:357-377`
- Modify: `src/service/context.rs`
- Test: `tests/service_integration.rs`
- Test: `tests/explain_provenance.rs`

- [ ] **Step 1: Add a failing integration test for typed `entity_links`**

Add a focused test to `tests/service_integration.rs` that converts a seeded fact to typed record refs and asserts retrieval still works:

```rust
#[tokio::test]
async fn typed_fact_entity_links_still_support_retrieval() {
    let service = crate::common::make_service().await;

    let episode_id = service
        .ingest(
            memory_mcp::models::IngestRequest {
                source_type: "note".to_string(),
                source_id: "typed-links-1".to_string(),
                content: "Atlas budget increased".to_string(),
                t_ref: chrono::Utc::now(),
                scope: "org".to_string(),
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest");

    let extracted = service.extract(&episode_id, None).await.expect("extract");
    let entity_id = extracted.entities[0].entity_id.clone();

    let namespace = service.namespace_for_scope("org");
    service
        .db_client
        .query(
            "UPDATE fact SET entity_links = [<record> $entity_id] WHERE source_episode = $episode_id",
            Some(serde_json::json!({"entity_id": entity_id, "episode_id": episode_id})),
            &namespace,
        )
        .await
        .expect("force typed refs");

    let result = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "atlas".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 5,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
        .await
        .expect("assemble");

    assert!(!result.is_empty());
}
```

Run: `cargo test --test service_integration typed_fact_entity_links_still_support_retrieval -- --test-threads=1`
Expected: FAIL before the migration and compatibility shim are in place.

- [ ] **Step 2: Add the migration file and register it**

Create `src/migrations/014_fact_entity_links_typed.surql` with the additive schema change only:

```sql
-- Store entity links as typed record references while keeping the field optional.
DEFINE FIELD OVERWRITE entity_links ON fact TYPE option<array<record<entity>>> DEFAULT [];
```

Register it in `versioned_migrations()` immediately after `013_fact_index_keys_fts.surql`:

```rust
        MigrationScript {
            file_name: "014_fact_entity_links_typed.surql",
            sql: include_str!("migrations/014_fact_entity_links_typed.surql"),
        },
```

- [ ] **Step 3: Make `Fact.entity_links` robust to both string and record-shaped JSON**

Change the field in `src/models.rs`:

```rust
    #[serde(default, deserialize_with = "deserialize_entity_links")]
    pub entity_links: Vec<String>,
```

Add this helper next to the `Fact` model:

```rust
fn deserialize_entity_links<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<serde_json::Value>::deserialize(deserializer)?;
    Ok(values
        .into_iter()
        .filter_map(|value| match value {
            serde_json::Value::String(s) => Some(s),
            serde_json::Value::Object(map) => map
                .get("String")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .or_else(|| map.get("id").and_then(serde_json::Value::as_str).map(str::to_string))
                .or_else(|| map.get("tb").and_then(serde_json::Value::as_str).zip(map.get("id").and_then(serde_json::Value::as_str)).map(|(tb, id)| format!("{tb}:{id}"))),
            _ => None,
        })
        .collect())
}
```

- [ ] **Step 4: Re-run the retrieval paths that depend on `entity_links`**

Run: `cargo test --test service_integration typed_fact_entity_links_still_support_retrieval -- --test-threads=1`
Expected: PASS.

Run: `cargo test --test explain_provenance explain_includes_linked_episodes_via_shared_entity -- --test-threads=1`
Expected: PASS.

Run: `cargo test community_expansion_returns_empty_when_no_entity_links_match -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/migrations/014_fact_entity_links_typed.surql src/storage.rs src/models.rs src/service/context.rs tests/service_integration.rs tests/explain_provenance.rs
git commit -m "feat(storage): migrate fact entity links to typed record refs"
```

---

## Wave 4 — Remove only confirmed dead helpers from `embedding.rs` (P1)

### Task 4: Delete the test-only validation wrappers, keep live providers untouched

**Why this task exists:** The runtime providers are alive, so they must stay. The confirmed dead-ish code in `HEAD` is the trio of non-runtime helper functions guarded by `#[cfg_attr(not(test), allow(dead_code))]`: `parse_openai_embedding_response`, `parse_ollama_embedding_response`, and `validate_dimension`. They exist only for tests; the live provider paths use the `*_without_validation` variants.

**Files:**
- Modify: `src/service/embedding.rs:570-626`
- Modify: `src/service/embedding.rs:809-861`

- [ ] **Step 1: Add a failing cleanup assertion by running clippy with dead-code warnings visible**

Run: `cargo clippy --all-targets -- -W dead_code`
Expected: the only relevant dead-code findings in `src/service/embedding.rs` should point at the validated parsing wrappers and `validate_dimension`, not at the live provider impls.

- [ ] **Step 2: Delete the unused validation wrappers and simplify the tests to use the live parser helpers**

Remove these functions entirely:

```rust
fn parse_openai_embedding_response(
    body: &Value,
    expected_dimension: usize,
) -> Result<Vec<f64>, MemoryError>
```

```rust
fn parse_ollama_embedding_response(
    body: &Value,
    expected_dimension: usize,
) -> Result<Vec<f64>, MemoryError>
```

```rust
fn validate_dimension(
    embedding: Vec<f64>,
    expected_dimension: usize,
) -> Result<Vec<f64>, MemoryError>
```

Update the parser tests to use the live helpers directly:

```rust
#[test]
fn parse_openai_embedding_response_without_validation_reads_vector() {
    let embedding = parse_openai_embedding_response_without_validation(&json!({
        "data": [
            {"embedding": [0.1, 0.2, 0.3]}
        ]
    }))
    .expect("embedding");

    assert_eq!(embedding.len(), 3);
    assert_eq!(
        embedding,
        vec![0.2672612419124244, 0.5345224838248488, 0.8017837257372731]
    );
}
```

```rust
#[test]
fn parse_ollama_embedding_response_without_validation_reads_vector() {
    let embedding = parse_ollama_embedding_response_without_validation(
        &json!({"embedding": [0.4, 0.5, 0.6]}),
    )
    .expect("embedding");

    assert_eq!(embedding.len(), 3);
    assert_eq!(
        embedding,
        vec![0.4558423058385518, 0.5698028822981898, 0.6837634587578276]
    );
}
```

- [ ] **Step 3: Re-run the targeted unit tests and full lints**

Run: `cargo test parse_openai_embedding_response_without_validation_reads_vector --lib`
Expected: PASS.

Run: `cargo test parse_ollama_embedding_response_without_validation_reads_vector --lib`
Expected: PASS.

Run: `cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/service/embedding.rs
git commit -m "refactor(embedding): remove dead test-only parsing helpers"
```

---

## Wave 5 — MCP ergonomics without breaking the public contract (P2)

### Task 5: Tighten tool descriptions and app-specific invalid-parameter guidance while keeping the schema stable

**Why this task exists:** `guidance` already exists, so a new response field is unnecessary. The real friction in `HEAD` is narrower: `explain.context_items` is still a JSON string parameter, `open_app` is still a very wide flat launcher, and some descriptions/errors can do more to steer an agent correctly without changing the public schema.

**Files:**
- Modify: `src/mcp/handlers.rs:36-1650`
- Modify: `src/mcp/params.rs:34-170`
- Modify: `src/mcp/error.rs:1-63`
- Modify: `tests/tools_e2e.rs:1-450`

- [ ] **Step 1: Add a failing schema/description test for the public MCP surface**

Add or extend a handler/schema test with exact expectations for the descriptions:

```rust
#[tokio::test]
async fn explain_and_open_app_descriptions_are_head_aligned() {
    let mcp = create_test_mcp().await;

    let explain_tool = mcp.get_tool("explain").expect("explain tool");
    let open_app_tool = mcp.get_tool("open_app").expect("open_app tool");

    let explain_desc = explain_tool.description.expect("explain description");
    assert!(explain_desc.contains("JSON-encoded array string"));
    assert!(explain_desc.contains("Do NOT use this tool to search memory"));

    let open_app_desc = open_app_tool.description.expect("open_app description");
    assert!(open_app_desc.contains("inspector -> `target_type` + `target_id`"));
    assert!(open_app_desc.contains("graph -> `from_entity_id` + `to_entity_id`"));
    assert!(open_app_desc.contains("guidance"));
}
```

Run: `cargo test explain_and_open_app_descriptions_are_head_aligned -- --test-threads=1`
Expected: FAIL until the descriptions are updated.

- [ ] **Step 2: Rewrite the descriptions in `src/mcp/handlers.rs` without changing parameter types**

Use these tightened descriptions as the new source of truth:

```rust
description = "Store a new episode in long-term memory. Use this tool when you need to persist raw source text as memory before any downstream extraction. Do NOT use this tool for retrieval. Arguments must include ISO 8601 `t_ref` and a memory `scope`. Returns the created or existing `episode_id`, plus `guidance` telling the agent what to do next."
```

```rust
description = "Extract entities, facts, and relationships from remembered content. Use this tool when you need structured information from an existing `episode_id` or from new inline `content`/`text`. Do NOT use this tool for retrieval. If inline content is provided, the server ingests it first and then extracts. Returns extracted entities, facts, links, and `guidance` for the next step."
```

```rust
description = "Explain context items with provenance-ready citations. Use this tool when you already have selected context items and need source snippets for a final answer. Do NOT use this tool to search memory. `context_items` must be a JSON-encoded array string. Accepted item forms: source ID strings, objects with `source_episode`, or objects with `id`. Returns citation-ready items and `guidance`."
```

```rust
description = "Open a Memory MCP app through the minimal public launcher. Use this tool only when an interactive app workflow is required and no canonical memory tool already matches the intent. Required fields depend on `app`: inspector -> `target_type` + `target_id`; diff -> `as_of_left` + `as_of_right`; graph -> `from_entity_id` + `to_entity_id`; ingestion_review -> `scope` plus optional `source_text` or `draft_episode_id`; lifecycle -> `scope` only. Returns `session_id`, `resource_uri`, `fallback`, and `guidance`."
```

- [ ] **Step 3: Add a small helper for clearer app-specific missing-parameter errors**

Add this helper to `src/mcp/handlers.rs`:

```rust
    fn missing_app_field(app: &str, field: &str) -> ErrorData {
        Self::invalid_params(format!(
            "`{field}` is required for {app}. Re-check the open_app contract for that app and retry."
        ))
    }
```

Then replace repeated error construction, for example:

```rust
let from_entity_id = p
    .from_entity_id
    .as_deref()
    .ok_or_else(|| Self::missing_app_field("graph", "from_entity_id"))?;
```

```rust
let as_of_left = p
    .as_of_left
    .as_deref()
    .ok_or_else(|| Self::missing_app_field("diff", "as_of_left"))?;
```

- [ ] **Step 4: Re-run the MCP E2E tests that depend on `guidance` and `explain`**

Run: `cargo test --test tools_e2e -- --test-threads=1`
Expected: PASS.

Run: `cargo test explain_and_open_app_descriptions_are_head_aligned -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/mcp/handlers.rs src/mcp/params.rs src/mcp/error.rs tests/tools_e2e.rs
git commit -m "docs(mcp): reduce tool description friction without schema changes"
```

---

## Wave 6 — Semantic + entity-graph retrieval activation (P0)

### Task 6a: Enable local embedding provider by default in eval config

**Why this task exists:** `collect_semantic_facts` is already wired into `assemble_context` and uses `select_facts_ann` (HNSW ANN index) when a provider is active. However, in eval runs the provider is inactive because `EMBEDDINGS_ENABLED` defaults to false. `LocalCandleEmbeddingProvider` already exists and can run without any external API. Enabling it in the eval test setup gives semantic retrieval for free.

**Files:**
- Modify: `tests/common/mod.rs` (or `tests/embedded_support.rs`) — the `make_service()` factory
- Test: `tests/eval_retrieval.rs`

- [ ] **Step 1: Add a failing test asserting semantic recall fires**

```rust
#[tokio::test]
async fn semantic_retrieval_fires_when_local_provider_enabled() {
    let service = crate::common::make_service_with_local_embeddings().await;

    service
        .ingest(memory_mcp::models::IngestRequest {
            source_type: "note".to_string(),
            source_id: "semantic-test-1".to_string(),
            content: "Annual remuneration package was increased by the board.".to_string(),
            t_ref: chrono::Utc::now(),
            scope: "org".to_string(),
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
        })
        .await
        .expect("ingest");

    // Query uses a synonym ("salary raise") — FTS would miss, semantic should hit
    let results = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "salary raise".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 5,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
        .await
        .expect("assemble");

    assert!(
        results.iter().any(|item| item.content.contains("remuneration")),
        "semantic retrieval must surface fact about remuneration for query 'salary raise'"
    );
}
```

Run: `cargo test --test eval_retrieval semantic_retrieval_fires_when_local_provider_enabled -- --test-threads=1`
Expected: FAIL (no `make_service_with_local_embeddings`).

- [ ] **Step 2: Add `make_service_with_local_embeddings` to `tests/common/mod.rs`**

```rust
pub async fn make_service_with_local_embeddings() -> memory_mcp::service::MemoryService {
    let db_client = crate::embedded_support::connect_in_memory().await;
    let embedding_provider = memory_mcp::service::create_local_embedding_provider()
        .await
        .expect("local embedding provider");
    memory_mcp::service::MemoryService::new_with_embedding_provider(
        std::sync::Arc::new(db_client),
        vec!["org".to_string()],
        "warn".to_string(),
        50,
        100,
        std::sync::Arc::new(embedding_provider),
        memory_mcp::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
        std::sync::Arc::new(memory_mcp::service::AnnoEntityExtractor::new().expect("anno")),
    )
    .expect("service")
}
```

Note: verify the actual function/method name for creating a local embedding provider by searching `src/service/embedding.rs` for `fn.*local\|LocalCandle` before implementing.

- [ ] **Step 3: Make the standard eval suite use `make_service_with_local_embeddings`**

In `tests/eval_retrieval.rs`, update the service factory in the main eval case to use the embedding-enabled variant so semantic ANN contributes to recall metrics.

- [ ] **Step 4: Re-run the test**

Run: `cargo test --test eval_retrieval semantic_retrieval_fires_when_local_provider_enabled -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tests/common/mod.rs tests/eval_retrieval.rs
git commit -m "test(eval): enable local embedding provider for semantic retrieval baseline"
```

---

### Task 6b: Wire entity resolution + 1-hop graph traversal into retrieval

**Why this task exists:** `assemble_context` currently uses FTS + alias expansion + community facts + semantic ANN. It does **not** use the entity graph: if a query mentions "Charles Dickens", it matches text literally but doesn't resolve `entity:charles-dickens`, walk its 1-hop edges, and retrieve facts for neighbor entities (e.g. a spouse entity). The building blocks all exist — `select_entity_lookup`, `select_entities_batch`, `select_edge_neighbors`, `select_facts_by_entity_links` — but are not composed for query-time graph expansion. This is the server-side fix that feeds 1-2-hop multi-hop questions with relevant facts.

**Files:**
- Modify: `src/service/context.rs`
- Modify: `src/service/test_support.rs` (if needed for collect helper mocks)
- Test: `tests/eval_retrieval.rs`

- [ ] **Step 1: Add a failing integration test for entity-aware graph expansion**

Add a test in `tests/eval_retrieval.rs` that seeds a two-entity chain and asserts the downstream entity's linked fact appears in `assemble_context` results:

```rust
#[tokio::test]
async fn assemble_context_expands_via_entity_graph() {
    // Seed: entity:dickens -[spouse]-> entity:catherine
    // Fact about catherine with entity_link = entity:catherine
    // Query: "Charles Dickens"
    // Expected: fact about catherine appears (1-hop graph expansion)

    let service = crate::common::make_service().await;

    service
        .ingest(memory_mcp::models::IngestRequest {
            source_type: "note".to_string(),
            source_id: "dickens-catherine-1".to_string(),
            content: "Catherine Hogarth, wife of Charles Dickens, lived in London.".to_string(),
            t_ref: chrono::Utc::now(),
            scope: "org".to_string(),
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
        })
        .await
        .expect("ingest");

    let results = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "Charles Dickens spouse".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 5,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
        .await
        .expect("assemble");

    assert!(
        results
            .iter()
            .any(|item| item.content.contains("Catherine")),
        "entity-graph expanded fact about Catherine should appear for Charles Dickens query"
    );
}
```

Run: `cargo test --test eval_retrieval assemble_context_expands_via_entity_graph -- --test-threads=1`
Expected: PASS via FTS if text overlap is sufficient, but validates the plumbing. After Step 3, the entity graph path should be the one triggering for queries where FTS alone would miss.

- [ ] **Step 2: Add `collect_entity_expansion_facts` helper to `src/service/context.rs`**

Add this helper alongside `collect_semantic_facts` and `collect_community_facts`:

```rust
struct CollectEntityExpansionFactsRequest<'a> {
    namespace: &'a str,
    scope: &'a str,
    cutoff_iso: &'a str,
    query: &'a str,
    access: &'a AccessContext,
    excluded_fact_ids: &'a HashSet<String>,
    budget: i32,
}

async fn collect_entity_expansion_facts(
    service: &crate::service::MemoryService,
    request: CollectEntityExpansionFactsRequest<'_>,
) -> Result<Vec<(crate::models::Fact, String)>, MemoryError> {
    // Step A: extract candidate entity names from query text via NER
    let entity_names = service
        .entity_extractor
        .extract(request.query)
        .await
        .unwrap_or_default();

    if entity_names.is_empty() {
        return Ok(Vec::new());
    }

    // Step B: resolve names to entity_ids (batch lookup)
    let normalized: Vec<String> = entity_names
        .iter()
        .map(|e| super::normalize_name(&e.text))
        .collect();

    let seed_entities = service
        .db_client
        .select_entities_batch(request.namespace, &normalized)
        .await
        .unwrap_or_default();

    if seed_entities.is_empty() {
        return Ok(Vec::new());
    }

    // Step C: expand to 1-hop neighbors via entity graph edges
    let cutoff = request.cutoff_iso;
    let mut all_entity_ids: Vec<String> = seed_entities
        .iter()
        .filter_map(|v| v.get("entity_id").and_then(|id| id.as_str()).map(str::to_string))
        .collect();

    for entity in &seed_entities {
        let Some(entity_id) = entity.get("entity_id").and_then(|v| v.as_str()) else {
            continue;
        };
        let neighbors = service
            .db_client
            .select_edge_neighbors(request.namespace, entity_id, cutoff, GraphDirection::Both)
            .await
            .unwrap_or_default();

        for neighbor in neighbors {
            if let Some(neighbor_id) = neighbor
                .get("neighbor_id")
                .and_then(|v| v.as_str())
            {
                if !all_entity_ids.contains(&neighbor_id.to_string()) {
                    all_entity_ids.push(neighbor_id.to_string());
                }
            }
        }
    }

    if all_entity_ids.is_empty() {
        return Ok(Vec::new());
    }

    // Step D: retrieve facts for the full entity set
    let search_limit = request.budget.max(1) * 3;
    let fact_records = service
        .db_client
        .select_facts_by_entity_links(
            request.namespace,
            request.scope,
            cutoff,
            &all_entity_ids,
            search_limit,
        )
        .await
        .unwrap_or_default();

    Ok(fact_records
        .into_iter()
        .filter_map(|record| {
            let fact = super::episode::fact_from_record(&record)?;
            if fact.scope != request.scope
                || request.excluded_fact_ids.contains(&fact.fact_id)
                || !fact_allowed_by_policy(&fact, request.access)
                || !fact_is_active_at(&fact, chrono::Utc::now())
            {
                return None;
            }
            Some((fact, "entity-graph expansion".to_string()))
        })
        .collect())
}
```

Note: check the real field name returned by `select_edge_neighbors` for neighbor ID — use `grep_search "neighbor_id\|out\b" src/storage.rs` to verify before implementing.

- [ ] **Step 3: Wire the helper into `assemble_context`**

In the `if let Some(query) =` branch of `assemble_context`, after `excluded_fact_ids` is computed from direct + community + semantic facts, add:

```rust
        let entity_expansion_facts = collect_entity_expansion_facts(
            service,
            CollectEntityExpansionFactsRequest {
                namespace: &namespace,
                scope: &request.scope,
                cutoff_iso: &cutoff_iso,
                query,
                access: &access,
                excluded_fact_ids: &excluded_fact_ids,
                budget: request.budget,
            },
        )
        .await?;
```

Pass `entity_expansion_facts` into `build_ranked_context_facts` by merging it with semantic_facts or as a new parameter — match the signature and ranking weight used for community facts.

- [ ] **Step 4: Re-run retrieval eval and unit tests**

Run: `cargo test --test eval_retrieval -- --test-threads=1`
Expected: PASS. Entity expansion facts appear for entity-linked queries.

Run: `cargo test --lib context -- --test-threads=1`
Expected: all existing context unit tests still PASS.

- [ ] **Step 5: Commit**

```bash
git add src/service/context.rs tests/eval_retrieval.rs
git commit -m "feat(retrieval): entity-graph expansion in assemble_context"
```

---

## Wave 7 — Expose `t_ref`/`t_valid` in `AssembledContextItem` (P1)

### Task 7: Add temporal fields to `AssembledContextItem` for LLM temporal reasoning

**Why this task exists:** Temporal questions like "How many days before X happened?" require the LLM to compute date differences between events. `AssembledContextItem` currently has no timestamp fields — the only temporal signal is the `confidence` value (which encodes decay, not event time). Adding `t_ref` and `t_valid` to the response gives the LLM the raw timestamps it needs to perform date math. This is an **additive** MCP response change; it does not remove or rename existing fields.

**Files:**
- Modify: `src/models.rs:262-271`
- Modify: `src/service/context.rs` (result mapping at end of `assemble_context`)
- Test: `tests/tools_e2e.rs`

- [ ] **Step 1: Add a failing test asserting `t_ref` is populated in context results**

Add to `tests/tools_e2e.rs`:

```rust
#[tokio::test]
async fn assemble_context_result_items_expose_t_ref() {
    let mcp = create_test_mcp().await;

    let ingest_resp = mcp
        .ingest(Parameters(serde_json::from_value(json!({
            "source_type": "note",
            "source_id": "temporal-e2e-1",
            "content": "Project kickoff on 2026-01-15.",
            "t_ref": "2026-01-15T00:00:00Z",
            "scope": "org"
        })).unwrap()))
        .await
        .expect("ingest");

    let episode_id = ingest_resp.result["episode_id"]
        .as_str()
        .expect("episode_id");

    mcp.extract(Parameters(serde_json::from_value(json!({
        "episode_id": episode_id,
        "scope": "org"
    })).unwrap()))
    .await
    .expect("extract");

    let ctx = mcp
        .assemble_context(Parameters(serde_json::from_value(json!({
            "query": "Project kickoff",
            "scope": "org",
            "budget": 3
        })).unwrap()))
        .await
        .expect("assemble");

    for item in &ctx.result {
        assert!(
            item.get("t_ref").is_some() || item.get("t_valid").is_some(),
            "each context item must expose at least t_ref or t_valid"
        );
    }
}
```

Run: `cargo test --test tools_e2e assemble_context_result_items_expose_t_ref -- --test-threads=1`
Expected: FAIL — no `t_ref` field in current `AssembledContextItem`.

- [ ] **Step 2: Add `t_ref` and `t_valid` to `AssembledContextItem` in `src/models.rs`**

```rust
/// A ranked context item returned by the MCP `assemble_context` tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AssembledContextItem {
    pub fact_id: String,
    pub content: String,
    pub quote: String,
    pub source_episode: String,
    pub confidence: f64,
    pub provenance: serde_json::Value,
    pub rationale: String,
    /// Temporal anchor of the fact — ISO 8601. Use this to compute date differences.
    pub t_ref: Option<chrono::DateTime<chrono::Utc>>,
    /// Validity interval start — ISO 8601. May differ from t_ref for retconned facts.
    pub t_valid: Option<chrono::DateTime<chrono::Utc>>,
}
```

- [ ] **Step 3: Populate `t_ref`/`t_valid` in the result mapping in `src/service/context.rs`**

In the `.map(|ranked| { ... })` block at the end of `assemble_context`, add:

```rust
            AssembledContextItem {
                fact_id: ranked.fact.fact_id,
                content: ranked.fact.content,
                quote: ranked.fact.quote,
                source_episode: ranked.fact.source_episode,
                confidence,
                provenance: ranked.fact.provenance,
                rationale: ranked.rationale,
                t_ref: ranked.fact.t_ref,
                t_valid: ranked.fact.t_valid,
            }
```

Verify that `Fact` already has `t_ref: Option<DateTime<Utc>>` and `t_valid: Option<DateTime<Utc>>` — use `grep_search "t_ref|t_valid" src/models.rs` to confirm field types before writing.

- [ ] **Step 4: Fix existing tests that construct `AssembledContextItem` directly**

Search for direct struct literals:

```bash
grep -rn "AssembledContextItem {" tests/ src/
```

Add `t_ref: None, t_valid: None` to any literal that needs it, or update struct update syntax to stay compatible.

- [ ] **Step 5: Re-run all context and E2E tests**

Run: `cargo test --test tools_e2e assemble_context_result_items_expose_t_ref -- --test-threads=1`
Expected: PASS.

Run: `cargo test --test tools_e2e -- --test-threads=1`
Expected: all PASS.

Run: `cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/models.rs src/service/context.rs tests/tools_e2e.rs
git commit -m "feat(context): expose t_ref and t_valid on AssembledContextItem"
```

---

## Wave 8 — In-memory SurrealDB eval stability (P2)

### Task 8: Fix embedded in-memory degradation after ~20 eval cases

**Why this task exists:** SurrealDB embedded (in-memory) degrades after approximately 20 eval cases in `tests/eval_retrieval.rs` and `longmem_acceptance.rs`. This manifests as retrieval returning stale or empty results for cases that should match. The root cause is likely index degradation in the HNSW embedding index after many inserts without compaction, or connection-level state buildup. The fix must not require a persistent file-backed DB for eval tests (that would slow CI significantly).

**Files:**
- Modify: `tests/embedded_support.rs`
- Modify: `tests/eval_retrieval.rs`
- Modify: `tests/longmem_acceptance.rs`

- [ ] **Step 1: Reproduce the degradation with a controlled count**

Add a diagnostic test to `tests/eval_retrieval.rs`:

```rust
#[tokio::test]
async fn in_memory_db_handles_25_sequential_ingests_without_degradation() {
    let service = crate::common::make_service().await;

    for i in 0..25 {
        service
            .ingest(memory_mcp::models::IngestRequest {
                source_type: "note".to_string(),
                source_id: format!("stability-{i}"),
                content: format!("Stability test fact number {i} about project gamma."),
                t_ref: chrono::Utc::now(),
                scope: "org".to_string(),
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            })
            .await
            .unwrap_or_else(|_| panic!("ingest {i} failed"));
    }

    let results = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "project gamma".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 10,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
        .await
        .expect("assemble after 25 ingests");

    assert!(
        !results.is_empty(),
        "retrieval must return results after 25 sequential ingests — in-memory DB degraded"
    );
}
```

Run: `cargo test --test eval_retrieval in_memory_db_handles_25_sequential_ingests_without_degradation -- --test-threads=1`
Expected: FAIL if degradation is present (empty results). If it passes, the degradation is case-specific — investigate `longmem_acceptance.rs` instead.

- [ ] **Step 2: Add per-batch DB recycling to `tests/embedded_support.rs`**

If the degradation reproduces: add a `make_fresh_service()` helper that creates a completely new in-memory SurrealDB connection, so the eval harness can recycle every N cases:

```rust
/// Creates a fresh in-memory MemoryService. Use when you need to isolate
/// eval cases that would otherwise share cumulative state.
pub async fn make_fresh_service() -> crate::service::MemoryService {
    crate::common::make_service().await
}
```

Update the eval test loop in `tests/eval_retrieval.rs` and `tests/longmem_acceptance.rs` to call `make_fresh_service()` at the start of each test case rather than sharing a single service across the full eval loop.

- [ ] **Step 3: If batch recycling is insufficient — add HNSW index rebuild after each batch**

In `embedded_support.rs`, after ingestion of each batch, execute a raw SurrealDB REBUILD INDEX query:

```rust
/// Forces a HNSW index rebuild on the fact table.
/// Call when in-memory embedding index degrades after many sequential writes.
pub async fn rebuild_hnsw_index(service: &crate::service::MemoryService, namespace: &str) {
    // Only run if embeddings are enabled, otherwise no-op
    if !service.embedding_provider.is_enabled() {
        return;
    }
    // Fire-and-forget: do not propagate errors to keep eval harness lean
    let _ = service
        .db_client
        .execute_raw("REBUILD INDEX fact_embedding_hnsw ON fact", namespace)
        .await;
}
```

Call `rebuild_hnsw_index` every 10 cases in the eval loop.

Note: check whether `DbClient` / `SurrealDbClient` exposes `execute_raw` or a similarly named raw-query method — grep for `execute_query` and `raw` in `src/storage.rs` to find the right method. Add a `DbClient` default-no-op or behind `#[cfg(test)]` if the method doesn't belong on the production trait.

- [ ] **Step 4: Verify eval stability**

Run: `cargo test --test eval_retrieval -- --test-threads=1`
Expected: PASS, no empty results after the recycling or index rebuild is applied.

Run: `cargo test --test longmem_acceptance -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tests/embedded_support.rs tests/eval_retrieval.rs tests/longmem_acceptance.rs
git commit -m "test(eval): fix in-memory SurrealDB degradation with connection recycling"
```

---

## Final Verification Pipeline

After all waves, run the full repository verification pipeline required by the repo constitution:

```bash
cargo fmt --all
cargo check
cargo clippy --all-targets -- -D warnings
cargo test
cargo doc --no-deps
```

All five commands must pass before the remediation work is considered complete.

---

## Self-Review Checklist

- **Spec coverage:** This plan covers all residual items that still exist in `HEAD`: backlog rebasing, `DbClient` boundary cleanup, typed `fact.entity_links`, narrow `embedding.rs` cleanup, MCP guidance friction, entity-graph expansion retrieval, temporal field exposure, and in-memory eval stability.
- **Eval gap coverage:** The critical retrieval gaps from LongMemEval/MAB evaluation are addressed -- graph-based retrieval (Wave 6), temporal field exposure for LLM date math (Wave 7), in-memory stability (Wave 8). Multi-hop reasoning chains and abstention are explicitly called out as out-of-scope (LLM-layer concerns / dataset filtering issue).
- **Placeholder scan:** No `TODO`, `TBD`, or implicit 'figure it out later' steps remain; each task names files, concrete code snippets, commands, and expected outcomes.
- **Type consistency:** The plan preserves the public MCP contract (`guidance` stays canonical; `ExplainParams.context_items` stays `String`; new `t_ref`/`t_valid` fields are additive and optional, not breaking).
- **Building-block inventory:** Wave 6 uses only existing `DbClient` methods (`select_entities_batch`, `select_edge_neighbors`, `select_facts_by_entity_links`) -- no new storage methods required. Wave 7 reads existing `Fact.t_ref`/`Fact.t_valid` fields -- no migration required.
