# Streamable HTTP SaaS design specification

**Status:** Approved design, 2026-08-27
**Decision:** [ADR-0052](../../adr/0052-streamable-http-saas-profile.md)
**Protocol target:** MCP `2026-07-28`
**SDK baseline:** `rmcp` 3.1.2
**Implementation status:** Not implemented

## 1. Purpose and non-goals

This specification adds a public multi-user Streamable HTTP deployment profile
without changing the existing local stdio profile. It defines transport,
authentication, tenancy, persistence, scheduling, browser control plane,
operational behavior, and release gates.

V1 does not provide:

- legacy MCP sessions or pre-`2026-07-28` HTTP compatibility;
- request-selected namespaces;
- shared memory between Accounts;
- filesystem or URL ingestion in the SaaS profile;
- user export, per-Tenant restore, or application-managed backup;
- MRTR, sampling, elicitation, or roots;
- billing-grade metering;
- application-level encryption of memory content;
- application-enforced production egress restrictions;
- a distributed quota backend beyond SurrealDB durable counters;
- hot reload of environment configuration or secrets.

## 2. Deployable units and feature boundaries

### 2.1 Binaries

| Binary | Purpose | Storage selection | Public operations |
|---|---|---|---|
| `memory_mcp` | Existing CLI and stdio MCP | One startup-bound Active Namespace | Existing CLI commands and eight MCP tools |
| `memory_mcp_http` | SaaS HTTP process | Authenticated Tenant Runtime | `/mcp`, optional control plane/UI, health, metrics |

`memory_mcp_http` has no ingest/extract/reembed/admin CLI commands. Operator
automation uses the protected control-plane API. A future admin CLI may be a thin
HTTP client, never a direct production-DB client.

### 2.2 Cargo features

Planned additive features:

- `streamable-http`: HTTP binary, Axum/Tower integration, and the corresponding
  `rmcp` Streamable HTTP server feature;
- `control-plane`: OIDC, browser sessions, `/api/v1`, and provisioning routes;
- `control-plane-ui`: Dioxus web/WASM crate and embedded/served assets;
- existing `mcp-apps`: MCP App resources and App Session behavior;
- existing `prometheus`: metrics route/recorder.

The exact dependency graph is an implementation-plan decision. Package defaults
remain `[]`. No dependency or `Cargo.toml` change is authorized by this document.

### 2.3 Dioxus boundary

The Dioxus crate is web-only. It owns components, client routing, browser API
DTO consumption, and static assets. It does not use Dioxus server functions. Axum
remains the single backend router and serves the built SPA after higher-priority
API routes.

## 3. HTTP topology

### 3.1 Routes

| Route | Authentication | Response/use |
|---|---|---|
| `POST /mcp` | Bearer API key; later OAuth token | MCP JSON or request-scoped SSE response |
| `GET/DELETE /mcp` | None needed before method rejection | `405` |
| `/api/v1/account/*` | Control Plane Session + CSRF for mutation | Account and key management |
| `/api/v1/operator/*` | Operator OIDC principal + CSRF/recent-auth | Provisioning retry, suspension, purge, recovery status |
| `/auth/oidc/*` | OIDC flow state | Login/callback/logout |
| `/` and SPA fallback | Public static assets | Dioxus application |
| `/health/live` | Public | Minimal process status |
| `/health/ready` | Public | Minimal admission status |
| `/metrics` | No app auth | Expected to be proxy/network restricted |

MCP Bearer authentication never accepts browser cookie authentication. Browser
control-plane handlers never accept an Account API Key in place of a cookie.

### 3.2 Proxy contract

Production terminates TLS at a reverse proxy/load balancer. The backend defaults
to `0.0.0.0:8080`. The proxy must:

- preserve streaming and disable response buffering for `/mcp`;
- use a read timeout greater than the 120-second ordinary request deadline and a separately configured idle/liveness policy for long-lived `subscriptions/listen`; the policy must allow rmcp keep-alives and must not impose the ordinary call deadline on that stream;
- pass only normalized trusted forwarding information;
- restrict `/metrics` from public access;
- enforce coarse invalid-auth/IP rate limits;
- not rewrite MCP mirrored headers;
- not expose the backend port directly.

