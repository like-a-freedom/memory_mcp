# Local administrator authentication and client provisioning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Provide local administrator login and client/API-key management without an OIDC provider.

**Architecture:** Keep local administrator identities and sessions separate from client Accounts/Tenants. Mode-selected HTTP adapters, an administrator CLI and Dioxus pages use service workflows backed by durable registry transactions. Preserve key-derived tenant isolation and the existing OIDC deployment option.

**Tech Stack:** Rust 1.97.1, Tokio, Axum 0.8.9, SurrealDB 3.2.4, Dioxus 0.7, existing cryptographic primitives; a password-KDF dependency requires separate approval.

**Spec:** [Approved requirements and proposed technical design](../specs/2026-09-18-local-admin-auth.md).

**Architecture decision:** [ADR-0055](../../adr/0055-local-admin-authentication-for-remote-deployment.md). It records the approved direction without approving the technical proposals or execution.

## Global Constraints

- Workspace Rust floor: `1.97.1`; edition `2024`.
- Keep `main.rs` thin; business workflows belong in `service/`.
- Errors remain thiserror-based; sanitize infrastructure errors at adapter boundaries.
- Features remain additive; package `default = []`; preserve stdio behavior.
- Preserve the eight-tool MCP surface and key-derived Account/Tenant isolation.
- Ask before changing dependencies, generated code, or migration files; do not edit historical migrations.
- Do not commit unless separately requested.
- Never delete facts; no client deletion or quota-editing UI in this feature.

---

**Date:** 2026-09-18  
**Status:** Proposed implementation; not authorized for execution.  
**Specification:** [Approved R1–R12 and proposed design](../specs/2026-09-18-local-admin-auth.md)  
**Review:** [Source/security review and remaining blockers](2026-09-18-local-admin-auth-review.md)

## 1. Scope, gates and execution rules

Implement deployment-exclusive local administrator authentication, administrator-only client provisioning and key management, while retaining key-derived tenant isolation. Local admins are not Accounts, Tenants, OIDC operators or data-plane principals. The eight MCP tools remain unchanged. `main.rs` remains parsing/dispatch only; business workflows belong in `service/`; default features remain `[]`.

This document is **not permission to implement**. The documentation scope includes this plan, its companion spec, the requested review report, and ADR-0055 with related cross-references. Preserve preexisting `README.md` edits. No application, dependency, migration, generated file or commit changes are authorized now.

Before implementation:

- [ ] Approve technical proposals separately from R1–R12: CLI-selected username, password policy/KDF parameters, timings/throttle collisions and direct-peer aggregation, offline mode/rotation procedure, ready-only suspend, local-created-client listing, idempotency/one-time-response semantics and image packaging.
- [ ] Obtain explicit dependency permission with exact Argon2 crate/version/features, RNG compatibility, Rust 1.97.1 compatibility and lockfile delta. Gate it under `control-plane`; do not expand default features or hand-write a KDF. Browser test tooling and any UI bindings require their own dependency review if additions are necessary.
- [ ] Obtain explicit migration permission for the complete schema, postconditions, upgrade/restart tests and operational rollback. `047_local_admin_auth.surql` is a proposed absent file; recheck its number immediately before creation. Never edit 001/045/046.
- [ ] Stop all older control-plane replicas before enforcing the durable mode policy. Old binaries cannot honor a table they do not know. No online mode switching or new mode-switch CLI is in scope.

Rust 1.97.1 / edition 2024; Axum 0.8.9; SurrealDB 3.2.4; Dioxus 0.7. Existing HMAC/SHA-256, rand, subtle, Tokio, thiserror and serde are reusable. Axum's `json` feature is **not currently enabled**: use bounded body reads plus serde, or request separate dependency-feature approval. Do not silently assume `axum::Json` is available.

### Evidence and test notation

**Existing** means verified live source, not tested behavior. **NEW** means a proposal to implement. All names in §§3–4 are NEW unless explicitly identified as existing. Task checklists are acceptance contracts, not executable tests.

The complete Rust test modules in T1, T3 and T4 are intended to be installed at their stated locations during implementation. They have imports and helper bodies and depend only on the named proposed production interfaces. They are **not currently runnable against the unmodified repository**, and were not compiled during this documentation review. No partial SQL transaction, undefined `ready_client`, browser `page`/`expect`, or fake PHC sample is presented as executable evidence.

For each task: write the test first, run RED, implement the minimum behavior, run GREEN, review the diff and security invariant. A missing proposed symbol may be the first RED; obtain a behavioral RED once the interface compiles. Unrelated compilation failures and zero selected tests are not RED/GREEN evidence. Use `--lib` for inline unit tests; use `--test NAME` only after that NEW target exists. Record actual test counts. Never weaken predicates to satisfy fixtures.

## 2. Verified source map and change inventory

All paths below are repository-relative; abbreviated sibling names are expanded in prose, not shell globs. Use graph/structural navigation and targeted reads again at implementation time. The spec §3 and review contain source evidence and caveats.

| Existing paths | Required work / reuse boundary |
|---|---|
| `crates/memory-mcp/src/http/config.rs`; `crates/memory-mcp/src/http/config/types.rs`; `crates/memory-mcp/src/http/config/parse.rs`; `crates/memory-mcp/src/http/config/validate.rs` | Mode-specific parsing, facade exports, strict plan and HTTPS validation. `load_signup_plan_limits` is `pub(super)` and returns `Result<Option<PlanLimits>, MemoryError>`, not a mandatory plan. |
| `crates/memory-mcp/src/http.rs`; `crates/memory-mcp/src/http/composition.rs`; `crates/memory-mcp/src/http/runtime/bootstrap.rs`; `crates/memory-mcp/src/http/test_state.rs`; `crates/memory-mcp/src/http/oauth.rs` | Update real `assemble` caller as well as test builders; optional local services; no OIDC discovery in local/off. Reuse one concrete store Arc, never downcast RegistryHandle or reopen RocksDB. |
| `crates/memory-mcp/src/http/registry/models.rs`; `crates/memory-mcp/src/http/registry/storage.rs`; `crates/memory-mcp/src/http/registry/surreal_store.rs`; `crates/memory-mcp/src/http/registry/migrations.rs` | Existing Account/Tenant/Plan/ApiKey ownership. Explicit transaction-result adapter, local capability and small OIDC fencing additions; no omnibus capability refactor. |
| `crates/memory-mcp/src/http/registry/provisioning.rs`; `crates/memory-mcp/src/http/leases/migration.rs` | Worker scans Tenant rows, not an event queue. Repair `NamespaceCreating` restart and suspended-row scheduling behavior as required for R5/R7. |
| `crates/memory-mcp/src/control/session.rs`; `crates/memory-mcp/src/control/csrf.rs`; `crates/memory-mcp/src/control/recent_auth.rs`; `crates/memory-mcp/src/control/secret.rs`; `crates/memory-mcp/src/control/error.rs` | Reuse primitive conventions, not Account-based identity or existing error mapping. The live OIDC session uses String cookie hash; the similarly named registry model uses `[u8;32]`. |
| `crates/memory-mcp/src/control/application/api_keys.rs`; `crates/memory-mcp/src/control/application/oidc_signup.rs`; `crates/memory-mcp/src/control/oidc/handlers.rs`; `crates/memory-mcp/src/control/account_api.rs`; `crates/memory-mcp/src/control/operator.rs` | Extract key material only; local persistence must atomically audit. Fenced OIDC account/session writes. Existing Tenant-only suspend/resume is insufficient. |
| `crates/memory-mcp/src/http/principal/auth.rs`; `crates/memory-mcp/src/http/principal/cache.rs`; `crates/memory-mcp/src/http/principal/api_keys.rs` | Cache-hit key status/expiry revalidation; preserve credential formatting and tenant binding. |
| `crates/memory-mcp/src/http/router.rs`; `crates/memory-mcp/src/http/middleware/auth.rs`; `crates/memory-mcp/src/http/middleware/host_origin.rs`; `crates/memory-mcp/src/http/server.rs` | Exclusive route sets, explicit merged-router protections and API 404. Existing server supplies ConnectInfo; forwarded client-IP parsing does not exist. |
| `crates/memory-mcp/src/service.rs`; `crates/memory-mcp/src/control.rs`; `crates/memory-mcp/src/cli.rs`; `crates/memory-mcp/src/cli/args.rs`; `crates/memory-mcp/src/cli/commands.rs`; `crates/memory-mcp/src/runner.rs` | Feature-gated modules, CLI `mode_label` and `into_one_shot` exhaustive matches, early dispatch. |
| `crates/control-plane-ui/src/api.rs`; `crates/control-plane-ui/src/router.rs`; `crates/control-plane-ui/src/pages.rs`; `crates/control-plane-ui/src/main.rs`; `crates/control-plane-ui/src/pages/login.rs` | Separate local UI/client DTOs. No backend dependency; preserve OIDC pages. |
| `crates/memory-mcp/build.rs`; `crates/memory-mcp/src/control/static_assets.rs`; `Dockerfile`; `docker-compose.yml`; `.github/workflows/ci.yml` | Embed real assets; ship CLI and HTTP; retain nonroot/shell-free runtime and migration path. Compose currently injects OIDC-only/sample keys. |
| `crates/memory-mcp/tests/http_control_plane.rs`; `crates/memory-mcp/tests/common/http_server.rs`; `crates/memory-mcp/tests/common/mod.rs` | Existing subprocess/restart framework. Add explicit local fixtures: existing helpers inject HTTP origins and OIDC settings. OIDC discovery mock is not callback coverage. |

