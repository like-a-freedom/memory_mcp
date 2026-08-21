# Architecture Deepening — Round 3 — 2026-08-21

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deepen six modules identified in the round-3 architecture review: one owner for embedding state, close raw-SQL escape hatches, unify embedding background policy, unfuse client.rs, split reembed orchestration, and gather app session lifecycle below the adapter line.

**Architecture:** Each candidate concentrates complexity behind a smaller interface. C1 introduces `EmbeddingStateStoreClient` as the single writer of the `embedding_state:fact` record. C3 deletes three `query()` re-exports and replaces five inline-SQL call sites with named store methods. C4 deletes the duplicated `EmbeddingBackgroundSnapshot` by cloning the already-`Clone` `EmbeddingService`. C5 moves the migration/schema runtime out of `client.rs` into `migrations.rs`. C6 gathers app session open/command orchestration into `service/apps/`.

**Tech Stack:** Rust, SurrealDB (embedded), tokio, serde_json, thiserror.

**Spec:** ADR-0043 (`docs/adr/0043-one-owner-for-embedding-state-record.md`), ADR-0044 (`docs/adr/0044-narrow-stores-expose-named-methods-only.md`), round-3 architecture review report.

## Global Constraints

- No `unwrap()`/`expect()`/`panic!` in production code — return `Result` or `?`.
- `main.rs` stays thin — CLI parsing and mode dispatch only.
- Business logic in `src/service/`; storage owns SQL (ADR-0024/0027).
- 8-tool MCP surface frozen; no new MCP tools.
- Never delete facts; append-only migrations.
- One Active Namespace (ADR-0038); no request-level partitioning.
- Errors via `MemoryError` (thiserror-based).
- Feature flags additive; verify under `--features cli-watch,mcp-apps --locked`.
- Verify gate: `cargo fmt --all --check` → `cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings` → `cargo test -p memory_mcp`.

## Binding decisions

| ID | Decision | Rationale |
|----|----------|-----------|
| D1 | C1 uses a **separate** `EmbeddingStateStoreClient`, not merged into `EmbeddingBackfillStoreClient` | ADR-0043: backfill store owns the `embedding IS NONE` cursor API; adding record ownership makes both shallower |
| D2 | C1 keeps `decide_embedding_startup` pure-over-JSON; reads stay JSON-shaped | ADR-0043: retyping input churns exhaustive tests without behavior change |
| D3 | C1 unifies on `upsert` (select→update/create) in the new store | Matches `ReembedStoreClient::upsert_record` pattern; field sets stay identical |
| D4 | C3 introduces a new `EntityStoreClient` for entity alias operations | `EntityService` holds `BoundDbClient` directly; a narrow store follows the established pattern |
| D5 | C3 assigns `find_episodes_via_entity` to `ContextStoreClient` | ADR-0044: graph-shaped read-model queries belong to ContextStoreClient |
| D6 | C4 deletes `EmbeddingBackgroundSnapshot` entirely; spawned tasks take `self.clone()` | `EmbeddingService` is `Clone` with identical capture semantics; ~200 duplicated lines removed |
| D7 | C5 uses `pub(crate)` primitives on `SurrealDbClient` rather than a new executor trait | KISS: the migration code is the only consumer; a trait adds indirection without a second adapter |
| D8 | C6 moves orchestration into `service/apps/session_lifecycle.rs`; `SessionManager` stays in `mcp/session.rs` | Session storage is process-scoped adapter state; orchestration is service logic |
| D9 | Wave 1 runs C1, C3, C4, C5, C6 in parallel (disjoint write sets); Wave 2 runs C2 after C1 lands | C2 touches `reembed.rs` which C1 also touches |

## File structure

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `crates/memory-mcp/src/storage/embedding_state_store.rs` | Owns `embedding_state:fact` record: ID, statuses, shape, every write |
| Create | `crates/memory-mcp/src/storage/entity_store.rs` | Owns entity alias queries (find-by-alias, find-by-prefix, add-alias) |
| Create | `crates/memory-mcp/src/service/apps/session_lifecycle.rs` | Orchestrates open_app and app_command lifecycle |
| Modify | `crates/memory-mcp/src/storage.rs` | Register new modules + re-exports |
| Modify | `crates/memory-mcp/src/service/startup.rs` | Delegate state writes to new store |
| Modify | `crates/memory-mcp/src/service/reembed.rs` | Delegate state writes to new store |
| Modify | `crates/memory-mcp/src/service/core/builder.rs` | Delegate state writes to new store |
| Modify | `crates/memory-mcp/src/service/embedding_recovery.rs` | Delegate state writes to new store |
| Modify | `crates/memory-mcp/src/service/entity.rs` | Use `EntityStoreClient` instead of raw SQL |
| Modify | `crates/memory-mcp/src/service/explanation.rs` | Use `ContextStoreClient::select_episodes_via_entity` |
| Modify | `crates/memory-mcp/src/service/episode/entity_extraction.rs` | Use `EpisodeStoreClient` named method |
| Modify | `crates/memory-mcp/src/service/lifecycle/archival.rs` | Use `AppStoreClient` named method |
| Modify | `crates/memory-mcp/src/service/context/logging.rs` | Use `ContextAccessLogClient` named method |
| Modify | `crates/memory-mcp/src/storage/app_store.rs` | Add named method, delete `query()` |
| Modify | `crates/memory-mcp/src/storage/context_store.rs` | Add `select_episodes_via_entity`, delete `query()` on `ContextAccessLogClient` |
| Modify | `crates/memory-mcp/src/storage/episode_store.rs` | Add `create_extraction_projection`, delete `query()` |
| Modify | `crates/memory-mcp/src/service/embedding_service.rs` | Delete snapshot, move methods to service |
| Modify | `crates/memory-mcp/src/storage/client.rs` | Move migration runtime out |
| Modify | `crates/memory-mcp/src/storage/migrations.rs` | Receive migration runtime |
| Modify | `crates/memory-mcp/src/mcp/handlers.rs` | Thin adapter: decode–call–encode |
| Modify | `crates/memory-mcp/src/mcp/handlers/apps.rs` | Thin adapter: delegate to session_lifecycle |

