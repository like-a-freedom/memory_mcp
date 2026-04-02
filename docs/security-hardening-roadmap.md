# Security hardening roadmap

This note records the current hardening posture after the remediation waves and the remaining work needed before treating `memory_mcp` as a broadly deployable network service.

## 1. Query-surface inventory

### High-risk paths addressed in this pass

- `src/service/core.rs::resolve()`
  - previous risk: built a raw `UPDATE {entity_id} SET aliases = $aliases` string
  - remediation: removed the follow-up query because aliases are already persisted in the initial `create()` payload
- `src/storage.rs::select_table()`
  - previous risk: interpolated any caller-supplied table name into `SELECT * FROM {table}`
  - remediation: table names are now restricted to an internal allow-list before query construction

### Medium-risk paths still present

- `src/storage.rs::build_select_one_query()`
  - interpolates internal record/table identifiers for trusted record IDs
  - current assessment: acceptable for internally generated deterministic IDs, but should move to fully parameterized record access where the SDK permits it
- `src/storage.rs::{build_create_query, build_update_query, build_relate_edge_query}`
  - compose SurrealQL identifiers from internally generated record IDs and schema-owned field names
  - current assessment: lower risk because identifiers are not taken directly from end users, but still worth minimizing over time
- migration execution in `src/storage.rs::apply_migrations_impl()`
  - executes repository-owned `.surql` files verbatim
  - current assessment: acceptable because files are trusted code, and checksums now detect post-apply drift

## 2. Deployment model

### Local / embedded default

Recommended for developer workstations and single-user agent setups.

- use `SURREALDB_EMBEDDED=true`
- prefer the built-in RocksDB engine over a network-exposed database
- embedded SurrealDB is now initialized with `Capabilities::default()` instead of `Capabilities::all()`
- keep the process bound to stdio transport only

### Remote / shared deployment minimums

Before exposing the server to a remote SurrealDB instance or multi-user host:

- do **not** use root credentials for routine MCP traffic
- provision a dedicated least-privileged database user for this service
- isolate namespaces per tenant/scope and avoid wildcard cross-scope permissions
- keep authentication and authorization at the MCP host boundary as well as the database boundary
- document which SurrealDB functions/network targets are actually required before broadening capabilities

## 3. Remaining hardening work

- parameterize or otherwise constrain the remaining identifier-based query builders where possible
- define a concrete RBAC matrix for local dev, single-user desktop, and shared remote deployments
- document capability allow/deny choices explicitly if the project enables anything beyond `Capabilities::default()`
- add deployment-time checks that reject obvious unsafe combinations (for example: remote URL + root credentials + broad namespaces)
- add retry/backoff and failure classification for transient remote DB errors

## 4. Repository risk assessment

- **License / deployment assumption**: the repository assumes SurrealDB is acceptable for the intended deployment model; validate licensing and hosted-service assumptions before commercial remote rollout
- **Migration drift**: improved by checksum validation, but still dependent on disciplined review of migration files committed to Git
- **Compatibility**: current docs and runtime target stdio/local-first usage; remote production deployment needs stricter auth/RBAC guidance
- **Operational assumption**: deterministic IDs, request-path invalidation, and community updates are authoritative; background consolidation jobs remain intentionally deferred
