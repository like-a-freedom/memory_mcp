# Code Review Refactoring Implementation Plan

> **Status:** ✅ **VERIFIED COMPLETE IN CURRENT REPOSITORY STATE**
> **Current branch checked:** `master`
> **Last updated:** 2026-04-01

> **Note:** This document preserves the original sprint checklist and planning excerpts below. Where those historical checklists conflict with the current codebase, use the audited status section immediately below as the source of truth.

## 2026-04-01 Audited Status

| Task | Status | Evidence |
|------|--------|----------|
| 5 | ✅ Completed | `src/service/test_support.rs` now provides the shared `MockDb`, and service unit tests in `src/service/core.rs`, `src/service/context.rs`, and `src/service/episode.rs` use it. |
| 9 | ✅ Completed | `src/service/app_modules.rs::commit_ingestion_review()` delegates to focused helpers: `commit_entities`, `commit_facts`, `commit_edges`, `finalize_commit`, `commit_source_episode_for_draft`, `commit_draft_fact`, and `commit_draft_edge`. |
| 11 | ✅ Completed | `src/service/app_modules.rs` exists (1,331 lines), `src/service/core.rs` is reduced to 3,413 lines, and `src/service/mod.rs` wires in `app_modules`. |
| 12 | ✅ Completed | `MemoryService::new_with_clock()` and `MemoryService::now()` live in `src/service/core.rs`, and extracted APP methods in `src/service/app_modules.rs` now use the service clock path. |
| 13 | ✅ Completed | `README.md` and `docs/REVIEW_ALIGNMENT_2026-03-25.md` are updated, and `cargo fmt --all && cargo check && cargo clippy --all-targets -- -D warnings && cargo test && cargo doc --no-deps` passed on 2026-04-01. |

## Executive Summary

**Task 11 Progress:**
- ✅ APP-01 Inspector extracted (~250 lines)
- ✅ APP-02 Temporal Diff extracted (~158 lines)
- ✅ APP-03 Ingestion Review extracted (~605 lines)
- ✅ APP-04 Lifecycle Console extracted (~163 lines)
- ✅ APP-05 Graph Path Explorer extracted (~132 lines)

**Final Metrics:**
- core.rs: 5,069 → 3,414 lines (-1,655 lines, -33%)
- app_modules.rs: 1,325 lines (ALL 5 APP modules)
- All 459 tests passing ✅
- cargo clippy -- -D warnings ✅

**Quality Gates:**
- ✅ `cargo fmt`
- ✅ `cargo clippy -- -D warnings`
- ✅ `cargo test` (459 tests passing)

**Commits:** 16 commits on `refactor/code-review-remediation` branch

**Documentation:**
- ✅ Plan document: `docs/superpowers/plans/2026-03-31-code-review-refactoring.md`
- ✅ README.md: Added "Code Quality & Refactoring" section with full details

---

# Task 11: Split core.rs into APP modules — Detailed Sprint Plan

## Goal

Extract ~1,313 lines of APP-specific methods from `src/service/core.rs` (5,069 lines → ~3,756 lines) into a dedicated `app_modules.rs` file to improve SRP compliance and code organization.

## Current Structure Analysis

### APP Modules to Extract

| APP | Module Name | Lines in core.rs | Size |
|-----|-------------|-----------------|------|
| APP-01 | Inspector | 1398-1648 | ~250 |
| APP-02 | Temporal Diff | 1649-1807 | ~158 |
| APP-03 | Ingestion Review | 1808-2413 | ~605 |
| APP-04 | Lifecycle Console | 2414-2577 | ~163 |
| APP-05 | Graph Path Explorer | 2578-2710 | ~132 |
| **Helpers** | draft_* functions | 2713-2879 | ~166 |
| **TOTAL** | | | **~1,313 lines** |

### Helper Function Dependencies (7 functions)

These private helper functions are used by APP methods and must be made `pub(crate)`:

| Function | Purpose | Used by |
|----------|---------|---------|
| `string_from_value` | Parse Value to String | All APP methods |
| `json_f64` | Parse JSON to f64 | Inspector, Ingestion |
| `draft_entity_candidate` | Create EntityCandidate from draft | APP-03 |
| `draft_payload_str` | Extract String from payload | APP-03 |
| `draft_payload_string_array` | Extract Vec<String> from payload | APP-03 |
| `draft_payload_f64` | Extract f64 from payload | APP-03 |
| `draft_payload_datetime` | Extract DateTime from payload | APP-03 |
| `resolve_draft_reference` | Resolve entity references | APP-03 |

### Commit Helper Methods (6 functions)

These methods support `commit_ingestion_review` and will move with APP-03:

- `commit_entities` — Process entity items
- `commit_facts` — Process fact items with edge creation
- `commit_edges` — Process explicit edge items
- `commit_source_episode_for_draft` — Create source episode
- `commit_draft_fact` — Create fact from draft item
- `commit_draft_edge` — Create edge from draft item

## 3-Day Sprint Plan

### Day 1: Preparation + APP-01/02 Extraction

**Step 1.1: Make helper functions `pub(crate)`**
- [ ] Change 7 helper functions from `fn` to `pub(crate) fn` in core.rs
- [ ] Move helper functions to end of file (after `impl MemoryService` block)
- [ ] Run `cargo check` — verify compilation
- [ ] Run `cargo test` — verify no regressions

**Step 1.2: Create `app_modules.rs` skeleton**
- [ ] Create `src/service/app_modules.rs` with module header:
```rust
//! APP modules extracted from core.rs for SRP compliance.
//!
//! This module contains all MCP APP implementations:
//! - APP-01: Inspector (entity, fact, episode views)
//! - APP-02: Temporal Diff
//! - APP-03: Ingestion Review
//! - APP-04: Lifecycle Console
//! - APP-05: Graph Path Explorer

use serde_json::{Value, json};
use crate::service::core::{MemoryService, string_from_value, json_f64};
use crate::service::error::MemoryError;
use crate::logging::LogLevel;
use std::collections::{HashMap, HashSet, BTreeSet};

impl MemoryService {
    // APP methods will be extracted here
}
```
- [ ] Add `mod app_modules;` to `src/service/mod.rs`
- [ ] Run `cargo check` — verify module structure