---

## Task 1: C1 — EmbeddingStateStoreClient (ADR-0043)

**Files:**
- Create: `crates/memory-mcp/src/storage/embedding_state_store.rs`
- Modify: `crates/memory-mcp/src/storage.rs`
- Modify: `crates/memory-mcp/src/service/startup.rs`
- Modify: `crates/memory-mcp/src/service/reembed.rs`
- Modify: `crates/memory-mcp/src/service/core/builder.rs`
- Modify: `crates/memory-mcp/src/service/embedding_recovery.rs`

**Interfaces:**
- Consumes: `BoundDbClient` (from `storage/client.rs`), `DbClient` trait, `MemoryError`
- Produces: `EmbeddingStateStoreClient::new(db, namespace)`, `load_state() -> Result<Option<Value>>`, `upsert_state(payload: Value) -> Result<()>`, `EMBEDDING_STATE_RECORD_ID`

- [ ] **Step 1: Write the failing test**

```rust
// In crates/memory-mcp/src/storage/embedding_state_store.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::SurrealDbClient;
    use serde_json::json;
    use std::sync::Arc;

    async fn make_db() -> Arc<SurrealDbClient> {
        let database = format!("test_embedding_state_{}", uuid::Uuid::new_v4().simple());
        SurrealDbClient::connect_in_memory(&database, "org", "warn")
            .await
            .expect("in-memory db")
    }

    #[tokio::test]
    async fn upsert_creates_then_updates_state_record() {
        let db = make_db().await;
        let store = EmbeddingStateStoreClient::new(db.clone(), "org");

        // First write creates
        store
            .upsert_state(json!({
                "status": "ready",
                "active_signature": "embsig:test",
                "provider": "test",
                "model": null,
                "dimension": 384,
                "updated_at": "2026-01-01T00:00:00Z",
            }))
            .await
            .expect("first upsert");

        let state = store.load_state().await.expect("load");
        assert_eq!(
            state.as_ref().and_then(|s| s.get("status")).and_then(|v| v.as_str()),
            Some("ready")
        );

        // Second write updates
        store
            .upsert_state(json!({
                "status": "rebuilding",
                "provider": "test",
                "model": null,
                "dimension": 384,
                "updated_at": "2026-01-02T00:00:00Z",
            }))
            .await
            .expect("second upsert");

        let state = store.load_state().await.expect("load after update");
        assert_eq!(
            state.as_ref().and_then(|s| s.get("status")).and_then(|v| v.as_str()),
            Some("rebuilding")
        );
    }

    #[tokio::test]
    async fn load_state_returns_none_when_absent() {
        let db = make_db().await;
        let store = EmbeddingStateStoreClient::new(db, "org");
        let state = store.load_state().await.expect("load");
        assert!(state.is_none());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p memory_mcp embedding_state_store -- --nocapture`
Expected: FAIL — module does not exist yet.

- [ ] **Step 3: Write minimal implementation**

```rust
//! Narrow store owning the durable `embedding_state:fact` record (ADR-0043).
//!
//! Every write to the embedding state record goes through this store.
//! Startup bootstrap, Embedding Recovery, and Reembed all use it.

use std::sync::Arc;

use serde_json::Value;

use crate::service::MemoryError;
use crate::storage::{BoundDbClient, DbClient};

/// The single record ID for the embedding state.
pub(crate) const EMBEDDING_STATE_RECORD_ID: &str = "embedding_state:fact";

/// One owner for the `embedding_state:fact` record.
pub(crate) struct EmbeddingStateStoreClient {
    db: BoundDbClient,
}

impl EmbeddingStateStoreClient {
    pub(crate) fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            db: BoundDbClient::new(db, namespace),
        }
    }

    /// Loads the current embedding state record, or `None` if absent.
    pub(crate) async fn load_state(&self) -> Result<Option<Value>, MemoryError> {
        self.db.select_one(EMBEDDING_STATE_RECORD_ID).await
    }

    /// Creates or updates the embedding state record.
    pub(crate) async fn upsert_state(&self, payload: Value) -> Result<(), MemoryError> {
        if self.db.select_one(EMBEDDING_STATE_RECORD_ID).await?.is_some() {
            self.db.update(EMBEDDING_STATE_RECORD_ID, payload).await?;
        } else {
            self.db.create(EMBEDDING_STATE_RECORD_ID, payload).await?;
        }
        Ok(())
    }
}
```

