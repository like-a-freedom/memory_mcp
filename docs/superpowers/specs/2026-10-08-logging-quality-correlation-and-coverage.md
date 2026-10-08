# Logging quality, correlation and coverage — design

**Status:** design with decisions taken; no code changes at this stage.
**Date:** 2026-10-08.
**Baseline:** the working tree of 2026-10-08, which includes the in-progress
`http-bounded-memory` change. Every reference below is to a file or symbol, not a
line number, so it survives that work landing.
**Decisions:** [ADR-0079](../adr/0079-log-level-policy.md) (levels),
[ADR-0080](../adr/0080-one-request-identity.md) (correlation),
[ADR-0081](../adr/0081-operation-naming-contract.md) (operation names).
**Related plan:** `../plans/2026-10-08-logging-quality-correlation-and-coverage.md`.

## 1. Purpose and scope

Make `memory_mcp` logs operable: one id per request, a predictable level policy,
full coverage of the events that matter (including failures and security
refusals), and a testable operation-name contract — without changing the line
format or the public configuration operators and tests already depend on
(ADR-0078).

The format and infrastructure (**the "format" stage**) are done and stay as they
are: `tracing`/`tracing-subscriber`, a custom `MemoryFormat`, `RUST_LOG` with
`prefix=level` semantics over `op` segments, `MEMORY_LOG_TARGETS`,
`MEMORY_LOG_FORMAT`, `MEMORY_LOG_COLOR`, `MEMORY_LOG_FILE`, and capture of
third-party events. This work builds on that foundation.

Out of scope: changing the line format or fields, adding an OTLP exporter or
distributed tracing, changing environment variables, changing metrics (beyond one
bounded counter, §5.2), and changing product behaviour or protocol contracts.

## 2. What the audit found

Read-only audit of the working tree.

**Format and infrastructure (a strength; unchanged).**
- `logging.rs`: `LogLevel`, `Directives` (selection by `op`), `MemoryFormat`
  (human/JSON), `StdoutLogger` + the `emit` facade, `capture`.
- Line shape and token order: `README.md` "Log line format"; the special tokens
  `op`, `request_id`→`req`, `duration_ms` in `render_human`.
- HTTP operation inventory and its completeness test: `HTTP_OPERATIONS` in
  `logging.rs` and `the_http_operation_inventory_matches_what_the_code_emits` in
  `http/logging.rs`.

**Levels.**
- All six canonical tool error events are `WARN`, none is `ERROR`
  (`tools/{ingest,extract,resolve,explain,invalidate,assemble_context}.rs`); the
  two app-tool error events (`open_app.error`, `app_command.error`) are also
  `WARN`.
- Runtime errors are `ERROR`: `http.job.failed`, `http.scheduler.failed`,
  `http.job.panicked` (`http/leases/scheduler.rs`),
  `http.background_cleanup_secondary_failure` (`http/runtime/bootstrap.rs`),
  `control.internal_error` (`control/error.rs`).
- There is no level policy — neither an ADR nor an `AGENTS.md` convention. The
  README describes levels informally.

**Correlation.**
- The access log carries `req=<uuid>` (`request_log` in `http/logging.rs`), from
  an inbound `x-request-id` or a fresh UUID. The control plane already documents
  that this id, the error envelope's `correlation_id`, and the audit trail name
  one value (`control/error.rs`, `control/local_admin.rs`).
- The tool layer mints a **second, unrelated** id from a process-global counter
  (`tools/request_id.rs` — `req_NNNN`), set on `ToolEvent` (`tools/context.rs`
  and the six `tools/*.rs`); the app tools mint and pass one too
  (`MemoryMcp::next_request_id`, `open_app`/`app_command` in `mcp/handlers.rs`).
- Our own events are not enriched from the active span: in `format_event` the
  `payload` and `op` branches render without `span_fields`; the span is used for
  foreign events only.
- The single production span is `http_request` in `http/logging.rs`; there are no
  `#[instrument]` spans.
- Consequence: in HTTP, `ingest.done req=req_0001` and the access `req=<uuid>`
  do not join; retrieval, embedding and extraction lines join nothing.

**Coverage (silent failures).** `Result`s discarded with `let _ =` in production
with no event: `memory/episode/triples.rs` (`create_triple`,
`resolve_conflicts_for_triple`); `service/fs_watch/processor.rs`
(`mark_failed_cycle`); `service/fs_watch/runtime.rs` (`discover_prepared`,
`requeue_failed_for_startup`, `requeue_expired_leases`); `storage/migrations.rs`
(`mark_migration_failed`); `http/leases/migration.rs`
(`release_provisioning_lease`, `transition_tenant_fenced`);
`bootstrap/integration/registry_operations.rs` (`release_deletion_lease`
swallows the inner `release_provisioning_lease`; the discard at the port call in
`operations/api.rs` is not one, because that adapter returns `Ok` whatever
happens); `http/registry/surreal_store.rs` (the migration-failure marker);
`knowledge/entity_extraction/gliner.rs` (`reject_candidate`);
`http/principal/auth.rs` (`touch_api_key`, with a comment).

