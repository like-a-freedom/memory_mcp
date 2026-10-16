# OIDC provider resilience

**Status:** Proposed implementation contract, distilled from the incident analysis and the user's request to prepare a plan. No implementation or execution method has been approved by this document.

## Problem and evidence

On 2026-10-16 the deployed server received HTTP 502 from OIDC discovery and repeatedly exited during HTTP startup. In the inspected checkout, `control/oidc/client.rs::OidcClient::new` classifies discovery transport/status failures as `MemoryError::ConfigInvalid`; `http.rs::HttpState::assemble_with_deployment_policy` propagates them; `http/runtime/bootstrap.rs::build_state` returns exit code 2. ADR-0057 already records the startup dependency as a limitation.

The existing JWKS cache has a 300-second TTL. Its mutex serializes refreshes but does not share unsuccessful outcomes or limit unknown-key refresh frequency. The browser callback maps provider failures to 503, whereas the MCP OAuth path collapses them into an authentication refusal and returns 401. API-key verification and durable browser-session resolution have no intrinsic discovery dependency.

## Goal and scope

An unavailable identity provider must not prevent the HTTP service from starting or stop independent authentication paths. OIDC must recover automatically in the same process, while every authorization decision continues to require its normal evidence.

This is one feature spanning the existing OIDC adapter and HTTP composition, not separate authentication products. It includes bounded provider I/O, discovery recovery, JWKS refresh control, HTTP error semantics, observability, shutdown ownership and regression coverage.

Deferred: persistent discovery/JWKS storage, accepting keys beyond their TTL, periodic discovery-document replacement after successful initialization, new login methods, UI redesign, new MCP tools, dependency changes, migrations, deployment and release publication. Successful discovery is retained for the process lifetime; a changed issuer or endpoint configuration requires a restart. JWKS continues to refresh independently.

## Global constraints

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

## Availability and trust contract

| Condition | Required behavior |
| --- | --- |
| Discovery unavailable at cold start | Bind HTTP without waiting for discovery; background recovery starts immediately. |
| OIDC configured but discovery not validated | Keep OIDC routes mounted; operations needing metadata return 503 and `Retry-After`. |
| Invalid local configuration | Fail startup before spawning an OIDC worker. |
| Remote metadata invalid, missing fields, wrong issuer, or unusable algorithms | Never publish it; keep the server running, report `invalid_response`, and retry at the slow interval. |
| Valid API key, valid local login or existing valid browser session | Continue independent of provider availability; retain all existing authorization checks. |
| JWT with a matching key younger than 300 seconds | Verify locally; unrelated discovery/token-endpoint failures do not block it. |
| No usable fresh key and JWKS unavailable | Refuse authentication with 503, not 401. |
| Successful usable JWKS response lacks the requested `kid` | Refuse with 401; do not label this a provider outage. |
| Invalid signature/issuer/audience/expiry/algorithm with verification material available | Refuse with 401. |
| Successful JWKS replacement removes a key, including a valid empty key set | Stop using that key immediately; never union old keys into the replacement. |
| Provider restored | Initialize automatically without restart or an incoming user request. |
| Shutdown during retry sleep, blocked fetch or refresh | Cancel and join the owned worker within the existing cleanup deadline. |

`None` in the HTTP state's OIDC field means the method is disabled. A configured method has a runtime handle even while it has no discovered client. Readiness of metadata and JWKS are separate; token-endpoint health must not become a gate on offline JWT verification.

Preserve the existing configured-versus-published trailing-slash compatibility rule. After discovery, ID-token validation and identity indexing use the provider's published issuer exactly. The access-token path must consume the same resolved issuer and accepted algorithm policy, with its own existing access-token claims and resource audience. Do not derive endpoints from JWT headers or merge identities by email.

## Provider calls and retry policy

All discovery, JWKS and token calls use an explicitly configured reusable `reqwest::Client`: connect timeout 2 seconds, total request timeout 5 seconds, redirects disabled. Bound each decoded response body to 262,144 bytes while reading; a Content-Length check alone is insufficient. Do not log token-response bodies or raw reqwest errors that might carry request data.

Discovery and JWKS GET failures are handled by one retry owner per operation. The first attempt is immediate. For transport errors, timeouts, HTTP 429 and HTTP 5xx, use retry index `n = 0, 1, ...` with a ceiling of `min(2^n, 60)` seconds and equal jitter in `[ceiling / 2, ceiling]`, with a minimum delay of 1 second. Saturate the exponent. For HTTP 429 or 503, honor a valid upstream `Retry-After` (delta seconds or HTTP date) as a lower bound, capped at 300 seconds; absent, malformed or past values use ordinary backoff. Other non-success HTTP responses and invalid metadata/body responses use a 60-second interval. Reset the failure streak after success.

JWKS has a provider-wide minimum interval of 1 second between demand-triggered fetches, independent of `kid`. Concurrent callers share the same completed refresh generation and outcome, including failure. During cooldown, return the cached outcome immediately; never queue another network attempt for each caller. Fresh known keys remain usable during cooldown. Every fetch path, including the public `refresh` method and background upkeep, passes through the same gate.