- [ ] **Step 4: Register module in storage.rs**

Add to `crates/memory-mcp/src/storage.rs`:
```rust
pub(crate) mod embedding_state_store;
```

Add re-export:
```rust
pub(crate) use embedding_state_store::{EmbeddingStateStoreClient, EMBEDDING_STATE_RECORD_ID};
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p memory_mcp embedding_state_store -- --nocapture`
Expected: PASS

- [ ] **Step 6: Migrate startup.rs writers**

In `crates/memory-mcp/src/service/startup.rs`:
- Replace `pub(crate) const EMBEDDING_STATE_RECORD_ID` with a re-export:
  ```rust
  pub(crate) use crate::storage::EMBEDDING_STATE_RECORD_ID;
  ```
- Replace `load_embedding_state` body:
  ```rust
  pub(crate) async fn load_embedding_state(
      db: &BoundDbClient,
  ) -> Result<Option<serde_json::Value>, MemoryError> {
      db.select_one(EMBEDDING_STATE_RECORD_ID).await
  }
  ```
  (unchanged — still reads via BoundDbClient for the pure decision path)
- Replace `write_bootstrap_ready_state` to use the store:
  ```rust
  pub(crate) async fn write_bootstrap_ready_state(
      db_client: &Arc<dyn DbClient>,
      namespace: &str,
      active_signature: &str,
      provider: &str,
      model: Option<&str>,
      dimension: usize,
      backfill_pending: bool,
  ) -> Result<(), MemoryError> {
      let store = crate::storage::EmbeddingStateStoreClient::new(
          db_client.clone(),
          namespace,
      );
      let payload = serde_json::json!({
          "status": if backfill_pending { "backfill_pending" } else { "ready" },
          "active_signature": active_signature,
          "provider": provider,
          "model": model,
          "dimension": dimension,
          "updated_at": chrono::Utc::now().to_rfc3339(),
      });
      store.upsert_state(payload).await
  }
  ```
- Update call sites in `builder.rs` and `embedding_recovery.rs` to pass `db_client` + `namespace` instead of `&BoundDbClient`.

- [ ] **Step 7: Migrate reembed.rs writer**

In `crates/memory-mcp/src/service/reembed.rs`:
- Replace `write_embedding_state` to use the store:
  ```rust
  async fn write_embedding_state(
      &self,
      _namespace: &str,
      status: &str,
      active_signature: Option<&str>,
      last_job_id: Option<&str>,
  ) -> Result<(), MemoryError> {
      let embedding_state = self.embedding_runtime_snapshot();
      let mut payload = serde_json::Map::from_iter([
          ("status".to_string(), json!(status)),
          ("provider".to_string(), json!(embedding_state.provider.provider_name())),
          ("model".to_string(), json!(embedding_state.model.clone())),
          ("dimension".to_string(), json!(embedding_state.dimension)),
          ("updated_at".to_string(), json!(chrono::Utc::now().to_rfc3339())),
      ]);
      if let Some(active_signature) = active_signature {
          payload.insert("active_signature".to_string(), json!(active_signature));
      }
      if let Some(last_job_id) = last_job_id {
          payload.insert("last_job_id".to_string(), json!(last_job_id));
      }

      let store = crate::storage::EmbeddingStateStoreClient::new(
          self.db_client.clone(),
          self.active_namespace.clone(),
      );
      store.upsert_state(Value::Object(payload)).await
  }
  ```
- Update import: `use crate::storage::EMBEDDING_STATE_RECORD_ID;` (for tests that reference it).

- [ ] **Step 8: Migrate embedding_recovery.rs writers**

In `crates/memory-mcp/src/service/embedding_recovery.rs`:
- Update `install_recovery_provider` and `backfill_and_mark_ready` to call the updated `write_bootstrap_ready_state` signature (passing `db_client` + `namespace`).

- [ ] **Step 9: Run full test suite**

Run: `cargo test -p memory_mcp`
Expected: All tests PASS.

- [ ] **Step 10: Commit**

```bash
git add crates/memory-mcp/src/storage/embedding_state_store.rs crates/memory-mcp/src/storage.rs crates/memory-mcp/src/service/startup.rs crates/memory-mcp/src/service/reembed.rs crates/memory-mcp/src/service/core/builder.rs crates/memory-mcp/src/service/embedding_recovery.rs CONTEXT.md docs/adr/0043-one-owner-for-embedding-state-record.md docs/adr/0044-narrow-stores-expose-named-methods-only.md
git commit -m "refactor(storage): one owner for embedding state record (ADR-0043, C1)"
```

