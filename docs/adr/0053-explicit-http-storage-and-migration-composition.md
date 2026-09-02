# ADR-0053: Make HTTP storage and migration composition explicit

## Status

Accepted — 2026-09-02, architecture audit remediation.

## Context

The `test-fixtures` Cargo feature currently changes `memory_mcp_http` composition: `HttpState` selects an in-memory Registry and the provisioning scheduler selects `NoopMigrations`. Black-box commands that enable the feature therefore do not exercise the durable production Registry or tenant migration adapter. Cargo features are compile-time capability gates, not deployment configuration.

## Decision

1. `memory_mcp_http` always constructs production Registry and tenant migration adapters from validated `HttpConfig`.
2. `test-fixtures` exposes builders, deterministic bootstrap, and fault injectors only. Enabling it does not select storage or migrations.
3. In-memory adapters are selected only through explicit test composition values.
4. Registry and tenant migration catalogs remain distinct and append-only. Existing migration files and schema versions are unchanged.
5. Normal CI exercises durable embedded composition. Remote, multi-replica, restore, rotation, proxy, interoperability, and 500-tenant evidence are separate release gates.
6. Request handling never runs migrations and never accepts a namespace selector.

## Consequences

Tests must state which adapters they use. Production-like tests are slightly more expensive but prove the actual composition seam. External evidence remains pending until executed against a supported environment; documents cannot mark an unexecuted gate as passed.

## Relationships

This decision refines ADR-0011, ADR-0038, and ADR-0052. It does not change their migration, tenancy, or protocol semantics.