SSE responses set `Content-Type: text/event-stream`, `Cache-Control: no-cache`,
and `X-Accel-Buffering: no`. Compression is disabled unless an integration test
proves correct incremental delivery and cancellation.

The ordinary request deadline applies to handler execution and to polling the
returned response body; dropping or timing out a body releases request admission
and any operation/runtime guard. `subscriptions/listen` is the exception: it is a
long-lived POST response stream and is not subject to the ordinary 120-second
body deadline. Its lifetime is bounded by client cancellation, server shutdown,
authorization recheck, and the separately configured proxy idle/liveness policy.
The runtime/admission lease for a streaming response is owned by the body, not
only by the handler future or request extensions.

### 3.3 Host and Origin

Production startup requires explicit allowed hosts and allowed origins. Wildcard
origins are rejected. Missing Origin is accepted for non-browser MCP clients.
Present Origin must be allowlisted. Forwarded host/origin are trusted only under
explicit trusted-proxy configuration.

## 4. MCP `2026-07-28` profile

### 4.1 Discovery and metadata

The HTTP profile is modern-only. It does not implement protocol-level sessions,
`initialize`, `notifications/initialized`, `ping`, standalone GET streaming,
DELETE session termination, `Mcp-Session-Id`, `Last-Event-ID`, or legacy
`resources/subscribe`/`resources/unsubscribe` methods. Legacy-only clients receive
the stable unsupported-version response rather than an implicit compatibility mode.

`server/discover` advertises only:

- protocol version `2026-07-28`;
- the frozen tool surface;
- resources/Apps only when enabled;
- App/resource subscriptions when enabled;
- Tasks for extraction;
- no MRTR, roots, sampling, elicitation, prompts-change, or tool-list-change
  capability.

Every modern result uses the required result envelope. Ordinary results use
`resultType: "complete"`. The server never returns `input_required`.

For every POST, `Mcp-Method` is required and must match the JSON-RPC method. The
`MCP-Protocol-Version` header is required and must match
`_meta.io.modelcontextprotocol/protocolVersion`. `Mcp-Name` is required for
`tools/call`, `resources/read`, and `prompts/get`; it is not required for other
methods. These mirrored headers are validated before any routing or authorization
decision depends on them.

A successful request may return either the standard JSON response or an SSE
response according to the protocol response rules. Notifications return HTTP
`202` with no response body.

### 4.2 Validation order

The request pipeline is:

1. path and HTTP method;
2. Host/Origin checks;
3. content type, Accept, and compressed/decompressed body limits;
4. JSON syntax and JSON-RPC envelope;
5. modern protocol metadata and supported version;
6. `Mcp-Method`, `Mcp-Name`, and body/header consistency;
7. credential extraction and authentication;
8. Account/Tenant state and authorization;
9. edge/account admission and rate limiting;
10. Tenant Runtime acquisition;
11. handler dispatch.

Infrastructure may route or limit on mirrored headers only after consistency has
been verified at the trust boundary. Mismatch returns HTTP 400 and the modern
`HeaderMismatch` JSON-RPC error.

### 4.3 HTTP/MCP error boundary

Before protocol admission:

- 400: malformed request, missing/mismatched modern metadata;
- 401: missing or invalid credential, with appropriate challenge metadata;
- 403: authenticated but suspended/forbidden principal;
- 405: unsupported method;
- 406: unacceptable response media types where applicable;
- 413: body too large;
- 415: unsupported content type;
- 408: ordinary request handler deadline exceeded before a response is produced;
- 429: edge/account rate limit or an explicitly exceeded quota;
- 503: temporary server overload (including runtime/admission capacity), registry,
  schema, or dependency unavailability. Capacity overload is not reported as a
  durable quota decision.

After admission, protocol and tool errors use MCP/JSON-RPC and established
`ToolResponse` guidance. External messages do not expose namespaces, SQL,
migrations, identity claims, or dependency errors. They include a correlation ID.

