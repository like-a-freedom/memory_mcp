# ADR-0055: Local administrator authentication for remote deployment

## Status

Accepted product direction, 2026-09-19. Technical design approved and implemented; release verification is incomplete. The evidence-based [runbook](../operations/LOCAL_ADMIN.md) lists what is verified and what is not. This ADR records the approved requirements, not approval of dependencies, migrations, or implementation.

Amended 2026-09-21 by [ADR-0057](0057-additive-browser-auth-methods.md): browser authentication methods are additive, so the exclusivity clause in §Deployment authentication modes no longer describes the shipped behaviour. Everything else below — local administrator identity, CLI bootstrap and recovery, client provisioning, key issuance, audit and isolation — remains in force.

## Context

[ADR-0052](0052-streamable-http-saas-profile.md) describes an optional OIDC self-service control plane. MCP requests authenticate independently with Account API keys. A deployment without the control plane can serve existing keys, but does not provide a browser workflow for onboarding clients without an OIDC provider.

Operators who do not use OIDC need to create clients and issue their credentials. Requiring an identity provider adds an external service to that workflow. Adding only a key-management CLI would leave the requested browser administration workflow unavailable.

## Decision

### Deployment authentication modes

An enabled control plane has exactly one browser authentication mode: local or OIDC. OIDC retains its self-service workflow. Local mode provides username/password login to an administrator-only UI, with no client self-registration or email dependency. The modes must not expose each other's authentication or administration routes.

Disabling the control plane leaves Streamable HTTP MCP available to clients with valid existing API keys. It does not enable anonymous MCP access. Browser authentication and MCP bearer authentication remain separate.

### Administrators and clients

Local administrators are separate identities from client Accounts and Tenants. Multiple administrators have equal privileges. Creating or authenticating an administrator does not create a client Account or Tenant.

An operator uses the CLI to create an administrator and obtain a one-time expiring activation code. Browser activation establishes the password. CLI recovery invalidates that administrator's sessions and issues a one-time expiring reset code. The CLI manages administrators; client provisioning belongs in the admin UI.

Each client has its own Account and Tenant. Administrators can create and list clients, inspect provisioning readiness, suspend or resume access, and manage named API keys. Key issuance supports an expiry date or an explicit "Never expire" choice. The UI lists non-secret key metadata and supports revocation. Client provisioning uses an explicitly configured default plan; this feature includes no client deletion or quota-editing UI.

API-key secrets are shown once and delivered to clients outside the product. Administrators can access client data by issuing a client key, so issuance must be audited. An administrator browser session alone does not authorize MCP requests.

### Security and isolation

Local authentication requires password hashing, login rate limiting, CSRF protection, secure sessions, and redacted audit/log output. Recovery, key revocation and client suspension must have tested authorization effects, including concurrent requests and multiple replicas.

MCP requests continue to resolve their Account and Tenant from the authenticated API key. Clients cannot select a namespace through request parameters. This decision adds no MCP tools and does not change the eight-tool surface, tenant isolation, or fact-retention rules.

## Alternatives considered

- Require OIDC for every enabled control plane. This preserves one browser authentication implementation but excludes the approved deployment workflow without an identity provider.
- Provide only a client/key administration CLI. This reduces browser functionality but does not meet the UI requirement. CLI scope remains administrator bootstrap and recovery.
- Enable local and OIDC authentication together. This requires additional identity-linking and authorization rules. The selected scope uses one mode per deployment.
- Treat local administrators as client Accounts. This couples deployment administration to tenant ownership. Separate identities keep that distinction explicit.

## Consequences

Local deployments can onboard clients without an external identity provider. The project also takes responsibility for password verification, recovery, session security and abuse prevention. CLI recovery requires privileged access to the deployment; there is no email recovery workflow.

The local UI is an administrator interface, not a client portal. Operators must deliver issued credentials securely. Equal administrator privileges include the ability to issue credentials for client data, not merely manage account metadata.

The [specification](../superpowers/specs/2026-09-18-local-admin-auth.md) separates approved requirements from proposed mechanisms. Dependency selection, password parameters, session and throttle limits, schema, offline mode transitions, legacy OIDC-session handling, key-response loss semantics and image packaging still require technical approval. None becomes accepted solely through this ADR. Where the shipped implementation diverges from the proposed mechanisms, the divergence is recorded in the [runbook](../operations/LOCAL_ADMIN.md) §13.4 rather than left implicit.

The [implementation plan](../superpowers/plans/2026-09-18-local-admin-auth.md) and [review findings](../superpowers/plans/2026-09-18-local-admin-auth-review.md) define the pending evidence. Still untested: the replica race tests against an isolated remote SurrealDB, a live identity provider, the `linux/amd64` image, and forced-order transaction interleavings. The [runbook](../operations/LOCAL_ADMIN.md) §13.2 lists every outstanding claim and what does cover it. This ADR does not claim those checks have passed.

## Relationships

This decision amends ADR-0052's OIDC-only browser control-plane choice for the planned local mode. Its External Identity and self-service rules continue to describe OIDC mode. In the shipped implementation, an enabled control plane requires exactly one browser authentication mode — `oidc` (the default) or `local`, selected by `MEMORY_MCP_HTTP_AUTH_MODE` — and neither mode exposes the other's authentication or administration routes.

[ADR-0057](0057-additive-browser-auth-methods.md) amends that last sentence: an enabled control plane now serves a **set** of methods (`oidc`, `local`), selected by `MEMORY_MCP_HTTP_AUTH_METHODS`, and each method's routes are mounted when it is enabled rather than instead of the other. The Alternatives entry below that rejected enabling local and OIDC together is superseded for the same reason.

ADR-0052's API-key transport, tenancy and HTTP boundaries remain in force. The reviewed proposal to revalidate key state on cache hits strengthens its authorization checks; exact invalidation guarantees require executed tests and documentation before release.

[ADR-0053](0053-explicit-http-storage-and-migration-composition.md) continues to govern explicit storage composition and release evidence. [ADR-0054](0054-capability-specific-control-registry-interfaces.md) remains superseded: this feature does not reinstate its speculative trait split. Any narrower interface must be justified by its actual consumers and tests.