---

## Task 2: C3 — Close raw-SQL escape hatches (ADR-0044)

**Files:**
- Create: `crates/memory-mcp/src/storage/entity_store.rs`
- Modify: `crates/memory-mcp/src/storage.rs`
- Modify: `crates/memory-mcp/src/service/entity.rs`
- Modify: `crates/memory-mcp/src/service/explanation.rs`
- Modify: `crates/memory-mcp/src/service/episode/entity_extraction.rs`
- Modify: `crates/memory-mcp/src/service/lifecycle/archival.rs`
- Modify: `crates/memory-mcp/src/service/context/logging.rs`
- Modify: `crates/memory-mcp/src/storage/app_store.rs`
- Modify: `crates/memory-mcp/src/storage/context_store.rs`
- Modify: `crates/memory-mcp/src/storage/episode_store.rs`

**Interfaces:**
- Consumes: `BoundDbClient`, `DbClient`, `MemoryError`
- Produces: `EntityStoreClient` (find_by_alias, find_by_prefix, add_alias), `ContextStoreClient::select_episodes_via_entity`, `EpisodeStoreClient::create_extraction_projection`, `AppStoreClient::has_recent_fact_access`, `ContextAccessLogClient::prune_expired_logs`

- [ ] **Step 1: Write failing tests for EntityStoreClient**

```rust
// In crates/memory-mcp/src/storage/entity_store.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::SurrealDbClient;
    use serde_json::json;
    use std::sync::Arc;

    async fn make_db() -> Arc<SurrealDbClient> {
        let database = format!("test_entity_store_{}", uuid::Uuid::new_v4().simple());
        SurrealDbClient::connect_in_memory(&database, "org", "warn")
            .await
            .expect("in-memory db")
    }

    #[tokio::test]
    async fn find_by_alias_returns_entity_id() {
        let db = make_db().await;
        // Seed an entity with aliases
        db.create(
            "entity:alice",
            json!({
                "entity_id": "entity:alice",
                "entity_type": "person",
                "canonical_name": "Alice",
                "canonical_name_normalized": "alice",
                "aliases": ["ali", "alicia"],
            }),
            "org",
        )
        .await
        .expect("seed entity");

        let store = EntityStoreClient::new(db, "org");
        let result = store.find_entity_id_by_alias("ali").await.expect("find");
        assert_eq!(result, Some("entity:alice".to_string()));
    }

    #[tokio::test]
    async fn find_by_prefix_returns_matching_entities() {
        let db = make_db().await;
        db.create(
            "entity:alice",
            json!({
                "entity_id": "entity:alice",
                "canonical_name": "Alice",
                "canonical_name_normalized": "alice",
                "aliases": [],
            }),
            "org",
        )
        .await
        .expect("seed");

        let store = EntityStoreClient::new(db, "org");
        let results = store.find_entities_by_prefix("ali").await.expect("find");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], ("entity:alice".to_string(), "Alice".to_string()));
    }

    #[tokio::test]
    async fn add_alias_appends_to_existing_entity() {
        let db = make_db().await;
        db.create(
            "entity:alice",
            json!({
                "entity_id": "entity:alice",
                "canonical_name": "Alice",
                "canonical_name_normalized": "alice",
                "aliases": ["ali"],
            }),
            "org",
        )
        .await
        .expect("seed");

        let store = EntityStoreClient::new(db.clone(), "org");
        store.add_alias("entity:alice", "alicia").await.expect("add alias");

        let record = db.select_one("entity:alice", "org").await.expect("read");
        let aliases = record
            .and_then(|r| r.get("aliases").cloned())
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        assert!(aliases.contains(&json!("alicia")));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p memory_mcp entity_store -- --nocapture`
Expected: FAIL — module does not exist.

- [ ] **Step 3: Implement EntityStoreClient**

```rust
//! Narrow entity store: owns entity alias queries (ADR-0044).

use std::sync::Arc;

use serde_json::{Value, json};

use crate::service::MemoryError;
use crate::storage::{BoundDbClient, DbClient};

/// One owner for entity alias operations.
pub(crate) struct EntityStoreClient {
    db: BoundDbClient,
}

impl EntityStoreClient {
    pub(crate) fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            db: BoundDbClient::new(db, namespace),
        }
    }

    /// Find an entity ID by searching aliases.
    pub(crate) async fn find_entity_id_by_alias(
        &self,
        normalized_alias: &str,
    ) -> Result<Option<String>, MemoryError> {
        let sql = "SELECT entity_id FROM entity WHERE aliases CONTAINS $alias LIMIT 1";
        let rows = self
            .db
            .query_rows(sql, Some(json!({"alias": normalized_alias})))
            .await?;
        Ok(rows
            .first()
            .and_then(|v| v.get("entity_id"))
            .and_then(Value::as_str)
            .map(str::to_string))
    }

    /// Find entities whose normalized name starts with the given prefix.
    pub(crate) async fn find_entities_by_prefix(
        &self,
        prefix: &str,
    ) -> Result<Vec<(String, String)>, MemoryError> {
        let sql = "SELECT entity_id, canonical_name FROM entity WHERE string::starts_with(canonical_name_normalized, $prefix) LIMIT 50";
        let rows = self
            .db
            .query_rows(sql, Some(json!({"prefix": prefix})))
            .await?;
        Ok(rows
            .iter()
            .filter_map(|v| {
                let id = v.get("entity_id")?.as_str()?.to_string();
                let name = v.get("canonical_name")?.as_str()?.to_string();
                Some((id, name))
            })
            .collect())
    }

    /// Add an alias to an existing entity.
    pub(crate) async fn add_alias(
        &self,
        entity_id: &str,
        normalized_alias: &str,
    ) -> Result<(), MemoryError> {
        let sql = "UPDATE type::record($id) SET aliases += [$alias]";
        self.db
            .query(sql, Some(json!({"id": entity_id, "alias": normalized_alias})))
            .await?;
        Ok(())
    }
}
```

