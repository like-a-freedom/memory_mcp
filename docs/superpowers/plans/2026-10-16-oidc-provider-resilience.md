# OIDC Provider Resilience Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Start and keep serving the HTTP product during an OIDC outage, recover OIDC automatically, and refuse operations lacking valid authentication evidence with accurate, bounded responses.

**Architecture:** Replace mandatory startup discovery with a configured OIDC runtime holding an optional immutable discovered client and an owned recovery worker. Retain discovery for the process lifetime; independently maintain the existing JWKS cache through one refresh gate shared by background and request paths. Keep provider protocol mechanics in `control/oidc/`, authentication/HTTP mapping in their current adapters, and task ownership in HTTP bootstrap.

**Tech Stack:** Rust 1.99.0, existing Tokio/tokio-util, reqwest, jsonwebtoken, serde, thiserror, rand, chrono, metrics and axum; existing HTTP subprocess and durable-registry fixtures. No manifest changes.

**Spec:** [2026-10-16-oidc-provider-resilience.md](../specs/2026-10-16-oidc-provider-resilience.md). Read the entire spec and this plan before implementation. This is a proposed plan; this planning turn does not authorize implementation.

## Global Constraints

- Workspace `rust-version` is `1.99.0`.
- No changes to `Cargo.toml`, dependency versions, generated code or migration files.
- Preserve `default = ["fs-watch"]`; `streamable-http` remains the public HTTP product switch.
- Preserve the frozen 8-tool MCP surface.
- Business use cases remain in the owning context's `api.rs`; OIDC protocol/network mechanics remain in `control/oidc/`, and worker ownership remains at the HTTP composition edge.
- No new business logic in `main.rs` or `src/service/`; inject narrow dependencies rather than `MemoryService` or `HttpState` into the provider worker.
- No production `unwrap()`, process-global environment mutation in new tests, or secret/raw-token logging.
- Preserve Account status checks, tenant resolution, API-key revocation, session expiry, CSRF, recent-auth, PKCE, nonce and authorization-state replay protection.
- Do not change configured authentication methods, durable browser-policy fingerprints or epoch because the provider is unavailable.
- Run workspace clippy with `--features fs-watch,mcp-apps,streamable-http --locked -- -D warnings` before shipping.
- Prefix shell commands with `rtk`; use Octocode before source reads. Preserve the pre-existing changes in `.github/workflows/{ci,evaluations,release}.yml` and never stage them with feature commits.
- Tests using network, databases or subprocesses are integration tests. New pure unit tests use controlled time/source adapters. Use permits/channels/future polling for ordering, and bound every wait and child lifetime.
- TDD applies per task: add the named regression, observe its failure, implement, rerun, then commit only that task. A compile failure proves only that a new interface is missing; rerun until the original behavioral regression is demonstrably red before claiming regression coverage.

## Review Focus

- Failed refresh with queued callers and varying unknown `kid`: share failure and cap upstream fetches globally, while fresh known keys continue working (Task 2).
- Successful empty JWKS replacement and exact TTL boundary: never retain removed keys or extend trust through a failed fetch (Task 2).
- Wrong-issuer metadata, redirect responses, oversized chunked bodies and malformed/date-form `Retry-After`: reject untrusted input, keep HTTP alive, and avoid tight retries (Tasks 1, 3 and 6).
- Token endpoint outage during a valid cached-JWT request, and the default `auto` algorithm policy: preserve independent paths and use the discovered policy without weakening claims checks (Task 4).
- Shutdown during blocked discovery/JWKS I/O: reap the OIDC owner and still attempt other background cleanup under one absolute deadline (Tasks 3 and 6).

---

## File structure and boundaries

Paths below are repository-relative; line anchors refer to the inspected checkout and will move.

