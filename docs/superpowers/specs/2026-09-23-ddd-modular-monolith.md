# DDD Modular Monolith — design spec

**Date:** 2026-09-23; architecture review: 2026-09-24
**Status:** Accepted direction; strengthened design contract. Phase 0 evidence is required before structural extraction.
**Baseline reviewed:** `5bcb2bf3309ecebf71417c5ab2576e6cca0ca4ee` (`ddd-refactorings`).
**Plan:** [implementation plan](../plans/2026-09-23-ddd-modular-monolith.md)
**ADR:** [ADR-0058](../../adr/0058-bounded-contexts-modular-monolith.md)

## Context and retained decisions

The current `control/`, `service/`, and `storage/` organization mixes policies,
use cases, database access and delivery. The objective is explicit ownership
and inward dependencies, not a directory rename or a trait around every method.
A glossary term is not automatically an aggregate or a bounded context:
Authenticated Principal is an authentication result; Tenant Runtime is a
runtime resource; Tenant Registry is a control-plane persistence capability.

Retain the agreed scope: one coordinated initiative covers the full monolith,
contexts outside and layers inside, one production library crate, console
package rename to `ui`, and removal of both runtime product-shape switches.
The initiative is implemented as buildable commits on the branch and released
as one coherent change. Intermediate extraction is not an independently
supported product architecture. No microservices, per-context crates, new MCP
tools, generic event bus, distributed transaction framework or repository
framework are introduced.

The seven names below are **module boundaries with different roles**. The
business boundaries are working hypotheses verified against invariants and
transactions in Phase 0; embedding and operational orchestration must not be
presented as seven equally rich business domains. Empty domain layers are not
required for modules that contain no domain policy.

## Ownership and context map

| Module | Responsibility and invariant | Explicit exclusions |
|---|---|---|
| `identity` | Accounts, external links, local administrators, credential verification, browser-session lifecycle, auth-method policy and last-administrator rule. An external identity is linked only after proof; a credential is never a namespace selector. | Tenant activation, client provisioning workflow, HTTP cookies/headers, SQL |
| `tenancy` | Account-to-Tenant binding, readiness and immutable namespace binding; bounded runtime acquisition/eviction. Trusted resolution selects one runtime for one tenant. | Account authentication, global service locator, constructing memory services in application code |
| `provisioning` | Client/account/tenant provisioning orchestration, API-key issuance/revocation administration, provisioning leases; tenant task and app-session workflow submodules. Preserve durable state transitions, lease fencing and command idempotency. | Owning every table touched by an app; implementing fact/entity writes or credential verification a second time |
| `operations` | Authorized administrative deletion/recovery orchestration over the owning modules. Preserve recent-auth checks, confirmation and durable deletion stages. | A second implementation of identity rules, a generic privileged database API |
| `memory` | Episode ingestion, recall/assembly, explanation, lifecycle and procedures. Source episodes and provenance survive derived-knowledge changes. | SQL over another module's canonical tables, shared service container |
| `knowledge` | Entities, aliases, facts, claims, triples, communities, extraction/reconciliation and knowledge queries. Distinguish contradiction, correction, supersession and retraction. | Transport DTOs, episode lifecycle ownership, model runtime management |
| `embedding` | Technical capability: embeddings, cache, model artifacts/runtime, recovery/backfill and re-embedding. Own model/version/dimension consistency and job state. | Owning facts, claims or episodes because their vectors need updating |

`provisioning` retains tasks/app sessions as cohesive submodules for this
initiative, not as a claim that an MCP App Session is a client-provisioning
aggregate. Its workflow methods call memory/knowledge capabilities; task state
and leases are not domain entities of identity. Split this module later only
with demonstrated independent invariants/change pressure.

### Directed dependencies

Allowed **static business-module dependencies** (consumer → provider API):

- `operations` → `identity`, `tenancy`, `provisioning`.
- `provisioning` → `identity`, `tenancy`, `memory`, `knowledge`.
- `memory` → `knowledge`, `embedding`.
- `knowledge` → `embedding`.
- `identity`, `tenancy`, `embedding` have no outgoing business-context edges.

