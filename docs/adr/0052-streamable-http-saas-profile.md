# ADR-0052: Add a modern Streamable HTTP SaaS profile

## Status

Accepted and implemented — 2026-08-27 design, implementation completed in the
`streamable-http-mcp` branch. The reviewed completion plan and the repository
quality gates are the implementation record; production open-signup launch still
requires the operational evidence listed in §20.5 of the specification.

## Context

Memory MCP currently runs as a local stdio server with one immutable Active
Namespace. ADR-0038 deliberately removed request-level namespace routing from
that profile. A public remote deployment has different requirements: multiple
private users, authenticated tenant selection, horizontal replicas, remote
lifecycle management, and standard MCP Streamable HTTP.

The target protocol is MCP `2026-07-28`. That revision is stateless at the core:
it removes `initialize`, protocol sessions, `Mcp-Session-Id`, the GET stream,
DELETE session termination, and `Last-Event-ID` recovery. This profile deliberately
implements only that modern path; `rmcp` 3.1.2 also provides optional compatibility
paths, but they are not enabled here.

SurrealDB namespaces provide a native administrative isolation boundary. The
SaaS design must use that boundary without reintroducing namespace as an MCP
argument or mutable service parameter. Account identity and storage identity
must remain independent so OAuth issuers, API keys, and future Account/Tenant
cardinality can evolve. Raw OIDC subject values are sensitive identifiers and
must not be persisted or logged; registry lookup uses a keyed blind index.

## Decision

### Two deployment profiles

Memory MCP has two composition roots:

1. `memory_mcp` remains the existing CLI and local stdio MCP server. It uses one
   Active Namespace selected at startup. Its default behavior and default Cargo
   features remain unchanged.
2. A feature-gated `memory_mcp_http` binary runs the SaaS HTTP profile. It has no
   memory-operation CLI commands. It receives deploy-varying configuration from
   environment variables and exposes Streamable HTTP, optional control-plane
   routes, and optional Dioxus assets.

The binaries share protocol-agnostic capabilities and domain services. They do
not share transport state or composition-root configuration.

### Modern-only MCP transport

The HTTP profile supports only MCP `2026-07-28`:

- `server/discover` is implemented and advertises only the supported modern
  version and actually implemented capabilities;
- the MCP endpoint is one stable `POST /mcp` route;
- GET and DELETE return `405 Method Not Allowed`;
- requests require modern protocol metadata and standard mirrored headers;
- header/body mismatches are rejected before routing or authorization decisions
  trust those headers;
- accepted requests receive request-scoped SSE responses ending in a final
  response;
- disconnect cancels request-owned work, not already committed durable work;
- no protocol session store, `Mcp-Session-Id`, GET stream, DELETE session, event
  ID, or `Last-Event-ID` behavior exists;
- legacy-only clients receive a stable unsupported-version response.

The implemented `rmcp` configuration uses a no-session manager, disables legacy
session mode, requires stateless protocol metadata, and pins supported protocol
versions. The workspace requests `rmcp` 3.1.2 compatibility and the lockfile
currently resolves the compatible 3.1.4 release.

### Identity and tenancy

An External Identity is `(issuer, subject)`. It authenticates an Account. An
Account initially owns exactly one Tenant, but Account and Tenant use independent
opaque identifiers and an explicit relation. A Tenant owns exactly one private
Tenant Namespace in v1. Memory is never shared across Tenants.

The Tenant Registry lives in a separate SurrealDB control namespace/database and
contains Account, identity, credential, provisioning, plan, and immutable
Tenant-to-namespace bindings. Tenant domain data does not live in the registry.
The namespace name is opaque, server-generated, never recycled, and never
returned as a data-plane selector.

Every request first produces an Authenticated Principal, then resolves its
Account and ready Tenant, then acquires an immutable Tenant Runtime. MCP
arguments, URLs, OAuth claims other than the verified identity mapping, and API
key contents never select a namespace. The raw OIDC subject may exist only in
transient validated request memory; it is represented in the registry by a
keyed blind index.

V1 uses one privileged SurrealDB credential across namespaces. Every runtime
owns a clone-once, bind-once namespace/database session that cannot be rebound.
The storage seam is designed so a later namespace-scoped credential can replace
the privileged credential without changing capabilities.

### Authentication and control plane

V1 authenticates MCP requests with named Account API Keys sent as
`Authorization: Bearer`. A key has an opaque public identifier and a
cryptographically random secret. The secret is shown once. The registry stores a
keyed verifier, lifecycle metadata, and no recoverable secret. Verification uses
constant-time comparison and an environment-provided pepper. Rotating the pepper
after disaster restore invalidates all restored keys.

