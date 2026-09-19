# Local administrator authentication and client provisioning

**Date:** 2026-09-18  
**Status:** Approved product requirements; proposed technical design awaiting review. This document does not approve dependency changes, migrations, implementation, or commits.  
**Companion:** [Implementation plan](../plans/2026-09-18-local-admin-auth.md)
**Architecture decision:** [ADR-0055](../../adr/0055-local-admin-authentication-for-remote-deployment.md) records the approved product direction; technical proposals below remain subject to review.

## 1. Approval boundary

The user approved the requirements in §2. Everything marked **Proposal**, including API names, timing limits, schema, password algorithm, configuration defaults, and rollout policy, is a recommendation rather than an additional user decision. The implementation plan can be reviewed now but must not cross the dependency/migration gates without explicit permission. R12 records the original two-document scope. Subsequent requests add the review report and ADR-0055 with related cross-references. These documentation changes do not authorize application changes.

### Global constraints

- Workspace Rust floor: `1.97.1`; edition `2024`.
- Keep `main.rs` thin: CLI parsing and dispatch only; new business logic belongs in `src/service/`.
- Retain the existing thiserror-based `MemoryError` for infrastructure; use the proposed typed `LocalAdminError` wrapper for local domain outcomes. Adapters map allowlisted errors without exposing secrets.
- Features remain additive; package `default = []`; preserve existing stdio behavior.
- No new MCP tools; preserve the eight-tool surface.
- Do not change the tenant boundary or add request-selected namespaces. A client owns an Account/Tenant; the server generates its immutable namespace binding.
- Ask before changing dependencies in `Cargo.toml`, generated code, or migration files. Approval of requirements is not approval of these changes.
- Do not commit unless separately requested.
- Never delete facts; this feature adds no client deletion or purge UI.

## 2. User-approved requirements

| ID | Requirement |
|---|---|
| R1 | Deployment authentication mode is exclusive: local or OIDC, never both concurrently. |
| R2 | Local administrator identities are separate from client Accounts/Tenants. Multiple administrators have equal privileges. |
| R3 | A local CLI command creates an administrator and emits a one-time expiring activation code. Activation establishes the password; later login uses username/password. |
| R4 | CLI recovery invalidates that administrator's sessions and issues a one-use expiring reset code. |
| R5 | An admin-only UI creates and lists clients, each with its own Account/Tenant, and displays provisioning readiness. |
| R6 | Admins issue named API keys with optional expiry or explicit “Never expire”; list non-secret metadata and revoke keys. |
| R7 | Admins suspend and resume clients. |
| R8 | Secrets are shown once. Delivery to clients is external to this product. An admin can access client data by issuing a client key; that issuance must be audited. |
| R9 | No email dependency, no client self-registration in local mode, no delete/quota-editing UI, and an explicitly configured default plan. |
| R10 | Data-plane-only deployment continues to work with existing keys. CLI scope is administrator management, not client provisioning. |
| R11 | Password hashing, login rate limiting, CSRF protection, secure sessions, and redacted audit/log output are required. |
| R12 | Deliver only this specification and its implementation plan; no implementation or commits in this task. |

Proposal: the operator supplies the username to the CLI; activation displays that username and lets the browser set only the password. Username allocation was not separately decided during requirements approval. Administrators are separate from client Accounts/Tenants.

## 3. Verified repository baseline

Paths below are repository-relative. Navigation used the live graph, semantic search, structural search, signature views, then targeted reads. Search summaries can lag live files; source bodies take precedence. No claim here means that the current test suite was executed.