| File | Responsibility/change |
| --- | --- |
| Create `crates/memory-mcp/src/control/oidc/provider.rs` | Narrow provider settings, reusable bounded HTTP client, typed sanitized provider failures and discovery transport. |
| Create `crates/memory-mcp/src/control/oidc/retry.rs` | Shared bounded scheduling and monotonic clock interface; no HTTP/registry dependency. |
| Create `crates/memory-mcp/src/control/oidc/runtime.rs` | Optional discovered-client snapshot and recovery lifecycle; no `HttpState`/registry container. |
| Create `crates/memory-mcp/src/control/oidc/diagnostics.rs` | Component snapshots, transition events and bounded provider metrics. |
| Modify `crates/memory-mcp/src/control/oidc/{client,jwks,flow_material,handlers}.rs` and façade `control/oidc.rs` | Provider I/O, cache gate, typed auth failures and runtime snapshot consumers. Preserve flow cryptography and identity workflows. |
| Modify `crates/memory-mcp/src/http.rs:73,331` | Store `Option<Arc<OidcRuntime>>`; construct enabled runtime without discovery. |
| Modify `crates/memory-mcp/src/http/runtime/bootstrap.rs:23,45,360` | Spawn, cancel and join the one OIDC worker. |
| Modify `crates/memory-mcp/src/http/test_state.rs:177` | Retain ready-client fixture support and add explicit cold-runtime fixture support. |
| Modify `crates/memory-mcp/src/http/{oauth.rs,middleware/auth.rs}` | Preserve provider-unavailable outcomes through OAuth authentication; keep API-key success independent. |
| Modify `crates/memory-mcp/src/control/error.rs` | 401/503 distinction and no-store/retry headers for OIDC unavailability. |
| Modify `crates/memory-mcp/src/http/health.rs` | Add bounded OIDC component details without changing overall readiness status logic. |
| Modify `crates/memory-mcp/src/{observability.rs,shared/observability.rs}` | Register/describe the new metric families using the current registry conventions. |
| Create `crates/memory-mcp/tests/common/oidc_provider.rs` | Owned programmable loopback provider; real signing key, request counters and permit-controlled blocking. |
| Create `crates/memory-mcp/tests/{oidc_provider_transport,oidc_jwks,http_oidc_resilience}.rs` | Provider transport, cache/crypto and real-binary outage/recovery integration coverage. |
| Modify `crates/memory-mcp/tests/{http_health,http_background_shutdown}.rs` | Readiness independence and background-owner cleanup regression coverage. |
| Modify `docs/adr/0057-additive-browser-auth-methods.md`, `docs/operations/{LIMITATIONS,LOCAL_ADMIN,LOGGING}.md`, `observability/README.md` | Correct the recorded startup limitation and document actual degradation/recovery and diagnostics after passing acceptance. |

Do not refactor unrelated authentication modules or modify identity business rules. No UI or new diagnostic endpoint is needed: extend the existing readiness JSON and metrics. `control/oidc/test_provider.rs` remains a private fixture for existing tests; the new external integration fixture reuses the existing PEM file, not production-test visibility widening.

### Task 1: Bound and classify OIDC provider calls

**Files:**
- Create: `crates/memory-mcp/src/control/oidc/provider.rs`, `crates/memory-mcp/src/control/oidc/retry.rs`.
- Modify: `crates/memory-mcp/src/control/oidc.rs`, `crates/memory-mcp/src/control/oidc/client.rs:31,140`, `crates/memory-mcp/src/control/oidc/flow_material.rs:158`, `crates/memory-mcp/src/control/error.rs:158`, `crates/memory-mcp/src/control/oidc/handlers.rs:185`.
- Create/Test: `crates/memory-mcp/tests/common/oidc_provider.rs`, `crates/memory-mcp/tests/oidc_provider_transport.rs`.

**Interfaces:**
- Keep public `OidcClient::new(issuer: &str, client_id: &str, audience: &str, redirect_uri: &str, allowed_algorithm: &str) -> Result<Self, MemoryError>` async as the eager adapter API used by existing clients/tests. Local settings errors are `ConfigInvalid`; remote failures map to `Unavailable` rather than `ConfigInvalid`.
- Produce crate-visible `ProviderSettings::new(issuer: &str, client_id: &str, audience: &str, redirect_uri: &str, allowed_algorithm: &str) -> Result<Self, MemoryError>`; it copies only those five fields and performs local URL/algorithm validation without I/O.
- Produce crate-visible `provider::http_client() -> Result<reqwest::Client, MemoryError>` and async `OidcClient::discover(settings: &ProviderSettings, http: &reqwest::Client) -> Result<Self, ProviderFailure>`.
- `ProviderFailure` is cloneable/thiserror-based with `kind: ProviderFailureKind`, `status: Option<u16>`, `retry_after: Option<Duration>`; kinds are `Transport`, `Timeout`, `Http`, `InvalidResponse`. Its display contains bounded labels/status only. It never stores a request body, token, code or raw reqwest error string.
- Produce `OidcClient::jwt_validation(&self, algorithm: jsonwebtoken::Algorithm) -> Result<jsonwebtoken::Validation, AuthError>`; it enforces the resolved safe algorithm policy, published issuer, existing audience and expiry policy, and is shared by ID-token and later access-token validation.
- Extend `AuthError` with `InvalidGrant`, `UnknownKeyId`, and `Unavailable { retry_after: Duration }`; map the first two to 401 and the last to 503. Keep existing variants compatible; update exhaustive matches and bounded reason tags in this task.
- Update `From<AuthError> for MemoryError` in `flow_material.rs`: provider/JWKS unavailability becomes `MemoryError::Unavailable`, invalid grant/key/credential errors become `MemoryError::Auth`; reserve `ConfigInvalid` for local configuration failures. Preserve the sealing error's existing integrity/error treatment.
- Produce crate-visible `OidcClock: Send + Sync` with `fn now(&self) -> std::time::Instant` and `fn sleep_until(&self, deadline: std::time::Instant) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>`. `SystemOidcClock` delegates to monotonic time and Tokio; controlled test clocks release sleepers explicitly.
- Produce `retry::retry_delay(attempt: u32, sample: u64, failure: &ProviderFailure) -> Duration` and `retry::refresh_age(sample: u64) -> Duration`. Implement the spec's equal-jitter/backoff/caps; `sample` comes from existing `rand::random::<u64>()`, not a new dependency.
- Integration fixture exports `OidcProviderFixture::start() -> Self` async; `issuer(&self) -> &str`; `set_reply(&self, route: ProviderRoute, reply: ProviderReply)`; `count(&self, route: ProviderRoute) -> usize`; async `next_request(&mut self, route: ProviderRoute) -> ProviderRequest`; `hold(&self, route: ProviderRoute) -> RequestGate`. Routes are `Discovery`, `Jwks`, `Token`; reply fields are `status: StatusCode`, `headers: HeaderMap`, `body: Vec<u8>`; request fields include parsed token-form values for fixture assertions only; `RequestGate::release(self)` unblocks a held response. Default replies publish a valid discovery document and the existing RSA fixture public key. Drop aborts/reaps fixture tasks, and `next_request` has a 10-second deadline. Keep all submitted secrets out of assertion messages/logs.