An Account may have at most ten active keys by default. New keys default to one
year expiry, with a configurable shorter period or no expiry. Positive principal
resolution may be cached for at most 60 seconds; unknown or expired credentials
fail closed. Revocation is durable immediately, but without a cross-replica push
invalidation channel its externally effective bound is 60 seconds. Every valid v1 key authorizes the full frozen eight-tool surface.

The optional self-service control plane uses an external OIDC provider,
Authorization Code with PKCE, server-side Control Plane Sessions in secure
cookies, CSRF protection, and recent authentication for credential management,
identity linking, and deletion. External Identities are linked only by an
authenticated action, never by matching email. Signup supports `invite_only`
(default) and explicit `open` modes. OIDC is optional at startup when the control
plane is disabled; enabling the control plane without its required OIDC
configuration fails startup. Transient server-side OIDC flow material is
encrypted and authenticated.

Future standard MCP OAuth is implemented as an OAuth Resource Server. Strict
issuer, audience/resource, signature algorithm, expiry, and Account status
validation produce the same Authenticated Principal shape as API keys. Protected
Resource Metadata follows RFC 9207 where applicable; JWT audience accepts string
or array form; JWKS refresh is bounded single-flight and fails closed without
blocking the async runtime.

### Tenant Runtime pool

A Tenant Runtime contains immutable Tenant identity, namespace-bound storage,
tenant-bound capabilities, and bounded tenant state. Heavy tenant-independent
resources such as model weights, HTTP pools, configuration, telemetry, and
schedulers remain process-global.

The local pool defaults to 32 runtimes, 15-minute idle eviction, LRU pressure
eviction, per-Tenant request concurrency four, two-second bounded capacity wait,
and 30-second cold activation timeout. Values are environment-configurable.
Concurrent activation is single-flight per Tenant and globally bounded. One
replica has at most one nonterminal runtime for a Tenant; replacement activation
waits until the draining runtime is unloaded.

Runtime lifecycle is:

```text
loading -> ready -> draining -> unloaded
             |
             +-> failed
```

An operation guard pins a runtime. Only unpinned runtimes may enter draining.
Draining rejects new work and allows bounded completion. Subscription streams
retain only minimal subscription context and do not indefinitely pin the full
runtime. Ordinary request permits and runtime guards are owned by the returned
body until completion, error, or drop; request extensions/handler-future lifetime
are not sufficient for streaming responses. Subscriptions use a separate bounded
admission budget and recheck authorization within the 60-second revocation bound.
Failed activation has bounded negative caching/backoff. Runtime presence is
never proof of continued authorization.

### Provisioning, migration, and maintenance

Account/Tenant provisioning is a durable, idempotent state machine. The registry
reserves stable identifiers and an immutable namespace binding, then a fenced
worker creates the namespace, applies append-only migrations, verifies schema
postconditions, and marks the Tenant ready. Data-plane access is allowed only in
the ready state. Orphan namespaces and partial transitions are reconciled; names
and IDs are never reused. Lease claims use CAS and carry owner, lease ID, expiry,
heartbeat, and monotonically increasing fencing generation; state transitions,
worker commits, and releases verify the current fence.

Migrations run in the control plane, never on the first MCP request. Rolling
migration supports a narrow schema compatibility window, normally `N` and
`N-1`. Each replica declares its supported range. Incompatible Tenants receive a
stable temporary-unavailability response while the scheduler migrates them.

Long-lived maintenance is owned by process-level schedulers. They are tracked
tier-1 loops under ADR-0046. Request handlers may commit durable work records but
must not launch untracked correctness-sensitive execution. Schedulers select due
Tenants, acquire datastore-time leases with monotonically increasing fencing
tokens, perform bounded work, and unload any temporary runtime. Provisioning,
schema migration, App Session cleanup, Task retry/reconciliation/retention,
quota reconciliation, runtime eviction, and subscription/outbox repair are
explicit tracked jobs, joined during shutdown. A stale worker cannot commit after
lease takeover.

### Durable App Sessions, Tasks, and subscriptions

In stdio, App Session state may remain process-local. In the HTTP profile an App
Session is durable tenant-owned application state with optimistic versioning,
30-minute idle expiry, 24-hour absolute expiry, and a default limit of 32 open
sessions. It is not an MCP transport session.