### NEW files and visibility

Verify parent directories before creation. These files do not exist as part of this deliverable.

| NEW path | Owner |
|---|---|
| `crates/memory-mcp/src/service/local_admin/mod.rs` | T1 registration; T4 crate-visible constructors and exports |
| `crates/memory-mcp/src/service/local_admin/contracts.rs` | T2 domain commands/errors/store trait; T7 client extensions |
| `crates/memory-mcp/src/service/local_admin/policy.rs` | T1 username/password policy; T7 expiry/name policy |
| `crates/memory-mcp/src/service/local_admin/password.rs` | T3 bounded KDF |
| `crates/memory-mcp/src/service/local_admin/auth.rs` | T4 authority, management and browser-auth services |
| `crates/memory-mcp/src/service/local_admin/clients.rs` | T7 client service |
| `crates/memory-mcp/src/service/local_admin/tests.rs` | T4/T7 complete service fixtures and tests |
| `crates/memory-mcp/src/service/credential_material.rs` | T4 random/HMAC primitives; T7 reusable key material, independent of control adapters |
| `crates/memory-mcp/src/http/registry/surreal_store/local_admin.rs` | T2/T4 durable policy/identity/challenge/session transactions |
| `crates/memory-mcp/src/http/registry/surreal_store/local_admin_rate.rs` | T4 durable reservation/denied aggregates |
| `crates/memory-mcp/src/http/registry/surreal_store/local_admin_clients.rs` | T7 client/key/CAS/audit transactions |
| `crates/memory-mcp/migrations/047_local_admin_auth.surql` | T2, separately gated |
| `crates/memory-mcp/src/cli/commands/admin.rs` | T5 registry-only CLI |
| `crates/memory-mcp/src/control/local_admin/mod.rs` | T6 router/allowlisted error mapping |
| `crates/memory-mcp/src/control/local_admin/auth.rs` | T6 HTTP DTOs, CSRF/cookies and session extraction |
| `crates/memory-mcp/src/control/local_admin/clients.rs` | T7 HTTP client adapters |
| `crates/memory-mcp/tests/http_local_admin.rs`; `crates/memory-mcp/tests/local_admin_cli.rs`; `crates/memory-mcp/tests/local_admin_durable.rs` | T5/T6/T10 black-box, CLI and remote tests |
| `crates/control-plane-ui/src/admin_api.rs`; `crates/control-plane-ui/src/pages/admin_auth.rs` | T8 wire client/auth components |
| `crates/control-plane-ui/src/pages/admin_clients.rs`; `crates/control-plane-ui/src/pages/admin_client.rs` | T9 list/detail components |
| `scripts/ci/local_admin_browser.mjs`; `scripts/ci/local_admin_image.py` | T10 NEW acceptance harnesses, not installed tooling |
| `docs/operations/LOCAL_ADMIN.md` | T10 evidence-based runbook after implementation |

Keep `service::local_admin` and its service/contracts exports `pub(crate)` under `control-plane`. Unit tests can access them. External integration tests use HTTP/CLI; put direct store race tests in inline adapter modules and run them with `--lib`. Do not claim external tests can import a private service module. No feature-gated public test API is needed for this plan. Existing public registry types retain their current visibility. Register the shared credential module under `control-plane`, because its OIDC callers are already gated there.

## 3. Proposed interface ledger

This is a normative design ledger, **not a compilable declaration file**. Every command field is listed; `String` identifiers are validated server-generated ids unless otherwise stated. Sensitive types have redacted Debug, no blanket Serialize; HTTP DTOs are separate. All local operations return `LocalResult<T> = Result<T, LocalAdminError>`.

### 3.1 Errors and common types

Define thiserror-based `LocalAdminError` in contracts with variants: `InvalidInput`, `InvalidCredentials`, `InvalidChallenge`, `Unauthenticated`, `Forbidden`, `ReauthRequired`, `NotFound`, `StateConflict`, `VersionConflict`, `IdempotencyConflict`, `KeyCap`, `SecretAlreadyIssued { key_id: String }`, `Throttled { retry_after_seconds: u32 }`, `Unavailable`, and `Infrastructure(MemoryError)` with `From<MemoryError>`. Display strings are constant/safe; infrastructure is not transparent-display. Its custom Debug also redacts the underlying error. Do not string-parse a public key id or Retry-After out of MemoryError.

HTTP maps to spec §8: 400 input/challenge; 401 credentials/session; 403 forbidden/reauth; 404; 409 state/version/idempotency/cap/already-issued; 429; 503 infrastructure/admission. Body parsing separately maps 413/415. CLI converts to safe MemoryError categories before the existing runner formatter; never hand raw database error text to it. Preserve existing OIDC error behavior rather than globally changing its mapper.

| Type | Fields / meaning |
|---|---|
| `BrowserAuthMode` | `Local`, `Oidc`; defined/re-exported by HTTP config, not duplicated in service |
| `BrowserPolicyFence` | `mode: BrowserAuthMode`, `epoch: u64`; defined in registry models under `control-plane` so OIDC persistence need not import local business logic |
| `LocalKeyFingerprints` | `session: [u8;32]`, `csrf: [u8;32]`; purpose-separated HMAC over fixed configuration labels, never raw keys/log fields |
| `AdminState`; `ChallengeKind` | `PendingActivation/Active/RecoveryRequired`; `Activate/Reset` |
| `RequestContext` | `request_id: uuid::Uuid`; generated by server/CLI, never trust inbound correlation text |
| `AuthAttemptContext` | `request: RequestContext`, `source: std::net::IpAddr`; normalized direct peer only |
| `AdminFence` | `admin_id: String`, `session_id: String`, `credential_generation: u64`, `policy: BrowserPolicyFence` |
| `AdminPrincipal` | `fence: AdminFence`, `username: String`, `auth_time: DateTime<Utc>`, `absolute_expiry: DateTime<Utc>` |
| `CredentialSnapshot` | `admin_id: String`, `username: String`, `state: AdminState`, `credential_generation: u64`, `password_phc: Option<String>`; deliberately no contention version |
| `ChallengeIssue` | `username: String`, `kind: ChallengeKind`, `verifier: [u8;32]`, `policy: BrowserPolicyFence`, `request: RequestContext`; store fixes TTL at 900 seconds |
| `IssuedChallenge` | `admin_id: String`, `username: String`, `expires_at: DateTime<Utc>` |
| `ChallengeView` | `username: String`, `expires_at: DateTime<Utc>`; no credential/generation exposed |
| `ChallengeFinish` | `verifier: [u8;32]`, `kind: ChallengeKind`, `password_phc: String`, `policy: BrowserPolicyFence`, `request: RequestContext` |
| `SessionOpen` | `credential: CredentialSnapshot`, `cookie_verifier: [u8;32]`, `policy: BrowserPolicyFence`, `request: RequestContext` |
| `SessionRotate` | `fence: AdminFence`, `credential: CredentialSnapshot`, `cookie_verifier: [u8;32]`, `request: RequestContext` |
| `OneTimeChallenge`; `AdminLogin` | `{issued: IssuedChallenge, code: String}`; `{principal: AdminPrincipal, cookie: String}`; no Serialize |
| `AttemptDomain` | `Credentials`, `Challenge`; login/reauth share first, inspect/activate/reset share second |
| `AttemptInput` | `domain: AttemptDomain`, `username_bucket: Option<u16>`, `source_bucket: u16`, `policy: BrowserPolicyFence`, `request: RequestContext`; username required only for Credentials; buckets 0–4095 |
| `AttemptDecision` | `Allowed`, `Limited { retry_after_seconds: u32 }`; reservation atomic across all required dimensions |
| `FailureAudit` | `request: RequestContext`, `policy: BrowserPolicyFence`, `action: FailureAction`, `reason: FailureReason`, `admin_id: Option<String>`, `username_bucket: Option<u16>`, `source_bucket: Option<u16>`, `admitted_auth_attempt: bool` (service-set, not request input) |
| `FailureAction`; `FailureReason` | Actions `Login/Reauth/Challenge/Session/ClientMutation`; reasons `InvalidCredentials/InvalidChallenge/InvalidSession/StaleFence/Forbidden`; closed enums, no raw error text |
| `PageRequest`; `Page<T>` | `{after: Option<String>, limit: u16}`; `{items: Vec<T>, next_cursor: Option<String>}`; default 50/max100, validated opaque id cursor |