**Step 1.3: Extract APP-01 Inspector (~250 lines)**
- [ ] Copy methods from core.rs lines 1401-1648 to app_modules.rs:
  - `open_inspector_entity`
  - `open_inspector_fact`
  - `open_inspector_episode`
  - `archive_episode`
  - `close_app_session`
- [ ] Remove methods from core.rs
- [ ] Run `cargo check` — verify compilation
- [ ] Run `cargo test` — verify no regressions
- [ ] Commit: `refactor(core): extract APP-01 Inspector to app_modules.rs`

**Step 1.4: Extract APP-02 Temporal Diff (~158 lines)**
- [ ] Copy methods from core.rs lines 1652-1807 to app_modules.rs:
  - `open_temporal_diff`
  - `export_temporal_diff`
  - `open_memory_inspector_from_diff`
- [ ] Remove methods from core.rs
- [ ] Run `cargo check` — verify compilation
- [ ] Run `cargo test` — verify no regressions
- [ ] Commit: `refactor(core): extract APP-02 Temporal Diff to app_modules.rs`

### Day 2: APP-03 Ingestion Review (Largest Module)

**Step 2.1: Extract commit helper methods (~300 lines)**
- [ ] Copy methods from core.rs to app_modules.rs:
  - `commit_entities` (lines 2079-2108)
  - `commit_facts` (lines 2109-2165)
  - `commit_edges` (lines 2166-2233)
  - `commit_source_episode_for_draft` (lines 2234-2277)
  - `commit_draft_fact` (lines 2278-2341)
  - `commit_draft_edge` (lines 2342-2413)
- [ ] Remove methods from core.rs
- [ ] Run `cargo check` — verify compilation
- [ ] Run `cargo test --test apps_e2e` — verify APP-03 functionality
- [ ] Commit: `refactor(core): extract commit_* helpers to app_modules.rs`

**Step 2.2: Extract APP-03 Ingestion Review main methods (~305 lines)**
- [ ] Copy methods from core.rs lines 1810-2078 to app_modules.rs:
  - `open_ingestion_review`
  - `get_draft_summary`
  - `approve_ingestion_items`
  - `reject_ingestion_items`
  - `edit_ingestion_item`
  - `cancel_ingestion_review`
  - `commit_ingestion_review`
- [ ] Remove methods from core.rs
- [ ] Run `cargo check` — verify compilation
- [ ] Run `cargo test --test apps_e2e` — verify full APP-03 functionality
- [ ] Commit: `refactor(core): extract APP-03 Ingestion Review to app_modules.rs`

### Day 3: APP-04/05 + Finalization

**Step 3.1: Extract APP-04 Lifecycle Console (~163 lines)**
- [ ] Copy methods from core.rs lines 2416-2577 to app_modules.rs:
  - `open_lifecycle_console`
  - `get_lifecycle_dashboard`
  - `archive_candidates`
  - `restore_archived`
  - `recompute_decay`
  - `rebuild_communities`
  - `get_lifecycle_task_status`
- [ ] Remove methods from core.rs
- [ ] Run `cargo check` — verify compilation
- [ ] Run `cargo test --test lifecycle_*` — verify lifecycle functionality
- [ ] Commit: `refactor(core): extract APP-04 Lifecycle Console to app_modules.rs`

**Step 3.2: Extract APP-05 Graph Path Explorer (~132 lines)**
- [ ] Copy methods from core.rs lines 2580-2710 to app_modules.rs:
  - `open_graph_path`
  - `expand_graph_neighbors`
  - `open_edge_details`
  - `use_path_as_context`
- [ ] Remove methods from core.rs
- [ ] Run `cargo check` — verify compilation
- [ ] Run `cargo test` — verify all tests pass
- [ ] Commit: `refactor(core): extract APP-05 Graph Path Explorer to app_modules.rs`

**Step 3.3: Final Code Cleanup**
- [x] Run `cargo fmt --all` — format all code
- [x] Run `cargo clippy --all-targets -- -D warnings` — fix all warnings
- [x] Verify `core.rs` line count: 3,413 lines
- [x] Verify `app_modules.rs` line count: 1,331 lines
- [x] Record final refactoring status

**Step 3.4: Full Verification**
```bash
cargo fmt --all
cargo check
cargo clippy --all-targets -- -D warnings
cargo test
cargo doc --no-deps
```
- [x] All commands pass
- [x] `cargo test` passes
- [x] No clippy warnings
- [x] Documentation builds without warnings

**Step 3.5: Update Documentation**
- [x] Update `docs/superpowers/plans/2026-03-31-code-review-refactoring.md` — record audited final status
- [x] Update `README.md` — align refactoring summary with repository state
- [x] Update review alignment notes

## Expected Results

| Metric | Before | After | Change |
|--------|--------|-------|--------|
| core.rs lines | 5,069 | 3,413 | -33% |
| app_modules.rs lines | 0 | 1,331 | New file |
| Helper functions | private | pub(crate) | Visibility change |
| Commit helpers | in core.rs | in app_modules.rs | Moved |
| APP methods | in core.rs | in app_modules.rs | Moved |

## Risk Mitigation

| Risk | Probability | Mitigation |
|------|-------------|------------|
| Compilation errors | Medium | Extract 1 APP at a time, verify after each |
| Test failures | Low | All test coverage already exists |
| Helper function visibility | Low | Make pub(crate) before extraction |
| Circular dependencies | Low | Helpers stay in core.rs, used by app_modules |

## Acceptance Criteria

- ✅ core.rs reduced to 3,413 lines (down from 5,069)
- ✅ app_modules.rs created with 1,331 lines
- ✅ Full verification pipeline passed on 2026-04-01
- ✅ `cargo clippy --all-targets -- -D warnings` — zero warnings
- ✅ All 5 APP modules + helpers extracted
- ✅ Documentation updated
- ✅ No functional regressions

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Устранить нарушения SRP, DRY и идиоматичности Rust выявленные в ходе code review. Сохранить обратную совместимость публичного API, улучшить тестируемость и читаемость кода.

