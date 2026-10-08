# Logging quality, correlation and coverage — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make logs operable: one request id across a unit of work, a stated and enforced level policy, silent failures and security refusals made visible, and a testable operation-name contract — without changing the line format or the public configuration.

**Architecture:** `logging` stays the single emission choke point. A new `logging::correlation` task-local carries the request id; the formatter injects it into any event that lacks one, and the `http_request` span is retired. A failed operation takes its level from `MemoryError::log_level()`. Operation names are constrained by a pinned `OP_NAMESPACES` registry and a source-scanning lint. Producers stay dumb: no id threading, no per-subsystem correlation.

**Tech Stack:** Rust 1.99.0, `tokio` (`task_local!`), `tracing`/`tracing-subscriber`, `axum`/`tower`, `metrics`, `serde_json`.

**Spec:** `docs/superpowers/specs/2026-10-08-logging-quality-correlation-and-coverage.md`; decisions in ADR-0079, ADR-0080, ADR-0081. Read the spec and the ADRs alongside this plan.

**Baseline:** the working tree of 2026-10-08, including the in-progress `http-bounded-memory` change. All references are to files and symbols, not line numbers.

## Global Constraints

- Rust channel `1.99.0`; no new dependencies, no `Cargo.toml` changes.
- Do not change the line shape, token order, `MEMORY_LOG_FORMAT`, `MEMORY_LOG_COLOR`, `MEMORY_LOG_FILE`, `MEMORY_LOG_TARGETS`, or `RUST_LOG` `prefix=level` semantics.
- The only compatibility break is the eight `op` renames in ADR-0081.
- A new `http.*` operation name must be listed in `HTTP_OPERATIONS` **and** be
  readable by the inventory scanner: if it is emitted from a file the scanner does
  not `include_str!`, add that file to the scanner's list in the same change, or
  the completeness test fails.
- Logs go to **stderr** (stdout is the MCP stdio channel); the file sink stays opt-in and append-only.
- No secrets, API keys, tokens, or raw credentials in any event; ids used as log values are UUID-validated before storage.
- Metric labels stay a closed vocabulary; never add an unbounded value (ADR-0005).
- No `unwrap()` in production code; `fs-watch`, `mcp-apps` and `streamable-http` must all build.
- Business logic stays in the owning context; `main.rs` stays thin; no new MCP tools.
- Every new `http.*` operation name must be added to `HTTP_OPERATIONS`, or the inventory completeness test fails.
- Gate before shipping: `cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings` (zero warnings), `cargo fmt --all --check` (zero diff), `cargo test -p memory_mcp`.
- Run the smallest test set per task; escalate to the workspace gate only at the last task.
- Each task ends with an isolated commit; do not commit unrelated work.

## Review Focus

1. An inbound `x-request-id` that is not a UUID (log injection / unbounded value): discarded and replaced, never stored — Task 3.
2. An event that already carries its own `request_id` (the HTTP access log, preflight): injection must not overwrite it — Task 2.
3. Two requests in flight at once: one request's id must never appear on a line the other emits (task-local isolation, not a process-global) — Task 1.
4. An `op` whose first segment is unregistered (a bare `model_loader`) and an empty `op` (a foreign event): the lint must fail on the former, and the formatter must not emit a bare `op=` token for the latter — Tasks 6 and 2.
5. `MemoryError::Unavailable` (an HTTP 503-class "cannot serve now"): an actionable failure, so `ERROR`, not `WARN` — Task 5.

---

## File structure and responsibilities

`crates/memory-mcp/src/` is abbreviated `src/`.