- [ ] **Step 1: Write failing transport integration cases.** Add independently named tests with these focal assertions (use the fixture API above; each assertion belongs to its own test):

```rust
// discovery_502_is_unavailable_not_bad_configuration
assert!(matches!(observed, Err(MemoryError::Unavailable(_))));
// discovery_redirect_is_not_followed
assert_eq!(redirect_target.count(ProviderRoute::Discovery), 0);
// chunked_discovery_over_262144_bytes_is_refused
assert!(observed.is_err());
// token_timeout_is_bounded_and_does_not_replay_code
assert!(tokio::time::timeout(Duration::from_secs(6), exchange).await.is_ok());
assert_eq!(provider.count(ProviderRoute::Token), 1);
// invalid_grant_is_a_credential_refusal
assert!(matches!(observed, Err(AuthError::InvalidGrant)));
```

Also retain separate wrong-issuer, missing-endpoint and unsupported-algorithm cases from the existing client tests; update only their expected typed remote-error class. Add pure schedule cases `retry_502_uses_backoff`, `retry_exponent_saturates`, `retry_after_delta_is_capped`, `retry_after_date_is_bounded`, `malformed_or_past_hint_uses_backoff`, and `invalid_metadata_retries_after_60_seconds` through `retry_delay`, with fixed random samples; assert the exact spec bounds and 300-second maximum server hints. Parse HTTP-date headers at the transport boundary before constructing `ProviderFailure`; the schedule itself consumes only durations and needs no wall clock.

- [ ] **Step 2: Run the red tests.** Run `rtk cargo test -p memory_mcp --features streamable-http,test-fixtures --locked --test oidc_provider_transport`. Expected: current 502 classification, redirect/body/timeout behavior fail their named assertions; missing-interface errors are resolved before recording the behavioral red run.
- [ ] **Step 3: Implement the provider interfaces and fixture.** Enforce 2-second connect/5-second total timeouts, no redirects and streaming body cap 262,144 bytes on all three endpoint types; parse only the bounded OAuth `error` code for invalid-grant classification. `Retry-After` supports integer seconds and HTTP dates using existing chrono parsing. Preserve issuer trailing-slash compatibility and `auto` safe-set intersection. `exchange_code` reuses the client and sends one POST only; the returned body is never logged. Existing eager construction delegates to the typed discover adapter.
- [ ] **Step 4: Run green transport and policy coverage.** Run the Step 2 command and `rtk cargo test -p memory_mcp --features streamable-http,test-fixtures --locked --lib control::oidc::client`. Expected: transport regressions and existing claim/algorithm/PKCE tests pass; errors expose no raw fixture code or token.
- [ ] **Step 5: Commit only Task 1 paths.** Stage the task's Files list plus the two planning documents using the exact staging rule below; commit with `rtk git commit -m 'fix(oidc): bound provider calls and classify outages'`.

### Task 2: Make JWKS refresh bounded, shared and expiry-safe

**Files:**
- Modify: `crates/memory-mcp/src/control/oidc/jwks.rs:18,62,87`, `crates/memory-mcp/src/control/oidc.rs`, and Task 1's `provider.rs`/`retry.rs` where the bounded GET adapter is reused.
- Test: controlled-source unit cases in `jwks.rs`; create `crates/memory-mcp/tests/oidc_jwks.rs` using `tests/common/oidc_provider.rs`.

