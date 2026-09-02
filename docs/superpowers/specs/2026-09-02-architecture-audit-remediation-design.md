# Architecture Audit Remediation Design

**Status:** Approved in design review on 2026-09-02

## Goal

Resolve every finding from the 2026-09-02 architecture audit while preserving the frozen eight-tool MCP surface, stdio behavior, the One Active Namespace invariant, existing database schemas, and the Streamable HTTP protocol contract.

## Delivery strategy

Work is delivered as seven independently reviewable stages:

1. restore feature-matrix correctness;
2. separate production and test composition;
3. make HTTP release gates executable and truthful;
4. split the control Registry by domain capability;
5. move API-key creation and OIDC signup workflows out of Axum adapters;
6. improve internal `MemoryService` and context-pipeline testability;
7. classify public wrappers and reconcile documentation with implementation.

Each stage must leave the repository compiling and its focused tests passing. Work within a stage may run in parallel only when file write sets do not overlap.

## Architectural decisions

### ADR-0053: Explicit HTTP storage and migration composition

Cargo features expose code but do not select production storage. `test-fixtures` remains available for builders, deterministic bootstrap, and fault-injection helpers, but merely enabling it must not replace the production Registry or migration adapters. The `memory_mcp_http` composition root always constructs `SurrealRegistryStore` and `SurrealTenantMigrations` from validated environment configuration. Tests select in-memory adapters explicitly through a test builder.

Normal CI exercises durable startup, migration, restart, and binding through isolated embedded SurrealDB storage. A separate release profile records remote SurrealDB, multi-replica, restore, credential-rotation, proxy, interoperability, and 500-tenant evidence. Evidence that requires an external environment is never fabricated.

ADR-0053 refines ADR-0011, ADR-0038, and ADR-0052. It does not change migrations or request-level namespace rules.

### ADR-0054: Capability-specific control Registry interfaces

The current `RegistryStore` is replaced incrementally by crate-private capability interfaces:

- `RegistryHealth`;
- `AccountIdentityStore`;
- `CredentialStore`;
- `TenantProvisioningStore`;
- `PlanUsageStore`;
- `OidcRequestStore`;
- `ControlSessionStore`;
- `AccountDeletionStore`.

Required operations have no permissive default implementation. Cross-row atomic operations remain named storage methods and are not reconstructed as application-layer sequences. One concrete `SurrealRegistryStore` or `InMemoryStore` allocation may be viewed through several capability trait objects; the split does not create eight adapter instances.

ADR-0054 extends ADR-0044's named-method and SQL-locality rule and refines ADR-0052's internal dependency shape without changing identity, tenancy, provisioning, quota, session, or deletion semantics.

## HTTP state construction

`HttpState` has one shared assembly path. Small feature-gated public wrappers may normalize the Prometheus argument, but they do not duplicate Registry, pool, authentication, resolver, OIDC, admission, or state construction.

Tests construct state through `HttpStateTestBuilder`. The builder accepts source inputs—configuration, Registry handle, and optional metrics handle—and derives the pool, authenticator, resolver, admission gate, and OIDC state consistently. Direct `HttpState` literals are removed from tests.

## Release evidence

The existing protocol, isolation, and proxy integration tests share one subprocess fixture. The fixture owns a unique durable storage directory, process lifecycle, readiness wait, client construction, modern MCP metadata, tenant bootstrap, restart, and cleanup.

The 20-tenant test runs real HTTP requests in normal CI and asserts successful responses, tenant isolation, explicit error counts, and measured latency statistics. The 500-tenant test is not ignored; it requires an explicit release environment gate and emits machine-readable evidence.

Crash/recovery tests inject failures at named durable transition points, restart against the same database, run reconciliation, and assert convergence and tenant isolation. Durable Task and subscription tests cover restart and stale-fence behavior. Proxy and interoperability evidence records exact versions, commands, commit, and observed results.

Project status remains: core implementation complete, release verification incomplete, not production-ready. It may advance only when every required evidence record contains an observed passing run.

## Control-plane workflows

Axum handlers retain request extraction, authentication/session extensions, JSON parsing, headers, redirects, cookies, and status mapping.

`ApiKeyCreation` owns account and tenant validation, plan lookup, expiry calculation, random-secret generation, verifier construction, capped atomic insertion, and the one-time-secret result.

`OidcSignup` accepts only a verified issuer and blind-indexed subject. It resolves an existing account or atomically creates the Account/Tenant/ExternalIdentity bundle. On a uniqueness race it rereads the identity and returns the winning account. Raw OIDC subjects never enter this module.

No command bus, generic repository framework, or new public control-plane abstraction is introduced.

## Core service testability

Existing public `MemoryService` constructors remain compatible. A crate-private `MemoryServiceDependencies` bundle supplies only collaborators that genuinely vary in tests: `DbClient`, `EntityExtractor`, `EmbeddingProvider`, and `TripleExtractor`. Derived stores, caches, rate limiters, loggers, semaphores, and runtime state remain hidden implementation details.

The context pipeline receives characterization tests for ordering, deduplication, temporal filtering, ranking, episode rescue, first-person rescue, logging, and budget behavior. The large orchestration function is then decomposed into a small number of concrete phases with explicit result structs. No traits or parallel execution are introduced because retrieval phases have ordering dependencies.

## Compatibility and documentation

Public wrappers with no repository callers are not removed based on that fact alone. Each is classified as an intentional compatibility interface or an accidental surface. Intentional wrappers receive documentation or tests; accidental wrappers are deprecated before removal unless a breaking release is explicitly approved.

The canonical CSRF route is `/api/v1/account/csrf`. Documentation is aligned to the mounted route. Stale placeholder comments are removed. Existing uncommitted `README.md` edits are preserved and not overwritten.

## Constraints

- Rust version remains `1.97.1`.
- No new MCP tool is added.
- No dependency is added or changed.
- No generated code or migration file is modified.
- No request field, URL, or claim selects a namespace.
- Default Cargo features remain `[]`.
- Feature flags remain additive.
- `main.rs` remains CLI parsing and dispatch only.
- Business logic stays outside protocol adapters.
- Production errors remain `MemoryError`/`thiserror` based.
- Production code does not use `unwrap()`.
- Facts remain append-only and are invalidated rather than deleted.
- Remote operational evidence must come from an actual supported deployment.

## Completion criteria

1. The full additive feature matrix compiles, including Prometheus, control plane, and test fixtures together.
2. Enabling `test-fixtures` does not change production Registry or migration selection.
3. At least one black-box suite exercises durable embedded production composition.
4. HTTP load, crash/recovery, durable Task, subscription replica, isolation, protocol, and proxy gates execute real behavior.
5. Operational evidence clearly separates executed local gates from pending external gates.
6. `RegistryStore` and `RegistryHandle::store_clone()` are removed after all consumers move to capability interfaces.
7. API-key creation and OIDC signup are directly testable without Axum.
8. Existing public `MemoryService` behavior and stdio behavior remain unchanged.
9. Public wrappers and documentation have deliberate, truthful status.
10. Formatting, tests, workspace checks, and required all-feature clippy pass with zero warnings.