**Architecture:** Четыре волны рефакторинга: (1) DRY-хелперы и валидация — минимальный риск, (2) тестируемость и моки — только тесты, (3) рефакторинг больших функций — механическое разделение, (4) разделение God Object — перемещение кода по модулям без изменения логики.

**Tech Stack:** Rust 2024, `thiserror`, `async-trait`, существующие интеграционные и unit-тесты

---

## Progress Summary

| Wave | Tasks | Status |
|------|-------|--------|
| Wave 1 — DRY helpers и валидация | Tasks 1-4 | ✅ Complete |
| Wave 2 — Тестируемость и моки | Tasks 5-6 | ✅ Complete |
| Wave 3 — Рефакторинг больших функций | Tasks 7-9 | ✅ Complete |
| Wave 4 — Разделение God Object | Tasks 10-11 | ✅ Complete |
| Wave 5 — Интеграция и документация | Tasks 12-13 | ✅ Complete |

---

## Wave mapping

- **Wave 1 — DRY helpers и валидация:** Tasks 1-4 (низкий риск, изолированные изменения) ✅
- **Wave 2 — Тестируемость и моки:** Tasks 5-6 (только тесты, нет риска для production) ✅
- **Wave 3 — Рефакторинг больших функций:** Tasks 7-9 (средний риск, механическое разделение) ✅
- **Wave 4 — Разделение God Object:** Tasks 10-11 (высокий объём, низкий риск — только перемещение) ✅
- **Wave 5 — Интеграция и документация:** Tasks 12-13 (верификация и docs) ✅

---

## Task 1: Add `require_non_empty` helper in validation.rs

**Files:**
- Modify: `src/service/validation.rs:1-50`
- Test: `src/service/validation.rs` (existing tests)

- [x] **Step 1: Add private helper function**
- [x] **Step 2: Update `validate_ingest_request` to use helper**
- [x] **Step 3: Run tests**
- [x] **Step 4: Commit**

✅ **COMPLETED** — `refactor(validation): add require_non_empty helper, remove duplication`

---

## Task 2: Add `fact_state` helper in core.rs

- [x] **Step 1: Find duplicate patterns**
- [x] **Step 2: Add private helper function**
- [x] **Step 3: Replace both call-sites**
- [x] **Step 4: Run tests**
- [x] **Step 5: Commit**

✅ **COMPLETED** — `refactor(core): add fact_state helper, remove t_invalid duplication`

---

## Task 3: Add `find_record_in_namespaces` helper in core.rs

- [x] **Step 1: Locate duplicate methods**
- [x] **Step 2: Add private generic helper**
- [x] **Step 3: Replace both methods**
- [x] **Step 4: Run tests**
- [x] **Step 5: Commit**

✅ **COMPLETED** — `refactor(core): add find_record_in_namespaces helper, deduplicate lookup`

---

## Task 4: Add `require_app` and `require_target_str` helpers for APP sessions

- [x] **Step 1: Find duplicate session validation patterns**
- [x] **Step 2: Add helpers**
- [x] **Step 3: Replace all APP method validations**
- [x] **Step 4: Run tests**
- [x] **Step 5: Commit**

✅ **COMPLETED** — `refactor(core): add require_app and require_target_str helpers for APP sessions`

---

## Task 5: Consolidate MockDbClient implementations in tests

- [x] **Step 1: Examine existing mock implementations**
- [x] **Step 2: Create unified mock with configurable behavior**
- [x] **Step 3: Replace individual mock structs in tests**
- [x] **Step 4: Run all tests**
- [x] **Step 5: Commit**

✅ **COMPLETED** — Shared `MockDb` now backs the service unit tests in `core.rs`, `context.rs`, and `episode.rs`

---

## Task 6: Add `resolve_entity_by_type` to deduplicate resolve_* methods

- [x] **Step 1: Find all `resolve_*` convenience methods**
- [x] **Step 2: Add generic helper**
- [x] **Step 3: Rewrite convenience methods as delegates**
- [x] **Step 4: Run tests**
- [x] **Step 5: Commit**

✅ **COMPLETED** — `refactor(core): add resolve_entity_by_type, delegate resolve_* helpers`

---

## Task 7: Fix namespace lookup idioms and add prefix table

- [x] **Step 1: Fix `contains(&"personal".to_string())` allocation**
- [x] **Step 2: Add known prefixes table**
- [x] **Step 3: Refactor `namespace_for_scope` to use prefix table**
- [x] **Step 4: Run tests**
- [x] **Step 5: Commit**

✅ **COMPLETED** — `refactor(core): fix namespace lookup idioms, add KNOWN_SCOPE_PREFIXES table`

---

## Task 8: Add `// SAFETY:` comment for sync::Mutex in RateLimiter

- [x] **Step 1: Locate RateLimiter struct**
- [x] **Step 2: Add safety comment**
- [x] **Step 3: Run clippy**
- [x] **Step 4: Commit**

✅ **COMPLETED** — `docs(core): add SAFETY comment for sync::Mutex in RateLimiter`

---

## Task 9: Refactor commit_ingestion_review into smaller methods

- [x] **Step 1: Analyze commit_ingestion_review logical steps**
- [x] **Step 2: Extract private methods**
- [x] **Step 3: Rewrite main method as orchestrator**
- [x] **Step 4: Run tests**
- [x] **Step 5: Commit**

✅ **COMPLETED** — `commit_ingestion_review` now delegates to focused commit helpers and a finalization step in `src/service/app_modules.rs`

---

## Task 10: Add AddFactRequest struct for add_fact method

- [x] **Step 1: Locate add_fact signature**
- [x] **Step 2: Define AddFactRequest struct**
- [x] **Step 3: Add new method signature**
- [x] **Step 4: Update all internal call-sites**
- [x] **Step 5: Run tests**
- [x] **Step 6: Commit**