| Existing source | Verified behavior and consequence |
|---|---|
| `Cargo.toml`; `crates/memory-mcp/Cargo.toml` | Rust 1.97.1, Axum 0.8.9, Tokio, SurrealDB 3.2.4, HMAC/SHA-256, rand, subtle, thiserror. No direct password-KDF dependency. `control-plane` includes `streamable-http`; `control-plane-ui` includes `control-plane`. |
| `crates/memory-mcp/src/http/config/{types,parse,validate}.rs` | `HttpConfig` currently loads all five browser/OIDC key values and signup mode even for data-plane-only operation. Enabling control plane requires OIDC issuer/client/audience/redirect. Local mode needs real config branching, not relaxed OIDC validation alone. |
| `crates/memory-mcp/src/http.rs` | `HttpState::assemble` ensures a version-1 `free` plan, with defaults when explicit limits are absent; creates an OIDC client whenever control plane is enabled. Neither behavior is the proposed local contract. |
| `crates/memory-mcp/src/http/router.rs` | Actual OIDC routes are `/auth/oidc/authorize`, `/auth/oidc/callback`, `/auth/oidc/logout`. Account/operator routes use cookie middleware. `create_account` is not mounted in production. Static fallback is added separately. |
| `crates/memory-mcp/src/http/middleware/auth.rs` | Operator authorization resolves account identities against configured issuer/verifier allowlist. Local principals cannot be injected into this account-based path. |
| `crates/memory-mcp/src/control/{session,csrf,recent_auth}.rs` | Existing sessions reference `account_id`, hash the cookie, use 30-minute idle/24-hour absolute expiry, and bind CSRF to account/session. Recent auth is 600 seconds. Reuse primitives, not the account identity assumption. |
| `crates/memory-mcp/src/control/account_api.rs` | `create_account` ignores `display_name`, constructs an Active Account/Reserved Tenant, hardcodes plan 1, then separately enqueues provisioning. Existing key HTTP adapter returns a full formatted credential. |
| `crates/memory-mcp/src/control/application/{api_keys,oidc_signup}.rs` | Existing application workflows use `Arc<dyn RegistryStore>`, not a shipped capability-store aggregator. `ApiKeyCreation::execute(CreateApiKeyCommand, DateTime<Utc>)` loads account/tenant/plan, generates a secret, and calls `create_api_key_if_below_limit`. Reuse this logic without claiming it already records an admin audit atomically. |
| `crates/memory-mcp/src/http/registry/{models,storage,surreal_store}.rs` | Existing Account, Tenant, Plan, ApiKey and metadata types; atomic `create_account_bundle`; durable store and fixture-only `InMemoryStore`. Plan limit, not README prose, is authoritative for active-key cap. `create_api_key_if_below_limit` counts then inserts inside a transaction; cross-replica phantom/write-skew resistance needs an explicit contention test. |
| `crates/memory-mcp/src/http/registry/{provisioning,migrations}.rs` | Fenced provisioning transitions already exist. Registry catalog is `001_registry`, `045_deletion_and_usage_hardening`, `046_registry_correctness`. Do not put admin tables into tenant migrations. |
| `crates/memory-mcp/migrations/{001_registry,045_deletion_and_usage_hardening,046_registry_correctness}.surql` | Schemafull control records, unique identity/cookie/plan constraints, durable sessions and audit-related schema. Migration runner has checksum/lease/postcondition checks. Append a migration only after permission; never edit history. |
| `crates/memory-mcp/src/control/operator.rs` | Suspend currently changes Tenant state; resume goes Suspended → Ready. Do not reuse this blindly for an incompletely provisioned client or for atomic Account/Tenant/audit changes. |
| `crates/memory-mcp/src/http/principal/{auth,cache,api_keys}.rs` | Structured `mem_sk_<key_id>_<secret>` credentials; HMAC verifiers; positive cache TTL 60 seconds. Cache-hit authentication rereads Account state but not key expiry/revocation; `is_current` rereads both. This is an existing correctness gap for R6/R7, not the target contract: T7 must revalidate key status/expiry on cache hits, including suspend→revoke/expire→resume. Local admin sessions never use this cache. |
| `crates/memory-mcp/src/{cli,runner}.rs`; `src/cli/{args,runtime,commands}.rs` | CLI dispatch and shared error formatting are established. An admin arm must run before MemoryService/NER construction and connect only to the control registry. |
| `crates/control-plane-ui/src/{api,router,pages,main}.rs`; `src/pages/{login,keys,status,delete}.rs` | Dioxus 0.7 account-oriented SPA. Local routing needs separate admin pages; hiding old links alone is not authorization. Existing key client accepts a name, not an expiry selection. |
| `crates/memory-mcp/build.rs`; `src/control/static_assets.rs` | UI embeds real prebuilt assets. `MEMORY_MCP_CONTROL_PLANE_UI_DIST` must be an absolute, real directory containing a nonempty `index.html`. Missing bundle fails the feature build. |
| `Dockerfile`; `docker-compose.yml`; `.github/workflows/ci.yml` | Current Dockerfile builds HTTP with `streamable-http,control-plane`, without UI, and copies only HTTP binary plus runtime libraries and migration assets into shell-free nonroot image. CI checks the actual image. No assumption that any published tag contains this feature, UI, or CLI is justified. |
| `crates/memory-mcp/tests/http_control_plane.rs`; `tests/common/http_server.rs` | Real HTTP subprocess fixtures, OIDC discovery mock/session seeding, CSRF/key tests and restart support exist. The mock does not exercise a real callback. The shared fixture injects OIDC keys and HTTP origins; it cannot be used unchanged for strict local mode or real Secure-cookie browser tests. Use shared remote DB for simultaneous processes; embedded RocksDB is single-process per path. |
| `crates/memory-mcp/src/http/registry/surreal_store.rs:65–95,113–144` | `query_json` checks all statement errors but decodes only result index 0. New multi-statement transactions returning records need an explicit result-index adapter; do not assume a final RETURN is returned by this helper. |
| `crates/memory-mcp/src/http/{config,oauth}.rs`; `http/runtime/bootstrap.rs` | Config types are re-exported through a private-module facade; OAuth helpers access OIDC fields; the real HTTP composition root directly invokes `HttpState::assemble`. All require T1/T4 wiring, beyond `HttpState::new`. |
| `crates/memory-mcp/src/http/{middleware/host_origin,server}.rs` | Real server supplies `ConnectInfo<SocketAddr>`. Existing middleware trusts forwarded Host only for trusted peers and allows absent Origin. It does not implement trusted client-IP extraction or strict local POST Origin checks. |
| `crates/memory-mcp/src/http/leases/migration.rs`; `http/registry/surreal_store.rs:1835–1851` | Worker discovers Tenant rows, not an event-consumption queue. Due query omits `namespace_creating` and includes `suspended`; restart at the omitted stage and suspended-client scheduler interaction require regression coverage and a focused fix in T7. |
| `crates/memory-mcp/src/control/error.rs:24–71` | Existing adapter maps Validation to 500, recent auth to 401, and prints Internal errors verbatim. Local routes need their own allowlisted mapper; reuse would violate the proposed local error/redaction contract. |

## 4. Architecture — Proposal

### Alternatives

1. **Separate local-admin identity/session tables and a mode-selected router (recommended).** Keeps client ownership unchanged and leaves OIDC records readable. Some duplicate session shape is intentional: it prevents an admin ID from being interpreted as an Account ID.
2. Generalize all OIDC/local sessions into a polymorphic table. Smaller apparent schema count, but it requires migrating OIDC sessions and every account extractor; unnecessary risk for this scope.
3. Model an admin as a privileged client Account. Rejected: violates R2, creates a tenant for each admin, and risks confused-deputy access through account endpoints.