**Interfaces:**
- Consume Task 1's clock, sanitized provider failure, scheduling policy and auth errors.
- Preserve public `JwksCache::from_parts(client: reqwest::Client, jwks_uri: String, ttl: Duration) -> Self`, async `key_for(&self, kid: &str) -> Result<DecodingKey, AuthError>`, and async `refresh(&self) -> Result<(), AuthError>`.
- Produce `JwksStatus { Empty, Fresh, Stale }` and `JwksCache::status(&self) -> JwksStatus` for nonblocking diagnostics.
- Produce crate-visible async `JwksCache::maintain(&self, cancel: CancellationToken)`; this performs immediate warmup and drives future refresh/retry, using the exact same fetch gate as `key_for`/`refresh`.
- For controlled unit inputs, introduce crate-visible `JwksSource: Send + Sync` with async `fetch(&self) -> Result<HashMap<String, DecodingKey>, ProviderFailure>` using the existing async-trait convention. `JwksCache::from_source(source: Arc<dyn JwksSource>, clock: Arc<dyn OidcClock>, ttl: Duration) -> Self` is the internal constructor also used by `from_parts`, not test-only production visibility. The HTTP source owns JWKS parsing/filtering and the shared bounded transport.

- [ ] **Step 1: Add cache behavior regressions through `key_for`, `refresh` and `maintain`.** Use a controlled source, explicit polling/barriers and clock advances, with separate tests for each scenario:

```rust
// failed_refresh_is_shared_by_waiters
assert_eq!(source.fetch_count(), 1); // after a gated cohort observes one failure
// varying_unknown_kids_share_the_global_cooldown
assert_eq!(source.fetch_count(), 1); // no clock advance, different misses
// cached_known_key_survives_failed_early_refresh
assert!(cache.key_for("old").await.is_ok()); // successful-fetch age 270s
// expired_key_is_not_served_after_failed_refresh
assert!(matches!(observed, Err(AuthError::Unavailable { .. }))); // age exactly 300s
// empty_successful_replacement_removes_old_keys
assert!(matches!(observed, Err(AuthError::UnknownKeyId)));
```

Add independent cases proving unknown-key refresh discovers a rotated key after the 1-second minimum interval, a failed fetch does not reset successful-fetch age, a background refresh occurs in the 240–270-second window without requests, a fresh key is not blocked by an unrelated miss in flight, and `refresh()` cannot bypass cooldown. Integration cases verify real RSA verification/key rotation and JSON/body/status failures against loopback HTTP; tests with network are kept in `tests/oidc_jwks.rs`.

- [ ] **Step 2: Run red cache tests.** Run `rtk cargo test -p memory_mcp --features streamable-http,test-fixtures --locked --lib control::oidc::jwks`, then `rtk cargo test -p memory_mcp --features streamable-http,test-fixtures --locked --test oidc_jwks`. Expected: current refresh request counts or typed absent-key outcomes fail; keep the observed pre-fix failure evidence.
- [ ] **Step 3: Implement the single refresh gate.** Track successful-fetch time, generation, shared last outcome, active refresh and next-attempt time. Check fresh known keys before waiting. All callers from the same cohort observe that generation's outcome; cooldown is provider-wide, not keyed by attacker-supplied `kid`. On successful fetch replace the complete key set, including an empty set; on failure preserve keys and their original time. Return `UnknownKeyId` only when a successful current lookup establishes absence; return typed unavailability with positive remaining cooldown when verification material cannot be refreshed. Hold no synchronous lock across `.await`. Preserve the accepted key types/algorithm filters and 32-key limit.
- [ ] **Step 4: Run green cache and client coverage.** Repeat Step 2 and Task 1's transport test. Expected: all cache scenarios pass, no request storm, no expired-key fallback, and proactive upkeep can be cancelled while sleeping or fetching.
- [ ] **Step 5: Commit only Task 2 paths.** `rtk git commit -m 'fix(oidc): coalesce jwks refresh and enforce cache expiry'` after exact staging.

### Task 3: Decouple HTTP startup and own OIDC recovery/shutdown

**Files:**
- Create: `crates/memory-mcp/src/control/oidc/runtime.rs`.
- Modify: `crates/memory-mcp/src/control/oidc.rs`, `crates/memory-mcp/src/http.rs:73,331`, `crates/memory-mcp/src/http/runtime/bootstrap.rs:23,45,360`, `crates/memory-mcp/src/http/test_state.rs:177`, `crates/memory-mcp/src/control/oidc/handlers.rs:203,342,643`, `crates/memory-mcp/src/http/middleware/auth.rs:126`.
- Test: create initial `crates/memory-mcp/tests/http_oidc_resilience.rs`; extend `crates/memory-mcp/tests/http_background_shutdown.rs` and existing browser-flow cases in `handlers.rs`.