| Slice | Files | Responsibility |
|---|---|---|
| Correlation | Create `src/logging/correlation.rs`; modify `src/logging.rs` | Ambient request id: `current`, `scope` |
| Formatter | Modify `src/logging.rs` (`format_event`, remove `span_fields`) | Inject `req`; retire the request span |
| HTTP boundary | Modify `src/http/logging.rs` (`request_log`) | Open the scope; validate the inbound id |
| Tool boundary | Modify `src/tools/*.rs`, `src/tools/context.rs`, `src/service/memory_container_shims/tool_context_impl.rs`, `src/service/core.rs`, `src/mcp/handlers.rs` | Adopt the ambient id; drop `ToolEvent.request_id` |
| Levels | Modify `src/shared/error.rs`, `src/tools/*.rs`, `src/mcp/handlers.rs` | `MemoryError::log_level()`; use it |
| Naming | Modify `src/logging.rs`, `src/bootstrap/stdio.rs`, `src/service/startup.rs`, `src/embedding/{model_loader,providers}.rs`, `src/memory/episode/fact_extraction.rs`, `src/http/logging.rs` | `OP_NAMESPACES`; renames; lint; scanner coverage |
| Field hygiene | Modify `src/http/logging.rs`, `src/platform/log_event.rs`, `src/embedding/{model_loader,providers}.rs`, `src/http/middleware/preflight.rs`, the `log_args_with_duration` call sites | `error`; top-level `duration_ms` |
| Coverage | Modify `src/memory/episode/triples.rs`, `src/service/fs_watch/{processor,runtime}.rs`, `src/storage/migrations.rs`, `src/http/leases/migration.rs`, `src/bootstrap/integration/registry_operations.rs`, `src/http/registry/surreal_store.rs`, `src/knowledge/entity_extraction/gliner.rs`, `src/http/principal/auth.rs`, `src/logging.rs` | Log discarded `Result`s |
| Security | Modify `src/http/middleware/auth.rs`, `src/http/principal/auth.rs`, `src/http/logging.rs`, `src/logging.rs`, `src/observability.rs` | `http.auth.rejected`, `http.auth.touch_failed` + a bounded counter |
| Readiness | Modify `src/http/health.rs`, `src/http.rs`, `src/logging.rs` | Log readiness transitions |
| Binary hygiene | Modify `src/bin/memory_mcp_http.rs`, `src/logging.rs` | Post-install errors through the logger |
| Docs | Create `docs/operations/LOGGING.md`; modify `AGENTS.md`, `README.md`, `docs/BACKLOG.md` | Conventions, runbook, record |

---

### Task 1: Ambient correlation context

**Files:** create `crates/memory-mcp/src/logging/correlation.rs`; modify `crates/memory-mcp/src/logging.rs` (declare `pub mod correlation;`).

**Interfaces:**
- Consumes: `tokio::task_local!`.
- Produces: `pub fn current() -> Option<String>`; `pub async fn scope<F: Future>(id: impl Into<String>, future: F) -> F::Output`.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn current_is_none_outside_a_scope() {
    assert_eq!(crate::logging::correlation::current(), None);
}

#[tokio::test]
async fn scope_makes_the_id_visible_inside() {
    let seen = crate::logging::correlation::scope("req_a", async {
        crate::logging::correlation::current()
    })
    .await;
    assert_eq!(seen.as_deref(), Some("req_a"));
}