Counters/generations/epochs are checked against Surreal signed integer range; overflow fails closed, never wraps. A request id also deduplicates the one admitted failure audit if its write has an uncertain outcome. Failure auditing is not a second success transaction. `record_failure` appends an event only for a service-reserved authentication attempt; session/client/pre-admission denials instead increment fixed action/reason aggregate slots in `local_admin_rate_bucket` (separate security-denial domain, no user-controlled cardinality). No append-per-invalid-cookie or append-per-stale-mutation path. Apply bounded process admission before this I/O; aggregate write failure still fails closed. Failure recording must accept a stale supplied epoch as an event attribute, not require that the failed authentication suddenly be valid; it grants no authority and cannot mutate credentials/resources. Otherwise rejecting a stale fence would always turn into an unrelated audit failure.

### 3.2 Local store and service boundaries

`LocalAdminStore: Send + Sync + 'static` uses the existing async-trait convention. Methods have no default success bodies. Introduce each method with its complete production implementation and tests in the owning task; the ledger is the final surface, not an instruction to leave unimplemented methods between tasks.

All methods below are async, take `&self`, return `LocalResult<Return>`; argument names/types are normative.

| Store method | Arguments → Return | Task |
|---|---|---|
| `join_local_policy` | `fingerprints: LocalKeyFingerprints` → `BrowserPolicyFence` | T2 |
| `issue_challenge` | `command: ChallengeIssue` → `IssuedChallenge` | T2 |
| `inspect_challenge` | `verifier: &[u8;32], kind: ChallengeKind, policy: &BrowserPolicyFence` → `ChallengeView` | T2 |
| `finish_challenge` | `command: ChallengeFinish` → `()` | T2 |
| `credential` | `username: &str, policy: &BrowserPolicyFence` → `Option<CredentialSnapshot>` | T4 |
| `open_session` | `command: SessionOpen` → `AdminPrincipal` | T4 |
| `resolve_session` | `cookie_verifier: &[u8;32], policy: &BrowserPolicyFence` → `AdminPrincipal` | T4 |
| `rotate_session` | `command: SessionRotate` → `AdminPrincipal` | T4 |
| `revoke_session` | `fence: &AdminFence, request: &RequestContext` → `()` | T4 |
| `reserve_attempt` | `input: AttemptInput` → `AttemptDecision` | T4 |
| `record_failure` | `event: FailureAudit` → `()` | T4 |
| Client methods | Defined in §3.4 | T7 |

`LocalAdminAuthority::join(store: Arc<dyn LocalAdminStore>, session_key: [u8;32], csrf_key: [u8;32]) -> LocalResult<Arc<Self>>` is async. It computes fingerprints, joins the durable Local policy, and privately owns store/keys/fence. No public constructor accepts an arbitrary epoch. It initializes no KDF, tenant runtime or plan. Never use `join_local_policy` as a mode switch or key rotation.

`AdminManagementService::new(authority: Arc<LocalAdminAuthority>) -> Self` is synchronous. Async methods `create_admin(&self, username: &str, request: &RequestContext)` and `recover_admin(...)` return `LocalResult<OneTimeChallenge>`. This is the CLI path and needs **no PasswordHasher**.

`LocalAdminService::new(authority: Arc<LocalAdminAuthority>, hasher: Arc<PasswordHasher>) -> Self` is synchronous. Its async methods:

| Method | Arguments after `&self` → `LocalResult<Return>` |
|---|---|
| `inspect_challenge` | `context: &AuthAttemptContext, code: &str, kind: ChallengeKind` → `ChallengeView` |
| `finish_challenge` | `context: &AuthAttemptContext, code: &str, kind: ChallengeKind, password: String` → `()` |
| `login` | `context: &AuthAttemptContext, username: &str, password: String` → `AdminLogin` |
| `resolve` | `request: &RequestContext, cookie: &str` → `AdminPrincipal` |
| `reauthenticate` | `context: &AuthAttemptContext, principal: &AdminPrincipal, password: String` → `AdminLogin` |
| `logout` | `request: &RequestContext, principal: &AdminPrincipal` → `()` |

The service owns attempt reservation before identity/code lookup or KDF, including invalid-format usernames. There is no separately exported HTTP `reserve_attempt` followed by an optional unthrottled login. Reauth derives the canonical username from its resolved principal, not request input. Store reads still check mode; mutation authorization is rechecked transactionally. HTTP is responsible for bounded parsing, trusted peer extraction, Origin and CSRF, never durable admission policy.

`PasswordHasher::new() -> LocalResult<Self>` initializes supported parameters, dummy PHC and bounded admission. Async `hash(&self, password: String) -> LocalResult<String>` and `verify(&self, password: String, phc: Option<String>) -> LocalResult<bool>`: None performs one dummy verification and always returns false. Password input validation is mandatory for hash; login accepts bounded strings without revealing account state. Oversized request/password inputs are handled uniformly before expensive work. Do not run a dummy hash generation per failed login.

### 3.3 Transaction substrate and OIDC compatibility

Add `SurrealHandle::query_json_at(&self, sql: &str, vars: Option<serde_json::Value>, result_index: usize) -> Result<Vec<serde_json::Value>, MemoryError>` with implementations for **both Local and Remote**. Preserve `query_json` index-0 semantics for old callers. New method checks every statement error before extracting the explicit result index; missing/malformed expected results are errors, not empty success. Each transaction owns a constant SQL body, documented result index and strict decoder. T2 tests the actual index of RETURN for BEGIN/LET/UPDATE/COMMIT on SurrealDB 3.2.4; never guess it from the SQL line count. No dynamic identifiers from request data.

Shared guard order is policy → admin → presented session → client sidecar → resource. Each required guard must be a real conditional write, not only SELECT. Session guard can rewrite a bounded idle deadline while requiring unrevoked/current generation/epoch and both deadlines; no new session-version field is required. Policy/admin/sidecar have contention versions. Credential revision is only `credential_generation` plus exact stored PHC/state comparison. General guard writes must not invalidate an in-progress valid KDF result. Retries rerun predicates with fresh DB time; bounded attempts end in 503. Test global policy/admin contention throughput: correctness-first shared guards may serialize traffic, and sharding them is not allowed without new proof/review.

For OIDC, add small gated operations to existing `RegistryStore`, implemented in both durable and fixture stores:

- `join_oidc_policy(&self) -> Result<BrowserPolicyFence, MemoryError>`: singleton compare/create, never switch.
- `create_oidc_account_bundle(&self, policy: &BrowserPolicyFence, account: &Account, tenant: &Tenant, identity: &ExternalIdentity) -> Result<(), MemoryError>`: policy guard and account bundle in **one** transaction. Keep general `create_account_bundle` unchanged for data-plane/bootstrap callers.
- Add `policy: &BrowserPolicyFence` to existing OIDC-only `store_oidc_request`, `take_oidc_request`, `store_session`, `find_session`, `touch_session`, `delete_session` signatures; update every call and both store implementations. Require OIDC mode/current epoch in the same transaction as writes/consumption. For touch, pass the cookie hash as well as session id and compute idle expiry in DB, rather than accepting an arbitrary timestamp. Resolve/touch must not recreate a missing/expired row.
- Add `browser_policy_epoch: Option<u64>` to the **live `control::session::ControlPlaneSession`**, decoder and approved additive schema. Newly created sessions always have Some(current epoch). Missing/old epoch fails authentication; existing users log in again after upgrade. The unrelated same-named registry DTO must not accidentally replace this type.

`HttpState` stores optional `browser_policy: BrowserPolicyFence` initialized by composition, not a browser value. OIDC handlers/signup use the dedicated guarded method; session middleware passes the fence to persistence. Stop older replicas, clear pending OIDC requests and rotate OIDC state/session keys during offline mode transitions; no callback survives away-and-back mode switching. Test legacy-session rejection explicitly. This is a documented compatibility proposal, not a claim of zero-impact migration.

### 3.4 Client and key contracts

All local types below live in contracts; existing Account/Tenant/ApiKeyMeta/KeyedVerifier remain registry types. Wire status names use existing snake_case serialization.