### 4.4 Request cancellation and retries

Closing a request SSE stream cancels request-owned work. It does not undo a
committed episode, fact, Task, App command, or durable job. There is no stream
resume or event replay.

No nonstandard `Idempotency-Key` is required. Operations define domain retry
semantics:

- `ingest`: duplicate source/content returns the existing episode;
- `extract`: durable Tasks fingerprint/reconcile work; synchronous retries must
  not create duplicate facts/claims;
- `resolve`: equivalent retry converges on the same entity/alias state;
- `invalidate`: repeated invalidation returns the established outcome;
- `open_app`: a lost response may leave a bounded orphan session removed by TTL;
- `app_command`: optimistic session versioning prevents silent lost updates and
  repeated commands must have deterministic or conflict semantics.

## 5. Identity and credential model

### 5.1 Records

- `ExternalIdentity { id, issuer, subject_verifier, account_id, created_at }`
- `Account { id, status, tenant_id, created_at }`
- `Tenant { id, status, namespace_binding, plan_version, schema_version,
  provisioning_lease }`
- `ApiKey { id, account_id, name, verifier, status, expires_at, last_used_at }`
- `ControlPlaneSession { verifier, account_id, auth_time, idle_expiry,
  absolute_expiry }`

Identifiers are independent opaque sortable IDs. V1 enforces a one-to-one
relationship with uniqueness on both `Account.tenant_id` and Tenant ownership;
the IDs and namespace binding do not encode that cardinality. Raw OIDC `subject` is never persisted or logged. Identity lookup stores a
keyed blind index (`subject_verifier`) derived from the normalized issuer and
subject with a dedicated server-side key; the raw subject exists only in the
transient validated request context. Email and display name are optional
minimized attributes, never identity keys. If retained for display, email is
encrypted; a keyed blind index is used only if exact lookup is required.

### 5.2 API keys

Conceptual format:

```text
mem_sk_<key_id>_<256-bit-random-secret>
```

The parser has a strict maximum length and grammar. Lookup uses the public key ID.
The secret is verified with a keyed HMAC/verifier and constant-time comparison.
Unknown IDs and invalid secrets have indistinguishable external responses. Failed
auth is rate-limited before expensive work.

Default policy:

- maximum ten active keys;
- default one-year expiry;
- optional shorter or no expiry;
- named keys and independent revoke;
- full eight-tool access in v1;
- secret shown once and never recoverable;
- positive auth cache at most 60 seconds and every cache hit still verifies the
  presented secret against the cached key record;
- negative cache substantially shorter;
- revocation is durable immediately and externally effective within at most 60
  seconds unless a deployment adds push invalidation;
- stale/unknown authentication fails closed.

### 5.3 OIDC control-plane login

When the control plane is enabled, use Authorization Code with PKCE, exact
issuer/audience validation, nonce/state, an algorithm allowlist, and encrypted
server-side storage for transient OIDC flow material. OIDC is optional at
startup when the control plane is disabled; enabling the control plane without
its required OIDC configuration fails startup. A successful first login creates
an idempotent Account provisioning request when signup policy permits. Default
signup is invite-only.

The browser receives only an opaque Secure, HttpOnly, SameSite cookie. Server-side
sessions have idle and absolute expiry and rotate after login. Credential creation,
identity linking, and deletion require authentication no older than ten minutes.

Operator principals come from an immutable `(issuer, subject)` allowlist or a
trusted role claim from the configured issuer/audience. Account APIs cannot grant
operator status. `/api/v1/operator/*` uses this principal and exposes only
provisioning inspection/retry, suspend/resume, purge initiation/status, and
recovery status. It shares the public listener but requires OIDC, CSRF, recent-auth
for destructive actions, and audit recording while the target Account exists.

### 5.4 Future MCP OAuth