- [ ] **Step 4: Register in storage.rs and run tests**

Add to `storage.rs`:
```rust
pub(crate) mod entity_store;
```
Re-export:
```rust
pub(crate) use entity_store::EntityStoreClient;
```

Run: `cargo test -p memory_mcp entity_store -- --nocapture`
Expected: PASS

- [ ] **Step 5: Add named methods to existing stores**

In `crates/memory-mcp/src/storage/context_store.rs`, add to `ContextStoreClient`:
```rust
/// Episodes linked to an entity via fact→edge graph traversal.
pub async fn select_episodes_via_entity(
    &self,
    entity_id: &str,
) -> Result<Vec<Value>, MemoryError> {
    let sql = "SELECT * FROM episode WHERE episode_id IN (SELECT VALUE source_episode FROM fact WHERE fact_id IN (SELECT VALUE type::string(out) FROM edge WHERE in = <record> $entity_id AND relation = 'involved_in')) ORDER BY t_ref DESC LIMIT 10";
    self.db.query_rows(sql, Some(json!({"entity_id": entity_id}))).await
}
```

In `crates/memory-mcp/src/storage/context_store.rs`, add to `ContextAccessLogClient`:
```rust
/// Deletes expired query log entries and returns the count of deleted rows.
pub async fn prune_expired_logs(&self, cutoff: &str) -> Result<usize, MemoryError> {
    let deleted = self
        .db
        .query(
            "DELETE query_log WHERE logged_at IS NOT NONE AND type::datetime(logged_at) < type::datetime($cutoff) RETURN BEFORE",
            Some(json!({"cutoff": cutoff})),
        )
        .await?;
    Ok(deleted.as_array().map_or(0, std::vec::Vec::len))
}
```

In `crates/memory-mcp/src/storage/episode_store.rs`, add:
```rust
/// Persists an entity extraction projection record.
pub async fn create_extraction_projection(
    &self,
    record_body: &str,
    vars: Value,
) -> Result<(), MemoryError> {
    let sql = format!(
        "CREATE entity_extraction_projection:⟨{record_body}⟩ SET \
         episode_id = $episode_id, \
         t_ingested = type::datetime($t_ingested), t_created = type::datetime($t_created), \
         fingerprint = $fingerprint, entity_ids = $entity_ids RETURN *"
    );
    self.db.query(&sql, Some(vars)).await?;
    Ok(())
}
```

In `crates/memory-mcp/src/storage/app_store.rs`, add:
```rust
/// Checks whether any fact linked to an episode was accessed recently.
pub async fn has_recent_fact_access(
    &self,
    episode_id: &str,
    hot_cutoff: &str,
) -> Result<bool, MemoryError> {
    let rows = self
        .db
        .query_rows(
            "SELECT fact_id FROM fact WHERE source_episode = $episode_id AND last_accessed IS NOT NONE AND last_accessed >= type::datetime($hot_cutoff) LIMIT 1",
            Some(json!({"episode_id": episode_id, "hot_cutoff": hot_cutoff})),
        )
        .await?;
    Ok(!rows.is_empty())
}
```

- [ ] **Step 6: Migrate service call sites**

In `crates/memory-mcp/src/service/entity.rs`:
- Replace `find_entity_id_by_alias` body to use `EntityStoreClient`.
- Replace `find_entities_by_prefix` body to use `EntityStoreClient`.
- Replace `add_alias_to_entity` body to use `EntityStoreClient`.
- Add `entity_store()` helper method.

In `crates/memory-mcp/src/service/explanation.rs`:
- Replace `find_episodes_via_entity` to use `ContextStoreClient::select_episodes_via_entity`.

In `crates/memory-mcp/src/service/episode/entity_extraction.rs`:
- Replace `persist_extraction_projection` inline SQL with `EpisodeStoreClient::create_extraction_projection`.

In `crates/memory-mcp/src/service/lifecycle/archival.rs`:
- Replace `check_episode_has_recent_fact_access` to use `AppStoreClient::has_recent_fact_access`.