| Type | Fields |
|---|---|
| `KeyExpiry` | serde internally tagged `kind`, deny unknown fields: `Never`, `Days { days: u32 }`; snake_case |
| `ClientCreate` | `display_name: String`, `operation_id: uuid::Uuid` |
| `ClientView` | `account_id: String`, `tenant_id: String`, `display_name: String`, `account_status: AccountStatus`, `tenant_status: TenantStatus`, `plan_version: u32`, `schema_version: u32`, `version: u64`, `provisioning_reason: Option<String>` (closed safe reason mapping) |
| `ClientBundle` | `account: Account`, `tenant: Tenant`, `display_name: String`, `operation_id: uuid::Uuid`, `request_fingerprint: [u8;32]` |
| `AdminKeyCreate` | `name: String`, `expiry: KeyExpiry`, `operation_id: uuid::Uuid` |
| `AdminKeyInsert` | `account_id: String`, `key_id: String`, `name: String`, `verifier: KeyedVerifier`, `expiry: KeyExpiry`, `operation_id: uuid::Uuid`, `request_fingerprint: [u8;32]` |
| `KeyInsertOutcome` | `Created(ApiKeyMeta)`, `AlreadyIssued { key_id: String }` |
| `IssuedClientKey` | `id: String`, `name: String`, `secret: String` (full mem_sk credential), `expires_at: Option<DateTime<Utc>>` |
| `ClientStateAction` | `Suspend`, `Resume` |

NEW store methods, all async with `&self`, return `LocalResult<T>`:

| Method | Arguments → T |
|---|---|
| `create_client` | `fence: &AdminFence, request: &RequestContext, bundle: ClientBundle` → `ClientView` |
| `list_clients` | `fence: &AdminFence, page: PageRequest` → `Page<ClientView>` |
| `client` | `fence: &AdminFence, account_id: &str` → `ClientView` |
| `list_client_keys` | `fence: &AdminFence, account_id: &str, page: PageRequest` → `Page<ApiKeyMeta>` |
| `insert_client_key` | `fence: &AdminFence, request: &RequestContext, command: AdminKeyInsert` → `KeyInsertOutcome` |
| `revoke_client_key` | `fence: &AdminFence, request: &RequestContext, account_id: &str, key_id: &str` → `()` |
| `set_client_state` | `fence: &AdminFence, request: &RequestContext, account_id: &str, expected_version: u64, action: ClientStateAction` → `()` |

`ClientAdminService::new(authority: Arc<LocalAdminAuthority>, plan_version: u32, pepper: String) -> Self`; no second RegistryStore dependency. Storage loads Account/Tenant/plan inside the atomic operation. Async service methods `create`, `list`, `get`, `keys`, `issue_key`, `revoke_key`, `set_state` mirror the store operations with `&AdminPrincipal` instead of fence; create takes ClientCreate, issue_key takes account_id and AdminKeyCreate. All mutators take `&RequestContext`. `issue_key` returns `LocalResult<IssuedClientKey>`: map AlreadyIssued to the typed SecretAlreadyIssued error and discard newly generated material. HTTP returns 409 with public key_id, never a second secret.

Shared material helper in NEW `service/credential_material.rs`: `generate_api_key_material(pepper: &[u8]) -> (String, KeyedVerifier, String)` returns `(key_id, verifier, full_credential)`, using existing `new_api_key_id`, cryptographic random token and `KeyedVerifier::compute`. It does **not** choose name/account/expiry/created_at, create records or audit. Existing OIDC key workflow can build its existing ApiKey using this helper without importing local-admin client workflow. Auth HMAC/random helpers also live here; service must not depend on `control::secret`.

Key insertion derives created_at/expires_at from DB time after admission and returns persisted ApiKeyMeta. Pure `expiry_at(&KeyExpiry, DateTime<Utc>) -> LocalResult<Option<DateTime<Utc>>>` tests arithmetic; storage enforces the equivalent checked rule, not a caller deadline. Name validation trims outer whitespace, requires 1–100 scalars/≤400 bytes, rejects controls. Request fingerprint is deterministic over normalized name/display name plus explicit expiry kind/days; no secret or newly generated candidate id in it. Scope operation uniqueness by admin id + operation kind + UUID; retain with resource, no reuse-enabling TTL.

## Task 1 — Configuration, pure policy and composition call sites

Files: existing config facade/types/parse/validate, `http.rs`, `http/oauth.rs`, real bootstrap/composition/test_state and config consumers in session/CSRF/OIDC/middleware. NEW service module/policy and gated registration in `service.rs`.

- [ ] Add `BrowserAuthConfig::{Local(LocalBrowserConfig),Oidc(OidcBrowserConfig)}` and `HttpConfig.browser_auth: Option<BrowserAuthConfig>`. Local config fields: session_key, csrf_key `[u8;32]`, default_plan_version `u32`, default_plan_limits existing PlanLimits. OIDC config owns existing OIDC strings, allowlist, signup policy and HmacKeys. Redact Debug of config and keys; never zero-fill missing secrets.
- [ ] Parse mode and enable flags before secrets. Off requires no browser keys/signup/plan and initializes neither OIDC nor KDF. Local rejects nonempty OIDC-only settings/open signup, requires HTTPS and every explicit limit. OIDC retains required validation/discovery. Default mode OIDC is a compatibility proposal.
- [ ] `load_signup_plan_limits` returns Option: Local must require Some; preserve its sibling visibility or call through a narrowly exposed config helper. Re-export new types in `http/config.rs`. Update all field references and both real/test composition callers; borrowed matching uses `config.browser_auth.as_ref()`.
- [ ] Ensure local plan via T2 compare/create; never ensure hardcoded `free` for local/off. Reject plan-version 0, overflow and invalid concurrency/limits. No browser-provided plan.
- [ ] Config tests first construct a **valid HTTPS local config and assert validate succeeds**, then mutate one setting and assert the precise error. Existing HTTP fixture plus generic is_err can pass for the wrong reason. Environment tests use serialized existing test patterns; never mutate process env concurrently.
- [ ] Test all missing-seven-limit cases, contradictory settings, invalid auth mode, enabled UI without feature/control, and off with browser variables omitted.

Complete test module for NEW `policy.rs`, after defining the two production signatures `normalize_username(&str) -> LocalResult<String>` and `validate_password(&str) -> LocalResult<()>`. Do not also register another module named tests in that file.

```rust
#[cfg(test)]
mod tests {
    use super::{normalize_username, validate_password};

    #[test]
    fn local_admin_username_policy() {
        assert_eq!(normalize_username("  Ops.One  ").expect("valid"), "ops.one");
        for raw in ["ab", "a b", "аdmin", "a\nadmin", "-admin"] {
            assert!(normalize_username(raw).is_err());
        }
        assert!(normalize_username(&"a".repeat(64)).is_ok());
        assert!(normalize_username(&"a".repeat(65)).is_err());
    }

    #[test]
    fn local_admin_password_policy() {
        assert!(validate_password(&"a".repeat(14)).is_err());
        assert!(validate_password(&"a".repeat(15)).is_ok());
        assert!(validate_password(&"界".repeat(128)).is_ok());
        assert!(validate_password(&"a".repeat(129)).is_err());
        assert!(validate_password("a valid password\0").is_err());
        assert!(validate_password("              x").is_ok());
    }
}
```

The final assertion verifies leading spaces are not trimmed from passwords. Implement ASCII username canonicalization exactly as spec; invalid public login input goes through the generic dummy path, not this validation error directly.

RED/GREEN: `cargo test -p memory_mcp --lib --features control-plane --locked local_admin_username_policy`; repeat with `local_admin_password_policy`. Config suite: `cargo test -p memory_mcp --lib --features control-plane,test-fixtures --locked http::config`. Gate: positive test counts and `cargo check -p memory_mcp --no-default-features --locked`.

## Task 2 — Schema, result decoding and durable fences

Files: registry storage/models/adapter/catalog, NEW adapter/contracts, composition; gated migration. T2 implements challenge transactions using validated PHC input supplied later by T3/T4. Test transaction fixtures must use a real supported PHC from T3 when claiming end-to-end activation.