New business workflows live under **NEW** `crates/memory-mcp/src/service/local_admin/`. Thin HTTP adapters live under **NEW** `crates/memory-mcp/src/control/local_admin/`. A narrow **NEW** `LocalAdminStore` trait is implemented by the existing durable store in a new child module; do not complete the unrelated deferred capability-registry refactor. Client transactions reuse Account/Tenant/ApiKey models and transaction idioms, not sequential calls to existing mutators.

Local administrator authentication yields `AdminPrincipal`, never `AuthenticatedPrincipal::ApiKey`, an OIDC operator, or `ControlPlaneSession`. An administrator cookie alone cannot call `/mcp`. To access client memory, the admin must issue a client key and use the existing data-plane authorization path. Tenant resolution still derives solely from the authenticated key's Account.

## 5. Deployment configuration — Proposal

| Setting | Contract |
|---|---|
| **NEW** `MEMORY_MCP_HTTP_AUTH_MODE` | `local` or `oidc`. Default `oidc` for backward compatibility; invalid values fail startup. Explicitly set it in new deployments. |
| Existing `MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE` | `false`: no browser auth or provisioning routes, no OIDC discovery, no admin KDF initialization, no browser key/default-plan requirement. Existing data-plane DB/pepper/host/origin requirements remain. |
| Existing `MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI` | Requires control plane and compiled `control-plane-ui`; otherwise startup fails. |
| Existing `MEMORY_MCP_HTTP_SESSION_KEY`, `MEMORY_MCP_HTTP_CSRF_KEY` | Required 32-byte hex keys in local mode. Domain-separated HMAC purposes prevent collision between admin sessions, challenges, CSRF, and throttle fingerprints. Rotation is offline across all replicas and the CLI: advance policy epoch, invalidate sessions/challenges/pre-auth state, and replace both keys together. Because keyed bucket placement changes on rotation, preserve existing rows and delay public authentication admission until the longest old throttle window (15 minutes) has elapsed; merely retaining rows under new keys would silently reset effective budgets. Reject replicas with differing domain-separated key-configuration fingerprints stored in the policy; otherwise different throttle fingerprints would multiply the budgets. Do not log the fingerprints or reset active throttle rows during rotation. No zero-key fallback. |
| Existing OIDC issuer/client/audience/redirect/algorithm and identity/state/nonce keys | Required only for enabled OIDC mode. Nonempty OIDC-only configuration in enabled local mode fails startup rather than being silently ignored. No OIDC network request in local mode. |
| Existing `MEMORY_MCP_HTTP_SIGNUP_MODE` | Required as today for enabled OIDC mode. In enabled local mode unset or `invite_only` is accepted; `open` is rejected even if quotas are present. |
| **NEW** `MEMORY_MCP_HTTP_LOCAL_DEFAULT_PLAN_VERSION` | Required positive `u32` in enabled local mode; never taken from browser input. |
| Existing seven explicit plan-limit environment variables | Require all seven in local mode, using `load_signup_plan_limits`: max ingested bytes, episode count, ingest/minute, open app sessions, active keys, request concurrency, extraction concurrency. Do not fall back to `PlanLimits::default()`. |
| Existing `MEMORY_MCP_HTTP_PUBLIC_BASE_URL`, allowlists, trusted proxy CIDRs | Require HTTPS public base URL for local browser auth. TLS may terminate at a trusted proxy. Validate exact Origin on browser mutations; do not trust arbitrary forwarded IP headers. |

The seven existing limit names are `MEMORY_MCP_HTTP_MAX_INGESTED_BYTES`, `MEMORY_MCP_HTTP_MAX_EPISODE_COUNT`, `MEMORY_MCP_HTTP_INGEST_PER_MINUTE`, `MEMORY_MCP_HTTP_MAX_OPEN_APP_SESSIONS`, `MEMORY_MCP_HTTP_MAX_ACTIVE_API_KEYS`, `MEMORY_MCP_HTTP_PER_TENANT_REQUEST_CONCURRENCY`, and `MEMORY_MCP_HTTP_EXTRACTION_CONCURRENCY`.

Local startup ensures a plan at the configured version, then loads it and compares every limit. Missing row is created; matching row is reused; different limits at the same version fail startup. Two replicas with different plans must not silently converge on the first writer. A changed plan requires a new version and distinct deterministic record id `local_plan_v{version}` (never reuse `free` for different versions); existing clients keep their assigned version. Lookup is by globally unique version, so an existing OIDC plan at that version is reusable only if every limit matches; never overwrite it. All limits obey the existing signed-integer/concurrency validation. This is not a quota-edit API.

Represent mode-specific browser configuration as a tagged enum plus `None` when control plane is off; do not fill unused HMAC keys with zeros. Preserve OIDC semantics through accessors while moving callers off unconditional `config.keys` assumptions.