**Interfaces:**
- Consume `ProviderSettings`, `OidcClient::discover`, `OidcClock`, the retry policy and `JwksCache::maintain` from Tasks 1–2.
- Produce public `OidcRuntime::new(config: &OidcBrowserConfig) -> Result<Arc<Self>, MemoryError>`; this is local validation/allocation only.
- Produce `OidcRuntime::client(&self) -> Result<Arc<OidcClient>, OidcUnavailable>` and crate-visible async `OidcRuntime::run(self: Arc<Self>, cancel: CancellationToken)`. `OidcUnavailable` is cloneable/thiserror-based with `retry_after: Duration`; it contains no raw error text. A short lock protects the optional ready-client snapshot and retry deadline; no lock spans I/O.
- Change `HttpState.oidc_client` to `HttpState.oidc: Option<Arc<OidcRuntime>>`. `None` means method disabled, never provider temporarily unreachable.
- Add `oidc_cancel: CancellationToken` and `oidc_worker: Option<JoinHandle<()>>` to `HttpRuntime`. Only `build_state` starts the production worker after all fallible state composition has succeeded. Join it concurrently with other background owners under the existing absolute cleanup deadline; abort and await an overdue handle, and record secondary failures without skipping other owners.
- Preserve `HttpStateTestBuilder::with_oidc_client(client: Arc<OidcClient>) -> Self` by wrapping the ready client in a fixture-only runtime constructor. Add `with_oidc_runtime(runtime: Arc<OidcRuntime>) -> Self` for cold-state tests; fixture tests explicitly own any worker they start.

- [ ] **Step 1: Add the startup/ownership regression.** In `http_oidc_resilience.rs`, add `cold_oidc_outage_preserves_api_key_service` using `common::http_server::{HttpServerConfig,HttpServerFixture,TestTenant,mcp_call}` and the programmable provider. Configure `oidc`, the provider issuer, a fixed client/audience/redirect and `RS256`; keep embeddings disabled. Start with discovery 502, then assert:

```rust
assert_eq!(live.status(), StatusCode::OK);
assert_eq!(ready.status(), StatusCode::OK);
assert!(api_key_mcp_initialize_succeeded);
assert_eq!(authorize.status(), StatusCode::SERVICE_UNAVAILABLE);
assert!(authorize.headers().contains_key("retry-after"));
```

Use a separately named `cold_combined_auth_outage_preserves_local_login` case for `local,oidc`. In `oidc_recovers_without_auth_demand`, restore discovery and wait on provider request notifications without sending authorize requests to trigger discovery; then expect authorize 303. In `invalid_issuer_never_publishes_before_valid_recovery`, return wrong-issuer metadata followed by a valid reply: HTTP stays available and recovery uses only the valid document. In `oidc_cleanup_runs_after_another_owner_fails`, block discovery and make another cleanup owner report failure; assert cancellation is observed and both joins are attempted.

- [ ] **Step 2: Run red startup and shutdown tests.** Run `rtk cargo test -p memory_mcp --features streamable-http,test-fixtures --locked --test http_oidc_resilience`, then `rtk cargo test -p memory_mcp --features streamable-http,test-fixtures --locked --test http_background_shutdown`. Expected: startup fails against 502 before the fix; the owned-worker cleanup assertion fails before cancellation is wired.
- [ ] **Step 3: Implement lazy runtime and migrate consumers.** Keep the durable browser-policy reconciliation unchanged and before worker spawn. The worker attempts discovery immediately, backs off on failure and retains the validated client for the process lifetime. Publish metadata before beginning JWKS upkeep. Every browser flow obtains one client snapshot; acquire it before storing new authorization flow material. Preserve consume-once callback/state ordering and PKCE/nonce checks. Existing OIDC routes stay mounted from configuration. Adapt the MCP fallback to read the runtime snapshot; Task 4 adds its final error semantics. Do not probe the token endpoint in the worker.
- [ ] **Step 4: Run green lifecycle and existing login tests.** Repeat Step 2 and `rtk cargo test -p memory_mcp --features streamable-http,test-fixtures --locked --lib control::oidc::handlers`. Expected: cold outage serves independent traffic, recovery occurs without auth demand, configured wrong issuer never becomes a client, existing browser flows still pass, and no worker survives cleanup.
- [ ] **Step 5: Commit only Task 3 paths.** `rtk git commit -m 'fix(http): start independently of oidc provider availability'` after exact staging.

### Task 4: Preserve authentication evidence and accurate 401/503 responses

**Files:**
- Modify: `crates/memory-mcp/src/http/oauth.rs:25,38,91`, `crates/memory-mcp/src/http/middleware/auth.rs:44,126`, `crates/memory-mcp/src/control/error.rs:80,158`, `crates/memory-mcp/src/control/oidc/handlers.rs:342`.
- Test: OAuth/middleware unit modules and integration cases in `crates/memory-mcp/tests/http_oidc_resilience.rs`.

