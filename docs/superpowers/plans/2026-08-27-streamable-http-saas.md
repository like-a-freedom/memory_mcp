# Streamable HTTP SaaS Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a public multi-user Streamable HTTP SaaS deployment profile to `memory_mcp` without changing the existing local stdio profile, per ADR-0052 and the design spec.

**Architecture:** Two composition roots in one workspace crate: the existing `memory_mcp` CLI/stdio binary stays unchanged; a new feature-gated `memory_mcp_http` binary hosts the SaaS HTTP profile (Streamable HTTP transport, Tenant Registry, API key auth, multi-tenant runtime pool, durable App Sessions/Tasks/subscriptions, control-plane API, optional Dioxus SPA). Heavy process-global resources (model weights, HTTP pools, telemetry, schedulers) are shared; tenant-bound storage and capabilities are clone-once/bind-once per Tenant. Authentication uses opaque API keys today (and OAuth in a separate later phase), both converging at one `AuthenticatedPrincipal` seam. Namespace selection is never driven by MCP arguments, URL paths, or arbitrary claims.

**Tech Stack:** Rust 1.97.1, `rmcp` 3.1.2 (Streamable HTTP server), `axum` 0.8 (HTTP router), `tower` 0.5 / `tower-http`, `tokio` 1.53, SurrealDB 3.2.4 (one privileged credential across namespaces, namespace-bound sessions), `oauth2` 5 + `jsonwebtoken` 11 (later phase), Dioxus 0.6 web/WASM (later phase), existing `serde`/`serde_json`/`schemars`/`thiserror`/`lru`/`metrics`.

**Spec:** `docs/superpowers/specs/2026-08-27-streamable-http-saas.md`
**ADR:** `docs/adr/0052-streamable-http-saas-profile.md`

**Invariant (repeated in every phase):** *namespace never enters ordinary MCP arguments or protocol-agnostic capability inputs*. Tenant is always derived from `AuthenticatedPrincipal → Account → ready Tenant → immutable Tenant Runtime`.

## Global Constraints

- Package default features remain `[]`; existing additivity preserved (`fs-watch`, `mcp-apps`, `prometheus`, `metal`, `accelerate`, `eval-support`, `mimalloc`).
- New features are strictly additive: `streamable-http`, `control-plane`, `control-plane-ui`, `test-fixtures` (test-only helpers; gates `#[cfg(any(test, feature = "test-fixtures"))]` code so integration tests can reuse fixtures; declared in Cargo.toml to satisfy the `unexpected_cfgs` lint).
- `memory_mcp_http` has no memory-operation CLI commands; configuration is environment-only (12-factor).
- No nonstandard `Idempotency-Key` header is required; retry safety is provided by domain fingerprints, unique indexes, CAS/versioning, and reconciliation.
- Raw OIDC `subject` values are never persisted or logged; registry identity lookup uses a keyed blind index with a dedicated identity-index key.
- HTTP `429` is reserved for edge/account rate limits or explicit quota exhaustion; temporary runtime/admission capacity overload returns `503`.
- `fs-watch` is a fatal startup error for `memory_mcp_http` (see spec §13).
- Server-local paths, directories, `file:` URLs, and remote URLs are rejected before any I/O in the SaaS profile (spec §13).
- Production code must not use `unwrap()`; errors are `thiserror`-based `MemoryError` variants.
- `main.rs` remains thin — CLI parsing + mode dispatch only. HTTP composition root lives in `crates/memory-mcp/src/http/`.
- MCP layer is a thin adapter; business logic stays in `src/service/`.
- Never physically delete memory facts or audit-bearing domain records; invalidate them instead. Ephemeral App Session and Task rows may be physically removed after their declared TTL/retention window; Account, Tenant, identity, credential, lease, and provisioning history remain durable and transition to terminal states.
- Tenant Registry lives in a separate SurrealDB control namespace/database; ordinary MCP tools cannot query it.
- Every durable worker commit verifies its fencing generation; stale workers cannot commit after takeover (spec §8, ADR-0046).
- The final scheduler hook list explicitly contains provisioning, App Session cleanup, Task retry/retention, and subscription/outbox repair; each job is tracked and joined during shutdown.
- Production snippets may not contain `TODO`, `TBD`, `todo!()`, `unimplemented!()`, or unexplained pseudocode. Test snippets may use the named fixture builders defined by the same task, but each fixture's inputs, observable outputs, and failure assertions must be specified in the task text and expanded before implementation begins.
- **rmcp 3.1.2 protocol-version facts (verified against installed source):** `rmcp::model::ProtocolVersion` is a newtype struct with associated constants `V_2024_11_05`, `V_2025_03_26`, `V_2025_06_18`, `V_2025_11_25`, `V_2026_07_28` — NOT an enum. `ProtocolVersion::LATEST == V_2025_11_25` (not `2026-07-28`!), so any code relying on rmcp defaults negotiates a legacy version. `negotiate_protocol_version` never rejects: on an unsupported requested version it falls back to the `protocol_version` carried by `ServerInfo`/`InitializeResult` returned from `get_info()`. Therefore a modern-only server must (a) override `supported_protocol_versions()` to `[V_2026_07_28]` AND (b) return `get_info()` with `.with_protocol_version(V_2026_07_28)` so the fallback is modern too. The 2026-07-28 transport also requires `MCP-Protocol-Version` to match `_meta.io.modelcontextprotocol/protocolVersion`, requires `Mcp-Method` on every POST, and requires `Mcp-Name` only for `tools/call`, `resources/read`, and `prompts/get`; it removes protocol sessions, `initialize`, `notifications/initialized`, `ping`, standalone GET, and resumable SSE (`Last-Event-ID`).
- **Edition 2024:** `std::env::set_var` / `std::env::remove_var` are `unsafe fn`; every env-mutating test wraps them in `unsafe {}` behind a process-wide test lock.
- Final lint gate (verbatim):

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets \
  --features fs-watch,mcp-apps,streamable-http,control-plane,test-fixtures --locked -- -D warnings
```

(`test-fixtures` is included so `--all-targets` compiles integration tests that consume the fixtures.)

(When `control-plane-ui` is added, it is a separate workspace crate and does not participate in the `memory-mcp` lint command; its own workspace `cargo clippy` is required.)

---

## File Structure

The plan introduces a feature-gated HTTP subtree inside `crates/memory-mcp/` and a separate workspace crate for the Dioxus SPA. The stdio surface, storage layer, migrations, and service core remain largely untouched.

### New files (HTTP subtree)

- `crates/memory-mcp/src/http/mod.rs` — public module surface, gated on `feature = "streamable-http"`.
- `crates/memory-mcp/src/http/config.rs` — typed `HttpConfig` + validation; environment-only.
- `crates/memory-mcp/src/http/error.rs` — `HttpError`, JSON-RPC error mapping, header/body mismatch mapping.
- `crates/memory-mcp/src/http/logging.rs` — structured request log with bounded labels.
- `crates/memory-mcp/src/http/metrics.rs` — extends ADR-0048 three families only.
- `crates/memory-mcp/src/http/health.rs` — `/health/live` + `/health/ready` handlers.
- `crates/memory-mcp/src/http/shutdown.rs` — coordinated shutdown sequence (spec §17).
- `crates/memory-mcp/src/http/transport.rs` — `StreamableHttpService` construction + axum route wiring.
- `crates/memory-mcp/src/http/oauth/` — future MCP OAuth Resource Server metadata and token validation, compiled only with `control-plane`.
- `crates/memory-mcp/src/http/middleware.rs` — Host/Origin/trusted-proxy/CSP/security headers.
- `crates/memory-mcp/src/http/validation.rs` — content-type, Accept, body-limit, deadline.
- `crates/memory-mcp/src/http/test_bootstrap.rs` — test-fixtures-only authenticated tenant bootstrap.
- `crates/memory-mcp/src/http/principal/mod.rs` — `AuthenticatedPrincipal` types.
- `crates/memory-mcp/src/http/principal/auth.rs` — bearer extraction + cache + rate-limit hook.
- `crates/memory-mcp/src/http/principal/api_keys.rs` — strict parser, keyed verifier, lifecycle.
- `crates/memory-mcp/src/http/principal/cache.rs` — positive ≤60s / negative ≪60s LRU.
- `crates/memory-mcp/src/http/registry/mod.rs` — Tenant Registry seam.
- `crates/memory-mcp/src/http/registry/models.rs` — `Account`, `Tenant`, `ApiKey`, `ExternalIdentity`, `ControlPlaneSession`, `Plan`, `Usage`.
- `crates/memory-mcp/src/http/registry/migrations.rs` — separate control-namespace migrations.
- `crates/memory-mcp/migrations/001_registry.surql` — initial control-namespace schema migration.
- `crates/memory-mcp/src/http/registry/storage.rs` — control-storage trait bound to a privileged SurrealDB credential.
- `crates/memory-mcp/src/http/registry/account.rs` — Account ↔ Tenant resolution.
- `crates/memory-mcp/src/http/registry/provisioning.rs` — idempotent enqueue/state transitions.
- `crates/memory-mcp/src/http/registry/plan.rs` — versioned plan + quota policy.
- `crates/memory-mcp/src/http/runtime/mod.rs` — pool surface.
- `crates/memory-mcp/src/http/runtime/lifecycle.rs` — state machine + types.
- `crates/memory-mcp/src/http/runtime/activation.rs` — single-flight + bounded concurrency.
- `crates/memory-mcp/src/http/runtime/guard.rs` — operation pinning RAII.
- `crates/memory-mcp/src/http/runtime/pool.rs` — LRU + idle TTL + capacity.
- `crates/memory-mcp/src/http/runtime/storage.rs` — clone-once/bind-once namespace session binding.
- `crates/memory-mcp/src/http/leases/mod.rs` — datastore-time lease primitives.
- `crates/memory-mcp/src/http/leases/scheduler.rs` — tier-1 process scheduler loop.
- `crates/memory-mcp/src/http/leases/migration.rs` — rolling N/N-1 migration orchestration.
- `crates/memory-mcp/src/http/tasks/mod.rs` — durable Tenant Task records.
- `crates/memory-mcp/src/http/tasks/state.rs` — state machine + version.
- `crates/memory-mcp/src/http/tasks/worker.rs` — fenced durable worker + `DurableTaskStore`; rmcp 3.1.2 `TaskManager` is a concrete struct, so there is no `rmcp_adapter.rs`.
- `crates/memory-mcp/src/http/tasks/scheduler.rs` — scheduler job factory for retry/reconciliation/retention.
- `crates/memory-mcp/src/http/app_sessions/mod.rs` — durable App Session records.
- `crates/memory-mcp/src/http/app_sessions/store.rs` — TTL + 32-cap + optimistic versioning.
- `crates/memory-mcp/src/http/app_sessions/scheduler.rs` — scheduler job factory for expired App Session cleanup.
- `crates/memory-mcp/src/http/subscriptions/mod.rs` — subscriptions/listen + outbox reader.
- `crates/memory-mcp/src/http/subscriptions/outbox.rs` — transactional outbox writer + reader.
- `crates/memory-mcp/src/http/subscriptions/scheduler.rs` — scheduler job factory for outbox polling/reconciliation.
- `crates/memory-mcp/src/http/subscriptions/stream.rs` — bounded queue + slow consumer drop.
- `crates/memory-mcp/src/http/router.rs` — top-level axum `Router` builder.
- `crates/memory-mcp/src/http/server.rs` — bind address + listener + serve loop.
- `crates/memory-mcp/src/bin/memory_mcp_http.rs` — thin composition-root entry.

### New files (control-plane subtree, gated on `control-plane`)

- `crates/memory-mcp/src/control/mod.rs`
- `crates/memory-mcp/src/control/error.rs` — control API error mapping.
- `crates/memory-mcp/src/control/operator.rs` — operator principal seam.
- `crates/memory-mcp/src/control/oidc.rs` — Authorization Code + PKCE, state/nonce.
- `crates/memory-mcp/src/control/session.rs` — secure cookie + verifier + rotation.
- `crates/memory-mcp/src/control/csrf.rs` — token issuance + verification.
- `crates/memory-mcp/src/control/recent_auth.rs` — auth-time threshold gate.
- `crates/memory-mcp/src/control/account_api.rs` — `/api/v1/account/*` handlers.
- `crates/memory-mcp/src/control/operator_api.rs` — `/api/v1/operator/*` handlers.
- `crates/memory-mcp/src/control/deletion.rs` — Account deletion flow.
- `crates/memory-mcp/src/control/static_assets.rs` — embedded Dioxus assets (when `control-plane-ui` enabled).

### New files (Dioxus workspace crate)

- `crates/control-plane-ui/Cargo.toml` — `dioxus = { version = "0.6", features = ["web"] }`.
- `crates/control-plane-ui/src/main.rs` — Dioxus web entry.
- `crates/control-plane-ui/src/api.rs` — typed `/api/v1` DTOs.
- `crates/control-plane-ui/src/router.rs` — client-side routes.
- `crates/control-plane-ui/src/pages/login.rs`, `pages/keys.rs`, `pages/delete.rs`, `pages/status.rs`.
- `crates/control-plane-ui/assets/` — CSP-compliant assets, no external scripts.
- `crates/control-plane-ui/dist/` — built artifacts (binary load via `include_bytes!`).

### Modified files

- `Cargo.toml` — workspace member for `crates/control-plane-ui`.
- `crates/memory-mcp/Cargo.toml` — new features `streamable-http`, `control-plane`, `control-plane-ui`, `test-fixtures`; new optional deps (`axum`, `tower`, `tower-http`, `tower-service`, `http`, `http-body`, `http-body-util`, `bytes`, `uuid`, `rand`, `hmac`, `subtle`, `oauth2`, `jsonwebtoken`, `chacha20poly1305`, `base64`; `async-trait` is already a required dependency; several HTTP dependencies are already transitive via `rmcp` server feature; `tower-service` is NOT re-exported by rmcp and must be declared explicitly).
- `crates/memory-mcp/src/lib.rs` — add `pub mod http;` behind `#[cfg(feature = "streamable-http")]` and `pub mod control;` behind `#[cfg(all(feature = "streamable-http", feature = "control-plane"))]`.
- `crates/memory-mcp/src/mcp/handlers.rs` — HTTP-profile constructors/builders that accept Tenant-bound service backends without changing the stdio constructor.
- `crates/memory-mcp/src/mcp/response.rs` — assert the modern `resultType: "complete"` envelope; reject `input_required`.
- `crates/memory-mcp/src/service/core/builder.rs` — expose `MemoryService::active_namespace()` and `into_arc()` for HTTP reuse.
- `crates/memory-mcp/build.rs` — copy the Dioxus release bundle into `OUT_DIR` when `control-plane-ui` is enabled.

### Tests

- `crates/memory-mcp/tests/http_proto_conformance.rs` — black-box protocol conformance (spec §20.1).
- `crates/memory-mcp/tests/http_isolation.rs` — two Tenants under high concurrency (spec §20.2).
- `crates/memory-mcp/tests/http_crash_recovery.rs` — injection between transitions (spec §20.3).
- `crates/memory-mcp/tests/http_proxy_streaming.rs` — `X-Accel-Buffering: no` etc.
- `crates/memory-mcp/tests/http_revocation.rs` — 60-second external bound.
- `crates/memory-mcp/tests/http_load_concurrency.rs` — pinned runtimes not evicted, single-flight, capacity.
- `crates/memory-mcp/tests/http_subscription_replica.rs` — cross-replica delivery.
- `crates/memory-mcp/tests/http_durable_tasks.rs` — fencing takeover + reconciliation.
- `crates/memory-mcp/tests/http_app_sessions_optimistic.rs` — version conflict.
- `crates/memory-mcp/tests/http_stdio_regression.rs` — frozen stdio behavior (spec §20.4).
- `crates/memory-mcp/tests/http_control_plane.rs` — control-plane API + Dioxus fetch.
- `crates/memory-mcp/tests/http_oauth_phase.rs` — later OAuth phase.

---

## Phase 1: Baseline stdio regression gate

Before any HTTP work, lock down the current stdio behavior so subsequent phases can be evaluated against it.

### Task 1.1: Capture stdio behavior snapshot

**Files:**
- Read-only: `crates/memory-mcp/src/cli/runtime.rs`, `crates/memory-mcp/src/mcp/handlers.rs`

**No code changes in this task.** Produce a written snapshot to be referenced later.

- [ ] **Step 1: Run stdio conformance suite**

Run:

```bash
cargo test -p memory_mcp --test service_acceptance --test tools_e2e --test tools_shared -- --nocapture
```

Expected: all existing tests pass. Capture the test count.

- [ ] **Step 2: Capture tool enumeration snapshot**

Run:

```bash
cargo test -p memory_mcp --test tools_e2e -- --nocapture 2>&1 | tee /tmp/memory-mcp-stdio-baseline.log
```

Expected: log includes the eight tool names: `ingest`, `extract`, `resolve`, `invalidate`, `assemble_context`, `explain`, `open_app`, `app_command`. Save this list — it is the frozen tool surface (spec §4.1, ADR-0038).

- [ ] **Step 3: Capture default-features compile**

Run:

```bash
cargo build -p memory_mcp
```

Expected: success with no warnings. This proves the default build remains local-only (no HTTP).

- [ ] **Step 4: Commit nothing (snapshot only)**

This task produces no commits — it documents the baseline only. Continue to Task 1.2.

### Task 1.2: Verify unrelated `Cargo.lock` modification

The handoff context notes `Cargo.lock` was modified before this work and may be unrelated.

**Files:** none modified.

- [ ] **Step 1: Diff Cargo.lock against HEAD~1**

Run:

```bash
git --no-pager diff HEAD~1 -- Cargo.lock | head -200
```

Expected: if diff is non-empty, classify each changed crate. If any change touches `rmcp`, `surrealdb`, or any HTTP-related crate, escalate before proceeding.

- [ ] **Step 2: Decide lock disposition**

If the diff is unrelated (e.g., dev-only test crates), document in the phase 2 commit message: `cargo.lock: unrelated change before HTTP work; see Task 1.2`. Do **not** revert unless asked.

If the diff is related (touches `rmcp` or `surrealdb` versions), STOP and ask the user. Continuing without confirmation may break the existing stdio build.

### Task 1.3: Document the invariant

- [ ] **Step 1: Add invariant reminder comment**

Add to `crates/memory-mcp/src/lib.rs` immediately above `pub mod service;`:

```rust
//! # SaaS tenant invariant
//!
//! Memory MCP has one namespace per process in the stdio profile (ADR-0038)
//! and a bounded pool of namespaces in the HTTP SaaS profile (ADR-0052).
//! Namespace MUST never be selected through MCP arguments, URL paths, OAuth
//! claims, or API-key contents. In every profile the Tenant is derived from
//! an `AuthenticatedPrincipal` resolved by authentication, never by request.
```

- [ ] **Step 2: Verify build still passes**

Run:

```bash
cargo build -p memory_mcp
```

Expected: success.

- [ ] **Step 3: Commit**

```bash
git add crates/memory-mcp/src/lib.rs
git commit -m "docs: invariant reminder for SaaS tenant isolation (ADR-0052)"
```

---

## Phase 2: Dependency and workspace approval gate

This phase **contains no code changes**. It exists to obtain explicit user approval for `Cargo.toml` modifications before any dependency is added, per `AGENTS.md` Boundaries ("Ask before: Changing dependencies in `Cargo.toml`").

### Task 2.1: Compile exact dependency proposal

- [ ] **Step 1: Produce the proposal document**

Create `docs/superpowers/plans/2026-08-27-streamable-http-saas.deps.md` with the following exact content:

````markdown
# Cargo.toml change proposal — Streamable HTTP SaaS

This proposal requires explicit user approval before any change is applied.

## Workspace `Cargo.toml`

Add (or update) these workspace dependencies:

| Crate | Version | Default features | New features | Purpose |
|---|---|---|---|---|
| `axum` | `0.8` | none | `["http1", "tokio"]` | HTTP router |
| `tower` | `0.5` | none | `["util"]` | Service combinators |
| `tower-http` | `0.6` | none | `["trace", "request-id", "set-header", "limit"]` | HTTP middleware |
| `http` | `1` | none | `[]` | already transitive via rmcp; pin for explicit use |
| `http-body` | `1` | none | `[]` | already transitive via rmcp |
| `http-body-util` | `0.1` | none | `[]` | already transitive via rmcp |
| `bytes` | `1` | none | `[]` | already transitive via rmcp |
| `uuid` | `1` | none | `["v4"]` | session/request IDs (already transitive via rmcp) |
| `rand` | `0.10` | none | `[]` | already transitive via rmcp |
| `tower-service` | `0.3` | none | `[]` | `Service` trait for calling `StreamableHttpService` from axum; NOT re-exported by rmcp |
| `hmac` | `0.12` | none | `[]` | keyed verifiers for API keys / session cookies / CSRF |
| `subtle` | `2` | none | `[]` | constant-time comparison for secrets |
| `oauth2` | `5` | none | `[]` | optional control-plane Authorization Code + PKCE client |
| `jsonwebtoken` | `11` | none | `[]` | optional OIDC ID/access-token signature and claim validation |
| `chacha20poly1305` | `0.10` | none | `[]` | authenticated encryption for short-lived OIDC flow material |
| `base64` | `0.22` | none | `[]` | URL-safe PKCE verifier/challenge encoding |

## `crates/memory-mcp/Cargo.toml`

Add (or update) these features and optional deps:

```toml
[features]
streamable-http = [
    "rmcp/transport-streamable-http-server",
    "dep:axum",
    "dep:tower",
    "dep:tower-http",
    "dep:tower-service",
    "dep:http",
    "dep:http-body",
    "dep:http-body-util",
    "dep:bytes",

    "dep:uuid",
    "dep:rand",
    "dep:hmac",
    "dep:subtle",
]
control-plane = ["streamable-http", "dep:oauth2", "dep:jsonwebtoken", "dep:chacha20poly1305", "dep:base64"]
control-plane-ui = ["control-plane"]
# Test-only: exposes #[cfg(any(test, feature = "test-fixtures"))] fixtures to
# integration tests in tests/. Declared so cfg references pass unexpected_cfgs.
test-fixtures = []
```

The OAuth deps are intentionally added only when the control-plane feature is
enabled. They remain optional and never appear in the default build. Add
`chacha20poly1305 = "0.10"` to the optional control-plane dependencies: OIDC
state/nonce/PKCE verifier material is authenticated-encrypted at rest, while
API-key and cookie values remain keyed-hash-only.

## Workspace member

Add `crates/control-plane-ui` (gated on `control-plane-ui`) — added in Phase 10,
not now.

In `crates/memory-mcp/Cargo.toml`, declare the HTTP binary explicitly so the
normal default build never tries to compile it:

```toml
[[bin]]
name = "memory_mcp_http"
path = "src/bin/memory_mcp_http.rs"
required-features = ["streamable-http"]
```

The existing stdio binary configuration is left unchanged.

## Rollback

If the proposal is rejected, no files outside this proposal document are
 touched and Phase 2 is marked complete with the proposal retained for audit.
````

- [ ] **Step 2: Present proposal to user**

Read the proposal aloud in chat and ASK: "Do you approve these exact `Cargo.toml` changes? Reply with explicit yes/no before any modification. Per `AGENTS.md`, no dependency change is authorized by this plan."

**STOP HERE** until the user replies. Do not proceed to Task 2.2 without approval.

### Task 2.2: Apply approved dependency changes

**Only execute this task if Task 2.1 was approved.**

- [ ] **Step 1: Update workspace `Cargo.toml`**

Apply the table from Task 2.1 exactly. Use `edit_file` with the existing `[workspace.dependencies]` block; do not reorder existing entries. Verify post-edit:

```bash
cargo metadata --format-version 1 --no-deps > /dev/null
```

Expected: exit code 0.

- [ ] **Step 2: Update `crates/memory-mcp/Cargo.toml`**

Add the three new features and the optional deps exactly as proposed. Keep `default = []`.

- [ ] **Step 3: Compile empty feature set**

Run:

```bash
cargo build -p memory_mcp
cargo build -p memory_mcp --no-default-features
cargo build -p memory_mcp --features streamable-http
cargo build -p memory_mcp --features streamable-http,control-plane
```

Expected: all four succeed. The fifth combination (`control-plane-ui`) is impossible until Task 10.9 and is intentionally skipped.

- [ ] **Step 4: Update `Cargo.lock` and verify it's a forward-only diff**

Run:

```bash
cargo update --workspace
git --no-pager diff -- Cargo.lock | wc -l
```

Expected: non-zero diff. Inspect: only approved crates should appear. If anything else changed, STOP.

- [ ] **Step 5: Run stdio regression to confirm no breakage**

Run:

```bash
cargo test -p memory_mcp --test service_acceptance --test tools_e2e
```

Expected: all pass.

- [ ] **Step 6: Run final lint**

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets \
  --features fs-watch,mcp-apps,streamable-http,control-plane --locked -- -D warnings
```

Expected: zero warnings, exit 0.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock crates/memory-mcp/Cargo.toml
git commit -m "build: add streamable-http + control-plane Cargo features (ADR-0052)"
```

### Task 2.3: Verify `rmcp` Streamable HTTP API surface against installed version

The plan assumes specific `rmcp 3.1.2` API. Verify the installed version matches.

- [ ] **Step 1: Confirm types exist**

Run:

```bash
grep -rn "pub struct StreamableHttpServerConfig\b\|pub struct StreamableHttpService\b\|pub struct NeverSessionManager\b" \
  ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rmcp-3.1.2/src/
```

Expected: three matches (one per struct). If `StreamableHttpServerConfig` or `StreamableHttpService` is missing, the installed version differs — STOP and re-derive this plan against that version.

- [ ] **Step 2: Confirm builder methods**

Run:

```bash
grep -n "pub fn with_\|pub fn new" \
  ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rmcp-3.1.2/src/transport/streamable_http_server/tower.rs | head -40
```

Expected output must include:

```text
pub fn new(...)
pub fn with_allowed_hosts(...)
pub fn with_allowed_origins(...)
pub fn with_legacy_session_mode(...)
pub fn with_stateless_protocol_metadata_required(...)
pub fn with_max_request_body_bytes(...)
pub fn with_cancellation_token(...)
pub fn with_sse_keep_alive(...)
pub fn with_sse_retry(...)
pub fn with_json_response(...)
```

If any are missing, re-derive. Document the actual method names in `crates/memory-mcp/src/http/transport.rs` doc comments.

- [ ] **Step 3: Confirm `supported_protocol_versions` is the override hook**

Run:

```bash
grep -n "fn supported_protocol_versions\b" \
  ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rmcp-3.1.2/src/handler/server.rs
```

Expected: at least one `pub fn` (default implementation) in the `ServerHandler` trait. The plan uses an override on `MemoryMcp` to advertise only `2026-07-28`.

- [ ] **Step 4: Record findings**

Append a short note to `crates/memory-mcp/src/http/transport.rs` (creating the file is fine even if empty) summarizing the verified method signatures. This becomes the anchor for all later tasks.

---

## Phase 3: HTTP composition root and modern transport (no tenancy)

This phase builds the HTTP binary skeleton and wires the modern-only `rmcp` Streamable HTTP transport. **No authentication, no tenancy, no provisioning yet.** Phase 3 runs a real tenantless handler over an in-memory database (Task 3.3) so protocol conformance is testable immediately; tenant selection arrives in Phase 4.

### Task 3.1: HTTP config skeleton

**Files:**
- Create: `crates/memory-mcp/src/http/mod.rs`
- Create: `crates/memory-mcp/src/http/config.rs`

- [ ] **Step 1: Create `mod.rs`**

Declare only what exists after this task. Every later task adds its own
`pub mod` line in the same commit that creates the module's file — this keeps
each task compiling. Do NOT pre-declare modules whose files do not exist yet.

```rust
//! HTTP SaaS profile (ADR-0052). Gated on `streamable-http` in lib.rs:
//! `#[cfg(feature = "streamable-http")] pub mod http;`

pub mod config;
// Later tasks append, each together with the file it creates:
//   pub mod shutdown; pub mod health; pub mod transport; pub mod router;
//   pub mod server; pub mod middleware; pub mod metrics; pub mod validation;
//   pub mod principal; pub mod registry; pub mod runtime;
//   #[cfg(feature = "control-plane")] pub mod control;
```

- [ ] **Step 2: Create typed `HttpConfig`**

```rust
//! HTTP configuration loaded exclusively from environment (12-factor).

use std::net::SocketAddr;
use std::time::Duration;

use serde::Deserialize;

use crate::error::MemoryError;

pub const DEFAULT_BIND: &str = "0.0.0.0:8080";
pub const DEFAULT_BODY_LIMIT_BYTES: usize = 8 * 1024 * 1024; // 8 MiB
pub const DEFAULT_REQUEST_DEADLINE: Duration = Duration::from_secs(120);
pub const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(30);
pub const DEFAULT_ALLOWED_HOSTS: &[&str] = &[]; // must be set explicitly in production
pub const DEFAULT_ALLOWED_ORIGINS: &[&str] = &[];

#[derive(Debug, Clone, Deserialize)]
pub struct HttpConfig {
    pub bind: SocketAddr,
    pub public_base_url: String,
    pub trusted_proxy_cidrs: Vec<cidr::Cidr>, // re-export from a tiny dep-free parser below
    pub allowed_hosts: Vec<String>,
    pub allowed_origins: Vec<String>,
    pub body_limit_bytes: usize,
    pub request_deadline: Duration,
    pub shutdown_grace: Duration,
    pub control_db: SurrealTargetConfig,
    pub tenant_db: SurrealTargetConfig,
    pub api_key_pepper: String,
    pub identity_index_key: [u8; 32],
    pub control_plane_session_key: [u8; 32],
    pub oidc_state_key: [u8; 32],
    pub oidc_nonce_key: [u8; 32],
    pub csrf_key: [u8; 32],
    pub oidc_issuer: String,
    pub oidc_client_id: String,
    pub oidc_audience: String,
    pub oidc_redirect_uri: String,
    pub oidc_allowed_alg: String,
    pub signup_mode: SignupMode,
    pub enable_control_plane: bool,
    pub enable_control_plane_ui: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SurrealTargetConfig {
    pub url: String,
    pub username: String,
    pub password: String,
    pub database: String,
    pub namespace: String, // separate for control vs. tenant
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub enum SignupMode { InviteOnly, Open }

impl HttpConfig {
    pub fn validate(&self) -> Result<(), MemoryError> {
        if self.bind.ip().is_unspecified() && !self.public_base_url.contains("localhost") {
            tracing::warn!(
                target: "memory_mcp::http::config",
                "binding to unspecified address; production must run behind a reverse proxy"
            );
        }
        if self.allowed_hosts.is_empty() {
            return Err(MemoryError::ConfigInvalid(
                "ALLOWED_HOSTS must be explicit in HTTP SaaS profile".into(),
            ));
        }
        if self.allowed_origins.iter().any(|o| o == "*") {
            return Err(MemoryError::ConfigInvalid(
                "wildcard ALLOWED_ORIGINS is rejected (spec §3.3)".into(),
            ));
        }
        if self.api_key_pepper.len() < 32 {
            return Err(MemoryError::ConfigInvalid(
                "MEMORY_MCP_API_KEY_PEPPER must be ≥32 bytes".into(),
            ));
        }
        if self.signup_mode == SignupMode::Open && !self.open_signup_quotas_set() {
            return Err(MemoryError::ConfigInvalid(
                "open signup requires explicit quota values (spec §12)".into(),
            ));
        }
        #[cfg(not(feature = "control-plane"))]
        if self.enable_control_plane || self.enable_control_plane_ui {
            return Err(MemoryError::ConfigInvalid(
                "control-plane settings require the control-plane feature".into(),
            ));
        }
        #[cfg(not(feature = "control-plane-ui"))]
        if self.enable_control_plane_ui {
            return Err(MemoryError::ConfigInvalid(
                "control-plane UI requires the control-plane-ui feature".into(),
            ));
        }
        Ok(())
    }

    fn open_signup_quotas_set(&self) -> bool { false } // Phase 6 will replace
}
```

- [ ] **Step 3: Add `cidr` tiny parser (no extra dep)**

Replace the `cidr::Cidr` import in the snippet above with a project-private enum and parser, because the plan does not add `cidr` as a dependency:

```rust
// In crates/memory-mcp/src/http/config.rs:
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustedCidr {
    V4(std::net::Ipv4Addr, u8),
    V6(std::net::Ipv6Addr, u8),
}

impl TrustedCidr {
    pub fn parse(s: &str) -> Result<Self, MemoryError> {
        // simple parser; reject malformed strings and out-of-range prefixes
        if let Some((addr, prefix)) = s.split_once('/') {
            let prefix: u8 = prefix.parse().map_err(|_| MemoryError::ConfigInvalid("trusted proxy CIDR".into()))?;
            if let Ok(v4) = addr.parse::<std::net::Ipv4Addr>() {
                if prefix > 32 {
                    return Err(MemoryError::ConfigInvalid(format!("invalid IPv4 prefix length: {prefix}")));
                }
                return Ok(Self::V4(v4, prefix));
            }
            if let Ok(v6) = addr.parse::<std::net::Ipv6Addr>() {
                if prefix > 128 {
                    return Err(MemoryError::ConfigInvalid(format!("invalid IPv6 prefix length: {prefix}")));
                }
                return Ok(Self::V6(v6, prefix));
            }
        } else if let Ok(v4) = s.parse::<std::net::Ipv4Addr>() {
            return Ok(Self::V4(v4, 32));
        } else if let Ok(v6) = s.parse::<std::net::Ipv6Addr>() {
            return Ok(Self::V6(v6, 128));
        }
        Err(MemoryError::ConfigInvalid(format!("invalid trusted proxy CIDR: {s}")))
    }

    pub fn contains(&self, addr: std::net::IpAddr) -> bool {
        match (self, addr) {
            (Self::V4(net, prefix), std::net::IpAddr::V4(v4)) => {
                let mask = u32::MAX.checked_shl((32 - *prefix) as u32).unwrap_or(0);
                u32::from(*net) & mask == u32::from(v4) & mask
            }
            (Self::V6(net, prefix), std::net::IpAddr::V6(v6)) => {
                let mask = u128::MAX.checked_shl((128 - *prefix) as u32).unwrap_or(0);
                u128::from(*net) & mask == u128::from(v6) & mask
            }
            _ => false,
        }
    }
}
```

Update the field type: `pub trusted_proxy_cidrs: Vec<TrustedCidr>`.

- [ ] **Step 4: Add unit tests for `TrustedCidr`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn parses_ipv4_cidr() {
        let c = TrustedCidr::parse("10.0.0.0/8").unwrap();
        assert!(c.contains(IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3))));
        assert!(!c.contains(IpAddr::V4(Ipv4Addr::new(11, 1, 2, 3))));
    }

    #[test]
    fn parses_ipv6_cidr() {
        let c = TrustedCidr::parse("2001:db8::/32").unwrap();
        assert!(c.contains(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(!c.contains(IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1))));
    }

    #[test]
    fn rejects_malformed() {
        assert!(TrustedCidr::parse("not-an-ip").is_err());
        assert!(TrustedCidr::parse("10.0.0.0/64").is_err());
    }
}
```

- [ ] **Step 5: Run tests**

Run:

```bash
cargo test -p memory_mcp --features streamable-http --lib http::config
```

Expected: all `TrustedCidr` tests above pass. `from_env` is intentionally not
part of this skeleton; Task 3.2 adds the production loader together with its
failing tests.

- [ ] **Step 6: Commit**

```bash
git add crates/memory-mcp/src/http/
git commit -m "feat(http): typed HttpConfig + CIDR parser skeleton"
```

### Task 3.2: HTTP config environment loader

**Files:**
- Modify: `crates/memory-mcp/src/http/config.rs`

- [ ] **Step 1: Write failing test for env loader**

Append to `mod tests` in `config.rs`:

```rust
use std::env;
use std::sync::Mutex;
// Edition 2024: set_var/remove_var are unsafe. ENV_LOCK serializes all
// env-mutating tests in this module, which is the safety condition.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn with_env<F: FnOnce()>(vars: &[(&str, &str)], f: F) {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for k in ["MEMORY_MCP_HTTP_BIND", "ALLOWED_HOSTS", "ALLOWED_ORIGINS",
              "MEMORY_MCP_API_KEY_PEPPER", "MEMORY_MCP_HTTP_SIGNUP_MODE",
              "MEMORY_MCP_HTTP_PUBLIC_BASE_URL", "MEMORY_MCP_HTTP_BODY_LIMIT",
              "MEMORY_MCP_HTTP_REQUEST_DEADLINE_SECS", "MEMORY_MCP_HTTP_SHUTDOWN_GRACE_SECS",
              "MEMORY_MCP_HTTP_TRUSTED_PROXY_CIDRS", "SURREALDB_CONTROL_URL",
              "SURREALDB_CONTROL_USERNAME", "SURREALDB_CONTROL_PASSWORD",
              "SURREALDB_CONTROL_DB", "SURREALDB_CONTROL_NAMESPACE",
              "SURREALDB_TENANT_URL", "SURREALDB_TENANT_USERNAME",
              "SURREALDB_TENANT_PASSWORD", "SURREALDB_TENANT_DB",
              "SURREALDB_TENANT_NAMESPACE", "MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE",
              "MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI",
              "MEMORY_MCP_HTTP_TEST_BOOTSTRAP",
              "MEMORY_MCP_HTTP_CSRF_KEY", "MEMORY_MCP_HTTP_OIDC_STATE_KEY",
              "MEMORY_MCP_HTTP_OIDC_NONCE_KEY", "MEMORY_MCP_HTTP_SESSION_KEY",
              "MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY"] {
        // SAFETY: serialized by ENV_LOCK; no other thread reads these vars in tests.
        unsafe { env::remove_var(k); }
    }
    for (k, v) in vars {
        // SAFETY: same as above.
        unsafe { env::set_var(k, v); }
    }
    f();
    for (k, _) in vars {
        // SAFETY: same as above.
        unsafe { env::remove_var(k); }
    }
}
```

```rust
#[test]
fn http_config_loads_from_env_with_minimum_required() {
    let pepper = "x".repeat(40);
    let key = "0".repeat(64);
    with_env(&[
        ("MEMORY_MCP_HTTP_BIND", "127.0.0.1:8080"),
        ("MEMORY_MCP_HTTP_PUBLIC_BASE_URL", "http://localhost"),
        ("ALLOWED_HOSTS", "localhost"),
        ("ALLOWED_ORIGINS", "http://localhost"),
        ("MEMORY_MCP_API_KEY_PEPPER", &pepper),
        ("MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY", &key),
        ("MEMORY_MCP_HTTP_SIGNUP_MODE", "invite_only"),
        ("MEMORY_MCP_HTTP_CSRF_KEY", &key),
        ("MEMORY_MCP_HTTP_OIDC_STATE_KEY", &key),
        ("MEMORY_MCP_HTTP_OIDC_NONCE_KEY", &key),
        ("MEMORY_MCP_HTTP_SESSION_KEY", &key),
        ("SURREALDB_CONTROL_URL", "ws://localhost:8000"),
        ("SURREALDB_CONTROL_USERNAME", "root"), ("SURREALDB_CONTROL_PASSWORD", "root"),
        ("SURREALDB_CONTROL_DB", "control"), ("SURREALDB_CONTROL_NAMESPACE", "control"),
        ("SURREALDB_TENANT_URL", "ws://localhost:8000"),
        ("SURREALDB_TENANT_USERNAME", "root"), ("SURREALDB_TENANT_PASSWORD", "root"),
        ("SURREALDB_TENANT_DB", "tenant"), ("SURREALDB_TENANT_NAMESPACE", "tenant"),
        ("MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE", "false"),
        ("MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI", "false"),
    ], || {
        let cfg = HttpConfig::from_env().expect("config loads");
        cfg.validate().expect("valid");
        assert_eq!(cfg.bind.port(), 8080);
        assert_eq!(cfg.allowed_hosts, vec!["localhost".to_string()]);
        assert_eq!(cfg.signup_mode, SignupMode::InviteOnly);
    });
}

#[test]
fn http_config_rejects_wildcard_origin() {
    let pepper = "x".repeat(40);
    let key = "0".repeat(64);
    with_env(&[
        ("MEMORY_MCP_HTTP_BIND", "127.0.0.1:8080"),
        ("MEMORY_MCP_HTTP_PUBLIC_BASE_URL", "http://localhost"),
        ("ALLOWED_HOSTS", "localhost"),
        ("ALLOWED_ORIGINS", "*"),
        ("MEMORY_MCP_API_KEY_PEPPER", &pepper),
        ("MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY", &key),
        ("MEMORY_MCP_HTTP_SIGNUP_MODE", "invite_only"),
        ("MEMORY_MCP_HTTP_CSRF_KEY", &key),
        ("MEMORY_MCP_HTTP_OIDC_STATE_KEY", &key),
        ("MEMORY_MCP_HTTP_OIDC_NONCE_KEY", &key),
        ("MEMORY_MCP_HTTP_SESSION_KEY", &key),
        ("SURREALDB_CONTROL_URL", "ws://localhost:8000"),
        ("SURREALDB_CONTROL_USERNAME", "root"), ("SURREALDB_CONTROL_PASSWORD", "root"),
        ("SURREALDB_CONTROL_DB", "control"), ("SURREALDB_CONTROL_NAMESPACE", "control"),
        ("SURREALDB_TENANT_URL", "ws://localhost:8000"),
        ("SURREALDB_TENANT_USERNAME", "root"), ("SURREALDB_TENANT_PASSWORD", "root"),
        ("SURREALDB_TENANT_DB", "tenant"), ("SURREALDB_TENANT_NAMESPACE", "tenant"),
        ("MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE", "false"),
        ("MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI", "false"),
    ], || {
        let cfg = HttpConfig::from_env().expect("parses");
        assert!(matches!(cfg.validate(), Err(MemoryError::ConfigInvalid(_))));
    });
}
```

- [ ] **Step 2: Run tests — expected FAIL**

Run:

```bash
cargo test -p memory_mcp --features streamable-http --lib http::config::tests
```

Expected: FAIL to compile because `HttpConfig::from_env` has not been added yet.

- [ ] **Step 3: Implement `HttpConfig::from_env`**

Add the typed `from_env` loader to `HttpConfig`:

```rust
pub fn from_env() -> Result<Self, MemoryError> {
    let bind: SocketAddr = parse_env_or("MEMORY_MCP_HTTP_BIND", DEFAULT_BIND)?;
    let public_base_url = require_env("MEMORY_MCP_HTTP_PUBLIC_BASE_URL")?;
    let allowed_hosts = parse_csv("ALLOWED_HOSTS")?;
    let allowed_origins = parse_csv("ALLOWED_ORIGINS")?;
    let body_limit_bytes = parse_env_or("MEMORY_MCP_HTTP_BODY_LIMIT", DEFAULT_BODY_LIMIT_BYTES)?;
    let request_deadline = Duration::from_secs(parse_env_or(
        "MEMORY_MCP_HTTP_REQUEST_DEADLINE_SECS", DEFAULT_REQUEST_DEADLINE.as_secs())?);
    let shutdown_grace = Duration::from_secs(parse_env_or(
        "MEMORY_MCP_HTTP_SHUTDOWN_GRACE_SECS", DEFAULT_SHUTDOWN_GRACE.as_secs())?);
    let trusted_proxy_cidrs = parse_csv("MEMORY_MCP_HTTP_TRUSTED_PROXY_CIDRS")?
        .into_iter()
        .map(|s| TrustedCidr::parse(&s))
        .collect::<Result<Vec<_>, _>>()?;
    let api_key_pepper = require_env("MEMORY_MCP_API_KEY_PEPPER")?;
    let identity_index_key = parse_hex32("MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY")?;
    let control_plane_session_key = parse_hex32("MEMORY_MCP_HTTP_SESSION_KEY")?;
    let oidc_state_key = parse_hex32("MEMORY_MCP_HTTP_OIDC_STATE_KEY")?;
    let oidc_nonce_key = parse_hex32("MEMORY_MCP_HTTP_OIDC_NONCE_KEY")?;
    let csrf_key = parse_hex32("MEMORY_MCP_HTTP_CSRF_KEY")?;
    let signup_mode = match require_env("MEMORY_MCP_HTTP_SIGNUP_MODE")?.as_str() {
        "invite_only" => SignupMode::InviteOnly,
        "open" => SignupMode::Open,
        other => return Err(MemoryError::ConfigInvalid(format!("signup mode: {other}"))),
    };
    let enable_control_plane = parse_bool("MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE", false)?;
    let enable_control_plane_ui = parse_bool("MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI", false)?;
    let oidc_issuer = optional_env("MEMORY_MCP_HTTP_OIDC_ISSUER").unwrap_or_default();
    let oidc_client_id = optional_env("MEMORY_MCP_HTTP_OIDC_CLIENT_ID").unwrap_or_default();
    let oidc_audience = optional_env("MEMORY_MCP_HTTP_OIDC_AUDIENCE").unwrap_or_default();
    let oidc_redirect_uri = optional_env("MEMORY_MCP_HTTP_OIDC_REDIRECT_URI").unwrap_or_default();
    let oidc_allowed_alg = optional_env("MEMORY_MCP_HTTP_OIDC_ALLOWED_ALG")
        .unwrap_or_else(|| "RS256".into());

    let control_db = SurrealTargetConfig {
        url: require_env("SURREALDB_CONTROL_URL")?,
        username: require_env("SURREALDB_CONTROL_USERNAME")?,
        password: require_env("SURREALDB_CONTROL_PASSWORD")?,
        database: require_env("SURREALDB_CONTROL_DB")?,
        namespace: require_env("SURREALDB_CONTROL_NAMESPACE")?,
    };
    let tenant_db = SurrealTargetConfig {
        url: require_env("SURREALDB_TENANT_URL")?,
        username: require_env("SURREALDB_TENANT_USERNAME")?,
        password: require_env("SURREALDB_TENANT_PASSWORD")?,
        database: require_env("SURREALDB_TENANT_DB")?,
        namespace: require_env("SURREALDB_TENANT_NAMESPACE")?,
    };

    Ok(Self {
        bind, public_base_url, trusted_proxy_cidrs,
        allowed_hosts, allowed_origins,
        body_limit_bytes, request_deadline, shutdown_grace,
        control_db, tenant_db, api_key_pepper, identity_index_key,
        control_plane_session_key, oidc_state_key, oidc_nonce_key, csrf_key,
        oidc_issuer, oidc_client_id, oidc_audience, oidc_redirect_uri,
        oidc_allowed_alg, signup_mode, enable_control_plane, enable_control_plane_ui,
    })
}

// helpers (private):
fn require_env(k: &str) -> Result<String, MemoryError> {
    std::env::var(k).map_err(|_| MemoryError::ConfigMissing(k.into()))
}
fn optional_env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|value| !value.trim().is_empty())
}
fn parse_env_or<T: std::str::FromStr>(k: &str, default: T) -> Result<T, MemoryError> {
    match std::env::var(k) {
        Ok(v) => v.parse::<T>().map_err(|_| MemoryError::ConfigInvalid(k.into())),
        Err(_) => Ok(default),
    }
}
fn parse_csv(k: &str) -> Result<Vec<String>, MemoryError> {
    match std::env::var(k) {
        Ok(v) => Ok(v.split(',').filter(|s| !s.is_empty()).map(str::to_string).collect()),
        Err(_) => Ok(Vec::new()),
    }
}
fn parse_bool(k: &str, default: bool) -> Result<bool, MemoryError> {
    match std::env::var(k) {
        Ok(v) => v.parse::<bool>().map_err(|_| MemoryError::ConfigInvalid(k.into())),
        Err(_) => Ok(default),
    }
}
fn parse_hex32(k: &str) -> Result<[u8; 32], MemoryError> {
    let raw = require_env(k)?;
    let bytes = hex::decode(&raw).map_err(|_| MemoryError::ConfigInvalid(k.into()))?;
    bytes.try_into().map_err(|_| MemoryError::ConfigInvalid(k.into()))
}
```

- [ ] **Step 4: Run tests — expected PASS**

Run:

```bash
cargo test -p memory_mcp --features streamable-http --lib http::config
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/http/config.rs
git commit -m "feat(http): environment loader for HttpConfig with validation"
```

### Task 3.3: Modern-only `StreamableHttpService` wiring (no tenancy)

**Files:**
- Create: `crates/memory-mcp/src/http/transport.rs`
- Create: `crates/memory-mcp/src/http/router.rs`
- Create: `crates/memory-mcp/src/http/server.rs`
- Modify: `crates/memory-mcp/src/mcp/handlers.rs`
- Modify: `crates/memory-mcp/src/storage/client.rs` (`connect_bound` constructor)

- [ ] **Step 1: Write failing test for unsupported-version response**

Create `crates/memory-mcp/src/http/transport.rs` with the skeleton below and the failing test:

```rust
use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode};
use axum::routing::post;
use axum::Router;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
};

pub const PROTOCOL_VERSION: &str = "2026-07-28";

/// Single production config builder. Every construction of the rmcp service
/// goes through this function — no second "default" builder exists (a second
/// builder would be dead code under clippy -D warnings).
pub fn build_server_config(
    http: &super::config::HttpConfig,
    cancellation_token: tokio_util::sync::CancellationToken,
) -> StreamableHttpServerConfig {
    StreamableHttpServerConfig::default()
        .with_allowed_hosts(http.allowed_hosts.iter().cloned())
        .with_allowed_origins(http.allowed_origins.iter().cloned())
        .with_legacy_session_mode(false)
        .with_stateless_protocol_metadata_required(true)
        .with_max_request_body_bytes(http.body_limit_bytes)
        .with_cancellation_token(cancellation_token)
        .with_sse_keep_alive(Some(std::time::Duration::from_secs(15)))
        .with_sse_retry(Some(std::time::Duration::from_secs(3)))
        .with_json_response(false) // SSE for everything; spec §4.1 requires request-scoped SSE
}

pub fn build_mcp_service<H>(
    factory: Arc<dyn Fn() -> Result<H, std::io::Error> + Send + Sync>,
    config: StreamableHttpServerConfig,
) -> StreamableHttpService<H, NeverSessionManager>
where
    H: rmcp::ServerHandler + Send + 'static,
{
    StreamableHttpService::new(factory, Arc::new(NeverSessionManager::default()), config)
}

#[cfg(test)]
mod tests {
    use super::*;

    // NOTE on the 400 mechanism: `2025-03-26` is a KNOWN version in rmcp, so
    // the MCP-Protocol-Version header check alone passes it. The 400 below
    // comes from `stateless_protocol_metadata_required = true`: legacy
    // requests carry no `_meta.io.modelcontextprotocol/protocolVersion`
    // per-request metadata, and rmcp rejects them before handler dispatch.
    #[tokio::test]
    async fn unsupported_legacy_version_returns_bad_request() {
        let state = crate::http::HttpState::default_for_test().await;
        let router = crate::http::router::build_router(state);
        let req = Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header("host", "localhost")
            .header("MCP-Protocol-Version", "2025-03-26")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)).unwrap();
        let resp: Response<Body> = tower::ServiceExt::oneshot(router, req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
```

- [ ] **Step 2: Implement `HttpConfig::default_for_test()`**

Add to `config.rs`:

```rust
#[cfg(any(test, feature = "test-fixtures"))]
impl HttpConfig {
    pub fn default_for_test() -> Self {
        Self {
            bind: "127.0.0.1:0".parse().unwrap(),
            public_base_url: "http://localhost".into(),
            trusted_proxy_cidrs: Vec::new(),
            allowed_hosts: vec!["localhost".into(), "127.0.0.1".into()],
            allowed_origins: vec!["http://localhost".into()],
            body_limit_bytes: DEFAULT_BODY_LIMIT_BYTES,
            request_deadline: DEFAULT_REQUEST_DEADLINE,
            shutdown_grace: DEFAULT_SHUTDOWN_GRACE,
            control_db: SurrealTargetConfig::default_for_test(),
            tenant_db: SurrealTargetConfig::default_for_test(),
            api_key_pepper: "x".repeat(40),
            identity_index_key: [0; 32],
            control_plane_session_key: [0; 32],
            oidc_state_key: [0; 32],
            oidc_nonce_key: [0; 32],
            csrf_key: [0; 32],
            oidc_issuer: "https://issuer.invalid".into(),
            oidc_client_id: "test-client".into(),
            oidc_audience: "memory-mcp".into(),
            oidc_redirect_uri: "http://localhost/auth/oidc/callback".into(),
            oidc_allowed_alg: "RS256".into(),
            signup_mode: SignupMode::InviteOnly,
            enable_control_plane: false,
            enable_control_plane_ui: false,
        }
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
impl SurrealTargetConfig {
    pub fn default_for_test() -> Self {
        Self {
            url: "mem://".into(),
            username: "root".into(),
            password: "root".into(),
            database: "memory_test".into(),
            namespace: "test".into(),
        }
    }
}
```

- [ ] **Step 3: Create `shutdown.rs`, `health.rs`, `router.rs`, `server.rs`, the Phase 3 `HttpState`, and register modules**

Append to `http/mod.rs`:

```rust
pub mod health;
pub mod router;
pub mod server;
pub mod shutdown;
pub mod transport;

use std::sync::Arc;

use config::HttpConfig;

/// Process-wide HTTP state. Phase 3 shape: config + the tenantless MCP
/// factory. Later tasks extend this struct (3.8 metrics handle, 3.9
/// shutdown/admission/registry, 4.4 authenticator, 4.5 account_resolver,
/// 5.6 pool). Task 5.6 removes `mcp_factory` again when the tenant runtime
/// pool takes over dispatch.
pub struct HttpState {
    pub config: HttpConfig,
    /// Phase 3 dispatch factory: clones a prebuilt tenantless handler per
    /// request. Replaced by the runtime-pool guard in Task 5.6.
    pub mcp_factory: Arc<dyn Fn() -> Result<crate::mcp::handlers::MemoryMcp, std::io::Error> + Send + Sync>,
    pub request_logger: Arc<crate::logging::StdoutLogger>,
}

impl HttpState {
    /// Phase 3 production constructor: single-tenant handler over the
    /// configured tenant target (no auth yet — auth lands in Phase 4).
    pub async fn new_tenantless(config: HttpConfig) -> Result<Arc<Self>, crate::error::MemoryError> {
        let mcp = transport::build_tenantless_handler(&config).await?;
        Ok(Arc::new(Self {
            mcp_factory: Arc::new(move || Ok((*mcp).clone())),
            request_logger: Arc::new(crate::logging::StdoutLogger::new("info")),
            config,
        }))
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
impl HttpState {
    pub async fn default_for_test() -> Arc<Self> {
        let config = HttpConfig::default_for_test();
        Self::new_tenantless(config)
            .await
            .expect("tenantless test state builds")
    }
}
```

In `shutdown.rs`:

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio_util::sync::CancellationToken;

pub fn cancellation_token() -> CancellationToken {
    static CT: std::sync::OnceLock<CancellationToken> = std::sync::OnceLock::new();
    CT.get_or_init(CancellationToken::new).clone()
}

#[derive(Clone)]
pub struct ShutdownState {
    flag: Arc<AtomicBool>,
    token: CancellationToken,
}

impl Default for ShutdownState {
    fn default() -> Self {
        Self { flag: Arc::new(AtomicBool::new(false)), token: CancellationToken::new() }
    }
}

impl ShutdownState {
    pub fn new() -> Self { Self::default() }
    pub fn is_shutting_down(&self) -> bool { self.flag.load(Ordering::SeqCst) }
    pub fn begin(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.token.cancel();
    }
    pub fn token(&self) -> CancellationToken { self.token.clone() }
}
```

In `health.rs`:

```rust
pub async fn live() -> &'static str { "ok" }
pub async fn ready() -> &'static str { "ok" }
```

(`ready` gains the registry/admission probe in Task 3.9.)

In `router.rs` — ONE production builder from day one (no test-only twin):

```rust
use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

use super::HttpState;

pub fn build_router(state: Arc<HttpState>) -> Router {
    Router::new()
        .route("/health/live", get(super::health::live))
        .route("/health/ready", get(super::health::ready))
        .route("/mcp", post(super::transport::mcp_handler))
        .with_state(state)
}
```

In `server.rs`:

```rust
use axum::serve;
use tokio::net::TcpListener;

use super::config::HttpConfig;

/// Binds, reports the local address on stdout as `memory_mcp_http bound=<addr>`
/// (integration tests parse this line), then serves until the shutdown token is
/// cancelled or the listener closes.
pub async fn serve(
    cfg: HttpConfig,
    router: axum::Router,
    shutdown: super::shutdown::ShutdownState,
) -> std::io::Result<()> {
    let listener = TcpListener::bind(cfg.bind).await?;
    let local_addr = listener.local_addr()?;
    tracing::info!(target: "memory_mcp::http", bound = %local_addr, "http listener bound");
    println!("memory_mcp_http bound={local_addr}");
    let token = shutdown.token();
    let grace = cfg.shutdown_grace;
    axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>())
        .with_graceful_shutdown(async move {
            token.cancelled().await;
            tokio::time::sleep(grace).await;
        })
        .await
}
```

In `transport.rs`, append the PRODUCTION Phase 3 handler and its factory.
Phase 3 is deliberately a real single-tenant server (over the configured
tenant target), so the Task 3.11 black-box suite exercises genuine
negotiation against the real binary — no test-only dispatch path:

```rust
use axum::extract::State;

/// Phase 3 production handler: dispatches through the tenantless factory in
/// state. Task 5.6 replaces the body with runtime-pool dispatch.
pub async fn mcp_handler(
    State(state): State<std::sync::Arc<super::HttpState>>,
    req: axum::extract::Request,
) -> axum::response::Response {
    let svc = build_mcp_service(
        state.mcp_factory.clone(),
        build_server_config(&state.config, super::shutdown::cancellation_token()),
    );
    forward(svc, req).await
}

/// Shared forward path: type-erase the axum body, call the rmcp service,
/// re-wrap the box body. `StreamableHttpService::Error = Infallible`.
pub async fn forward(
    mut svc: StreamableHttpService<crate::mcp::handlers::MemoryMcp, NeverSessionManager>,
    req: axum::extract::Request,
) -> axum::response::Response {
    let (parts, body) = req.into_parts();
    let http_req: http::Request<axum::body::Body> = http::Request::from_parts(parts, body);
    match <_ as tower_service::Service<http::Request<axum::body::Body>>>::call(
        &mut svc, http_req,
    )
    .await
    {
        Ok(resp) => resp.map(axum::body::Body::new),
        Err(infallible) => match infallible {},
    }
}

/// Builds the Phase 3 tenantless handler over the configured tenant target.
/// `mem://` selects the embedded in-memory engine (tests, smoke runs);
/// anything else connects to the remote endpoint.
pub async fn build_tenantless_handler(
    cfg: &super::config::HttpConfig,
) -> Result<std::sync::Arc<crate::mcp::handlers::MemoryMcp>, crate::error::MemoryError> {
    let t = &cfg.tenant_db;
    let client = if t.url == "mem://" {
        crate::storage::client::SurrealDbClient::connect_in_memory(
            &t.database, &t.namespace, "warn",
        )
        .await?
    } else {
        crate::storage::client::SurrealDbClient::connect_bound(
            &t.url, &t.username, &t.password, &t.namespace, &t.database, "warn",
        )
        .await?
    };
    client.apply_migrations(&t.namespace).await?;
    let service = crate::service::MemoryService::new(
        std::sync::Arc::new(client),
        t.namespace.clone(),
        "warn".into(),
        100,
        100,
    )?;
    Ok(std::sync::Arc::new(crate::mcp::handlers::MemoryMcp::new(service)))
}
```

Note: `tower-service` is declared explicitly in the Phase 2 proposal; rmcp
does not re-export it. Add this constructor in `storage/client.rs` in this
task so Phase 3 is self-contained; Task 4.1 only verifies and reuses it:

```rust
impl SurrealDbClient {
    /// Connect to a privileged remote SurrealDB endpoint and bind one
    /// namespace/database. The HTTP profile uses a root credential because
    /// later provisioning must issue namespace/database DDL; stdio never
    /// calls this constructor.
    pub async fn connect_bound(
        url: &str,
        username: &str,
        password: &str,
        namespace: &str,
        database: &str,
        log_level: &str,
    ) -> Result<Self, MemoryError> {
        let db = surrealdb::Surreal::new::<surrealdb::engine::remote::ws::Ws>(url)
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB connect failed: {err}")))?;
        db.signin(surrealdb::opt::auth::Root {
            username: username.to_string(),
            password: password.to_string(),
        })
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB signin failed: {err}")))?;
        db.use_ns(namespace).use_db(database)
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB use failed: {err}")))?;
        Ok(Self {
            engine: DbEngine::Remote(std::sync::Arc::new(db)),
            active_namespace: namespace.to_string(),
            logger: StdoutLogger::new(log_level),
            fact_embedding_dimension: crate::config::DEFAULT_EMBEDDING_DIMENSION,
        })
    }
}
```

Run:

```bash
cargo test -p memory_mcp --features streamable-http,test-fixtures --lib http::transport
```

Expected: PASS. The Step 1 test is written directly against the state-based
router (`HttpState::default_for_test().await` + `build_router(state)`), so it
only compiles from this step on; the legacy-version test exercises the real
rmcp validation path against a real handler — no `#[ignore]`, no deferred
factory, no test-only router twin.

- [ ] **Step 4: Commit**

```bash
git add crates/memory-mcp/src/http/ crates/memory-mcp/src/mcp/handlers.rs crates/memory-mcp/src/storage/client.rs
git commit -m "feat(http): wire modern-only StreamableHttpService skeleton"
```

### Task 3.4: Bind the HTTP profile to protocol `2026-07-28` only

**Files:**
- Modify: `crates/memory-mcp/src/mcp/handlers.rs`

**Background (verified against rmcp 3.1.2 source).** `rmcp::model::ProtocolVersion`
is a newtype struct with associated constants — not an enum:
`V_2024_11_05`, `V_2025_03_26`, `V_2025_06_18`, `V_2025_11_25`, `V_2026_07_28`,
and `LATEST == V_2025_11_25`. Two rmcp behaviors shape this task:

1. `ServerHandler::supported_protocol_versions()` defaults to all known
   versions; it is advertised by `server/discover` and bounds negotiation.
2. `negotiate_protocol_version` never rejects. When the client requests an
   unsupported version it falls back to the `protocol_version` carried by the
   `ServerInfo` returned from `get_info()` — which defaults to `LATEST`
   (`2025-11-25`). Overriding only `supported_protocol_versions` would still
   let a legacy client negotiate `2025-11-25`. The `get_info()` fallback must
   be pinned to `2026-07-28` as well.

The stdio profile must stay byte-for-byte unchanged. Features are crate-wide,
so a `#[cfg(feature = "streamable-http")]` override would also change the
stdio binary whenever the feature is compiled in. Instead the switch is a
**runtime flag** on `MemoryMcp`, set only by the HTTP constructor.

- [ ] **Step 1: Add the protocol-mode flag**

In `crates/memory-mcp/src/mcp/handlers.rs`:

```rust
#[derive(Clone)]
pub struct MemoryMcp {
    service: Arc<MemoryService>,
    #[cfg(feature = "mcp-apps")]
    session_manager: SessionManager,
    tasks: TaskManager,
    tool_router: ToolRouter<Self>,
    /// When true, advertise and negotiate only MCP 2026-07-28 (HTTP SaaS
    /// profile, ADR-0052). stdio constructors leave this false, preserving
    /// the frozen stdio behavior (ADR-0038) regardless of feature flags.
    modern_protocol_only: bool,
}

impl MemoryMcp {
    pub fn new(service: MemoryService) -> Self {
        Self {
            service: Arc::new(service),
            #[cfg(feature = "mcp-apps")]
            session_manager: SessionManager::new(),
            tasks: TaskManager::new(),
            tool_router: Self::tool_router(),
            modern_protocol_only: false,
        }
    }

    /// HTTP SaaS profile constructor: modern protocol only.
    pub fn new_modern(service: MemoryService) -> Self {
        Self {
            modern_protocol_only: true,
            ..Self::new(service)
        }
    }
}
```

- [ ] **Step 2: Override `get_info` fallback and `supported_protocol_versions`**

Inside `#[tool_handler(router = self.tool_router)] impl ServerHandler for MemoryMcp`:
keep `build_server_info()` unchanged for stdio, and add a separate HTTP metadata
builder. `get_info()` must select `build_http_server_info()` when
`modern_protocol_only` is true. The HTTP builder always enables tools, enables
Tasks only for the extraction path, enables Resources/App capabilities only
when `mcp-apps` is compiled and the HTTP App backend is attached, and enables
`resources.subscribe` only after the durable subscription backend is attached.
It does not
advertise MRTR, roots, sampling, elicitation, prompts-change, or tool-list-change.
This avoids changing stdio metadata merely because the HTTP feature was
compiled.

```rust
fn build_http_server_info(&self) -> ServerInfo {
    let mut builder = ServerCapabilities::builder().enable_tools().enable_tasks();
    #[cfg(feature = "mcp-apps")]
    {
        builder = builder.enable_resources();
    }
    ServerInfo::new(builder.build()).with_instructions(Self::SERVER_INSTRUCTIONS)
}

fn get_info(&self) -> ServerInfo {
    let mut info = if self.modern_protocol_only {
        self.build_http_server_info()
    } else {
        Self::build_server_info()
    };
    if self.modern_protocol_only {
        // Pin the negotiation fallback: rmcp falls back to this version when
        // a client requests one we do not support.
        info = info.with_protocol_version(rmcp::model::ProtocolVersion::V_2026_07_28);
    }
    info
}

fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [rmcp::model::ProtocolVersion]> {
    if self.modern_protocol_only {
        std::borrow::Cow::Owned(vec![rmcp::model::ProtocolVersion::V_2026_07_28])
    } else {
        std::borrow::Cow::Borrowed(rmcp::model::ProtocolVersion::KNOWN_VERSIONS)
    }
}
```

- [ ] **Step 3: Point the Phase 3 tenantless handler at the modern profile**

In `crates/memory-mcp/src/http/transport.rs`, change `build_tenantless_handler`
(added in Task 3.3 Step 3) to construct the handler via `new_modern` so
lib-level conformance tests and the Task 3.11 `server/discover` assertion see
the HTTP profile. Replace its final line:

```rust
    Ok(std::sync::Arc::new(crate::mcp::handlers::MemoryMcp::new(service)))
```

with:

```rust
    Ok(std::sync::Arc::new(crate::mcp::handlers::MemoryMcp::new_modern(service)))
```

- [ ] **Step 4: Add tests in the `handlers.rs` test module**

The existing test module builds the service inline inside the async
`create_test_mcp()` helper (over `test_db_client()`). Extract the service
construction into an async helper first and rewrite `create_test_mcp` to use
it (both live in `mod tests` in `handlers.rs`):

```rust
async fn test_service() -> MemoryService {
    MemoryService::new(
        test_db_client().await,
        "org".to_string(),
        "warn".to_string(),
        50,
        100,
    )
    .expect("create test service")
}

async fn create_test_mcp() -> MemoryMcp {
    MemoryMcp::new(test_service().await)
}
```

Then add the protocol-mode tests — async, because `test_service()` is:

```rust
#[tokio::test]
async fn stdio_mode_advertises_all_known_versions() {
    // Construct via the same path stdio uses (modern_protocol_only = false).
    let mcp = MemoryMcp::new(test_service().await);
    let versions = mcp.supported_protocol_versions();
    assert_eq!(versions.len(), rmcp::model::ProtocolVersion::KNOWN_VERSIONS.len());
    assert_eq!(mcp.get_info().protocol_version, rmcp::model::ProtocolVersion::LATEST);
}

#[tokio::test]
async fn modern_mode_advertises_only_2026_07_28() {
    let mcp = MemoryMcp::new_modern(test_service().await);
    let versions = mcp.supported_protocol_versions();
    let names: Vec<&str> = versions.iter().map(|v| v.as_str()).collect();
    assert_eq!(names, vec!["2026-07-28"]);
    // The negotiation fallback must be modern too.
    assert_eq!(
        mcp.get_info().protocol_version,
        rmcp::model::ProtocolVersion::V_2026_07_28
    );
}
```

- [ ] **Step 5: Run the tests**

Run:

```bash
cargo test -p memory_mcp --features streamable-http --lib mcp::handlers
```

Expected: PASS.

- [ ] **Step 6: Run stdio regression**

Run:

```bash
cargo test -p memory_mcp --features fs-watch,mcp-apps --test tools_e2e --test service_acceptance
```

Expected: PASS. The stdio path constructs `MemoryMcp::new(...)`
(`modern_protocol_only = false`), so its advertised versions and negotiation
fallback are exactly the rmcp defaults — unchanged even when the
`streamable-http` feature is compiled in.

- [ ] **Step 7: Commit**

```bash
git add crates/memory-mcp/src/mcp/handlers.rs crates/memory-mcp/src/http/transport.rs
git commit -m "feat(http): pin HTTP profile to MCP 2026-07-28 via runtime flag"
```

### Task 3.5: GET/DELETE on `/mcp` return 405

**Files:**
- Modify: `crates/memory-mcp/src/http/router.rs`
- Modify: `crates/memory-mcp/src/http/middleware.rs` (new)

- [ ] **Step 1: Write failing tests**

Append to `transport.rs` `mod tests`. The body assertion is what makes the
test non-vacuous: axum's matcher already answers 405 for a wrong method on a
`post(...)` route, but with an EMPTY body — only the middleware produces the
`POST required` text:

```rust
#[tokio::test]
async fn get_on_mcp_returns_405_from_middleware() {
    use tower::ServiceExt;
    let state = crate::http::HttpState::default_for_test().await;
    let router = crate::http::router::build_router(state);
    let resp = router.oneshot(Request::builder().method(Method::GET).uri("/mcp")
        .header("host", "localhost").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&body[..], b"POST required");
}

#[tokio::test]
async fn delete_on_mcp_returns_405_from_middleware() {
    use tower::ServiceExt;
    let state = crate::http::HttpState::default_for_test().await;
    let router = crate::http::router::build_router(state);
    let resp = router.oneshot(Request::builder().method(Method::DELETE).uri("/mcp")
        .header("host", "localhost").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&body[..], b"POST required");
}
```

Run:

```bash
cargo test -p memory_mcp --features streamable-http,test-fixtures --lib http::transport
```

Expected: FAIL — status is 405 (axum matcher) but the body is empty, because
the middleware does not exist yet.

- [ ] **Step 2: Implement the middleware and wire it into the router**

Create `middleware.rs`. Axum `from_fn` contract: `Next` must be the LAST
parameter and the middleware must call `next.run(req).await` — without it the
middleware silently terminates every request:

```rust
use axum::extract::{OriginalUri, Request};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

/// Reject every non-POST method on `/mcp` (spec §4). Runs before routing;
/// all other paths pass through untouched.
pub async fn reject_non_post_mcp(
    method: Method,
    OriginalUri(uri): OriginalUri,
    req: Request,
    next: Next,
) -> Result<Response, (StatusCode, &'static str)> {
    if uri.path() == "/mcp" && method != Method::POST {
        return Err((StatusCode::METHOD_NOT_ALLOWED, "POST required"));
    }
    Ok(next.run(req).await)
}
```

Register the module in `http/mod.rs` (`pub mod middleware;`) and add the
layer to the production `build_router` (Task 3.3 shape):

```rust
pub fn build_router(state: Arc<HttpState>) -> Router {
    Router::new()
        .route("/health/live", get(super::health::live))
        .route("/health/ready", get(super::health::ready))
        .route("/mcp", post(super::transport::mcp_handler))
        .layer(axum::middleware::from_fn(super::middleware::reject_non_post_mcp))
        .with_state(state)
}
```

- [ ] **Step 3: Run tests**

Run:

```bash
cargo test -p memory_mcp --features streamable-http,test-fixtures --lib http::transport
```

Expected: PASS — 405 now carries the middleware's `POST required` body
(defense-in-depth per spec §4, on top of axum's own matcher).

- [ ] **Step 4: Commit**

```bash
git add crates/memory-mcp/src/http/middleware.rs crates/memory-mcp/src/http/router.rs crates/memory-mcp/src/http/transport.rs
git commit -m "feat(http): GET/DELETE on /mcp return 405"
```

### Task 3.6: Host/Origin allowlist middleware

**Files:**
- Modify: `crates/memory-mcp/src/http/middleware.rs`

- [ ] **Step 1: Write tests**

The middleware extracts `State<Arc<HttpState>>`, so tests must attach state.
The server installs `SocketAddr` connect-info via
`into_make_service_with_connect_info`; forwarding headers are honored only
when that peer address matches `trusted_proxy_cidrs`. Unit tests without
connect-info are treated as direct clients and therefore use the ordinary
`Host` header.
A bare `axum::middleware::from_fn(mw)` on a raw tower stack would panic at
runtime with "state not provided". Use a `Router` with
`from_fn_with_state` + `with_state`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    async fn router_with_host_origin() -> Router {
        let state = crate::http::HttpState::default_for_test().await;
        Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                host_origin,
            ))
            .with_state(state)
    }

    #[tokio::test]
    async fn rejects_disallowed_origin() {
        let resp = router_with_host_origin().await.oneshot(
            Request::builder().uri("/").header("host", "localhost")
                .header("origin", "https://evil.example").body(Body::empty()).unwrap()
        ).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn allows_missing_origin_for_non_browser() {
        let resp = router_with_host_origin().await.oneshot(
            Request::builder().uri("/").header("host", "localhost").body(Body::empty()).unwrap()
        ).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn rejects_disallowed_host() {
        let resp = router_with_host_origin().await.oneshot(
            Request::builder().uri("/").header("host", "evil.example").body(Body::empty()).unwrap()
        ).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
```

- [ ] **Step 2: Implement `host_origin`**

```rust
pub async fn host_origin(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::http::HttpState>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, (StatusCode, &'static str)> {
    let headers = req.headers();
    let peer = req
        .extensions()
        .get::<axum::extract::connect_info::ConnectInfo<std::net::SocketAddr>>()
        .map(|info| info.0.ip());
    let trusted_peer = peer.is_some_and(|ip| state.config.trusted_proxy_cidrs.iter().any(|cidr| cidr.contains(ip)));
    let host_header = if trusted_peer {
        headers.get("x-forwarded-host").or_else(|| headers.get("host"))
    } else {
        // Never honor forwarding headers from an untrusted peer.
        headers.get("host")
    };
    let host = host_header.and_then(|h| h.to_str().ok()).unwrap_or("");
    if !state.config.allowed_hosts.iter().any(|h| h.eq_ignore_ascii_case(host)) {
        return Err((StatusCode::FORBIDDEN, "host not allowed"));
    }
    if let Some(origin) = headers.get("origin").and_then(|h| h.to_str().ok()) {
        if !state.config.allowed_origins.iter().any(|o| o == origin) {
            return Err((StatusCode::FORBIDDEN, "origin not allowed"));
        }
    }
    Ok(next.run(req).await)
}
```

`HttpState` already exists from Task 3.3 (`config` + `mcp_factory`, async
`new_tenantless`, async `default_for_test`) — this task adds NO fields and NO
constructors, only the `host_origin` middleware above. Do not redefine the
struct here.

- [ ] **Step 3: Wire middleware into the router**

Add the `host_origin` layer to `build_router` (Task 3.5 shape). Axum layers
added LATER wrap layers added EARLIER, so on the request path `host_origin`
runs first, then `reject_non_post_mcp`:

```rust
pub fn build_router(state: Arc<HttpState>) -> Router {
    Router::new()
        .route("/health/live", get(super::health::live))
        .route("/health/ready", get(super::health::ready))
        .route("/mcp", post(super::transport::mcp_handler))
        .layer(axum::middleware::from_fn(super::middleware::reject_non_post_mcp))
        .layer(axum::middleware::from_fn_with_state(state.clone(), super::middleware::host_origin))
        .with_state(state)
}
```

All later handlers take `State(state): State<Arc<HttpState>>` — this is the
canonical state type across the whole plan; no handler uses bare `HttpState`.

- [ ] **Step 4: Run tests**

Run:

```bash
cargo test -p memory_mcp --features streamable-http --lib http::middleware
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/http/
git commit -m "feat(http): Host/Origin allowlist middleware"
```

### Task 3.7: SSE response headers and body deadline

**Files:**
- Modify: `crates/memory-mcp/src/http/middleware.rs`
- Create: `crates/memory-mcp/src/http/validation.rs`
- Modify: `crates/memory-mcp/src/http/transport.rs`
- Modify: `crates/memory-mcp/src/http/mod.rs`

- [ ] **Step 1: Add tests**

The old draft asserted `status == 400 || header present`, which passes even
if the headers are never injected — a vacuous test. Instead, exercise the
middleware against a handler that genuinely produces an SSE response and one
that produces JSON:

```rust
#[tokio::test]
async fn sse_responses_get_no_cache_and_no_buffering_headers() {
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    async fn sse_stub() -> (StatusCode, [(header::HeaderName, &'static str); 1], &'static str) {
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/event-stream")],
            "data: {}\n\n",
        )
    }

    let router = Router::new()
        .route("/sse", get(sse_stub))
        .layer(axum::middleware::from_fn(inject_sse_headers));
    let resp = router
        .oneshot(Request::builder().uri("/sse").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("cache-control").map(|v| v.to_str().unwrap()),
        Some("no-cache")
    );
    assert_eq!(
        resp.headers().get("x-accel-buffering").map(|v| v.to_str().unwrap()),
        Some("no")
    );
}

#[tokio::test]
async fn json_responses_are_not_modified() {
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    async fn json_stub() -> (StatusCode, [(header::HeaderName, &'static str); 1], &'static str) {
        (StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], "{}")
    }

    let router = Router::new()
        .route("/json", get(json_stub))
        .layer(axum::middleware::from_fn(inject_sse_headers));
    let resp = router
        .oneshot(Request::builder().uri("/json").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(resp.headers().get("x-accel-buffering").is_none());
    assert!(resp.headers().get("cache-control").is_none());
}
```

- [ ] **Step 2: Implement response header injection and request deadline**

In `middleware.rs` (no `unwrap`/`parse` at runtime — use static header values):

```rust
use axum::http::header::HeaderValue;

const NO_CACHE: HeaderValue = HeaderValue::from_static("no-cache");
const NO_BUFFERING: HeaderValue = HeaderValue::from_static("no");

pub async fn inject_sse_headers(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut resp = next.run(req).await;
    let is_sse = resp
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("text/event-stream"));
    if is_sse {
        let headers = resp.headers_mut();
        headers.entry(axum::http::header::CACHE_CONTROL).or_insert(NO_CACHE);
        headers.entry("x-accel-buffering").or_insert(NO_BUFFERING);
    }
    resp
}

/// Bound handler execution by the configured whole-request deadline. The
/// Streamable HTTP service also receives the same cancellation token, so a
/// timeout stops request-owned work; already durable commits remain durable.
pub async fn request_deadline(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::http::HttpState>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, axum::http::StatusCode> {
    tokio::time::timeout(state.config.request_deadline, next.run(req))
        .await
        .map_err(|_| axum::http::StatusCode::REQUEST_TIMEOUT)
}
```

The timeout above covers handler execution, but not body polling after the
handler returns. Add `validation.rs` and register `pub mod validation;` in
`http/mod.rs`. Update the Phase 3 transport handler in `transport.rs` to wrap the
returned body with this adapter before returning it, so a client that keeps an
SSE response open cannot hold the process indefinitely:

```rust
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use tokio::time::Sleep;

pub struct DeadlineBody {
    inner: Pin<Box<axum::body::Body>>,
    timer: Option<Pin<Box<Sleep>>>,
    deadline: Option<Instant>,
    finished: bool,
}

impl DeadlineBody {
    pub fn new(body: axum::body::Body, timeout: Option<Duration>) -> Self {
        Self {
            inner: Box::pin(body),
            timer: timeout.map(|value| Box::pin(tokio::time::sleep(value))),
            deadline: timeout.map(|value| Instant::now() + value),
            finished: false,
        }
    }
}

impl Body for DeadlineBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if self.finished {
            return Poll::Ready(None);
        }
        let expired = self.timer.as_mut().is_some_and(|timer| timer.as_mut().poll(cx).is_ready())
            || self.deadline.is_some_and(|deadline| Instant::now() >= deadline);
        if expired {
            self.finished = true;
            return Poll::Ready(Some(Err(axum::Error::new(
                std::io::Error::new(std::io::ErrorKind::TimedOut, "HTTP response deadline exceeded"),
            ))));
        }
        match self.inner.as_mut().poll_frame(cx) {
            Poll::Ready(None) => {
                self.finished = true;
                Poll::Ready(None)
            }
            other => other,
        }
    }

    fn is_end_stream(&self) -> bool { self.finished || self.inner.as_ref().get_ref().is_end_stream() }
    fn size_hint(&self) -> SizeHint { self.inner.as_ref().get_ref().size_hint() }
}

pub fn with_body_deadline(
    response: axum::response::Response,
    timeout: Option<Duration>,
) -> axum::response::Response {
    let (parts, body) = response.into_parts();
    axum::response::Response::from_parts(
        parts,
        axum::body::Body::new(DeadlineBody::new(body, timeout)),
    )
}
```

`DeadlineBody` emits one body error and then ends. The normal completion path
also ends the wrapper; dropping the response before the deadline cancels the
underlying body. Ordinary request responses use `Some(request_deadline)`, while
`Mcp-Method: subscriptions/listen` uses `None`: that is the intentionally
long-lived POST-response stream and is ended by client cancellation, server
shutdown, or authorization loss, not by the ordinary 120-second call deadline.
Tests must cover both a body that completes before the deadline and a body that
remains pending until the timer fires.

Update the Phase 3 `mcp_handler` from Task 3.3 to inspect the already-validated
`Mcp-Method` header and call
`with_body_deadline(forward(svc, req).await, if is_subscription { None } else { Some(state.config.request_deadline) })`.
Do not apply the wrapper twice: Task 5.6 composes `DeadlineBody` inside
`LeasedBody`, which adds the runtime/admission lifetime to the same body.

- [ ] **Step 3: Test the full body lifetime**

```rust
#[tokio::test]
async fn deadline_body_allows_completion_before_timeout() {
    use http_body_util::BodyExt;
    let body = DeadlineBody::new(axum::body::Body::from("ok"), Some(Duration::from_secs(1)));
    let collected = body.collect().await.expect("body completes");
    assert_eq!(&collected.to_bytes()[..], b"ok");
}

#[tokio::test]
async fn deadline_body_returns_timeout_error_when_expired() {
    use http_body::Body as _;
    let mut body = DeadlineBody::new(axum::body::Body::from("late"), Some(Duration::ZERO));
    let frame = body.frame().await.expect("timeout frame");
    assert!(frame.is_err());
}
```

Expected: the first test returns the payload, and the second returns one
`TimedOut` body error. A third integration assertion in Task 5.5 proves that
this body error also drops the runtime/admission lease.

- [ ] **Step 4: Wire and run**

Add the layer to `build_router` (Task 3.6 shape). It is added LAST, so it is
the OUTERMOST layer: it observes the final response after every other
middleware, which is exactly where SSE content-type detection belongs:

```rust
pub fn build_router(state: Arc<HttpState>) -> Router {
    Router::new()
        .route("/health/live", get(super::health::live))
        .route("/health/ready", get(super::health::ready))
        .route("/mcp", post(super::transport::mcp_handler))
        .layer(axum::middleware::from_fn(super::middleware::reject_non_post_mcp))
        .layer(axum::middleware::from_fn_with_state(state.clone(), super::middleware::host_origin))
        .layer(axum::middleware::from_fn_with_state(state.clone(), super::middleware::request_deadline))
        .layer(axum::middleware::from_fn(super::middleware::inject_sse_headers))
        .with_state(state)
}
```

This is the final Phase 3 layer stack; Task 4.6 and Task 5.6 add route-scoped
layers on `/mcp` only.

Run:

```bash
cargo test -p memory_mcp --features streamable-http --lib http::middleware
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/http/
git commit -m "feat(http): SSE response Cache-Control and X-Accel-Buffering headers"
```

### Task 3.8: `/metrics` route behind feature flag

**Files:**
- Modify: `crates/memory-mcp/src/http/router.rs`
- Modify: `crates/memory-mcp/src/http/metrics.rs` (new)
- Modify: `crates/memory-mcp/src/http/server.rs`
- Modify: `crates/memory-mcp/Cargo.toml` (no dep change; `prometheus` feature already gates `metrics-exporter-prometheus`)

**Design.** The codebase already standardizes on the `metrics` facade +
`metrics-exporter-prometheus` (ADR-0048, `src/observability.rs`). The stdio
profile optionally installs its own HTTP *listener* via `observability::install()`
when `MEMORY_PROMETHEUS_LISTEN_ADDR` is set. The HTTP profile instead serves
scrapes on its own axum router, so it installs a **recorder** once at startup
and renders via the stored `PrometheusHandle`. Installing a recorder twice
panics/errs — therefore:

1. The recorder is installed exactly once, in the composition root, and the
   result is mapped to `MemoryError` (no `expect()`/`unwrap()`).
2. Startup validation rejects the combination `prometheus` feature +
   `MEMORY_PROMETHEUS_LISTEN_ADDR` set in HTTP mode (two scrape surfaces for
   one recorder is a configuration error).

- [ ] **Step 1: Recorder installation**

In `metrics.rs`:

```rust
//! Prometheus scrape surface for the HTTP profile (ADR-0048).

use crate::error::MemoryError;

/// Install the process-wide recorder and return its render handle.
///
/// Fails when a recorder was already installed (e.g. something else called
/// `metrics_exporter_prometheus` install paths in this process) — the HTTP
/// composition root treats that as a startup error, never a panic.
#[cfg(feature = "prometheus")]
pub fn install_recorder() -> Result<metrics_exporter_prometheus::PrometheusHandle, MemoryError> {
    metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .map_err(|err| {
            MemoryError::ConfigInvalid(format!(
                "failed to install Prometheus recorder for /metrics: {err}"
            ))
        })
}

/// Reject the stdio-profile listener env var in HTTP mode: the HTTP profile
/// serves metrics on its own router and cannot share the recorder with a
/// second listener.
#[cfg(feature = "prometheus")]
pub fn validate_no_listener_env() -> Result<(), MemoryError> {
    match std::env::var(crate::observability::ENV_PROMETHEUS_LISTEN_ADDR) {
        Ok(v) if !v.trim().is_empty() => Err(MemoryError::ConfigInvalid(format!(
            "{} must not be set in the HTTP profile; metrics are served on /metrics",
            crate::observability::ENV_PROMETHEUS_LISTEN_ADDR
        ))),
        _ => Ok(()),
    }
}
```

- [ ] **Step 2: Store the handle in `HttpState`**

Extend the Task 3.3 `HttpState` ADDITIVELY: one new cfg-gated field and one
new cfg-gated parameter on `new_tenantless` (`#[cfg]` on a function parameter
is stable Rust and keeps a single constructor body instead of two duplicated
ones; call sites are cfg-split in Task 3.10). Do NOT redefine the struct or
drop `mcp_factory`:

```rust
pub struct HttpState {
    pub config: HttpConfig,
    /// Phase 3 dispatch factory (Task 3.3). Removed in Task 5.6.
    pub mcp_factory: Arc<dyn Fn() -> Result<crate::mcp::handlers::MemoryMcp, std::io::Error> + Send + Sync>,
    pub request_logger: Arc<crate::logging::StdoutLogger>,
    #[cfg(feature = "prometheus")]
    pub metrics_handle: metrics_exporter_prometheus::PrometheusHandle,
}

impl HttpState {
    /// Phase 3 production constructor. With the `prometheus` feature the
    /// recorder handle becomes an additional parameter.
    pub async fn new_tenantless(
        config: HttpConfig,
        #[cfg(feature = "prometheus")] metrics_handle: metrics_exporter_prometheus::PrometheusHandle,
    ) -> Result<Arc<Self>, crate::error::MemoryError> {
        let mcp = transport::build_tenantless_handler(&config).await?;
        Ok(Arc::new(Self {
            mcp_factory: Arc::new(move || Ok((*mcp).clone())),
            request_logger: Arc::new(crate::logging::StdoutLogger::new("info")),
            config,
            #[cfg(feature = "prometheus")]
            metrics_handle,
        }))
    }
}
```

Replace the Task 3.3 `default_for_test` with the cfg-split version below —
the handle is installed via a test-only `OnceLock` so repeated constructions
do not double-install the recorder:

```rust
#[cfg(all(any(test, feature = "test-fixtures"), feature = "prometheus"))]
fn test_metrics_handle() -> metrics_exporter_prometheus::PrometheusHandle {
    static HANDLE: std::sync::OnceLock<metrics_exporter_prometheus::PrometheusHandle> =
        std::sync::OnceLock::new();
    HANDLE
        .get_or_init(|| {
            metrics_exporter_prometheus::PrometheusBuilder::new()
                .install_recorder()
                .expect("first recorder install in the test process")
        })
        .clone()
}

#[cfg(any(test, feature = "test-fixtures"))]
impl HttpState {
    pub async fn default_for_test() -> Arc<Self> {
        let config = HttpConfig::default_for_test();
        #[cfg(feature = "prometheus")]
        {
            Self::new_tenantless(config, test_metrics_handle())
                .await
                .expect("tenantless test state builds")
        }
        #[cfg(not(feature = "prometheus"))]
        {
            Self::new_tenantless(config)
                .await
                .expect("tenantless test state builds")
        }
    }
}
```

(`expect` is permitted in test-gated code; production paths return `Result`.)

- [ ] **Step 3: Implement the `/metrics` handler**

```rust
use axum::extract::State;
use axum::http::StatusCode;

#[cfg(feature = "prometheus")]
pub async fn prometheus(
    State(state): State<std::sync::Arc<crate::http::HttpState>>,
) -> (StatusCode, [(axum::http::header::HeaderName, &'static str); 1], String) {
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        state.metrics_handle.render(),
    )
}

#[cfg(not(feature = "prometheus"))]
pub async fn prometheus() -> (StatusCode, &'static str) {
    (StatusCode::NOT_FOUND, "metrics disabled")
}
```

- [ ] **Step 4: Add to router**

```rust
#[cfg(feature = "prometheus")]
let router = router.route("/metrics", axum::routing::get(super::metrics::prometheus));
```

- [ ] **Step 5: Wire installation into the composition root**

In `server.rs` (or the Task 3.10 binary, which calls it): before building
`HttpState`, run `metrics::validate_no_listener_env()?` and, under
`#[cfg(feature = "prometheus")]`, `let handle = metrics::install_recorder()?;`.

- [ ] **Step 6: Test**

```rust
#[cfg(feature = "prometheus")]
#[tokio::test]
async fn metrics_route_returns_prometheus_text() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    let state = crate::http::HttpState::default_for_test().await;
    let router = crate::http::router::build_router(state);
    let resp = router.oneshot(Request::builder().uri("/metrics").header("host", "localhost").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    // A recorder with no recorded metrics still renders an empty exposition.
    assert!(std::str::from_utf8(&body).is_ok());
}
```

- [ ] **Step 7: Run**

Run:

```bash
cargo test -p memory_mcp --features streamable-http,prometheus,test-fixtures --lib http::metrics
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/memory-mcp/src/http/
git commit -m "feat(http): /metrics route gated on prometheus feature"
```

### Task 3.9: Health endpoints with registry/dependency probe

**Files:**
- Modify: `crates/memory-mcp/src/http/health.rs`

- [ ] **Step 1: Implement registry-aware ready**

```rust
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde_json::json;

use super::HttpState;

pub async fn live() -> &'static str { "ok" }

pub async fn ready(State(state): State<Arc<HttpState>>) -> (StatusCode, Json<serde_json::Value>) {
    // Spec §16: registry connectivity + admission. Not every Tenant.
    if state.shutdown.is_shutting_down() {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"status":"shutting_down"})));
    }
    if state.admission.is_closed() {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"status":"admission_closed"})));
    }
    if !state.registry.ping().await {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"status":"registry_unreachable"})));
    }
    (StatusCode::OK, Json(json!({"status":"ready"})))
}
```

`HttpState` gains three stub fields in this task. The stub types are created
HERE (with real code), because later phases build on the same names — no
forward references to files that do not exist yet.

Create `crates/memory-mcp/src/http/registry/mod.rs` and register it in
`http/mod.rs` (`pub mod registry;`):

```rust
//! Tenant Registry seam (ADR-0052). Phase 3 stub; real store in Phase 4.

/// Phase 3 stub handle. Phase 4 replaces the stub constructor with a real
/// control-namespace-backed handle; `ping` keeps this exact signature.
#[derive(Clone)]
pub struct RegistryHandle {
    stub: bool,
}

impl RegistryHandle {
    /// Phase 3 stub: always reachable. Removed in Task 4.1 when the real
    /// store lands.
    pub fn stub() -> Self {
        Self { stub: true }
    }

    pub async fn ping(&self) -> bool {
        self.stub
    }
}
```

Create `crates/memory-mcp/src/http/runtime/mod.rs` +
`crates/memory-mcp/src/http/runtime/pool.rs` and register `pub mod runtime;`:

```rust
// runtime/mod.rs
pub mod pool;

// runtime/pool.rs
//! Runtime pool. Phase 3 stub gate; real pool in Task 5.5.

/// Phase 3 stub admission gate: always open, never limits.
/// Task 5.5 replaces the internals without changing these two method names.
pub struct AdmissionGate {
    closed: std::sync::atomic::AtomicBool,
}

impl AdmissionGate {
    pub fn new() -> Self {
        Self { closed: std::sync::atomic::AtomicBool::new(false) }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Default for AdmissionGate {
    fn default() -> Self {
        Self::new()
    }
}
```

`ShutdownState` was defined in Task 3.3 and already contains the observable
flag plus cancellation token. Do not define it a second time here.

Update `HttpState` ADDITIVELY (extending the Task 3.8 shape — keep
`mcp_factory` and the cfg-gated `metrics_handle`; the constructor stays
`new_tenantless` with the cfg-gated parameter):

```rust
pub struct HttpState {
    pub config: HttpConfig,
    pub shutdown: crate::http::shutdown::ShutdownState,
    pub admission: std::sync::Arc<crate::http::runtime::pool::AdmissionGate>,
    pub registry: crate::http::registry::RegistryHandle,
    /// Phase 3 dispatch factory (Task 3.3). Removed in Task 5.6.
    pub mcp_factory: Arc<dyn Fn() -> Result<crate::mcp::handlers::MemoryMcp, std::io::Error> + Send + Sync>,
    pub request_logger: Arc<crate::logging::StdoutLogger>,
    #[cfg(feature = "prometheus")]
    pub metrics_handle: metrics_exporter_prometheus::PrometheusHandle,
}

impl HttpState {
    pub async fn new_tenantless(
        config: HttpConfig,
        #[cfg(feature = "prometheus")] metrics_handle: metrics_exporter_prometheus::PrometheusHandle,
    ) -> Result<Arc<Self>, crate::error::MemoryError> {
        let mcp = transport::build_tenantless_handler(&config).await?;
        Ok(Arc::new(Self {
            config,
            shutdown: crate::http::shutdown::ShutdownState::new(),
            admission: std::sync::Arc::new(crate::http::runtime::pool::AdmissionGate::new()),
            registry: crate::http::registry::RegistryHandle::stub(),
            mcp_factory: Arc::new(move || Ok((*mcp).clone())),
            request_logger: Arc::new(crate::logging::StdoutLogger::new("info")),
            #[cfg(feature = "prometheus")]
            metrics_handle,
        }))
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
impl HttpState {
    pub async fn default_for_test() -> Arc<Self> {
        let config = HttpConfig::default_for_test();
        #[cfg(feature = "prometheus")]
        {
            Self::new_tenantless(config, test_metrics_handle())
                .await
                .expect("tenantless test state builds")
        }
        #[cfg(not(feature = "prometheus"))]
        {
            Self::new_tenantless(config)
                .await
                .expect("tenantless test state builds")
        }
    }
}
```

Phase 4+ replace `RegistryHandle::stub()` with a real probe. Until then,
`ping()` returns `true` and the ready endpoint passes.

Because `ShutdownState` is now per `HttpState`, update the transport seam in
this task: change `build_server_config(http)` to
`build_server_config(http, cancellation_token)` and pass
`state.shutdown.token()` from `mcp_handler`. Remove the process-global
`cancellation_token()` use from `build_server_config`; retaining a global
cancelled token would make later test/application instances stop immediately.
The initial Phase 3 constructor may keep the helper until this task, but no
final code may share shutdown tokens between `HttpState` instances.

`health::ready` now extracts `State<Arc<HttpState>>`; `build_router` already
supplies it via `.with_state(state)` (Task 3.3), so NO router change and no
test-router twin is needed for this task.

- [ ] **Step 2: Test**

```rust
#[tokio::test]
async fn ready_returns_ok_when_registry_reachable() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    let router = crate::http::router::build_router(crate::http::HttpState::default_for_test().await);
    let resp = router.oneshot(Request::builder().uri("/health/ready").header("host","localhost").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
```

- [ ] **Step 3: Run**

Run:

```bash
cargo test -p memory_mcp --features streamable-http --lib http::health
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/memory-mcp/src/http/
git commit -m "feat(http): /health/ready with registry probe"
```

### Task 3.10: HTTP binary entry point

**Files:**
- Create: `crates/memory-mcp/src/http/logging.rs`
- Create: `crates/memory-mcp/src/bin/memory_mcp_http.rs`
- Modify: `crates/memory-mcp/src/http/mod.rs` (register `pub mod logging;`)
- Modify: `crates/memory-mcp/src/http/router.rs` (install request logger)

- [ ] **Step 0: Add structured request logging**

The logger must not record URI paths, headers, bodies, credentials, namespace
names, email, or memory content. It records only bounded categories and a
correlation ID. Add `http/logging.rs`:

```rust
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Instant;

#[derive(Clone)]
pub struct TenantLogContext {
    pub fingerprint: String,
}

pub async fn request_log(
    State(state): State<std::sync::Arc<crate::http::HttpState>>,
    mut req: Request,
    next: Next,
) -> Response {
    let started = Instant::now();
    let request_id = uuid::Uuid::new_v4().to_string();
    req.extensions_mut().insert(request_id.clone());
    let credential_kind = if req.headers().contains_key("authorization") {
        "bearer"
    } else if req.headers().contains_key("cookie") {
        "cookie"
    } else {
        "none"
    };
    let response = next.run(req).await;
    let tenant_fingerprint = response
        .extensions()
        .get::<TenantLogContext>()
        .map(|ctx| ctx.fingerprint.as_str())
        .unwrap_or("none");
    state.request_logger.log(
        HashMap::from([
            ("event".into(), Value::from("http_request")),
            ("request_id".into(), Value::from(request_id)),
            ("method_category".into(), Value::from("http")),
            ("credential_kind".into(), Value::from(credential_kind)),
            ("outcome".into(), Value::from(response.status().as_u16() / 100)),
            ("latency_ms".into(), Value::from(started.elapsed().as_millis() as u64)),
            ("tenant_fingerprint".into(), Value::from(tenant_fingerprint)),
        ]),
        crate::logging::LogLevel::Info,
    );
    response
}
```

`TenantLogContext` is copied by the runtime-acquisition middleware from the
request extension to the response extension before it returns (Task 5.6),
which lets this outer logger observe it without logging raw request data. The
important invariant is that the logger emits only a bounded pseudonymous
fingerprint, never an identifier or raw request data.

Wire it as the outermost layer in `build_router`:

```rust
.layer(axum::middleware::from_fn_with_state(
    state.clone(),
    super::logging::request_log,
))
```

- [ ] **Step 1: Implement thin entry**

The crate's logging is `StdoutLogger` (`src/logging.rs`) — there is no
`logging::init()`. rmcp/axum emit via `tracing`; the HTTP profile relies on
its own structured request log (Phase 3 `logging.rs` module arrives with the
request-log task), so the binary installs a `tracing_subscriber` fmt layer
ONLY if `tracing-subscriber` is present in deps — it is not today, so the
binary uses `StdoutLogger` for its own lines and leaves `tracing` events
uncollected (acceptable for v1; documented in `docs/operations/LIMITATIONS.md`,
Task 12.8).

```rust
use std::process::ExitCode;

use memory_mcp::http::config::HttpConfig;
use memory_mcp::http::HttpState;
use memory_mcp::logging::StdoutLogger;

#[tokio::main]
async fn main() -> ExitCode {
    let logger = StdoutLogger::new("info");
    let cfg = match HttpConfig::from_env() {
        Ok(c) => c,
        Err(err) => {
            eprintln!("config error: {err}");
            return ExitCode::from(2);
        }
    };
    if let Err(err) = cfg.validate() {
        eprintln!("config invalid: {err}");
        return ExitCode::from(2);
    }

    // Metrics surface (ADR-0048): recorder + /metrics route.
    #[cfg(feature = "prometheus")]
    let state = {
        if let Err(err) = memory_mcp::http::metrics::validate_no_listener_env() {
            eprintln!("config invalid: {err}");
            return ExitCode::from(2);
        }
        let handle = match memory_mcp::http::metrics::install_recorder() {
            Ok(handle) => handle,
            Err(err) => {
                eprintln!("metrics init error: {err}");
                return ExitCode::from(2);
            }
        };
        match HttpState::new_tenantless(cfg.clone(), handle).await {
            Ok(state) => state,
            Err(err) => {
                eprintln!("tenant runtime init error: {err}");
                return ExitCode::from(2);
            }
        }
    };
    #[cfg(not(feature = "prometheus"))]
    let state = match HttpState::new_tenantless(cfg.clone()).await {
        Ok(state) => state,
        Err(err) => {
            eprintln!("tenant runtime init error: {err}");
            return ExitCode::from(2);
        }
    };

    let shutdown = state.shutdown.clone();
    let signal_shutdown = shutdown.clone();
    let admission = state.admission.clone();
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut terminate = signal(SignalKind::terminate()).ok();
            match terminate.as_mut() {
                Some(terminate) => {
                    tokio::select! {
                        _ = tokio::signal::ctrl_c() => {}
                        _ = terminate.recv() => {}
                    }
                }
                None => {
                    let _ = tokio::signal::ctrl_c().await;
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
        admission.close();
        signal_shutdown.begin();
    });
    let router = memory_mcp::http::router::build_router(state);
    logger.log(
        std::collections::HashMap::from([
            ("event".to_string(), serde_json::Value::from("http_start")),
            ("profile".to_string(), serde_json::Value::from("streamable_http_saas")),
            ("bind".to_string(), serde_json::Value::from(cfg.bind.to_string())),
            ("control_plane".to_string(), serde_json::Value::from(cfg.enable_control_plane)),
            ("embedded_tenant_db".to_string(), serde_json::Value::from(cfg.tenant_db.url == "mem://")),
        ]),
        memory_mcp::logging::LogLevel::Info,
    );
    if let Err(err) = memory_mcp::http::server::serve(cfg, router, shutdown).await {
        eprintln!("server error: {err}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
```

(If `Logger::log`'s exact signature differs at implementation time — it takes
`(event: HashMap<String, Value>, level: LogLevel)` per `src/logging.rs` —
match the source; the shape above mirrors it.)

- [ ] **Step 2: Compile and run**

Run (literal values — no shell brace expansion, so this works in plain `sh`):

```bash
cargo build -p memory_mcp --features streamable-http --bin memory_mcp_http
ALLOWED_HOSTS=localhost \
  MEMORY_MCP_API_KEY_PEPPER=xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx \
  MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY=0000000000000000000000000000000000000000000000000000000000000000 \
  MEMORY_MCP_HTTP_SIGNUP_MODE=invite_only \
  MEMORY_MCP_HTTP_PUBLIC_BASE_URL=http://localhost \
  MEMORY_MCP_HTTP_BIND=127.0.0.1:18080 \
  MEMORY_MCP_HTTP_CSRF_KEY=0000000000000000000000000000000000000000000000000000000000000000 \
  MEMORY_MCP_HTTP_OIDC_STATE_KEY=0000000000000000000000000000000000000000000000000000000000000000 \
  MEMORY_MCP_HTTP_OIDC_NONCE_KEY=0000000000000000000000000000000000000000000000000000000000000000 \
  MEMORY_MCP_HTTP_SESSION_KEY=0000000000000000000000000000000000000000000000000000000000000000 \
  SURREALDB_CONTROL_URL=mem:// SURREALDB_CONTROL_USERNAME=root SURREALDB_CONTROL_PASSWORD=root \
  SURREALDB_CONTROL_DB=control SURREALDB_CONTROL_NAMESPACE=control \
  SURREALDB_TENANT_URL=mem:// SURREALDB_TENANT_USERNAME=root SURREALDB_TENANT_PASSWORD=root \
  SURREALDB_TENANT_DB=tenant SURREALDB_TENANT_NAMESPACE=tenant \
  MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE=false MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI=false \
  ./target/debug/memory_mcp_http &
SERVER_PID=$!
sleep 1
curl -sS -i http://127.0.0.1:18080/health/live
kill $SERVER_PID
```

Expected: `HTTP/1.1 200 OK` with `ok`. `new_tenantless` connects to the
tenant target at startup (Task 3.3): `mem://` selects the embedded in-memory
engine, so this smoke run needs no external SurrealDB. The control target is
not connected in Phase 3 (the registry lands in Phase 4); keeping it `mem://`
here makes later-phase smoke runs self-contained too.

- [ ] **Step 3: Commit**

```bash
git add crates/memory-mcp/src/bin/memory_mcp_http.rs
git commit -m "feat(http): memory_mcp_http binary entry point"
```

### Task 3.11: Black-box protocol conformance suite

**Files:**
- Create: `crates/memory-mcp/tests/http_proto_conformance.rs`

This is the spec §20.1 gate. Each test is a failing test until Phase 3 is complete, then a passing test that locks the behavior.

- [ ] **Step 1: Test fixture: spawn `memory_mcp_http`**

Use `env!("CARGO_BIN_EXE_memory_mcp_http")` — cargo sets this variable for
integration tests and builds the binary automatically. The binary prints
`memory_mcp_http bound=<addr>` on stdout (Task 3.3 `server.rs`); the helper
parses that line. Full helper code — no `// ...` bodies:

```rust
//! Black-box protocol conformance for the HTTP SaaS profile (spec §20.1).
//!
//! Run: cargo test -p memory_mcp --features streamable-http,test-fixtures \
//!      --test http_proto_conformance

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::json;

struct Server {
    child: Child,
    base_url: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Pick a free port (bind + drop). There is an inherent race between dropping
/// and the server re-binding; it is acceptable for tests and standard practice.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    listener.local_addr().expect("local addr").port()
}

fn base_env(port: u16) -> Vec<(String, String)> {
    let zeros = "0".repeat(64);
    vec![
        ("MEMORY_MCP_HTTP_BIND".into(), format!("127.0.0.1:{port}")),
        ("MEMORY_MCP_HTTP_PUBLIC_BASE_URL".into(), "http://localhost".into()),
        ("ALLOWED_HOSTS".into(), "localhost,127.0.0.1".into()),
        ("ALLOWED_ORIGINS".into(), "http://localhost".into()),
        ("MEMORY_MCP_API_KEY_PEPPER".into(), "x".repeat(40)),
        ("MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY".into(), zeros.clone()),
        ("MEMORY_MCP_HTTP_SIGNUP_MODE".into(), "invite_only".into()),
        ("MEMORY_MCP_HTTP_CSRF_KEY".into(), zeros.clone()),
        ("MEMORY_MCP_HTTP_OIDC_STATE_KEY".into(), zeros.clone()),
        ("MEMORY_MCP_HTTP_OIDC_NONCE_KEY".into(), zeros.clone()),
        ("MEMORY_MCP_HTTP_SESSION_KEY".into(), zeros),
        ("SURREALDB_CONTROL_URL".into(), "mem://".into()),
        ("SURREALDB_CONTROL_USERNAME".into(), "root".into()),
        ("SURREALDB_CONTROL_PASSWORD".into(), "root".into()),
        ("SURREALDB_CONTROL_DB".into(), "control".into()),
        ("SURREALDB_CONTROL_NAMESPACE".into(), "control".into()),
        ("SURREALDB_TENANT_URL".into(), "mem://".into()),
        ("SURREALDB_TENANT_USERNAME".into(), "root".into()),
        ("SURREALDB_TENANT_PASSWORD".into(), "root".into()),
        ("SURREALDB_TENANT_DB".into(), "tenant".into()),
        ("SURREALDB_TENANT_NAMESPACE".into(), "tenant".into()),
        ("MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE".into(), "false".into()),
        ("MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI".into(), "false".into()),
    ]
}

/// Spawn the server with extra env overrides and wait for the bound line.
async fn spawn_server(extra_env: &[(&str, &str)]) -> Server {
    let port = free_port();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_memory_mcp_http"));
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in base_env(port) {
        cmd.env(k, v);
    }
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn memory_mcp_http");
    let stdout = child.stdout.take().expect("stdout piped");
    let bound_line = tokio::task::spawn_blocking(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let line = line.expect("read stdout");
            if line.starts_with("memory_mcp_http bound=") {
                return line;
            }
        }
        panic!("server exited before printing bound line");
    })
    .await
    .expect("join");
    let addr = bound_line
        .trim_start_matches("memory_mcp_http bound=")
        .to_string();
    Server {
        child,
        base_url: format!("http://{addr}"),
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("client")
}

/// Modern per-request metadata required by stateless_protocol_metadata_required.
fn modern_meta() -> serde_json::Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {
            "name": "memory-mcp-conformance",
            "version": "0.0.0",
        },
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}
```

- [ ] **Step 2: Add tests (full bodies)**

```rust
#[tokio::test]
async fn get_on_mcp_returns_405() {
    let server = spawn_server(&[]).await;
    let resp = client()
        .get(format!("{}/mcp", server.base_url))
        .header("host", "localhost")
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 405);
}

#[tokio::test]
async fn delete_on_mcp_returns_405() {
    let server = spawn_server(&[]).await;
    let resp = client()
        .delete(format!("{}/mcp", server.base_url))
        .header("host", "localhost")
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 405);
}

#[tokio::test]
async fn server_discover_advertises_only_2026_07_28() {
    let server = spawn_server(&[]).await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": {},
        "_meta": modern_meta(),
    });
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "server/discover")
        .body(body.to_string())
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 200);
    let text = resp.text().await.expect("body");
    assert!(text.contains("\"2026-07-28\""), "discover must advertise 2026-07-28: {text}");
    for legacy in ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"] {
        assert!(!text.contains(legacy), "legacy version {legacy} advertised: {text}");
    }
}

#[tokio::test]
async fn header_body_mismatch_returns_header_mismatch_error() {
    // rmcp 3.1.2: ErrorCode::HEADER_MISMATCH = -32020
    // (verified in rmcp src/model.rs). The Mcp-Method header contradicts the
    // body method, so the server rejects before dispatch.
    let server = spawn_server(&[]).await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "ping",
        "_meta": modern_meta(),
    });
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "initialize")
        .body(body.to_string())
        .send()
        .await
        .expect("send");
    let status = resp.status();
    let text = resp.text().await.expect("body");
    assert!(
        status == 400 || text.contains("-32020"),
        "expected header mismatch rejection, got {status}: {text}"
    );
    assert!(text.contains("-32020") || text.to_lowercase().contains("mismatch"), "{text}");
}

#[tokio::test]
async fn tools_call_requires_matching_mcp_name() {
    let server = spawn_server(&[]).await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": { "name": "ingest", "arguments": {} },
        "_meta": modern_meta(),
    });
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .body(body.to_string())
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn removed_ping_method_is_not_available() {
    let server = spawn_server(&[]).await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "ping",
        "_meta": modern_meta(),
    });
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "ping")
        .body(body.to_string())
        .send()
        .await
        .expect("send");
    let text = resp.text().await.expect("body");
    assert!(text.contains("-32601") || text.to_ascii_lowercase().contains("method not found"), "{text}");
}

#[tokio::test]
async fn unsupported_legacy_version_returns_400() {
    // Mechanism: stateless_protocol_metadata_required rejects legacy requests
    // (they carry no per-request _meta protocol version). 2025-03-26 is a
    // KNOWN version, so the header check alone would pass it.
    let server = spawn_server(&[]).await;
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-03-26")
        .header("Mcp-Method", "ping")
        .body(body.to_string())
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn disallowed_host_returns_403() {
    let server = spawn_server(&[]).await;
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
        .header("host", "evil.example")
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 403);
}

#[tokio::test]
async fn disallowed_origin_returns_403() {
    let server = spawn_server(&[]).await;
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
        .header("host", "localhost")
        .header("origin", "https://evil.example")
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 403);
}

#[tokio::test]
async fn body_over_limit_returns_413() {
    // Shrink the limit so the test does not push 8 MiB.
    let server = spawn_server(&[("MEMORY_MCP_HTTP_BODY_LIMIT", "1024")]).await;
    let big = "a".repeat(2048);
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .body(big)
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 413);
}

#[tokio::test]
async fn missing_accept_returns_406() {
    // rmcp requires Accept to include both application/json and
    // text/event-stream on stateless POSTs.
    let server = spawn_server(&[]).await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "ping",
        "_meta": modern_meta(),
    });
    let req = client()
        .post(format!("{}/mcp", server.base_url))
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "ping")
        .body(body.to_string())
        .build()
        .expect("build");
    // reqwest inserts `accept: */*` by default; strip it.
    let resp = client().execute(req).await.expect("send");
    // If reqwest still forces an Accept header, this assertion documents the
    // client limitation and the test is adjusted to send raw bytes via hyper.
    assert_eq!(resp.status(), 406);
}

#[tokio::test]
async fn no_mcp_session_id_header_is_set() {
    let server = spawn_server(&[]).await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": {},
        "_meta": modern_meta(),
    });
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "server/discover")
        .body(body.to_string())
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 200);
    assert!(
        resp.headers().get("mcp-session-id").is_none(),
        "2026-07-28 stateless profile must never set Mcp-Session-Id"
    );
}

#[tokio::test]
async fn disconnect_does_not_wedge_the_server() {
    // Black-box proxy for "closing a request SSE stream cancels request-owned
    // work" (spec §3.2/§17): abort a request mid-flight, then prove the server
    // still serves subsequent requests.
    let server = spawn_server(&[]).await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": {},
        "_meta": modern_meta(),
    });
    let aborted = client()
        .post(format!("{}/mcp", server.base_url))
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "server/discover")
        .body(body.to_string())
        .send();
    let handle = tokio::spawn(async move {
        let resp = aborted.await.expect("send");
        drop(resp); // drop the connection immediately
    });
    let _ = tokio::time::timeout(Duration::from_millis(50), handle).await;

    // Server must remain responsive.
    let resp = client()
        .get(format!("{}/health/live", server.base_url))
        .header("host", "localhost")
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 200);
}
```

For each test, the implementation step is to actually run the test and confirm
PASS — these are documented as expected outcomes; the implementation is the
wiring done in earlier Phase 3 tasks.

- [ ] **Step 3: Verify tracing initialization in the binary**

`crates/memory-mcp/src/bin/memory_mcp_http.rs` uses `StdoutLogger` (Task 3.10).
No `tracing_subscriber` dependency exists in the crate today; do not add one
in this task. The `bound=` stdout line comes from `server::serve` (Task 3.3).

- [ ] **Step 4: Run**

Run:

```bash
cargo test -p memory_mcp --features streamable-http,test-fixtures --test http_proto_conformance
```

Expected: all PASS. Tests that depend on Phase 4+ (auth, tenancy) are added
in those phases, not here.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/tests/http_proto_conformance.rs crates/memory-mcp/src/bin/memory_mcp_http.rs
git commit -m "test(http): protocol conformance suite for Phase 3 (spec §20.1)"
```

### Task 3.12: Final lint for Phase 3

- [ ] **Step 1: Run lint gate**

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets \
  --features fs-watch,mcp-apps,streamable-http,test-fixtures --locked -- -D warnings
```

Expected: zero warnings.

- [ ] **Step 2: Run stdio regression**

```bash
cargo test -p memory_mcp --features fs-watch,mcp-apps --test service_acceptance --test tools_e2e
```

Expected: PASS.

- [ ] **Step 3: Brief summary commit if any**

Only commit if lint forced formatting changes.

---

## Phase 4: Tenant Registry, API keys, principals

This phase introduces the control namespace, registry storage, account/tenant resolution, API key parser/verifier, principal cache, and the request-scoped `AuthenticatedPrincipal` resolution. **Execution order exception:** Task 4.2 (record models) is a prerequisite for Task 4.1's trait signatures, so execute Task 4.2 before Task 4.1 even though it is listed immediately afterward.

### Task 4.1: Control storage abstraction

**Prerequisite:** `crates/memory-mcp/src/http/registry/models.rs` from Task 4.2
must be landed FIRST, because `RegistryStore` uses those concrete model types.
Although the model task is shown immediately after this section, execute its
four steps before starting Task 4.1; this preserves a compiling checkpoint.

**Files:**
- Modify: `crates/memory-mcp/src/error.rs`
- Modify: `crates/memory-mcp/src/http/registry/mod.rs`
- Create: `crates/memory-mcp/src/http/registry/storage.rs`
- Create: `crates/memory-mcp/src/http/registry/migrations.rs`
- Modify: `crates/memory-mcp/src/http/registry/models.rs` (prerequisite from Task 4.2)
- Modify: `crates/memory-mcp/src/storage/client.rs` (reuse the already planned `connect_bound`; add the local prebound twin used by embedded tests)

- [ ] **Step 0: Extend `MemoryError` with the variants Phase 4+ needs**

The current enum (`crates/memory-mcp/src/error.rs`) has: `ConfigMissing`,
`ConfigInvalid`, `Storage`, `Transient`, `NotFound`, `Validation`, `Conflict`,
`BudgetExhausted`, `ModelNotReady`. The HTTP profile needs two more. Add them
before any Phase 4 code references them (Tasks 4.3–4.5, 5.6 use `Auth`;
Tasks 6.3, 5.6 use `Unavailable`):

```rust
    /// Authentication or authorization failed (bad/unknown/revoked
    /// credential, expired key, suspended account). Never reveals which
    /// check failed to the caller.
    #[error("auth error: {0}")]
    Auth(String),

    /// The request cannot be served right now but may be later
    /// (provisioning in flight, schema incompatible, draining).
    /// Maps to HTTP 503 with retry guidance.
    #[error("unavailable: {0}")]
    Unavailable(String),
```

Add a unit test next to the existing ones in `error.rs`:

```rust
#[test]
fn auth_and_unavailable_variants_display_stable_prefixes() {
    assert!(MemoryError::Auth("x".into()).to_string().starts_with("auth error:"));
    assert!(MemoryError::Unavailable("x".into()).to_string().starts_with("unavailable:"));
}
```

Note: `is_transient_db_error` does not match the new variants — correct,
neither is retryable as a DB conflict.

- [ ] **Step 1: Register models and define the registry store trait**

At the start of this task, register all registry modules in
`http/registry/mod.rs`; otherwise later files are created but never compiled:

```rust
pub mod migrations;
pub mod models;
pub mod provisioning;
pub mod storage;

pub use storage::{RegistryStore, SurrealRegistryStore};
```

The model types are the complete definitions in Task 4.2. The trait uses the
same `TenantStatus` type for the CAS state check; there is no second
`TenantState` enum:

```rust
use chrono::{DateTime, Utc};
use super::models::*;
use crate::error::MemoryError;

#[async_trait::async_trait]
pub trait RegistryStore: Send + Sync + 'static {
    async fn ping(&self) -> bool;
    async fn find_account_by_id(&self, account_id: &str) -> Result<Option<Account>, MemoryError>;
    /// `subject_verifier` is a keyed blind index; raw OIDC `sub` is never persisted.
    async fn find_account_by_identity(&self, issuer: &str, subject_verifier: &[u8; 32]) -> Result<Option<Account>, MemoryError>;
    async fn find_tenant_by_account(&self, account_id: &str) -> Result<Option<Tenant>, MemoryError>;
    async fn find_tenant_by_id(&self, tenant_id: &str) -> Result<Option<Tenant>, MemoryError>;
    async fn find_api_key(&self, key_id: &str) -> Result<Option<ApiKey>, MemoryError>;
    async fn write_api_key(&self, key: &ApiKey) -> Result<(), MemoryError>;
    async fn list_api_keys(&self, account_id: &str) -> Result<Vec<ApiKeyMeta>, MemoryError>;
    async fn revoke_api_key(&self, account_id: &str, key_id: &str) -> Result<(), MemoryError>;
    async fn touch_api_key(&self, key_id: &str, used_at: DateTime<Utc>) -> Result<(), MemoryError>;
    async fn write_account(&self, account: &Account) -> Result<(), MemoryError>;
    async fn write_tenant(&self, tenant: &Tenant) -> Result<(), MemoryError>;
    /// CAS on both `version` and `status`; returns the new version.
    async fn update_tenant_state(
        &self,
        tenant_id: &str,
        expected_version: u64,
        expected_state: TenantStatus,
        new_state: TenantStatus,
    ) -> Result<u64, MemoryError>;
    async fn update_tenant_schema_version(
        &self,
        tenant_id: &str,
        expected_version: u64,
        schema_version: u32,
    ) -> Result<u64, MemoryError>;
    /// The fenced variants are the only methods provisioning may use after a
    /// lease is claimed. They CAS tenant version/status and the exact lease
    /// owner/id/generation in one durable update.
    async fn update_tenant_state_fenced(
        &self,
        tenant_id: &str,
        expected_version: u64,
        expected_state: TenantStatus,
        new_state: TenantStatus,
        owner_id: &str,
        lease_id: &str,
        fencing_generation: u64,
    ) -> Result<u64, MemoryError>;
    async fn update_tenant_schema_version_fenced(
        &self,
        tenant_id: &str,
        expected_version: u64,
        schema_version: u32,
        owner_id: &str,
        lease_id: &str,
        fencing_generation: u64,
    ) -> Result<u64, MemoryError>;
    /// Append a provisioning event (durable seam consumed by the Task 6.2
    /// scheduler; written by `enqueue_provisioning`, Task 4.7).
    async fn append_provisioning_event(&self, tenant_id: &str, stage: &str) -> Result<(), MemoryError>;
    async fn load_plan(&self, plan_id: &str) -> Result<Plan, MemoryError>;
    async fn increment_usage(&self, tenant_id: &str, counter: UsageCounter, delta: u64) -> Result<u64, MemoryError>;
    async fn list_due_provisioning(&self, limit: u32, now: DateTime<Utc>) -> Result<Vec<Tenant>, MemoryError>;
    /// Atomic claim: status/retry eligibility, lease expiry, and generation are
    /// checked in one UPDATE ... RETURN AFTER. `None` means another worker won.
    async fn claim_provisioning(
        &self,
        tenant_id: &str,
        owner_id: &str,
        lease_id: &str,
        now: DateTime<Utc>,
        lease_expiry: DateTime<Utc>,
    ) -> Result<Option<crate::http::leases::ProvisioningLease>, MemoryError>;
    async fn heartbeat_provisioning(
        &self,
        tenant_id: &str,
        owner_id: &str,
        lease_id: &str,
        fencing_generation: u64,
        now: DateTime<Utc>,
        lease_expiry: DateTime<Utc>,
    ) -> Result<(), MemoryError>;
    async fn release_provisioning(
        &self,
        tenant_id: &str,
        owner_id: &str,
        lease_id: &str,
        fencing_generation: u64,
    ) -> Result<(), MemoryError>;
}
```

- [ ] **Step 2: Implement `SurrealRegistryStore`**

Implement against a privileged SurrealDB credential. Use the **control**
`SurrealTargetConfig` from `HttpConfig`. The credential is a SurrealDB ROOT
credential: namespace/database DDL is not authorized by a database-scoped
`signin(Database { ... })`. `connect_bound` (Task 3.3) therefore signs in
with `surrealdb::opt::auth::Root { username, password }`, then binds the
requested namespace/database. Deployment documentation must state that this
credential can create namespaces and databases.

**Seam decision (verified against `storage/client.rs`).** The `DbClient`
trait already takes a `namespace` parameter on every method and validates it
against the client's `active_namespace` (`ensure_active_namespace`), and each
`SurrealDbClient` binds `use_ns(...).use_db(...)` once at construction. So
the registry does NOT need per-call database overloads: it gets its **own**
`SurrealDbClient` bound to the control namespace/database. The focused
`connect_bound` constructor below was ALREADY added in Task 3.3 Step 3 (to
keep Phase 3 self-contained) — verify the existing implementation matches
this contract exactly and do NOT add it again (stdio never calls it, so
stdio behavior is unchanged):

```rust
impl SurrealDbClient {
    /// Connects to a remote SurrealDB endpoint and binds the session to the
    /// given namespace/database once. Used by the HTTP profile for the
    /// control registry and for tenant provisioning (ADR-0052).
    pub async fn connect_bound(
        url: &str,
        username: &str,
        password: &str,
        namespace: &str,
        database: &str,
        log_level: &str,
    ) -> Result<Self, MemoryError> {
        let db = surrealdb::Surreal::new::<surrealdb::engine::remote::ws::Ws>(
            url,
        )
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB connect failed: {err}")))?;
        db.signin(surrealdb::opt::auth::Root {
            username: username.to_string(),
            password: password.to_string(),
        })
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB signin failed: {err}")))?;
        db.use_ns(namespace)
            .use_db(database)
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB use failed: {err}")))?;
        Ok(Self {
            engine: DbEngine::Remote(std::sync::Arc::new(db)),
            active_namespace: namespace.to_string(),
            logger: StdoutLogger::new(log_level),
            fact_embedding_dimension: crate::config::DEFAULT_EMBEDDING_DIMENSION,
        })
    }
}
```

(If the existing `connect(config)` path already exposes the same primitives,
reuse its internals; the contract above is what matters: one constructor,
bind-once, no trait changes.)

Define the concrete store before its impl:

```rust
pub struct SurrealRegistryStore {
    db: std::sync::Arc<crate::storage::client::SurrealDbClient>,
}
```

`SurrealRegistryStore` then wraps `Arc<SurrealDbClient>` and implements
`RegistryStore` with record-level CRUD via the existing trait methods,
passing the control namespace. Every write uses an explicit unique key or
CAS predicate; no raw query is exposed as an MCP tool.

Add this local pre-bound constructor to `storage/client.rs` now, because the
embedded registry and its privileged engine must share one `Mem` handle:

```rust
impl SurrealDbClient {
    /// Wrap an already-bound local SurrealDB handle. Test/development only;
    /// never used by the stdio or production remote composition roots.
    pub fn from_prebound_mem(
        db: surrealdb::Surreal<surrealdb::engine::local::Db>,
        active_namespace: &str,
        log_level: &str,
    ) -> Self {
        Self {
            engine: DbEngine::Local(std::sync::Arc::new(db)),
            active_namespace: active_namespace.to_string(),
            logger: StdoutLogger::new(log_level),
            fact_embedding_dimension: crate::config::DEFAULT_EMBEDDING_DIMENSION,
        }
    }
}
```

This is the planned change to `storage/client.rs` in Task 4.1; Task 5.3 adds
only the remote `from_prebound` twin.

- [ ] **Step 3: Test using embedded SurrealDB**

Write a test that boots `SurrealDbClient::connect_in_memory` against the control namespace, creates the registry tables (`account`, `tenant`, `api_key`, `external_identity`, `plan`, `usage_counter`, `provisioning_event`), and asserts CRUD roundtrips.

- [ ] **Step 4: Add and apply separate registry migrations**

Add `crates/memory-mcp/src/http/registry/migrations.rs` with
`versioned_registry_migrations()` mirroring `storage/migrations.rs`. The
control migrations are **separate** from tenant migrations. Tables mirror the
schema in spec §5.1. Provide the exact adapter used by
`SurrealRegistryStore::connect*`:

```rust
use crate::error::MemoryError;
use crate::storage::client::{DbClient, SurrealDbClient};

pub fn versioned_registry_migrations() -> Vec<&'static str> {
    vec![
        include_str!("../../../migrations/001_registry.surql"),
    ]
}

pub async fn apply_registry_migrations(
    client: &SurrealDbClient,
    namespace: &str,
) -> Result<(), MemoryError> {
    for sql in versioned_registry_migrations() {
        client
            .query(sql, None, namespace)
            .await
            .map(|_| ())?;
    }
    Ok(())
}
```

Create the migration file at
`crates/memory-mcp/migrations/001_registry.surql`; it defines the control
records (`account`, `external_identity`, `tenant`, `api_key`, `plan`,
`usage_counter`, `provisioning_event`) and unique indexes required by the
models. The `tenant` record must also contain these fields because the lease
and provisioning CAS must be checked on the same record:

```surql
DEFINE FIELD IF NOT EXISTS retry_stage ON tenant TYPE option<string>;
DEFINE FIELD IF NOT EXISTS lease_owner ON tenant TYPE option<string>;
DEFINE FIELD IF NOT EXISTS lease_id ON tenant TYPE option<string>;
DEFINE FIELD IF NOT EXISTS lease_expiry ON tenant TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS lease_generation ON tenant TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS heartbeat_at ON tenant TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS last_error ON tenant TYPE option<string>;
```

The filename is versioned and append-only; never edit an applied migration — add
`002_registry_leases.surql` for an already-initialized deployment and make the
startup migration runner apply both files in order.

- [ ] **Step 4.5: Replace the Phase 3 stub `RegistryHandle` with the real store**

Task 3.9 created `RegistryHandle::stub()` so `/health/ready` could compile.
Replace it now with the real handle (same file, `registry/mod.rs`). The
handle keeps its `ping()` signature (health endpoint unchanged) and gains the
accessors later phases rely on — `store_clone()` (Tasks 4.4, 4.5, 4.7) and
`tenant_engine()` (Task 5.4 runtime binding, Task 5.2 provisioning DDL):

```rust
use std::sync::Arc;

use super::storage::{RegistryStore, SurrealRegistryStore};

/// Tenant Registry handle (ADR-0052). Wraps the control-namespace store and
/// the privileged engine used for provisioning DDL and tenant binding.
#[derive(Clone)]
pub enum PrivilegedEngine {
    Remote(Arc<surrealdb::Surreal<surrealdb::engine::remote::ws::Client>>),
    Local(Arc<surrealdb::Surreal<surrealdb::engine::local::Db>>),
}

#[derive(Clone)]
pub struct RegistryHandle {
    store: Arc<dyn RegistryStore>,
    /// Privileged raw client for clone-once/bind-once tenant sessions.
    /// The local variant is deliberately available only for embedded
    /// test/development deployments; production uses `Remote`.
    tenant_engine: PrivilegedEngine,
}

impl RegistryHandle {
    pub fn new(store: Arc<dyn RegistryStore>, tenant_engine: PrivilegedEngine) -> Self {
        Self { store, tenant_engine }
    }

    pub fn store_clone(&self) -> Arc<dyn RegistryStore> {
        self.store.clone()
    }

    pub fn tenant_engine(&self) -> PrivilegedEngine {
        self.tenant_engine.clone()
    }

    pub async fn ping(&self) -> bool {
        self.store.ping().await
    }
}
```

`SurrealRegistryStore` gains the control-store connect entry point used by
`HttpState::new_tenantless` below. Add `use super::migrations;` in
`registry/storage.rs` so the calls below resolve. The store is connected from
`control_db`; the separate `PrivilegedEngine` used for tenant namespace DDL
and runtime binding is connected from `tenant_db`. This prevents an accidental
assumption that the two configured endpoints are the same deployment. The
embedded control store and embedded tenant engine are intentionally separate
logical Mem instances. Add the small
`SurrealDbClient::from_prebound_mem` constructor in this task (the remote
`from_prebound` remains a Task 5.3 dependency):

```rust
impl SurrealRegistryStore {
    /// Connect to the control target and apply the registry migrations
    /// (Step 4). Returns only the control registry store.
    pub async fn connect(
        cfg: &super::super::config::SurrealTargetConfig,
    ) -> Result<Arc<dyn RegistryStore>, MemoryError> {
        let client = crate::storage::client::SurrealDbClient::connect_bound(
            &cfg.url,
            &cfg.username,
            &cfg.password,
            &cfg.namespace,
            &cfg.database,
            "warn",
        )
        .await?;
        migrations::apply_registry_migrations(&client, &cfg.namespace).await?;
        Ok(Arc::new(Self { db: Arc::new(client) }))
    }

    /// In-memory control store for tests (`control_db.url == "mem://"`).
    pub async fn connect_in_memory(
        cfg: &super::super::config::SurrealTargetConfig,
    ) -> Result<Arc<dyn RegistryStore>, MemoryError> {
        let raw = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .map_err(|err| MemoryError::Storage(format!("control memory init failed: {err}")))?;
        raw.use_ns(&cfg.namespace)
            .use_db(&cfg.database)
            .await
            .map_err(|err| MemoryError::Storage(format!("control memory bind failed: {err}")))?;
        let client = crate::storage::client::SurrealDbClient::from_prebound_mem(
            raw.clone(),
            &cfg.namespace,
            "warn",
        );
        migrations::apply_registry_migrations(&client, &cfg.namespace).await?;
        Ok(Arc::new(Self { db: Arc::new(client) }))
    }

    /// Connect the privileged tenant engine from the tenant target. This is
    /// separate from the control store connection because deployments may use
    /// different SurrealDB endpoints. Root authentication is required for
    /// namespace/database DDL.
    pub async fn connect_privileged(
        cfg: &super::super::config::SurrealTargetConfig,
    ) -> Result<super::PrivilegedEngine, MemoryError> {
        if cfg.url == "mem://" {
            let raw = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
                .await
                .map_err(|err| MemoryError::Storage(format!("tenant memory init failed: {err}")))?;
            raw.use_ns(&cfg.namespace)
                .use_db(&cfg.database)
                .await
                .map_err(|err| MemoryError::Storage(format!("tenant memory bind failed: {err}")))?;
            return Ok(super::PrivilegedEngine::Local(Arc::new(raw)));
        }
        let raw = surrealdb::Surreal::new::<surrealdb::engine::remote::ws::Ws>(&cfg.url)
            .await
            .map_err(|err| MemoryError::Storage(format!("tenant connect failed: {err}")))?;
        raw.signin(surrealdb::opt::auth::Root {
            username: cfg.username.to_string(),
            password: cfg.password.to_string(),
        })
        .await
        .map_err(|err| MemoryError::Storage(format!("tenant signin failed: {err}")))?;
        raw.use_ns(&cfg.namespace)
            .use_db(&cfg.database)
            .await
            .map_err(|err| MemoryError::Storage(format!("tenant bind failed: {err}")))?;
        Ok(super::PrivilegedEngine::Remote(Arc::new(raw)))
    }
}
```

Update `HttpState::new_tenantless` (Task 3.9 shape — still async, still with
the cfg-gated metrics parameter; `mcp_factory` stays until Task 5.6): before
the `Ok(Arc::new(Self { ... }))` expression, build the real registry and
replace `registry: RegistryHandle::stub()` with the `registry` variable:

```rust
let store = if config.control_db.url == "mem://" {
    SurrealRegistryStore::connect_in_memory(&config.control_db).await?
} else {
    SurrealRegistryStore::connect(&config.control_db).await?
};
let engine = SurrealRegistryStore::connect_privileged(&config.tenant_db).await?;
let registry = RegistryHandle::new(store, engine);
```

Use that `registry` local in the constructor's final `Self` literal. Do not
leave `RegistryHandle::stub()` in the constructor after this task:

```rust
Ok(Arc::new(Self {
    config,
    shutdown: crate::http::shutdown::ShutdownState::new(),
    admission: std::sync::Arc::new(crate::http::runtime::pool::AdmissionGate::new()),
    registry,
    mcp_factory: Arc::new(move || Ok((*mcp).clone())),
    request_logger: Arc::new(crate::logging::StdoutLogger::new("info")),
    request_logger: Arc::new(crate::logging::StdoutLogger::new("info")),
    #[cfg(feature = "prometheus")]
    metrics_handle,
}))
```

`default_for_test` needs no signature change: `HttpConfig::default_for_test()`
carries `control_db.url == "mem://"` and `tenant_db.url == "mem://"`, so the
control store and tenant privileged engine are embedded automatically. From
this task on, the binary connects to both configured targets at startup (Task
3.10 smoke already uses `mem://` for both).

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/http/registry/ crates/memory-mcp/src/storage/client.rs
git commit -m "feat(registry): control storage abstraction + separate migrations"
```

### Task 4.2: Registry record models

**Files:**
- Create: `crates/memory-mcp/src/http/registry/models.rs`

- [ ] **Step 1: Define types**

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalIdentity {
    pub id: String,
    pub issuer: String,
    /// HMAC(identity_index_key, normalized_issuer || ":" || subject).
    pub subject_verifier: [u8; 32],
    pub account_id: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub status: AccountStatus,
    pub tenant_id: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus { Active, Suspended, Deleting }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tenant {
    pub id: String,
    pub status: TenantStatus,
    pub namespace_binding: NamespaceBinding,
    pub plan_version: u32,
    pub schema_version: u32,
    /// Stage to resume after a retryable failure; never inferred from a lease.
    pub retry_stage: Option<TenantStatus>,
    /// Durable snapshot of the currently claimed provisioning fence.
    pub provisioning_lease: Option<ProvisioningLeaseState>,
    pub created_at: DateTime<Utc>,
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespaceBinding {
    pub namespace: String, // server-generated, opaque, immutable
    pub database: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvisioningLeaseState {
    pub owner_id: String,
    pub lease_id: String,
    pub expires_at: DateTime<Utc>,
    pub fencing_generation: u64,
    pub heartbeat_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TenantStatus {
    Reserved, NamespaceCreating, Migrating, Ready, Suspended,
    Failed, Deleting, Purged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: String, // public, opaque
    pub account_id: String,
    pub name: String,
    pub verifier: KeyedVerifier, // HMAC over secret + pepper
    pub status: ApiKeyStatus,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub version: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeyStatus { Active, Revoked }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyMeta {
    pub id: String,
    pub name: String,
    pub status: ApiKeyStatus,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlPlaneSession {
    pub id: String,
    pub cookie_hash: [u8; 32],     // keyed HMAC; raw cookie is never persisted
    pub account_id: String,
    pub auth_time: DateTime<Utc>,
    pub idle_expiry: DateTime<Utc>,
    pub absolute_expiry: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub version: u32,
    pub limits: PlanLimits,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlanLimits {
    pub max_ingested_bytes: u64,
    pub max_episode_count: u64,
    pub max_open_app_sessions: u32,
    pub max_active_api_keys: u32,
    pub per_tenant_request_concurrency: u32,
    pub extraction_concurrency: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum UsageCounter { IngestedBytes, EpisodeCount, OpenAppSessions, ActiveApiKeys }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyedVerifier(pub [u8; 32]); // HMAC-SHA256(pepper, secret)
```

- [ ] **Step 2: Add opaque-id helpers**

```rust
pub fn new_account_id() -> String { format!("acct_{}", uuid::Uuid::new_v4()) }
pub fn new_tenant_id() -> String { format!("ten_{}", uuid::Uuid::new_v4()) }
pub fn new_api_key_id() -> String { format!("ak_{}", uuid::Uuid::new_v4()) }
pub fn new_external_identity_id() -> String { format!("idn_{}", uuid::Uuid::new_v4()) }
pub fn new_namespace_name() -> String { format!("tns_{}", uuid::Uuid::new_v4().simple()) }
```

- [ ] **Step 3: Tests**

```rust
#[test] fn ids_have_expected_prefixes() { /* ... */ }
#[test] fn tenant_version_round_trips() { /* ... */ }
```

- [ ] **Step 4: Commit**

```bash
git add crates/memory-mcp/src/http/registry/models.rs
git commit -m "feat(registry): account/tenant/api-key record models"
```

### Task 4.3: API key parser

**Files:**
- Create: `crates/memory-mcp/src/http/principal/mod.rs`
- Create: `crates/memory-mcp/src/http/principal/api_keys.rs`
- Modify: `crates/memory-mcp/src/http/mod.rs` (register `pub mod principal;`)

At the start of this task, register the file so its tests compile:

```rust
// http/principal/mod.rs
pub mod api_keys;
```

- [ ] **Step 1: Write parser tests**

The structured key format is `mem_sk_<key_id>_<secret>` where
`key_id = ak_<uuid v4>` (see `new_api_key_id()` in Task 4.2). Splitting on
`_` yields `["mem", "sk", "ak", "<uuid>", <secret parts>...]` — the parser
must reassemble `key_id` from the `ak` marker + uuid part, and the secret may
itself contain `_`.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn well_formed_key() -> String {
        // 36-char uuid v4 shape + 40-char urlsafe secret
        "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_Ab3defghij0123456789Ab3defghij0123456789".to_string()
    }

    #[test]
    fn parses_well_formed_key() {
        let key = ApiKeyCredential::parse(&well_formed_key()).unwrap();
        assert_eq!(key.key_id(), "ak_01234567-89ab-4cde-8f01-23456789abcd");
        assert_eq!(key.secret().len(), 40);
    }

    #[test]
    fn secret_may_contain_underscores() {
        let raw = "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_secret_with_underscores_and_32_plus_chars";
        let key = ApiKeyCredential::parse(raw).unwrap();
        assert_eq!(
            std::str::from_utf8(key.secret()).unwrap(),
            "secret_with_underscores_and_32_plus_chars"
        );
    }

    #[test]
    fn rejects_wrong_prefix() {
        assert!(ApiKeyCredential::parse("sk_mem_xxx").is_err());
        assert!(ApiKeyCredential::parse("mem_sk_").is_err());
        assert!(ApiKeyCredential::parse("mem_sk_onlyone").is_err());
        assert!(ApiKeyCredential::parse("mem_tk_ak_01234567-89ab-4cde-8f01-23456789abcd_abcdefabcdefabcdefabcdefabcdefabcd").is_err());
        assert!(ApiKeyCredential::parse("mem_sk_xx_01234567-89ab-4cde-8f01-23456789abcd_abcdefabcdefabcdefabcdefabcdefabcd").is_err());
    }

    #[test]
    fn rejects_over_max_length() {
        let s = "a".repeat(1024);
        assert!(ApiKeyCredential::parse(&format!("mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_{s}")).is_err());
    }

    #[test]
    fn rejects_non_urlsafe_characters() {
        assert!(ApiKeyCredential::parse("mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_secret with space padding padding").is_err());
    }

    #[test]
    fn rejects_short_secret() {
        assert!(ApiKeyCredential::parse("mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_tooshort").is_err());
    }

    #[test]
    fn rejects_missing_secret() {
        assert!(ApiKeyCredential::parse("mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd").is_err());
    }

    #[test]
    fn constant_time_eq_for_secrets() {
        let a = ApiKeyCredential::parse(&well_formed_key()).unwrap();
        let b = ApiKeyCredential::parse(&well_formed_key()).unwrap();
        let c = ApiKeyCredential::parse("mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_Bb3defghij0123456789Ab3defghij0123456789").unwrap();
        assert!(a.constant_time_eq(&b));
        assert!(!a.constant_time_eq(&c));
    }
}
```

- [ ] **Step 2: Implement parser**

```rust
use subtle::ConstantTimeEq;

use crate::error::MemoryError;

const MAX_LEN: usize = 200;
const MIN_SECRET_LEN: usize = 32;

#[derive(Debug, Clone)]
pub struct ApiKeyCredential {
    key_id: String,
    secret: Vec<u8>,
}

impl ApiKeyCredential {
    pub fn parse(raw: &str) -> Result<Self, MemoryError> {
        if raw.len() > MAX_LEN {
            return Err(MemoryError::Auth("api key length".into()));
        }
        let mut parts = raw.split('_');
        let prefix = parts.next().ok_or_else(|| MemoryError::Auth("api key prefix".into()))?;
        let kind = parts.next().ok_or_else(|| MemoryError::Auth("api key kind".into()))?;
        let marker = parts.next().ok_or_else(|| MemoryError::Auth("api key marker".into()))?;
        if prefix != "mem" || kind != "sk" || marker != "ak" {
            return Err(MemoryError::Auth("api key prefix".into()));
        }
        let uuid_part = parts.next().ok_or_else(|| MemoryError::Auth("api key id".into()))?;
        // uuid v4 canonical shape: 8-4-4-4-12 hex digits with hyphens.
        if !is_canonical_uuid(uuid_part) {
            return Err(MemoryError::Auth("api key id shape".into()));
        }
        let key_id = format!("ak_{uuid_part}");
        // The secret is everything remaining; it may contain underscores.
        let secret: String = parts.collect::<Vec<_>>().join("_");
        if secret.len() < MIN_SECRET_LEN
            || !secret.chars().all(|c| matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_'))
        {
            return Err(MemoryError::Auth("api key secret".into()));
        }
        Ok(Self { key_id, secret: secret.into_bytes() })
    }

    pub fn key_id(&self) -> &str { &self.key_id }
    pub fn secret(&self) -> &[u8] { &self.secret }
    pub fn constant_time_eq(&self, other: &Self) -> bool {
        self.secret.ct_eq(&other.secret).into()
    }
}

fn is_canonical_uuid(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (i, &b) in bytes.iter().enumerate() {
        let is_hyphen_pos = i == 8 || i == 13 || i == 18 || i == 23;
        if is_hyphen_pos {
            if b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}
```

`subtle` and `hmac` were added to the Phase 2 proposal (Task 2.1) and are
approved together with it — no mid-plan dependency additions.

- [ ] **Step 3: Add `KeyedVerifier::verify`**

In `models.rs` (or a sub-module):

```rust
impl KeyedVerifier {
    pub fn compute(pepper: &[u8], secret: &[u8]) -> Self {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        // HMAC-SHA256 accepts a key of any length, so `new_from_slice` cannot
        // fail here; the expect documents that invariant rather than hiding a
        // real error path.
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(pepper)
            .expect("HMAC-SHA256 accepts any key length");
        mac.update(secret);
        Self(mac.finalize().into_bytes().into())
    }

    pub fn verify(&self, pepper: &[u8], secret: &[u8]) -> bool {
        let expected = Self::compute(pepper, secret).0;
        expected.ct_eq(&self.0).into()
    }
}
```

`hmac`, `sha2`, and `subtle` are all declared in the Phase 2 proposal
(`sha2` is an existing workspace dep; `hmac` and `subtle` were added to the
proposal in the audit pass).

- [ ] **Step 4: Run tests**

Run:

```bash
cargo test -p memory_mcp --features streamable-http --lib http::principal::api_keys
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/http/principal/api_keys.rs crates/memory-mcp/src/http/registry/models.rs
git commit -m "feat(principal): strict API key parser + keyed verifier"
```

### Task 4.4: Authenticated principal cache

**Files:**
- Create: `crates/memory-mcp/src/http/principal/cache.rs`
- Modify: `crates/memory-mcp/src/http/principal/mod.rs` (add `cache` and `auth`)
- Create: `crates/memory-mcp/src/http/principal/auth.rs`

- [ ] **Step 1: Implement LRU cache**

Use `std::sync::Mutex` — the critical sections are LRU get/put only, and the
workspace does not depend on `parking_lot` (adding it just for this would
violate the dependency gate):

```rust
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lru::LruCache;

use super::auth::AuthDecision;
use super::super::registry::models::Account;

const POSITIVE_TTL: Duration = Duration::from_secs(60);
const NEGATIVE_TTL: Duration = Duration::from_secs(5);

pub struct CachedPrincipal {
    pub account: Arc<Account>,
    pub verifier: super::super::registry::models::KeyedVerifier,
}

pub struct PrincipalCache {
    // Cache the verifier together with the account. Caching only Account by
    // key id would accept any secret for a known key id until TTL expiry.
    positive: Mutex<LruCache<String, (Arc<CachedPrincipal>, std::time::Instant)>>,
    negative: Mutex<LruCache<String, std::time::Instant>>,
}

impl PrincipalCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            positive: Mutex::new(LruCache::new(
                std::num::NonZeroUsize::new(capacity).expect("capacity is a non-zero constant"),
            )),
            negative: Mutex::new(LruCache::new(
                std::num::NonZeroUsize::new(capacity).expect("capacity is a non-zero constant"),
            )),
        }
    }

    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        // A poisoned guard still protects the data; recover instead of panic.
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get_positive(&self, key_id: &str) -> Option<Arc<CachedPrincipal>> {
        let mut g = Self::lock(&self.positive);
        let v = g.get(key_id)?;
        if v.1.elapsed() > POSITIVE_TTL { g.pop(key_id); None } else { Some(v.0.clone()) }
    }
    pub fn put_positive(
        &self,
        key_id: String,
        account: Arc<Account>,
        verifier: super::super::registry::models::KeyedVerifier,
    ) {
        let cached = Arc::new(CachedPrincipal { account, verifier });
        Self::lock(&self.positive).put(key_id, (cached, std::time::Instant::now()));
    }
    pub fn get_negative(&self, key_id: &str) -> bool {
        let mut g = Self::lock(&self.negative);
        match g.get(key_id) {
            Some(t) if t.elapsed() > NEGATIVE_TTL => { g.pop(key_id); false }
            Some(_) => true,
            None => false,
        }
    }
    pub fn put_negative(&self, key_id: String) {
        Self::lock(&self.negative).put(key_id, std::time::Instant::now());
    }
    pub fn invalidate(&self, key_id: &str) {
        Self::lock(&self.positive).pop(key_id);
        Self::lock(&self.negative).pop(key_id);
    }
}
```

(`lru::LruCache::new` takes `NonZeroUsize`; the capacity is a compile-time
constant, so the expect documents an invariant rather than a runtime risk.)

- [ ] **Step 2: Implement `RateLimiter` then `authenticate`**

`RateLimiter` was referenced by earlier drafts but never defined — define it
here, in `principal/auth.rs`. Fixed-window per key id, bounded memory via
LRU (spec §12):

```rust
use std::sync::Mutex;
use std::time::{Duration, Instant};

use lru::LruCache;

/// Fixed-window rate limiter keyed by API key id. Bounded to `capacity`
/// tracked keys; evicted keys simply start a fresh window.
pub struct RateLimiter {
    window: Duration,
    max_per_window: u32,
    windows: Mutex<LruCache<String, (Instant, u32)>>,
}

impl RateLimiter {
    pub fn new(capacity: usize, window: Duration, max_per_window: u32) -> Self {
        Self {
            window,
            max_per_window,
            windows: Mutex::new(LruCache::new(
                std::num::NonZeroUsize::new(capacity).expect("capacity is a non-zero constant"),
            )),
        }
    }

    pub fn allow(&self, key_id: &str) -> bool {
        let mut g = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        match g.get_mut(key_id) {
            Some((start, count)) if now.duration_since(*start) < self.window => {
                if *count >= self.max_per_window {
                    return false;
                }
                *count += 1;
                true
            }
            _ => {
                g.put(key_id.to_string(), (now, 1));
                true
            }
        }
    }
}
```

Then the authenticator:

```rust
// principal/auth.rs
use std::sync::Arc;

use super::api_keys::ApiKeyCredential;
use super::cache::PrincipalCache;
use super::AuthenticatedPrincipal;
use super::super::registry::models::{Account, AccountStatus, ApiKeyStatus};
use super::super::registry::RegistryStore;

pub enum AuthDecision {
    Allow(AuthenticatedPrincipal),
    Deny,
    NotApplicable,
}

pub struct Authenticator {
    store: Arc<dyn RegistryStore>,
    cache: Arc<PrincipalCache>,
    pepper: Arc<Vec<u8>>,
    rate_limiter: Arc<RateLimiter>,
}

impl Authenticator {
    pub fn new(
        store: Arc<dyn RegistryStore>,
        cache: Arc<PrincipalCache>,
        pepper: Vec<u8>,
        rate_limiter: Arc<RateLimiter>,
    ) -> Self {
        Self { store, cache, pepper: Arc::new(pepper), rate_limiter }
    }

    pub async fn authenticate_bearer(&self, header: &str) -> AuthDecision {
        let cred = match ApiKeyCredential::parse(header) {
            Ok(c) => c,
            Err(_) => return AuthDecision::Deny,
        };
        if self.cache.get_negative(cred.key_id()) { return AuthDecision::Deny; }
        if !self.rate_limiter.allow(cred.key_id()) { return AuthDecision::Deny; }
        if let Some(cached) = self.cache.get_positive(cred.key_id()) {
            // Preserve the ≤60s revocation bound without weakening secret
            // verification: a cache hit still verifies the supplied secret.
            if cached.verifier.verify(&self.pepper, cred.secret()) {
                let _ = self.store.touch_api_key(cred.key_id(), chrono::Utc::now()).await;
                return AuthDecision::Allow(AuthenticatedPrincipal::ApiKey {
                    account: cached.account.clone(),
                    key_id: cred.key_id().to_owned(),
                });
            }
            self.cache.put_negative(cred.key_id().to_string());
            return AuthDecision::Deny;
        }
        let key = match self.store.find_api_key(cred.key_id()).await {
            Ok(Some(k)) if k.status == ApiKeyStatus::Active
                && k.expires_at.map(|e| e > chrono::Utc::now()).unwrap_or(true)
                && k.verifier.verify(&self.pepper, cred.secret()) => k,
            _ => {
                self.cache.put_negative(cred.key_id().to_string());
                return AuthDecision::Deny;
            }
        };
        let account = match self.store.find_account_by_id(&key.account_id).await {
            Ok(Some(a)) if a.status == AccountStatus::Active => Arc::new(a),
            _ => {
                self.cache.put_negative(cred.key_id().to_string());
                return AuthDecision::Deny;
            }
        };
        self.cache.put_positive(
            cred.key_id().to_string(),
            account.clone(),
            key.verifier.clone(),
        );
        // Update last_used_at with a monotonic/CAS registry write. A transient
        // telemetry timestamp failure must not turn an already valid request
        // into an authentication failure, and the raw secret is never written.
        let _ = self.store.touch_api_key(&key.id, chrono::Utc::now()).await;
        AuthDecision::Allow(AuthenticatedPrincipal::ApiKey {
            account,
            key_id: cred.key_id().to_owned(),
        })
    }

    pub async fn is_current(&self, principal: &AuthenticatedPrincipal) -> bool {
        match principal {
            AuthenticatedPrincipal::ApiKey { account, key_id } => {
                matches!(
                    self.store.find_api_key(key_id).await,
                    Ok(Some(key)) if key.account_id == account.id
                        && key.status == ApiKeyStatus::Active
                        && key.expires_at.map(|expiry| expiry > chrono::Utc::now()).unwrap_or(true)
                ) && matches!(
                    self.store.find_account_by_id(&account.id).await,
                    Ok(Some(current)) if current.status == AccountStatus::Active
                )
            }
            #[cfg(feature = "control-plane")]
            AuthenticatedPrincipal::Oidc { account, .. } => matches!(
                self.store.find_account_by_id(&account.id).await,
                Ok(Some(current)) if current.status == AccountStatus::Active
            ),
        }
    }
}
```

The subscription listener calls `is_current` at least every 30 seconds; this
makes the external revocation bound strictly below the 60-second contract.

- [ ] **Step 3: Test cache semantics**

```rust
#[tokio::test]
async fn negative_cache_swallows_repeated_unknown_keys() { /* ... */ }
#[tokio::test]
async fn positive_cache_does_not_keep_expired_keys() { /* ... */ }
#[tokio::test]
async fn revoke_immediately_invalidates_positive_cache_within_bound() { /* ... */ }
```

- [ ] **Step 4: Auth cache belongs to `HttpState`**

Add the field to `HttpState` and construct it inside `new_tenantless` (the
single constructor — its metrics parameter is cfg-gated, Task 3.8; no
cfg-variant duplication). `default_for_test` needs no change: the registry is
the in-memory store wired in Task 4.1 Step 4.5.

```rust
pub authenticator: Arc<crate::http::principal::auth::Authenticator>,
```

Construction in `HttpState::new_tenantless` (the registry handle already
exists at this point — Task 4.1 Step 4.5 replaced the Phase 3 stub with the
real store):

```rust
let authenticator = Arc::new(crate::http::principal::auth::Authenticator::new(
    registry.store_clone(),                       // Arc<dyn RegistryStore>
    Arc::new(crate::http::principal::cache::PrincipalCache::new(1024)),
    config.api_key_pepper.as_bytes().to_vec(),
    Arc::new(crate::http::principal::auth::RateLimiter::new(
        4096,
        std::time::Duration::from_secs(1),
        20,
    )),
));
```

(`registry.store_clone()` is the accessor added in Task 4.1 Step 4.5.)

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/http/principal/
git commit -m "feat(principal): auth cache + Authenticator"
```

### Task 4.5: Account → Tenant resolver

**Files:**
- Create: `crates/memory-mcp/src/http/registry/account.rs`

- [ ] **Step 1: Implement resolver**

```rust
pub struct AccountResolver {
    store: Arc<dyn RegistryStore>,
}

pub enum ResolvedTenant {
    Ready(Tenant),
    Provisioning(TenantStatus, String), // correlation ID
    Suspended,
    Failed(String),
    NotFound,
}

impl AccountResolver {
    pub fn new(store: Arc<dyn RegistryStore>) -> Self {
        Self { store }
    }

    pub async fn resolve_ready_tenant(&self, account_id: &str) -> Result<ResolvedTenant, MemoryError> {
        let Some(tenant) = self.store.find_tenant_by_account(account_id).await? else {
            return Ok(ResolvedTenant::NotFound);
        };
        Ok(match tenant.status {
            TenantStatus::Ready => ResolvedTenant::Ready(tenant),
            TenantStatus::Suspended => ResolvedTenant::Suspended,
            TenantStatus::Failed => ResolvedTenant::Failed(tenant.id),
            other => ResolvedTenant::Provisioning(other, tenant.id),
        })
    }
}
```

A missing tenant is a RESOLUTION OUTCOME (`NotFound` → 404 in Task 5.6), not
an `Auth` error — the caller was already authenticated. Do not map it to
`MemoryError::Auth`.

- [ ] **Step 2: Test**

```rust
#[tokio::test]
async fn returns_ready_when_tenant_state_is_ready() { /* ... */ }
#[tokio::test]
async fn returns_provisioning_when_state_is_reserved_or_migrating() { /* ... */ }
```

- [ ] **Step 2.5: Wire into `HttpState`**

Add the field to `HttpState` and construct it inside `new_tenantless`
(Task 4.1 Step 4.5 shape; `default_for_test` unchanged):

```rust
pub account_resolver: Arc<crate::http::registry::account::AccountResolver>,
```

constructed as `Arc::new(AccountResolver::new(registry.store_clone()))`.
Task 5.6's middleware consumes it via `state.account_resolver`.

- [ ] **Step 3: Commit**

```bash
git add crates/memory-mcp/src/http/registry/account.rs
git commit -m "feat(registry): account→tenant resolver"
```

### Task 4.6: Wire authentication into HTTP pipeline (still no runtime pool)

**Files:**
- Modify: `crates/memory-mcp/src/http/middleware.rs`
- Modify: `crates/memory-mcp/src/http/server.rs`

- [ ] **Step 1: Add `auth_middleware`**

First, the concrete principal type in `principal/mod.rs` (later phases extend
the enum with an OAuth variant; the accessor names are stable):

```rust
use std::sync::Arc;

use super::registry::models::Account;

The request-scoped authenticated identity. Every namespace decision derives
/// from this value — never from MCP arguments, URL paths, or claims.
#[derive(Clone)]
pub enum AuthenticatedPrincipal {
    ApiKey {
        account: Arc<Account>,
        key_id: String,
    },
    #[cfg(feature = "control-plane")]
    Oidc {
        account: Arc<Account>,
        issuer: String,
        /// Verified raw claim retained only in transient request memory.
        subject: String,
    },
}

impl AuthenticatedPrincipal {
    pub fn account_id(&self) -> &str {
        match self {
            Self::ApiKey { account, .. } => &account.id,
            #[cfg(feature = "control-plane")]
            Self::Oidc { account, .. } => &account.id,
        }
    }

    pub fn account(&self) -> &Arc<Account> {
        match self {
            Self::ApiKey { account, .. } => account,
            #[cfg(feature = "control-plane")]
            Self::Oidc { account, .. } => account,
        }
    }
}
```

Then the middleware:

```rust
pub async fn authenticate(
    State(state): State<Arc<HttpState>>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, StatusCode> {
    let header = req.headers().get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok()).map(str::to_owned);
    let decision = match header.as_deref() {
        Some(h) if h.starts_with("Bearer ") => state.authenticator.authenticate_bearer(&h[7..]).await,
        _ => AuthDecision::Deny,
    };
    let principal = match decision {
        AuthDecision::Allow(principal) => principal,
        _ => {
            return Err(StatusCode::UNAUTHORIZED);
        }
    };
    req.extensions_mut().insert(principal);
    Ok(next.run(req).await)
}
```

On the 401 boundary, add `WWW-Authenticate: Bearer realm="memory-mcp"`
without distinguishing missing, unknown, expired, revoked, or malformed keys.
The middleware must not put the raw Authorization value in an error, log, or
correlation context.

- [ ] **Step 2: Wire into router**

Auth applies ONLY to `/mcp` — never to `/health/*` or `/metrics` (ADR-0052
"Route groups use separate middleware"). Axum supports layers on a single
`MethodRouter`, so attach it to the route, not to the whole router. Layers on
a `MethodRouter` run AFTER router-level layers, in reverse addition order —
with a single route-scoped layer the full builder becomes (Task 3.7 shape):

```rust
pub fn build_router(state: Arc<HttpState>) -> Router {
    Router::new()
        .route("/health/live", get(super::health::live))
        .route("/health/ready", get(super::health::ready))
        .route(
            "/mcp",
            post(super::transport::mcp_handler).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                super::middleware::authenticate,
            )),
        )
        .layer(axum::middleware::from_fn(super::middleware::reject_non_post_mcp))
        .layer(axum::middleware::from_fn_with_state(state.clone(), super::middleware::host_origin))
        .layer(axum::middleware::from_fn(super::middleware::inject_sse_headers))
        .with_state(state)
}
```

NOTE: from this task on, every `POST /mcp` requires a valid Bearer key. The
Task 3.11 black-box suite carries no credentials and therefore FAILS with 401
between this task and Task 5.8 (which adds the test bootstrap and updates the
suite). Do not run the Phase 12 conformance gate in between.

- [ ] **Step 3: Test**

```rust
#[tokio::test]
async fn mcp_without_bearer_returns_401() {
    use tower::ServiceExt;
    let state = HttpState::default_for_test().await;
    let router = build_router(state);
    let resp = router.oneshot(Request::builder().method("POST").uri("/mcp")
        .header("host", "localhost").header("content-type", "application/json")
        .body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
```

- [ ] **Step 4: Run**

Run:

```bash
cargo test -p memory_mcp --features streamable-http --lib http
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/http/
git commit -m "feat(http): MCP bearer auth middleware (401 without key)"
```

### Task 4.7: Account/Tenant identity record CRUD via control API (operator-only)

**Files:**
- Create: `crates/memory-mcp/src/control/mod.rs`
- Create: `crates/memory-mcp/src/control/error.rs`
- Create: `crates/memory-mcp/src/control/operator.rs`
- Create: `crates/memory-mcp/src/control/account_api.rs`
- Create: `crates/memory-mcp/src/http/registry/provisioning.rs`
- Modify: `crates/memory-mcp/src/lib.rs` (register `pub mod control;` under `#[cfg(feature = "control-plane")]`)

For Phase 4, only the operator stub is needed; user-facing API arrives in Phase 10. This task is gated on `control-plane`, because the control module is optional and must not be compiled into a data-plane-only HTTP build.

- [ ] **Step 1: Control module skeleton: `ApiError`, operator stub, endpoint**

`ApiError` is referenced by every control-plane handler (4.7, Phase 10) but
defined nowhere else — define it here, in `control/error.rs`:

```rust
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::error::MemoryError;

/// Control-plane API error. Maps to HTTP status at the axum boundary.
/// Phase 10 extends the variants; the mapping stays in this one place.
pub enum ApiError {
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict,
    Unavailable,
    ReauthRequired,
    Internal(MemoryError),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            ApiError::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not found"),
            ApiError::Conflict => (StatusCode::CONFLICT, "conflict"),
            ApiError::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "temporarily unavailable"),
            ApiError::ReauthRequired => {
                (StatusCode::UNAUTHORIZED, "recent authentication required")
            }
            // Internal details are logged server-side; the response body
            // stays generic (no storage/error-shape leak).
            ApiError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
        };
        (status, message).into_response()
    }
}

impl From<MemoryError> for ApiError {
    fn from(err: MemoryError) -> Self {
        ApiError::Internal(err)
    }
}
```

`OperatorPrincipal` stub in `control/operator.rs` — Phase 10 (Task 10.6)
replaces it with OIDC-derived operator identity; the accessor name stays:

```rust
/// Phase 4 stub operator principal. Injection: with the `test-fixtures`
/// feature, the stub middleware below accepts `X-Operator-Auth: stub`;
/// without `test-fixtures` there is NO operator injection in Phase 4
/// (operator endpoints are unreachable until Phase 10).
#[derive(Clone)]
pub struct OperatorPrincipal {
    pub authenticated_at: chrono::DateTime<chrono::Utc>,
}

impl OperatorPrincipal {
    /// Phase 4 stub: always recent. Task 10.4 enforces the 10-minute bound.
    pub fn require_recent_auth(&self) -> Result<(), crate::control::error::ApiError> {
        Ok(())
    }
}

/// Test-fixtures-only stub middleware: injects the operator principal for
/// the Phase 4–9 operator endpoints. Removed by Task 10.6 (OIDC operators).
#[cfg(any(test, feature = "test-fixtures"))]
pub async fn stub_operator_inject(
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, axum::http::StatusCode> {
    let is_stub = req
        .headers()
        .get("x-operator-auth")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == "stub");
    if !is_stub {
        return Err(axum::http::StatusCode::UNAUTHORIZED);
    }
    req.extensions_mut().insert(OperatorPrincipal {
        authenticated_at: chrono::Utc::now(),
    });
    Ok(next.run(req).await)
}
```

The endpoint, in `control/account_api.rs`:

```rust
use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};

use super::error::ApiError;
use super::operator::OperatorPrincipal;
use crate::http::registry::models::*;
use crate::http::registry::provisioning::enqueue_provisioning;
use crate::http::HttpState;

#[derive(serde::Deserialize)]
pub struct CreateAccountRequest {
    pub display_name: Option<String>,
}

pub async fn create_account(
    State(state): State<std::sync::Arc<HttpState>>,
    Extension(operator): Extension<OperatorPrincipal>,
    Json(_req): Json<CreateAccountRequest>,
) -> Result<(StatusCode, Json<Account>), ApiError> {
    operator.require_recent_auth()?;
    let account = Account {
        id: new_account_id(),
        status: AccountStatus::Active,
        tenant_id: new_tenant_id(),
        created_at: chrono::Utc::now(),
    };
    let tenant = Tenant {
        id: account.tenant_id.clone(),
        status: TenantStatus::Reserved,
        namespace_binding: NamespaceBinding { namespace: new_namespace_name(), database: "memory".into() },
        plan_version: 1,
        schema_version: 0,
        retry_stage: None,
        provisioning_lease: None,
        created_at: chrono::Utc::now(),
        version: 0,
    };
    let store = state.registry.store_clone();
    store.write_account(&account).await?;
    store.write_tenant(&tenant).await?;
    enqueue_provisioning(&store, &tenant).await?;
    Ok((StatusCode::CREATED, Json(account)))
}
```

`enqueue_provisioning` must exist in Phase 4 (this task calls it), so create
`http/registry/provisioning.rs` HERE with the durable seam; Task 5.1 extends
the same file with the state machine:

```rust
//! Provisioning seam (ADR-0052). Task 5.1 adds the stage machine; the
//! Task 6.2 scheduler consumes the events.

use crate::error::MemoryError;
use crate::http::registry::models::Tenant;
use crate::http::registry::RegistryStore;

/// Durable enqueue: append a provisioning event for the reserved tenant.
/// Idempotency is enforced by the store (duplicate `(tenant_id, stage)`
/// events are ignored by the scheduler, Task 6.2).
pub async fn enqueue_provisioning(
    store: &std::sync::Arc<dyn RegistryStore>,
    tenant: &Tenant,
) -> Result<(), MemoryError> {
    store.append_provisioning_event(&tenant.id, "reserved").await
}
```

(`append_provisioning_event` is the trait method added in Task 4.1 Step 1.
Register the control modules in `control/mod.rs`:

```rust
pub mod account_api;
pub mod error;
pub mod operator;
```

Do not expose this Phase 4 stub endpoint in a normal runtime: without OIDC
operator authentication it would be an unauthenticated control-plane write.
For the test-fixtures build only, construct a test router with the stub
operator middleware:

```rust
#[cfg(any(test, feature = "test-fixtures"))]
pub fn test_operator_router(state: std::sync::Arc<crate::http::HttpState>) -> axum::Router {
    axum::Router::new()
        .route("/api/v1/operator/accounts", axum::routing::post(create_account))
        .layer(axum::middleware::from_fn(operator::stub_operator_inject))
        .with_state(state)
}
```

The MCP route never shares this middleware. Phase 10 mounts the production
operator routes only after replacing the stub with OIDC-derived principal
resolution.

- [ ] **Step 2: Test**

```rust
#[tokio::test]
async fn create_account_writes_registry_records_and_enqueues_provisioning() { /* ... */ }
```

- [ ] **Step 3: Commit**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --lib control

git add crates/memory-mcp/src/control/ crates/memory-mcp/src/http/registry/provisioning.rs crates/memory-mcp/src/lib.rs
git commit -m "feat(control): operator endpoint to create Account + reserved Tenant"
```

---

## Phase 5: Provisioning + Tenant Runtime pool

This phase builds the durable provisioning state machine and the runtime pool. Once complete, a request can authenticate, resolve its Tenant, acquire a runtime, and dispatch into `MemoryService`.

### Task 5.1: Provisioning state machine

**Files:**
- Modify: `crates/memory-mcp/src/http/registry/provisioning.rs` (file created in Task 4.7 with `enqueue_provisioning`)

- [ ] **Step 1: Define transitions**

```rust
use crate::error::MemoryError;
use crate::http::registry::RegistryStore;
use crate::http::registry::models::TenantStatus;

pub use TenantStatus as ProvisioningStage;

/// Fenced variant used by every provisioning transition. The implementation
/// delegates to `RegistryStore::update_tenant_state_fenced`; the unfenced
/// `transition` helper is retained only for operator state changes that do not
/// perform provisioning work.
pub async fn transition_fenced(
    store: &dyn RegistryStore,
    tenant_id: &str,
    expected_version: u64,
    from: ProvisioningStage,
    to: ProvisioningStage,
    lease: &crate::http::leases::ProvisioningLease,
) -> Result<u64, MemoryError> {
    if !can_transition(from, to) {
        return Err(MemoryError::Validation(format!("provisioning transition {from:?}->{to:?}")));
    }
    store.update_tenant_state_fenced(
        tenant_id, expected_version, from, to,
        &lease.owner_id, &lease.lease_id, lease.fencing_generation,
    ).await
}

pub fn can_transition(from: ProvisioningStage, to: ProvisioningStage) -> bool {
    use ProvisioningStage::*;
    match (from, to) {
        (Reserved, NamespaceCreating) => true,
        (NamespaceCreating, Migrating) => true,
        (NamespaceCreating, Failed) => true,
        (Migrating, Ready) => true,
        (Migrating, Failed) => true,
        (Ready, Suspended) => true,
        (Suspended, Ready) => true,
        (_, Deleting) => matches!(from, Reserved | NamespaceCreating | Migrating | Ready | Suspended | Failed),
        (Deleting, Purged) => true,
        (Failed, NamespaceCreating) => true, // retry
        (Failed, Migrating) => true,
        _ => false,
    }
}
```

- [ ] **Step 2: CAS update**

```rust
pub async fn transition(
    store: &dyn RegistryStore,
    tenant_id: &str,
    expected_version: u64,
    from: ProvisioningStage,
    to: ProvisioningStage,
) -> Result<u64, MemoryError> {
    if !can_transition(from, to) {
        return Err(MemoryError::Validation(format!("provisioning transition {from:?}->{to:?}")));
    }
    store
        .update_tenant_state(tenant_id, expected_version, from, to)
        .await
}
```

(CAS on `(version, status)` is enforced inside `update_tenant_state`, using
both `version = $expected_version` and `status = $expected_state` in the
SurrealQL predicate. A stale worker therefore cannot advance a newer state
that happens to reuse the same transition path.)

- [ ] **Step 3: Test all legal transitions and a few illegal ones**

```rust
#[test] fn reserved_to_namespace_creating_legal() { /* ... */ }
#[test] fn migrating_to_purged_illegal() { /* ... */ }
```

- [ ] **Step 4: Commit**

```bash
git add crates/memory-mcp/src/http/registry/provisioning.rs
git commit -m "feat(registry): provisioning state machine"
```

### Task 5.2: Idempotent namespace creation

**Files:**
- Modify: `crates/memory-mcp/src/http/registry/storage.rs`

- [ ] **Step 1: Implement `ensure_namespace`**

**Verified seam:** the `DbClient` trait already has
`query(sql, vars, namespace)` — no new trait method for raw SQL is needed.
What provisioning needs is a *privileged* session that can issue DDL. Two
SurrealQL facts shape the implementation:

1. `DEFINE NAMESPACE IF NOT EXISTS` may run from any session holding a
   root-level credential, regardless of its current ns/db.
2. `DEFINE DATABASE IF NOT EXISTS` creates the database in the session's
   **current namespace** — so the session must first be bound to the target
   tenant namespace.

Therefore `ensure_namespace` is generic over the SurrealDB connection so the
same function can test the real Mem engine and run against the remote engine.
It clones the privileged `Surreal<C>` handle (a cheap handle clone), binds
the clone, and runs the DDL. Tenant namespace names are server-generated
(`tns_<uuid>` — safe identifier charset), so backtick-quoting is sufficient:

```rust
pub async fn ensure_namespace<C>(
    privileged: &surrealdb::Surreal<C>,
    namespace: &str,
    database: &str,
) -> Result<(), MemoryError>
where
    C: surrealdb::Connection,
{
    if !is_safe_identifier(namespace) || !is_safe_identifier(database) {
        return Err(MemoryError::Validation(
            "namespace/database name must be server-generated tns_/db identifier".into(),
        ));
    }
    // 1. Namespace DDL is global for a root credential.
    privileged
        .query(format!("DEFINE NAMESPACE IF NOT EXISTS `{namespace}`;"))
        .await
        .map_err(|err| MemoryError::Storage(format!("define namespace failed: {err}")))?;
    // 2. Database DDL applies to the session's current namespace: bind first.
    let bound = privileged.clone();
    bound
        .use_ns(namespace)
        .use_db(database)
        .await
        .map_err(|err| MemoryError::Storage(format!("bind for define database failed: {err}")))?;
    bound
        .query(format!("DEFINE DATABASE IF NOT EXISTS `{database}`;"))
        .await
        .map_err(|err| MemoryError::Storage(format!("define database failed: {err}")))?;
    Ok(())
}

/// Server-generated identifiers only: ascii alphanumerics and underscore.
fn is_safe_identifier(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}
```

This is a free function in `http/registry/storage.rs` operating on the raw
privileged client held by the provisioning worker — it does not extend the
`DbClient` trait (the trait stays stdio-shaped).

- [ ] **Step 2: Test**

```rust
#[tokio::test]
async fn ensure_namespace_is_idempotent() {
    // Embedded SurrealDB (Mem engine) accepts the same DDL; the privileged
    // handle here is the raw engine client used by connect_in_memory tests.
    let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(()).await.unwrap();
    db.use_ns("control").use_db("control").await.unwrap();
    ensure_namespace(&db, "ns_a", "db_a").await.unwrap();
    ensure_namespace(&db, "ns_a", "db_a").await.unwrap();
}

#[tokio::test]
async fn ensure_namespace_rejects_non_server_generated_names() {
    let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(()).await.unwrap();
    db.use_ns("control").use_db("control").await.unwrap();
    assert!(ensure_namespace(&db, "ns;drop", "db_a").await.is_err());
}
```

The generic `C: surrealdb::Connection` signature is intentional: the test
uses `local::Mem`, while production uses the remote WebSocket connection; the
DDL semantics and safety checks are identical. Keep the engine's root
privilege requirement explicit — `ensure_namespace` must never be callable
from an ordinary tenant-bound credential.

- [ ] **Step 3: Run stdio regression**

Run:

```bash
cargo test -p memory_mcp --features fs-watch,mcp-apps --test service_acceptance
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/memory-mcp/src/http/registry/storage.rs crates/memory-mcp/src/storage/client.rs
git commit -m "feat(registry): idempotent namespace/database creation"
```

### Task 5.3: Migration of Tenant Namespaces

**Files:**
- Create: `crates/memory-mcp/src/http/leases/mod.rs`
- Create: `crates/memory-mcp/src/http/leases/migration.rs`
- Modify: `crates/memory-mcp/src/http/mod.rs` (register `pub mod leases;`)
- Modify: `crates/memory-mcp/src/http/registry/storage.rs` (`update_tenant_schema_version` seam)

- [ ] **Step 1: Register the lease/migration module and apply tenant migrations**

In `http/mod.rs`, add `pub mod leases;`; in `http/leases/mod.rs`, add
`pub mod migration;`. The tenant namespace uses **the same migration scripts** as the stdio profile (`storage/migrations::versioned_migrations`). The provisioning worker:

1. Requires a live `ProvisioningLease`; there is no un-fenced provisioning path.
2. Starts from `Reserved`, or from the durable `Tenant.retry_stage` recorded with
   `Failed`; a retry of `Migrating` does not rerun namespace creation as a state
   transition, but still calls idempotent `ensure_namespace` before migrations.
3. Transitions `Reserved → NamespaceCreating` (or the recorded retry stage) with
   the lease owner/id/generation in the same CAS.
4. Transitions `NamespaceCreating → Migrating` before running migrations.
5. Calls the generic `ensure_namespace` from Task 5.2 with the privileged
   raw engine. The `Local` and `Remote` variants use the same function.
6. Obtains the privileged engine, clones the raw handle, binds the clone ONCE
   (`use_ns(namespace).use_db(database)`), and wraps it with the appropriate
   prebound constructor. The `Local` branch uses the already-landed
   `from_prebound_mem`; the `Remote` branch uses `SurrealDbClient::from_prebound`
   added in this task and specified in Task 5.4 Step 3.
7. Calls `apply_migrations` against that bound client and persists the new
   `schema_version` with a versioned and fenced CAS update.
8. Verifies schema postconditions, then transitions `Migrating → Ready` with the
   same fencing generation. A stale worker receives `MemoryError::Conflict` and
   cannot mark the Tenant ready.
   Every failure path records the error and transitions to `Failed` using the
   same fencing/version guard, including `retry_stage`; it never marks a partially
   migrated tenant `Ready`.

Implement the worker entry point in `leases/migration.rs` so the test and the
scheduler have one production path:

```rust
use crate::error::MemoryError;
use crate::http::leases::ProvisioningLease;
use crate::http::registry::models::TenantStatus;
use crate::http::registry::provisioning::transition_fenced;
use crate::http::registry::{PrivilegedEngine, RegistryHandle, RegistryStore};
use crate::http::registry::storage::ensure_namespace;

pub async fn provision_one(
    registry: &RegistryHandle,
    tenant_id: &str,
    lease: ProvisioningLease,
) -> Result<(), MemoryError> {
    let store = registry.store_clone();
    let mut tenant = store.find_tenant_by_id(tenant_id).await?
        .ok_or_else(|| MemoryError::NotFound(format!("tenant {tenant_id}")))?;
    let retry_from = tenant.retry_stage;
    if tenant.status == TenantStatus::Reserved {
        transition_fenced(&*store, tenant_id, tenant.version,
            TenantStatus::Reserved, TenantStatus::NamespaceCreating, &lease).await?;
    } else if tenant.status == TenantStatus::Failed {
        let stage = retry_from.ok_or_else(|| MemoryError::Validation(
            "failed tenant has no retry stage".into()))?;
        transition_fenced(&*store, tenant_id, tenant.version,
            TenantStatus::Failed, stage, &lease).await?;
    }
    tenant = store.find_tenant_by_id(tenant_id).await?
        .ok_or_else(|| MemoryError::NotFound(format!("tenant {tenant_id}")))?;
    if tenant.status == TenantStatus::NamespaceCreating {
        transition_fenced(&*store, tenant_id, tenant.version,
            TenantStatus::NamespaceCreating, TenantStatus::Migrating, &lease).await?;
    }
    tenant = store.find_tenant_by_id(tenant_id).await?
        .ok_or_else(|| MemoryError::NotFound(format!("tenant {tenant_id}")))?;
    if tenant.status != TenantStatus::Migrating {
        return Err(MemoryError::Conflict("tenant is no longer in Migrating state".into()));
    }
    let binding = tenant.namespace_binding.clone();

    let migration_result: Result<u32, MemoryError> = lease.run_with_heartbeat(registry.clone(), tenant_id, async {
        match registry.tenant_engine() {
            PrivilegedEngine::Remote(privileged) => {
                ensure_namespace(&*privileged, &binding.namespace, &binding.database).await?;
                let bound = (*privileged).clone();
                bound.use_ns(&binding.namespace).use_db(&binding.database).await
                    .map_err(|err| MemoryError::Storage(format!("tenant bind failed: {err}")))?;
                let client = crate::storage::client::SurrealDbClient::from_prebound(
                    bound, &binding.namespace, "warn",
                );
                client.apply_migrations(&binding.namespace).await?;
                Ok(crate::storage::migrations::versioned_migrations().len() as u32)
            }
            PrivilegedEngine::Local(privileged) => {
                ensure_namespace(&*privileged, &binding.namespace, &binding.database).await?;
                let bound = (*privileged).clone();
                bound.use_ns(&binding.namespace).use_db(&binding.database).await
                    .map_err(|err| MemoryError::Storage(format!("tenant bind failed: {err}")))?;
                let client = crate::storage::client::SurrealDbClient::from_prebound_mem(
                    bound, &binding.namespace, "warn",
                );
                client.apply_migrations(&binding.namespace).await?;
                Ok(crate::storage::migrations::versioned_migrations().len() as u32)
            }
        }
    }).await;

    let schema_version = match migration_result {
        Ok(version) => version,
        Err(error) => {
            if let Some(failed) = store.find_tenant_by_id(tenant_id).await?
                && matches!(failed.status, TenantStatus::NamespaceCreating | TenantStatus::Migrating)
            {
                let _ = transition_fenced(&*store, tenant_id, failed.version,
                    failed.status, TenantStatus::Failed, &lease).await;
            }
            return Err(error);
        }
    };
    tenant = store.find_tenant_by_id(tenant_id).await?
        .ok_or_else(|| MemoryError::NotFound(format!("tenant {tenant_id}")))?;
    let version = store.update_tenant_schema_version_fenced(
        tenant_id, tenant.version, schema_version,
        &lease.owner_id, &lease.lease_id, lease.fencing_generation,
    ).await?;
    transition_fenced(&*store, tenant_id, version,
        TenantStatus::Migrating, TenantStatus::Ready, &lease).await?;
    lease.release(&*store, tenant_id).await
}
```

The implementation must preserve the stage ordering shown above. If the
migration fails before `NamespaceCreating → Migrating`, transition directly
from the current `NamespaceCreating` row to `Failed`; if it fails later,
transition from the current `Migrating` row. The `failed` transition is best
effort but the original error is always returned.

- [ ] **Step 2: Use fenced lease**

`Task 6.1` Steps 1–3 are a prerequisite for this step and must be executed
before the provisioning worker test; there is no temporary un-fenced fallback.
The scheduler calls `claim_provisioning`, receives a `ProvisioningLease { owner_id,
lease_id, fencing_generation }`, passes it to `provision_one`, and the worker uses
only the fenced RegistryStore methods. A heartbeat task runs at
`lease_ttl / 3` with ±20% jitter and is cancelled/joined before release. Release
is attempted only with the same owner and lease id; a lost heartbeat or failed
fenced write aborts the worker and prevents `Ready`.

- [ ] **Step 3: Test with embedded SurrealDB**

```rust
#[tokio::test]
async fn provisioning_runs_migrations_to_ready_state() {
    let (registry, tenant_id) = build_test_registry_with_reserved_tenant().await;
    let now = chrono::Utc::now();
    let lease = registry.store_clone().claim_provisioning(
        &tenant_id, "test-replica", "lease-test-1", now,
        now + chrono::Duration::seconds(60),
    ).await.unwrap().expect("test tenant claim");
    provision_one(&registry, &tenant_id, lease).await.unwrap();
    let tenant = registry.store_clone().find_tenant_by_id(&tenant_id).await
        .unwrap().unwrap();
    assert_eq!(tenant.status, TenantStatus::Ready);
    assert!(tenant.schema_version > 0);
}
```

- [ ] **Step 4: Commit**

```bash
git add crates/memory-mcp/src/http/leases/ crates/memory-mcp/src/http/registry/ crates/memory-mcp/src/storage/client.rs
git commit -m "feat(registry): migration step in provisioning"
```

### Task 5.4: Tenant Runtime contents + clone-once/bind-once binding

**Files:**
- Create: `crates/memory-mcp/src/http/runtime/storage.rs`
- Modify: `crates/memory-mcp/src/http/runtime/mod.rs` (exists since Task 3.9 with the `pool` stub module)

- [ ] **Step 1: Define runtime types**

```rust
pub struct TenantRuntime {
    pub tenant_id: String,
    pub namespace: String,
    pub database: String,
    pub schema_version: u32,
    pub tenant_db: std::sync::Arc<crate::storage::client::SurrealDbClient>, // clone-once, bind-once; never rebound
    /// Namespace-free adapter for App Sessions, outbox, and other tenant
    /// stores; it always delegates with this runtime's immutable namespace.
    pub bound_db: std::sync::Arc<crate::storage::client::BoundDbClient>,
    pub mcp_service: crate::mcp::handlers::MemoryMcp, // constructed from Tenant-bound MemoryService
    pub created_at: std::time::Instant,
}
```

The tenant-bound `SurrealDbClient` is acquired by **cloning** the privileged
raw `Surreal<Client>`/`Surreal<Db>` handle and calling
`use_ns(namespace).use_db(database)` exactly once, then wrapping and storing
the resulting adapter. This satisfies "clone-once, bind-once" — see spec
§identity-and-tenancy.

- [ ] **Step 2: Implement factory**

```rust
pub async fn build_runtime(
    registry: &RegistryHandle,
    tenant: &Tenant,
) -> Result<TenantRuntime, MemoryError> {
    let tenant_db = std::sync::Arc::new(match registry.tenant_engine() {
        crate::http::registry::PrivilegedEngine::Remote(privileged) => {
            let ns_client = (*privileged).clone();
            ns_client
                .use_ns(&tenant.namespace_binding.namespace)
                .use_db(&tenant.namespace_binding.database)
                .await
                .map_err(|err| MemoryError::Storage(format!("tenant bind failed: {err}")))?;
            crate::storage::client::SurrealDbClient::from_prebound(
                ns_client,
                &tenant.namespace_binding.namespace,
                "info",
            )
        }
        crate::http::registry::PrivilegedEngine::Local(privileged) => {
            let ns_client = (*privileged).clone();
            ns_client
                .use_ns(&tenant.namespace_binding.namespace)
                .use_db(&tenant.namespace_binding.database)
                .await
                .map_err(|err| MemoryError::Storage(format!("tenant bind failed: {err}")))?;
            crate::storage::client::SurrealDbClient::from_prebound_mem(
                ns_client,
                &tenant.namespace_binding.namespace,
                "info",
            )
        }
    });
    let bound_db = std::sync::Arc::new(crate::storage::client::BoundDbClient::new(
        tenant_db.clone(),
        tenant.namespace_binding.namespace.clone(),
    ));
    let service = crate::service::MemoryService::new(
        tenant_db.clone(),
        tenant.namespace_binding.namespace.clone(),
        "info".into(),
        100, // rate_limit_rps: plan-driven value arrives with quotas (Task 6.4)
        100, // rate_limit_burst
    )?;
    let mcp = crate::mcp::handlers::MemoryMcp::new_modern(service);
    Ok(TenantRuntime {
        tenant_id: tenant.id.clone(),
        namespace: tenant.namespace_binding.namespace.clone(),
        database: tenant.namespace_binding.database.clone(),
        schema_version: tenant.schema_version,
        tenant_db,
        bound_db,
        mcp_service: mcp,
        created_at: std::time::Instant::now(),
    })
}
```

(`MemoryService::new(db_client, active_namespace, log_level, rate_limit_rps,
rate_limit_burst)` is the existing public constructor in
`service/core/builder.rs` — no `new_for_tenant` is added. `new_modern` pins
the MCP protocol per Task 3.4.)

- [ ] **Step 3: `SurrealDbClient::from_prebound` (canonical code) + `from_prebound_mem` twin**

The local `from_prebound_mem` constructor was already added in Task 4.1 so
embedded registry/runtime tests share one Mem handle — verify it and do NOT
add it twice. This step adds the remote `from_prebound` constructor needed by
production tenant runtimes.

In `storage/client.rs`. No `PreboundDbClient` shim and no `todo!()` — the
existing method implementations already dispatch through `engine` and check
`ensure_active_namespace`, so a pre-bound engine reuses all of them:

```rust
impl SurrealDbClient {
    /// Wraps an already-bound remote client (clone-once/bind-once, ADR-0052).
    /// The caller MUST have called `use_ns(...).use_db(...)` on `db` exactly
    /// once before passing it in; this constructor never rebinds.
    /// stdio never calls this constructor.
    pub fn from_prebound(
        db: surrealdb::Surreal<surrealdb::engine::remote::ws::Client>,
        active_namespace: &str,
        log_level: &str,
    ) -> Self {
        Self {
            engine: DbEngine::Remote(std::sync::Arc::new(db)),
            active_namespace: active_namespace.to_string(),
            logger: StdoutLogger::new(log_level),
            fact_embedding_dimension: crate::config::DEFAULT_EMBEDDING_DIMENSION,
        }
    }

}
```

`SurrealDbClient` has no public `Clone`; create exactly one bound adapter,
wrap it in `Arc<SurrealDbClient>`, pass a clone of that Arc to
`MemoryService::new`, and store the same Arc in `TenantRuntime.tenant_db`.
The `build_runtime` code in Step 2 already follows this ownership rule.

- [ ] **Step 4: Tests**

```rust
#[tokio::test]
async fn prebound_client_rejects_foreign_namespace_queries() {
    // The pre-bound client's ensure_active_namespace guard is the proof that
    // no re-binding happens: a query naming another namespace must fail.
    let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(()).await.unwrap();
    db.use_ns("tenant_a").use_db("memory").await.unwrap();
    // (Mem-engine variant: from_prebound_mem mirrors from_prebound for tests.)
    let client = SurrealDbClient::from_prebound_mem(db, "tenant_a", "error");
    assert!(client.select_one("fact:x", "tenant_b").await.is_err());
    assert!(client.select_one("fact:x", "tenant_a").await.is_ok());
}

#[tokio::test]
async fn two_runtimes_have_independent_db_handles() {
    let registry = build_test_registry().await; // seeds two ready tenants
    let t1 = registry.first_ready_tenant().await;
    let t2 = registry.second_ready_tenant().await;
    let r1 = build_runtime(&registry.handle(), &t1).await.unwrap();
    let r2 = build_runtime(&registry.handle(), &t2).await.unwrap();
    assert_ne!(r1.namespace, r2.namespace);
    assert!(!std::sync::Arc::ptr_eq(&r1.tenant_db, &r2.tenant_db));
}
```

(`from_prebound_mem` is the `DbEngine::Local` constructor from Task 4.1;
`build_test_registry` is the Task 4.1 embedded registry fixture extended to
seed ready tenants.)

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/http/runtime/ crates/memory-mcp/src/storage/client.rs
git commit -m "feat(runtime): Tenant Runtime with clone-once/bind-once DB handle"
```

### Task 5.5: Runtime lifecycle + LRU pool

**Files:**
- Create: `crates/memory-mcp/src/http/runtime/lifecycle.rs`
- Modify: `crates/memory-mcp/src/http/runtime/pool.rs` (Phase 3 `AdmissionGate` stub from Task 3.9)
- Create: `crates/memory-mcp/src/http/runtime/activation.rs`
- Create: `crates/memory-mcp/src/http/runtime/guard.rs`

Pool defaults (spec §7.3): 32 active, 15-min idle, 2-sec capacity wait, 4 concurrent per Tenant, 30-sec activation.

- [ ] **Step 1: Implement lifecycle states**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimePhase { Absent, Loading, Ready, Draining, Unloaded, Failed }
```

`TenantRuntime` carries a `Mutex<RuntimePhase>`. Transitions happen under a per-Tenant mutex inside the pool.

- [ ] **Step 2: Implement single-flight activation**

```rust
pub struct ActivationSlot {
    pub state: RuntimePhase,
    pub generation: u64,        // increments on each (re)activation
    pub in_flight: Option<tokio::sync::broadcast::Sender<std::sync::Arc<TenantRuntime>>>,
    pub negative_backoff_until: Option<Instant>,
}
```

`pool::activate(tenant_id)` subscribes callers to the SAME in-flight
activation via the slot's broadcast channel and returns
`broadcast::Receiver<Arc<TenantRuntime>>` (matching `ActivationSlot::in_flight`
— do NOT mix `watch` and `broadcast` here). If activation fails, store
`negative_backoff_until` and reject new activations until expiry.

- [ ] **Step 3: Implement LRU + idle eviction**

```rust
pub struct TenantRuntimeSlot {
    pub runtime: Option<Arc<TenantRuntime>>,
    pub phase: RuntimePhase,
    pub pin_count: Arc<AtomicUsize>,
    pub active_operations: AtomicU32,
    pub last_used: Instant,
    pub activation: ActivationSlot,
}

pub struct Pool {
    map: Arc<Mutex<lru::LruCache<String, Arc<Mutex<TenantRuntimeSlot>>>>>,
    cap: usize,
    idle_ttl: Duration,
    capacity_wait: Duration,
    activation_timeout: Duration,
    per_tenant_concurrency: u32,
}
```

Idle eviction tick: a background task (registered with the tier-1 scheduler — Task 6.2) iterates `map` and calls `mark_draining_if_idle` on entries older than `idle_ttl`. **Pinned runtimes are skipped.**

The constructor contract consumed by Task 5.6 is:

```rust
pub fn new(
    cap: usize,
    idle_ttl: Duration,
    capacity_wait: Duration,
    activation_timeout: Duration,
    per_tenant_concurrency: u32,
) -> Self
```

The acquisition entry point consumed by Task 5.6:

```rust
/// Errors returned by bounded runtime acquisition.
#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    #[error("runtime capacity wait timed out")]
    CapacityTimeout,
    #[error("tenant runtime activation failed")]
    ActivationFailed,
    #[error("server is shutting down")]
    ShuttingDown,
}
```

The `Pool` implementation MUST expose this method (the following is an
interface signature, not a standalone inherent-impl body):

```rust
async fn acquire_or_wait(&self, tenant: &Tenant) -> Result<OperationGuard, PoolError>
```

Its implementation is completed in this task before the Phase 5 test suite
is run: `activate()` single-flight → ready runtime → increment pin count →
`OperationGuard { runtime, pin_count }`, with a bounded `capacity_wait`.

- [ ] **Step 4: Implement operation guard**

```rust
pub struct OperationGuard {
    runtime: std::sync::Arc<TenantRuntime>,
    pin_count: std::sync::Arc<AtomicUsize>,
}

impl OperationGuard {
    pub fn runtime(&self) -> &std::sync::Arc<TenantRuntime> {
        &self.runtime
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) { self.pin_count.fetch_sub(1, Ordering::SeqCst); }
}

/// Keeps the optional operation pin and selected admission slot alive for the
/// entire response body, including an SSE stream. Dropping the handler future
/// is not sufficient because axum may return before the body has been consumed.
pub struct ResponseLease {
    _operation: Option<OperationGuard>,
    _admission: super::pool::AdmissionPermit,
}

impl ResponseLease {
    pub fn new(
        operation: Option<OperationGuard>,
        admission: super::pool::AdmissionPermit,
    ) -> Self {
        Self { _operation: operation, _admission: admission }
    }
}

pub struct LeasedBody<B> {
    inner: std::pin::Pin<Box<B>>,
    _lease: Option<ResponseLease>,
}

impl<B> LeasedBody<B> {
    pub fn new(body: B, lease: ResponseLease) -> Self {
        Self { inner: Box::pin(body), _lease: Some(lease) }
    }
}

// Moving the Pin<Box<B>> does not move the pinned B, so this wrapper is safe to
// move as an HTTP response value.
impl<B> Unpin for LeasedBody<B> {}

impl<B> http_body::Body for LeasedBody<B>
where
    B: http_body::Body,
{
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        match this.inner.as_mut().poll_frame(cx) {
            std::task::Poll::Ready(None) => {
                this._lease.take();
                std::task::Poll::Ready(None)
            }
            std::task::Poll::Ready(Some(Err(error))) => {
                this._lease.take();
                std::task::Poll::Ready(Some(Err(error)))
            }
            other => other,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.as_ref().get_ref().is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.as_ref().get_ref().size_hint()
    }
}
```

Only runtimes with `pin_count == 0` are eligible for `Draining`. The HTTP
handler must move both permits into `ResponseLease`; it must never rely on
request/response extensions to extend the lifetime of a streaming body.

- [ ] **Step 5: Implement capacity admission**

REPLACE the Phase 3 stub `AdmissionGate` (Task 3.9: only `closed` +
`is_closed()`) with the full gate. `is_closed()` and `new()` keep working —
`/health/ready` (Task 3.9) and `HttpState::new_tenantless` call them; `new`
gains the global limit:

```rust
pub struct AdmissionGate {
    global_limit: u32,
    global_active: AtomicU32,
    subscription_limit: u32,
    subscription_active: AtomicU32,
    closed: AtomicBool,
}

/// Owned RAII permit: it can be moved into the response body. Long-lived
/// subscriptions use a separate bounded budget and never consume ordinary
/// request capacity.
pub enum AdmissionPermit {
    Request { gate: std::sync::Arc<AdmissionGate> },
    Subscription { gate: std::sync::Arc<AdmissionGate> },
}

impl Drop for AdmissionPermit {
    fn drop(&mut self) {
        let counter = match self {
            Self::Request { gate } => &gate.global_active,
            Self::Subscription { gate } => &gate.subscription_active,
        };
        counter.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

impl AdmissionGate {
    /// Default global in-flight request bound (spec §7.3 admission control).
    /// Environment-configurable override arrives with Task 6.4 quotas.
    pub fn new(global_limit: u32) -> Self {
        Self {
            global_limit,
            global_active: AtomicU32::new(0),
            subscription_limit: 32,
            subscription_active: AtomicU32::new(0),
            closed: AtomicBool::new(false),
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn close(&self) {
        self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn try_acquire(self: &std::sync::Arc<Self>) -> Result<AdmissionPermit, ()> {
        self.try_acquire_for(false)
    }

    pub fn try_acquire_for(
        self: &std::sync::Arc<Self>,
        subscription: bool,
    ) -> Result<AdmissionPermit, ()> {
        if self.is_closed() {
            return Err(());
        }
        let (limit, counter) = if subscription {
            (self.subscription_limit, &self.subscription_active)
        } else {
            (self.global_limit, &self.global_active)
        };
        // Bounded increment: admit only under the selected budget.
        let mut current = counter.load(std::sync::atomic::Ordering::SeqCst);
        loop {
            if current >= limit {
                return Err(());
            }
            match counter.compare_exchange_weak(
                current,
                current + 1,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            ) {
                Ok(_) => {
                    return Ok(if subscription {
                        AdmissionPermit::Subscription { gate: self.clone() }
                    } else {
                        AdmissionPermit::Request { gate: self.clone() }
                    });
                }
                Err(actual) => current = actual,
            }
        }
    }
}
```

Update the `HttpState::new` construction site accordingly:
`admission: Arc::new(AdmissionGate::new(256))` (the Phase 3 stub used
`AdmissionGate::new()`). The rename happens in Task 5.6 Step 0; the executor
must update this field initialization as part of that rename. The pool's
per-tenant semaphore is separate from this global gate.

- [ ] **Step 6: Tests**

Add the following assertions to the runtime unit tests. The body-lifetime test
must poll one frame and then keep the body alive; a second global admission
attempt must fail until the body is dropped.

```rust
#[tokio::test]
async fn single_flight_activation_runs_once() {
    let pool = test_pool_with_counting_activation().await;
    let tenant = test_ready_tenant();
    let mut joins = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let pool = pool.clone();
        let tenant = tenant.clone();
        joins.spawn(async move { pool.acquire_or_wait(&tenant).await });
    }
    while let Some(result) = joins.join_next().await {
        assert!(result.expect("activation task").is_ok());
    }
    assert_eq!(pool.activation_count(&tenant.id), 1);
}

#[tokio::test]
async fn pinned_runtime_is_not_evicted() {
    let pool = test_pool_with_one_runtime().await;
    let tenant = test_ready_tenant();
    let guard = pool.acquire_or_wait(&tenant).await.expect("acquire");
    pool.mark_draining_if_idle(&tenant.id, Instant::now() + Duration::from_secs(3600)).await;
    assert!(pool.contains_ready(&tenant.id).await);
    drop(guard);
}

#[tokio::test]
async fn response_body_keeps_pin_and_global_admission_until_drop() {
    let gate = std::sync::Arc::new(AdmissionGate::new(1));
    let permit = gate.try_acquire().expect("first permit");
    let guard = test_operation_guard();
    let lease = ResponseLease::new(Some(guard), permit);
    let mut body = LeasedBody::new(axum::body::Body::from("first"), lease);
    use http_body::Body as _;
    let _ = http_body_util::BodyExt::frame(&mut body).await;
    assert!(gate.try_acquire().is_err());
    drop(body);
    assert!(gate.try_acquire().is_ok());
}

#[tokio::test]
async fn capacity_overflow_returns_503() {
    let pool = test_pool_with_capacity_one().await;
    let first = pool.acquire_or_wait(&test_ready_tenant()).await.expect("first");
    assert!(matches!(pool.acquire_or_wait(&test_ready_tenant()).await, Err(PoolError::CapacityTimeout)));
    drop(first);
}

#[tokio::test]
async fn negative_cache_swallows_repeated_failures() {
    let pool = test_pool_with_failing_activation().await;
    let tenant = test_ready_tenant();
    assert!(matches!(pool.acquire_or_wait(&tenant).await, Err(PoolError::ActivationFailed)));
    assert!(matches!(pool.acquire_or_wait(&tenant).await, Err(PoolError::ActivationFailed)));
    assert_eq!(pool.activation_count(&tenant.id), 1);
}
```

The helper functions above are test fixtures in the same runtime test module;
they use a deterministic in-memory tenant and a counting/failing activation
closure, not production network state.

- [ ] **Step 7: Commit**

```bash
git add crates/memory-mcp/src/http/runtime/
git commit -m "feat(runtime): LRU pool + lifecycle + single-flight activation + operation guards"
```

### Task 5.6: Wire runtime acquisition into HTTP pipeline

**Files:**
- Modify: `crates/memory-mcp/src/http/middleware.rs`
- Modify: `crates/memory-mcp/src/http/transport.rs`
- Modify: `crates/memory-mcp/src/http/mod.rs` (`HttpState`)
- Modify: `crates/memory-mcp/src/http/router.rs`
- Modify: `crates/memory-mcp/src/bin/memory_mcp_http.rs`

- [ ] **Step 0: Retire the Phase 3 tenantless dispatch**

The runtime pool now owns dispatch; the Phase 3 tenantless factory is dead.
All four removals are mandatory — leftovers are `dead_code` under the
`-D warnings` lint gate:

1. Delete the `mcp_factory` field from `HttpState` (added in Task 3.3).
2. Delete `build_tenantless_handler` from `transport.rs` (its only consumer
   was the `mcp_factory` construction inside `new_tenantless`). Keep
   `build_server_config`, `build_mcp_service`, and `forward` — the new
   handler below reuses all three.
3. Rename `HttpState::new_tenantless` → `HttpState::new`. It is now the full
   composition constructor (registry, admission, shutdown, authenticator,
   account_resolver, metrics handle — and the pool added in Step 1). Update
   the two call sites: `default_for_test` (same file) and the Task 3.10
   binary (`HttpState::new_tenantless(cfg.clone(), …)` →
   `HttpState::new(cfg.clone(), …)`).
4. Add the pool field and construct it in `HttpState::new` with the spec
   §7.3 defaults (Task 5.5 header): 32 active, 15-min idle, 2-s capacity
   wait, 30-s activation, 4 per-tenant:

```rust
pub pool: std::sync::Arc<crate::http::runtime::pool::Pool>,
// in HttpState::new:
pool: std::sync::Arc::new(crate::http::runtime::pool::Pool::new(
    32,
    std::time::Duration::from_secs(15 * 60),
    std::time::Duration::from_secs(2),
    std::time::Duration::from_secs(30),
    4,
)),
```

(`Pool::new(cap, idle_ttl, capacity_wait, activation_timeout,
per_tenant_concurrency)` — environment-configurable overrides arrive with
Task 6.4.)

- [ ] **Step 1: Add `acquire_runtime` middleware and wire it**

```rust
pub async fn acquire_runtime(
    State(state): State<Arc<HttpState>>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, StatusCode> {
    let principal = req.extensions().get::<AuthenticatedPrincipal>().cloned();
    let principal = match principal {
        Some(p) => p,
        None => return Err(StatusCode::UNAUTHORIZED),
    };
    let tenant = match state.account_resolver.resolve_ready_tenant(principal.account_id()).await {
        Ok(ResolvedTenant::Ready(t)) => t,
        Ok(ResolvedTenant::Provisioning(_, _)) => return Err(StatusCode::SERVICE_UNAVAILABLE),
        Ok(ResolvedTenant::Suspended) => return Err(StatusCode::FORBIDDEN),
        Ok(ResolvedTenant::Failed(_)) => return Err(StatusCode::SERVICE_UNAVAILABLE),
        Ok(ResolvedTenant::NotFound) | Err(_) => return Err(StatusCode::NOT_FOUND),
    };
    let is_subscription = req
        .headers()
        .get("Mcp-Method")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|method| method == "subscriptions/listen");
    let permit = state.admission.try_acquire_for(is_subscription)
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let guard = state.pool.acquire_or_wait(&tenant).await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    req.extensions_mut().insert(permit);
    req.extensions_mut().insert(guard);
    use sha2::Digest;
    let fingerprint = hex::encode(&sha2::Sha256::digest(tenant.id.as_bytes())[..8]);
    req.extensions_mut().insert(crate::http::logging::TenantLogContext { fingerprint });
    let tenant_log_context = req.extensions().get::<crate::http::logging::TenantLogContext>().cloned();
    let mut resp = next.run(req).await;
    if let Some(context) = tenant_log_context {
        resp.extensions_mut().insert(context);
    }
    Ok(resp)
}
```

The middleware does not drop the admission permit itself: it is extracted by
`mcp_handler` and moved into the response body lease. Ordinary requests use the
global budget; `subscriptions/listen` uses the separate bounded subscription
budget. If runtime acquisition or routing fails after the permit is created, the
request extension is dropped and releases it normally.

Wire it into `build_router` as a second route-scoped layer on `/mcp`.
Route-scoped layers added EARLIER are INNER (run later), so `acquire_runtime`
is added first and `authenticate` last — request flow: `authenticate`
(inserts the principal) → `acquire_runtime` (reads it) → `mcp_handler`:

```rust
pub fn build_router(state: Arc<HttpState>) -> Router {
    Router::new()
        .route("/health/live", get(super::health::live))
        .route("/health/ready", get(super::health::ready))
        .route(
            "/mcp",
            post(super::transport::mcp_handler)
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    super::middleware::acquire_runtime,
                ))
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    super::middleware::authenticate,
                )),
        )
        .layer(axum::middleware::from_fn(super::middleware::reject_non_post_mcp))
        .layer(axum::middleware::from_fn_with_state(state.clone(), super::middleware::host_origin))
        .layer(axum::middleware::from_fn(super::middleware::inject_sse_headers))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            super::logging::request_log,
        ))
        .with_state(state)
}
```

This is the final router shape for the data plane; Phase 10 nests the
control-plane routes alongside it.

- [ ] **Step 2: Update `mcp_handler` to dispatch through the runtime guard**

`build_router` already routes `/mcp` to `mcp_handler` (since Task 3.5); only
the handler body changes. Reuse the Task 3.3 `forward` helper instead of
inlining the service call:

```rust
use axum::response::IntoResponse;

pub async fn mcp_handler(
    State(state): State<std::sync::Arc<super::HttpState>>,
    mut req: axum::extract::Request,
) -> axum::response::Response {
    // `Extension<T>` requires `T: Clone` in axum and would either force the
    // permits to be cloneable or release them at the wrong ownership boundary.
    // Remove the owned values explicitly and move them into the response body.
    let Some(guard) = req
        .extensions_mut()
        .remove::<super::runtime::guard::OperationGuard>() else {
        return (axum::http::StatusCode::SERVICE_UNAVAILABLE, "runtime guard missing").into_response();
    };
    let Some(permit) = req
        .extensions_mut()
        .remove::<super::runtime::pool::AdmissionPermit>() else {
        return (axum::http::StatusCode::SERVICE_UNAVAILABLE, "admission permit missing").into_response();
    };
    let runtime = guard.runtime().clone(); // Arc<TenantRuntime>
    // Phase 5.6 has no subscription hook yet; Task 9.2 changes this local to
    // a request-scoped handler with the principal revalidator before enabling
    // subscriptions/listen.
    let request_handler = runtime.mcp_service.clone();
    let svc = build_mcp_service(
        std::sync::Arc::new(move || Ok(request_handler.clone())),
        build_server_config(&state.config, state.shutdown.token()),
    );
    let is_subscription = req
        .headers()
        .get("Mcp-Method")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|method| method == "subscriptions/listen");
    let response = forward(svc, req).await;
    let (parts, body) = response.into_parts();
    let operation = (!is_subscription).then_some(guard);
    let lease = super::runtime::guard::ResponseLease::new(operation, permit);
    let timeout = (!is_subscription).then_some(state.config.request_deadline);
    let body = super::validation::DeadlineBody::new(body, timeout);
    // Re-erase the wrapper into axum's Body. The wrapper remains owned by the
    // body and therefore lives until SSE consumption ends or the client drops it.
    axum::response::Response::from_parts(
        parts,
        axum::body::Body::new(super::runtime::guard::LeasedBody::new(body, lease)),
    )
}
```

Notes:
- `OperationGuard` is `'static` here: it holds `Arc<TenantRuntime>` + the pin
  counter, so it can live in request extensions (the Task 5.5 definition owns
  Arcs: `runtime: Arc<TenantRuntime>`, `pin_count: Arc<AtomicUsize>`, with a
  `pub fn runtime(&self) -> &Arc<TenantRuntime>` accessor).
- `acquire_runtime` inserts both owned permits into request extensions; the
  handler extracts them and moves them into `ResponseLease` before returning.
- `LeasedBody` owns the lease, so `OperationGuard::Drop` and
  `AdmissionPermit::Drop` run only when the complete response body is dropped,
  including after SSE streaming ends or when the client disconnects. They do
  not run merely because the handler future returned.
- `DeadlineBody` is inside `LeasedBody`; ordinary calls receive the configured
  120-second deadline, while `subscriptions/listen` receives `None` and relies
  on rmcp keep-alives, client cancellation, shutdown, and the 30-second auth
  recheck loop.

- [ ] **Step 3: Test two Tenants under concurrency**

```rust
#[tokio::test]
async fn two_tenants_under_concurrency_share_no_state() { /* spec §20.2 */ }
```

- [ ] **Step 4: Run**

Run:

```bash
cargo test -p memory_mcp --features streamable-http --lib http
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/http/
git commit -m "feat(http): tenant runtime acquisition in middleware"
```

### Task 5.7: Reconciliation of orphan namespaces

**Files:**
- Modify: `crates/memory-mcp/src/http/registry/provisioning.rs` (the periodic REGISTRATION of the loop happens in Task 6.2's scheduler)

- [ ] **Step 1: Implement `reconcile`**

A periodic loop:

1. Reads all `Tenant` records in non-terminal states.
2. For each, checks whether `namespace_binding.namespace` exists on the control DB.
3. If missing, transitions `Failed` and emits a `provisioning_event`.
4. Reads all namespaces in the privileged DB; flags any with no matching tenant record as orphans. Orphan handling: log and emit metric; do not delete (operator decision only).

- [ ] **Step 2: Test**

```rust
#[tokio::test]
async fn orphan_namespace_is_logged_and_metric_incremented() { /* ... */ }
```

- [ ] **Step 3: Commit**

```bash
git add crates/memory-mcp/src/http/registry/provisioning.rs
git commit -m "feat(registry): orphan namespace reconciliation"
```

### Task 5.8: Test-only bootstrap for black-box suites (`test-fixtures`)

**Files:**
- Create: `crates/memory-mcp/src/http/test_bootstrap.rs`
- Modify: `crates/memory-mcp/src/http/mod.rs` (register `pub mod test_bootstrap;` under `#[cfg(feature = "test-fixtures")]`)
- Modify: `crates/memory-mcp/src/http/config.rs` (reject the bootstrap env var without `test-fixtures`)
- Modify: `crates/memory-mcp/src/bin/memory_mcp_http.rs`
- Modify: `crates/memory-mcp/tests/http_proto_conformance.rs`

**Why.** Task 4.6 put Bearer auth in front of `/mcp`, so the Task 3.11
black-box suite (and Phase 12 isolation/load suites) fail with 401. The
operator API that creates accounts is a stub until Phase 10, and normal
provisioning is async. Black-box suites therefore need a deterministic way
to provision ready tenants: a bootstrap spec, strictly gated behind the
`test-fixtures` feature (never compiled into production builds).

- [ ] **Step 1: Bootstrap env var + startup hook**

Env var `MEMORY_MCP_HTTP_TEST_BOOTSTRAP`: comma-separated entries
`<name>=<api_key>` where `<api_key>` is a full well-formed key
`mem_sk_ak_<uuid>_<secret>` (parsed by Task 4.3's `ApiKeyCredential::parse`).
Each entry creates one Account + one ready Tenant + one active ApiKey:

```rust
//! Test-only bootstrap (test-fixtures feature). NEVER compiled without it.

use std::sync::Arc;

use crate::error::MemoryError;
use crate::http::principal::api_keys::ApiKeyCredential;
use crate::http::registry::models::*;
use crate::http::HttpState;

pub const ENV_TEST_BOOTSTRAP: &str = "MEMORY_MCP_HTTP_TEST_BOOTSTRAP";

pub async fn apply_test_bootstrap(state: &Arc<HttpState>) -> Result<(), MemoryError> {
    let Some(raw) = std::env::var(ENV_TEST_BOOTSTRAP)
        .ok()
        .filter(|v| !v.trim().is_empty())
    else {
        return Ok(());
    };
    for entry in raw.split(',') {
        let (name, key) = entry.split_once('=').ok_or_else(|| {
            MemoryError::ConfigInvalid("bootstrap entry must be <name>=<api_key>".into())
        })?;
        let cred = ApiKeyCredential::parse(key)?;
        bootstrap_one(state, name, &cred).await?;
    }
    Ok(())
}

/// Idempotently provision one ready tenant:
/// 1. Account (active) + Tenant (`Ready`, deterministic binding
///    `tns_<name>` / database `memory` — safe identifier charset, re-runs
///    are no-ops) + ApiKey (verifier computed with the configured pepper).
/// 2. Ensure the tenant namespace exists and is migrated, synchronously,
///    reusing the Task 5.2 `ensure_namespace` + Task 5.3 migration path
///    over `state.registry.tenant_engine()`.
async fn bootstrap_one(
    state: &Arc<HttpState>,
    name: &str,
    cred: &ApiKeyCredential,
) -> Result<(), MemoryError> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(MemoryError::ConfigInvalid(
            "test bootstrap account name must be alphanumeric/underscore".into(),
        ));
    }
    // Stable fixture IDs make repeated process starts idempotent without
    // exposing a production account-creation API. The digest is an internal
    // test identifier, not a tenant selector accepted by MCP.
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(name.as_bytes());
    let suffix = hex::encode(&digest[..8]);
    let account_id = format!("acct_test_{suffix}");
    let tenant_id = format!("ten_test_{suffix}");
    let tenant_namespace = format!("tns_test_{suffix}");
    let now = chrono::Utc::now();
    let store = state.registry.store_clone();
    let account = Account {
        id: account_id.clone(),
        status: AccountStatus::Active,
        tenant_id: tenant_id.clone(),
        created_at: now,
    };
    let tenant = Tenant {
        id: tenant_id.clone(),
        status: TenantStatus::Reserved,
        namespace_binding: NamespaceBinding {
            namespace: tenant_namespace,
            database: "memory".into(),
        },
        plan_version: 1,
        schema_version: 0,
        retry_stage: None,
        provisioning_lease: None,
        created_at: now,
        version: 0,
    };
    let api_key = ApiKey {
        id: cred.key_id().to_string(),
        account_id,
        name: format!("test-bootstrap-{name}"),
        verifier: KeyedVerifier::compute(
            state.config.api_key_pepper.as_bytes(),
            cred.secret(),
        ),
        status: ApiKeyStatus::Active,
        created_at: now,
        expires_at: None,
        last_used_at: None,
        version: 0,
    };
    store.write_account(&account).await?;
    store.write_tenant(&tenant).await?;
    store.write_api_key(&api_key).await?;

    let current = store.find_tenant_by_id(&tenant.id).await?
        .ok_or_else(|| MemoryError::NotFound("bootstrap tenant".into()))?;
    if current.status != TenantStatus::Ready {
        crate::http::leases::migration::provision_one(&state.registry, &tenant.id).await?;
    }
    Ok(())
}
```

Without the `test-fixtures` feature the env var is REJECTED at startup —
add to `HttpConfig::validate` (Task 3.1):

```rust
#[cfg(not(feature = "test-fixtures"))]
if std::env::var("MEMORY_MCP_HTTP_TEST_BOOTSTRAP").ok().is_some() {
    return Err(MemoryError::ConfigInvalid(
        "MEMORY_MCP_HTTP_TEST_BOOTSTRAP is only valid with the test-fixtures feature".into(),
    ));
}
```

The literal is intentional: this validation compiles without the
`test_bootstrap` module, so a production build cannot accidentally gain the
bootstrap implementation by name resolution.

- [ ] **Step 2: Wire into the binary**

In `memory_mcp_http.rs`, after `HttpState::new(…)` succeeds and before
`serve`:

```rust
#[cfg(feature = "test-fixtures")]
if let Err(err) = memory_mcp::http::test_bootstrap::apply_test_bootstrap(&state).await {
    eprintln!("test bootstrap error: {err}");
    return ExitCode::from(2);
}
```

- [ ] **Step 3: Update the conformance suite**

In `tests/http_proto_conformance.rs`:

1. `base_env` gains a fixed bootstrap account:

```rust
/// Fixed bootstrap key for the suite (well-formed per Task 4.3).
const BOOTSTRAP_KEY: &str =
    "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_conformancesuite0123456789abcdef";

// inside base_env(port):
("MEMORY_MCP_HTTP_TEST_BOOTSTRAP".into(), format!("conformance={BOOTSTRAP_KEY}")),
```

2. Every `POST /mcp` request gains the Bearer header:

```rust
.header("authorization", format!("Bearer {BOOTSTRAP_KEY}"))
```

The GET/DELETE 405 tests do NOT need it — the method check
(`reject_non_post_mcp`) runs before route-scoped auth.

- [ ] **Step 4: Run**

Run:

```bash
cargo test -p memory_mcp --features streamable-http,test-fixtures --test http_proto_conformance
```

Expected: PASS — the suite is green again after the 401 window opened by
Task 4.6.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/http/ crates/memory-mcp/src/bin/memory_mcp_http.rs crates/memory-mcp/tests/http_proto_conformance.rs
git commit -m "feat(http): test-fixtures bootstrap + authenticated conformance suite"
```

---

## Phase 6: Leases, scheduler, migration compatibility, quotas

### Task 6.1: Lease primitive

**Files:**
- Modify: `crates/memory-mcp/src/http/leases/mod.rs` (created in Task 5.3; keep `pub mod migration;`)

- [ ] **Step 1: Lease record**

```rust
use chrono::{DateTime, Utc};
use crate::error::MemoryError;

pub struct LeaseRecord {
    pub owner_id: String,    // replica id
    pub lease_id: String,     // uuid
    pub expires_at: DateTime<Utc>,
    pub fencing_generation: u64,
    pub heartbeat_at: DateTime<Utc>,
}

/// Lease token passed into provisioning. It is intentionally not constructible
/// by a request handler; only an atomic registry claim returns it.
#[derive(Debug, Clone)]
pub struct ProvisioningLease {
    pub owner_id: String,
    pub lease_id: String,
    pub fencing_generation: u64,
    pub expires_at: DateTime<Utc>,
}

impl ProvisioningLease {
    pub async fn heartbeat(
        &self,
        store: &dyn crate::http::registry::RegistryStore,
        tenant_id: &str,
        now: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        store.heartbeat_provisioning(
            tenant_id, &self.owner_id, &self.lease_id,
            self.fencing_generation, now, expires_at,
        ).await
    }

    pub async fn release(
        &self,
        store: &dyn crate::http::registry::RegistryStore,
        tenant_id: &str,
    ) -> Result<(), MemoryError> {
        store.release_provisioning(
            tenant_id, &self.owner_id, &self.lease_id, self.fencing_generation,
        ).await
    }

    pub async fn run_with_heartbeat<T, F>(
        &self,
        registry: crate::http::registry::RegistryHandle,
        tenant_id: &str,
        work: F,
    ) -> Result<T, MemoryError>
    where
        T: Send + 'static,
        F: std::future::Future<Output = Result<T, MemoryError>> + Send,
    {
        // The heartbeat is tracked and joined. A lost lease cancels the work
        // future; all final writes still carry the fencing token as defense in
        // depth against a race between heartbeat and commit.
        let store = registry.store_clone();
        let tenant_id = tenant_id.to_owned();
        let heartbeat_cancel = tokio_util::sync::CancellationToken::new();
        let (lost_tx, mut lost_rx) = tokio::sync::oneshot::channel();
        let lease = self.clone();
        let cancel = heartbeat_cancel.clone();
        let heartbeat = tokio::spawn(async move {
            let mut first = true;
            loop {
                let delay = if first {
                    std::time::Duration::ZERO
                } else {
                    // 16–24 seconds: lease_ttl / 3 with ±20% jitter.
                    std::time::Duration::from_secs(16 + u64::from(rand::random::<u8>() % 9))
                };
                first = false;
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(delay) => {
                        let now = chrono::Utc::now();
                        let expiry = now + chrono::Duration::seconds(60);
                        if lease.heartbeat(store.as_ref(), &tenant_id, now, expiry).await.is_err() {
                            let _ = lost_tx.send(());
                            break;
                        }
                    }
                }
            }
        });
        let result = tokio::select! {
            result = work => result,
            _ = &mut lost_rx => Err(MemoryError::Conflict("provisioning lease lost".into())),
        };
        heartbeat_cancel.cancel();
        let _ = heartbeat.await;
        result
    }
}
```

The tenant id is passed explicitly to `run_with_heartbeat`; it is never derived
by parsing the opaque lease id. The lease token is returned only by an atomic
registry claim, so a request handler cannot manufacture a valid fencing token.

- [ ] **Step 2: CAS on fencing generation**

```rust
use chrono::{DateTime, Utc};
use crate::error::MemoryError;

/// A closed set of lease mutations; callers cannot inject arbitrary SurrealQL
/// through the fence helper.
#[derive(Debug, Clone)]
pub enum FenceUpdate {
    Claim {
        owner_id: String,
        lease_id: String,
        lease_expiry: DateTime<Utc>,
    },
    Heartbeat {
        owner_id: String,
        lease_id: String,
        lease_expiry: DateTime<Utc>,
        heartbeat_at: DateTime<Utc>,
    },
    Release { owner_id: String, lease_id: String },
}

pub async fn commit_with_fence(
    client: &std::sync::Arc<dyn crate::storage::client::DbClient>,
    namespace: &str,
    record_id: &str,
    expected_generation: u64,
    update: FenceUpdate,
) -> Result<u64, MemoryError> {
    // record_id is server-generated (safe identifier charset) — validate
    // before interpolating; everything else goes through $params.
    if record_id.is_empty() || !record_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':') {
        return Err(MemoryError::Validation(format!("unsafe record id: {record_id}")));
    }
    // Bind the expected and next fence tokens HERE — callers cannot supply or
    // override either generation through an interpolated clause.
    let mut params = serde_json::Map::new();
    params.insert("expected_gen".to_string(), serde_json::Value::from(expected_generation));
    let (set_clause, owner_clause, update_vars, returned_generation) = match update {
        FenceUpdate::Claim { owner_id, lease_id, lease_expiry } => {
            let next_generation = expected_generation
                .checked_add(1)
                .ok_or_else(|| MemoryError::Conflict("fencing generation overflow".into()))?;
            (
                "lease_owner = $owner_id, lease_id = $lease_id, lease_expiry = $lease_expiry, lease_generation = $next_gen",
                "(lease_expiry IS NONE OR lease_expiry < time::now())",
                serde_json::json!({
                    "owner_id": owner_id,
                    "lease_id": lease_id,
                    "lease_expiry": lease_expiry,
                    "next_gen": next_generation,
                }),
                next_generation,
            )
        },
        FenceUpdate::Heartbeat { owner_id, lease_id, lease_expiry, heartbeat_at } => (
            "lease_expiry = $lease_expiry, heartbeat_at = $heartbeat_at",
            "lease_owner = $owner_id AND lease_id = $lease_id",
            serde_json::json!({
                "owner_id": owner_id,
                "lease_id": lease_id,
                "lease_expiry": lease_expiry,
                "heartbeat_at": heartbeat_at,
            }),
            expected_generation,
        ),
        FenceUpdate::Release { owner_id, lease_id } => (
            "lease_owner = NONE, lease_id = NONE, lease_expiry = NONE",
            "lease_owner = $owner_id AND lease_id = $lease_id",
            serde_json::json!({
                "owner_id": owner_id,
                "lease_id": lease_id,
            }),
            expected_generation,
        ),
    };
    if let serde_json::Value::Object(update_vars) = update_vars {
        params.extend(update_vars);
    }
    // A claim increments the generation; heartbeat/release preserve it. The
    // owner/lease predicates prevent stale workers from heartbeating or
    // releasing a lease taken over by another replica.
    let sql = format!(
        "UPDATE {record_id} SET {set_clause} WHERE lease_generation = $expected_gen AND {owner_clause} RETURN AFTER;"
    );
    let result = client
        .query(&sql, Some(serde_json::Value::Object(params)), namespace)
        .await?;
    let rows: Vec<serde_json::Value> = serde_json::from_value(result)
        .map_err(|err| MemoryError::Storage(format!("fence commit result: {err}")))?;
    match rows.first() {
        Some(row) => {
            let gen = row
                .get("lease_generation")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| MemoryError::Storage("fence commit returned no generation".into()))?;
            if gen != returned_generation {
                return Err(MemoryError::Conflict(format!(
                    "fencing generation mismatch: expected {returned_generation}, found {gen}"
                )));
            }
            Ok(gen)
        }
        None => Err(MemoryError::Conflict(
            "fence commit matched no rows (lease lost)".into(),
        )),
    }
}
```

- [ ] **Step 3: Test**

```rust
#[tokio::test]
async fn stale_fenced_worker_cannot_commit() { /* spec §20.2 */ }
```

- [ ] **Step 4: Commit**

```bash
git add crates/memory-mcp/src/http/leases/
git commit -m "feat(leases): datastore-time lease with fencing generation"
```

### Task 6.2: Tier-1 scheduler loop (ADR-0046)

**Files:**
- Create: `crates/memory-mcp/src/http/leases/scheduler.rs`
- Modify: `crates/memory-mcp/src/http/leases/mod.rs` (register `pub mod scheduler;`)
- Modify: `crates/memory-mcp/src/http/leases/migration.rs` (bounded due-provisioning job)
- Modify: `crates/memory-mcp/src/bin/memory_mcp_http.rs` (tracked scheduler lifecycle)

- [ ] **Step 1: Implement the tracked scheduler and explicit job seam**

Process-level loop. Each cycle discovers due work through the registry. Each
job is responsible for acquiring a datastore-time lease, heartbeating with
jitter while its bounded pass runs, verifying the fencing generation before
committing, and releasing only its own lease. App Session cleanup, Task retry,
and subscription/outbox jobs are registered by Tasks 7–9; the provisioning job
is registered here. There is no empty/default scheduler: constructing hooks
with an empty job list returns a configuration error.

Use a tracked `JoinSet`; a scheduler job may not spawn detached correctness-
sensitive work:

```rust
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinSet;
use tokio::sync::Semaphore;

use crate::error::MemoryError;
use crate::http::registry::RegistryHandle;

pub type JobFuture = Pin<Box<dyn Future<Output = Result<(), MemoryError>> + Send>>;
pub type SchedulerJob = Arc<dyn Fn(RegistryHandle) -> JobFuture + Send + Sync>;

#[derive(Clone)]
pub struct SchedulerHooks {
    jobs: Arc<Vec<SchedulerJob>>,
    maintenance_parallelism: usize,
}

impl SchedulerHooks {
    pub fn new(jobs: Vec<SchedulerJob>, maintenance_parallelism: usize) -> Result<Self, MemoryError> {
        if jobs.is_empty() || maintenance_parallelism == 0 {
            return Err(MemoryError::ConfigInvalid(
                "scheduler requires at least one job and positive parallelism".into(),
            ));
        }
        Ok(Self { jobs: Arc::new(jobs), maintenance_parallelism })
    }

    pub fn with_provisioning_only() -> Result<Self, MemoryError> {
        Self::new(vec![Arc::new(|registry| {
            Box::pin(crate::http::leases::migration::run_due_provisioning(registry))
        })], 4)
    }

    /// Tasks 7–9 call this before the binary starts serving to add their
    /// cleanup/retry/outbox jobs. The returned value is immutable thereafter.
    pub fn with_additional_job(mut self, job: SchedulerJob) -> Self {
        Arc::make_mut(&mut self.jobs).push(job);
        self
    }
}

pub struct SchedulerHandle {
    join: tokio::task::JoinHandle<()>,
}

pub fn start(
    registry: RegistryHandle,
    hooks: SchedulerHooks,
    shutdown: tokio_util::sync::CancellationToken,
) -> SchedulerHandle {
    let join = tokio::spawn(run_scheduler(registry, hooks, shutdown));
    SchedulerHandle { join }
}

impl SchedulerHandle {
    pub async fn join(self) {
        if let Err(error) = self.join.await {
            tracing::error!(target: "memory_mcp::http::scheduler", %error, "scheduler task failed");
        }
    }
}

async fn run_scheduler(
    registry: RegistryHandle,
    hooks: SchedulerHooks,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = ticker.tick() => run_cycle(registry.clone(), &hooks, shutdown.clone()).await,
        }
    }
}

async fn run_cycle(
    registry: RegistryHandle,
    hooks: &SchedulerHooks,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let semaphore = Arc::new(Semaphore::new(hooks.maintenance_parallelism));
    let mut jobs = JoinSet::new();
    for job in hooks.jobs.iter().cloned() {
        let semaphore = semaphore.clone();
        let registry = registry.clone();
        let shutdown = shutdown.clone();
        jobs.spawn(async move {
            let permit = tokio::select! {
                _ = shutdown.cancelled() => return,
                permit = semaphore.acquire_owned() => match permit {
                    Ok(permit) => permit,
                    Err(_) => return,
                },
            };
            let _permit = permit;
            if let Err(error) = job(registry).await {
                tracing::warn!(target: "memory_mcp::http::scheduler", %error, "scheduled job failed");
            }
        });
    }
    while let Some(result) = jobs.join_next().await {
        if let Err(error) = result {
            tracing::error!(target: "memory_mcp::http::scheduler", %error, "scheduled job panicked");
        }
    }
}
```

Add `run_due_provisioning(registry: RegistryHandle) -> impl Future<Output =
Result<(), MemoryError>>` in `leases/migration.rs`. It must use the following
bounded algorithm: call `list_due_provisioning(100, Utc::now())`; for each row,
generate a fresh server-side lease id; call `claim_provisioning` with the
replica id and a 60-second expiry; skip `None` claims; call
`provision_one(&registry, &tenant.id, lease)` for a successful claim; record a
bounded warning/metric for a conflict; and never mutate a Tenant directly from
the scheduler. `provision_one` heartbeats at a jittered `lease_ttl / 3` during
remote migration, commits every transition/schema update with the lease
generation, and releases only when owner and lease id still match. A lost lease
returns a conflict and never marks the Tenant `Ready`. Tasks 7–9 add their jobs
through `with_additional_job`; the scheduler framework itself remains
unchanged.

- [ ] **Step 2: Wire and test lifecycle**

In the binary, after `HttpState::new` and before `server::serve`, construct the
non-empty hooks, start the handle, and after `serve` returns close admission,
cancel the state token, then await `scheduler.join()`:

```rust
let shutdown = state.shutdown.clone();
let admission = state.admission.clone();
let hooks = match memory_mcp::http::leases::scheduler::SchedulerHooks::with_provisioning_only() {
    Ok(hooks) => hooks,
    Err(error) => {
        eprintln!("scheduler config error: {error}");
        return ExitCode::from(2);
    }
};
let scheduler = memory_mcp::http::leases::scheduler::start(
    state.registry.clone(),
    hooks,
    shutdown.token(),
);
let router = memory_mcp::http::router::build_router(state);
let server_result = memory_mcp::http::server::serve(cfg, router, shutdown.clone()).await;
admission.close();
shutdown.begin();
scheduler.join().await;
if let Err(error) = server_result {
    eprintln!("server error: {error}");
    return ExitCode::FAILURE;
}
return ExitCode::SUCCESS;
```

This replaces the earlier direct `serve` call in the binary. It uses explicit
`ExitCode` branches because the composition root returns `ExitCode`; do not
change `main.rs` into a business-logic module. Test both that
shutdown causes `join()` to return and that a long-running job is joined rather
than detached.

```rust
#[tokio::test]
async fn scheduler_advances_due_work_and_skips_idle() {
    let registry = test_registry_handle().await;
    let runs = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let runs_for_job = runs.clone();
    let hooks = SchedulerHooks::new(vec![std::sync::Arc::new(move |_registry| {
        let runs = runs_for_job.clone();
        Box::pin(async move {
            runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        })
    })], 1).expect("non-empty hooks");
    let shutdown = tokio_util::sync::CancellationToken::new();
    let handle = start(registry, hooks, shutdown.clone());
    tokio::time::sleep(Duration::from_millis(1100)).await;
    shutdown.cancel();
    handle.join().await;
    assert!(runs.load(std::sync::atomic::Ordering::SeqCst) >= 1);
}

#[tokio::test]
async fn empty_scheduler_hooks_are_rejected() {
    assert!(SchedulerHooks::new(Vec::new(), 1).is_err());
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/memory-mcp/src/http/leases/ crates/memory-mcp/src/bin/memory_mcp_http.rs
git commit -m "feat(scheduler): tracked tier-1 process scheduler (ADR-0046)"
```

### Task 6.3: Rolling N/N-1 schema compatibility

**Files:**
- Modify: `crates/memory-mcp/src/http/leases/migration.rs`

- [ ] **Step 1: Replica min/max schema version**

Each replica advertises `[min_schema_version, max_schema_version]` via an
inclusive range. Use a concrete current schema constant rather than an
undefined `N` and use the correct Rust type:

```rust
pub const CURRENT_SCHEMA_VERSION: u32 = 30;
pub const REPLICA_SCHEMA_RANGE: std::ops::RangeInclusive<u32> =
    (CURRENT_SCHEMA_VERSION - 1)..=CURRENT_SCHEMA_VERSION;
```

The scheduler migrates Tenants only if their `schema_version` falls in this
range; the value is bumped only when a new append-only migration is shipped.

- [ ] **Step 2: Tenant outside range**

If a Tenant is outside the range, scheduler returns `Err(MemoryError::Unavailable("schema incompatible"))`. Data plane returns 503 stable.

- [ ] **Step 3: Tests**

```rust
#[tokio::test]
async fn replica_skips_tenant_outside_schema_range() { /* ... */ }
#[tokio::test]
async fn migration_after_compatible_roll_marks_tenant_ready() { /* ... */ }
```

- [ ] **Step 4: Commit**

```bash
git add crates/memory-mcp/src/http/leases/migration.rs
git commit -m "feat(scheduler): rolling N/N-1 schema compatibility"
```

### Task 6.4: Quota/plan integration

**Files:**
- Modify: `crates/memory-mcp/src/http/registry/plan.rs`

- [ ] **Step 1: Implement quota enforcement points**

Wired at:
- `ingest`: durable counter increment on `usage_counter`; reject with stable HTTP `429` plus retry/guidance metadata if limit exceeded.
- `extract`: enforce `extraction_concurrency`.
- `open_app`: enforce `max_open_app_sessions`.
- API key create: enforce `max_active_api_keys`.
- Request concurrency: pool enforces `per_tenant_request_concurrency` independently.

- [ ] **Step 2: Reconciler**

Periodic task runs `select count(*) ...` over each table and rewrites the durable counter if drift exceeds a threshold (spec §12). Counter remains the authoritative admission gate; reconciler repairs drift.

- [ ] **Step 3: Tests**

```rust
#[tokio::test]
async fn quota_exceeded_rejects_ingest_with_retry_guidance() { /* ... */ }
#[tokio::test]
async fn reconciler_repairs_drift_after_concurrent_writers() { /* spec §20.3 */ }
```

- [ ] **Step 4: Commit**

```bash
git add crates/memory-mcp/src/http/registry/plan.rs
git commit -m "feat(quota): plan limits + reconciler"
```

### Task 6.5: Startup config validation

**Files:**
- Modify: `crates/memory-mcp/src/http/server.rs`

- [ ] **Step 1: Validate at startup**

Already partially done in `HttpConfig::validate`. Extend to reject:

- `fs-watch` enabled (check `SURREALDB_FS_WATCH_INBOX` set) — spec §13.
- Open signup without quotas (already covered).
- Missing required security values.
- Wildcard production origin (already covered).

- [ ] **Step 2: Tests**

```rust
#[test] fn rejects_fs_watch_env_in_http_mode() { /* ... */ }
#[test] fn rejects_open_signup_without_quotas() { /* ... */ }
```

- [ ] **Step 3: Commit**

```bash
git add crates/memory-mcp/src/http/
git commit -m "feat(http): startup config validation (fs-watch forbidden, etc.)"
```

---

## Phase 7: Durable App Sessions

### Task 7.1: App session schema

**Files:**
- Create: `crates/memory-mcp/src/http/app_sessions/mod.rs`
- Create: `crates/memory-mcp/src/http/app_sessions/store.rs`
- Create: `crates/memory-mcp/src/http/app_sessions/scheduler.rs`
- Modify: `crates/memory-mcp/src/http/mod.rs` (register `pub mod app_sessions;`)
- Modify: `crates/memory-mcp/src/bin/memory_mcp_http.rs` (register the cleanup job)

At the start of this task:

```rust
// http/app_sessions/mod.rs
pub mod store;
pub mod scheduler;
```

- [ ] **Step 1: Tenant-bound schema**

Migrations are added to the tenant migrations (`storage/migrations.rs`) under a new optional script that runs only when the control plane indicates a SaaS tenant. The simpler path for v1: create the table inside the **provisioning** step of the new tenant (`migrating` state). Add a migration `040_app_sessions.surql` and gate its application on a `feature_saas` flag carried in the registry binding.

```surql
DEFINE TABLE IF NOT EXISTS app_session SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS handle ON app_session TYPE string;
DEFINE FIELD IF NOT EXISTS tenant_id ON app_session TYPE string;
DEFINE FIELD IF NOT EXISTS app ON app_session TYPE string;
DEFINE FIELD IF NOT EXISTS version ON app_session TYPE int;
DEFINE FIELD IF NOT EXISTS payload ON app_session TYPE object;
DEFINE FIELD IF NOT EXISTS idle_expiry ON app_session TYPE datetime;
DEFINE FIELD IF NOT EXISTS absolute_expiry ON app_session TYPE datetime;
DEFINE INDEX IF NOT EXISTS idx_app_session_handle ON app_session FIELDS handle UNIQUE;
```

- [ ] **Step 2: Tests**

```rust
#[tokio::test]
async fn app_session_table_is_present_after_provisioning() { /* ... */ }
```

### Task 7.2: Open/close/optimistic-versioning handlers

**Files:**
- Modify: `crates/memory-mcp/src/http/app_sessions/store.rs`

- [ ] **Step 1: Implement store**

```rust
pub struct AppSessionStore {
    db: std::sync::Arc<BoundDbClient>,
}

impl AppSessionStore {
    pub fn new(db: std::sync::Arc<BoundDbClient>) -> Self { Self { db } }

    pub async fn open(&self, tenant_id: &str, app: &str, payload: Value) -> Result<(String, u64), MemoryError> {
        // In one transaction count this Tenant's non-expired sessions; reject
        // before insert when count >= 32. Generate a random opaque handle,
        // store only the tenant-local record, set idle_expiry to now + 30 min
        // and absolute_expiry to now + 24 h, and return (handle, version=1).
    }

    pub async fn command(&self, handle: &str, expected_version: u64, mutation: Value) -> Result<u64, MemoryError> {
        // Optimistic: UPDATE WHERE version = $expected RETURN version; on a
        // successful command set idle_expiry = min(now + 30 minutes,
        // absolute_expiry). If returned version != expected_version+1, return
        // MemoryError::Conflict and do not mutate payload or expiry.
    }

    pub async fn close(&self, handle: &str) -> Result<(), MemoryError> {
        self.db.query(
            "DELETE FROM app_session WHERE handle = $handle",
            Some(serde_json::json!({"handle": handle})),
        ).await.map(|_| ())
    }
}
```

Add a backend seam in `mcp/handlers.rs` next to the existing in-process
`SessionManager`:

```rust
#[cfg(feature = "mcp-apps")]
#[derive(Clone)]
pub enum AppSessionBackend {
    InMemory(SessionManager),
    #[cfg(feature = "streamable-http")]
    Durable(std::sync::Arc<crate::http::app_sessions::store::AppSessionStore>),
}

#[cfg(all(feature = "mcp-apps", feature = "streamable-http"))]
impl MemoryMcp {
    pub fn with_durable_app_sessions(
        mut self,
        store: std::sync::Arc<crate::http::app_sessions::store::AppSessionStore>,
    ) -> Self {
        self.app_sessions = AppSessionBackend::Durable(store);
        self
    }
}
```

Change the existing `session_manager` field to `app_sessions:
AppSessionBackend`; `MemoryMcp::new` initializes `InMemory(SessionManager::new())`.
When `streamable-http` is absent, no `Durable` enum variant or HTTP import is
compiled, preserving the existing stdio-only feature combination.
The existing stdio methods keep using the in-memory branch. HTTP `open_app` and
`app_command` use the durable branch and map version conflicts to the existing
MCP conflict/guidance envelope; they never fall back to in-memory state.

- [ ] **Step 2: Hook into MCP `open_app` / `app_command`

In `mcp/handlers.rs`, when the HTTP profile is active, dispatch `open_app`,
`app_command`, and App resource reads to the AppSessionStore of the acquired
Tenant Runtime rather than the existing in-process store. Every durable lookup
includes the runtime's immutable Tenant binding plus the opaque handle; a handle
from another Tenant returns not-found without revealing ownership. Resource
reads must reauthorize the current principal and enforce both idle and absolute
expiry. The durable branch never falls back to process-local state.

- [ ] **Step 3: Tests**

```rust
#[tokio::test]
async fn open_app_returns_handle_and_initial_version() { /* spec §9 */ }
#[tokio::test]
async fn app_command_with_stale_version_returns_conflict() { /* spec §9 */ }
#[tokio::test]
async fn app_session_count_per_tenant_is_capped_at_32() { /* spec §9 */ }
```

- [ ] **Step 4: Scheduler cleanup**

Add `app_sessions/scheduler.rs` and register its job in the binary's hook
construction. The job is a process-level bounded pass, not a per-Tenant loop:

```rust
pub fn scheduler_job() -> crate::http::leases::scheduler::SchedulerJob {
    std::sync::Arc::new(|registry| {
        Box::pin(async move {
            crate::http::app_sessions::store::cleanup_expired_for_all(&registry).await
        })
    })
}
```

Implement `pub async fn cleanup_expired_for_all(registry: &RegistryHandle) -> Result<(), MemoryError>` by enumerating at most 100 ready Tenants, binding each
namespace through the privileged maintenance factory, and issuing a
parameterized `DELETE FROM app_session WHERE idle_expiry < time::now() OR
absolute_expiry < time::now()`. Physical deletion is allowed only for this
TTL-governed ephemeral table; it never touches facts or registry history.

The binary changes `with_provisioning_only()` to
`with_provisioning_only()?.with_additional_job(app_sessions::scheduler_job())`
before `start`. The job must be bounded by the scheduler maintenance budget and
must not hold a full runtime pin while iterating tenants.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/http/app_sessions/ crates/memory-mcp/src/mcp/handlers.rs crates/memory-mcp/src/storage/migrations.rs
git commit -m "feat(app-sessions): durable HTTP App Sessions with optimistic versioning"
```

---

## Phase 8: Durable extraction Tasks

### Task 8.1: Task record schema

**Files:**
- Create: `crates/memory-mcp/src/http/tasks/state.rs`
- Create: `crates/memory-mcp/src/http/tasks/mod.rs`
- Modify: `crates/memory-mcp/src/http/mod.rs` (register `pub mod tasks;`)

At the start of this task:

```rust
// http/tasks/mod.rs
pub mod state;
```

- [ ] **Step 1: Schema migration**

Add `041_tenant_tasks.surql` to the SaaS-gated migrations.

```surql
DEFINE TABLE IF NOT EXISTS tenant_task SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS id ON tenant_task TYPE string;
DEFINE FIELD IF NOT EXISTS tenant_id ON tenant_task TYPE string;
DEFINE FIELD IF NOT EXISTS fingerprint ON tenant_task TYPE string;
DEFINE FIELD IF NOT EXISTS state ON tenant_task TYPE string;
DEFINE FIELD IF NOT EXISTS version ON tenant_task TYPE int;
DEFINE FIELD IF NOT EXISTS lease_owner ON tenant_task TYPE option<string>;
DEFINE FIELD IF NOT EXISTS lease_generation ON tenant_task TYPE option<int>;
DEFINE FIELD IF NOT EXISTS lease_expiry ON tenant_task TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS cancellation_intent ON tenant_task TYPE option<bool>;
DEFINE FIELD IF NOT EXISTS progress ON tenant_task TYPE option<object>;
DEFINE FIELD IF NOT EXISTS result ON tenant_task TYPE option<object>;
DEFINE FIELD IF NOT EXISTS error ON tenant_task TYPE option<object>;
DEFINE FIELD IF NOT EXISTS created_at ON tenant_task TYPE datetime;
DEFINE FIELD IF NOT EXISTS updated_at ON tenant_task TYPE datetime;
DEFINE FIELD IF NOT EXISTS retention_expiry ON tenant_task TYPE datetime;
DEFINE INDEX IF NOT EXISTS idx_tenant_task_state ON tenant_task FIELDS state;
DEFINE INDEX IF NOT EXISTS idx_tenant_task_fingerprint ON tenant_task FIELDS fingerprint;
```

The Rust projection is added with the state machine in Task 8.2, after
`TaskState` exists.

- [ ] **Step 2: Commit**

```bash
git add crates/memory-mcp/src/storage/migrations.rs crates/memory-mcp/src/http/tasks/
git commit -m "feat(tasks): durable Tenant Task record schema"
```

### Task 8.2: State machine + version

**Files:**
- Modify: `crates/memory-mcp/src/http/tasks/state.rs`

- [ ] **Step 1: States**

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskState { Queued, Running, Completed, CompletedBeforeCancel, CancelRequested, Cancelled, CancelledBeforeCommit, Failed }

pub fn is_terminal(s: TaskState) -> bool {
    matches!(s, TaskState::Completed | TaskState::CompletedBeforeCancel | TaskState::Cancelled | TaskState::CancelledBeforeCommit | TaskState::Failed)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TenantTaskRecord {
    pub id: String,
    pub tenant_id: String,
    pub fingerprint: String,
    pub state: TaskState,
    pub version: u64,
    pub cancellation_intent: bool,
    pub lease_owner: Option<String>,
    pub lease_generation: Option<u64>,
    pub lease_expiry: Option<chrono::DateTime<chrono::Utc>>,
    pub progress: Option<serde_json::Value>,
    pub result: Option<serde_json::Value>,
    pub error: Option<serde_json::Value>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub retention_expiry: chrono::DateTime<chrono::Utc>,
}

/// Task lease identity shared by the worker and the durable TaskStore trait.
#[derive(Debug, Clone)]
pub struct TaskHandle {
    pub tenant_id: String,
    pub task_id: String,
    pub lease_owner: String,
    pub lease_generation: u64,
    pub lease_expiry: chrono::DateTime<chrono::Utc>,
}
```

- [ ] **Step 2: Tests**

```rust
#[test] fn only_terminal_states_are_terminal() { /* spec §10.2 */ }
#[test] fn cancelled_before_commit_is_distinct_from_completed_before_cancel() { /* ... */ }
```

### Task 8.3: Fenced worker

**Files:**
- Create: `crates/memory-mcp/src/http/tasks/worker.rs`
- Create: `crates/memory-mcp/src/http/tasks/scheduler.rs`
- Modify: `crates/memory-mcp/src/http/tasks/mod.rs` (add `pub mod worker; pub mod scheduler;`)
- Modify: `crates/memory-mcp/src/bin/memory_mcp_http.rs` (register retry/reconcile/retention job)

- [ ] **Step 1: Lease acquisition with monotonic generation**

```rust
use super::state::TaskHandle;
```

The worker function signature is `pub async fn claim_next_due(store: &dyn TaskStore, tenant_id: &str, replica_id: &str) -> Result<Option<TaskHandle>, MemoryError>`. Implement the signature with one Tenant-local CAS: select a `queued` task or a
`running` task whose lease has expired, increment its generation, set owner/id/
expiry, and return the resulting `TaskHandle`. A claim that matches no row
returns `Ok(None)`; it never creates a second task for the same fingerprint.

- [ ] **Step 2: Commit with CAS**

Every write to a Task record must verify `lease_generation = current`.

- [ ] **Step 3: Cancellation as intent**

The cancellation function signature is `pub async fn cancel(store: &dyn TaskStore, tenant_id: &str, task_id: &str) -> Result<(), MemoryError>`. It sets intent with a Tenant-local CAS.

If state is `Running`, the worker observes `cancellation_intent` and transitions to `CancelRequested` → `CancelledBeforeCommit` (no rollback of facts).

If state is `Queued`, transition to `Cancelled`.

- [ ] **Step 4: Reconciliation**

When extraction writes and Task terminal state diverge (different transactions), a reconciler derives the terminal outcome from durable artifacts + fingerprint. This is the spec §10.2 atomicity boundary.

- [ ] **Step 5: Tests**

```rust
#[tokio::test]
async fn stale_fenced_worker_cannot_transition_terminal_state() { /* spec §10.2 */ }
#[tokio::test]
async fn cancel_during_running_does_not_rollback_committed_facts() { /* spec §10.2 */ }
#[tokio::test]
async fn reconciler_recovers_terminal_outcome_from_artifacts() { /* spec §10.2 */ }
```

- [ ] **Step 6: Commit**

```bash
git add crates/memory-mcp/src/http/tasks/
git commit -m "feat(tasks): fenced worker + cancellation intent + reconciliation"
```

### Task 8.4: Durable Task backend behind the `ServerHandler` seam

**Files:**
- Modify: `crates/memory-mcp/src/mcp/handlers.rs`
- Modify: `crates/memory-mcp/src/http/tasks/state.rs` (`TaskStore` trait)
- Modify: `crates/memory-mcp/src/http/runtime/storage.rs` (`build_runtime` wiring)

**Verified against rmcp 3.1.2 source:** `rmcp::task_manager::TaskManager` is a
CONCRETE in-memory struct (`pub struct TaskManager`, `src/task_manager.rs`)
— there is no trait to implement, so an earlier draft's
`impl rmcp::TaskManager for …` cannot compile. The durable integration point
is the `ServerHandler` methods this codebase ALREADY overrides (`get_task`,
`update_task`, `cancel_task` in `mcp/handlers.rs`) plus the `call_tool`
extract path. rmcp stays the protocol adapter; the durable record is the
source of truth (ADR-0052).

- [ ] **Step 1: `TaskStore` trait**

In `http/tasks/state.rs`, next to `TaskState`:

```rust
use serde_json::Value;

use crate::error::MemoryError;

/// Durable Tenant Task seam (spec §10). Implemented by the Task 8.3 fenced
/// worker store over the tenant namespace's `tenant_task` table.
#[async_trait::async_trait]
pub trait TaskStore: Send + Sync + 'static {
    /// Enqueue a new task; returns the task id.
    async fn enqueue(&self, tenant_id: &str, fingerprint: &str, params: Value) -> Result<String, MemoryError>;
    /// Load the durable record (state, version, progress, result, error).
    async fn load(&self, tenant_id: &str, task_id: &str) -> Result<Option<TenantTaskRecord>, MemoryError>;
    /// Set cancellation intent; never deletes (spec §10.2).
    async fn set_cancellation_intent(&self, tenant_id: &str, task_id: &str) -> Result<(), MemoryError>;
    async fn claim_next_due(
        &self,
        tenant_id: &str,
        replica_id: &str,
    ) -> Result<Option<TaskHandle>, MemoryError>;
    async fn update_progress_fenced(
        &self,
        handle: &TaskHandle,
        progress: Value,
    ) -> Result<(), MemoryError>;
    async fn complete_fenced(
        &self,
        handle: &TaskHandle,
        result: Value,
        completed_before_cancel: bool,
    ) -> Result<(), MemoryError>;
    async fn fail_fenced(
        &self,
        handle: &TaskHandle,
        error: Value,
    ) -> Result<(), MemoryError>;
    async fn requeue_expired_running(&self, tenant_id: &str) -> Result<u64, MemoryError>;
    async fn reconcile_artifacts(&self, tenant_id: &str) -> Result<u64, MemoryError>;
    async fn delete_expired(&self, tenant_id: &str) -> Result<u64, MemoryError>;
}
```

(`TenantTaskRecord` is the projection of the Task 8.1 schema: id, state,
version, progress, result, error, cancellation_intent, timestamps.)

- [ ] **Step 2: `TaskBackend` in `mcp/handlers.rs`**

```rust
#[cfg(feature = "streamable-http")]
#[derive(Clone)]
pub enum TaskBackend {
    /// stdio + default: rmcp in-memory manager (frozen behavior, ADR-0038).
    InMemory(rmcp::task_manager::TaskManager),
    /// HTTP SaaS profile: durable Tenant Task records are the source of
    /// truth (ADR-0052).
    Durable(std::sync::Arc<dyn crate::http::tasks::state::TaskStore>),
}
```

`MemoryMcp` field `tasks: TaskManager` becomes a cfg-split field:

```rust
#[cfg(feature = "streamable-http")]
tasks: TaskBackend,
#[cfg(not(feature = "streamable-http"))]
tasks: TaskManager,
```

With `streamable-http`, `MemoryMcp::new` uses
`TaskBackend::InMemory(TaskManager::new())`; without it, `MemoryMcp::new` keeps
the original `TaskManager` field and initialization. The constructor literal
must use matching cfg arms:

```rust
#[cfg(feature = "streamable-http")]
let tasks = TaskBackend::InMemory(TaskManager::new());
#[cfg(not(feature = "streamable-http"))]
let tasks = TaskManager::new();
```

The stdio path remains source- and behavior-compatible for the default feature
matrix. Every `self.tasks` match in `call_tool`, `get_task`, `update_task`, and
`cancel_task` must have the non-HTTP original branch compiled under
`not(feature = "streamable-http")` and the durable/in-memory enum branch under
`feature = "streamable-http"`; do not reference `crate::http` from a stdio-only
build.
- Add a durable-task builder next to `new_modern` (Task 3.4), gated on
  `streamable-http`:

```rust
#[cfg(feature = "streamable-http")]
/// Attach the durable task backend after the Tenant Runtime has been
/// built. This consuming builder preserves the stdio constructor exactly.
pub fn with_durable_tasks(
    mut self,
    tasks: std::sync::Arc<dyn crate::http::tasks::state::TaskStore>,
) -> Self {
    self.tasks = TaskBackend::Durable(tasks);
    self
}
```

- [ ] **Step 3: Dispatch**

- `call_tool` extract path: `InMemory` keeps the existing `spawn` path
  untouched. `Durable` enqueues via `TaskStore::enqueue` and returns
  `CallToolResponse::Task(CreateTaskResult::new(seed))` where
  `seed = rmcp::model::Task::new(task_id, TaskStatus::Working, now, now)`
  (public constructor — verified in rmcp 3.1.2 `model/task.rs`).
- `get_task`: `InMemory` unchanged; `Durable` loads the record and projects
  to `GetTaskResult::new(DetailedTask::new(task, payload))` — both public
  constructors; `TaskPayload` maps from the durable state/result/error.
- `update_task`: `Durable` returns `ErrorData::invalid_params` — v1 durable
  tasks request no inputs (no elicitation, ADR-0052).
- `cancel_task`: `Durable` delegates to `set_cancellation_intent` (cooperative;
  never deletes, never rolls back facts — spec §10.2).

- [ ] **Step 4: Wire into the tenant runtime**

Update `build_runtime` (Task 5.4): construct the Task 7.2 `AppSessionStore`
from `bound_db.clone()` and call `with_durable_app_sessions` before the runtime
is returned. The same runtime-bound `Arc<BoundDbClient>` is reused by all
Tenant-local stores. Then construct the Task 8.3 `TaskStore` over
the runtime's bound client and attach it through the consuming builders:

```rust
let tasks: std::sync::Arc<dyn crate::http::tasks::state::TaskStore> =
    std::sync::Arc::new(crate::http::tasks::worker::DurableTaskStore::new(bound_db.clone()));
let mut mcp = crate::mcp::handlers::MemoryMcp::new_modern(service);
#[cfg(feature = "mcp-apps")]
{
    mcp = mcp.with_durable_app_sessions(std::sync::Arc::new(
        crate::http::app_sessions::store::AppSessionStore::new(bound_db.clone()),
    ));
}
let mcp = mcp.with_durable_tasks(tasks);
```

- [ ] **Step 5: Tests**

```rust
#[tokio::test]
async fn durable_backend_get_task_projects_record_to_rmcp_task() { /* ... */ }
#[tokio::test]
async fn durable_backend_cancel_sets_intent_and_does_not_delete_record() { /* spec §10.1 */ }
#[tokio::test]
async fn stdio_backend_still_uses_in_memory_manager() { /* ADR-0038 freeze */ }
```

- [ ] **Step 6: Commit**

```bash
git add crates/memory-mcp/src/mcp/handlers.rs crates/memory-mcp/src/http/tasks/ crates/memory-mcp/src/http/runtime/storage.rs
git commit -m "feat(tasks): durable Task backend behind the ServerHandler seam"
```

### Task 8.5: `extract` returns Task; `ingest` does not

**Files:**
- Modify: `crates/memory-mcp/src/mcp/handlers.rs`

- [ ] **Step 1: Conditional behavior**

`extract` checks `client_capabilities.tasks`. If present, enqueue a Tenant Task and return the task id. If absent and the work fits in the synchronous bound, run inline; otherwise preflight rejection.

`ingest` always commits synchronously and returns the episode result (spec §10.1, ADR §121). Its retry path is deterministic without a nonstandard header: use the existing stable `source_id` when supplied, otherwise derive a canonical content hash from the normalized source type plus content and enforce a Tenant-local unique dedupe key in the same transaction as episode creation. A duplicate returns the existing episode and does not increment usage twice. Add the dedupe field/index migration if the current episode schema does not already provide this invariant.

- [ ] **Step 2: Tests**

```rust
#[tokio::test]
async fn extract_with_tasks_capability_returns_task_id() { /* spec §10 */ }
#[tokio::test]
async fn extract_without_tasks_capability_runs_synchronously_when_bounded() { /* spec §10 */ }
#[tokio::test]
async fn extract_without_tasks_capability_returns_preflight_rejection_for_large_work() { /* spec §10 */ }
#[tokio::test]
async fn ingest_always_returns_synchronous_episode_result() { /* spec §10 */ }
```

- [ ] **Step 3: Commit**

```bash
git add crates/memory-mcp/src/mcp/handlers.rs
git commit -m "feat(extract): advertise Tasks capability; ingest stays synchronous"
```

### Task 8.6: Task retention

**Files:**
- Modify: `crates/memory-mcp/src/http/tasks/worker.rs`
- Modify: `crates/memory-mcp/src/http/tasks/scheduler.rs`

- [ ] **Step 1: Scheduled cleanup**

Create `tasks/scheduler.rs` with one bounded job that retries/reconciles due work
and removes only expired ephemeral Task rows:

```rust
pub fn scheduler_job() -> crate::http::leases::scheduler::SchedulerJob {
    std::sync::Arc::new(|registry| {
        Box::pin(async move {
            crate::http::tasks::worker::retry_reconcile_and_retain(&registry).await
        })
    })
}
```

Add `pub async fn retry_reconcile_and_retain(
registry: &RegistryHandle) -> Result<(), MemoryError>` in `worker.rs`. It
enumerates a bounded Tenant batch, claims each Tenant maintenance lease before opening its namespace, and performs retry,
artifact reconciliation, and `retention_expiry < time::now()` cleanup with the
Task fingerprint/version invariants. The binary registers this job after the
App Session job and before `scheduler::start`; no detached worker is spawned.

- [ ] **Step 2: Tests**

```rust
#[tokio::test]
async fn expired_tasks_are_deleted_after_retention_window() { /* spec §10 */ }
```

- [ ] **Step 3: Commit**

```bash
git add crates/memory-mcp/src/http/tasks/
git commit -m "feat(tasks): retention cleanup"
```

---

## Phase 9: Durable subscriptions

### Task 9.1: Transactional outbox

**Files:**
- Create: `crates/memory-mcp/src/http/subscriptions/mod.rs`
- Create: `crates/memory-mcp/src/http/subscriptions/outbox.rs`
- Modify: `crates/memory-mcp/src/http/mod.rs` (register `pub mod subscriptions;`)

At the start of this task:

```rust
// http/subscriptions/mod.rs
pub mod outbox;
```

- [ ] **Step 1: Schema migration**

Add `042_tenant_change_event.surql` (SaaS-gated):

Define the shared event DTO before the writer/stream modules use it:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TenantChangeEvent {
    pub sequence: u64,
    pub resource_id: String,
    pub revision: u64,
    pub change_kind: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}
```

```surql
DEFINE TABLE IF NOT EXISTS tenant_change_sequence SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS value ON tenant_change_sequence TYPE int DEFAULT 0;
DEFINE TABLE IF NOT EXISTS tenant_change_event SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS sequence ON tenant_change_event TYPE int;
DEFINE FIELD IF NOT EXISTS resource_id ON tenant_change_event TYPE string;
DEFINE FIELD IF NOT EXISTS revision ON tenant_change_event TYPE int;
DEFINE FIELD IF NOT EXISTS change_kind ON tenant_change_event TYPE string;
DEFINE FIELD IF NOT EXISTS created_at ON tenant_change_event TYPE datetime;
DEFINE INDEX IF NOT EXISTS idx_event_seq ON tenant_change_event FIELDS sequence;
DEFINE INDEX IF NOT EXISTS idx_event_seq_unique ON tenant_change_event FIELDS sequence UNIQUE;
```

- [ ] **Step 2: Mutation helper**

Implement `commit_mutation_with_event(db: &BoundDbClient, mutation:
TenantMutation, event: TenantChangeEvent) -> Result<(), MemoryError>` as one
parameterized SurrealDB transaction through `db.query`: `BEGIN TRANSACTION`,
atomically increment the tenant-local sequence row, apply the closed internal
`TenantMutation`, insert the event with that sequence, and `COMMIT TRANSACTION`.
Any statement error aborts/rolls back the transaction and returns the storage
error without emitting an event. `TenantMutation` is an internal enum/DTO with validated record IDs and patch
values; it is never caller-provided SQL. Replace the write paths for `ingest`,
`resolve`, `invalidate`, durable Task state changes, and durable App Session
commands with this helper (or an equivalent service-layer call) so no canonical
mutation can silently bypass the outbox. `extract` artifact writes and their
reconciliation events follow the same rule; read-only retrieval does not emit
an event.

- [ ] **Step 3: Tests**

```rust
#[tokio::test]
async fn mutation_and_event_commit_atomically() { /* spec §11 */ }
#[tokio::test]
async fn rolled_back_mutation_does_not_emit_event() { /* spec §11 */ }
```

- [ ] **Step 4: Commit**

```bash
git add crates/memory-mcp/src/http/subscriptions/ crates/memory-mcp/src/storage/migrations.rs
git commit -m "feat(subscriptions): transactional TenantChangeEvent outbox"
```

### Task 9.2: `subscriptions/listen` handler

**Files:**
- Create: `crates/memory-mcp/src/http/subscriptions/stream.rs`
- Create: `crates/memory-mcp/src/http/subscriptions/scheduler.rs`
- Modify: `crates/memory-mcp/src/http/subscriptions/mod.rs` (add `pub mod stream; pub mod scheduler;`)
- Modify: `crates/memory-mcp/src/mcp/handlers.rs` (implement rmcp subscription hooks)
- Modify: `crates/memory-mcp/src/http/transport.rs` (attach request-scoped subscription authorization)
- Modify: `crates/memory-mcp/src/http/runtime/storage.rs` (bind the subscription store to the Tenant runtime)
- Modify: `crates/memory-mcp/src/bin/memory_mcp_http.rs` (register outbox job)

- [ ] **Step 1: Implement the rmcp 3.1.2 subscription seam**

`rmcp` already parses `subscriptions/listen`, sends its acknowledgment, and
invokes `ServerHandler::accepted_subscription_filter` followed by
`ServerHandler::listen(SubscriptionContext)`. Do not add a second JSON-RPC
handler or a custom HTTP route. Add this Tenant-bound backend to
`subscriptions/mod.rs`:

```rust
#[async_trait::async_trait]
pub trait SubscriptionStore: Send + Sync + 'static {
    async fn current_sequence(&self) -> Result<u64, MemoryError>;
    async fn next_batch(
        &self,
        after_sequence: u64,
        requested: &rmcp::model::SubscriptionFilter,
    ) -> Result<Vec<TenantChangeEvent>, MemoryError>;
}

pub struct DurableSubscriptionStore {
    db: std::sync::Arc<crate::storage::client::BoundDbClient>,
    tenant_id: String,
}

impl DurableSubscriptionStore {
    pub fn new(
        db: std::sync::Arc<crate::storage::client::BoundDbClient>,
        tenant_id: String,
    ) -> Self {
        Self { db, tenant_id }
    }
}
```

Implement `SubscriptionStore` for `DurableSubscriptionStore`. It owns
`Arc<BoundDbClient>` and the immutable Tenant identity. Its `next_batch` reads
at most 256 events with `sequence > after`, filters by the requested resource
URI/category, and coalesces consecutive updates for one resource while
retaining the highest revision. It never exposes
the tenant namespace or full resource body.

Update `build_http_server_info()` from Task 3.4 in this task: when
`subscription_store.is_some()`, call the rmcp 3.1.2 builder method
`enable_resources_subscribe()` in the same `mcp-apps` cfg branch. A handler with
no subscription backend must not advertise resource subscriptions.

Add to `MemoryMcp` an optional
`Arc<dyn SubscriptionStore>` and a consuming
`with_durable_subscriptions(store)` builder. The stdio constructor leaves this
field absent. Add the field as `subscription_store: Option<Arc<dyn
SubscriptionStore>>`, `subscription_principal: Option<AuthenticatedPrincipal>`,
and `subscription_authenticator: Option<Arc<Authenticator>>`; initialize all
three to `None` in every existing constructor. The last two values are request
scoped and are never persisted. Import
`crate::http::principal::AuthenticatedPrincipal` and the authenticator only
under `streamable-http`. Add the consuming builders before implementing the
handler hooks:

```rust
pub fn with_durable_subscriptions(
    mut self,
    store: std::sync::Arc<dyn crate::http::subscriptions::SubscriptionStore>,
) -> Self {
    self.subscription_store = Some(store);
    self
}

pub fn with_subscription_authorization(
    mut self,
    principal: AuthenticatedPrincipal,
    authenticator: std::sync::Arc<crate::http::principal::auth::Authenticator>,
) -> Self {
    self.subscription_principal = Some(principal);
    self.subscription_authenticator = Some(authenticator);
    self
}
```

In `ServerHandler` implement the actual rmcp hooks. Both overrides are
`#[cfg(feature = "streamable-http")]`; when the feature is absent the rmcp
trait defaults leave subscriptions unimplemented and the stdio build has no
HTTP imports:

```rust
#[cfg(feature = "streamable-http")]
fn accepted_subscription_filter(
    &self,
    requested: &rmcp::model::SubscriptionFilter,
) -> Option<rmcp::model::SubscriptionFilter> {
    self.subscription_store.as_ref().map(|_| requested.clone())
}

#[cfg(feature = "streamable-http")]
fn listen(
    &self,
    context: rmcp::service::SubscriptionContext,
) -> impl std::future::Future<Output = Result<(), rmcp::ErrorData>> + rmcp::service::MaybeSendFuture + '_ {
    let store = self.subscription_store.clone();
    async move {
        let Some(store) = store else {
            return Err(rmcp::ErrorData::method_not_found::<
                rmcp::model::SubscriptionsListenRequestMethod,
            >());
        };
        let Some(principal) = self.subscription_principal.clone() else {
            return Err(rmcp::ErrorData::internal_error("subscription principal missing", None));
        };
        let Some(authenticator) = self.subscription_authenticator.clone() else {
            return Err(rmcp::ErrorData::internal_error("subscription authenticator missing", None));
        };
        // No replay/resume exists in the 2026-07-28 transport. Start at the
        // current durable sequence and deliver only changes committed after
        // this subscription is acknowledged.
        let mut cursor = store.current_sequence().await
            .map_err(|error| rmcp::ErrorData::internal_error(error.to_string(), None))?;
        loop {
            tokio::select! {
                _ = context.cancelled() => return Ok(()),
                batch = store.next_batch(cursor, context.sink().accepted()) => {
                    if !authenticator.is_current(&principal).await {
                        return Err(rmcp::ErrorData::internal_error("subscription authorization expired", None));
                    }
                    let batch = batch.map_err(|error| rmcp::ErrorData::internal_error(error.to_string(), None))?;
                    for event in batch {
                        cursor = cursor.max(event.sequence);
                        crate::http::subscriptions::stream::send_invalidation(
                            context.sink(), event,
                        ).await.map_err(|error| rmcp::ErrorData::internal_error(error.to_string(), None))?;
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        }
    }
}
```

The `SubscriptionContext`, `SubscriptionSink::accepted`,
`SubscriptionSink::send`, `SubscriptionsListenRequestMethod`, and `ErrorData`
constructors above are public in rmcp 3.1.2; verify their imports against the
pinned source during implementation. The control flow and ownership are fixed.
The listener rechecks the API-key/account validity before
each polling interval and terminates within 60 seconds after revocation. The
stream uses rmcp's bounded sink; a closed/slow sink ends the listener without
pinning the full Tenant Runtime. Define the event-to-notification adapter in
`subscriptions/stream.rs`:

```rust
pub async fn send_invalidation(
    sink: &rmcp::service::SubscriptionSink,
    event: TenantChangeEvent,
) -> Result<(), rmcp::service::SubscriptionSendError> {
    use rmcp::model::{ResourceUpdatedNotification, ResourceUpdatedNotificationParam, ServerNotification};
    sink.send(ServerNotification::ResourceUpdatedNotification(
        ResourceUpdatedNotification::new(ResourceUpdatedNotificationParam::new(event.resource_id)),
    )).await
}
```

Update the Task 5.6 `mcp_handler` local before building the service: extract
`AuthenticatedPrincipal` from request extensions, call
`runtime.mcp_service.clone().with_subscription_authorization(principal,
state.authenticator.clone())`, and use that request-scoped clone as the
`StreamableHttpService` factory. This is added here, not in Phase 5.6, so the
Phase 5.6 checkpoint compiles before the subscription fields exist.

Update `build_runtime` to attach
`DurableSubscriptionStore::new(bound_db.clone(), tenant.id.clone())` with
`with_durable_subscriptions`. The store is minimal and tenant-bound; it is not a
full runtime guard and is safe for inactive-runtime maintenance.

- [ ] **Step 2: Tests**

```rust
#[tokio::test]
async fn stream_emits_only_filtered_events() { /* spec §11 */ }
#[tokio::test]
async fn coalesces_consecutive_events_for_same_resource() { /* spec §11 */ }
#[tokio::test]
async fn slow_consumer_is_disconnected() { /* spec §11 */ }
#[tokio::test]
async fn authorization_recheck_within_60_seconds() { /* spec §11 */ }
```

- [ ] **Step 3: Commit**

```bash
git add crates/memory-mcp/src/http/subscriptions/ crates/memory-mcp/src/mcp/handlers.rs crates/memory-mcp/src/http/transport.rs crates/memory-mcp/src/http/runtime/storage.rs crates/memory-mcp/src/bin/memory_mcp_http.rs
git commit -m "feat(subscriptions): listen handler with bounded queue + auth recheck"
```

### Task 9.3: Cross-replica wake

**Files:**
- Modify: `crates/memory-mcp/src/http/subscriptions/outbox.rs`

- [ ] **Step 1: SurrealDB live query as wake**

Use `LIVE SELECT * FROM tenant_change_event WHERE sequence > $last_seq` to wake a sleeping replica. Wake loss is repaired by polling the durable outbox on next reconnect.

Add the process-level polling hook and register it in the binary alongside the
provisioning, App Session, and Task jobs:

```rust
pub fn scheduler_job() -> crate::http::leases::scheduler::SchedulerJob {
    std::sync::Arc::new(|registry| {
        Box::pin(async move {
            crate::http::subscriptions::outbox::poll_and_repair_all(&registry).await
        })
    })
}
```

Implement `pub async fn poll_and_repair_all(registry: &RegistryHandle) -> Result<(), MemoryError>` by reading a bounded sequence window for each active
subscription, emitting only invalidation events, and advancing the durable
cursor after delivery. Reconnect/poll repairs a lost `LIVE SELECT` wake; it
never relies on an in-memory broadcast as the source of truth.

`memory_mcp_http.rs` must now build hooks in this exact order:

```rust
let hooks = SchedulerHooks::with_provisioning_only()?
    .with_additional_job(app_sessions::scheduler::scheduler_job())
    .with_additional_job(tasks::scheduler::scheduler_job())
    .with_additional_job(subscriptions::scheduler::scheduler_job());
```

The final hook list is non-empty, all jobs are tracked by `JoinSet`, and each
job remains within the bounded maintenance parallelism.

- [ ] **Step 2: Tests**

```rust
#[tokio::test]
async fn subscription_event_from_other_replica_is_delivered() { /* spec §20.2 */ }
#[tokio::test]
async fn subscription_event_repaired_via_outbox_when_wake_lost() { /* spec §20.3 */ }
```

- [ ] **Step 3: Commit**

```bash
git add crates/memory-mcp/src/http/subscriptions/
git commit -m "feat(subscriptions): cross-replica wake with outbox fallback"
```

---

## Phase 10: Control-plane API + Dioxus SPA

This phase introduces OIDC login, server-side Control Plane Sessions, CSRF, recent-auth, the `/api/v1` API surface, and the optional Dioxus SPA. **Requires** the `control-plane` Cargo feature.

### Task 10.1: OIDC Authorization Code + PKCE

**Files:**
- Create: `crates/memory-mcp/src/control/oidc.rs`
- Modify: `crates/memory-mcp/src/http/config.rs`
- Modify: `crates/memory-mcp/src/http/mod.rs` (`HttpState`)
- Modify: `crates/memory-mcp/src/http/registry/storage.rs` (`RegistryStore`)

- [ ] **Step 0: Extend config, state, and registry for the control plane**

Later steps reference `state.oidc_client`, `state.config.oidc_issuer`, and
registry session/OIDC methods — none exist yet; create them here:

1. `HttpConfig` gains OIDC provider fields, loaded from env and REQUIRED
   when `enable_control_plane = true` (rejected by `validate` otherwise):
   `oidc_issuer: String`, `oidc_client_id: String`, `oidc_audience: String`,
   `oidc_redirect_uri: String`, `oidc_allowed_alg: String` (default `RS256`).
   `from_env` reads them from `MEMORY_MCP_HTTP_OIDC_ISSUER`,
   `MEMORY_MCP_HTTP_OIDC_CLIENT_ID`, `MEMORY_MCP_HTTP_OIDC_AUDIENCE`,
   `MEMORY_MCP_HTTP_OIDC_REDIRECT_URI`, and
   `MEMORY_MCP_HTTP_OIDC_ALLOWED_ALG`; it permits them to be absent only when
   the control plane is disabled. `validate` accepts only `RS256`, `ES256`, or
   `EdDSA`, rejects an issuer/redirect URI that is not HTTPS in production
   (loopback HTTP is allowed only under `test-fixtures`), and rejects an enabled
   control plane if any required OIDC field is empty:

```rust
if self.enable_control_plane
    && [
        self.oidc_issuer.as_str(), self.oidc_client_id.as_str(),
        self.oidc_audience.as_str(), self.oidc_redirect_uri.as_str(),
    ].iter().any(|value| value.trim().is_empty())
{
    return Err(MemoryError::ConfigInvalid("OIDC settings are incomplete".into()));
}
```
2. `HttpState` gains `oidc_client: Option<Arc<OidcClient>>` (cfg-gated on
   `control-plane`). `HttpState::new` constructs it only when
   `config.enable_control_plane` is true; a disabled control plane must not
   perform OIDC discovery or require network access. Enabled control-plane
   startup fails closed if discovery fails:

```rust
#[cfg(feature = "control-plane")]
pub oidc_client: Option<Arc<OidcClient>>,

#[cfg(feature = "control-plane")]
let oidc_client = if config.enable_control_plane {
    Some(Arc::new(OidcClient::new(&config).await?))
} else {
    None
};
```

Add `oidc_client` to the final `HttpState` literal under the same cfg. The
`HttpConfig::from_env` and `default_for_test` implementations must also be
updated in this task with the OIDC fields below.
   `OidcClient` wraps the `oauth2` crate (Phase 2 proposal) with PKCE
   helpers: `authorize_url(state, pkce)`, `exchange_code(code, pkce)`,
   `async validate_id_token(token)` (exact issuer/audience/alg/JWKS validation).
3. Define the shared OIDC/auth types in `control/oidc.rs` before adding the
   state store methods. Also define the registry-safe subject index helper; raw
   OIDC subjects remain transient only:

```rust
use crate::error::MemoryError;

pub fn identity_subject_verifier(
    key: &[u8; 32],
    issuer: &str,
    subject: &str,
) -> Result<[u8; 32], MemoryError> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .map_err(|_| MemoryError::ConfigInvalid("identity index key".into()))?;
    mac.update(issuer.trim().as_bytes());
    mac.update(b":");
    mac.update(subject.as_bytes());
    Ok(mac.finalize().into_bytes().into())
}

/// Finds or creates the Account/ExternalIdentity mapping according to signup
/// policy. The subject argument is a keyed blind index, never raw OIDC `sub`.
async fn upsert_account_for_identity(
    state: &HttpState,
    issuer: &str,
    subject_verifier: &[u8; 32],
) -> Result<Account, MemoryError>;
```

   Then define the shared OIDC/auth types in `control/oidc.rs`:

```rust
use std::sync::Arc;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use jsonwebtoken::DecodingKey;

#[derive(Debug, Clone)]
pub struct OidcState(String);
impl OidcState {
    pub fn new() -> Self { Self(hex::encode(rand::random::<[u8; 32]>())) }
    pub fn as_str(&self) -> &str { &self.0 }
}

#[derive(Debug, Clone)]
pub struct OidcNonce(String);
impl OidcNonce {
    pub fn new() -> Self { Self(hex::encode(rand::random::<[u8; 32]>())) }
    pub fn as_str(&self) -> &str { &self.0 }
}

#[derive(Debug, Clone)]
pub struct PkceCode {
    pub verifier: String,
    pub challenge: String,
}
impl PkceCode {
    pub fn new() -> Self {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        use rand::RngCore;
        use sha2::{Digest, Sha256};
        let mut bytes = [0_u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let verifier = URL_SAFE_NO_PAD.encode(bytes);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Self { verifier, challenge }
    }
}

#[derive(Debug, Clone)]
pub struct StoredOidcRequest {
    pub state: OidcState,
    pub nonce: OidcNonce,
    pub pkce: PkceCode,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct OidcTokens {
    pub id_token: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct OidcCallback {
    pub code: Option<String>,
    pub state: String,
    pub error: Option<String>,
    pub error_description: Option<String>,
    /// RFC 9207 issuer parameter; if present it must match the configured
    /// issuer before the authorization code is redeemed.
    pub iss: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("malformed token")]
    MalformedToken,
    #[error("token has no key id")]
    MissingKeyId,
    #[error("token algorithm is not allowed")]
    DisallowedAlgorithm,
    #[error("JWT validation failed: {0}")]
    Jwt(#[source] jsonwebtoken::errors::Error),
    #[error("JWKS lookup failed: {0}")]
    Jwks(String),
    #[error("OIDC provider request failed: {0}")]
    Provider(String),
    #[error("OIDC flow material could not be sealed")]
    Sealing,
}

fn deserialize_audience<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Audience {
        One(String),
        Many(Vec<String>),
    }
    match Audience::deserialize(deserializer)? {
        Audience::One(value) => Ok(vec![value]),
        Audience::Many(values) => Ok(values),
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct AccessClaims {
    pub iss: String,
    pub sub: String,
    #[serde(deserialize_with = "deserialize_audience")]
    pub aud: Vec<String>,
    pub exp: u64,
    pub nonce: Option<String>,
}

#[derive(Clone)]
pub struct JwksCache {
    // Holds a bounded, TTL'd map of kid -> DecodingKey. The refresh lock makes
    // an unknown-kid refresh single-flight; there is no unbounded fetch loop.
    inner: Arc<std::sync::RwLock<JwksState>>,
    refresh_lock: Arc<tokio::sync::Mutex<()>>,
    client: reqwest::Client,
    jwks_uri: String,
    ttl: std::time::Duration,
}

struct JwksState {
    keys: std::collections::HashMap<String, DecodingKey>,
    fetched_at: Option<std::time::Instant>,
}

impl JwksCache {
    pub fn find_key(&self, kid: &str) -> Result<Option<DecodingKey>, AuthError> {
        Ok(self
            .inner
            .read()
            .map_err(|_| AuthError::Jwks("JWKS cache lock poisoned".into()))?
            .keys
            .get(kid)
            .cloned())
    }

    pub async fn key_for(&self, kid: &str) -> Result<DecodingKey, AuthError> {
        let fresh = self.inner.read()
            .map_err(|_| AuthError::Jwks("JWKS cache lock poisoned".into()))?
            .fetched_at
            .is_some_and(|at| at.elapsed() < self.ttl);
        if fresh && let Some(key) = self.find_key(kid)? {
            return Ok(key);
        }
        let _refresh = self.refresh_lock.lock().await;
        let fresh = self.inner.read()
            .map_err(|_| AuthError::Jwks("JWKS cache lock poisoned".into()))?
            .fetched_at
            .is_some_and(|at| at.elapsed() < self.ttl);
        if fresh && let Some(key) = self.find_key(kid)? {
            return Ok(key);
        }
        self.refresh().await?;
        self.find_key(kid)?.ok_or_else(|| AuthError::Jwks("unknown key id".into()))
    }

    pub async fn refresh(&self) -> Result<(), AuthError> {
        #[derive(serde::Deserialize)]
        struct JwksDocument { keys: Vec<Jwk> }
        #[derive(serde::Deserialize)]
        struct Jwk {
            kid: String,
            kty: String,
            n: Option<String>,
            e: Option<String>,
            crv: Option<String>,
            x: Option<String>,
            y: Option<String>,
            alg: Option<String>,
        }
        let document = self.client
            .get(&self.jwks_uri)
            .send().await
            .map_err(|error| AuthError::Jwks(error.to_string()))?
            .error_for_status()
            .map_err(|error| AuthError::Jwks(error.to_string()))?
            .json::<JwksDocument>().await
            .map_err(|error| AuthError::Jwks(error.to_string()))?;
        let mut keys = std::collections::HashMap::new();
        for jwk in document.keys.into_iter().take(32) {
            let key = match (jwk.kty.as_str(), jwk.n.as_deref(), jwk.e.as_deref(), jwk.crv.as_deref(), jwk.x.as_deref(), jwk.y.as_deref()) {
                ("RSA", Some(n), Some(e), _, _, _) => DecodingKey::from_rsa_components(n, e),
                ("EC", _, _, Some("P-256"), Some(x), Some(y)) => DecodingKey::from_ec_components(x, y),
                ("OKP", _, _, Some("Ed25519"), Some(x), _) => DecodingKey::from_ed_components(x),
                _ => continue,
            }.map_err(|error| AuthError::Jwks(error.to_string()))?;
            if jwk.alg.as_deref().is_none_or(|alg| alg == "RS256" || alg == "ES256" || alg == "EdDSA") {
                keys.insert(jwk.kid, key);
            }
        }
        let mut state = self.inner.write()
            .map_err(|_| AuthError::Jwks("JWKS cache lock poisoned".into()))?;
        state.keys = keys;
        state.fetched_at = Some(std::time::Instant::now());
        Ok(())
    }
}

#[derive(Clone)]
pub struct OidcClient {
    issuer: String,
    client_id: String,
    audience: String,
    redirect_uri: String,
    allowed_algorithm: String,
    jwks: JwksCache,
}
```

`OidcClient` exposes these exact methods (implement them in this task; the
signatures are the seam consumed by the handlers):

- `async fn new(cfg: &HttpConfig) -> Result<Self, MemoryError>` — validate
  issuer/client/audience, perform OIDC discovery with redirects disabled, and
  initialize the bounded `JwksCache`.
- `fn authorize_url(&self, state: OidcState, pkce: PkceCode, nonce: OidcNonce) -> String`
  — use `oauth2::basic::BasicClient`, `CsrfToken`, `PkceCodeChallenge` with
  S256, the configured redirect URI, `openid` scope, and the nonce parameter.
- `async fn exchange_code(&self, code: String, pkce: PkceCode) -> Result<OidcTokens, AuthError>`
  — use `set_pkce_verifier` and `request_async` with a reqwest client whose
  redirect policy is `none`; retain only the ID token needed by the callback.
- `async fn validate_id_token(&self, token: &str) -> Result<AccessClaims, AuthError>`
  — delegate to the same algorithm/issuer/audience/JWKS validation used by
  Task 11.2. JWKS lookup may refresh once on an unknown or expired `kid`, so
  this method is async and callback code must await it.

The registry row stores `state_hash = HMAC(oidc_state_key, state)` and an
authenticated-encrypted `sealed_payload` containing nonce + PKCE verifier.
`store_oidc_request` seals before writing; `take_oidc_request` hashes the
supplied callback state, atomically consumes the row, decrypts once, and returns
`StoredOidcRequest`. Raw state/nonce/verifier are never durable columns and
never logged. Use `chacha20poly1305` with a random nonce per row; store the
ciphertext and AEAD nonce, and reject authentication failure.
4. `RegistryStore` (Task 4.1 Step 1) gains the control-plane persistence
   methods used below:

```rust
async fn store_oidc_request(&self, state: OidcState, pkce: PkceCode, nonce: OidcNonce) -> Result<(), MemoryError>;
async fn take_oidc_request(&self, state: &str) -> Result<Option<StoredOidcRequest>, MemoryError>;
async fn store_session(&self, session: &ControlPlaneSession) -> Result<(), MemoryError>;
async fn find_session(&self, verifier: &str) -> Result<Option<ControlPlaneSession>, MemoryError>;
async fn delete_session(&self, verifier: &str) -> Result<(), MemoryError>;
```

(OIDC request rows TTL 10 minutes per Step 1; sessions carry idle +
absolute expiry per Task 10.2. The durable OIDC representation is the keyed
state hash plus sealed payload described above. `StoredOidcRequest` is the
short-lived decrypted in-memory projection; `ControlPlaneSession` stores `id`
and keyed `cookie_hash`, never the raw cookie.

- [ ] **Step 1: State/nonce storage**

State and nonce are stored in the **registry** database (not in cookies) so they survive across replicas. TTL: 10 minutes.

- [ ] **Step 2: Authorize endpoint**

```rust
pub async fn authorize(State(state): State<Arc<HttpState>>) -> Result<impl IntoResponse, ApiError> {
    let pkce = PkceCode::new();
    let state_token = OidcState::new();
    let nonce = OidcNonce::new();
    state.registry.store_clone()
        .store_oidc_request(state_token.clone(), pkce.clone(), nonce.clone())
        .await?;
    let oidc = state.oidc_client.as_ref().ok_or(ApiError::Unavailable)?;
    let url = oidc.authorize_url(state_token, pkce, nonce);
    Ok(Redirect::to(url.as_str()))
}
```

- [ ] **Step 3: Callback handler**

```rust
pub async fn callback(
    State(state): State<Arc<HttpState>>,
    Query(params): Query<OidcCallback>,
) -> Result<impl IntoResponse, ApiError> {
    let stored = state.registry.store_clone().take_oidc_request(&params.state).await?
        .ok_or(ApiError::Unauthorized)?;
    if stored.expires_at < chrono::Utc::now() || params.error.is_some() {
        return Err(ApiError::Unauthorized);
    }
    if params.iss.as_deref().is_some_and(|issuer| issuer != state.config.oidc_issuer) {
        return Err(ApiError::Unauthorized);
    }
    let code = params.code.ok_or(ApiError::Unauthorized)?;
    let oidc = state.oidc_client.as_ref().ok_or(ApiError::Unavailable)?;
    let tokens = oidc.exchange_code(code, stored.pkce).await?;
    let claims = oidc.validate_id_token(&tokens.id_token).await?;
    // Validate issuer, audience, signature algorithm, expiry, and the nonce
    // generated for THIS authorization request before accepting the identity.
    if claims.nonce.as_deref() != Some(stored.nonce.as_str()) {
        return Err(ApiError::Unauthorized);
    }
    let (issuer, subject) = (claims.iss, claims.sub);
    let subject_verifier = identity_subject_verifier(
        &state.config.identity_index_key,
        &issuer,
        &subject,
    )?;
    // Idempotent account provisioning when signup policy permits; only the
    // keyed blind index is persisted, never the raw subject.
    let account = upsert_account_for_identity(&state, &issuer, &subject_verifier).await?;
    // Create a random raw cookie value. Store only its keyed hash; the raw
    // value is returned once and never persisted in the registry.
    let cookie_value = generate_session_cookie_value();
    let session = ControlPlaneSession::new(&account, &cookie_value, &state.config)?;
    state.registry.store_clone().store_session(&session).await?;
    let cookie = build_session_cookie(cookie_value, &state.config);
    Ok(([(SET_COOKIE, cookie)], Redirect::to("/")))
}
```

- [ ] **Step 4: Tests**

```rust
#[tokio::test]
async fn authorize_stores_state_and_returns_oidc_url() { /* ... */ }
#[tokio::test]
async fn callback_rejects_state_mismatch() { /* spec §5.3 */ }
#[tokio::test]
async fn callback_rejects_audience_mismatch() { /* spec §5.4 (future) */ }
#[tokio::test]
async fn callback_rotates_session_after_login() { /* spec §5.3 */ }
```

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/control/oidc.rs
git commit -m "feat(control): OIDC Authorization Code + PKCE"
```

### Task 10.2: Control Plane Session cookie

**Files:**
- Create: `crates/memory-mcp/src/control/session.rs`

- [ ] **Step 1: Session record and verifier-backed cookie**

Add the session constructor and hash helper in `control/session.rs`; the raw
cookie is never sent to the registry store:

```rust
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub fn keyed_session_hash(key: &[u8; 32], raw: &[u8]) -> Result<[u8; 32], MemoryError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .map_err(|_| MemoryError::ConfigInvalid("invalid session HMAC key".into()))?;
    mac.update(raw);
    Ok(mac.finalize().into_bytes().into())
}

pub fn generate_session_cookie_value() -> String {
    use rand::RngCore;
    let mut raw = [0_u8; 32];
    rand::rng().fill_bytes(&mut raw);
    hex::encode(raw)
}

impl ControlPlaneSession {
    pub fn new(account: &Account, raw_cookie: &str, cfg: &HttpConfig) -> Result<Self, MemoryError> {
        let now = Utc::now();
        Ok(Self {
            id: uuid::Uuid::new_v4().to_string(),
            cookie_hash: keyed_session_hash(&cfg.control_plane_session_key, raw_cookie.as_bytes())?,
            account_id: account.id.clone(),
            auth_time: now,
            idle_expiry: now + chrono::Duration::hours(1),
            absolute_expiry: now + chrono::Duration::hours(24),
        })
    }
}

pub fn build_session_cookie(cookie_value: String, _cfg: &HttpConfig) -> String {
    format!(
        "__Host-memory_mcp_session={cookie_value}; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=86400",
    )
}
```

- [ ] **Step 2: Resolve from cookie**

```rust
pub async fn resolve_session(state: &HttpState, cookie_value: &str) -> Result<Option<Account>, ApiError> {
    let cookie_hash = keyed_session_hash(
        &state.config.control_plane_session_key,
        cookie_value.as_bytes(),
    )?;
    let session = state.registry.store_clone()
        .find_session(&hex::encode(cookie_hash))
        .await?
        .ok_or(ApiError::Unauthorized)?;
    if session.absolute_expiry < Utc::now() { return Err(ApiError::Unauthorized); }
    if session.idle_expiry < Utc::now() { return Err(ApiError::Unauthorized); }
    let account = state.registry.store_clone().find_account_by_id(&session.account_id).await?
        .ok_or(ApiError::Unauthorized)?;
    Ok(Some(account))
}
```

- [ ] **Step 3: Tests**

```rust
#[tokio::test]
async fn expired_absolute_session_is_rejected() { /* spec §5.3 */ }
#[tokio::test]
async fn expired_idle_session_is_rejected() { /* spec §5.3 */ }
```

- [ ] **Step 4: Commit**

```bash
git add crates/memory-mcp/src/control/session.rs
git commit -m "feat(control): server-side Control Plane Session"
```

### Task 10.3: CSRF tokens

**Files:**
- Create: `crates/memory-mcp/src/control/csrf.rs`

- [ ] **Step 1: Token issuance/verification**

CSRF tokens are HMACs of `(account_id, session_id, csrf_key)`. Stored in a hidden form field / `X-CSRF-Token` header. Required on all `/api/v1` mutations.

- [ ] **Step 2: Tests**

```rust
#[tokio::test]
async fn mutation_without_csrf_returns_403() { /* spec §3.1 */ }
#[tokio::test]
async fn mutation_with_tampered_csrf_returns_403() { /* ... */ }
```

- [ ] **Step 3: Commit**

```bash
git add crates/memory-mcp/src/control/csrf.rs
git commit -m "feat(control): CSRF tokens for state-changing endpoints"
```

### Task 10.4: Recent-auth gate

**Files:**
- Create: `crates/memory-mcp/src/control/recent_auth.rs`

- [ ] **Step 1: Gate**

```rust
pub fn require_recent_auth(session: &ControlPlaneSession, max_age: Duration) -> Result<(), ApiError> {
    if Utc::now() - session.auth_time > max_age {
        return Err(ApiError::ReauthRequired);
    }
    Ok(())
}
```

`max_age = 10 minutes` for credential creation, identity linking, and Account deletion (spec §5.3).

- [ ] **Step 2: Tests**

```rust
#[tokio::test]
async fn delete_account_requires_recent_auth_under_10_minutes() { /* spec §14 */ }
```

- [ ] **Step 3: Commit**

```bash
git add crates/memory-mcp/src/control/recent_auth.rs
git commit -m "feat(control): recent-auth gate for destructive operations"
```

### Task 10.5: Account management API

**Files:**
- Modify: `crates/memory-mcp/src/control/account_api.rs`

- [ ] **Step 1: Endpoints**

| Method + path | Behavior |
|---|---|
| `GET /api/v1/account` | Read account metadata + tenant status |
| `POST /api/v1/account/api_keys` | Create key (returns `secret` once) |
| `GET /api/v1/account/api_keys` | List keys with expiry/last-used |
| `DELETE /api/v1/account/api_keys/:id` | Revoke |
| `GET /api/v1/account/identity_links` | List linked External Identities |
| `DELETE /api/v1/account/identity_links/:id` | Unlink |
| `POST /api/v1/account/delete` | Start deletion flow |
| `POST /api/v1/account/delete/confirm` | Confirm with typed phrase |

- [ ] **Step 2: One-time key display**

```rust
pub async fn create_api_key(...) -> Result<(StatusCode, [(HeaderName, HeaderValue); 1], Json<CreateApiKeyResponse>), ApiError> {
    let secret = generate_secret();
    let key = ApiKey {
        id: new_api_key_id(),
        account_id: ...,
        name: req.name,
        verifier: KeyedVerifier::compute(&pepper, secret.as_bytes()),
        status: ApiKeyStatus::Active,
        ...
    };
    state.registry.store_clone().write_api_key(&key).await?;
    let resp = CreateApiKeyResponse { id: key.id.clone(), secret, expires_at: key.expires_at };
    let mut headers = [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))];
    Ok((StatusCode::CREATED, headers, Json(resp)))
}
```

- [ ] **Step 3: Tests**

```rust
#[tokio::test]
async fn create_api_key_returns_secret_only_once() { /* spec §5.2 */ }
#[tokio::test]
async fn list_api_keys_excludes_secret() { /* spec §5.2 */ }
#[tokio::test]
async fn revoke_api_key_drops_access_within_60s() { /* spec §20.2 */ }
```

- [ ] **Step 4: Commit**

```bash
git add crates/memory-mcp/src/control/
git commit -m "feat(control): /api/v1/account/* endpoints"
```

### Task 10.6: Operator API

**Files:**
- Modify: `crates/memory-mcp/src/control/operator_api.rs`

- [ ] **Step 1: Operator principal**

Operators are mapped from OIDC `(issuer, subject)` allowlist or a trusted role claim from the configured issuer/audience. `/api/v1/account/*` cannot grant operator status.

- [ ] **Step 2: Endpoints**

| Method + path | Behavior |
|---|---|
| `GET /api/v1/operator/tenants/:id` | Read provisioning state |
| `POST /api/v1/operator/tenants/:id/retry` | Retry failed provisioning stage |
| `POST /api/v1/operator/tenants/:id/suspend` | Suspend |
| `POST /api/v1/operator/tenants/:id/resume` | Resume |
| `POST /api/v1/operator/tenants/:id/purge` | Initiate Account deletion |
| `GET /api/v1/operator/recovery/status` | Read recovery status |

All require CSRF + recent-auth. Audit records are appended while the target Account exists.

- [ ] **Step 3: Tests**

```rust
#[tokio::test]
async fn operator_endpoints_require_oidc_operator_principal() { /* spec §5.3 */ }
#[tokio::test]
async fn account_api_cannot_grant_operator_status() { /* spec §5.3 */ }
```

- [ ] **Step 4: Commit**

```bash
git add crates/memory-mcp/src/control/
git commit -m "feat(control): /api/v1/operator/* endpoints"
```

### Task 10.7: Account deletion flow

**Files:**
- Modify: `crates/memory-mcp/src/control/deletion.rs`

- [ ] **Step 1: Flow**

1. Recent OIDC reauthentication (Task 10.4).
2. Display notice: no export/recovery (spec §14).
3. Typed-phrase confirmation by the user.
4. Server-issued short-lived one-use confirmation token bound to Account + session.
5. Durable credential/session revocation; cached auth becomes ineffective within ≤60s.
6. Idempotent logical deletion job: durable tombstone, Account/Tenant terminal deletion states, permanent data-plane denial, memory-record invalidation according to domain policy, and removal only of expired ephemeral Task/App Session rows. Account, Tenant, identity, credential, lease, provisioning, and audit-bearing registry records remain durable; the namespace binding is never reused.

- [ ] **Step 2: Tests**

```rust
#[tokio::test]
async fn deletion_flow_rejects_wrong_typed_phrase() { /* spec §14 */ }
#[tokio::test]
async fn deletion_flow_revokes_sessions_within_60s() { /* spec §14 */ }
#[tokio::test]
async fn deletion_flow_keeps_non_reusable_tombstone_after_terminal_transition() { /* spec §14 */ }
```

- [ ] **Step 3: Commit**

```bash
git add crates/memory-mcp/src/control/deletion.rs
git commit -m "feat(control): Account deletion flow"
```

### Task 10.8: Static asset serving

**Files:**
- Modify: `crates/memory-mcp/src/control/static_assets.rs`

- [ ] **Step 1: Serve Dioxus-built SPA**

When `control-plane-ui` is enabled, embed built assets via `include_bytes!` and serve them under `/` with a fallback to `index.html`. The router must give priority to API routes (axum's `nest`).

- [ ] **Step 2: Security headers**

```rust
pub async fn attach_security_headers<B>(mut resp: Response<B>) -> Response<B> {
    resp.headers_mut().insert("content-security-policy",
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self'; \
             connect-src 'self'; frame-ancestors 'none'; object-src 'none'; \
             base-uri 'none'; form-action 'self'"));
    resp.headers_mut().insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    resp.headers_mut().insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    resp
}
```

- [ ] **Step 3: Tests**

```rust
#[tokio::test]
async fn served_assets_include_strict_csp() { /* spec §14 */ }
#[tokio::test]
async fn api_routes_take_priority_over_spa_fallback() { /* ... */ }
```

- [ ] **Step 4: Commit**

```bash
git add crates/memory-mcp/src/control/
git commit -m "feat(control): static asset serving with CSP"
```

### Task 10.9: Dioxus workspace crate

**Files:**
- Create: `crates/control-plane-ui/Cargo.toml`
- Create: `crates/control-plane-ui/src/main.rs`
- Create: `crates/control-plane-ui/src/api.rs`
- Create: `crates/control-plane-ui/src/router.rs`
- Create: `crates/control-plane-ui/src/pages/login.rs`
- Create: `crates/control-plane-ui/src/pages/keys.rs`
- Create: `crates/control-plane-ui/src/pages/delete.rs`
- Create: `crates/control-plane-ui/src/pages/status.rs`
- Modify: `Cargo.toml` (workspace member)
- Modify: `crates/memory-mcp/Cargo.toml` (Dioxus feature)

- [ ] **Step 1: Cargo.toml**

`crates/control-plane-ui/Cargo.toml`:

```toml
[package]
name = "control-plane-ui"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[[bin]]
name = "control-plane-ui"
path = "src/main.rs"

[dependencies]
dioxus = { version = "0.6", default-features = false, features = ["web"] }
dioxus-router = { version = "0.6" }
serde = { workspace = true }
serde_json = { workspace = true }
gloo-net = { version = "0.5" }
gloo-storage = { version = "0.5" }
```

- [ ] **Step 2: Dioxus router**

```rust
// crates/control-plane-ui/src/router.rs
use dioxus::prelude::*;
use dioxus_router::{Route, Router};

#[derive(Routable, Clone)]
pub enum Route {
    #[route("/")]
    Status {},
    #[route("/login")]
    Login {},
    #[route("/keys")]
    Keys {},
    #[route("/delete")]
    Delete {},
}
```

- [ ] **Step 3: API client**

```rust
// crates/control-plane-ui/src/api.rs
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct ApiClient { base: String }

impl ApiClient {
    pub fn new(base: String) -> Self { Self { base } }
    pub async fn me(&self) -> Result<AccountMeta, ApiError> { /* ... */ }
    pub async fn list_keys(&self) -> Result<Vec<ApiKeyMeta>, ApiError> { /* ... */ }
    pub async fn create_key(&self, name: String) -> Result<CreateApiKeyResponse, ApiError> { /* ... */ }
    pub async fn revoke_key(&self, id: String) -> Result<(), ApiError> { /* ... */ }
    pub async fn start_delete(&self) -> Result<DeleteChallenge, ApiError> { /* ... */ }
    pub async fn confirm_delete(&self, phrase: String, confirm_token: String) -> Result<(), ApiError> { /* ... */ }
}
```

- [ ] **Step 4: Secret handling**

API-key secrets live in page memory only; never `localStorage`, never URL. `Cache-Control: no-store` is honored by the API (set in Task 10.5).

- [ ] **Step 5: Build artifacts**

Build with:

```bash
cd crates/control-plane-ui
dx build --release --platform web
```

The output is consumed by the backend via `include_bytes!`. Add a build script in `crates/memory-mcp/build.rs` (gated on `control-plane-ui`) that copies `crates/control-plane-ui/dist/**/*` into `OUT_DIR`, then `include_bytes!("...")` them from `static_assets.rs`.

- [ ] **Step 6: Tests**

Test the API DTO mapping in `crates/control-plane-ui/src/api.rs` against typed JSON fixtures. (Dioxus component tests require a DOM environment; defer to Dioxus docs.)

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/control-plane-ui/ crates/memory-mcp/Cargo.toml crates/memory-mcp/build.rs crates/memory-mcp/src/control/static_assets.rs
git commit -m "feat(ui): Dioxus web SPA for control plane"
```

---

## Phase 11: Future MCP OAuth Resource Server

This phase is implemented as a separately-tested phase per spec §21 step 9.
It requires the existing `control-plane` feature because it reuses the validated
OIDC issuer, audience, JWKS cache, and `OidcClient`; `streamable-http` alone does
not compile or expose OAuth routes.

### Task 11.1: Protected Resource Metadata publishing

**Files:**
- Create: `crates/memory-mcp/src/http/oauth/mod.rs`
- Modify: `crates/memory-mcp/src/http/mod.rs` (register `pub mod oauth;`)

At the start of this task:

```rust
// http/mod.rs
#[cfg(feature = "control-plane")]
pub mod oauth;
```

- [ ] **Step 1: Add route**

```rust
pub async fn protected_resource_metadata(State(state): State<Arc<HttpState>>) -> Json<Value> {
    Json(json!({
        "resource": state.config.public_base_url,
        "authorization_servers": [state.config.oidc_issuer],
        "bearer_methods_supported": ["header"],
        "scopes_supported": ["memory:read", "memory:write"],
    }))
}
```

Mount at `/.well-known/oauth-protected-resource`.

- [ ] **Step 2: Tests**

```rust
#[tokio::test]
async fn prm_publishes_resource_and_issuer() { /* spec §5.4 */ }
```

### Task 11.2: JWT validation

**Files:**
- Modify: `crates/memory-mcp/src/http/oauth/mod.rs`

- [ ] **Step 1: Validate issuer, audience, expiry, algorithm allowlist**

```rust
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};

pub async fn validate_token(
    token: &str,
    cfg: &HttpConfig,
    jwks: &JwksCache,
) -> Result<AccessClaims, AuthError> {
    let header = decode_header(token).map_err(|_| AuthError::MalformedToken)?;
    let algorithm = match cfg.oidc_allowed_alg.as_str() {
        "RS256" => Algorithm::RS256,
        "ES256" => Algorithm::ES256,
        "EdDSA" => Algorithm::EdDSA,
        _ => return Err(AuthError::DisallowedAlgorithm),
    };
    if header.alg != algorithm {
        return Err(AuthError::DisallowedAlgorithm);
    }
    let kid = header.kid.ok_or(AuthError::MissingKeyId)?;
    let key: DecodingKey = jwks.key_for(&kid).await?;
    let mut validator = Validation::new(algorithm);
    validator.set_audience(&[cfg.oidc_audience.as_str()]);
    validator.set_issuer(&[cfg.oidc_issuer.as_str()]);
    validator.set_required_spec_claims(&["exp", "iss", "aud"]);
    let claims = decode::<AccessClaims>(token, &key, &validator)
        .map_err(AuthError::Jwt)?
        .claims;
    Ok(claims)
}
```

- [ ] **Step 2: Account resolution**

Use `(issuer, subject)` to find Account (the same path Task 4.5 uses for OIDC login).

- [ ] **Step 3: Tests**

```rust
#[tokio::test]
async fn rejects_token_with_unknown_audience() { /* spec §5.4 */ }
#[tokio::test]
async fn rejects_token_with_disallowed_algorithm() { /* spec §5.4 */ }
#[tokio::test]
async fn unknown_jwks_key_refresh_fails_safely() { /* spec §5.4 */ }
```

- [ ] **Step 4: Commit**

```bash
git add crates/memory-mcp/src/http/oauth/
git commit -m "feat(oauth): MCP OAuth Resource Server validation"
```

---

## Phase 12: Operational & release gates

### Task 12.1: Isolation tests (spec §20.2)

**Files:**
- Create: `crates/memory-mcp/tests/http_isolation.rs`

- [ ] **Step 1: Two Tenants under high concurrency**

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn two_tenants_share_no_state_under_high_concurrency() {
    // Create two Tenants, fire 200 ingest requests each, assert
    // - cross-Tenant query returns 0 results
    // - quota counters are independent
    // - pool cache keys include Tenant identity
    // - all changes stay in their namespaces
}
```

- [ ] **Step 2: Run**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_isolation -- --test-threads=1
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/memory-mcp/tests/http_isolation.rs
git commit -m "test(http): isolation suite (spec §20.2)"
```

### Task 12.2: Crash recovery tests (spec §20.3)

**Files:**
- Create: `crates/memory-mcp/tests/http_crash_recovery.rs`

- [ ] **Step 1: Crash between transitions**

```rust
#[tokio::test]
async fn crash_during_namespace_creating_recovers_to_ready_or_failed() { /* ... */ }
#[tokio::test]
async fn crash_during_task_running_recovers_via_fencing_takeover() { /* ... */ }
#[tokio::test]
async fn crash_during_outbox_poll_repairs_via_durable_event_log() { /* ... */ }
```

Inject crashes by spawning a child process that SIGKILLs the server between transitions; observe reconciliation.

- [ ] **Step 2: Commit**

```bash
git add crates/memory-mcp/tests/http_crash_recovery.rs
git commit -m "test(http): crash recovery suite (spec §20.3)"
```

### Task 12.3: Black-box protocol conformance (spec §20.1)

The test suite from Task 3.11 is the gate. Run it under `--release` and document its execution in `docs/operations/CONFORMANCE.md`.

- [ ] **Step 1: Create ops doc**

`docs/operations/CONFORMANCE.md` lists every test in `http_proto_conformance.rs` with the spec section it covers.

- [ ] **Step 2: Commit**

```bash
git add docs/operations/CONFORMANCE.md
git commit -m "docs(ops): protocol conformance coverage map"
```

### Task 12.4: Proxy streaming/no-buffering

**Files:**
- Create: `crates/memory-mcp/tests/http_proxy_streaming.rs`

- [ ] **Step 1: Test**

```rust
#[tokio::test]
async fn sse_response_carries_x_accel_buffering_no() { /* spec §3.2 */ }
#[tokio::test]
async fn sse_response_carries_cache_control_no_cache() { /* spec §3.2 */ }
#[tokio::test]
async fn read_timeout_larger_than_120s_deadline_is_required() { /* spec §3.2 */ }
```

- [ ] **Step 2: Commit**

```bash
git add crates/memory-mcp/tests/http_proxy_streaming.rs
git commit -m "test(http): proxy streaming/no-buffering assertions"
```

### Task 12.5: Load test (≤20 + contingency ≤500)

**Files:**
- Create: `crates/memory-mcp/tests/http_load_concurrency.rs`

- [ ] **Step 1: Test**

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
#[ignore = "load test; run with --ignored in CI"]
async fn load_20_active_tenants_under_expected_qps() { /* spec §20.5 */ }

#[tokio::test(flavor = "multi_thread", worker_threads = 32)]
#[ignore = "load test; run with --ignored in CI"]
async fn load_500_tenants_under_contingency_qps() { /* spec §20.5 */ }
```

- [ ] **Step 2: Commit**

```bash
git add crates/memory-mcp/tests/http_load_concurrency.rs
git commit -m "test(http): load test scaffolding for ≤20 / ≤500 active classes"
```

### Task 12.6: Remote SurrealDB restore drill

- [ ] **Step 1: Document the procedure**

Create `docs/operations/RESTORE_DRILL.md` covering:

1. Snapshot the chosen remote SurrealDB deployment using its standard mechanism.
2. Restore into a fresh namespace pair (`control_restore`, `tenant_restore`).
3. Boot `memory_mcp_http` against the restored pair.
4. Provisioning workers detect missing tenants; no resurrection of "deleted" Tenants is auto-attempted.
5. Before opening ingress, rotate:
   - API-key verifier pepper
   - OIDC identity-index key; require users to relink restored OIDC identities
   - Control Plane Session cookie/verifier key
   - OIDC state and nonce keys
   - CSRF keys

6. Document the limitation: historical backups are immutable, so restored data may include data marked deleted before the snapshot. State this explicitly.

- [ ] **Step 2: Commit**

```bash
git add docs/operations/RESTORE_DRILL.md
git commit -m "docs(ops): SurrealDB restore drill + credential rotation runbook"
```

### Task 12.7: Credential rotation recovery runbook

- [ ] **Step 1: Document**

Create `docs/operations/CREDENTIAL_ROTATION.md` listing:

- Which environment variables hold which keys.
- Rotation order: API-key pepper first (invalidates restored keys), OIDC identity-index key second (requires restored identities to relink), then browser/OIDC session keys, then CSRF.
- Confirmation that rotation alone does not erase restored data.

- [ ] **Step 2: Commit**

```bash
git add docs/operations/CREDENTIAL_ROTATION.md
git commit -m "docs(ops): credential rotation runbook"
```

### Task 12.8: Documentation updates

- [ ] **Step 1: Update README**

Add a new "Streamable HTTP SaaS" section to `README.md` covering:

- Quickstart (`cargo build --features streamable-http,control-plane --bin memory_mcp_http`).
- Required environment variables (link to Task 2.1 proposal).
- Reverse proxy requirements (link to spec §3.2).
- Limitations: no export, no per-Tenant restore, historical backup resurrection, embedded profile warning.

- [ ] **Step 2: Add limitations doc**

Create `docs/operations/LIMITATIONS.md` listing every accepted v1 limitation from spec §1 and ADR-0052 §consequences.

- [ ] **Step 3: Embedded profile warning**

In `src/http/server.rs::serve`, log at WARN level on first startup if embedded SurrealDB is in use (per spec §18).

- [ ] **Step 4: Commit**

```bash
git add README.md docs/operations/LIMITATIONS.md crates/memory-mcp/src/http/server.rs
git commit -m "docs(http): SaaS profile docs and embedded-profile warning"
```

### Task 12.9: Final lint gate

- [ ] **Step 1: Run full lint**

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets \
  --features fs-watch,mcp-apps,streamable-http,control-plane,test-fixtures --locked -- -D warnings
```

If `control-plane-ui` is built, also:

```bash
cargo clippy -p control-plane-ui --all-targets --locked -- -D warnings
```

Expected: zero warnings.

- [ ] **Step 2: Run stdio regression (spec §20.4)**

```bash
cargo test -p memory_mcp --features fs-watch,mcp-apps --test service_acceptance --test tools_e2e
```

Expected: PASS.

- [ ] **Step 3: Run conformance suite**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_proto_conformance
```

Expected: PASS.

- [ ] **Step 4: Tag a milestone**

Per project rules, do not tag unless the user asks. Just summarize.

---

## Self-Review

After writing this plan, the following self-review checklist was applied:

### 1. Spec coverage

Spec sections → plan phases:

| Spec section | Plan phase/task |
|---|---|
| §1 Purpose & non-goals | Phases 1, 2, 12.8 |
| §2 Deployable units and features | Phase 2 (features), Phase 10.9 (Dioxus), Task 3.10 (binary) |
| §3 HTTP topology | Phase 3 (router, host/origin, metrics, headers), Phase 10 (Dioxus, CSP) |
| §4 MCP `2026-07-28` profile | Phase 3, Task 8.5, Phase 11 (OAuth) |
| §5 Identity and credential model | Phase 4 |
| §6 Tenant Registry & provisioning | Phases 4, 5 |
| §7 Tenant Runtime pool | Phase 5 |
| §8 Durable workers and schema rollout | Phase 6 |
| §9 App Sessions | Phase 7 |
| §10 Tenant Tasks | Phase 8 |
| §11 Subscriptions | Phase 9 |
| §12 Quotas and rate limits | Phase 6, Task 4.4 (rate limit), Task 6.4 |
| §13 Ingestion and provider privacy | Task 3.1 (path/URL rejection), Task 6.5 (fs-watch rejection) |
| §14 Control-plane API and SPA | Phase 10 |
| §15 Data protection, deletion, recovery | Phase 10.7 (deletion), Phase 12.6 (restore), Phase 12.7 (rotation) |
| §16 Observability and health | Phase 3 (health, metrics), Phase 4 (logs) |
| §17 Shutdown | Phase 3 (`shutdown.rs`), Phase 5 (drain), Phase 6 (lease expiry), Phase 8 (task retention) |
| §18 Embedded HTTP profile | Phase 12.8 (warning + docs) |
| §19 Configuration contract | Phase 2, Phase 3.1, Phase 6.5 |
| §20.1 Protocol conformance | Phase 3.11, Phase 12.3 |
| §20.2 Isolation & concurrency | Phase 12.1, Task 5.6 |
| §20.3 Crash/recovery | Phase 12.2 |
| §20.4 Compatibility | Phase 1, Phase 12.9 |
| §20.5 Operations | Phase 12.4–12.7 |
| §21 Sequencing | Entire phase order |

### 2. Placeholder and expansion audit

- Production snippets contain no executable placeholder macro and no unbounded
  fallback. The only intentionally deferred implementation choices are called
  out as exact SDK/database verification gates below.
- Several later test blocks remain condensed test contracts rather than fully
  expanded Rust bodies. This is an explicit limitation of this document, not a
  claim that the plan is mechanically executable as-is: before an executor
  starts any task containing a condensed test, that task's test body must be
  expanded with the fixture setup, request, expected status/result, and cleanup
  assertions. The implementation gate rejects a task that leaves a condensed
  test in the committed change.
- The test-only bootstrap is not a stub: its required observable contract is a
  real Account, Tenant, namespace migration, and keyed verifier, and the
  conformance suite must fail if any one is absent.
- OAuth and Dioxus browser tests remain separately scoped because they require a
  real provider/browser runtime; their unit and black-box API assertions are
  still required in Tasks 10.1, 10.9, and 11.2.

### 3. Type/signature consistency (re-verified in the audit)

- `AuthenticatedPrincipal` — concrete enum defined in Task 4.6 with
  `account_id()` / `account()` accessors; consumed by middleware (4.6),
  runtime acquisition (5.6), `mcp_handler` (5.6).
- `HttpState` state type is **`Arc<HttpState>` everywhere** (axum
  `State<Arc<HttpState>>`); `build_router(state: Arc<HttpState>)`. No handler
  uses bare `HttpState`.
- `TenantRuntime` — defined in Task 5.4 with `tenant_db: Arc<SurrealDbClient>`,
  `bound_db: Arc<BoundDbClient>`, and `mcp_service: MemoryMcp`; consumed
  unchanged in 5.6 and by App Sessions/subscriptions.
- `RegistryHandle` owns a control `RegistryStore` and a separate
  `PrivilegedEngine::{Remote,Local}` built from the tenant target; control and
  tenant endpoints are not assumed to be the same deployment.
- `ApiKeyCredential::parse`, `KeyedVerifier::{compute,verify}`,
  `Authenticator::authenticate_bearer`, `RateLimiter::allow` — same names
  across 4.3 → 4.4 → 4.6.
- `MemoryError` gains exactly two variants (`Auth`, `Unavailable`) in Task 4.1
  step 0; all later usages reference existing or these two variants. `ApiError`
  separately gains `Unavailable` for the control-plane boundary.
- DB seams: `SurrealDbClient::connect_bound` (added in Task 3.3, contract
  re-verified in Task 4.1), `SurrealDbClient::from_prebound_mem` (Task 4.1, required so embedded
  registry and privileged engine share one Mem handle) and
  `SurrealDbClient::from_prebound` (Task 5.3, canonical code in Task 5.4
  Step 3), free function `ensure_namespace` (Task 5.2, provisioning).
  `PrivilegedEngine::{Remote,Local}` (Task 4.1) keeps runtime/provisioning
  code testable without creating a second isolated Mem database.
  The `DbClient` trait itself is unchanged. The pre-existing `pub(crate)
  BoundDbClient` adapter in `storage/client.rs` is reused by Tasks 7.2/9.1
  and is NOT the removed `PreboundDbClient` shim.
- Protocol: `MemoryMcp::new_modern` (Task 3.4) is the HTTP-profile handler
  constructor; `with_durable_app_sessions`, `with_durable_tasks`, and
  `with_durable_subscriptions` attach Tenant-bound backends without changing
  the stdio constructor; `new` stays stdio-default.
- Tasks: `TaskBackend::{InMemory, Durable}` (Task 8.4). rmcp's
  `TaskManager` is a concrete struct in 3.1.2 (source-verified) and remains
  only the in-memory stdio backend; the durable seam is the `ServerHandler`
  overrides (`get_task`, `update_task`, `cancel_task`) plus the `call_tool`
  extract path, never a trait impl. The field and variant are cfg-split so a
  stdio-only build never imports `crate::http`.
- `TaskState` enum, `TenantChangeEvent`, `AppSessionStore` — defined once and
  reused.

Final `HttpState` field evolution — every task EXTENDS the struct, none
redefines it (audit pass 3 enforced this):

| Field | Added | Removed |
|---|---|---|
| `config` | Task 3.3 | — |
| `mcp_factory` | Task 3.3 | Task 5.6 Step 0 |
| `metrics_handle` (cfg `prometheus`) | Task 3.8 | — |
| `shutdown`, `admission`, `registry` | Task 3.9 | — (registry stub → real store in Task 4.1 Step 4.5) |
| `authenticator` | Task 4.4 | — |
| `account_resolver` | Task 4.5 | — |
| `pool` | Task 5.6 Step 0 | — |
| `app_sessions` backend (cfg `mcp-apps`) | Task 7.2 | — |
| `oidc_client: Option<Arc<OidcClient>>` (cfg `control-plane`) | Task 10.1 Step 0 | — |


`MemoryMcp` additionally gains the Tenant-local durable backend fields
`subscription_store`, `subscription_principal`, and
`subscription_authenticator` in Task 9.2; these are not `HttpState` fields and
are request-scoped where noted above.

Constructor line: `HttpState::new_tenantless` (Task 3.3, async) → cfg-gated
metrics parameter (Task 3.8, `#[cfg]` on the parameter, single body) →
renamed `HttpState::new` (Task 5.6 Step 0). `default_for_test` is async from
Task 3.3 on; every call site uses `.await`.

### 4. Pitfalls re-checked

- ✅ No `Idempotency-Key` header (spec §4.4, ADR §67).
- ✅ No MRTR/roots/sampling/elicitation advertised (spec §4.1).
- ✅ Tasks only for `extract` (spec §10, ADR §121).
- ✅ `subscriptions/listen` uses rmcp's native `accepted_subscription_filter` /
  `listen(SubscriptionContext)` seam; no duplicate custom RPC route is added.
- ✅ Response-body ownership holds the ordinary runtime pin plus request permit
  until completion/error/disconnect; a long-lived subscription holds only its
  separate subscription permit and releases the full runtime guard before
  polling.
- ✅ Scheduler jobs are registered explicitly for provisioning, App Session
  cleanup, Task retry/retention, and outbox repair; all are tracked and joined.
- ✅ `rmcp::TaskManager` remains the concrete in-memory stdio backend; the HTTP
  durable Task path uses the `ServerHandler` seam (spec §10.2).
- ✅ Embedded SurrealDB warning emitted (spec §18).
- ✅ `/metrics` lacks app auth by decision (spec §3.1, ADR-0048 amendment).
- ✅ Wildcard production origin rejected (spec §3.3, Task 3.1 validation).
- ✅ `fs-watch` is a fatal HTTP startup error (spec §13, Task 6.5).
- ✅ Local paths/URLs rejected before I/O (spec §13, Task 3.1 + future Task in Phase 4 handler).
- ✅ `NeverSessionManager` not `LocalSessionManager` (Task 3.3).
- ✅ Clone-once, bind-once namespace-bound clients (spec §identity-and-tenancy, Phase 5.4).
- ✅ Registry/control engine and tenant privileged engine have an explicit seam; different configured endpoints do not cross (Task 4.1).
- ✅ Revocation durable immediately, externally bounded ≤60s without push invalidation (spec §5.2, Task 4.4).
- ✅ Positive auth cache re-verifies the supplied secret on every cache hit; cached Account alone is never sufficient (Task 4.4).
- ✅ Frozen 8-tool surface preserved (Task 1.1, Task 12.9).
- ✅ Stdio path remains default and zero-config (Task 1.1, Phase 12.9).

### 5. Runtime verification gates (not design gaps)

- **rmcp/Axum body integration**: compile Task 3.7 and Task 5.6 against the
  pinned 3.1.2 body types; the plan now requires a body-owning deadline and
  lease, but the exact trait bounds are compiler-verified at that checkpoint.
- **SurrealDB v3 semantics**: verify DDL namespace/database scope,
  `RETURN AFTER`, CAS predicates, transaction sequence allocation, and `LIVE
  SELECT` syntax against the deployed 3.2.4 engine in Tasks 5.2, 6.1, and 9.3.
- **OIDC provider behavior**: verify discovery metadata, JWKS rotation,
  response `iss`, token endpoint error handling, and the configured algorithm
  against the selected provider in Tasks 10.1 and 11.2; no provider-specific
  assumption may leak into the generic principal seam.
- **Dioxus 0.6**: verify web router/build asset syntax in Task 10.9 and run a
  browser-level smoke test; do not add server functions.
- **Condensed tests**: expand every marked later-phase test contract before its
  implementation task is started, as stated in the placeholder audit.

### 6. Audit trail (2026-08-27)

The plan was audited against the installed `rmcp 3.1.2` source, the actual
codebase seams, and the spec. Verified facts (source-checked, not assumed):

- `StreamableHttpServerConfig` + all nine builder methods used by the plan
  exist in `rmcp-3.1.2/src/transport/streamable_http_server/tower.rs`
  (`config.rs` does not exist in 3.1.2).
- `StreamableHttpService::new(impl Fn() -> Result<S, io::Error> + Send + Sync
  + 'static, Arc<M>, StreamableHttpServerConfig)`; implements
  `tower_service::Service<Request<RequestBody>>` with `Error = Infallible`.
- `NeverSessionManager` at `session/never.rs`, `Default + Clone`.
- `ProtocolVersion` newtype struct; constants `V_2024_11_05` … `V_2026_07_28`;
  `LATEST == V_2025_11_25`; `KNOWN_VERSIONS` list; negotiation falls back to
  the `get_info()` version, never rejects.
- `stateless_protocol_metadata_required` checks metadata presence, not a
  version allowlist; rmcp validates Accept (406), Content-Type (415), body
  limit (413), header/body mismatch (`ErrorCode::HEADER_MISMATCH = -32020`).
- `server/discover` implemented (`DiscoverRequestMethod`), default `discover`
  advertises `supported_protocol_versions()`.
- `ServerHandler::supported_protocol_versions() -> Cow<'static,
  [ProtocolVersion]>`; `InitializeResult::with_protocol_version` builder.
- rmcp feature name `transport-streamable-http-server`; rmcp does NOT
  re-export `tower-service`.
- Codebase: `MemoryError` variants (no `Auth`/`Unavailable` before this
  plan); `DbClient` trait shape (namespace param + `ensure_active_namespace`);
  `SurrealDbClient` fields/connectors; `MemoryService::new` public
  constructor; `StdoutLogger` (no `logging::init`); `metrics` +
  `metrics-exporter-prometheus` gated by `prometheus` feature (ADR-0048);
  workspace deps include `sha2`/`lru`/`hex`/`reqwest` but NOT
  `hmac`/`subtle`/`parking_lot`; edition 2024 (unsafe env setters).

Defects found and fixed in place (grouped):

1. **Compile blockers**: wrong `ProtocolVersion` API (was enum-style
   `V2025_07_28`); unsafe `env::set_var` in edition 2024 tests; undeclared
   `test-fixtures` feature (`unexpected_cfgs` under `-D warnings`); production
   binary calling test-gated `HttpState::default_for_test()`; nonexistent
   `logging::init()`; `parking_lot` dep not in workspace; `hmac`/`subtle`
   missing from the Phase 2 proposal; `tower-service` not re-exported by
   rmcp; `mod.rs` declaring modules with no files; `State` extractor tests
   without state; `MemoryError::Auth/Invalid/Unavailable` variants that did
   not exist.
2. **Correctness holes**: modern-only fallback (rmcp would negotiate
   `2025-11-25` without pinning `get_info().protocol_version`); API key
   parser contradicted its own format/tests (`ak_<uuid>` split semantics,
   dead iterator check, `+` in test secrets); `ensure_namespace` ignored
   SurrealDB's "DEFINE DATABASE applies to current namespace" rule;
   `RateLimiter` referenced but never defined; metrics recorder
   double-install panic path; CIDR prefix range unchecked.
3. **Rule violations**: `todo!()` in production paths; `unwrap()`/`expect()`
   in production code; dead `build_default` config builder; vacuous SSE test;
   placeholder conformance suite (now fully written incl. `spawn_server`);
   bash-only brace expansion in run commands.
4. **Consistency**: unified `Arc<HttpState>`; `OperationGuard` owns Arcs;
   single production `build_router` from Task 3.3 (no test-only router twin);
   File Structure dep list aligned with the Phase 2 proposal; lint gate
   includes `test-fixtures`.

Audit pass 3 (same day) re-read the plan end-to-end after the Phase 3
restructure and found + fixed 15 more defects:

1. **Compile blockers**: Task 3.5 middleware violated the axum `from_fn`
   contract (no `Next` parameter, never called `next.run`); Tasks
   3.6/3.8/3.9 each REDEFINED `HttpState` from scratch, dropping
   `mcp_factory` and reverting the async constructor; Task 3.10 called a
   sync `HttpState::new` that no longer existed; Task 8.4 implemented
   `rmcp::TaskManager`, which is a concrete struct in rmcp 3.1.2 (no trait
   exists) — rewritten as the `TaskBackend` enum over the `ServerHandler`
   overrides.
2. **Correctness holes**: Task 4.5 mapped a missing tenant to
   `MemoryError::Auth` (dead `NotFound` variant, wrong status class); Task
   6.1 `commit_with_fence` referenced `$gen` without ever binding it (every
   fence commit would have failed); Task 4.1 never replaced the stub
   `RegistryHandle` that Task 4.4 claimed existed (added Step 4.5); the Task
   3.11 conformance suite carried no Bearer credentials and would 401 on
   every `POST /mcp` from Task 4.6 on (fixed by the new Task 5.8
   test-fixtures bootstrap + suite update); Task 4.7 referenced
   `OperatorPrincipal`, `ApiError`, and `enqueue_provisioning` that were
   defined nowhere (all three defined in Task 4.7 now).
3. **Consistency**: stale `build_router_for_tests` / `mcp_handler_for_tests`
   / `tenantless_test_service` references removed; sync `default_for_test()`
   call sites updated to `.await` (3.6, 3.8, 3.9, 4.6); Task 3.4 tests made
   async to match the real `handlers.rs` fixtures (`test_service()`
   extraction); `connect_bound` duplication between 3.3/4.1 consolidated;
   Task 5.5 `watch`/`broadcast` mismatch unified on `broadcast`;
   `from_prebound` ordering fixed (needed by 5.3, canonical code in 5.4);
   Task 5.6 now explicitly removes `mcp_factory` + `build_tenantless_handler`
   and renames the constructor; Task 5.7 file refs corrected; Task 12.9 lint
   gate now includes `test-fixtures`; Task 10.1 gained Step 0 with the
   missing HttpConfig/HttpState/RegistryStore control-plane extensions.

Audit pass 4 (same day) checked the plan after the previous fixes and found
+ fixed 11 more defects:

1. **Security/correctness**: positive auth cache previously cached only
   `Account`, so a wrong secret with a known key id could bypass verification
   for the cache TTL; cache entries now retain `KeyedVerifier` and re-verify
   every supplied secret. The registry engine was previously conflated with
   the tenant engine; `PrivilegedEngine` is now built from `tenant_db`, while
   the registry store uses `control_db`. `connect_bound` now uses SurrealDB
   root authentication, which is required for namespace/database DDL. Fence
   updates use a closed `FenceUpdate` enum instead of interpolating a caller
   supplied SET clause.
2. **Compile/order blockers**: `RegistryStore` now has a concrete
   `find_tenant_by_id`, schema-version CAS, and state-aware tenant CAS;
   `TenantState`/`ProvisioningStage` duplication was removed. The shared
   local prebound constructor is landed before embedded registry/runtime use;
   the migration worker has one `provision_one` path and supports both
   `PrivilegedEngine` variants. The Dioxus crate declares a binary rather
   than a library-only package; the HTTP binary has `required-features`.
3. **Operational gaps**: `server::serve` now uses the shutdown token for
   axum graceful shutdown; the composition root installs Ctrl-C/SIGTERM
   handling, closes admission before cancelling, and passes connect-info so
   forwarded host headers are accepted only from configured proxy CIDRs.
   The configured request deadline is wired as middleware. Final test gates
   include `test-fixtures` wherever bootstrap is required, and the stale
   `ApiError`/registry-handle calls were corrected.

Remaining unverified (checked at implementation time, flagged in tasks):
exact SurrealDB v3 `LIVE SELECT` syntax (9.3), SurrealDB v3 `RETURN AFTER`
projection shape (6.1), Dioxus 0.6 router syntax (10.9), the exact OIDC/JWKS
provider API behavior (10.1/11.2), and the test-body expansion gate (§2
above).

Audit pass 5 (2026-08-28) cross-checked the plan against the published
2026-07-28 Streamable HTTP page/changelog and the installed rmcp 3.1.2 source:

1. **Protocol corrections**: removed the invalid modern `notifications/initialized`
   acceptance test; added a removed-`ping` negative test; added required
   `Mcp-Method` coverage and conditional `Mcp-Name` coverage; documented that
   `server/discover` is mandatory, sessions/GET/DELETE/resumption are absent,
   and request/response headers must match the body.
2. **Streaming correctness**: corrected the Axum `ConnectInfo<SocketAddr>` type;
   replaced `Extension<non-Clone permit>` extraction with explicit removal from
   request extensions; made `LeasedBody` release guards on end/error; added a
   body-polling deadline rather than timing only the handler future; and exempted
   `subscriptions/listen` from the ordinary deadline while giving it a separate
   bounded admission budget and no full-runtime pin.
3. **Storage/worker correctness**: made Tenant retry stage and lease owner/id/
   generation durable; added fenced RegistryStore transitions and atomic claim,
   heartbeat, and release seams; made provisioning consume only a claimed lease
   and wired jittered, joined heartbeats; moved scheduler parallelism acquisition
   inside tracked jobs so shutdown cannot hang waiting for a slot.
4. **Feature-boundary correctness**: cfg-split durable Task/App/Subscription
   backends so `mcp-apps` and the default stdio build never import HTTP modules;
   made App Sessions and `subscriptions/listen` use rmcp's native handler seams;
   added request-scoped subscription reauthorization; and registered all four
   maintenance jobs in the final binary hook list.
5. **OIDC correctness**: made OIDC startup optional when the control plane is
   disabled; added redirect URI/env validation, RFC 9207 `iss` checking,
   string-or-array audience deserialization, bounded single-flight JWKS refresh,
   and async token validation shared by browser login and future MCP OAuth.

This pass intentionally does not claim compile-time certainty for SurrealDB,
rmcp body bounds, Dioxus, or a concrete OIDC provider until the implementation
checkpoints run.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-08-27-streamable-http-saas.md`.

Two execution options:

**1. Subagent-Driven (recommended)** — Fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using `executing-plans`, batch execution with checkpoints.

Which approach? (Phase 2 has an explicit approval gate; if Subagent-Driven is chosen, Task 2.1 still requires the user to confirm before any `Cargo.toml` change.)