**Replica mode fence:** store a singleton control-registry browser-auth policy containing selected mode and a monotonically increasing epoch. Initial creation is atomic. Startup may join only a matching mode. Switching modes is an offline operator operation: stop all control-plane replicas, update policy through a reviewed maintenance procedure, increment epoch, invalidate browser sessions/challenges, then restart one mode. No automatic switching or identity conversion; no new mode-switch CLI in this scope. Every local auth transaction and admin mutation checks this policy. Data-plane keys do not depend on it. This rejects mixed modes among upgraded replicas. An older binary does not know this fence and cannot be made safe by a new table alone: stopping all older control-plane replicas is a mandatory rollout precondition. OIDC callbacks in the upgraded binary must check the durable policy before creating accounts or sessions too. The plan adds an optional policy-epoch field to the existing OIDC session schema through the approved new migration, not a polymorphic session table. Upgraded OIDC session persistence and resolve/touch check the epoch; legacy sessions without it require login again (an explicit compatibility impact). Pending OIDC requests are cleared during the offline transition, and OIDC state/session keys are rotated; no old callback is accepted after switching away and back. OIDC-only fenced account creation is a separate adapter method; do not make the general Account/Tenant bundle operation OIDC-only. No online mode-switch endpoint is proposed.

## 6. Administrator lifecycle and password contract — Proposal

### CLI

- `memory_mcp admin create --username <name>` reserves the canonical username and prints `{admin_id, username, code, expires_at, activation_url}` once. The URL is a fixed `/admin/activate` URL **without** the code. The operator passes the code separately or types it locally.
- `memory_mcp admin recover --username <name>` atomically increments credential generation, sets `recovery_required`, invalidates all sessions and older challenges, and emits a reset code plus fixed `/admin/reset` URL once.
- Duplicate usernames return conflict, not another activation code. Recovery also works for pending admins to replace lost/expired activation material. Code kind remains `reset`; it may establish the first password while retaining the allocated username.
- Require local mode and privileged control-registry configuration. HTTP server may be stopped; remote registry is preferred. With embedded RocksDB, stop the HTTP process before CLI opens that path. The command does not start tenant provisioning, a runtime pool, embeddings, or OIDC discovery.
- No password/code arguments or environment-variable passwords. CLI never accepts a browser's new password. Secret stdout is an explicit one-time delivery channel; stderr/logs redact it. Do not capture terminal output in CI artifacts. If output is lost, recover again.

Username proposal: ASCII lowercase `[a-z0-9][a-z0-9._-]{2,63}`. CLI and login trim outer ASCII whitespace and lowercase ASCII before validating; no Unicode lookalikes or account aliases. Browser activation displays the reserved username after valid-code validation and never posts a replacement username. No rename/admin deletion/role hierarchy UI is added.

Passwords: 15–128 Unicode scalar values, maximum 1024 UTF-8 bytes; reject NUL; do not trim, normalize, silently truncate, or impose character-class rules. Allow paste/password managers. Hash with **proposed Argon2id** PHC encoding, independent random 16-byte salt, version 19, `m=19456 KiB,t=2,p=1`, output 32 bytes. Benchmark on deployment hardware; strengthening parameters requires explicit review, not weakening under load. Use `spawn_blocking` and a per-process semaphore of two KDF jobs with a bounded queue of eight and a two-second admission timeout. Validate stored PHC parameters against supported bounds before computing; corrupt hashes fail closed. No SHA/HMAC-only password storage.

**Dependency gate:** choosing a vetted Argon2 crate, its exact version/features, compatible RNG interfaces, and lockfile changes requires approval. Do not implement a custom KDF or treat a transitive crate as an approved direct dependency.

## 7. Durable authentication and concurrency — Proposal

### Records (all NEW, control registry only)

| Record | Durable fields / uniqueness |
|---|---|
| `local_admin` | UUID-based id; unique canonical username; state `pending_activation|active|recovery_required`; optional PHC password hash; `credential_generation`; `version` (contention guard only); created/updated timestamps. Every password change increments generation; guard writes do not. No Account/Tenant foreign key. |
| `local_admin_challenge` | id, admin id, kind `activate|reset`, HMAC verifier (unique), credential generation, mode epoch, DB-issued expiry, consumed/revoked timestamps. Raw code never stored. |
| `local_admin_session` | id, unique cookie verifier, admin id, credential generation, mode epoch, authenticated time, idle/absolute expiry, revoked timestamp. No raw cookie. |
| `local_admin_rate_bucket` | keyed fixed-size bucket id, window start, count, expiry, saturating denied-attempt count; separate fixed action/reason slots for denials without an admitted auth attempt. Shared across replicas. No raw username/IP/code. |
| `local_admin_client` | Account id (unique), display name, creating admin id, created time, monotonic mutation guard version, `suspended_from_ready` boolean (initial false). Client metadata sidecar; does not replace Account ownership. |
| `local_admin_operation` | unique admin id + operation kind + idempotency id, request fingerprint, resulting public resource id, committed time. No secrets. |
| `local_admin_audit` | event id/time, actor kind/id, action, target admin/account/tenant/key ids as applicable, outcome, safe reason enum, request correlation id; optional bounded bucket identifiers for admitted anonymous failures; `grants_client_data_access` for key issuance. Append-only. |
| `browser_auth_policy` | singleton selected mode + epoch + mutation guard version + local key-configuration fingerprints, used by upgraded replicas in both modes to reject stale/mixed configuration. Local fingerprints are required for Local and absent for OIDC; joining never overwrites them. |

Use schemafull definitions, deny unprivileged direct table operations, unique indices, strict decoders, and registry schema postcondition checks. **NEW proposed migration** the number `047` for `047_local_admin_auth.surql` is unused at review time (the file does not exist), but its number must be rechecked and its contents approved before creation. Never modify 001/045/046. Test fresh and upgraded registries, checksum mismatch, interrupted migration and simultaneous startup.

### Atomicity rules