Memory MCP acts as a Resource Server and publishes Protected Resource Metadata
with the RFC 9207 `iss` metadata when required by the deployment. It validates
exact issuer, resource/audience, expiry, accepted algorithms, and Account status.
OIDC audience decoding accepts both the JWT string and array forms. JWKS caching
is bounded; an unknown key triggers at most one bounded single-flight refresh,
and refresh failure fails closed without blocking the async runtime. Identity
Identity resolution uses `(issuer, subject_verifier)` derived from the verified
raw claims; matching email never links Accounts. The identity-index key is
rotated as a credential event and invalidates the ability to resolve restored
OIDC identities until the mapping is rebuilt.

## 6. Tenant Registry and provisioning

### 6.1 Registry boundary

The registry is bound to a control namespace/database separate from Tenant
Namespaces. It stores no memory content, App payload, Task result, or tenant
change body. Ordinary MCP tools cannot query it.

### 6.2 Provisioning state machine

```text
reserved -> namespace_creating -> migrating -> ready <-> suspended
    |              |                 |          |          |
    +--------------+-----------------+----------+----------+-> deleting -> purged

reserved | namespace_creating | migrating -> failed
failed -> the recorded retryable prior stage | deleting
```

Signup/provisioning workers drive creation states; operators may suspend/resume,
retry a failed stage, or begin deletion for any non-purged Tenant. Deletion
preempts further provisioning and maintenance. A worker claims a stage with an
atomic compare-and-set lease containing owner, lease ID, expiry, heartbeat, and
monotonically increasing fencing generation. Heartbeats use jitter; every state
transition and durable worker commit verifies the current owner/lease/generation,
and release is conditional so a stale worker cannot release a newer lease.
Provisioning is idempotent.

The namespace binding is created once, is immutable, and is never reused. A
reconciler identifies registry records without namespaces and orphan namespaces
without active registry bindings. Data plane admits only `ready`. The terminal
`purged` state means that live data-plane access is permanently denied; it does
not authorize physical deletion of retained memory or audit-bearing records.

### 6.3 Tenant state mapping

- `reserved`, `namespace_creating`, `migrating`: temporary unavailable with retry
  guidance;
- `suspended`: forbidden with owner/support guidance;
- `failed`: unavailable with correlation ID;
- `deleting`, `purged`: unauthorized/not found without storage disclosure;
- unknown credential and unknown Account remain indistinguishable.

## 7. Tenant Runtime pool

### 7.1 Contents

Process-global and safe to share:

- immutable model weights/tokenizers;
- provider HTTP connection pools;
- static configuration;
- telemetry handles;
- global admission controllers;
- schedulers.

Tenant-bound and never shared without a Tenant key:

- clone-once namespace/database session;
- capabilities and stores;
- authorization-derived Tenant identity;
- App/Task/change-log stores;
- quota counters and tenant concurrency limiter;
- tenant-keyed caches.

Retrieval results, App state, authorization decisions beyond the bounded auth
cache, and namespace-bound stores never cross Tenant keys.

### 7.2 State and guards

```text
absent -> loading -> ready -> draining -> unloaded
                    |           |
                    +-> failed <-+
```

Single-flight activation is keyed by immutable Tenant ID. Activation has a
30-second timeout, globally bounded concurrency, and bounded negative backoff.
Every operation holds a guard. A replica has at most one nonterminal runtime per
Tenant ID. LRU/idle eviction marks only unpinned runtimes `draining`; new
acquisition waits until that runtime is unloaded and may then activate one
replacement. Drain completion closes tenant-bound clients and removes the cache
entry.

Long-lived subscriptions do not hold a full runtime guard. They retain an
authorized minimal change-stream context and periodically recheck Account/key
validity no less frequently than every 30 seconds and always within the
60-second authorization bound. Ordinary requests and subscriptions use separate
admission budgets; a subscription cannot exhaust ordinary request capacity.

The response body owns the request admission permit and, for ordinary requests,
the operation/runtime guard until the body completes, errors, or is dropped. The
handler future returning is not sufficient to release these resources because an
SSE body may outlive the handler future.

### 7.3 Capacity defaults

- 32 active runtimes;
- 15-minute idle TTL;
- two-second bounded capacity wait;
- four concurrent requests per Tenant;
- bounded global request queue;
- separate budgets for activation, extraction, maintenance, and long-lived
  subscriptions;