This is an allowlist, not a requirement to use every edge. Cross-module calls
use provider `api` commands/queries and data-only contracts. No reverse static
dependency or static API cycle is permitted. For example, identity verifies a credential;
the outer request pipeline then asks tenancy to resolve its account. Identity
does not load a Tenant Runtime. Knowledge receives source evidence/episode IDs
as input and never calls memory back to fetch them.

Infrastructure integration may implement a **consumer-owned port** using a
provider facade (e.g. tenancy's runtime factory builds a memory runtime).
Such adapters live in `bootstrap/integration/`, are wired once, and are listed
separately from business edges. The guard checks the static module DAG and
these exact integration edges separately; implementing a port is not a blanket
cycle exemption. Enumerate the runtime paths as well:

- Tenancy runtime acquisition → injected factory → memory runtime construction;
  that constructor must not acquire the same tenant runtime again.
- Embedding recovery/re-embedding job → injected canonical-record vector port
  → knowledge or memory vector read/update endpoint. These endpoints only read
  the required source/version data or persist the already-computed vector under
  the existing concurrency/event contract; they must not call embedding again.
  Record-owner application paths may independently request embedding generation.

Thus a reverse runtime callback through a consumer-owned port is permitted
without a reverse static business dependency, but recursive orchestration is
not. Check integration paths with behavior tests in addition to source guards.
Passing `Arc<ServiceContext>` or a namespace-selecting database handle across
an API is not dependency inversion.

### Aggregates, ports and consistency boundaries

Each command names an invariant owner, authorized actor, commit boundary and
failure/retry result. An aggregate is the consistency boundary established by
those rules, not an entire directory. Phase 0 must identify actual persisted
records participating in the following operations before moving stores:

| Operation | Policy/use-case owner | Required preservation |
|---|---|---|
| Account + identity + tenant creation | `provisioning`, with identity/tenancy validation | Existing `create_account_bundle` remains one atomic operation; no sequential facade writes that can leave half a client |
| Identity links, auth methods, local admin/session changes | `identity` | Last-link/last-admin checks and mutation + control audit commit together; failed checks leave no audit of a successful change |
| API-key administration | `provisioning`, verification contract owned by `identity` | Issuance secret shown once, irreversible verifier, revocation/cache semantics preserved; no duplicate verifier policy |
| Begin/finalize account/operator deletion | `operations` | Preserve existing atomic registry methods and durable recovery state; tenant purge is not ordinary fact invalidation |
| Tenant task / app command | `provisioning` | Lease generation, cancellation, terminal result and durable change-event rules survive restart and replica races |
| Fact retraction, claims and temporal close | `knowledge` | Preserve paired timestamps, source facts, atomic retraction scope and existing event commit |
| Episode capture / projection work | `memory` | Preserve deduplication identity, durable pending work and retry behavior |

The physical control database is shared; **logical write ownership is not**.
Existing cross-module registry transactions are an explicit integration seam:
`platform/persistence/control/` implements narrow use-case-owned atomic ports
and composes private SQL fragments under one database transaction. Each port
is published through its owner's API for wiring only; applications cannot
get a raw transaction or `query(sql)` escape hatch. Cross-owner writes are
limited to the named operations above and recorded in the ownership manifest.
This seam depends on published contracts, never context internals. It does
not become a universal registry/service object. Ordinary stores remain in
their owner's `infra`.

Preserve existing synchronous/atomic and asynchronous behavior separately.
Do not introduce an outbox everywhere, eventual consistency, retries of
non-idempotent commands, or a saga merely to make a diagram acyclic. Existing
outbox, lease and recovery protocols are retained with failure-injection
checks. If the existing transaction cannot be preserved under a proposed
split, revise that split before implementation.

## Clean architecture contract

```text
src/<module>/
  mod.rs        # private internals, explicit facade exports
  api.rs        # commands, queries, data contracts, minimal wiring ports
  domain/       # pure policies/value objects, only when needed
  application/  # use cases, consumer-owned ports
  infra/        # persistence/provider implementations
src/shared/     # small pure kernel only
src/platform/   # technical mechanisms; no business orchestration
src/bootstrap/  # composition and narrowly named integration adapters
src/{http,mcp,cli,tools,ui}/  # input/output protocol adapters
```

| Caller | Allowed dependencies |
|---|---|
| Domain | Own domain and explicitly approved pure kernel types; no other module, application, infra, runtime, environment, filesystem, network, SQL or HTTP |
| Application | Own domain/ports, pure kernel, approved provider `api`; no concrete stores or platform/runtime/config parsing |
| API | Own application and deliberately exported data contracts/ports; no infra re-exports, handlers, database rows, router/state bags |
| Infra | Own ports/domain, pure kernel and specific platform mechanisms; another provider API only through an approved integration adapter |
| HTTP/MCP/CLI/tools/UI | Module APIs, protocol/runtime libraries and adapter utilities; never module internals or raw storage |
| Bootstrap | Explicit context factories, constructors/ports, platform adapters, validated configuration and integration wiring; no business decisions |
| Platform | Technical dependencies and pure kernel; the named control-transaction adapter may import published owner contracts only |

Composition is an explicit exception to the request-adapter rule. A context
may expose `wiring` factories that internally construct its private infra;
only bootstrap may call them. `main.rs` and `bin/memory_mcp_http.rs` remain
thin dispatchers into public library bootstrap entrypoints. Rust binaries,
integration tests and `eval-harness` are separate crates: `pub(crate)` APIs
alone cannot serve them. Inventory their existing public imports; retain
minimal library entrypoints and feature-gated test/eval facades rather than
making all context internals public. Existing external Rust compatibility
paths, if needed, are one-way delegating re-exports with no new internal users;
their removal is not silently included in the four declared changes.

### Pure kernel versus technical platform

`shared` may contain runtime/storage-independent `MemoryError` variants, common
validated identifiers and temporal value semantics. It cannot contain the
whole `models/` tree, `ServiceContext`, registry, database client, SQL, config
parser, global cache or worker runtime. Context-owned models stay with their
owner; API DTOs do not expose mutable aggregates. Reuse a pure value only
when both sides share its meaning, not because fields look alike.

Split current seams by responsibility:

- `ServiceContext` becomes explicit per-use-case dependencies with no service
  lookup, concrete stores or table names in application code.
- `DbClient`, connection pooling, migration runners and reusable transaction
  mechanics belong to `platform/persistence`; only infra/bootstrap use them.
  Keep migration SQL/order/checksums unchanged; moving generated/migration
  files still requires the repository's explicit approval.
- `storage/queries.rs`, record parsing and database-specific value helpers
  split by owning query/store. `close.rs` policy and fact/claim close operations
  belong to knowledge; common timestamp value semantics can be pure kernel.
- `error.rs`'s pure error enum can remain shared; database error classification
  goes to persistence. `control/error.rs` maps HTTP responses and stays in
  `http`, never the shared kernel.
- Environment parsing/secret loading belongs to bootstrap configuration;
  use cases receive typed values, never read environment variables themselves.
- `durable_work.rs` backoff/cancellation mechanics and model/download runtime
  are technical facilities, not business aggregates. Share mechanisms without
  sharing unrelated lease or retry policies accidentally.

### DRY, SOLID, KISS and YAGNI acceptance

One owner per business rule and canonical write path (SRP/DRY). Ports follow
use-case needs, not a universal CRUD repository (ISP/DIP). In-memory and durable
adapters obey the same error, atomicity and ordering contract; fakes must not
make impossible partial writes pass (LSP). Extend an established provider seam
when needed; do not invent plugin registries for hypothetical providers (OCP).
Use direct pure functions and concrete domain types where a trait adds no
boundary. Do not create one trait per struct, empty layers, generic units of
work, a mediator or an event bus. Similar DTOs in different contexts are not
necessarily duplicate policy. Preserve the existing single temporal-close
implementation while moving it to the correct owner.

### Mechanical enforcement

Use Rust privacy first: private `domain`, `application`, `infra` modules;
expose only named API/wiring items. `pub(crate)` is crate-wide visibility, not
a context wall. Restrict internal items to the context ancestor where needed.
Source guards supplement compiler privacy; they do not prove architectural
correctness or authorization.

`tests/module_boundaries.rs` must cover layer imports, API exports/signatures,
fully qualified paths, grouped/aliased imports, `crate`/`self`/`super` paths,
re-exports and the allowed dependency graph including cycle detection. Cover
feature-gated code, `#[path]`/`include!` and macro-based bypasses by an explicit
reviewed policy; do not claim a `use` substring scan resolves Rust semantics.
Use existing parsing facilities or a constrained guard plus compiler checks;
new dependencies require approval. Add negative fixtures for each forbidden
class and positive fixtures for bootstrap/test/eval exceptions. A checker
that passes empty directories is not evidence. New extracted modules must
have zero violations; legacy exceptions name exact paths/edges and a removal
phase, never `allow service::*` or a whole-tree exemption.

## Migration inventory and ownership corrections

A prose list of directory globs is not a completed machine census. Phase 0
produces a checked manifest for every tracked `src` file (and affected tests,
binaries, build scripts): source → destination(s), symbols when split,
layer, table/command owner, phase, consumers and exception expiry. A file may
split; every item ends with exactly one owner. Reject unmapped files and
unexplained overlaps. Module roots contain behavior too; never discard a
`*.rs` file merely because there is also a directory with the same name.

| Source family | Target and required split |
|---|---|
| `control/static_assets.rs` | Layer-free `ui` delivery adapter |
| `control/{operator,deletion}.rs` | HTTP handlers to `http`; administrative workflows to `operations`; registry transactions to the named control persistence seam |
| `control/{account_api,oidc,local_admin,session,recent_auth,secret,csrf}*`, `control/application/*`, `service/local_admin/*`, `credential_material.rs` | Identity policy/use cases versus provisioning client/key commands; keep cookies, CSRF HTTP checks and request/response mapping in `http`; KDF/OIDC client implementations in identity infra |
| `http/{oauth,principal}*` | Transport metadata/middleware stays HTTP; credential verification to identity; trusted tenant resolution remains a separate tenancy call |
| `http/registry*` | Split account/link/session records to identity contracts, tenant binding/readiness to tenancy, client/key/provisioning commands to provisioning; preserve atomic control adapter exceptions |
| `http/runtime*`, `http/sync.rs` | Tenancy lifecycle/pool; concrete runtime construction to bootstrap integration; protocol concerns remain HTTP |
| `http/{leases,tasks,app_sessions}*`, `service/apps*` | Provisioning workflow/state; transport parts remain adapters; memory/knowledge actions call owner APIs |
| `http/subscriptions*` | Stream delivery in HTTP; `outbox.rs` transaction mechanism in platform persistence with owner-owned mutations/events; no infra-to-HTTP dependency |
| `service/{episode,context,capabilities,agent_memory,lifecycle,core,procedures}*`, `ingestion.rs`, `explanation.rs` | Memory use cases/domain; split extraction/knowledge work, provider wiring, SQL and protocol logging instead of relocating wholesale |
| `service/{fact,entity,entity_resolution,claims,conflict_resolver,community,content_extraction,entity_extraction,triple_extractor,query}*` | Knowledge policy/use cases and infra as appropriate; model runtime adapters use embedding capability |
| `service/{embedding*,model_*,cache*,reembed*}` | Embedding technical capability; recall-result cache belongs to memory rather than all cache code automatically going to embedding |
| `storage/{episode_store,agent_memory,procedures}.rs` | Memory infra |
| `storage/{fact_store,entity_store,triple_store,claims,inbox_revision_store}.rs` | Knowledge infra; preserve cross-owner inbox/episode transaction behavior explicitly |
| `storage/context_store.rs` | Memory context-access log and episode reads; knowledge fact/entity/triple queries behind knowledge API. It is not a provisioning store |
| `storage/app_store.rs` | Split by canonical data owner: knowledge graph/fact/entity/community operations and memory episode/lifecycle operations; app workflow orchestration stays provisioning |
| `storage/{embedding_backfill_store,embedding_state_store,reembed_store}.rs` | Embedding infra job state; canonical record mutations through owner-owned integration ports |
| `models*`, `error.rs`, `control/error.rs`, `config*`, `http/config*`, `service/{service_context,util*,value_helpers,durable_work}*`, remaining storage helpers/client/migrations | Split by the pure-kernel/platform rules above, not wholesale into shared |
| `service/startup.rs`, `service/core/builder.rs` | Bootstrap; no business logic |
| `service/fs_watch*`, `service/mock_db.rs` | Filesystem input adapter and test support respectively |
| `logging.rs`, `observability.rs`, `runner.rs`, `eval_support.rs`, `lib.rs`, `main.rs`, `bin/*`, `http*`, `mcp*`, `cli*`, `tools*` and all module roots | Classify actual contents as adapter, bootstrap, platform, test/eval facade or module glue; retained roots for transports do not dissolve just because storage/service roots do |

### Rename surface (the four declared behavior-visible changes)

1. **Package and outward asset names**: `crates/control-plane-ui/` →
   `crates/ui/`; package + `[[bin]]` name `ui`; `Dioxus.toml` name
   `memory-mcp-ui`. Emitted assets become `ui-dxh<hash>.js`,
   `ui_bg-dxh<hash>.wasm`. The `<title>` copy ("Memory MCP control plane")
   stays: "control plane" is the domain term for the subsystem (see
   CONTEXT.md), it was never the package name's problem.
2. **`MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI` removed** —
   `HttpConfig::enable_control_plane_ui` is gone with no successor flag; the
   console is served whenever the profile carries it.
3. **`MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE` removed** —
   `HttpConfig::enable_control_plane` is gone, retiring the data-plane-only
   runtime mode and its "control off" test matrix rows.
   `memory_mcp_http` (the `streamable-http` profile) always mounts data plane
   + control plane + console as one product and has **no runtime product-shape
   switches at all**.
4. **Names**: feature `control-plane-ui` → `ui` (internal name, implied by
   `streamable-http` as before); build-time `MEMORY_MCP_CONTROL_PLANE_UI_DIST`
   → `MEMORY_MCP_UI_DIST` (hard rename — build env is not deployed API);
   generated manifest `control_plane_assets.rs` → `ui_assets.rs`.

Compatibility notes: hashed asset URLs change with any bundle rebuild anyway;
the relocatable-bundle sentinel contract (`/__memory_mcp_base__`) is
content-driven and **not** affected by file names. Cookie contracts, OIDC
redirects, HTTP routes, the eight-tool MCP surface and the bi-temporal model
are untouched.

### Runtime configuration and deployment

Removing the two flags is a product decision, **not a requirement of DDD or
Twelve-Factor**. Auth-method selection, signup policy, quotas, endpoints and
secrets remain runtime configuration. Cargo features remain additive:
`streamable-http` is the supported SaaS entrypoint, `default = ["fs-watch"]`
remains local, and orthogonal features retain their existing meaning. Internal
compatibility aliases must keep compiling until explicitly retired.

SaaS always mounts its compiled product capabilities. Reverse-proxy routing
controls exposure, but never replaces application authentication, authorization,
Host/Origin/CSRF checks or safe bind/firewall configuration. Method-specific
routes still follow the configured browser-auth method set. Removing a product
flag does not imply mounting every authentication method.

Preserve OIDC validation based on the selected method and local-auth durable
reconciliation. Test startup with the now-unconditional control plane: provider
discovery, local administrator bootstrap, readiness and method/key drift
failures. Do not remove optional-store safeguards solely because a flag was
removed; establish constructor invariants for every supported build/test path.
Document upgrade prerequisites for former data-plane-only deployments before
release. Old environment keys no longer select product shape; deploy manifests
must remove them and must not rely on ignored values to disable routes.

ADR-0056's no-bundle developer/test build remains valid: an unset
`MEMORY_MCP_UI_DIST` produces an empty catalog with the existing fallback
behavior. A supported release image must contain a real bundle; verify it from
the final binary/image at root and `/memory`. Thus "unconditional" means no
runtime UI switch, not that every developer binary embeds assets.

### Twelve-Factor application profile

The following are acceptance concerns, not a claim that this documentation
refactor achieves full Twelve-Factor compliance. The local embedded/stdio
profile deliberately retains durable local storage and stdout protocol framing.
For SaaS, record baseline behavior and preserve it; any newly discovered
operational shortfall requiring behavior change becomes a separate issue.

| Factor | Concrete check / boundary |
|---|---|
| I Codebase | One versioned source/release revision; modules are not separately deployed services |
| II Dependencies | Locked dependencies/toolchain, explicit build tools and reproducible UI assets |
| III Config | Startup parsing into typed values; no environment reads in domain/use cases, no baked-in deployment credentials |
| IV Backing services | Durable storage/provider adapters configured at composition; remote shared storage for replicas, not a shared embedded database directory |
| V Build/release/run | UI built and embedded before release, same immutable artifact promoted with runtime config; no runtime asset compilation |
| VI Processes | Caches/runtimes disposable; authoritative SaaS sessions/tasks/leases remain durable. Local embedded profile is an explicit exception |
| VII Port binding | HTTP listener lifecycle remains application-owned; proxy topology does not alter authorization |
| VIII Concurrency | Existing replica coordination and fencing verified against durable storage; no claim that folder separation grants horizontal scalability |
| IX Disposability | Stop accepting work, drain/cancel within existing bounds, preserve recoverable work and lease expiry on termination |
| X Dev/prod parity | Durable-adapter tests supplement fakes; exercise actual image, OIDC/local modes and mount bases |
| XI Logs | Structured diagnostic streams, bounded labels, no secrets; stdio stdout remains reserved for MCP protocol |
| XII Admin processes | CLI maintenance uses the same policies/config/adapters as the service; no bypass of audit or migrations |

## Compatibility and completion gates

Except for the four declared changes, preserve eight MCP tools, HTTP payloads,
status/envelope/error sanitization, cookie/OIDC contracts, temporal semantics,
config defaults and persisted schema. Namespace invariant: **one immutable
Active Namespace per local stdio process; one immutable Tenant Namespace per
SaaS Tenant Runtime**, selected through authenticated server-side resolution.
Neither accepts a namespace in data-plane arguments. Tenant A's cached/runtime
state must never be reused for Tenant B.

Identity administration, operational purge and ordinary fact invalidation are
different capabilities; preserve their distinct authorization and audit rules.
No schema, migration or data-copy change is authorized by a module move.
Historical ADRs 0001–0057 and prior specs/plans stay unchanged; living docs and
this ADR/spec/plan must agree on the target and implementation status.

Completion requires the plan's code gates, architecture negative tests,
transaction/tenant-isolation tests, actual evaluation gate and shipped-artifact
acceptance. Record exact revision/features/commands and skips. Unit tests,
source pins and a successful UI filename probe cannot alone prove this design.

## Reference basis

- [Clean Architecture dependency rule](https://blog.cleancoder.com/uncle-bob/2012/08/13/the-clean-architecture.html): inward source dependencies and simple boundary data.
- [Rust visibility and privacy](https://doc.rust-lang.org/reference/visibility-and-privacy.html): module privacy versus crate-wide visibility.
- [The Twelve-Factor App](https://12factor.net/): SaaS operational criteria, applied above with explicit local-profile exceptions.

These sources support the principles; ownership choices and the dependency
allowlist are project design decisions grounded in the reviewed checkout.