- Use database time for challenge expiry, throttle windows, session expiry, recent-auth checks and transactional authorization. Inject time only into pure tests; do not trust browser timestamps or independent replica clocks.
- Activation/reset: hash outside the transaction; then atomically require unconsumed/unrevoked/unexpired challenge, matching kind and credential generation, eligible admin state and current mode epoch. Update password/state and generation, consume this challenge, revoke sibling challenges and old sessions, append audit in one transaction. Return success with **no automatic login**; browser then logs in normally. Two concurrent submissions yield exactly one success.
- Recovery: atomic generation increment and state change, revoke sessions/challenges, insert one new reset verifier, append audit. Two concurrent recoveries serialize; only the challenge matching the final generation remains usable. A response from the earlier winner can become stale immediately and must not be described as guaranteed live.
- Login: lookup credential snapshot, verify password outside transaction, then atomically compare generation/state/PHC, insert session and success audit. Do not compare the general contention `version` captured before KDF: ordinary session touches/mutations change it and must not make an otherwise valid password fail. `credential_generation` is the credential revision. A password verified before recovery cannot insert a usable session afterward. Unknown/pending/recovery-required user gets one dummy KDF at the same parameters, then the same `401 invalid_credentials` as a wrong password. Do not disclose activation state publicly.
- Session resolve/touch: one conditional transaction checks policy epoch, active admin, matching credential generation, unrevoked session and both deadlines; sets idle deadline to `min(DB now + 30 minutes, absolute expiry)`. Never recreate or extend a revoked/expired session. Absolute expiry is 24 hours. No positive session cache.
- Mutation authorization: middleware is insufficient. Each client/key/suspension transaction rereads session, admin generation and policy epoch at commit. Touch the existing admin row's `version` in that transaction so it conflicts with recovery; also conditionally write the presented session row so it conflicts with logout/reauth, and the policy guard so it conflicts with epoch changes. Logout/rotation/recovery must use those same guards in the same order (policy → admin → session → client → resource). A snapshot read alone does not prevent write skew. Recheck recent auth against DB time inside the mutation, not just at middleware entry. The same rule applies to login insertion and challenge consumption. Retry bounded MVCC conflicts by rerunning all predicates, never by reusing an authorization result.
- Reauth preserves the old absolute deadline, refreshes auth/idle time, revokes the old cookie, and inserts a new cookie in one transaction. It does not require already-recent auth, or it could never repair a stale session. Logout requires Origin/CSRF but not recent auth. Concurrent rotations have one winner; logout of the old session after rotation revokes only that old session (it cannot revoke an independently issued new cookie). If logout wins before rotation, rotation fails. Reordered Set-Cookie responses can leave a browser holding a revoked cookie; return 401 and require login, never resurrect it.
- Linearization: a mutation committed before recovery/logout/rotation may finish returning afterward; a mutation serialized after recovery is rejected. Recovery is not a rollback of earlier committed work. Reads admitted before recovery may finish; subsequent requests fail. Document this rather than promising cancellation of all in-flight work.
- Storage outage fails auth closed with sanitized `503`, never process-local fallback. Client mutation and audit must commit together; audit failure rolls back the mutation. Failed-login audit failure never permits login.

One-time material: 32 random bytes encoded as lowercase hex; use existing cryptographic primitives with purpose separation. Comparisons use constant-time verification. Challenge lifetime: 15 minutes, not renewable by browser. On uncertain transaction outcome, do not replay a secret response. Recovery is the repair path.

### Shared throttling

Proposed policy: all login/reauth attempts consume a per-username budget of 5 per 15-minute fixed window (not a rolling window) and a per-source budget of 30 per 5 minutes; challenge inspection, activation and reset share one budget of 10 per source per 15 minutes. A normal inspect+finish flow consumes two attempts; splitting routes must not multiply this budget. Known and unknown usernames use the same keyed normalization. Fixed-window boundary bursts are documented; no persistent account lockout. Return `429` plus `Retry-After`, not “account locked”. Successful login does not clear the source budget or permit another replica to bypass it.

Bound storage cardinality by mapping keyed username/source fingerprints into 4096 buckets per dimension, with operation-domain separation (login and reauth deliberately share one credential-attempt domain). Collisions can throttle unrelated users but never permit extra attempts. All applicable counters are reserved atomically before credential/code lookup and KDF admission by the service, not an optional HTTP pre-call. Invalid-format login names use a keyed raw-input bucket plus the same source bucket and dummy verification; do not turn syntax validation into an account-existence oracle. Malformed JSON is still 400. Validate bucket range 0–4095 and required dimensions in the store; callers cannot omit the username dimension for login/reauth. Saturation fails closed, never evicts an active bucket to admit a request. Expired counters reset by DB time; cleanup is bounded. A coarse process admission limit supplements, not replaces, durable counters. For this release, use the direct socket peer IP for throttling, even behind a proxy; ignore `Forwarded`, `X-Forwarded-For` and `X-Real-IP`. This deliberately aggregates proxy users and can reduce availability, but is spoof-resistant and avoids introducing an unverified chain parser. Normalize IPv4-mapped IPv6 to IPv4; missing peer metadata fails closed. Trusted forwarded Host remains separate: the proxy must overwrite it, reject duplicates/comma lists, and prevent direct access to the private listener. Forwarded client-IP support is deferred, not an approved requirement. Test spoofed forwarded headers, different replicas, restart, random-user floods, and bucket collisions. These numerical limits and collision trade-off require review.

## 8. Browser/API contract — Proposal

All paths in this section are **NEW**. Existing OIDC paths remain unchanged only in OIDC mode. Local requests are same-origin JSON with a 16 KiB body limit, strict DTOs (`deny_unknown_fields`), validated content type, deadlines, exact allowed Host/Origin, no permissive CORS, and no credentials in URLs. Responses with auth/session/client/key data use `Cache-Control: no-store`; auth pages use `Referrer-Policy: no-referrer` and no third-party assets/analytics.

