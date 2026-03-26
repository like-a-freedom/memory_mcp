# Memory Lifecycle Background Jobs Implementation Plan

> **Status:** ✅ **COMPLETE** (2026-03-26)
> **Implementation:** All tasks completed including integration tests. See `src/service/lifecycle/` module.

**Goal:** Implement background jobs for confidence decay refresh and episode archival to prevent unbounded growth and maintain memory hygiene.

**Architecture:** Two independent background workers running on configurable intervals: (1) decay job marks stale facts as invalid, (2) archival job marks old episodes as archived. Both workers are optional and controlled via environment flags.

**Tech Stack:** Tokio intervals, async tasks, SurrealDB batch updates, environment-based configuration.

---

## Implementation Status Summary

| Task | Status | Commits |
|------|--------|---------|
| Task 1: Lifecycle Configuration | ✅ Complete | `1a7b143` |
| Task 2: Decay Background Job | ✅ Complete | `c8aff5b` |
| Task 3: Episode Archival Job | ✅ Complete | `c8aff5b` |
| Task 4: Wire Up Workers | ✅ Complete | `343af30` |
| Task 5: Documentation | ✅ Complete | `c8acd51` |
| **Task 6: Integration Tests** | ✅ Complete | `c25b259` |

---

## File Structure

**Actual implementation:**

- ✅ **Created:** `src/service/lifecycle/decay.rs`
- ✅ **Created:** `src/service/lifecycle/archival.rs`
- ✅ **Created:** `src/service/lifecycle/mod.rs` (public)
- ✅ **Modified:** `src/service/mod.rs`
- ✅ **Modified:** `src/config.rs`
- ✅ **Modified:** `src/service/core.rs` (public namespace_for_scope, db_client)
- ✅ **Modified:** `.env.example`
- ✅ **Modified:** `README.md`
- ✅ **Created:** `docs/LIFECYCLE_BACKGROUND_JOBS.md`
- ✅ **Created:** `tests/lifecycle_decay.rs` (3 tests)
- ✅ **Created:** `tests/lifecycle_archival.rs` (3 tests)

---

## Task 1: Lifecycle Configuration ✅

**Files:** `src/config.rs`

- [x] **Step 1: Add lifecycle configuration struct** ✅
  - **Implemented:** `src/config.rs:153-171`

- [x] **Step 2: Add lifecycle config loading from env** ✅
  - **Implemented:** `src/config.rs:189-207`

- [x] **Step 3: Add lifecycle config to SurrealConfig** ✅
  - **Implemented:** `src/config.rs:42`

- [x] **Step 4: Add builder methods for lifecycle config** ✅
  - **Implemented:** `src/config.rs:324-328`

- [x] **Step 5: Add unit tests for lifecycle config** ✅
  - **Implemented:** `src/config.rs:504-544`

- [x] **Step 6: Run tests** ✅
  - **Result:** `cargo test config::tests --lib -v` — PASS (10 tests)

- [x] **Step 7: Commit** ✅
  - **Commit:** `1a7b143 feat(config): add LifecycleConfig for background jobs`

---

## Task 2: Decay Background Job ✅

**Files:** `src/service/lifecycle/decay.rs`

- [x] **Step 1: Create lifecycle module structure** ✅
  - **Implemented:** `src/service/lifecycle/mod.rs`

- [x] **Step 2: Create decay job implementation** ✅
  - **Implemented:** `src/service/lifecycle/decay.rs`
  - **Note:** Uses inline decay calculation: `base * exp(-λ * days)` where λ = 0.693/365

- [x] **Step 3: Export decayed_confidence_raw helper** ✅
  - **Resolved:** Inline decay formula used instead of separate helper

- [x] **Step 4: Write integration test for decay job** ✅
  - **Implemented:** `tests/lifecycle_decay.rs` (3 tests)
  - **Tests:** `decay_pass_with_empty_database`, `decay_pass_preserves_recent_high_confidence_facts`, `decay_pass_different_thresholds_produce_different_results`

- [x] **Step 5: Run decay test** ✅
  - **Command:** `cargo test --test lifecycle_decay -- --test-threads=1`
  - **Result:** PASS (3 tests)

- [x] **Step 6: Commit** ✅
  - **Commit:** `c8aff5b feat(lifecycle): implement decay and archival background workers`
  - **Commit:** `c25b259 test: add integration tests for lifecycle and provenance`

---

## Task 3: Episode Archival Background Job ✅

**Files:** `src/service/lifecycle/archival.rs`

- [x] **Step 1: Create archival job implementation** ✅
  - **Implemented:** `src/service/lifecycle/archival.rs`