The Tasks extension is advertised only for extraction. `ingest` synchronously
commits and returns its episode result. `extract` may return a durable Tenant Task
when the client advertises Tasks; clients without Tasks receive bounded
synchronous execution or preflight rejection for work exceeding the synchronous
policy. Task state, cancellation intent, result, retention, lease owner, and
fencing generation live in the Tenant Namespace. `rmcp` is the protocol adapter,
not the durable source of truth. Cancellation is cooperative and never rolls
back already committed facts.

MRTR is not advertised. Public operations do not request roots, sampling, or
elicitation from the client. Modern ordinary results use the required complete
result envelope and never return `input_required`.

`subscriptions/listen` is implemented only for tenant-owned App/resource
changes through rmcp's native subscription handler hooks, not a second custom
JSON-RPC route. The mutation and a Tenant Change Event are committed atomically
through a durable outbox/change log. Notifications are invalidations containing
resource identity and revision, not copies of every intermediate state. A
connected stream uses a bounded cursor and queue; slow consumers are disconnected.
A broker or SurrealDB live signal may wake replicas, but the durable outbox is
authoritative. After disconnect there is no transport replay: the client
reconnects and rereads canonical resource state. Tool and prompt list changes are
not advertised.

### HTTP and browser boundary

The HTTP binary defaults to `0.0.0.0:8080` and is expected to run behind a reverse
proxy that terminates TLS. One origin hosts:

- `/mcp` for Bearer-authenticated MCP;
- `/api/v1/*` and OIDC callbacks for cookie-authenticated control plane;
- Dioxus SPA static assets and client-side fallback;
- minimal liveness/readiness routes;
- `/metrics` when Prometheus is configured.

Route groups use separate middleware. The MCP route never accepts browser cookie
auth. Control mutations require CSRF and recent authentication. Host and Origin
allowlists are explicit in production; an absent Origin is allowed for
non-browser MCP clients. Forwarded headers are trusted only from the configured
proxy boundary.

The Dioxus UI is a separate web/WASM workspace crate behind an additive
`control-plane-ui` feature. It calls the explicit same-origin `/api/v1` contract;
it does not use Dioxus server functions or call MCP through loopback. API-key
secrets are returned once with `Cache-Control: no-store`, held only in page
memory, and never placed in storage or URLs. A strict CSP and standard browser
security headers apply.

`/metrics` has no application-level authentication by decision. The reverse
proxy/network deployment must restrict it. Direct public exposure of the backend
port can disclose metrics and is an accepted deployment risk. Metrics retain the
bounded-label rules of ADR-0048 and never use Account, Tenant, namespace, key,
resource, Task, or request identifiers as labels. This ADR amends ADR-0048 to
permit new HTTP/auth/runtime/Task/subscription operation categories within its
existing three generic metric families; it does not authorize new unbounded
families or labels.

### Input, quotas, and resource limits

The SaaS HTTP profile accepts only inline ingest content. Server-local paths,
directories, `file:` URLs, and remote URL fetching are rejected before I/O. The
filesystem watcher remains a local stdio capability and is a startup
configuration error for HTTP SaaS.

A default plan/policy in the Tenant Registry controls cumulative ingested bytes,
episode count, open App Sessions, active API keys, request limits, and rate
limits. Durable usage counters provide admission control and periodic
reconciliation repairs drift. Retrieval remains available when write quota is
exceeded. Explicit quota/rate rejection is HTTP `429`; temporary runtime or
admission capacity overload is HTTP `503`. Open signup is a startup error unless
explicit quota values are set and release evidence covers memory amplification
and storage cost.

The default request body limit is 8 MiB, ordinary request handler/body deadline
120 seconds, and graceful shutdown period 30 seconds. `subscriptions/listen` is a
long-lived POST response and is exempt from the ordinary body deadline; it ends
on client cancellation, authorization loss, shutdown, or its configured
liveness policy. The returned body owns ordinary request permits and runtime
pins until completion, error, or drop. Admission control separately bounds
global requests, per-Tenant requests, cold activations, maintenance,
subscriptions, and their queues.
Edge rate limiting protects invalid-auth traffic; application limiting protects
Account/API-key traffic. Production network egress is not restricted by the
application deployment contract, but no tenant-triggerable generic fetch exists
in the SaaS profile.

### Deletion, backups, and recovery

After recent authentication and explicit typed confirmation, Account deletion
durably revokes live access and runs an idempotent logical deletion workflow.
The Account and Tenant enter terminal deletion states, data-plane access is
permanently denied, retained memory records are invalidated according to domain
policy, and only expired ephemeral sessions/Tasks may be physically removed.
Account, Tenant, identity, credential, lease, provisioning, and audit-bearing
registry records remain durable as a non-reusable tombstone. Cached authorization
can remain effective for at most the documented 60-second bound. There is no
recovery window and no user export in v1.