- [ ] Approve migration first: eight new tables from spec §7; deny unprivileged operations; unique canonical username, verifier, session verifier, sidecar Account id, operation tuple and policy singleton. Include audit failure deduplication/request id, safe anonymous buckets/access-grant flag and saturating throttle-denied counts. Include optional OIDC session epoch in the same approval, not a later surprise. No new tenant migration.
- [ ] Implement `query_json_at` first and test result selection plus error propagation when a nonselected statement fails. Test rollback and missing-result decode separately. Cover Mem and real remote server; do not make a transaction test accidentally assert an empty index-0 result.
- [ ] Test schema fresh/upgrade-from-046, checksum mismatch, partial apply lease recovery, simultaneous startup and required indices/fields. Strictly decode required ids/statuses/numeric bounds; do not silently default an invalid admin row.
- [ ] `ensure_local_plan(&self, plan: &Plan) -> Result<(), MemoryError>` on durable store: unique version compare/create and compare **every** limit after races. Use id `local_plan_v{version}`, never reuse `free` for new versions. Existing version with different limits fails; compatible existing OIDC version may be reused without overwrite.
- [ ] Policy creation is atomic, joins compare mode/epoch/config fingerprints. Local key drift fails startup. Rotation is reviewed offline maintenance, increments epoch, revokes sessions/challenges, changes keys and delays public auth 15 minutes to avoid an effective bucket reset under new HMAC placement. Retain existing counters.
- [ ] Challenge create: unique canonical admin, no Account/Tenant, fixed 900-second DB TTL, verifier only and CLI audit in one transaction. Recovery: shared guards, generation increment, RecoveryRequired, revoke old sessions/challenges, insert Reset verifier/audit atomically.
- [ ] Finish: DB now; policy/admin guards; valid kind/generation/epoch/state/unconsumed/unrevoked/unexpired challenge; update PHC/state/generation; consume and revoke siblings/old sessions; append audit. No session created. Inspect is nonconsuming and returns only username/expiry.
- [ ] Add OIDC operations from §3.3 and update both store implementations and all structural-search call sites. General bundle operation remains mode-neutral. Upgraded old OIDC sessions require fresh login. Regression tests must create epoch-bearing sessions explicitly.

Use `connect_in_memory` for real Surreal Mem adapter tests (it migrates). For two handles over one Mem engine use `from_local_db(Arc<Surreal<Db>>, ns, db)` twice, call `apply_migrations` on the first before testing. It does not migrate itself. Separate remote connections/processes are mandatory before claiming distributed safety; two objects sharing a mutex prove nothing.

RED/GREEN: `cargo test -p memory_mcp --lib --features control-plane,test-fixtures --locked local_admin` and the existing registry migration test selection discovered at execution (list tests before selecting; do not invent a nonexistent module filter). Review gate: approved schema, both result-adapter branches tested, no fake PHC accepted as KDF evidence, complete guard/audit transactions. Remote semantics remain a T10 release gate.

## Task 3 — KDF, bounded admission and redaction

Files: NEW password module; policy/contracts/exports; dependency/lockfile edits only after separate approval.

- [ ] Implement proposed Argon2id v19, m19456/t2/p1, 16-byte random independent salt, 32-byte output and PHC encoding using approved library APIs compatible with existing rand 0.10. Verify the approved version rather than copying an assumed RNG trait import.
- [ ] Parse stored PHC first and bound algorithm/version/memory/time/parallelism/output/salt; corrupt or hostile hashes fail closed without raw errors. Do not allocate according to arbitrary stored parameters.
- [ ] Exactly two running blocking jobs, at most eight queued admissions, two-second admission deadline. Semaphore alone does not bound waiter count: use a separate bounded queue/admission count. Move execution permit into `spawn_blocking` closure so cancellation does not free capacity until actual work stops. Saturation returns503.
- [ ] Dummy PHC generated once at startup, not each request; unknown/pending/recovery usernames perform one dummy verification. Wrong active password performs one real verification. Count work in tests; do not claim exact wall-clock equality.
- [ ] No secret Debug/Serialize/logs; instrumented tests cover cancellation, queue overflow, dummy path and parameter rejection. Document hardware measurements before accepting parameter cost.

Complete module for NEW `password.rs` (production PasswordHasher contract is §3.2):

```rust
#[cfg(test)]
mod tests {
    use super::PasswordHasher;

    #[tokio::test]
    async fn local_admin_password_hashes_are_salted() {
        let hasher = PasswordHasher::new().expect("supported KDF");
        let password = "correct horse battery staple".to_owned();
        let first = hasher.hash(password.clone()).await.expect("hash");
        let second = hasher.hash(password.clone()).await.expect("hash");
        assert_ne!(first, second);
        assert!(first.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
        assert!(hasher.verify(password.clone(), Some(first)).await.expect("verify"));
        assert!(!hasher.verify("a different password".into(), Some(second)).await.expect("verify"));
        assert!(!hasher.verify(password, None).await.expect("dummy"));
    }
}
```

RED/GREEN: `cargo test -p memory_mcp --lib --features control-plane --locked local_admin_password_hashes_are_salted`; run the entire password module afterward and default-feature check. No dependency approval means stop, not replace Argon2 with HMAC.

## Task 4 — Auth service, durable sessions and mandatory throttle

Files: NEW auth/tests/rate adapter/credential material; contracts/exports/registry adapter and real composition/bootstrap.

- [ ] Implement constructors and methods in §3.2. Move only reusable random/HMAC material below the control layer; avoid service→control dependency. Same concrete store Arc is exposed as RegistryStore and LocalAdminStore at composition. CLI builds only management service; HTTP Local builds authority, hasher and auth service.
- [ ] Login snapshots state/generation/PHC, performs KDF outside transaction, then compares those same credentials under policy/admin guard and inserts session/audit. Do **not** compare a version changed by unrelated session touches. No session cache.
- [ ] Resolve conditional transaction requires current policy/generation, active admin, both deadlines and unrevoked session; update idle=min(DB now+30min, absolute). Login absolute24h. Rotation preserves old absolute expiry, updates auth time, revokes old cookie and inserts new atomically. Logout/reauth and all client mutations conditionally write the **presented session** as well as admin/policy guards.
- [ ] Reauth requires a valid session/password, not already-recent auth; logout requires CSRF/Origin, not recent auth. Client mutators recheck auth_time within600s at commit even after waiting. A mutation before revocation may return later; it is not retroactively rolled back.
- [ ] Service reserves all applicable counters before lookup/KDF: Credentials=username5/15min+source30/5min; Challenge=source10/15min. Login/reauth share domain; inspect/activate/reset share domain. Fixed windows, not rolling-equivalent. Map keyed normalized input to4096 slots/dimension; invalid-format usernames use bounded raw-input keying and dummy verification. Store rejects omitted username for Credentials or out-of-range buckets.
- [ ] Direct socket peer only; ignore XFF/Forwarded/X-Real-IP, normalize mapped IPv6; missing peer fails closed. Behind one proxy users share its limit; this availability trade-off needs approval. No new forwarded chain parser.
- [ ] Throttled requests increment bounded saturating denied aggregates, not unlimited audit rows. Admitted failures record one deduplicated allowlisted event; storage failure503. An event in a rolled-back transaction is not durable failure audit: call record_failure after rollback. Do not retry secret-returning success on ambiguous commit. Storage outage never falls back to process memory.

Complete NEW `service/local_admin/tests.rs` starting fixture/test. Register with `#[cfg(test)] mod tests;` in mod.rs; re-export Authority, ManagementService, LocalAdminService, PasswordHasher and listed contracts crate-visibly. No external test visibility expansion is needed. The fixture migrates real Mem, joins policy with fixed test-only keys, initializes the real approved KDF and has no clients.

```rust
use super::{
    AdminManagementService, AuthAttemptContext, ChallengeKind,
    LocalAdminAuthority, LocalAdminService, LocalAdminStore, PasswordHasher,
    RequestContext,
};
use crate::http::registry::surreal_store::SurrealRegistryStore;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

async fn service() -> (AdminManagementService, LocalAdminService) {
    let namespace = format!("local_admin_test_{}", uuid::Uuid::new_v4().simple());
    let concrete = Arc::new(
        SurrealRegistryStore::connect_in_memory(&namespace, "registry")
            .await.expect("migrated Mem registry"),
    );
    let store: Arc<dyn LocalAdminStore> = concrete;
    let authority = LocalAdminAuthority::join(store, [1; 32], [2; 32])
        .await.expect("local policy");
    let management = AdminManagementService::new(authority.clone());
    let auth = LocalAdminService::new(
        authority,
        Arc::new(PasswordHasher::new().expect("supported KDF")),
    );
    (management, auth)
}

fn request() -> RequestContext {
    RequestContext { request_id: uuid::Uuid::new_v4() }
}

fn attempt() -> AuthAttemptContext {
    AuthAttemptContext {
        request: request(),
        source: IpAddr::V4(Ipv4Addr::LOCALHOST),
    }
}

#[tokio::test]
async fn local_admin_recovery_invalidates_old_credentials() {
    let (management, auth) = service().await;
    let invitation = management.create_admin("ops.one", &request()).await.expect("create");
    auth.finish_challenge(&attempt(), &invitation.code, ChallengeKind::Activate,
        "first correct password".into()).await.expect("activate");
    let login = auth.login(&attempt(), "ops.one", "first correct password".into())
        .await.expect("login");
    let reset = management.recover_admin("ops.one", &request()).await.expect("recover");
    assert!(auth.resolve(&request(), &login.cookie).await.is_err());
    assert!(auth.login(&attempt(), "ops.one", "first correct password".into()).await.is_err());
    auth.finish_challenge(&attempt(), &reset.code, ChallengeKind::Reset,
        "second correct password".into()).await.expect("reset");
    assert!(auth.login(&attempt(), "ops.one", "second correct password".into()).await.is_ok());
    assert!(auth.resolve(&request(), &login.cookie).await.is_err());
}
```