**Interfaces:**
- Consume Task 3's `OidcRuntime::client` and Task 1's `OidcClient::jwt_validation`/typed `AuthError`.
- Change async `oauth::validate_token(token: &str, client: &OidcClient) -> Result<AccessClaims, TokenError>` to use the client's resolved issuer, accepted algorithms and JWKS; keep the access-token claim type separate from the ID-token type.
- Change async `oauth::resolve_bearer_principal(config: &HttpConfig, accounts: &dyn AccountStore, client: &OidcClient, token: &str) -> Result<Option<AuthenticatedPrincipal>, TokenError>`; `Ok(None)` means no eligible active Account/identity. Preserve existing registry-refusal behavior; this feature does not redesign registry outage classification.
- Add `TokenError::ProviderUnavailable { retry_after: Duration }`. Retain credential-fault variants. Both cfg versions of private async `oauth_bearer` return `Result<Option<AuthenticatedPrincipal>, TokenError>`; the no-OIDC version returns `Ok(None)`.
- Add `ApiError::OidcUnavailable { retry_after: Duration }`, preserving its request-id wrapper. Map `OidcUnavailable` and `AuthError::Unavailable` to this variant. It emits 503, existing `temporarily_unavailable`, `Cache-Control: no-store`, and integer `Retry-After` >= 1; round up remaining cooldown. Generic legacy provider/JWKS failures use a 1-second retry hint. Map `InvalidGrant`/`UnknownKeyId` to generic 401.

- [ ] **Step 1: Write independent refusal cases.** In `fresh_auto_jwt_survives_token_endpoint_outage`, warm real JWKS and verify a correctly signed access token with default `auto`; block only the token endpoint and assert MCP authentication still succeeds. In `unknown_key_with_failed_refresh_returns_503`, use a syntactically valid token with a new `kid` while JWKS is unavailable. In `successful_refresh_without_key_returns_401`, restore a successful JWKS response without that key and assert 401. Synchronize on provider requests/cooldown notifications; do not attempt to inject a controlled clock into a child process. Task 2's controlled-source test already proves the exact 300-second expiry boundary. With verification material available, use separately named `expired_token_is_refused`, `wrong_audience_is_refused`, `wrong_issuer_is_refused`, `invalid_signature_is_refused`, and `disallowed_algorithm_is_refused` cases. Focal assertions:

```rust
assert_eq!(provider_outage.status(), StatusCode::SERVICE_UNAVAILABLE);
assert_eq!(provider_outage.headers()["cache-control"], "no-store");
assert!(retry_after_seconds >= 1);
assert_eq!(known_absent_key.status(), StatusCode::UNAUTHORIZED);
assert!(known_absent_key.headers().contains_key("www-authenticate"));
assert!(fresh_jwt_mcp_initialize_succeeded);
```

Add separate durable-session/local-login outage cases preserving absolute/idle expiry, Account suspension and recent-auth refusal. Snapshot the browser-policy methods/epoch before and after outage/recovery and assert equality. Never accept malformed/missing credentials merely because the provider is down; assert 401 for those forms.

- [ ] **Step 2: Run red authentication cases.** Run `rtk cargo test -p memory_mcp --features streamable-http,test-fixtures --locked --test http_oidc_resilience`; run `rtk cargo test -p memory_mcp --features streamable-http,test-fixtures --locked --lib http::oauth`. Expected: outage currently collapses to 401, and `auto` is rejected by the access-token path's separate algorithm switch.
- [ ] **Step 3: Implement typed propagation and shared validation policy.** Validate credential form before classifying missing provider readiness. A successful API-key decision returns immediately without consulting OIDC. Propagate only provider-unavailable outcomes to 503; keep ordinary refusals generic. Use the published issuer for signature validation/identity lookup and the existing resource audience; do not accept ID tokens as access tokens through a new fallback. Keep link/signup/session business workflows unchanged. Preserve HTTP request correlation when mapping the new error variant.
- [ ] **Step 4: Run green authentication coverage.** Repeat Step 2 and `rtk cargo test -p memory_mcp --features streamable-http,test-fixtures --locked --lib http::middleware::auth`. Expected: independent paths succeed, real invalid credentials still fail, and 503/401/header/correlation assertions pass.
- [ ] **Step 5: Commit only Task 4 paths.** `rtk git commit -m 'fix(auth): distinguish oidc outages from invalid credentials'` after exact staging.

### Task 5: Expose bounded OIDC health and transition diagnostics

**Files:**
- Create: `crates/memory-mcp/src/control/oidc/diagnostics.rs`.
- Modify: `crates/memory-mcp/src/control/oidc/{runtime,provider,jwks}.rs`, `crates/memory-mcp/src/control/oidc.rs`, `crates/memory-mcp/src/http/health.rs:34`, `crates/memory-mcp/src/{observability.rs,shared/observability.rs}`.
- Test: `crates/memory-mcp/tests/http_health.rs`; controlled logger/metrics tests beside the owning diagnostics interface.