### Pre-auth and session routes

| Method/path | Request / response |
|---|---|
| `GET /api/v1/auth/config` | Public `{mode:"local"|"oidc"}` only, when control plane enabled. No admin count, signup state, secrets or issuer credentials. |
| `GET /api/v1/auth/local/csrf` | Establish short-lived pre-auth cookie; return bound CSRF token. Five-minute signed pre-auth lifetime, no identity. |
| `POST /api/v1/auth/local/challenge` | `{code,kind:"activate"|"reset"}` validates without consuming; returns `{username,expires_at}` only for valid material. Rate-limited; all invalid/expired/used codes return identical `400 invalid_challenge`. |
| `POST /api/v1/auth/local/activate` | `{code,password}`; atomic activate; `204`; no session. |
| `POST /api/v1/auth/local/reset` | `{code,password}`; atomic reset; `204`; no session. |
| `POST /api/v1/auth/local/login` | `{username,password}`; `204` with new admin cookie. Invalid credentials always `401 invalid_credentials`. Rotate pre-auth cookie on success. |
| `GET /api/v1/admin/session` | `{admin_id,username,auth_time,absolute_expiry,csrf_token}`; requires local session, not bearer key. |
| `POST /api/v1/admin/reauth` | `{password}`; verify current admin, rotate session and CSRF under generation/epoch fence; `204`. |
| `POST /api/v1/admin/logout` | Revoke presented session, clear cookie; valid-session request requires CSRF; repeat with no session may return `204` but must not mutate another session. |

Cookies: `__Host-memory_mcp_admin` and `__Host-memory_mcp_admin_preauth`, `Path=/; Secure; HttpOnly; SameSite=Strict`, no Domain. Require TLS in real browser tests; do not add an insecure production cookie switch. Pre-auth CSRF uses a random signed timestamped cookie plus domain-separated HMAC token, and exact Origin checking on every POST, including login and activation. SameSite alone is not sufficient. Pre-auth signatures also bind policy epoch. Their five-minute timestamp is checked against server UTC with a reviewed maximum clock-skew allowance; unlike durable session deadlines, this stateless check does not use database time. Recheck current policy before admitting POSTs, require synchronized replica clocks, and fail closed on an out-of-range timestamp. Session CSRF binds admin id, session id and policy epoch. Both pre-auth and session mutations send their bound token in `X-CSRF-Token`. Reject ambiguous duplicate cookie values. No localStorage/sessionStorage/IndexedDB for passwords, codes, CSRF or API secrets.

Credential-affecting and client-mutating operations require authentication within 600 seconds. A stale session receives `403 reauth_required`; UI asks for the password and retries only after explicit user confirmation. Login/reauth use the same durable throttle mechanism. No password reset from a browser session; recovery remains CLI-only.

### Client administration routes

| Method/path | Contract |
|---|---|
| `POST /api/v1/admin/clients` | `{display_name}` plus `Idempotency-Key` UUID; creates metadata + Account/Tenant + provisioning event + audit atomically; `202 ClientView` and Location `/api/v1/admin/clients/{account_id}`. `ClientView` is defined in the plan and includes account/tenant status, schema/plan version and CAS version; initial tenant status is `reserved`, replay returns the same resource's current view. |
| `GET /api/v1/admin/clients?after=<opaque>&limit=50` | Stable Account-id cursor; max 100; `{items,next_cursor}`. Lists clients created by this local workflow, including failed/suspended clients. No adoption of preexisting OIDC accounts in this scope. |
| `GET /api/v1/admin/clients/{account_id}` | Metadata, account status, tenant status/schema version, safe provisioning reason, plan version. No namespace credentials/raw storage error. |
| `GET /api/v1/admin/clients/{account_id}/keys` | Paginated `{items,next_cursor}` with stable key-id cursor and limit 50/max 100 (same `after`/`limit` contract as clients). Metadata only: id, name, status, created_at, expires_at, last_used_at. Expired is a derived UI status, not a new stored ApiKeyStatus. History can grow despite the active-key cap; never return an unbounded Vec. |
| `POST /api/v1/admin/clients/{account_id}/keys` | `{name,expiry:{kind:"never"}}` or `{name,expiry:{kind:"days",days:30}}`, required explicit choice; `Idempotency-Key`; `201 {id,name,secret,expires_at}`. `secret` is full `mem_sk_...` credential, as in existing account adapter. |
| `DELETE /api/v1/admin/clients/{account_id}/keys/{key_id}` | Scoped revoke, idempotent `204` for already revoked owned key; wrong-owner key `404`; records audit on first change. |
| `POST /api/v1/admin/clients/{account_id}/suspend` | `{expected_version}`; CAS; atomic Account + Tenant state change + audit; `204`. |
| `POST /api/v1/admin/clients/{account_id}/resume` | `{expected_version}`; CAS; atomic Account + Tenant state change + audit; `204`. |

Validation: display/key names trim outer whitespace, 1–100 Unicode scalar values and ≤400 UTF-8 bytes, reject control characters; escape as text in UI. Duplicate display/key names are allowed; ids disambiguate. Expiry days 1–3650 with checked timestamp addition; missing/null expiry is invalid rather than accidental non-expiring issuance. “Never expire” maps to existing `expires_at=None`.