- [x] **Step 2: Write integration test for archival job** ✅
  - **Implemented:** `tests/lifecycle_archival.rs` (3 tests)
  - **Tests:** `archival_pass_with_empty_database`, `archival_pass_preserves_episodes_with_active_facts`, `archival_pass_different_thresholds`

- [x] **Step 3: Run archival tests** ✅
  - **Command:** `cargo test --test lifecycle_archival -- --test-threads=1`
  - **Result:** PASS (3 tests)

- [x] **Step 4: Commit** ✅
  - **Commit:** `c8aff5b feat(lifecycle): implement decay and archival background workers`
  - **Commit:** `c25b259 test: add integration tests for lifecycle and provenance`

---

## Task 4: Wire Up Lifecycle Workers ✅

**Files:** `src/service/core.rs`, `src/service/mod.rs`

- [x] **Step 1: Update lifecycle module to export spawn functions** ✅
  - **Implemented:** `src/service/lifecycle/mod.rs`
  - **Made public:** `pub mod lifecycle` for testing access

- [x] **Step 2: Add lifecycle worker spawning to MemoryService** ✅
  - **Implemented:** `src/service/core.rs:135-172`
  - **Note:** Workers spawned in `new_from_env()`

- [x] **Step 3: Add lifecycle module to service exports** ✅
  - **Implemented:** `src/service/mod.rs:30`

- [x] **Step 4: Run full test suite** ✅
  - **Result:** `cargo test --lib -v` — PASS (269 tests)

- [x] **Step 5: Run clippy** ✅
  - **Result:** `cargo clippy -- -D warnings` — PASS

- [x] **Step 6: Commit** ✅
  - **Commit:** `343af30 feat(lifecycle): wire up workers in MemoryService::new_from_env`

---

## Task 5: Documentation and Environment Setup ✅

**Files:** `.env.example`, `README.md`, `docs/LIFECYCLE_BACKGROUND_JOBS.md`

- [x] **Step 1: Update .env.example** ✅
  - **Implemented:** `.env.example:34-48`

- [x] **Step 2: Update README.md** ✅
  - **Implemented:** `README.md:165-173, 183-188`

- [x] **Step 3: Create lifecycle documentation** ✅
  - **Implemented:** `docs/LIFECYCLE_BACKGROUND_JOBS.md` (full implementation guide)

- [x] **Step 4: Commit** ✅
  - **Commit:** `c8acd51 docs: add lifecycle background jobs documentation`

---

## Task 6: Integration Tests ✅

**Files:** `tests/lifecycle_decay.rs`, `tests/lifecycle_archival.rs`

- [x] **Step 1: Create lifecycle decay tests** ✅
  - **File:** `tests/lifecycle_decay.rs`
  - **Tests:** 3 tests covering empty database, recent facts preservation, threshold differences

- [x] **Step 2: Create lifecycle archival tests** ✅
  - **File:** `tests/lifecycle_archival.rs`
  - **Tests:** 3 tests covering empty database, active facts preservation, threshold differences

- [x] **Step 3: Make MemoryService APIs public for testing** ✅
  - **Modified:** `src/service/core.rs` — `pub fn namespace_for_scope()`, `pub db_client`

- [x] **Step 4: Run all lifecycle tests** ✅
  - **Command:** `cargo test --test lifecycle_decay --test lifecycle_archival -- --test-threads=1`
  - **Result:** PASS (6 tests total)

- [x] **Step 5: Commit** ✅
  - **Commit:** `c25b259 test: add integration tests for lifecycle and provenance`

---

## Verification

**All verification steps completed:**

```bash
cargo fmt — ✅
cargo check — ✅
cargo clippy -- -D warnings — ✅
cargo test --lib — ✅ (269 tests passed)
cargo test --test lifecycle_decay --test lifecycle_archival -- --test-threads=1 — ✅ (6 tests passed)
```

---

## Running Tests

**Note:** Tests require `--test-threads=1` due to embedded SurrealDB LOCK file contention:

```bash
# Run lifecycle tests
cargo test --test lifecycle_decay --test lifecycle_archival -- --test-threads=1

# Run all tests
cargo test -- --test-threads=1
```

---

## Summary

**Lifecycle background jobs are fully implemented and tested.** 

- Workers can be enabled via `LIFECYCLE_ENABLED=true` environment variable
- Module is public (`pub mod lifecycle`) for testing access
- **6 integration tests** verify decay and archival logic
- Full documentation in `.env.example`, `README.md`, and `docs/LIFECYCLE_BACKGROUND_JOBS.md`