**Security.**
- Bearer/API-key refusals are not logged: `authenticate` in
  `http/middleware/auth.rs` returns `unauthorized_response()` with no event, and
  `authenticate_bearer` in `http/principal/auth.rs` returns `Deny` for
  parse/cache-hit/rate-limit/verify failures with no reason given.
- Only OIDC refusals are logged (`oidc.callback_rejected`) and counted
  (`record_auth_refusal` in `observability.rs`).

**Readiness.** `/health/ready` returns 503 (`shutting_down`, `admission_closed`,
`registry_unreachable`) with no event (`http/health.rs`).

**Infrastructure hygiene.**
- Post-install `eprintln!` in the HTTP binary bypasses level, format and sink
  (`bin/memory_mcp_http.rs`).
- `StdoutLogger::from_env()` at an emission site: `emit_preflight_log` in
  `http/middleware/preflight.rs` (the rate-bounded DEBUG refusal log the
  `http-bounded-memory` work added).
- `unwrap_or_default()` when decoding rows hides missing fields
  (`http/app_sessions/store.rs`, `embedding/infra.rs`). Recorded but not part of
  this work: it is a decode-robustness concern, not a log-coverage gap (§7).

**Name consistency.**
- `op` first segments in use: `assemble_context`, `app_command`, `cache`,
  `config` (`service/core/builder.rs`), `control`, `db`, `embedding`, `explain`,
  `extract`, `fs_watch`, `graph`, `http`, `ingest`, `invalidate`, `knowledge`,
  `lifecycle`, `main`, `ner`, `oidc`, `open_app`, `reembed`, `resolve`, `schema`
  (`storage/migrations.rs`), `triple_extraction` — plus the non-conforming
  `startup`, `startup.version_probe_failed` (`bootstrap/stdio.rs`) and
  `startup.versions` (`service/startup.rs`), `model_loader`
  (`embedding/model_loader.rs`), `dimension_override_mismatch` and
  `cosine_similarity.dimension_mismatch` (`embedding/providers.rs`), and
  `extract_from_episode.{start,done}` (`memory/episode/fact_extraction.rs`).
- Some names are built with `format!`: `db.{op}.retry`/`.timeout`
  (`platform/persistence/transactions.rs`).
- Error-field names drift: `error`, `detail` (`http/logging.rs`), `reason`
  (`http/runtime/storage.rs`, `control/oidc/handlers.rs`), `message`
  (`embedding/model_loader.rs`, `embedding/providers.rs`).
- `duration_ms` is top-level in `log_event` but nested under `args` in
  `log_args_with_duration` (`platform/log_event.rs`), and the formatter's special
  token lifts only the top-level one.

## 3. Decisions

Full reasoning is in ADR-0079/0080/0081. In brief:

1. **Levels** (ADR-0079): a five-level policy; `ERROR` is a failed unit of work
   or a risk to integrity; `WARN` is handled/degraded or a client refusal;
   `INFO` is a business event or state change; `DEBUG`/`TRACE` are diagnostics.
   The error class decides the level, via `MemoryError::log_level()`, not a
   hardcoded level at the callsite.

2. **Correlation** (ADR-0080): one id per unit of work in an ambient context
   (`logging::correlation`, `tokio::task_local!`); the formatter injects
   `request_id` into any event that lacks one; the boundary (HTTP, tools) opens
   the scope; `x-request-id` is validated as a UUID; the `http_request` span and
   `span_fields` are removed; correlation is per task, and a spawn that must
   correlate re-enters the scope.

3. **Naming** (ADR-0081): `op` is `<namespace>` or `<namespace>.<event>`;
   `<namespace>` comes from the closed `OP_NAMESPACES` registry (24 names);
   enforcement is a source-scanning lint plus the existing HTTP inventory; eight
   names are renamed (`startup` → `main.config`, `startup.version_probe_failed`
   → `main.version_probe_failed`, `startup.versions` → `main.versions`,
   `model_loader` → `embedding.model_loader`, `dimension_override_mismatch` →
   `embedding.dimension_override_mismatch`, `cosine_similarity.dimension_mismatch`
   → `embedding.cosine_similarity_mismatch`, `extract_from_episode.{start,done}`
   → `extract.from_episode.{start,done}`).

4. **Fields:** `error` is the one name for error text (migrating `detail`,
   `reason`, `message`); `reason` remains only as a machine label for a closed
   branch; `duration_ms` is top-level only.

## 4. Contracts

### 4.1 Level policy

```
ERROR  a failed unit of work / a risk to integrity / a failure after retries / loss of readiness
WARN   handled but unexpected; degraded; a client refusal (4xx class)
INFO   a business event or a state change
DEBUG  diagnostics, off by default
TRACE  fine-grained flow, opted into by name; anything repetitive is a metric
```

`MemoryError` → level:

| Variant | Level |
|---|---|
| `Storage`, `Transient`, `ConfigMissing`, `ConfigInvalid`, `Unavailable` | `ERROR` |
| `NotFound`, `Validation`, `Conflict`, `BudgetExhausted`, `ModelNotReady`, `Auth` | `WARN` |

### 4.2 Correlation

- `logging::correlation::current() -> Option<String>`;
  `logging::correlation::scope(id, future)`.
- The formatter takes `current()` when an event has no `request_id`; an explicit
  field always wins.
- HTTP accepts `x-request-id` only as a valid UUID (otherwise a new one), echoes
  the same header, and runs the request inside the scope.
- Tools take `current()` when present, otherwise `next_request_id()`;
  `ToolEvent` carries no `request_id`.
- Foreign (`tracing`) events take `req` from `current()`, not from `span_fields`.

### 4.3 Names

- `OP_NAMESPACES` = `assemble_context, app_command, cache, config, control, db,
  embedding, explain, extract, fs_watch, graph, http, ingest, invalidate,
  knowledge, lifecycle, main, ner, oidc, open_app, reembed, resolve, schema,
  triple_extraction`.
- Rule: the first `op` segment is in `OP_NAMESPACES`; the event tail is optional
  (`extract` and `extract.done` are one namespace). Dynamic
  `db.{op}.retry`/`db.{op}.timeout` are covered by the static prefix.
- Enforcement is a lint over `crates/memory-mcp/src` covering the emission idioms
  (a literal on an `op` key in any wrapper — `json!`, `Value::String`, `.into()`,
  `event!` — plus `op: "…"`, `op = "…"`, and the first argument of `log_event`,
  `log_claim_event`, `log_op`); the existing HTTP inventory stays the exhaustive
  `http` check.

## 5. Coverage and security

### 5.1 Silent failures
Every `let _ = <Result>` from §2 gets an event: an infrastructure failure at
`WARN` with `op` and `error`; pure telemetry (`touch_api_key`) at `DEBUG`. The
event changes no control flow — best-effort stays best-effort, but stops being
invisible.

### 5.2 Authentication refusals
A new `http.auth.rejected` event with a bounded branch (`missing`, `bad_scheme`,
`parse`, `rate_limited`, `cached_rejection`, `verify_failed`) and a counter. This
extends the existing `record_auth_refusal`, whose `surface` label is currently
hardcoded to `"oidc"` (`observability.rs`); the signature gains `surface`, and
the OIDC caller and two `observability.rs` tests are updated. No secret or key
material is written.

### 5.3 Readiness
A readiness transition is logged once per change, not per probe: `HttpState`
holds the last state and `ready` emits on change.

### 5.4 Post-install HTTP binary errors
Errors after `install()` go through the logger (level, format, sink);
pre-install errors (configuration parsed before logging is up) stay `eprintln!`,
because there is nothing to log through yet.

## 6. Compatibility

- The line format, token order, `MEMORY_LOG_*`, and `RUST_LOG` semantics do not
  change.
- Logs still go to **stderr** (stdout is the MCP stdio channel); the file sink
  stays opt-in. This is a deliberate departure from twelve-factor stdout,
  recorded in ADR-0078.
- The only compatibility break is the eight `op` renames (ADR-0081).
- `req_NNNN` in stdio is preserved byte-for-byte.
- No secrets or PII; new labels stay within a closed vocabulary (ADR-0005).

## 7. Non-goals

- OTLP export, distributed tracing, `span_id` — additive future work.
- Changing the MCP tool set, response schemas, or environment variables.
- Rotating the file sink (append-only, as in ADR-0078).
- Replacing `tracing`/`tracing-subscriber` or adding dependencies.
- Hardening the `unwrap_or_default()` row decoders named in §2 — a decode
  concern tracked separately, not a log gap.

## 8. Acceptance

- `cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings` — zero warnings.
- `cargo fmt --all --check` — zero diff.
- `cargo test -p memory_mcp` (and `-p xtask` if touched) — green.
- Tests: correlation (adopt/mint, task isolation, an explicit `request_id` not
  overwritten); level classification from `MemoryError`; the `op` registry lint;
  `req` injection for our own and foreign events; the `http.auth.rejected` event;
  the readiness transition; the silent-failure coverage.
- A live check: with `RUST_LOG=debug`, an HTTP `ingest` shows the same `req` on
  the tool line and the access line; a stdio `ingest` shows `req_NNNN`.

## 9. Sources

- Google SRE Book, *Monitoring Distributed Systems* / *Practical Alerting* —
  `ERROR` for action, not routine refusals; alert on symptoms.
- OpenTelemetry, *Logs Data Model* — `TraceId`/`SpanId`/`SeverityText`; the
  `TRACE…ERROR` ranges; forwards compatibility.
- W3C Trace Context and correlation practice — mint at the boundary, adopt the
  inbound id, never regenerate mid-request, validate the inbound id (log
  injection), carry it into async work.
- The Twelve-Factor App, *Logs* — a log is an event stream; the app does not
  manage files.
- Rust/`tracing` practice — context via span/task-local, `.instrument()` across
  spawns.