Client create idempotency is scoped to admin + operation + request UUID. Same request fingerprint returns the same public resource; different body with same key is `409 idempotency_conflict`. Key create retries never return the secret twice: committed duplicate returns `409 secret_already_issued` with key id. On a lost response the admin lists/revokes the unknown key and explicitly creates another. Keep operation records with their resources; no TTL that could later reuse an issuance id.

Key issuance requires Active Account and Ready Tenant, validated again in the transaction. Preserve configured plan cap and count only nonexpired active keys. Serialize issuance for a given Account by updating a shared sidecar/version guard before counting; concurrent inserts cannot exceed the cap. Generate credential locally, store verifier only, append actor/target/key audit in the same transaction. Pass the expiry choice to storage; derive created_at/expires_at from database time inside the insertion transaction and return persisted metadata. Never accept a browser-computed deadline or calculate the issued deadline on a replica before waiting for admission. Do not call existing `ApiKeyCreation::execute` and then append an audit separately. Extract its key-material construction into a reusable pure helper or add an atomic admin persistence path without changing the OIDC contract.

For this release, suspend only a Ready client; resume only a client with Account=Suspended, Tenant=Suspended and sidecar `suspended_from_ready=true` set by this workflow. Suspend requires Account=Active, Tenant=Ready and marker=false; resume restores that pair and clears the marker. Reserved/Migrating/Failed/Deleting/Purged yield `409`; UI explains why. This avoids falsely resuming an unfinished tenant as Ready. After authorization, validate the coherent pair/marker first. A request already at its coherent desired state returns `204` without writes/audit even if its expected_version is old; otherwise stale expected_version is `409` and triggers refresh. Never repair an incoherent pair implicitly. Mutations increment both sidecar version and the existing Tenant version; issuance/revoke also advance the sidecar guard and may require a UI refresh. Suspending changes both Account and Tenant atomically so cached bearer principals are rejected on their next Account reread; existing subscriptions must fail their next current-auth check. Resume preserves keys; revoked/expired keys stay invalid. This requires T7 cache-hit key revalidation on every bearer request in all modes, not just local cache invalidation. Existing `is_current` subscription polling remains bounded by configured recheck interval (default 30 seconds, maximum 60); in-flight operations are not retroactively cancelled. Data-plane expiry currently uses replica UTC time: maintain synchronized clocks and test the exact boundary with a controlled clock; do not claim DB-time expiry enforcement there.

Errors: malformed body `400`, missing/invalid admin session `401`, CSRF/Origin/recent-auth `403`, absent or wrong-owner target `404`, state/CAS/idempotency/key-cap conflict `409`, throttle `429`, store/admission outage `503`. Use stable `{error:{code,message,key_id?}}` (key_id only for secret_already_issued), a server-generated request-id header, existing `MemoryError` for infrastructure, and the plan's typed local failure wrapper for domain outcomes. Unsupported Content-Type is 415, oversized bodies 413, and local deadline/admission exhaustion 503; malformed JSON remains 400. These are local contracts, not the existing OIDC mapper. No storage SQL, hashes, usernames on failed login, or secrets in error messages.

### Bypass protection

In local mode do not mount `/auth/oidc/*`, `/api/v1/account/*`, or `/api/v1/operator/*`, including identity linking and purge. Explicit API/auth fallback returns `404`, not SPA HTML. In OIDC mode do not mount local login/admin routes. Both branches check mode policy at the service boundary, not only router construction. No automatic account creation at local login/activation. Bearer client keys cannot administer clients; OIDC cookies cannot authenticate locally; admin cookies cannot authorize data-plane access. `X-Operator-Auth: stub` and seeded sessions remain fixture-only and must not activate production local routes.

## 9. UI — Proposal

- `/login`: fetch mode config; render existing OIDC flow or local username/password. Generic error text, pending state, password-manager autocomplete, keyboard submission.
- `/admin/activate` and `/admin/reset`: paste code; validate; show readonly username; password/confirmation fields; clear code/password after submission/unmount. Invalid/expired/used code uses the same message and points to CLI recovery. Success redirects to login, never creates a client.
- `/admin/clients`: pagination, create form, loading/error/empty states, stable ids, status/plan display. Poll visible nonterminal provisioning states every 2 seconds, back off to 30 seconds on failure, stop on unmount; Failed is shown with safe reason, not infinite loading.
- `/admin/clients/:account_id`: metadata/readiness; key name/expiry form; no silent “Never expire”; secret reveal panel shown only from successful POST result, explicit copy action, “Save this now; it cannot be shown again. Deliver it outside this service.” Clear on close/navigation/logout. Do not auto-retry key POST.
- Explain privilege honestly: “Administrators can issue keys that access this client's memory. Issuance is audited.” Do not claim tenant privacy from administrators.
- List key status/expiry/last use and scoped revoke confirmation. Suspend/resume confirmation and stale-state refresh. No delete/quota/identity-link screens in local mode, even on direct navigation.
- Reauth dialog, visible session-expiry handling, unsaved-secret warning, accessible labels/focus/error region. No token in route/query/fragment, browser persistence, telemetry, or screenshots used as test artifacts.

## 10. Audit and operational security — Proposal

Audit administrator create/recovery, activation/reset, login outcome, logout/reauth, client creation, key issuance/revocation, suspend/resume, and rejected policy/generation fences. CLI actor is `local_cli` plus safe correlation id, not a fabricated authenticated human identity. Browser actor is durable admin id. Key issuance records target Account/Tenant/key id and `grants_client_data_access=true`; never stores the key or password hash in audit.