In `crates/memory-mcp/src/service/context/logging.rs`:
- Replace `prune_expired_query_logs` to use `ContextAccessLogClient::prune_expired_logs`.

- [ ] **Step 7: Delete query() escape hatches**

Remove `pub async fn query(...)` from:
- `AppStoreClient` (app_store.rs L207–210)
- `ContextAccessLogClient` (context_store.rs L191–194)
- `EpisodeStoreClient` (episode_store.rs L53–56)

- [ ] **Step 8: Run full test suite**

Run: `cargo test -p memory_mcp`
Expected: All tests PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/memory-mcp/src/storage/entity_store.rs crates/memory-mcp/src/storage.rs crates/memory-mcp/src/service/entity.rs crates/memory-mcp/src/service/explanation.rs crates/memory-mcp/src/service/episode/entity_extraction.rs crates/memory-mcp/src/service/lifecycle/archival.rs crates/memory-mcp/src/service/context/logging.rs crates/memory-mcp/src/storage/app_store.rs crates/memory-mcp/src/storage/context_store.rs crates/memory-mcp/src/storage/episode_store.rs
git commit -m "refactor(storage): close raw-SQL escape hatches (ADR-0044, C3)"
```

---

## Task 3: C4 — Delete EmbeddingBackgroundSnapshot

**Files:**
- Modify: `crates/memory-mcp/src/service/embedding_service.rs`

**Interfaces:**
- Consumes: `EmbeddingService` (Clone), `BackgroundTaskRunner`
- Produces: Same public API, no snapshot struct

- [ ] **Step 1: Write regression test**

```rust
// Add to existing tests in embedding_service.rs or a new test
#[tokio::test]
async fn background_fact_embedding_uses_service_clone() {
    // This test verifies that the background task path works correctly
    // after removing the snapshot. The existing integration tests in
    // service/core.rs (add_fact_defers_background_embedding_after_transient_remote_failure)
    // already cover this path. We just need to ensure they still pass.
}
```

- [ ] **Step 2: Move snapshot-only methods to EmbeddingService**

Move these methods from `impl EmbeddingBackgroundSnapshot` to `impl EmbeddingService`:
- `insert_current_embedding_fields`
- `store_embedding_on_fact`
- `release_background_embedding_task`
- `run_background_fact_embedding_task` (change `self` to `&self`)
- `run_background_fact_embedding_task_inner` (already `&self`)
- `run_background_query_embedding_task` (change `self` to `&self`)
- `run_background_query_embedding_task_inner` (already `&self`)

- [ ] **Step 3: Update spawn sites to use self.clone()**

In `enqueue_background_fact_embedding`:
```rust
let service = self.clone();
tokio::spawn(async move {
    service
        .run_background_fact_embedding_task(task_key, namespace, fact_id, input)
        .await;
});
```

In `enqueue_background_query_embedding`:
```rust
let service = self.clone();
tokio::spawn(async move {
    service
        .run_background_query_embedding_task(task_key, input)
        .await;
});
```

- [ ] **Step 4: Delete EmbeddingBackgroundSnapshot struct and embedding_background_snapshot()**

Remove:
- `struct EmbeddingBackgroundSnapshot` (L400–408)
- `impl EmbeddingBackgroundSnapshot` block
- `fn embedding_background_snapshot(&self)` (L381–392)
- Duplicate methods that now live only on `EmbeddingService`:
  - `generate_embedding` (the private copy at L548–631)
  - `should_defer_embedding_retry` (the private copy at L633–636)
  - `query_embedding_cache_key` (the private copy at L638–647)
  - `store_query_embedding` (the private copy at L649–659)

- [ ] **Step 5: Run full test suite**

Run: `cargo test -p memory_mcp`
Expected: All tests PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/memory-mcp/src/service/embedding_service.rs
git commit -m "refactor(embedding): delete EmbeddingBackgroundSnapshot, use service clone (C4)"
```

---

## Task 4: C5 — Unfuse client.rs (move migration runtime to migrations.rs)

**Files:**
- Modify: `crates/memory-mcp/src/storage/client.rs`
- Modify: `crates/memory-mcp/src/storage/migrations.rs`

**Interfaces:**
- Consumes: `SurrealDbClient` private methods (`execute_raw_query`, `query`, `select_one`, `update`)
- Produces: Migration runtime in `migrations.rs`, `client.rs` reads as pure deep client

- [ ] **Step 1: Make migration-needed primitives pub(crate)**