- subscription capacity is bounded independently from ordinary request capacity.

When no evictable runtime appears before the deadline, admission returns temporary
overload. The pool never exceeds configured capacity.

## 8. Durable workers and schema rollout

Process-level schedulers discover due work through the registry, acquire
datastore-time leases, and run bounded Tenant passes. Lease records contain
owner replica, unique lease ID, expiry, heartbeat, and monotonically increasing
fencing generation. Every durable commit performed by a leased worker verifies
its current owner/lease/generation. Ordinary request writes use their own
transaction/version invariants and do not require a worker lease. Heartbeats use
jitter, and lease release is conditional on the same fence. Lease expiry does
not alone prove an operation is safe to repeat; each job defines reconciliation.

The tracked scheduler set includes provisioning, schema migration, runtime idle
eviction, App Session cleanup, Task retry/reconciliation/retention, quota
reconciliation, and subscription/outbox repair. These jobs are bounded,
observable, and joined during shutdown; correctness-sensitive work is not left
in detached per-Tenant loops.

No per-Tenant eternal worker loop is created on runtime activation. Inactive
Tenants may be temporarily activated for due maintenance and unloaded afterward.

Each replica advertises minimum and maximum supported schema versions. Control
plane rolls migrations with bounded concurrency. A Tenant outside a replica's
range is not activated. Migration steps and their ledger transitions are
idempotent and verify postconditions, consistent with ADR-0011 and ADR-0038.

## 9. App Sessions

HTTP App Sessions are versioned records in the Tenant Namespace. `open_app`
creates an opaque handle; `app_command` loads by Tenant and handle and commits only
when the expected version is unchanged. Conflict returns retry guidance and never
uses last-write-wins.

Defaults:

- 30-minute idle TTL;
- 24-hour absolute TTL;
- maximum 32 open sessions per Tenant;
- explicit close;
- scheduler cleanup of expired records.

Resource routes and MCP reads reauthorize Tenant ownership. App resources never
contain API keys or control-plane credentials. App CSP is distinct from the
Dioxus control-plane CSP.

## 10. Tenant Tasks

### 10.1 Scope

Only `extract` may return a Task. `ingest` always synchronously commits the episode
and returns its result. A client that did not advertise Tasks receives bounded
synchronous extract or a preflight size/complexity rejection.

### 10.2 State

```text
queued -> running -> completed | completed_before_cancel
   |         |
   |         +-> cancel_requested -> cancelled_before_commit
   |         +-> failed
   |         +-> queued (retry after fenced lease expiry, within retry policy)
   +-> cancelled
   +-> failed
```

`completed`, `completed_before_cancel`, `cancelled`,
`cancelled_before_commit`, and `failed` are terminal. `cancel_requested` is an
internal nonterminal intent. A queued cancellation becomes `cancelled`; a running
cancellation becomes `cancel_requested`. Lease takeover uses CAS on state/version
and a higher fencing generation. A retryable crash returns `running` to `queued`;
exhausted retry policy becomes `failed`. Reconciliation selects the terminal
outcome when extraction artifacts and Task state were not committed atomically.

A Task stores input fingerprint, state/version, lease owner/generation/expiry,
cancellation intent, bounded progress, terminal result/error, timestamps, and
retention expiry. Task protocol methods always resolve Authenticated Principal
and Tenant before lookup.

A fenced worker cannot commit after takeover. The implementation must define the
atomicity boundary between extraction writes and terminal Task state. If one DB
transaction cannot contain both, a reconciler derives terminal state from durable
artifacts and the input fingerprint. Cancellation is intent, not rollback.

Task retention is a Memory MCP policy, not `rmcp::TaskManager`'s process-local
default. The configured retention is bounded and explicit; the retention worker
may physically remove only expired ephemeral Task rows. Task artifacts that
represent memory facts follow the memory invalidation rules and are not removed
by Task retention.

## 11. Subscriptions

`subscriptions/listen` accepts only supported App/resource filters through
rmcp's native `ServerHandler::accepted_subscription_filter` and
`ServerHandler::listen(SubscriptionContext)` hooks. It is not implemented as a
second custom JSON-RPC route. Tool and prompt list changes are rejected/not
advertised.