Memory MCP implements no backup, export, per-Tenant restore, backup rewrite, or
erasure ledger. Backup and disaster recovery use only the selected SurrealDB
deployment's standard mechanisms. Historical immutable backups are not modified
by live logical deletion and may retain or restore logically purged data until
their provider-managed retention expires. Therefore the product must describe
deletions as durable access revocation and logical invalidation, not immediate
physical erasure from every live or historical copy.

A disaster restore is an explicit operator recovery event. Before opening ingress,
the operator rotates the API-key verifier pepper, the OIDC identity-index key,
and all browser/OIDC session, state, nonce, CSRF, and handle-signing secrets,
invalidating restored credentials and requiring restored OIDC identities to be
relinked.
Without an external erasure ledger the application cannot prove which restored
Tenants had been purged after the snapshot; possible data resurrection is an
accepted limitation and must be documented in the recovery runbook.

### Observability and shutdown

Structured logs may contain correlation IDs and access-controlled pseudonymous
Tenant fingerprints, but never credentials, raw namespace, email, request body,
memory content, or unbounded identifiers in metrics. Control-plane security
changes produce append-only audit events. Account deletion retains the
necessary audit-bearing records in the durable non-reusable tombstone; it does
not physically delete audit history.

Liveness reports process health without probing every dependency. Readiness
requires safe authentication, registry access, and runtime admission, but not
every Tenant Namespace or an optional healthy remote embedding provider. During
shutdown readiness falls first, new admission and lease acquisition stop, active
requests drain for 30 seconds, remaining request-owned work is cancelled, and
durable work remains reclaimable by another replica.

## Amendment to ADR-0038

ADR-0038 remains fully in force for `memory_mcp` CLI and stdio deployments. Its
statements that one process has one Active Namespace and that `tenant` is not a
partitioning concept are narrowed to that local profile.

For `memory_mcp_http`, this ADR supersedes only those process-wide statements:
one process may host a bounded set of immutable Tenant Runtimes, each bound to a
separate Tenant Namespace selected from an Authenticated Principal and Tenant
Registry. Namespace remains absent from ordinary data-plane arguments.

## Consequences

### Positive

- Local stdio remains simple and backward compatible.
- HTTP transport follows the current stateless MCP architecture without legacy
  session machinery.
- Tenant selection is centralized and cannot drift through capability arguments.
- Multi-replica routing requires no affinity.
- Durable App Sessions, Tasks, subscriptions, migrations, and maintenance survive
  replica changes.
- Heavy process resources can be shared without sharing tenant data.
- API keys and future OAuth converge at one authorization seam.

### Negative

- The HTTP profile adds a substantial control plane, scheduler, storage, browser,
  and conformance surface.
- One privileged SurrealDB credential has a broad blast radius.
- Namespace isolation does not protect against complete HTTP-process compromise.
- Dioxus, HTTP transport, OAuth, and durable extension support increase build and
  test cost.
- Proxy mistakes can expose unauthenticated metrics.
- Application-unrestricted egress increases defense-in-depth risk even though
  tenant-triggered fetching is disabled.
- Live logical deletion does not erase immutable historical backups, and restore
  may resurrect logically purged data without an external erasure ledger.
- No per-Tenant export or restore exists.

## Alternatives considered

### Add HTTP transport to the existing CLI composition root

Rejected because it mixes local administration and SaaS process identity, makes
transport modes easier to misconfigure, and expands the existing CLI surface.

### Create independent HTTP and core crates immediately

Deferred. Compile-time separation is attractive, but a separate binary and
feature-gated modules provide the needed boundary without a premature broad
workspace refactor.

### Run an HTTP gateway over stdio subprocesses

Rejected because subprocess lifecycle, multiplexing, state routing, Tasks, and
subscriptions would duplicate capabilities already available through native
`rmcp` Streamable HTTP.

### Support legacy and modern MCP sessions

Rejected. Dual-era behavior is optional, not required by the deprecation policy,
and would add session ownership, storage, lifecycle, and security complexity to a
new service.

### Put Tenant in the URL or MCP arguments

Rejected because credentials and request parameters would become competing
sources of storage truth.

### Separate SurrealDB credentials per Tenant in v1

Deferred. It provides stronger defense in depth but adds credential provisioning,
rotation, and connection-pool complexity. The immutable storage seam preserves a
migration path.

### Implement a custom backup/export service

Rejected in favor of standard SurrealDB deployment mechanisms and a smaller v1
surface.