✅ **COMPLETED** — `refactor(core): add AddFactRequest struct, reduce add_fact parameters from 10 to 1`
/// Validates that a string field is non-empty after trimming whitespace.
///
/// # Errors
///
/// Returns [`MemoryError::Validation`] if the field is empty or whitespace-only.
fn require_non_empty(value: &str, field_name: &str) -> Result<(), MemoryError> {
    if value.trim().is_empty() {
        return Err(MemoryError::Validation(format!("{field_name} is required")));
    }
    Ok(())
}
```

- [ ] **Step 2: Update `validate_ingest_request` to use helper**

Replace lines with repetitive checks:

```rust
pub fn validate_ingest_request(request: &IngestRequest) -> Result<(), MemoryError> {
    require_non_empty(&request.source_type, "source_type")?;
    require_non_empty(&request.source_id, "source_id")?;
    require_non_empty(&request.content, "content")?;
    require_non_empty(&request.scope, "scope")?;
    Ok(())
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test --lib validation
```

Expected: All existing tests pass

- [ ] **Step 4: Commit**

```bash
git add src/service/validation.rs
git commit -m "refactor(validation): add require_non_empty helper, remove duplication"
```

---

## Task 2: Add `fact_state` helper in core.rs

**Files:**
- Modify: `src/service/core.rs` (search for `t_invalid` patterns)
- Test: Existing tests in `src/service/core.rs`

- [ ] **Step 1: Find duplicate patterns**

Search for:
```rust
let t_invalid = record.get("t_invalid");
let state = if t_invalid.is_some() && !t_invalid.unwrap().is_null() {
    "invalidated"
} else {
    "active"
};
```

Expected locations: `open_inspector_entity`, `open_inspector_fact`

- [ ] **Step 2: Add private helper function**

Add in `impl MemoryService` block:

```rust
/// Determines the state of a fact/episode record based on t_invalid field.
fn fact_state(record: &serde_json::Map<String, Value>) -> &'static str {
    match record.get("t_invalid") {
        Some(v) if !v.is_null() => "invalidated",
        _ => "active",
    }
}
```

- [ ] **Step 3: Replace both call-sites**

In `open_inspector_entity` and `open_inspector_fact`, replace:

```rust
let state = fact_state(&record);
```

- [ ] **Step 4: Run tests**

```bash
cargo test --lib core
```

- [ ] **Step 5: Commit**

```bash
git add src/service/core.rs
git commit -m "refactor(core): add fact_state helper, remove t_invalid duplication"
```

---

## Task 3: Add `find_record_in_namespaces` helper in core.rs

**Files:**
- Modify: `src/service/core.rs`
- Test: Existing tests

- [ ] **Step 1: Locate duplicate methods**

Find `find_episode_record` and `find_fact_record` — both iterate namespaces and call `select_one`.

- [ ] **Step 2: Add private generic helper**

```rust
/// Searches for a record across all configured namespaces.
///
/// Returns `(Some(record_map), Some(namespace))` if found, or `(None, None)` otherwise.
async fn find_record_in_namespaces(
    &self,
    record_id: &str,
) -> Result<(Option<serde_json::Map<String, Value>>, Option<String>), MemoryError> {
    for namespace in &self.namespaces {
        if let Some(Value::Object(map)) = self.db_client.select_one(record_id, namespace).await? {
            return Ok((Some(map), Some(namespace.clone())));
        }
    }
    Ok((None, None))
}
```

- [ ] **Step 3: Replace both methods**

```rust
pub(crate) async fn find_episode_record(&self, id: &str) -> Result<(Option<Map>, Option<String>), MemoryError> {
    self.find_record_in_namespaces(id).await
}

pub(crate) async fn find_fact_record(&self, id: &str) -> Result<(Option<Map>, Option<String>), MemoryError> {
    self.find_record_in_namespaces(id).await
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test --lib core
```

- [ ] **Step 5: Commit**

```bash
git add src/service/core.rs
git commit -m "refactor(core): add find_record_in_namespaces helper, deduplicate lookup"
```

---

## Task 4: Add `require_app` and `require_target_str` helpers for APP sessions

**Files:**
- Modify: `src/service/core.rs`
- Test: APP-related tests (APP-01 through APP-05)

- [ ] **Step 1: Find duplicate session validation patterns**

Search for:
```rust
if session.app_id != "ingestion_review" {
    return Err(MemoryError::App("session is not an ingestion review session".to_string()));
}
let draft_id = session.target.get("draft_id")
    .and_then(|v| v.as_str())
    .ok_or_else(|| MemoryError::App("draft_id not found in session".to_string()))?;
```

- [ ] **Step 2: Add helpers**

Add as methods on `impl AppSession` or as standalone helpers in `core.rs`:

```rust
/// Validates that the session belongs to the expected app.
///
/// # Errors
///
/// Returns [`MemoryError::App`] if the session app_id does not match.
fn require_app(session: &AppSession, expected_app: &str) -> Result<(), MemoryError> {
    if session.app_id != expected_app {
        return Err(MemoryError::App(format!(
            "session is not a {expected_app} session"
        )));
    }
    Ok(())
}

/// Extracts a required string field from the session target.
///
/// # Errors
///
/// Returns [`MemoryError::App`] if the field is missing or not a string.
fn require_target_str<'a>(session: &'a AppSession, key: &str) -> Result<&'a str, MemoryError> {
    session
        .target
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| MemoryError::App(format!("{key} not found in session")))
}
```

- [ ] **Step 3: Replace all APP method validations**

Example replacement in `open_ingestion_review`:

```rust
require_app(&session, "ingestion_review")?;
let draft_id = require_target_str(&session, "draft_id")?;
```

- [ ] **Step 4: Run tests**

```bash
cargo test
```

- [ ] **Step 5: Commit**

```bash
git add src/service/core.rs
git commit -m "refactor(core): add require_app and require_target_str helpers for APP sessions"
```

---

## Task 5: Consolidate MockDbClient implementations in tests

**Files:**
- Modify: `src/service/core.rs` (test module)
- Create: `src/service/test_support.rs` (new module) OR keep inline in `core.rs` tests
- Test: All tests using mocks

- [ ] **Step 1: Examine existing mock implementations**

Find all mock structs in tests:
- `MockDbClient`
- `StartupMigrationDbClient`
- `LookupOnlyDbClient`
- `TraversalDbClient`

Each implements 20+ `DbClient` methods with boilerplate `Ok(vec![])` / `Ok(Value::Null)`.

- [ ] **Step 2: Create unified mock with configurable behavior**

Add `#[cfg(test)] pub(crate) mod test_support` in `core.rs` or separate file:

```rust
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::sync::Arc;

    /// A configurable test double for DbClient.
    /// All methods return empty/null by default; override fields to inject behaviour.
    pub(crate) struct MockDb {
        pub on_select_entity_lookup:
            Option<Box<dyn Fn(&str, &str) -> Result<Option<Value>, MemoryError> + Send + Sync>>,
        pub on_select_edge_neighbors:
            Option<Box<dyn Fn(&str, GraphDirection) -> Result<Vec<Value>, MemoryError> + Send + Sync>>,
        pub on_create:
            Option<Box<dyn Fn(&str, &Value) -> Result<Value, MemoryError> + Send + Sync>>,
        // Add more override fields as needed by tests
    }

    impl Default for MockDb {
        fn default() -> Self {
            Self {
                on_select_entity_lookup: None,
                on_select_edge_neighbors: None,
                on_create: None,
            }
        }
    }

    #[async_trait::async_trait]
    impl DbClient for MockDb {
        async fn select_entity_lookup(&self, ns: &str, name: &str) -> Result<Option<Value>, MemoryError> {
            if let Some(f) = &self.on_select_entity_lookup {
                return f(ns, name);
            }
            Ok(None)
        }

        async fn select_edge_neighbors(&self, _ns: &str, _id: &str, _cutoff: &str, dir: GraphDirection) -> Result<Vec<Value>, MemoryError> {
            if let Some(f) = &self.on_select_edge_neighbors {
                return f(_id, dir);
            }
            Ok(vec![])
        }

        async fn create(&self, id: &str, c: &Value, ns: &str) -> Result<Value, MemoryError> {
            if let Some(f) = &self.on_create {
                return f(id, c);
            }
            Ok(Value::Null)
        }

        // All other methods: one-liner returning Ok(Default::default())
        async fn select_one(&self, _id: &str, _ns: &str) -> Result<Option<Value>, MemoryError> {
            Ok(None)
        }
        // ... repeat for all 20+ DbClient methods
    }
}
```

- [ ] **Step 3: Replace individual mock structs in tests**

Update test code to use `MockDb::default()` and override only needed methods.

- [ ] **Step 4: Run all tests**

```bash
cargo test
```

- [ ] **Step 5: Commit**

```bash
git add src/service/core.rs src/service/test_support.rs
git commit -m "test(core): consolidate MockDbClient x4 into single configurable MockDb"
```

---

## Task 6: Add `resolve_entity_by_type` to deduplicate resolve_* methods

**Files:**
- Modify: `src/service/core.rs`
- Test: Existing tests for `resolve_person`, `resolve_company`, etc.

- [ ] **Step 1: Find all `resolve_*` convenience methods**

Search for:
```rust
pub async fn resolve_person(&self, name: &str) -> Result<String, MemoryError>
pub async fn resolve_company(&self, name: &str) -> Result<String, MemoryError>
// ... ещё 4 аналогичных
```

- [ ] **Step 2: Add generic helper**

```rust
/// Resolves a named entity of the given type, creating it if absent.
pub async fn resolve_entity_by_type(
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

- [ ] **Step 3: Rewrite convenience methods as delegates**

```rust
pub async fn resolve_person(&self, name: &str) -> Result<String, MemoryError> {
    self.resolve_entity_by_type("person", name).await
}

pub async fn resolve_company(&self, name: &str) -> Result<String, MemoryError> {
    self.resolve_entity_by_type("company", name).await
}