Every canonical resource mutation commits a `TenantChangeEvent` in the same
transaction:

```text
{ sequence, resource_id, revision, change_kind, created_at }
```

The event carries no full resource body. Connected listeners read ordered events
and emit invalidations tagged by subscription identity. A wake mechanism may use
SurrealDB live queries or another broker, but wake loss is repaired by polling the
durable outbox.

Queues and event retention are bounded. Slow consumers are closed. Intermediate
changes may be coalesced by resource/revision. On reconnect the client rereads
canonical resources; the server does not promise `Last-Event-ID` replay.

Authorization is rechecked no less frequently than every 30 seconds and always
within the 60-second auth bound. Revocation is therefore durable immediately but
may take up to 60 seconds to terminate a stream. Suspended, deleting, or purged
Accounts terminate under the same bound. Graceful shutdown terminates streams
after drain.

## 12. Quotas and rate limits

The registry assigns a versioned plan. V1 may have one plan but does not hardcode
commercial values in capabilities. It contains at least:

- cumulative ingested source bytes;
- episode count;
- open App Sessions;
- active API keys;
- per-Tenant request concurrency/rate;
- extraction concurrency.

Writes update durable usage counters transactionally where possible. A periodic
reconciler corrects drift. Complex multi-step work may have bounded documented
overshoot; quota is abuse protection, not billing-grade accounting. Exceeding
write quota does not disable retrieval.

`signup_mode=open` fails startup unless all required quota values are explicit.
Public launch additionally requires storage-cost and extraction-amplification
evidence.

## 13. Ingestion and provider privacy

HTTP SaaS accepts only inline content. Before content preparation it rejects
strings classified as local paths, directories, `file:` URLs, or remote URLs.
No tenant-triggerable generic HTTP client exists. `fs-watch` configuration is a
fatal HTTP startup error.

Embedding/entity providers are deployment-level policy. A remote provider uses
operator credentials and receives only required content, never Account/Tenant
identity unless technically necessary. Product privacy documentation discloses
remote processing. Tenant-provided provider credentials and per-Tenant opt-out are
not v1 features.

The application does not require production egress allowlisting. Operators may
add network policy as defense in depth. This residual risk is documented.

## 14. Control-plane API and SPA

The same-origin Dioxus SPA supports:

- OIDC login/logout;
- provisioning status;
- create/list/revoke API keys;
- one-time key display/copy;
- expiry and last-used display;
- External Identity linking;
- irreversible live Account deletion/access revocation; the UI must explain the
  retained logical/audit data policy and absence of export/recovery.

State-changing `/api/v1` calls require cookie authentication, CSRF, Origin checks,
and recent-auth where specified. Responses use a typed error envelope and
correlation ID. Security-sensitive responses use `Cache-Control: no-store`.

Account deletion flow:

1. recent OIDC reauthentication;
2. display that no export/recovery is available;
3. typed phrase confirmation;
4. short-lived one-use confirmation token bound to Account and session;
5. durable credential/session revocation, effective across cached requests within
   at most 60 seconds;
6. idempotent deletion workflow that retains a durable tombstone, transitions
   the Account and Tenant to terminal deletion states, permanently denies
   data-plane access, invalidates retained memory records according to the
   memory domain policy, and removes only expired ephemeral Task/App Session
   rows. Account, Tenant, identity, credential, lease, provisioning, namespace
   binding, and audit-bearing registry history remain non-reusable durable
   records; no namespace is rebound to another Account.

There is no cancellation window.

Browser baseline includes CSP without `unsafe-eval`, no external scripts by
default, `frame-ancestors 'none'`, `object-src 'none'`, restrictive `connect-src`,
`base-uri 'none'`, `form-action 'self'`, no-sniff, and strict referrer policy.

## 15. Data protection, deletion, and recovery