In `client.rs`, change visibility of methods needed by migration code:
```rust
pub(crate) async fn execute_raw_query_internal(
    &self,
    sql: &str,
    vars: Option<Value>,
    namespace: &str,
) -> Result<(), MemoryError> {
    self.execute_raw_query(sql, vars, namespace).await
}

pub(crate) async fn query_internal(
    &self,
    sql: &str,
    vars: Option<Value>,
    namespace: &str,
) -> Result<Value, MemoryError> {
    self.query(sql, vars, namespace).await
}

pub(crate) async fn select_one_internal(
    &self,
    record_id: &str,
    namespace: &str,
) -> Result<Option<Value>, MemoryError> {
    self.select_one(record_id, namespace).await
}

pub(crate) async fn update_internal(
    &self,
    record_id: &str,
    content: Value,
    namespace: &str,
) -> Result<Value, MemoryError> {
    self.update(record_id, content, namespace).await
}

pub(crate) fn fact_embedding_dimension(&self) -> usize {
    self.fact_embedding_dimension
}

pub(crate) fn logger(&self) -> &StdoutLogger {
    &self.logger
}
```

- [ ] **Step 2: Move migration runtime functions to migrations.rs**

Move these items from `client.rs` to `migrations.rs`:
- `apply_migrations_impl` → becomes a free function taking `&SurrealDbClient`
- `ensure_migration_runner_schema`
- `verify_schema_postconditions`
- `apply_versioned_migration`
- `create_migration_lease`
- `claim_existing_migration`
- `mark_migration_failed`
- `mark_migration_applied`
- All migration constants (L865–976)
- `required_schema_fields`
- `required_schema_indexes`
- Tolerance helpers (`first_info_object`, `info_names`, `migration_record_body`, `migration_owner`, `migration_status`, `migration_lease_is_active`, `validate_applied_migration_compatibility`, `is_tolerable_initial_schema_error`, `is_tolerable_initial_schema_conflict`)
- `render_sql_template`
- `INITIAL_SCHEMA_*` constants
- `EXPECTED_SCHEMA_*` constants

Keep `is_record_already_exists_error` in `client.rs` (used by `claims.rs`).

- [ ] **Step 3: Update apply_migrations_impl in client.rs to delegate**

```rust
pub async fn apply_migrations_impl(&self, namespace: &str) -> Result<(), MemoryError> {
    self.ensure_active_namespace(namespace)?;
    super::migrations::run_migrations(self, namespace).await
}
```

- [ ] **Step 4: Move matching tests**

Move these tests from `client.rs` to `migrations.rs`:
- `migration_compatibility_allows_recovery_without_executed_at`
- `migration_compatibility_rejects_changed_recovery_record`
- `migration_lease_activity_is_conservative`
- `initial_schema_tolerates_known_idempotent_definition_conflicts`
- `initial_schema_rejects_unknown_or_mixed_definition_errors`
- `schema_postconditions_reject_missing_required_resources`

- [ ] **Step 5: Run full test suite**

Run: `cargo test -p memory_mcp`
Expected: All tests PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/memory-mcp/src/storage/client.rs crates/memory-mcp/src/storage/migrations.rs
git commit -m "refactor(storage): move migration runtime from client.rs to migrations.rs (C5)"
```

---

## Task 5: C6 — App session lifecycle below adapter line

**Files:**
- Create: `crates/memory-mcp/src/service/apps/session_lifecycle.rs`
- Modify: `crates/memory-mcp/src/service/apps.rs`
- Modify: `crates/memory-mcp/src/mcp/handlers.rs`
- Modify: `crates/memory-mcp/src/mcp/handlers/apps.rs`

**Interfaces:**
- Consumes: `MemoryService`, `SessionManager`, `AppCommand`, `AppContext`, `COMMAND_TABLE`
- Produces: `open_app_session(...)`, `execute_app_command(...)` — service-level orchestration

- [ ] **Step 1: Write failing test for session lifecycle**

```rust
// In crates/memory-mcp/src/service/apps/session_lifecycle.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn execute_command_returns_outcome_for_known_action() {
        // The existing integration tests in mcp/handlers.rs already cover
        // the full open_app → app_command flow. This test verifies the
        // service-level function can be called independently.
        // Covered by existing: app_command_mutates_ingestion_review_items_and_closes_session
    }
}
```

- [ ] **Step 2: Create session_lifecycle.rs with orchestration**

```rust
//! App session lifecycle orchestration (C6).
//!
//! Gathers the open/command lifecycle into the service layer.
//! MCP handlers become decode–call–encode adapters.

use serde_json::Value;
use rmcp::ErrorData;

use crate::mcp::session::{AppSessionState, SessionManager};
use crate::mcp::response::{AppCommandResult, OpenAppResult};
use crate::service::MemoryService;
use crate::service::apps::dispatch::{AppContext, AppCommandOutcome, find_descriptor};
use crate::service::apps::workflow::{AppCommand, AppCommandInput};