Cache keys for a maximum of 300 seconds measured from the last successful complete fetch using monotonic time. Schedule proactive refresh at a jittered age of 240–270 seconds. An unsuccessful refresh neither clears keys nor changes their successful-fetch timestamp. Never serve a cached key at age >= 300 seconds. A successful replacement, even an empty set, updates both the generation and successful-fetch timestamp; transport/JSON errors do neither. Preserve the current accepted key types and 32-key bound rather than widening algorithm policy.

Discovery publishes a validated `OidcClient` before JWKS warmup, so a JWKS outage does not erase known metadata. Its owner immediately attempts JWKS warmup and then drives proactive refresh/recovery. Request-triggered unknown-key refresh and background work share the same gate. A token endpoint failure affects that code exchange only.

An authorization-code POST is attempted once. A timeout may occur after the provider consumed the code, so do not automatically replay it. A provider outage returns 503 and requires a new browser sign-in flow; invalid/used code is a 401 refusal. Inspect only the bounded OAuth `error` code needed for this classification; never expose or log `error_description`, tokens or codes.

## HTTP and operations

Keep `/health/live` unchanged. Keep the existing `/health/ready` HTTP status and `status` field governed by shutdown, admission and registry connectivity, not OIDC. Add a bounded `oidc` object to readiness JSON: `enabled: bool`, `metadata: "disabled" | "initializing" | "ready" | "backoff" | "invalid_response"`, and `jwks: "disabled" | "empty" | "fresh" | "stale"`. Do not expose issuer URLs, `kid`, credentials or failure text there. No network calls are allowed from health handlers for OIDC status.

503 authentication responses use the existing `temporarily_unavailable` control-plane error code where an envelope already exists, `Cache-Control: no-store`, and a positive integer `Retry-After` rounded up from the actual cooldown; use 1 when no cooldown is known. Retain generic 401 responses and existing Bearer challenges. In the MCP path preserve `Unavailable` versus credential refusal through the resolver and middleware rather than using `Option` for both. A provider error must not turn malformed credentials into a successful principal.

Use `oidc.provider.state_changed` for component transitions: handled degradation is WARN, recovery/initialization state changes INFO. Record one transition per component/state, not one WARN per retry. Repeated attempts are DEBUG. Propagated `MemoryError` failures use `MemoryError::log_level()`; do not weaken existing logging policy. A logical event is logged once by its owner. Allowed bounded fields: component, state, reason, status, retry_after_secs, attempt. Never put issuer, `kid`, request codes, subjects, tokens or raw content into metric labels.

Expose `memory_oidc_provider_ready{component="metadata"|"jwks"}` as a 0/1 gauge and `memory_oidc_provider_requests_total{stage="discovery"|"jwks"|"token",outcome="success"|"unavailable"|"invalid_response"|"refused"}` as actual upstream request counts. An enabled but cold component reports zero; a disabled provider produces no provider series. JWKS readiness reflects expiry even without incoming auth traffic. Reuse the current metrics facade and exporter; no additional listener or dashboard redesign.

## Acceptance and review-sensitive conditions

1. Real HTTP binary starts against discovery 502, serves a valid API-key MCP request, and returns a bounded OIDC 503; after the provider recovers, the same child process completes a real code/PKCE/nonce sign-in.
2. A failing refresh with a cohort of waiting callers and a stream of different unknown `kid` values produces one upstream attempt per allowed interval, not per caller.
3. Empty replacement keys, TTL boundary and failure after a successful warmup never resurrect an expired or removed key.
4. Invalid metadata, redirects, oversized/chunked bodies and malformed `Retry-After` neither publish untrusted state nor create tight retry loops.
5. Shutdown while a fetch is blocked cancels/reaps the worker and still attempts cleanup of the other background owners.

Use controlled time/permits/future polling for ordering. Unit tests exercise owning public interfaces with controlled adapters; loopback HTTP, storage and subprocess scenarios are integration tests. Bound every wait and child lifetime. No document-inventory tests or tests that only mirror constants.

## Source rationale

- [Spring Security resource-server startup](https://docs.spring.io/spring-security/reference/servlet/oauth2/resource-server/jwt.html): defer discovery to remove authorization-server availability from startup.
- [Auth0 JWKS guidance](https://auth0.com/docs/secure/tokens/json-web-tokens/json-web-key-sets): bounded caching, rotation-triggered refresh and limiting unknown-key fetches.
- [AWS retry guidance](https://aws.amazon.com/builders-library/timeouts-retries-and-backoff-with-jitter/): explicit timeouts, backoff/jitter and avoiding unsafe replay of side effects.
- [OIDC Discovery §4.3](https://openid.net/specs/openid-connect-discovery-1_0.html#ProviderConfigurationValidation): failed validation cannot authorize use of provider metadata.
- [Kubernetes probe semantics](https://kubernetes.io/docs/concepts/workloads/pods/probes/): liveness controls restarts and readiness controls traffic admission; OIDC component diagnostics must preserve independent service paths.

Numeric limits above are implementation choices for this feature, not requirements claimed to come from those standards.