RED/GREEN: `cargo test -p memory_mcp --lib --features control-plane,test-fixtures --locked local_admin_recovery_invalidates_old_credentials`; then all `service::local_admin` tests. This test is a serial regression, **not race evidence**. Add forced-order tests from §5 in adapter unit modules using real handles; no random sleeps.

## Task 5 — Administrator-only CLI

Files: cli enum/args/commands/runner; NEW admin adapter and CLI integration target.

- [ ] Feature-gated `Command::Admin(AdminArgs)`; `AdminOperation::{Create { username: String }, Recover { username: String }}`. Update **both** `mode_label` and `into_one_shot`; Admin returns None for ordinary one-shot flow. Dispatch to admin adapter before MemoryService/NER construction. Keep main thin.
- [ ] NEW `AdminCliConfig::from_env() -> Result<Self, MemoryError>` in adapter: privileged control SurrealTargetConfig, explicit local mode, session/CSRF keys, HTTPS public URL. It does not require tenant credentials, pepper, plan or OIDC configuration. Do not reuse full HttpConfig parser. Compose management service without KDF.
- [ ] `run(args: AdminArgs) -> Result<(), MemoryError>` owns safe local-error conversion before existing report_cli_error. Output explicit JSON once with admin_id, username, code, expires_at and fixed activation/reset URL without code. stdout is intentionally secret; stderr/debug never is. No password/code argument or environment password support.
- [ ] Parser tests reject client management/password/code flags; default build does not expose admin. Process tests use isolated remote DB, omit tenant/model settings and verify no model initialization or OIDC request. Lost output repaired by recover, not secret retrieval.
- [ ] Embedded RocksDB CLI requires server stopped on that path; simultaneous processes use remote DB. No second embedded connection in HTTP composition.

After target exists: `cargo test -p memory_mcp --features control-plane,test-fixtures --locked --test local_admin_cli`. Help checks: `cargo run -p memory_mcp --features control-plane --locked --bin memory_mcp -- admin --help` and `cargo run -p memory_mcp --no-default-features --locked --bin memory_mcp -- --help`. Do not invoke live create/recover during documentation review.

## Task 6 — Local HTTP, errors and mode-bypass closure

Files: NEW local router/auth, existing control registration/router/middleware/session/OIDC/config call sites, local HTTP tests.

- [ ] Add optional auth service and browser policy to HttpState and real `assemble` wiring. NEW `control::local_admin::router(state: Arc<HttpState>) -> axum::Router` returns state-bound router. Local admin extractors produce AdminPrincipal only; bearer/OIDC principals cannot substitute.
- [ ] Explicitly wrap **all merged local routes**, not just original MCP router, with body/deadline/Host protections. Parse JSON manually with bounded16KiB body and deny_unknown_fields. Content-Type415, oversized413, deadline503, malformed400. Generate request UUID. Local mapper never prints underlying SQL/secret-bearing MemoryError.
- [ ] Exact allowed Origin on every local POST/DELETE, including login/activation/reset. Existing generic middleware permits absent Origin and is insufficient. Trusted forwarded Host only from trusted socket peers; reject ambiguous/duplicate/comma headers; require proxy overwrites and backend access restrictions. Throttle still uses peer IP only.
- [ ] Preauth cookie and token expire5min, bind current policy epoch and purpose. Stateless timestamp uses synchronized server UTC with a reviewed skew bound; durable authorization uses DB time. Reject future/out-of-range timestamps. Test against both replicas. Limit issuance through process admission; no unbounded preauth database rows.
- [ ] Admin cookies `__Host-memory_mcp_admin` and preauth `__Host-memory_mcp_admin_preauth`, Secure/HttpOnly/Strict/Path=/ and no Domain. Reject duplicate values. Clearing cookie has same scope. Reauth Set-Cookie Max-Age must reflect **remaining absolute lifetime**, not a fresh24h. No positive session cache.
- [ ] Serve spec §8 status/body shapes exactly. Auth/session/client/key responses no-store; auth pages no-referrer/no external assets. Challenge invalid/expired/used/wrong-kind uniform400. Credentials unknown/wrong/pending/recovery uniform401 after mandatory service admission.
- [ ] Valid-session logout requires CSRF but not recent auth; absent-session repeat may204 without any other session mutation. Rotation races can leave a revoked cookie after reordered responses; require fresh login, never resurrect it.
- [ ] Local mounts no `/auth/oidc/*`, `/api/v1/account/*`, `/api/v1/operator/*`; OIDC mounts no local/admin routes; off mounts neither. Explicit unmatched `/api/` and `/auth/`404 before SPA fallback. Fixture stub headers never enable production auth. Probe actual existing route literals from router, not stale handler comments.
- [ ] Extend test-state builder with an explicit durable local composition path; validate compatible policy/services/config together, no partially injected state. HTTP in-process fixtures must attach ConnectInfo and explicit cookie/CSRF/Origin headers. No hidden auto-CSRF helper for negative tests. Test subprocess fixtures need nonempty bootstrap value only in fixture builds; never enable it in production.

Write tests for every route/negative case before handlers. After target exists: `cargo test -p memory_mcp --features control-plane,test-fixtures --locked --test http_local_admin`; OIDC regression `cargo test -p memory_mcp --features control-plane,test-fixtures --locked --test http_control_plane`. HTTP fixtures do not prove Secure-cookie browser behavior; T10 TLS browser gate is separate.

## Task 7 — Atomic clients, key lifecycle, provisioning and cache

Files: NEW client service/adapter/router; contracts/composition; existing provisioning worker/registry adapter/principal auth and OIDC key-material caller. Any schema addition not covered in T2 reopens migration approval.

- [ ] Create transaction: guard policy/admin/session/recent auth; idempotency; server-generated Account/Tenant/namespace; local sidecar marker=false/version; durable provisioning event; audit; operation record. Account Active/Tenant Reserved, explicit persisted plan version. No staged partial writes visible on failure. Return202 ClientView with Location; duplicate same body returns same resource's **current** view; changed body409.
- [ ] Worker discovers Reserved rows independently of event; retain event as durable history. Test crash before worker, restart in NamespaceCreating/schema0 and mid-Migrating. Fix due query omission and stage handling, not just append another event. Exclude properly suspended tenants from provisioning work; preserve legitimate failed retry semantics. Recheck state/lease fences so a delayed worker cannot turn Suspended back into Ready.
- [ ] List/get only sidecar-backed local clients, no OIDC adoption by arbitrary Account id. Stable Account-id cursors,50/max100. Key history also paginated stable key-id cursor; no unbounded Vec hidden behind cap on active keys.
- [ ] Key issue transaction locks client guard before count, validates Active/Ready and plan cap with DB now, counts active **nonexpired** keys, derives checked expiry from explicit choice, inserts verifier/operation/audit and returns metadata. No audit-after-insert sequence. Two different admins at N−1 must produce one success, not two.
- [ ] Key secret retained only in process until first201 response. Same operation after commit returns409 secret_already_issued/key_id; conflicting body409 idempotency_conflict. Uncertain commit must not retry a secret response; list/revoke/reissue explicitly. Audit includes granting admin, Account/Tenant/key and grants_client_data_access=true.
- [ ] Revoke requires owned key, first transition audited; already-revoked owned key204; wrong owner404. Define lookup/authorization before idempotent response; do not leak foreign resource ids through duplicate operations.
- [ ] Suspend only Active/Ready/marker=false; resume only Suspended/Suspended/marker=true. After current authorization, coherent desired-state no-op204 takes precedence over stale expected_version; otherwise stale version409. Reject incoherent pairs and early/deleting/purged states. Changed state transaction updates Account, Tenant version, sidecar version/marker and audit atomically. Issue/revoke also advance sidecar guard on change.
- [ ] **Fix bearer cache-hit auth in all modes**: reread current key status/expiry/ownership as well as Account; reject equality at expiry, revoke and owner mismatch before accepting cached principal. Do not merely invalidate one process cache. Existing subscription `is_current` checks remain at configured interval(default30s/max60s). Already admitted work is not cancelled; synchronized replica UTC still governs data-plane expiry.
- [ ] Warm cache, suspend, revoke/expire, resume and assert denied on both replicas. Test direct revoke with no suspend too. No obsolete “60-second cache allowance” for new HTTP authorization after this fix; subscription bounds are distinct.
- [ ] Build test fixtures explicitly: real Mem store + T4 activated admin + explicit persisted test plan + create through service. For service-only key tests, a test-only transaction may set a **coherent** Ready/schema/lease state, clearly not provisioning evidence. Worker/restart tests must invoke the real worker with isolated tenant storage. No undefined `ready_client` helper is supplied as a runnable slice.