Authentication secrets use irreversible or encrypted storage appropriate to their
role. OIDC state/nonce material and browser/session flow material that must be
stored server-side are encrypted and authenticated with an approved AEAD (the
implementation plan selects `chacha20poly1305`). Memory content, entities,
facts, embeddings, App payload, and Task results do not receive application-level
field encryption; TLS and storage/backup encryption are deployment
responsibilities.

For Account deletion, live API-key/session credentials are revoked, optional
encrypted identity attributes are redacted or made undecryptable according to
their storage policy, and the Account/Tenant/audit tombstone remains durable.
Memory and audit-bearing records are invalidated/retained rather than physically
deleted. Only ephemeral Task/App Session rows may be physically removed after
their declared retention/TTL window.

Infrastructure secrets come from environment variables. Rotation uses rolling
restart and may support old/new overlap for normal rotation. No hot reload or
secret-manager abstraction is required.

Live-system purge is a logical, access-revoking terminal transition. It does not
edit immutable historical backups and it does not physically delete memory or
audit-bearing records. The deleted Account/Tenant tombstone remains durable so
that the namespace binding cannot be reused and stale credentials cannot be
re-associated. Ephemeral Task/App Session rows may be removed only by their
normal retention/TTL jobs.

Memory MCP has no erasure ledger. Consequently:

- physical erasure from backups is bounded by provider retention, not purge time;
- a historical disaster restore may resurrect data from a logically purged
  Tenant;
- the application cannot identify every resurrected Tenant without information
  outside that snapshot;
- recovery is an operator procedure, not a user restore promise.

Before restored infrastructure receives traffic, rotate:

- API-key verifier pepper;
- the OIDC identity-index key (restored OIDC identities must be relinked);
- the server-side Control Plane Session cookie/verifier key;
- OIDC state and nonce keys;
- CSRF keys;
- any signed App-handle MAC keys.

The browser cookie is opaque; its durable session identifier is stored only as a
verifier. Rotating the cookie/verifier key invalidates every restored browser
session.

Disable ingress during recovery validation. All users must log in and issue new
API keys. This prevents restored credentials from authorizing automatically but
does not erase restored memory; that limitation must remain visible in operations
and privacy documentation.

## 16. Observability and health

Structured request logs include correlation ID, replica ID, credential kind,
method/tool category, outcome class, latency, and an access-controlled
pseudonymous Tenant fingerprint. They exclude credentials, verifier fragments,
email, identity claims, namespace, request bodies, memory content, and resource
payloads.

Prometheus labels are bounded enums only. ADR-0048's existing generic operation
families gain bounded operation/outcome/result categories for auth, transport,
runtime cache, activation/eviction, scheduler, schema, Task, and subscription
behavior. Tenant/Account/key/resource/request IDs never become labels.

`/health/live` checks process responsiveness. `/health/ready` checks admission,
registry connectivity, and required common dependencies, not every Tenant. An
optional degraded embedding provider leaves readiness true when the existing
degraded contract can safely serve requests.

## 17. Shutdown

On termination:

1. readiness becomes false;
2. new HTTP admission, cold activation, and lease acquisition stop;
3. active requests and SSE streams receive up to 30 seconds;
4. request-scoped work is cancelled at the deadline;
5. worker heartbeats stop and durable leases expire/release safely;
6. Task/change state already committed remains available to another replica;
7. tenant runtimes drain and process-global resources close last.

No shutdown path waits forever.

## 18. Embedded HTTP profile

Embedded SurrealDB remains technically supported for HTTP development,
demonstration, and single-process testing. It is explicitly non-production:

- one process exclusively owns its storage directory;
- no horizontal replicas or HA;
- no zero-downtime rolling deployment;
- process-local failure affects all Tenants;
- standard external SurrealDB backup/operations assumptions may not apply;
- startup emits a prominent profile warning.

The chosen global bind default remains `0.0.0.0:8080`; documentation must warn
that embedded mode can therefore be network-visible and still requires auth,
Host/Origin policy, and a proxy for safe exposure.

## 19. Configuration contract

Environment names are finalized during implementation, but the typed config must
cover:

- deployment profile and bind address;
- public base URL, trusted proxy, allowed hosts/origins;
- SurrealDB control and tenant connection settings;
- API-key pepper, OIDC identity-index key, and browser/OIDC security keys;
- OIDC issuer/client/audience and operator mapping;
- signup mode and control-plane/UI enablement;
- body/deadline/shutdown limits;
- global/per-Tenant admission limits;
- runtime capacity, TTL, activation timeout/backoff;
- Task and subscription retention/queue limits;
- plan/quota values;
- provider policy;
- metrics enablement.

Startup validates the whole profile before opening the listener. It rejects
missing security-critical values, wildcard production Origin, incompatible
feature/config combinations, `fs-watch`, and open signup without quotas. Logs
print a redacted profile summary.

## 20. Validation and release gates

### 20.1 Protocol conformance

A black-box suite starts `memory_mcp_http` and verifies:

- `server/discover` and version pinning;
- required modern metadata and result envelopes;
- standard header/body matching and `HeaderMismatch`;
- POST request SSE framing and final response;
- notification `202` with no body;
- GET/DELETE `405`;
- no session headers or resume behavior;
- disconnect cancellation and response-body release of admission/runtime ownership;
- Host/Origin, content type, Accept, body limit, auth, overload, and unsupported
  legacy version behavior;
- subscriptions and Tasks only when advertised.

### 20.2 Isolation and concurrency

Tests alternate two Tenants under high concurrency and prove:

- no namespace-bound client is rebound or reused across Tenants;
- all cache keys include Tenant identity;
- App, Task, quota, and change data cannot cross Tenants;
- activation single-flight and pool capacity hold;
- pinned runtimes are not evicted;
- stale fenced workers cannot commit;
- optimistic App conflicts do not lose updates;
- subscription events from another replica are delivered or repaired through the
  outbox;
- separate subscription capacity cannot starve ordinary request capacity;
- revocation terminates access within the documented bound;
- a logically purged Tenant remains inaccessible, its namespace binding is not
  reused, audit/tombstone records remain durable, and only expired ephemeral
  Task/App Session rows are physically removed.

### 20.3 Crash/recovery

Inject crashes between every provisioning, migration, Task, lease, logical
deletion, and outbox transition. Restart/reconcile must converge without
cross-Tenant effects or silent lost durable work. Deletion recovery must preserve
the non-reusable Account/Tenant tombstone and audit history while keeping all
revoked credentials unusable.

### 20.4 Compatibility

Existing stdio tests remain unchanged and prove:

- default features and zero-config startup remain local;
- no HTTP/OIDC environment is required;
- Active Namespace behavior remains ADR-0038 compliant;
- the frozen eight-tool snapshot remains stable.

A non-normative interop matrix exercises current official SDK clients and selected
real clients. Client incompatibility does not enable legacy sessions.

### 20.5 Operations

Before public open signup:

- load-test expected `<=20` and contingency `<=500` active-user classes;
- measure Tenant Runtime memory and cold activation;
- validate quotas against extraction amplification and storage cost;
- verify proxy streaming/no-buffering and `/metrics` restriction;
- perform a standard backup restore drill for the selected production remote
  SurrealDB deployment; an embedded-profile drill does not satisfy this gate;
- execute the credential-rotation recovery runbook;
- document backup resurrection, no export/restore, remote provider processing,
  embedded limitations, and unrestricted application egress policy.

## 21. Implementation sequencing

1. Preserve stdio through regression gates and introduce feature/binary boundaries.
2. Add modern-only HTTP transport and raw conformance tests without tenancy.
3. Add Tenant Registry, API keys, principal resolution, provisioning, and immutable
   runtime pool.
4. Add multi-replica leases, migration scheduler, quotas, and lifecycle controls.
5. Make App Sessions durable with optimistic concurrency.
6. Add durable extraction Tasks.
7. Add durable resource outbox and subscriptions.
8. Add OIDC control-plane API and Dioxus SPA.
9. Add OAuth Resource Server support as a separately tested phase.
10. Complete load, restore, security, and client interoperability release gates.

Each phase must preserve the invariant that namespace never enters ordinary MCP
arguments or protocol-agnostic capability inputs.