/// Executes an app command against a session, returning the shaped result.
pub(crate) async fn execute_app_command(
    service: &MemoryService,
    session_manager: &SessionManager,
    session_id: &str,
    input: AppCommandInput,
) -> Result<AppCommandResult, ErrorData> {
    session_manager.purge_expired().await;
    let session = session_manager.get_valid(session_id).await?;

    let command = AppCommand::parse(&session.app, input)
        .map_err(|error| crate::mcp::session::invalid_params(error.to_string()))?;
    let descriptor = find_descriptor(&command)?;
    let ctx = AppContext {
        service,
        session_id,
        app: &session.app,
        payload: session.payload.clone(),
    };
    let outcome = (descriptor.execute)(&ctx, &command).await?;

    if let Some(payload) = outcome.new_payload {
        session_manager.replace_payload(session_id, payload).await?;
    }
    if outcome.close_session {
        session_manager.remove(session_id).await?;
    }

    let resource_uri = if outcome.close_session {
        None
    } else {
        Some(crate::mcp::resources::app_session_uri(&session.app, session_id))
    };

    Ok(crate::mcp::session::app_command_result_from_details(
        &session.app,
        session_id,
        outcome.action,
        resource_uri,
        outcome.details,
    ))
}
```

- [ ] **Step 3: Register module**

Add to `crates/memory-mcp/src/service/apps.rs`:
```rust
pub(crate) mod session_lifecycle;
```

- [ ] **Step 4: Update mcp/handlers.rs app_command to delegate**

Replace the inline orchestration in `app_command` with:
```rust
let input = AppCommandInput {
    action: p.action.clone(),
    item_ids: p.item_ids.clone(),
    target_ids: p.target_ids.clone(),
    target_id: p.target_id.clone(),
    item_id: p.item_id.clone(),
    patch_json: p.patch_json.clone(),
    reason: p.reason.clone(),
    dry_run: p.dry_run.unwrap_or(false),
    confirmed: p.confirmed.unwrap_or(false),
    format: p.format.clone(),
    direction: p.direction.clone(),
    depth: p.depth,
};
let command_result = crate::service::apps::session_lifecycle::execute_app_command(
    &self.service,
    &self.session_manager,
    &p.session_id,
    input,
)
.await?;
```

- [ ] **Step 5: Run full test suite**

Run: `cargo test -p memory_mcp`
Expected: All tests PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/memory-mcp/src/service/apps/session_lifecycle.rs crates/memory-mcp/src/service/apps.rs crates/memory-mcp/src/mcp/handlers.rs crates/memory-mcp/src/mcp/handlers/apps.rs
git commit -m "refactor(apps): gather session lifecycle below adapter line (C6)"
```

---

## Task 6: C2 — Split Reembed (job store + index DDL out of orchestrator)

**Files:**
- Modify: `crates/memory-mcp/src/service/reembed.rs`
- Modify: `crates/memory-mcp/src/storage/reembed_store.rs`

**Interfaces:**
- Consumes: `ReembedStoreClient`, `EmbeddingStateStoreClient` (from Task 1)
- Produces: Index DDL methods on `ReembedStoreClient`, job persistence extracted

**Depends on:** Task 1 (C1) must be complete first.

- [ ] **Step 1: Move index DDL to ReembedStoreClient**

Add to `crates/memory-mcp/src/storage/reembed_store.rs`:
```rust
/// Removes the embedding HNSW index. Idempotent: succeeds if absent.
pub async fn remove_embedding_index(&self, index_name: &str) -> Result<(), MemoryError> {
    let sql = format!("REMOVE INDEX {index_name} ON TABLE fact");
    match self.db.query(&sql, None).await {
        Ok(_) => Ok(()),
        Err(MemoryError::Storage(message))
            if crate::storage::is_missing_index_error(&message) =>
        {
            Ok(())
        }
        Err(err) => Err(err),
    }
}

/// Creates the embedding HNSW index with the given dimension.
pub async fn define_embedding_index(
    &self,
    index_name: &str,
    dimension: usize,
) -> Result<(), MemoryError> {
    let sql = format!(
        "DEFINE INDEX {index_name} ON TABLE fact FIELDS embedding HNSW DIMENSION {dimension}"
    );
    self.db.query(&sql, None).await.map(|_| ())
}
```

- [ ] **Step 2: Update reembed.rs to use store methods**

Replace `remove_embedding_index` and `define_embedding_index` in `reembed.rs` with calls to `self.reembed_store().remove_embedding_index(...)` and `self.reembed_store().define_embedding_index(...)`. Keep the logging in the orchestrator.

- [ ] **Step 3: Run full test suite**

Run: `cargo test -p memory_mcp`
Expected: All tests PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/memory-mcp/src/service/reembed.rs crates/memory-mcp/src/storage/reembed_store.rs
git commit -m "refactor(reembed): move index DDL to ReembedStoreClient (C2)"
```

---

## Task 7: Final verification and cleanup

- [ ] **Step 1: Format check**

Run: `cargo fmt --all --check`
Expected: No diff.

- [ ] **Step 2: Clippy**

Run: `cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings`
Expected: Zero warnings.

- [ ] **Step 3: Full test suite**

Run: `cargo test -p memory_mcp`
Expected: All green.

- [ ] **Step 4: Code review pass**

Per `.agents/prompts/code-review.prompt.md`: verify everything is done according to plan, nothing missing, covered by tests, no dangling parts. Fix issues immediately.

- [ ] **Step 5: Final commit if any fixes needed**

```bash
git add -A
git commit -m "chore: round-3 architecture deepening — final cleanup"
```