Use typed allowlisted event fields. Redact request bodies, Authorization/Cookie/Set-Cookie, activation/reset material, passwords, PHC hashes, HMAC verifiers, CSRF tokens, and raw IP/username throttle input from tracing, errors, metrics and Debug. Avoid user-controlled metric labels. An audit row is not permission to log an entire command DTO. Security events before identity resolution use safe fingerprint/bucket and outcome, not guessed admin ids. Durable success auditing is transactional. A rejected transaction cannot append its failure audit and then THROW: rollback would erase that event. After rollback, write a separate bounded failure event, or return a committed typed rejection from a no-mutation transaction. Failed-login/fence audit storage failure returns sanitized 503, never success. Throttle rows aggregate denied attempts in-place with saturating counters; no append per denied request. Admitted failures may append at most one deduplicated event per reserved attempt. Session/client/pre-admission failures without such a reservation use fixed action/reason aggregate slots in the same rate table, not an unbounded append-per-invalid-cookie or append-per-stale-mutation stream. Bounded process admission precedes audit I/O; aggregate storage failure still fails closed. Recovery/CLI success audit is not best-effort. Enforce append-only behavior at the application boundary; privileged DB credentials can bypass table permissions, so this is not tamper-proof audit storage.

Registry backup contains password hashes and session verifiers: protect it as a credential database. Restoring an old snapshot can resurrect old credentials/generations; recovery after restore must rotate browser session/CSRF keys and invalidate all local sessions/challenges before traffic returns. Keep CLI access restricted to operators with registry credentials. Multi-admin equality does not remove the need for host/DB access controls.

## 11. Packaging, rollout and compatibility — Proposal

Build and test from source; do not assert a published image/tag supports local authentication. Extend the Docker build to bundle Dioxus assets, embed them via the existing absolute-path build contract, and ship `memory_mcp` alongside `memory_mcp_http`. Preserve nonroot, shell-free runtime and current HTTP entrypoint. Admin commands run by entrypoint override, not `docker exec ... sh`. Preserve migration assets at the compiled path until the existing loader changes separately.

Current Compose injects nonempty OIDC keys and example secrets unconditionally. T10 must remove OIDC-only defaults from the local/off profile and require operator-supplied local secrets, mode, seven limits and HTTPS settings; merely adding AUTH_MODE would fail local validation. Test all three runtime modes with resolved Compose configuration without printing secrets.

Use a protected operator environment file containing control-registry credentials for CLI; the HTTP deployment also needs tenant credentials, pepper and browser keys. Do not bake secrets or insecure example defaults into images. UI build needs compatible Dioxus CLI 0.7 and wasm target; pin the exact tested CLI version in CI after verification. Installing build tooling is not permission to add runtime dependencies.

Local rollout: get design/dependency/migration approval; back up registry; stop incompatible replicas; apply append-only schema; set explicit mode/plan/HTTPS; create first admin; activate/login; create test client; wait Ready; issue key; verify isolation and revoke/suspend behavior. Start additional replicas only after shared-store race tests pass. A data-plane-only process does not need browser configuration or a local default plan and can serve existing keys while UI is disabled.

Rollback: disable local control plane before running an older binary; retain additive tables; preserve data-plane Accounts/Tenants/keys. Do not run old OIDC signup code against a local-mode registry. Re-enabling local mode retains admins unless an explicit credential recovery/rotation is performed. Mode migration/account adoption, admin removal, MFA, email, self-registration, client billing/quota editing, and purge are outside scope.

## 12. Acceptance, traceability and open approval decisions

| Requirements | Plan tasks | Required evidence |
|---|---|---|
| R1, R9, R10 | 1, 2, 6, 10 | Mode/config matrix; no discovery in local/off; durable mode fence; direct signup-route probes; explicit plan mismatch rejection. |
| R2, R3 | 2–6, 8 | Two equal admins with no Accounts/Tenants; unique CLI username; browser cannot replace it; activation one winner across handles. |
| R4, R11 | 2–6, 10 | Recovery races against activation/reset/login/session touch/client mutation; no stale credential session or mutation after fence; shared throttle tests. |
| R5, R9 | 7, 9 | Atomic create/idempotency, one Account/Tenant, provisioning restart, paginated metadata, no delete/quota editing. |
| R6, R8 | 7, 9, 10 | Expiry/never-expire DTO tests, scoped revoke, cap race, one-time response loss, atomic audit and redaction. |
| R7, R10 | 7, 9, 10 | Suspend/resume CAS and key preservation; existing-key data-plane-only and tenant-isolation regression. |
| R11 | 3–10 | KDF/dummy work/admission, Origin+CSRF, cookie flags, HTTPS browser tests, secret leak scanning. |
| R12 | Planning deliverable | Original two-document deliverable preserved; subsequent requests add the audit report and ADR-0055 with related cross-references. No application/dependency/migration edits or commits. |

**Decisions requiring technical approval:** Argon2 dependency/version and parameters; challenge/session/throttle limits and bucket-collision trade-off; default OIDC compatibility setting; offline mode-switch policy; new schema/migration; ready-only suspension; local-created-client-only listing; direct-peer throttle aggregation; 15-minute admission hold during local key rotation; legacy OIDC-session re-login on epoch upgrade; explicit key idempotency/no-secret-replay behavior; container CLI/UI inclusion. These are specified proposals so review can accept or amend them; they are not silently added to the approved requirement set.

**Verification still required at execution:** actual SurrealDB multi-record conflict/conditional-write semantics on 3.2.4, especially shared guard writes; migration upgrade behavior; KDF resource cost; exact Dioxus bundle layout/CLI version; final image contents; production proxy/TLS behavior. If any cannot meet the proposed invariant, stop and revise the design rather than weakening it or substituting a process-local lock.