**Interfaces:**
- Produce serializable `OidcHealth { enabled: bool, metadata: MetadataStatus, jwks: JwksStatus }`; `MetadataStatus` values serialize exactly as the spec table, and `JwksStatus` gains `Disabled` for the health representation.
- Produce `OidcRuntime::health(&self) -> OidcHealth` with no I/O. Add `oidc` to the readiness body; the disabled branch constructs the disabled snapshot without creating a runtime.
- Internal `ProviderDiagnostics::transition(&self, component: ProviderComponent, state: ProviderState, reason: &'static str, status: Option<u16>, retry_after: Duration)` deduplicates state transitions; component/state enums own the finite vocabulary. `ProviderDiagnostics::request_finished(&self, stage: ProviderStage, outcome: ProviderOutcome)` increments only actual upstream calls. Map failures to static reason tags at the adapter boundary.
- Define `ProviderComponent { Metadata, Jwks, Token }`, `ProviderState { Disabled, Initializing, Ready, Backoff, InvalidResponse, Empty, Fresh, Stale }`, `ProviderStage { Discovery, Jwks, Token }`, and `ProviderOutcome { Success, Unavailable, InvalidResponse, Refused }`; serialized/metric values use snake case. Token transition state never changes metadata/JWKS readiness.
- Register `memory_oidc_provider_ready{component="metadata"|"jwks"}` and `memory_oidc_provider_requests_total{stage="discovery"|"jwks"|"token",outcome="success"|"unavailable"|"invalid_response"|"refused"}` through existing metric-description conventions. No issuer/subject/`kid` labels and no provider series for a disabled method.

- [ ] **Step 1: Write readiness and observability regressions.** Assert `status == "ready"` and HTTP 200 while `oidc.metadata == "backoff"`; use separate shutdown/admission/registry-down cases to preserve their existing 503 precedence. Through the controlled diagnostics interface, simulate repeated failed attempts then recovery:

```rust
assert_eq!(transition_warn_count, 1); // repeated same component/state
assert_eq!(recovery_info_count, 1);
assert_eq!(upstream_request_counter, actual_fixture_request_count);
assert!(!rendered.contains("fixture-secret-token"));
assert!(!metric_labels.contains("kid"));
```

Advance the controlled JWKS clock past 300 seconds with no auth traffic and assert its readiness gauge becomes 0. Add disabled-provider coverage proving no worker/network calls and no provider metric series. Use isolated metric capture following the existing test-recorder convention, not process-global recorder replacement.

- [ ] **Step 2: Run red diagnostics coverage.** Run `rtk cargo test -p memory_mcp --features streamable-http,test-fixtures --locked --test http_health`, then `rtk cargo test -p memory_mcp --features streamable-http,test-fixtures --locked --lib control::oidc::diagnostics`. Expected: missing OIDC JSON/metrics or duplicate transition events fail.
- [ ] **Step 3: Wire snapshots and bounded signals.** Use namespace `oidc` already present in `OP_NAMESPACES`; emit `oidc.provider.state_changed`. Degradation transitions are handled-state WARN, recovery INFO, repeated attempts DEBUG. If an actual propagated failure is logged, use its `MemoryError::log_level()` and preserve one-owner logging. A token failure affects counters/logs, not metadata/JWKS readiness. Ensure an expiry timer updates the JWKS gauge even without auth requests; health handlers merely read state.
- [ ] **Step 4: Run green diagnostics and exporter checks.** Repeat Step 2; run `rtk cargo run -p xtask --locked -- check-observability`. Expected: tests pass and the existing observability checks accept the exported families; no dashboard redesign or snapshot claim is needed.
- [ ] **Step 5: Commit only Task 5 paths.** `rtk git commit -m 'feat(oidc): expose provider degradation and recovery'` after exact staging.

### Task 6: Prove the incident lifecycle in the real binary and document it

**Files:**
- Test/Modify: `crates/memory-mcp/tests/http_oidc_resilience.rs`, `crates/memory-mcp/tests/common/oidc_provider.rs`, `crates/memory-mcp/tests/http_background_shutdown.rs`; use existing `tests/common/http_server.rs` unchanged unless adding a fixture-owned graceful-stop operation is necessary.
- Modify: `docs/adr/0057-additive-browser-auth-methods.md:289`, `docs/operations/{LIMITATIONS,LOCAL_ADMIN,LOGGING}.md`, `observability/README.md`.

**Interfaces:**
- Consume the completed runtime, provider fixture, real HTTP binary fixture and metrics/health contract from Tasks 1–5.
- If required by the real-child shutdown case, add `HttpServerFixture::stop_gracefully(&mut self, deadline: Duration) -> std::process::ExitStatus` async. It sends the same graceful signal used by existing HTTP shutdown coverage, bounds/reaps the child, and leaves Drop safe; it is integration-fixture code only. Do not shell out to remote services or mutate the host environment.
- Deliver the documented outage/recovery contract and executed gate results; do not tag, push, deploy or publish a release in this task.