RED/GREEN: unit `cargo test -p memory_mcp --lib --features control-plane,test-fixtures --locked local_admin`; local HTTP target; existing principal/provisioning/OIDC-key unit suites after listing exact tests. Gate includes forced cap/rollback/cache/provisioning tests, not just happy-path APIs.

## Task 8 — Local browser auth UI

Files: NEW admin_api/admin_auth; existing UI main/router/pages/login. No backend crate dependency.

- [ ] Define UI-only DTOs matching spec JSON; timestamp strings. `AdminApiError { status: u16, code: String, message: String, key_id: Option<String> }`; preserve existing OIDC ApiError. Challenge response has username/expires_at only; session has admin_id/username/auth_time/absolute_expiry/csrf_token.
- [ ] NEW same-origin `AdminApi` async methods mode, inspect, finish, login, session, reauth, logout map one-to-one to spec routes. Preauth CSRF fetch for public POST; current-session token for mutations, both sent in `X-CSRF-Token`. Fetch cookies only same-origin; never automatic credential POST retry. Handle204 without JSON decoding.
- [ ] Login mode config determines pages; readonly username after valid code; finish posts code/password only. Components AdminLoginPage, AdminActivationPage, AdminResetPage, AdminReauthDialog. Password confirmation local, policy enforced server-side.
- [ ] Pending/error/live-region/focus/keyboard/password-manager support; generic failures; clear secret state on completion/unmount/logout. No localStorage/sessionStorage/IndexedDB, URLs, analytics, HAR or screenshots containing credentials. Reauth retries a mutation only after explicit confirmation.
- [ ] Unit wire/validation tests before components; browser acceptance cases in §5 after NEW T10 runner is implemented. No undefined browser bindings in this plan.

Commands after tests exist: `cargo test -p control-plane-ui --locked`; `cargo check -p control-plane-ui --locked`. Native checks do not prove wasm compilation or browser bundle boot. Dependency permission required for any missing browser crypto bindings/testing package.

## Task 9 — Client and one-time-key UI

Files: NEW admin_clients/admin_client and admin_api additions, router/pages. Existing OIDC account/delete/key pages remain separate.

- [ ] UI API operations: clients(after,limit)→Page<ClientView>; create_client(name,operation UUID)→ClientView; client(id)→ClientView; keys(id,after,limit)→Page<ApiKeyMeta>; issue_key(id,name,expiry,operation UUID)→existing compatible CreateApiKeyResponse; revoke_key(id,key)→unit; set_state(id,expected_version,action)→unit. Wire DTOs exclude backend namespace/verifier fields.
- [ ] Generate idempotency UUID once per deliberate action using verified browser crypto bindings, not per retry; gate new dependencies. Client create may manually retry same id/body after response loss. Key POST is never auto-retried;409 public id guides revoke/reissue.
- [ ] Paginate clients **and keys**. Poll visible nonterminal provisioning2s, failure backoff≤30s, cancel on unmount. Failed shows safe reason; no added retry/purge/quota UI. Ready enables key issue/suspend; coherent suspended state enables resume.
- [ ] Explicit expiry selection: blank invalid,1–3650days or Never, simultaneous values invalid. One-time secret only from201 response, explicit copy action, clear close/navigation/logout, warning before losing unsaved secret. State admin access honestly and external delivery requirement.
- [ ] Suspend/resume confirmation, expected version,409 refresh, reauth dialog with confirm-before-retry. No local navigation to account identity/deletion/quota screens; backend remains authoritative.
- [ ] Browser tests two equal admins/two clients, failure/readiness/pagination, expiry/never, no secret after refresh, wrong-owner revoke, stale CAS, cache-warmed revoke/suspend/resume and no OIDC direct-route bypass. Do not record secret reveal panel.

RED/GREEN: UI tests/check above; browser `clients` scenario only after T10 harness prerequisites. Accessible loading/empty/error/expired-session behavior is part of acceptance.

## Task 10 — Remote race evidence, packaging and handoff

Files: NEW acceptance scripts/integration targets/runbook, existing Docker/Compose/CI/build/static assets and fixture helpers. README changes are implementation-time only and must preserve user work.

### Remote harness contract (NEW)

- [ ] Implement `local_admin_remote_replica_races` as an inline adapter test, `#[ignore = "requires isolated remote SurrealDB 3.2.4"]`; private contracts remain accessible. Test reads NEW `LOCAL_ADMIN_TEST_CONTROL_URL`, `LOCAL_ADMIN_TEST_CONTROL_USERNAME`, `LOCAL_ADMIN_TEST_CONTROL_PASSWORD`; explicitly selected test fails if missing, never skips/succeeds. Random isolated namespace/database per run; no production target. Two independent connections plus CLI/HTTP subprocess tests, not shared Mem only.
- [ ] `tests/local_admin_durable.rs` is black-box reopen/process coverage, not a private-contract import. Harness creates disposable remote3.2.4, credentials and isolated namespace; barriers control stale snapshots. Fault hooks are test-only and never enabled by HTTP headers. Test assertions use exact typed outcomes/postconditions, not any is_err.
- [ ] Implement image harness `python3 scripts/ci/local_admin_image.py --image memory-mcp-local-admin:test --scenario all`. It validates Docker/server/browser/tool prerequisites, creates disposable network/DB/protected temp env files/trusted local TLS endpoint at https://localhost:8443, invokes both source-built binaries, runs browser scenarios, and cleans up in finally. Secret stdout captured in memory only; no credentials in console/artifacts. It must fail on missing prerequisites, not skip.
- [ ] Implement browser runner `node scripts/ci/local_admin_browser.mjs --base-url https://localhost:8443 --scenario auth` with scenarios auth/clients/regression. Before declaring it runnable, select/approve a specific browser package/version, add locked manifest/install command and imports, install the matching browser, and define fixtures. At review time no runner package manifest was found and dx is absent; **the exact package install/bundle command is a release blocker, not guessed here**. The image harness supplies a protected fixture config path via NEW `LOCAL_ADMIN_BROWSER_FIXTURE` env; config identifies disposable CLI/container and TLS trust, never embeds production passwords. Runner obtains fresh codes via CLI, redacts assertions and cleans up sensitive state. Direct invocation needs that same fixture contract.

### Packaging

- [ ] Verify installed `dx --version` and `dx bundle --help`, select/pin compatible0.7 CLI and wasm target, then record the exact tested bundle command/output layout in runbook/CI. No speculative `--out-dir` invocation is supplied as executable. Build must produce nonempty index+JS+WASM in literal absolute nonsymlink dist directory for `MEMORY_MCP_CONTROL_PLANE_UI_DIST`.
- [ ] Extend Docker build to build actual UI and both memory_mcp and memory_mcp_http with streamable-http,control-plane,control-plane-ui. Preserve HTTP entrypoint, nonroot/shell-free runtime, native libs and current compiled migration-assets path `/src/crates/memory-mcp/migrations/`. CLI runs with entrypoint override, not a shell. Do not modify generated asset catalog by hand.
- [ ] Test real browser JS/WASM boot under shipped CSP; current `script-src 'self'` is not evidence WebAssembly executes. If required, review minimal `wasm-unsafe-eval` rather than blanket unsafe-eval. Validate MIME types, auth routes, API404 and disabled UI404, not just HTML index presence.
- [ ] Split/condition Compose local/OIDC/off configuration: omit OIDC-only keys in local/off; require operator-generated secrets, HTTPS and explicit local plan. Merely setting AUTH_MODE against today's unconditional OIDC config fails. No production DB root/root or example browser-key defaults. Validate resolved configuration without printing secrets.
- [ ] Test source-built image digest/binaries, CLI create/recover, TLS activation/login/client/key/revoke, restart persistence, plan mismatch, no OIDC discovery, mode exclusivity and data-plane-only existing-key operation. No published app image is evidence.
- [ ] Write runbook only after verification: exact commands/tool versions, secrets/rotation/15-minute admission hold, backups/restore, offline embedded CLI, legacy OIDC relogin, old replica shutdown, default plan, response loss, direct-peer limits, fresh key revalidation vs subscription recheck bounds. Rollback disables local browser control before old binary; retain additive records/data-plane keys. No client adoption/admin removal/MFA/email/purge added.

## 5. Required security experiments (not executable snippets)

Implement each test fixture and assertion fully before claiming RED/GREEN. Force ordering with barriers/channels at named service/store seams; SQL fault hooks remain test-only. No sleep-based claim of linearization.