#[tokio::test]
async fn concurrent_scopes_do_not_cross_tasks() {
    let a = tokio::spawn(crate::logging::correlation::scope("req_a", async {
        tokio::task::yield_now().await;
        crate::logging::correlation::current()
    }));
    let b = tokio::spawn(crate::logging::correlation::scope("req_b", async {
        tokio::task::yield_now().await;
        crate::logging::correlation::current()
    }));
    assert_eq!(a.await.unwrap().as_deref(), Some("req_a"));
    assert_eq!(b.await.unwrap().as_deref(), Some("req_b"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p memory_mcp correlation`
Expected: FAIL — `logging::correlation` does not exist.

- [ ] **Step 3: Implement `correlation.rs`**

Back the id with `tokio::task_local! { static REQUEST_ID: String; }`; `scope` runs `REQUEST_ID.scope(id.into(), future).await`; `current` uses `REQUEST_ID.try_with(Clone::clone).ok()`. Declare the module from `logging.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p memory_mcp correlation`
Expected: PASS (three tests).

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/logging.rs crates/memory-mcp/src/logging/correlation.rs
git commit -m "feat(logging): ambient request-id correlation context"
```

---

### Task 2: Formatter injects `request_id`; retire the request span

**Files:** modify `crates/memory-mcp/src/logging.rs` — `MemoryFormat::format_event`, `render_foreign`; delete `span_fields`.

**Interfaces:**
- Consumes: `crate::logging::correlation::current`.
- Produces: no new public item. `render_foreign` loses its `span_context` parameter and takes the ambient id from `current()`.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn an_own_event_without_a_request_id_gets_the_ambient_one() {
    // emit an `op`-bearing event inside correlation::scope("req_x");
    // assert the rendered line contains "req=req_x".
}

#[tokio::test]
async fn an_explicit_request_id_is_not_overwritten() {
    // an event whose payload already has request_id="req_own", emitted inside
    // correlation::scope("req_ambient"), renders "req=req_own".
}

#[tokio::test]
async fn a_foreign_event_carries_the_ambient_request_id() {
    // a foreign tracing::warn!(target="surrealdb::kvs", ...) inside
    // correlation::scope("req_x") renders a "req=req_x" token.
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p memory_mcp logging::tests`
Expected: FAIL — own events carry no `req`; foreign events read the span, which no longer holds the id.

- [ ] **Step 3: Implement injection**

In `format_event`, compute `let req = correlation::current();` once. For the `payload` and `op` branches, insert `request_id` into the event map when it is absent and `req` is `Some`. For the foreign branch, pass the id into `render_foreign` and render it as the first token after the message. Drop `span_fields` and the `FmtContext::event_scope` lookup; rename the `ctx` parameter to `_ctx`. Remove the `FormattedFields` import (its only use was `span_fields`). Keep the `FmtContext` and `LookupSpan` imports: the `FormatEvent` trait signature and the impl's `S: LookupSpan` bound still name them. (`tracing::Instrument` in `http/logging.rs` is unused only after Task 3 drops the span, so it is removed there, not here.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p memory_mcp logging::tests`
Expected: PASS; the span test removed in Step 1 is gone.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/logging.rs
git commit -m "feat(logging): inject the ambient request id into every event"
```

---

### Task 3: HTTP boundary opens the correlation scope

**Files:** modify `crates/memory-mcp/src/http/logging.rs` — `request_log`.

**Interfaces:**
- Consumes: `crate::logging::correlation::scope`.
- Produces: unchanged `RequestLog` event and `x-request-id` response header.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn a_handler_sees_the_request_correlation_id() {
    // a handler captures correlation::current(); assert it equals the UUID
    // echoed in the response x-request-id header.
}

#[tokio::test]
async fn a_non_uuid_request_id_header_is_replaced() {
    // send x-request-id: not-a-uuid; assert the response id is a fresh UUID
    // and the captured correlation id is that UUID, never the header value.
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p memory_mcp http::logging`
Expected: FAIL — the handler's `correlation::current()` is `None` (the id lives only in a span).

- [ ] **Step 3: Implement**

Replace `next.run(req).instrument(span)` with `crate::logging::correlation::scope(request_id.to_string(), next.run(req)).await`, and delete the `info_span!`. Leave id resolution (adopt a valid UUID, else mint) as it is, and leave the access-log event untouched.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p memory_mcp http::logging`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/http/logging.rs
git commit -m "feat(http): carry the request id in the correlation scope, not a span"
```

---

### Task 4: Tools adopt the ambient id; drop `ToolEvent.request_id`

**Files:** modify `crates/memory-mcp/src/tools/context.rs`, `crates/memory-mcp/src/tools/{ingest,extract,resolve,explain,invalidate,assemble_context}.rs`, `crates/memory-mcp/src/service/memory_container_shims/tool_context_impl.rs`, `crates/memory-mcp/src/service/core.rs` (`log_tool_event`, `log_tool_event_with_duration`), `crates/memory-mcp/src/mcp/handlers.rs` (`MemoryMcp::next_request_id` and its call sites in `open_app`/`app_command`).

**Interfaces:**
- Consumes: `crate::logging::correlation::{current, scope}`, `crate::tools::request_id::next_request_id`.
- Produces: `ToolEvent { op, args, result, level, duration }` (no `request_id`); `log_tool_event`/`log_tool_event_with_duration` lose their `request_id: Option<&str>` parameter.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn a_tool_adopts_an_ambient_request_id() {
    // StubContext::record captures correlation::current(); call ingest inside
    // correlation::scope("req_x", ...); assert every recorded event saw "req_x".
}

#[tokio::test]
async fn a_tool_mints_an_id_when_none_is_ambient() {
    // call ingest with no outer scope; assert it saw Some("req_NNNN").
}
```

`StubContext::record` records `(event.op, correlation::current())`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p memory_mcp tools::ingest`
Expected: FAIL — `ToolEvent` still carries `request_id`; `current()` is `None` inside the tool.

- [ ] **Step 3: Implement**

Wrap each canonical tool body once:

```rust
let id = crate::logging::correlation::current()
    .unwrap_or_else(crate::tools::request_id::next_request_id);
crate::logging::correlation::scope(id, async move { /* body */ }).await;
```

Delete every `request_id: Some(...)` field and remove `request_id` from `ToolEvent`. Drop the parameter from the two `log_tool_event*` helpers and stop passing it in `tool_context_impl.rs`. Give `open_app` and `app_command` the same scope wrapper and delete their minted id and the argument. `MemoryMcp::next_request_id` then has no caller and is removed. `log_event` keeps its `request_id` parameter (other callers pass `None`); this path passes `None`, because the formatter supplies the id.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p memory_mcp tools`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/tools crates/memory-mcp/src/service crates/memory-mcp/src/mcp/handlers.rs
git commit -m "refactor(tools): adopt the ambient request id instead of minting a second one"
```

---

### Task 5: `MemoryError::log_level()` and tool error levels

**Files:** modify `crates/memory-mcp/src/shared/error.rs`; `crates/memory-mcp/src/tools/{ingest,extract,resolve,explain,invalidate,assemble_context}.rs`; `crates/memory-mcp/src/mcp/handlers.rs` (`open_app.error`, `app_command.error`).

**Interfaces:**
- Produces: `impl MemoryError { pub fn log_level(&self) -> crate::logging::LogLevel }`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn storage_and_unavailable_errors_are_errors() {
    use crate::logging::LogLevel::*;
    assert_eq!(MemoryError::Storage("x".into()).log_level(), Error);
    assert_eq!(MemoryError::Unavailable("x".into()).log_level(), Error);
}

#[test]
fn refusal_and_validation_errors_are_warnings() {
    use crate::logging::LogLevel::*;
    for e in [
        MemoryError::NotFound("x".into()),
        MemoryError::Validation("x".into()),
        MemoryError::Conflict("x".into()),
        MemoryError::Auth("x".into()),
        MemoryError::BudgetExhausted("x".into()),
    ] {
        assert_eq!(e.log_level(), Warn);
    }
}

#[tokio::test]
async fn a_tool_internal_failure_logs_at_error() {
    // StubContext returns MemoryError::Storage → the recorded event's level is Error.
}

#[tokio::test]
async fn a_tool_validation_failure_logs_at_warn() {
    // StubContext returns MemoryError::Validation → recorded level is Warn.
}
```

`StubContext::record` records `(event.op, event.level)`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p memory_mcp shared::error`
Expected: FAIL to compile — `MemoryError::log_level` does not exist.

- [ ] **Step 3: Implement**

Add `log_level` with the spec §4.1 mapping. In the six tools, replace `level: LogLevel::Warn` on the error event with `level: err.log_level()`. Do the same for `open_app.error` and `app_command.error`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p memory_mcp tools && cargo test -p memory_mcp shared::error`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/shared/error.rs crates/memory-mcp/src/tools crates/memory-mcp/src/mcp/handlers.rs
git commit -m "feat(logging): classify a failed operation by its error, not its callsite"
```

---

### Task 6: Operation-namespace registry, lint, and renames

**Files:** modify `crates/memory-mcp/src/logging.rs` (`OP_NAMESPACES`); `crates/memory-mcp/src/bootstrap/stdio.rs` (`startup`, `startup.version_probe_failed`); `crates/memory-mcp/src/service/startup.rs` (`startup.versions`, and its test); `crates/memory-mcp/src/embedding/model_loader.rs` (`model_loader`); `crates/memory-mcp/src/embedding/providers.rs` (`dimension_override_mismatch`, `cosine_similarity.dimension_mismatch`); `crates/memory-mcp/src/memory/episode/fact_extraction.rs` (`extract_from_episode.start`/`.done`); `crates/memory-mcp/src/http/logging.rs` (the scanner's `include_str!` list). Test in `crates/memory-mcp/src/logging.rs`.

**Interfaces:**
- Produces: `pub const OP_NAMESPACES: &[&str]` — the 24 namespaces from ADR-0081: `["assemble_context","app_command","cache","config","control","db","embedding","explain","extract","fs_watch","graph","http","ingest","invalidate","knowledge","lifecycle","main","ner","oidc","open_app","reembed","resolve","schema","triple_extraction"]`.

- [ ] **Step 1: Write the failing lint**

```rust
#[test]
fn every_emitted_operation_names_a_registered_namespace() {
    // walk ${CARGO_MANIFEST_DIR}/src for *.rs (std::fs::read_dir, no new dep;
    // env!("CARGO_MANIFEST_DIR") anchors the path, since a test's working
    // directory is not guaranteed) and
    // collect op literals from these idioms:
    //   a literal assigned to an "op" key, in any wrapper it appears in
    //   (serde_json::json!, Value::String, .into(), a map/JSON key, event!), or
    //   a format!("prefix.…") template -> the prefix
    //   op: "literal"   (struct field, e.g. ToolEvent)
    //   op = "literal"  (native tracing)
    //   the first string argument of log_event( / log_claim_event( / log_op(
    // assert namespace_of(op) is in OP_NAMESPACES for every collected op.
    // Do NOT treat an arbitrary dotted literal (e.g. the `mode` label
    // "cli.lifecycle") as an op — only the idioms above.
}

#[test]
fn op_namespaces_are_lowercase_snake_case_and_unique() {
    // every entry matches ^[a-z][a-z0-9_]*$; no duplicates.
}
```

- [ ] **Step 2: Run the lint to verify it fails**

Run: `cargo test -p memory_mcp registered_namespace`
Expected: FAIL on `startup`, `startup.version_probe_failed`, `startup.versions`, `model_loader`, `dimension_override_mismatch`, `cosine_similarity.dimension_mismatch`, `extract_from_episode.start`, `extract_from_episode.done` — and on any test-only literal the walk flags (for example the native `op = "x"` fixture in `logging.rs`, which is conformed to a registered namespace rather than excluded).

- [ ] **Step 3: Implement**

Add `OP_NAMESPACES`. Apply the eight renames from ADR-0081. Conform any other literal the walk flags — including a test-only op — to a registered namespace rather than excluding it from the walk. In `http/logging.rs`, add the files the later tasks emit new `http.*` names from to the scanner's `include_str!` list — `health.rs`, `principal/auth.rs`, and `../bin/memory_mcp_http.rs` — so the inventory is genuinely exhaustive. None carries an `http.*` literal yet, so the completeness test is unaffected at this point; the tasks that emit those names add them to `HTTP_OPERATIONS`.

- [ ] **Step 4: Run the lint to verify it passes**

Run: `cargo test -p memory_mcp registered_namespace && cargo test -p memory_mcp http::logging`
Expected: PASS (the extended inventory scan finds no new name, so the completeness test is still green).

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/logging.rs crates/memory-mcp/src/bootstrap/stdio.rs crates/memory-mcp/src/service/startup.rs crates/memory-mcp/src/embedding/model_loader.rs crates/memory-mcp/src/embedding/providers.rs crates/memory-mcp/src/memory/episode/fact_extraction.rs crates/memory-mcp/src/http/logging.rs
git commit -m "feat(logging): pin operation namespaces and conform the outliers"
```

---

### Task 7: Field-name hygiene

**Files:** modify `crates/memory-mcp/src/http/logging.rs` (`RequestWarning::log_into`: `detail` → `error`); `crates/memory-mcp/src/platform/log_event.rs` (remove `log_args_with_duration`, and its test in `src/service/core.rs`); `crates/memory-mcp/src/embedding/model_loader.rs` and `crates/memory-mcp/src/embedding/providers.rs` (`message` → `error`); `crates/memory-mcp/src/http/middleware/preflight.rs` (`emit_preflight_log`).

Update every `log_args_with_duration` call site to pass the duration to `log_event`'s `duration_ms` argument instead:
`src/embedding/service.rs` (three, in `generate_with_provider`),
`src/knowledge/entity_extraction/gliner.rs` (`acquire_inference_permit`,
`build_span_scoring_log_event`), `src/knowledge/entity_extraction/lfm2_gliner/model.rs`
(`compute_span_scores`), `src/knowledge/entity_extraction/lfm2_gliner.rs`
(`acquire_inference_permit`), `src/memory/capabilities/extract.rs` (`extract_with`),
`src/memory/episode/entity_extraction.rs` (`extract_entities`, `log_ner_error`),
`src/memory/episode/fact_extraction.rs` (`extract_from_episode`).

**Interfaces:**
- Produces: one error field name (`error`); `duration_ms` top-level only.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_request_warning_names_its_error_field() {
    // the RequestWarning line contains "error=" and not "detail=".
}

#[tokio::test]
async fn a_generation_duration_is_a_top_level_token() {
    // the "embedding.generate.skipped" line carries top-level "duration_ms=",
    // not "args.duration_ms=".
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p memory_mcp request_warning && cargo test -p memory_mcp embedding::service`
Expected: FAIL.

- [ ] **Step 3: Implement**

Rename `detail` to `error`. Replace each `log_args_with_duration(args, d)` with `log_event(op, args, result, access, None, Some(d_ms))`, moving the duration to the top-level parameter; then delete `log_args_with_duration` and its test. Rename the `message` field to `error` in `model_loader.rs` and `providers.rs`. In `emit_preflight_log`, replace `StdoutLogger::from_env().log(...)` with `crate::logging::emit(...)`, keeping the level.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p memory_mcp platform::log_event && cargo test -p memory_mcp embedding`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/http/logging.rs crates/memory-mcp/src/platform/log_event.rs crates/memory-mcp/src/embedding crates/memory-mcp/src/http/middleware/preflight.rs crates/memory-mcp/src/service/core.rs
git commit -m "refactor(logging): one error field name and one duration placement"
```

---

### Task 8: Log the discarded `Result`s

**Files:** modify each site to emit an event on `Err`:
- `src/memory/episode/triples.rs` → `triple_extraction.persist_failed`, `triple_extraction.reconcile_failed` — `WARN`.
- `src/service/fs_watch/processor.rs` → `fs_watch.mark_failed_failed` — `WARN`.
- `src/service/fs_watch/runtime.rs` → `fs_watch.recover_failed` — `WARN`.
- `src/storage/migrations.rs` → `db.migration_mark_failed` — `WARN`.
- `src/http/leases/migration.rs` → reuse the existing `http.lease.release_failed` — `WARN`.
- `src/bootstrap/integration/registry_operations.rs` → the `release_deletion_lease` adapter swallows the inner `release_provisioning_lease`; reuse the existing `http.lease.release_failed` — `WARN`. Leave the port-level discard in `src/operations/api.rs` alone: the adapter returns `Ok` regardless, so an event there could never fire.
- `src/http/registry/surreal_store.rs` → `db.migration_mark_failed` — `WARN`.
- `src/knowledge/entity_extraction/gliner.rs` → `knowledge.reject_candidate_failed` — `WARN`.
- `src/http/principal/auth.rs` → `http.auth.touch_failed` — `DEBUG`.

Also modify `src/logging.rs` (`HTTP_OPERATIONS`).

Every new `op` namespaces under an `OP_NAMESPACES` member. Add the one new `http.*` name (`http.auth.touch_failed`) to `HTTP_OPERATIONS`; the scanner already reads `principal/auth.rs` from Task 6.

**Interfaces:**
- Consumes: `crate::logging::emit`, `LogLevel`.
- Produces: no behavior change — `let _ =` becomes `if let Err(error) = … { emit(...) }`, control flow unchanged.

- [ ] **Step 1: Write the failing tests (one per touched module, using `logging::capture`)**

```rust
#[tokio::test]
async fn a_failed_triple_persist_is_logged() {
    // a controlled triple_store whose create_triple returns Err; run the path;
    // assert capture contains op=triple_extraction.persist_failed at WARN.
}
```

Repeat for `fs_watch.mark_failed_failed`, `db.migration_mark_failed`,
`http.lease.release_failed`, and `http.auth.touch_failed` at `DEBUG`. Use the
existing `capture` guard. Where the site runs in a `tokio::spawn` (the triple
path and the two fs-watch runtime spawns), the controlled store signals through a
`tokio::sync::Notify` or a oneshot when the discarded call returns `Err`; the
test awaits that signal under a bounded `tokio::time::timeout`, then reads
`capture`. No sleeps, no unbounded waits.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p memory_mcp triple_extraction && cargo test -p memory_mcp fs_watch && cargo test -p memory_mcp migration`
Expected: FAIL — capture is empty.

- [ ] **Step 3: Implement**

At each site keep best-effort semantics and add an event carrying `op` and
`error` (the `Display` of the discarded error). Change nothing about what is
returned or whether the failure is swallowed. Add `http.auth.touch_failed` to
`HTTP_OPERATIONS`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p memory_mcp triple_extraction && cargo test -p memory_mcp fs_watch && cargo test -p memory_mcp migration && cargo test -p memory_mcp auth`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/memory/episode/triples.rs crates/memory-mcp/src/service/fs_watch crates/memory-mcp/src/storage/migrations.rs crates/memory-mcp/src/http/leases/migration.rs crates/memory-mcp/src/bootstrap/integration/registry_operations.rs crates/memory-mcp/src/http/registry/surreal_store.rs crates/memory-mcp/src/knowledge/entity_extraction/gliner.rs crates/memory-mcp/src/http/principal/auth.rs crates/memory-mcp/src/logging.rs
git commit -m "feat(logging): make best-effort failures visible"
```

---

### Task 9: Authentication refusals are logged and counted

**Files:** modify `crates/memory-mcp/src/http/middleware/auth.rs` (`authenticate`); `crates/memory-mcp/src/http/principal/auth.rs` (`authenticate_bearer`); `crates/memory-mcp/src/http/logging.rs` (add `log_auth_rejection`); `crates/memory-mcp/src/logging.rs` (add `http.auth.rejected` to `HTTP_OPERATIONS`); `crates/memory-mcp/src/observability.rs` (`record_auth_refusal` gains a `surface` parameter). Update the existing caller `crates/memory-mcp/src/control/oidc/handlers.rs` (`rejection_event`) and the two `record_auth_refusal` calls in the `observability.rs` tests.

**Interfaces:**
- Produces:
  - `pub(crate) fn log_auth_rejection(reason: AuthRejection, request_id: Option<&str>)`
  - `enum AuthRejection { Missing, BadScheme, Parse, RateLimited, CachedRejection, VerifyFailed }` with `as_str`. The last two mirror the two distinct `authenticate_bearer` branches: a negative-cache hit and a signature/secret mismatch.
  - `op = "http.auth.rejected"`, `reason = <as_str>`, `WARN`, added to `HTTP_OPERATIONS`.
  - `observability::record_auth_refusal(surface, branch)`; `surface` gains the closed value `"bearer"` (the OIDC caller keeps `"oidc"`).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_missing_bearer_is_logged_with_reason_missing() { /* capture has op=http.auth.rejected reason=missing */ }

#[test]
fn a_rate_limited_key_is_logged_with_reason_rate_limited() { /* reason=rate_limited */ }

#[test]
fn a_refusal_line_never_contains_the_credential() {
    // a credential whose secret is "supersecret"; assert the line lacks it.
}

#[test]
fn the_auth_refusal_metric_surface_is_bounded() {
    // record_auth_refusal("bearer", "verify_failed") reaches the exposition
    // with surface="bearer".
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p memory_mcp a_missing_bearer`
Expected: FAIL to compile — `log_auth_rejection` and `AuthRejection` do not exist.

- [ ] **Step 3: Implement**

Map each refusal to an `AuthRejection`, call `log_auth_rejection`, and increment the metric. Log **inside `authenticate_bearer`** for `parse`, `rate_limited`, `cached_rejection`, `verify_failed` (the branches it owns), and **in `authenticate`** for `missing` (no `Authorization` header) and `bad_scheme` (a non-Bearer or malformed header). Leave `AuthDecision` unchanged — the reason is known at each refusal site, so no signature change is needed. `AuthRejection` is a closed enum, so its `as_str` is the bounded label. Add `http.auth.rejected` to `HTTP_OPERATIONS`. Never log the credential or key material.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p memory_mcp auth && cargo test -p memory_mcp observability`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/http/middleware/auth.rs crates/memory-mcp/src/http/principal/auth.rs crates/memory-mcp/src/http/logging.rs crates/memory-mcp/src/logging.rs crates/memory-mcp/src/observability.rs crates/memory-mcp/src/control/oidc/handlers.rs
git commit -m "feat(http): log and count authentication refusals"
```

---

### Task 10: Readiness transitions are logged once

**Files:** modify `crates/memory-mcp/src/http/health.rs` (`ready`); `crates/memory-mcp/src/http.rs` (add a last-readiness cell to `HttpState`); `crates/memory-mcp/src/logging.rs` (add `http.readiness.changed` to `HTTP_OPERATIONS`).

**Interfaces:**
- Produces: `op = "http.readiness.changed"`, field `state` (`ready` / `shutting_down` / `admission_closed` / `registry_unreachable`), emitted only on a change; added to `HTTP_OPERATIONS`. Plus a cell on `HttpState` initialized to "unknown".

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn a_readiness_transition_is_logged_once() {
    // unknown -> ready emits one http.readiness.changed state=ready;
    // a second ready probe emits nothing.
}

#[tokio::test]
async fn a_degraded_probe_logs_the_degraded_state() {
    // ready -> registry_unreachable emits state=registry_unreachable at WARN.
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p memory_mcp health`
Expected: FAIL — no event.

- [ ] **Step 3: Implement**

Compute the status, map it to a discriminant, compare-and-swap against the stored value, and emit only on change (`ready` at `INFO`, any degraded state at `WARN`). Keep the HTTP body and status unchanged.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p memory_mcp health`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/http/health.rs crates/memory-mcp/src/http.rs crates/memory-mcp/src/logging.rs
git commit -m "feat(http): log readiness transitions, not every probe"
```

---

### Task 11: Post-install HTTP binary errors go through the logger

**Files:** modify `crates/memory-mcp/src/bin/memory_mcp_http.rs`; `crates/memory-mcp/src/logging.rs` (add `http.serve_failed` to `HTTP_OPERATIONS`); create `crates/memory-mcp/tests/http_binary_failure_logging.rs`.

**Interfaces:**
- Consumes: `memory_mcp::logging::{emit, LogLevel}`.

- [ ] **Step 1: Write the failing test**

In `crates/memory-mcp/tests/http_binary_failure_logging.rs`, spawn `memory_mcp_http` against an already-bound port (so `server::serve` fails after logging is installed) and assert stderr contains a structured line `op=http.serve_failed` with `level=error`, not a bare `eprintln!` prose line. Reuse the spawn/drain helpers from `crates/memory-mcp/tests/common/http_server.rs` and gate the target with the same attributes (`#![cfg(all(feature = "streamable-http", feature = "test-fixtures"))]`).

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p memory_mcp --test http_binary_failure_logging serve_failed_is_structured`
Expected: FAIL — stderr carries the prose form.

- [ ] **Step 3: Implement**

After `logging::install()`, route the serve/cleanup failures through `emit(..., LogLevel::Error)` with an `op` from `OP_NAMESPACES` (add `http.serve_failed` to `HTTP_OPERATIONS`). Keep `eprintln!` only for the pre-install config-parse and `validate_no_listener_env` errors, which run before any sink exists.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p memory_mcp --test http_binary_failure_logging`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/bin/memory_mcp_http.rs crates/memory-mcp/src/logging.rs crates/memory-mcp/tests/http_binary_failure_logging.rs
git commit -m "fix(http): log post-install binary failures instead of printing them"
```

---

### Task 12: Documentation and the workspace gate

**Files:** create `docs/operations/LOGGING.md`; modify `AGENTS.md` (a "Logging conventions" section), `README.md` (the logging section), `docs/BACKLOG.md`.

**Deliverable content:**
- `LOGGING.md`: the level policy, the exact `OP_NAMESPACES` list, `RUST_LOG`, `MEMORY_LOG_TARGETS`, `MEMORY_LOG_FORMAT`, `MEMORY_LOG_COLOR`, `MEMORY_LOG_FILE`, the correlation id and how to follow one, and the eight renamed keys.
- `AGENTS.md`: the level policy one-liner, `op = <namespace>[.<event>]`, `error` as the error field, never log secrets, and the emission facades (`emit` / `logger.log` / native `tracing`).

- [ ] **Step 1: Write the docs** (as above).

- [ ] **Step 2: Run the full gate**

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
cargo test -p memory_mcp
```
Expected: fmt zero diff, clippy zero warnings, tests green.

- [ ] **Step 3: Live smoke check** (documented in `LOGGING.md`)

Run the HTTP profile with `RUST_LOG=info` and one `ingest`; confirm the tool line and the access line carry the same `req`. Run stdio `serve` with an `ingest`; confirm `req_NNNN`.

- [ ] **Step 4: Commit**

```bash
git add docs/operations/LOGGING.md AGENTS.md README.md docs/BACKLOG.md
git commit -m "docs(logging): conventions, runbook, and the delivered work"
```

---

## Dependency graph and shared write sets

```text
1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 11 → 12
```

Many tasks edit the same file, so the default order is **sequential**; run in
parallel only where the write sets are disjoint. The overlaps:

| File | Tasks |
|---|---|
| `src/logging.rs` | 1, 2, 6, 8, 9, 10, 11 |
| `src/http/logging.rs` | 3, 6, 7, 9 |
| `src/mcp/handlers.rs` | 4, 5 |
| `src/http/principal/auth.rs` | 8, 9 |
| `src/tools/*.rs` | 4, 5 |
| `src/shared/error.rs` | 5 |

Tasks 8, 9, 10 and 11 each add a name to the shared `HTTP_OPERATIONS`
`src/logging.rs`, so they stay sequential.

Task 8 consumes **both** Task 4 (the triple path is reached from a tool) and Task
6 (`OP_NAMESPACES` must exist and carry `triple_extraction`, `fs_watch`, `db`,
`http`, `knowledge`). Task 10 and Task 11 reach the files no other task edits,
but each also adds a name to the shared `HTTP_OPERATIONS`, so they follow Task 6.
Task 12 is last.

## Self-review

- **Spec coverage:** levels → Task 5; correlation → Tasks 1-4; naming → Task 6;
  fields → Task 7; silent failures → Task 8; auth → Task 9; readiness → Task 10;
  binary hygiene → Task 11; docs → Task 12. Every ADR consequence has a task.
- **Type consistency:** `correlation::{current, scope}` is used identically in
  Tasks 1-4; `MemoryError::log_level()` is defined and used in Task 5;
  `OP_NAMESPACES` is defined in Task 6 and referenced by Task 8's new ops;
  `ToolEvent` loses `request_id` in Task 4 and no later task reintroduces it.
- **Review Focus coverage:** items 1-5 map to Tasks 3, 2, 1, 6/2, 5, each with a
  named test.
- **Known limits (called out, not hidden):** four new `http.*` names
  (`http.auth.rejected`, `http.auth.touch_failed`, `http.readiness.changed`,
  `http.serve_failed`) must each be added to `HTTP_OPERATIONS`, and Task 6 extends
  the inventory scanner to read the files they are emitted from (`health.rs`,
  `principal/auth.rs`, `../bin/memory_mcp_http.rs`). The `log_args_with_duration`
  removal touches the eleven call sites listed in Task 7.
  The namespace lint is a text lint over a fixed idiom set; a future producer
  that builds an `op` a new way is not seen by it, which is why the registry and
  the runbook stay the contract.
- **Proportion:** the plan names the decisions an executor cannot make alone
  (mechanism, signatures, level table, registry, sites); it does not transcribe
  bodies.