- [ ] **Step 1: Add the complete incident regression.** Add `cold_outage_recovers_and_completes_browser_sign_in` covering one lifecycle: discovery 502 at startup -> health/API-key service -> provider recovery without auth demand -> authorize redirect -> real code exchange using the emitted PKCE challenge and nonce -> valid session -> token-endpoint outage while the established session/API-key/cached-JWT paths continue. Configure fixture-only open signup with `.with_env("MEMORY_MCP_HTTP_SIGNUP_MODE", "open")` and the HTTP fixture's existing explicit plan quotas so a fresh OIDC identity can legitimately obtain its Account; leave production signup policy unchanged. Record the provider's request counts and assert no code POST was replayed. Use independently named companion tests `invalid_issuer_never_publishes_before_valid_recovery`, `blocked_discovery_shutdown_reaps_child`, and `blocked_jwks_shutdown_reaps_child`. Bound child startup, HTTP calls, notifications and shutdown; synchronize with held requests rather than sleeps. Compare the served process identity before/after recovery using a fixture child-id accessor only if needed; never call `restart()` in the recovery test.
- [ ] **Step 2: Run the acceptance tests before any final repairs.** Run `rtk cargo test -p memory_mcp --features streamable-http,test-fixtures --locked --test http_oidc_resilience`. Expected: newly added process-level scenarios reveal any remaining integration gaps; record the actual failure if one occurs rather than manufacturing an unnecessary red test after completed behavior.
- [ ] **Step 3: Complete lifecycle fixes and operations documentation.** Repair only failures exposed by the acceptance scenarios. Amend ADR-0057's startup-fatal annotation with a dated description and a link to this spec, preserving its historical explanation. Document 401 versus 503, retry/TTL limits, independent readiness, fixed discovery for process lifetime, sign-in restart after code-exchange timeout, and the exact new logs/metrics. Explicitly retain the limitation that a cold process without keys cannot validate OIDC tokens and an outage beyond 300 seconds expires cached verification material. Describe enabling local auth as an explicit prior operator choice, never an automatic outage response.
- [ ] **Step 4: Run final gates once, after focused tests pass.** Run the following sequentially; expected outcome is exit 0, no formatting diff and no clippy warnings:

```bash
rtk cargo test -p memory_mcp --features streamable-http,test-fixtures --locked --test oidc_provider_transport
rtk cargo test -p memory_mcp --features streamable-http,test-fixtures --locked --test oidc_jwks
rtk cargo test -p memory_mcp --features streamable-http,test-fixtures --locked --test http_oidc_resilience
rtk cargo test -p memory_mcp --features streamable-http,test-fixtures --locked --test http_health
rtk cargo test -p memory_mcp --features streamable-http,test-fixtures --locked --test http_background_shutdown
rtk cargo test -p memory_mcp --features streamable-http,test-fixtures --locked
rtk cargo check -p memory_mcp --no-default-features --features streamable-http --locked
rtk cargo check -p memory_mcp --locked
rtk cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
rtk cargo fmt --all --check
rtk cargo run -p xtask --locked -- check-observability
rtk git diff --check
```

Do not repeat a passed focused command immediately if it was already run against the same final tree. If the production-feature clippy excludes a new test-fixtures-only path, additionally check that path with `rtk cargo clippy -p memory_mcp --all-targets --features streamable-http,test-fixtures --locked -- -D warnings`. Pre-existing workflow changes stay unstaged; distinguish environmental/build failures from feature failures. Do not claim Docker-packaged or hosted CI acceptance from local Rust tests alone.

- [ ] **Step 5: Review the complete feature diff and commit the acceptance/docs milestone.** Check the spec's trust/availability table against the actual assertions and review startup, HTTP rejection, key removal, cancellation and log redaction paths. Follow the user's selected execution/review method. Stage exact Task 6 files and commit `rtk git commit -m 'test(oidc): verify provider outage and recovery lifecycle'`. Include the two planning documents in the first feature commit if they were not committed separately; never stage all workspace changes blindly.

## Planning self-review and handoff

**Exact staging rule for every task:** Run `rtk git add --` with the literal paths in that task's Files list, expanding brace notation and removing line anchors; include earlier-task files only when this task explicitly lists them. Task 1 additionally stages `docs/superpowers/specs/2026-10-16-oidc-provider-resilience.md` and `docs/superpowers/plans/2026-10-16-oidc-provider-resilience.md`. Task 6 stages `tests/common/http_server.rs` only if its optional graceful-stop helper changed (with the `crates/memory-mcp/` prefix). Inspect `rtk git diff --cached --stat` before the stated commit; no `.github/workflows/` path may appear. No directory-wide or wildcard staging.

Completed during preparation: mapped every spec requirement to a task; checked cross-task names/signatures; assigned the five Review Focus inputs to concrete tests; kept numeric policies in the spec; excluded persistent caches, stale-key grace, periodic rediscovery, UI redesign and releases. The remaining implementation choices are local Rust bodies and test-fixture mechanics, not trust or availability policy. This plan does not assert that any proposed production test has run or passed.

Recommended execution: **Subagent-driven**, because the six sequential deliverables cross startup ownership, key trust and HTTP refusal semantics, and fresh per-task review can catch an authorization regression before the next interface is consumed. Native execution remains a valid lower-cost choice. Wait for the user's plan review and execution-method selection before touching production code, as required by `writing-plans`.