| Experiment / forced ordering | Required result |
|---|---|
| Same activation/reset verifier submitted through independent handles | Exactly one consumption; no double generation/session; wrong kind/used/expired identical public response. |
| KDF completes, pause before session insert; recover on second handle; release | No old-generation session. Ordinary unrelated session touch during KDF must not reject valid credentials. |
| Reset hash prepared, newer recovery commits, release finish | Old code rejected; only final recovery generation usable. Earlier successful recovery response may already be stale. |
| Resolve/touch and recovery/logout, both commit orders | No resurrection or deadline extension after revocation/expiry; reads already admitted may finish. |
| Client mutation paused after middleware, logout commits; release | Transaction rejects old session. Repeat recovery/reauth/policy epoch and recent-auth expiry while waiting. |
| Two rotations; logout-before-rotation and rotation-before-old-logout | One rotation winner; logout-first blocks rotation; old-cookie logout cannot revoke independent new cookie. Reordered response cookie is allowed to fail401, never resurrect. |
| Replica joins wrong mode or wrong local key fingerprints | Startup fails; callbacks/session writes with stale fence also fail even without router gate. |
| OIDC local→OIDC transition and away→back; legacy session lacks epoch | Cleared pending request/rotated keys reject old callback; legacy/old epoch cookies rejected; data keys unaffected. |
| Rate two handles/restart/random names/invalid names/collision/window boundary | Shared caps, bounded row count, common challenge domain, source limit cannot be bypassed; collision only denies extra; no account existence disclosure. |
| Spoof XFF/Forwarded/X-Real-IP, missing peer, mapped IPv6, proxy shared source | Headers never select rate source; normalization stable; absent peer denied; proxy aggregation documented. |
| KDF timeout/queue/cancellation/corrupt PHC | Bounded running+waiting counts; cancelled blocking job still holds permit; hostile parameters not executed; unavailable503. |
| Inject nonselected SQL statement error / audit insert error | Adapter propagates error; no partial credential/client/key/state success. Rejected-operation audit written after rollback or committed typed rejection, not discarded by THROW. |
| DB unavailable during reserve, auth, success audit, failure audit | Sanitized503; no session/key accepted; no local fallback; no secret retry on uncertain commit. |
| Two admins issue at cap−1; issue vs suspend/recovery | At most cap; authorized serialization, atomic key/audit/idempotency; no stale-session insert. |
| Create same operation/body concurrently; different body; lost response | One Account/Tenant/sidecar/event/operation; same current view on safe replay,409 changed body. |
| Key issue response lost and repeated | One stored verifier, never second secret;409 public key id; explicit revoke/reissue. |
| Provisioner restart Reserved/NamespaceCreating/schema0/Migrating | Eventually Ready or safe actionable Failed; no stuck omitted stage; suspended tenant not reprovisioned. |
| Coherent suspend/resume, no-op with old CAS, stale change, early/incoherent pair | Atomic pair/version/marker/audit, specified no-op precedence, no false Ready. |
| Warm key cache on two replicas; revoke or expire; suspend→resume | New auth rereads key and denies revoked/expired; valid keys preserved; exact expiry equality denied; subscription recheck bounded separately. |
| Public auth and every admin route: Origin/CSRF/content type/body/duplicate cookie/unknown fields | Exact spec status and no side effects, even through merged routers. |
| Local admin cookie→MCP; bearer/OIDC cookie→admin; old account/operator routes | No privilege substitution,404 for unmounted APIs not SPA200. Two client keys remain isolated. |
| Generated sentinel secrets through errors/Debug/logs/audit/metadata/UI storage | Absent except authorized one-time CLI stdout/HTTP201 and ephemeral UI state. No secret assertion dumps, HAR or screenshots. |
| Actual packaged UI/CLI over trusted TLS | Secure cookies really work; JS/WASM boot under CSP; no placeholder index or unpublished assumption. |

## 6. Execution validation commands and prerequisites

Run from repository root. These commands are **future implementation validation**, not claimed executed. Rust/native ONNX/RocksDB dependencies and locked packages must be available. New targets must first exist. For HTTP subprocess fixtures compiled with test-fixtures outside cfg(test), use explicit nonempty test bootstrap only inside disposable fixture environment; production constructors never select fixture storage.

```bash
cargo fmt --all --check
cargo check -p memory_mcp --no-default-features --locked
cargo check -p memory_mcp --no-default-features --features streamable-http --locked
cargo test -p memory_mcp --locked
cargo test -p memory_mcp --lib --features control-plane,test-fixtures --locked service::local_admin
cargo test -p memory_mcp --features control-plane,test-fixtures --locked --test http_control_plane
cargo test -p memory_mcp --features control-plane,test-fixtures --locked --test http_local_admin
cargo test -p memory_mcp --features control-plane,test-fixtures --locked --test local_admin_cli
cargo test -p memory_mcp --features control-plane,test-fixtures --locked --test local_admin_durable
cargo test -p memory_mcp --features control-plane,test-fixtures --locked
cargo test -p control-plane-ui --locked
cargo check -p control-plane-ui --locked
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http,control-plane --locked -- -D warnings
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http,control-plane,test-fixtures --locked -- -D warnings
```

With explicit disposable remote DB credentials in the three LOCAL_ADMIN_TEST_CONTROL_* variables and the NEW inline test installed:

```bash
cargo test -p memory_mcp --lib --features control-plane,test-fixtures --locked local_admin_remote_replica_races -- --ignored
```

Require one selected named test; an ignored test in ordinary runs is not remote evidence. Record server3.2.4, actual connection independence, test postconditions and clean teardown.

With a verified **literal absolute** MEMORY_MCP_CONTROL_PLANE_UI_DIST already exported by the approved build pipeline:

```bash
cargo test -p memory_mcp --lib --features control-plane-ui,test-fixtures --locked control::static_assets
cargo test -p memory_mcp --features control-plane-ui,test-fixtures --locked --test http_local_admin
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http,control-plane,control-plane-ui,test-fixtures --locked -- -D warnings
```

After the approved browser runner/tool versions and NEW harnesses exist, Docker daemon/TLS trust/ports are available, and Compose mode-specific required environment is supplied:

```bash
docker compose -f docker-compose.yml config --quiet
docker build --tag memory-mcp-local-admin:test .
python3 scripts/ci/local_admin_image.py --image memory-mcp-local-admin:test --scenario all
```

The harness owns its server lifecycle and browser fixture configuration; it runs auth/clients/regression scenarios. A manually provisioned equivalent can run the documented node entrypoint, but a bare URL without CLI/TLS fixture config is not sufficient. Exact dx/browser installation commands remain blocked on tool selection/verification; do not substitute guessed commands or mark these gates passed.

### Feature/runtime matrix

| Configuration | Must prove |
|---|---|
| Default stdio | No admin command/default dependency expansion, unchanged eight tools |
| streamable-http, control off | Existing keys/tenant isolation, no browser keys/plan/discovery/routes |
| control-plane, Local, UI off | Local APIs/CLI without OIDC; root absent; no account/operator/signup bypass |
| control-plane, OIDC | OIDC flows preserved except documented legacy-session relogin; local/admin404 |
| control-plane-ui, each enabled mode | Correct real bundle/mode pages, secure TLS workflow; opposite-mode API404 |
| UI enabled/control off or feature missing | Startup configuration error |
| test-fixtures | Explicit isolated fixtures only, no production header/storage escape |
| Two Local replicas | Shared durable auth/throttle/recovery/cap/audit/idempotency |
| Mixed Local/OIDC or drifted Local keys | Startup/transaction refusal, not silent first-writer acceptance |

## 7. Traceability and final review gate

| Requirement | Tasks / acceptance |
|---|---|
| R1 | T1/T2/T6/T10: config, durable policy, old-replica shutdown, exclusive routes |
| R2 | T2/T4/T7: equal admins, no admin Account/Tenant, all local clients accessible to either |
| R3 | T2–T6/T8: CLI expiring code, browser password setup/login; CLI username allocation remains proposal |
| R4 | T2/T4/T5/T6/T10: recovery generations, code/session revocation and forced races |
| R5 | T7/T9/T10: atomic client bundle, real worker restart, paginated readiness |
| R6 | T7/T9/T10: explicit expiry/never, capped issuance, paginated metadata, scoped revoke |
| R7 | T7/T9/T10: coherent CAS suspend/resume, no reprovision, valid-key preservation |
| R8 | T7/T9/T10: no secret replay, external delivery, atomic grant audit |
| R9 | T1/T2/T6/T7/T9: explicit plan, no self-registration/email/delete/quota UI |
| R10 | T1/T5/T6/T10: off-mode existing keys and admin-only registry CLI |
| R11 | T2–T10: KDF, mandatory shared admission, CSRF/cookies, audit/error redaction and races |
| R12 | Current documentation-only delivery plus separately requested report; no implementation/commit |

Reviewers must inspect each requirement and §5 case against **executed** evidence after implementation. Do not release with untested Surreal conflict semantics, unverified migration/PHC cost, placeholder fixtures, missing real TLS browser boot or unbuilt source image. Runtime/tooling uncertainty is an explicit blocker, not a reason to weaken security or an assertion that the current repository already supports this design.