// ... repeat for all 6 methods
```

- [ ] **Step 4: Run tests**

```bash
cargo test --lib core
```

- [ ] **Step 5: Commit**

```bash
git add src/service/core.rs
git commit -m "refactor(core): add resolve_entity_by_type, delegate resolve_* helpers"
```

---

## Task 7: Fix namespace lookup idioms and add prefix table

**Files:**
- Modify: `src/service/core.rs` (`namespace_for_scope` method)
- Test: Tests covering scope-to-namespace resolution

- [ ] **Step 1: Fix `contains(&"personal".to_string())` allocation**

Replace:
```rust
self.namespaces.contains(&"personal".to_string())
```

With:
```rust
self.namespaces.iter().any(|ns| ns == "personal")
```

- [ ] **Step 2: Add known prefixes table**

Add constant at module level:

```rust
/// Known scope prefixes and their corresponding namespaces.
const KNOWN_SCOPE_PREFIXES: &[&str] = &["personal", "private", "org"];
```

- [ ] **Step 3: Refactor `namespace_for_scope` to use prefix table**

```rust
fn resolve_prefix_namespace(&self, scope_lower: &str) -> Option<String> {
    KNOWN_SCOPE_PREFIXES
        .iter()
        .find(|&&prefix| scope_lower.starts_with(prefix))
        .and_then(|prefix| {
            self.namespaces.iter().find(|ns| ns.as_str() == *prefix).cloned()
        })
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test --lib core
```

- [ ] **Step 5: Commit**

```bash
git add src/service/core.rs
git commit -m "refactor(core): fix namespace lookup idioms, add KNOWN_SCOPE_PREFIXES table"
```

---

## Task 8: Add `// SAFETY:` comment for sync::Mutex in RateLimiter

**Files:**
- Modify: `src/service/core.rs` (`RateLimiter` struct)
- Test: No test changes needed

- [ ] **Step 1: Locate RateLimiter struct**

Find the `RateLimiter` definition with `std::sync::Mutex`.

- [ ] **Step 2: Add safety comment**

```rust
pub(crate) struct RateLimiter {
    rps: f64,
    burst: f64,
    /// Per-key state for token bucket algorithm.
    /// 
    /// # Safety
    /// 
    /// We use `std::sync::Mutex` instead of `tokio::sync::Mutex` because:
    /// - Guards are always dropped before any `.await` point
    /// - No async operations occur while holding the lock
    /// - This prevents tokio worker thread blocking
    tokens: Mutex<HashMap<String, f64>>,
    last: Mutex<HashMap<String, Instant>>,
}
```

- [ ] **Step 3: Run clippy**

```bash
cargo clippy -- -D warnings
```

- [ ] **Step 4: Commit**

```bash
git add src/service/core.rs
git commit -m "docs(core): add SAFETY comment for sync::Mutex in RateLimiter"
```

---

## Task 9: Refactor commit_ingestion_review into smaller methods

**Files:**
- Modify: `src/service/core.rs` (`commit_ingestion_review` method ~120+ lines)
- Test: Tests for ingestion review commit flow

- [ ] **Step 1: Analyze commit_ingestion_review logical steps**

Current method contains:
1. Поиск approved items
2. Создание source episode
3. Loop по entity items
4. Loop по fact items с созданием edges
5. Loop по edge items
6. Rebuild communities
7. Update draft status
8. Close session

- [ ] **Step 2: Extract private methods**

```rust
/// Validates session and extracts approved items for commit.
async fn prepare_commit(
    &self,
    session_id: &str,
) -> Result<(AppSession, String, String, Vec<Value>), MemoryError> {
    // ... extract logic
}

/// Creates source episode for the draft.
async fn commit_source_episode_for_draft(
    &self,
    draft_id: &str,
    scope: &str,
    approved: &[Value],
) -> Result<Option<String>, MemoryError> {
    // ... extract logic
}

/// Commits entity items and returns mapping of item ID to entity ID.
async fn commit_entities(
    &self,
    approved: &[Value],
    scope: &str,
) -> Result<(HashMap<String, String>, HashMap<String, String>), MemoryError> {
    // ... extract logic
}

/// Commits fact items and returns fact IDs with edge IDs from fact relations.
async fn commit_facts(
    &self,
    approved: &[Value],
    draft_id: &str,
    source_episode: Option<&str>,
    scope: &str,
    entity_ids_by_item: &HashMap<String, String>,
    entity_ids_by_name: &HashMap<String, String>,
) -> Result<(Vec<String>, Vec<String>), MemoryError> {
    // ... extract logic
}

/// Commits explicit edge items.
async fn commit_edges(
    &self,
    approved: &[Value],
    draft_id: &str,
    scope: &str,
    entity_ids_by_item: &HashMap<String, String>,
    entity_ids_by_name: &HashMap<String, String>,
) -> Result<Vec<String>, MemoryError> {
    // ... extract logic
}

/// Finalizes commit: rebuilds communities, updates draft, closes session.
async fn finalize_commit(
    &self,
    draft_id: &str,
    namespace: &str,
    session_id: &str,
    entity_count: usize,
    fact_ids: Vec<String>,
    edge_ids_from_facts: Vec<String>,
    edge_ids_explicit: Vec<String>,
    source_episode: Option<String>,
) -> Result<Value, MemoryError> {
    // ... extract logic
}
```

- [ ] **Step 3: Rewrite main method as orchestrator**

```rust
async fn commit_ingestion_review(&self, session_id: &str) -> Result<Value, MemoryError> {
    let (session, draft_id, namespace, approved) = self.prepare_commit(session_id).await?;
    let source_ep = self.commit_source_episode_for_draft(&draft_id, &session.scope, &approved).await?;

    let (entity_ids_by_item, entity_ids_by_name) =
        self.commit_entities(&approved, &session.scope).await?;

    let (fact_ids, edge_ids_from_facts) = self
        .commit_facts(&approved, &draft_id, source_ep.as_deref(), &session.scope, &entity_ids_by_item, &entity_ids_by_name)
        .await?;

    let edge_ids_explicit = self
        .commit_edges(&approved, &draft_id, &session.scope, &entity_ids_by_item, &entity_ids_by_name)
        .await?;

    self.finalize_commit(&draft_id, &namespace, session_id,
        entity_ids_by_item.len(), fact_ids, edge_ids_from_facts, edge_ids_explicit,
        source_ep).await
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test
```

- [ ] **Step 5: Commit**

```bash
git add src/service/core.rs
git commit -m "refactor(core): split commit_ingestion_review into 6 focused methods"
```

---

## Task 10: Add AddFactRequest struct for add_fact method

**Files:**
- Modify: `src/service/core.rs`
- Test: Tests for `add_fact`

- [ ] **Step 1: Locate add_fact signature**

Find the method with 10 parameters.

- [ ] **Step 2: Define AddFactRequest struct**

Add near `MemoryService` definition:

```rust
/// Input for creating a new fact record.
pub struct AddFactRequest<'a> {
    pub fact_type: &'a str,
    pub content: &'a str,
    pub quote: &'a str,
    pub source_episode: &'a str,
    pub t_valid: DateTime<Utc>,
    pub scope: &'a str,
    pub confidence: f64,
    pub entity_links: Vec<String>,
    pub policy_tags: Vec<String>,
    pub provenance: Value,
}
```

- [ ] **Step 3: Add new method signature**

```rust
pub async fn add_fact(&self, req: AddFactRequest<'_>) -> Result<String, MemoryError> {
    // ... implementation using req.field_name
}
```

- [ ] **Step 4: Add deprecated wrapper for backward compatibility (optional)**

If there are existing callers:

```rust
/// Legacy signature kept for backward compatibility.
/// 
/// # Deprecated
/// 
/// Use `add_fact(AddFactRequest)` instead.
#[deprecated(since = "0.1.0", note = "Use AddFactRequest struct instead")]
pub async fn add_fact_legacy(
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
) -> Result<String, MemoryError> {
    self.add_fact(AddFactRequest {
        fact_type,
        content,
        quote,
        source_episode,
        t_valid,
        scope,
        confidence,
        entity_links,
        policy_tags,
        provenance,
    })
    .await
}
```

- [ ] **Step 5: Update all internal call-sites**

Search for `self.add_fact(` calls and replace with struct initialization.

- [ ] **Step 6: Run tests**

```bash
cargo test --lib core
```

- [ ] **Step 7: Commit**

```bash
git add src/service/core.rs
git commit -m "refactor(core): add AddFactRequest struct, reduce parameter count from 10 to 1"
```

---

## Task 11: Split core.rs into APP-specific modules

**Files:**
- Modify: `src/service/core.rs` (reduce from ~2900 lines)
- Create: `src/service/inspector.rs` (APP-01)
- Create: `src/service/temporal_diff.rs` (APP-02)
- Create: `src/service/ingestion.rs` (APP-03)
- Create: `src/service/lifecycle_app.rs` (APP-04)
- Create: `src/service/graph_app.rs` (APP-05)
- Modify: `src/service/mod.rs` (add new module exports)
- Test: All tests (no behavior change expected)

- [ ] **Step 1: Identify APP boundaries**

Search for method patterns:
- APP-01: `open_inspector_entity`, `open_inspector_fact`, `archive_episode`
- APP-02: `open_temporal_diff`, `export_temporal_diff`, `open_memory_inspector_from_diff`
- APP-03: `open_ingestion_review`, `get_draft_summary`, `approve/reject/edit/cancel/commit`
- APP-04: `open_lifecycle_console`, `get_lifecycle_dashboard`, `archive_candidates`
- APP-05: `open_graph_path`, `expand_graph_neighbors`, `open_edge_details`, `use_path_as_context`

- [ ] **Step 2: Create inspector.rs (APP-01)**

```rust
// src/service/inspector.rs
use super::MemoryService;
use crate::models::*;
use serde_json::{Value, Map};

impl MemoryService {
    /// Opens the entity inspector view.
    pub async fn open_inspector_entity(
        &self,
        entity_id: &str,
        session_id: Option<&str>,
    ) -> Result<Value, MemoryError> {
        // ... move code from core.rs
    }

    /// Opens the fact inspector view.
    pub async fn open_inspector_fact(
        &self,
        fact_id: &str,
        session_id: Option<&str>,
    ) -> Result<Value, MemoryError> {
        // ... move code from core.rs
    }

    /// Archives an episode.
    pub async fn archive_episode(
        &self,
        episode_id: &str,
        session_id: Option<&str>,
    ) -> Result<Value, MemoryError> {
        // ... move code from core.rs
    }
}
```

- [ ] **Step 3: Create temporal_diff.rs (APP-02)**

Move `open_temporal_diff`, `export_temporal_diff`, `open_memory_inspector_from_diff`.

- [ ] **Step 4: Create ingestion.rs (APP-03)**

Move `open_ingestion_review`, `get_draft_summary`, `approve_ingestion_item`, `reject_ingestion_item`, `edit_ingestion_item`, `cancel_ingestion_review`, `commit_ingestion_review`.

- [ ] **Step 5: Create lifecycle_app.rs (APP-04)**

Move `open_lifecycle_console`, `get_lifecycle_dashboard`, `archive_candidates`, and related helpers.

- [ ] **Step 6: Create graph_app.rs (APP-05)**

Move `open_graph_path`, `expand_graph_neighbors`, `open_edge_details`, `use_path_as_context`.

- [ ] **Step 7: Update mod.rs**

```rust
// src/service/mod.rs
pub mod core;
pub mod inspector;
pub mod temporal_diff;
pub mod ingestion;
pub mod lifecycle_app;
pub mod graph_app;
// ... existing modules
```

- [ ] **Step 8: Leave in core.rs**

Keep only:
- `struct MemoryService`
- `build()`, `new()`, `new_from_env()`
- Infrastructure: `RateLimiter`, `cache`, `generate_embedding`
- Core domain methods: `resolve`, `add_fact`, `ingest`, `relate`, etc.
- NOT APP-specific methods

- [ ] **Step 9: Run all tests**

```bash
cargo test
```

Expected: All tests pass without modification (no behavior change)

- [ ] **Step 10: Commit**

```bash
git add src/service/*.rs
git commit -m "refactor(core): split God Object into APP-specific modules (inspector, temporal_diff, ingestion, lifecycle_app, graph_app)"
```

---

## Task 12: Add injectable clock for testability

**Files:**
- Modify: `src/service/core.rs`
- Modify: `src/service/query.rs` (`now()` function)
- Test: Time-dependent tests

- [ ] **Step 1: Locate `now()` calls**

Search for `super::query::now()` in `core.rs`:
- `ingest`
- `add_fact`
- `record_fact_access`
- `relate`

- [ ] **Step 2: Add optional clock field to ServiceBuildConfig**

```rust
pub struct ServiceBuildConfig {
    // ... existing fields
    /// Optional clock function for testability.
    /// If None, uses Utc::now().
    #[cfg(test)]
    pub clock: Option<Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>>,
}
```

- [ ] **Step 3: Add clock field to MemoryService**

```rust
pub struct MemoryService {
    // ... existing fields
    /// Clock function for deterministic testing.
    #[cfg(test)]
    clock: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
}
```

- [ ] **Step 4: Initialize clock in build()**

```rust
let service = MemoryService {
    // ... other fields
    #[cfg(test)]
    clock: config.clock.unwrap_or_else(|| Arc::new(Utc::now)),
};
```

- [ ] **Step 5: Replace `query::now()` calls with `self.now()`**

Add method:

```rust
#[cfg(test)]
fn now(&self) -> DateTime<Utc> {
    (self.clock)()
}

#[cfg(not(test))]
fn now(&self) -> DateTime<Utc> {
    Utc::now()
}
```

- [ ] **Step 6: Add test helper constructor**

```rust
#[cfg(test)]
pub(crate) fn new_with_clock(
    db_client: Arc<dyn DbClient>,
    namespaces: Vec<String>,
    log_level: String,
    rate_limit_rps: i32,
    rate_limit_burst: i32,
    clock: impl Fn() -> DateTime<Utc> + Send + Sync + 'static,
) -> Result<Self, MemoryError> {
    Self::build(ServiceBuildConfig {
        db_client,
        namespaces,
        log_level,
        rate_limit_rps,
        rate_limit_burst,
        clock: Some(Arc::new(clock)),
    })
}
```

- [ ] **Step 7: Run tests**

```bash
cargo test
```

- [ ] **Step 8: Commit**

```bash
git add src/service/core.rs src/service/query.rs
git commit -m "test(core): add injectable clock for deterministic time-dependent tests"
```

---

## Task 13: Update documentation and run final verification

**Files:**
- Modify: `docs/REVIEW_ALIGNMENT_2026-03-25.md` (update status)
- Modify: `README.md` (if needed)
- Run: Full verification

- [x] **Step 1: Update REVIEW_ALIGNMENT_2026-03-25.md**

Mark resolved issues:
- [x] `core.rs` God Object — split into modules
- [x] `add_fact` 10 params — `AddFactRequest` struct
- [x] MockDbClient duplication — consolidated
- [x] `resolve_*` duplication — `resolve_entity_by_type`
- [x] Session validation duplication — `require_app` / `require_target_str`
- [x] `fact_state` duplication — helper function
- [x] `find_record` duplication — `find_record_in_namespaces`
- [x] `require_non_empty` — validation helper
- [x] Namespace lookup idioms — fixed
- [x] `sync::Mutex` safety — documented
- [x] `commit_ingestion_review` SRP — split into 6 methods
- [x] Injectable clock — added for tests

- [x] **Step 2: Run full verification**

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
cargo doc --no-deps
```

- [x] **Step 3: Verify code metrics**

```bash
# Check core.rs line count (should be significantly reduced)
wc -l src/service/core.rs

# Check module structure
find src/service -name "*.rs" -exec wc -l {} \;
```

Observed on 2026-04-01:
- `core.rs`: 3,413 lines
- `app_modules.rs`: 1,331 lines
- Full verification pipeline passed

- [x] **Step 4: Final status recorded**

```bash
git add docs/REVIEW_ALIGNMENT_2026-03-25.md
git commit -m "docs: update review alignment with refactoring progress"
```

---

## Summary of Changes

| Task | File(s) | Change | Priority |
|------|---------|--------|----------|
| 1 | `src/service/validation.rs` | Add `require_non_empty` helper | 🟠 Medium |
| 2 | `src/service/core.rs` | Add `fact_state` helper | 🟠 Medium |
| 3 | `src/service/core.rs` | Add `find_record_in_namespaces` helper | 🟠 Medium |
| 4 | `src/service/core.rs` | Add `require_app` / `require_target_str` | 🟠 Medium |
| 5 | `src/service/core.rs` | Consolidate MockDbClient x4 → single `MockDb` | 🟠 Medium (tests only) |
| 6 | `src/service/core.rs` | Add `resolve_entity_by_type` delegate | 🟠 Medium |
| 7 | `src/service/core.rs` | Fix namespace lookup idioms, add prefix table | 🟡 Low |
| 8 | `src/service/core.rs` | Add SAFETY comment for `sync::Mutex` | 🟡 Low |
| 9 | `src/service/core.rs` | Split `commit_ingestion_review` into 6 methods | 🟡 Low |
| 10 | `src/service/core.rs` | Add `AddFactRequest` struct | 🔴 High |
| 11 | `src/service/*.rs` | Split God Object into 5 APP modules | 🔴 High |
| 12 | `src/service/core.rs`, `query.rs` | Add injectable clock for tests | 🟡 Low |
| 13 | `docs/` | Update documentation, final verification | 🟢 Integration |

---

## Dependencies

| Task | Dependencies |
|------|--------------|
| 1 (validation helper) | None |
| 2 (fact_state) | None |
| 3 (find_record) | None |
| 4 (session helpers) | None |
| 5 (mock consolidation) | None (tests only) |
| 6 (resolve_entity_by_type) | None |
| 7 (namespace idioms) | None |
| 8 (Mutex SAFETY) | None |
| 9 (commit_ingestion split) | None (can be done in parallel with 1-8) |
| 10 (AddFactRequest) | None (additive change) |
| 11 (God Object split) | Tasks 2, 3, 4, 6, 9 (helpers should be in place first) |
| 12 (injectable clock) | None |
| 13 (documentation) | All of above |

---

## Files to Modify

| File | Tasks | Lines Changed (est.) |
|------|-------|---------------------|
| `src/service/validation.rs` | 1 | +10, -8 |
| `src/service/core.rs` | 2, 3, 4, 6, 7, 8, 9, 10, 11, 12 | +150, -2000 (net reduction) |
| `src/service/inspector.rs` | 11 | +200 (new) |
| `src/service/temporal_diff.rs` | 11 | +150 (new) |
| `src/service/ingestion.rs` | 11 | +400 (new) |
| `src/service/lifecycle_app.rs` | 11 | +300 (new) |
| `src/service/graph_app.rs` | 11 | +250 (new) |
| `src/service/mod.rs` | 11 | +5 |
| `src/service/query.rs` | 12 | +5 |
| `docs/REVIEW_ALIGNMENT_2026-03-25.md` | 13 | +20 |

---

## Files NOT to Modify

| File | Reason |
|------|--------|
| `src/storage.rs` | No changes needed — storage layer is clean |
| `src/service/error.rs` | Already well-designed (noted in review) |
| `src/service/ids.rs` | Already well-designed (noted in review) |
| `src/service/cache.rs` | Already well-designed (noted in review) |
| `src/logging.rs` | Already well-designed (noted in review) |
| `src/migrations/*.surql` | No schema changes required |
| `src/mcp/handlers.rs` | No MCP API changes required |
| `Cargo.toml` | No new dependencies |

---

## Verification Commands

```bash
# After each task
cargo check
cargo clippy -- -D warnings
cargo test

# After Task 11 (God Object split)
wc -l src/service/core.rs  # Should be <1000 lines
find src/service -name "*.rs" -exec wc -l {} \;  # Verify module sizes

# Final verification (Task 13)
cargo fmt
cargo clippy -- -D warnings
cargo test
cargo doc --no-deps
```

---

## Notes

- **No public API changes:** All refactoring preserves existing MCP tool behavior. `AddFactRequest` is additive; old signature can be deprecated but not removed.
- **Test compatibility:** All existing tests pass without modification. Mock consolidation (Task 5) only affects test internals.
- **God Object split is mechanical:** No logic changes, only moving code between files. Each APP module uses `impl MemoryService` — no new types.
- **Order matters:** Complete Tasks 1-9 (helpers) before Task 11 (split) to minimize churn.
- **Clock injection is test-only:** `#[cfg(test)]` ensures zero runtime overhead in production.
- **SAFETY comment is documentation:** Does not change behavior, justifies `sync::Mutex` usage for Clippy and future reviewers.
- **Namespace prefix table is OCP-compliant:** Adding new prefix requires only data change, not code change.
