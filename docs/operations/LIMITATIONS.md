# Known Limitations

## v1 Limitations

1. **No tenant-scoped data export** — Data cannot be exported per-tenant. Full database export includes all tenants.

2. **No per-tenant restore** — Restore operations affect the entire SurrealDB instance, not individual tenants.

3. **Historical backup resurrection** — Restored data may include records marked as deleted before the snapshot timestamp. Deleted tenants are not automatically re-deleted after restore.

4. **Embedded SurrealDB profile warning** — The embedded SurrealDB profile is intended for development and testing only. It does not provide the isolation, durability, or performance characteristics required for production multi-tenant deployments.

5. **Namespace binding is never reused** — Once a Tenant's namespace binding is assigned, it is never reassigned to another Tenant, even after deletion. This prevents data leakage but means namespace values grow monotonically.

6. **No cross-tenant queries** — By design, tenants cannot query each other's data. This is enforced at the storage layer via namespace isolation.

7. **Session lifetime limits** — Browser sessions have absolute and idle expiry limits (configurable). Long-running operations may be interrupted by session expiry.

8. **API key secret shown once** — API key secrets are only displayed at creation time. If lost, a new key must be generated.

9. **Cross-provider vectors need an operator reembed** — In the HTTP profile, a tenant whose stored fact vectors were written by a *different* embedding provider (a changed `EMBEDDINGS_PROVIDER`, `EMBEDDINGS_MODEL`, `EMBEDDINGS_BASE_URL`, or effective dimension) stays on lexical/graph retrieval until an operator rewrites its vectors through the reembed route. The automatic backfill job does **not** apply to such a tenant: backfill only fills facts whose `embedding` is absent, and these facts already carry one. Automatic rewriting is deliberately not performed, because replacing every stored vector is a destructive, non-reversible operation on tenant data and is not a decision a background job should make.

10. **The stdio `reembed` command cannot reach an HTTP tenant** — `memory_mcp reembed` binds the Active Namespace selected at startup (`service::reembed` reads `self.active_namespace`), and an HTTP tenant lives in a per-tenant namespace provisioned through `SURREALDB_TENANT_*`. Running the stdio command against an HTTP deployment therefore rewrites whichever namespace the stdio binary was pointed at, not the tenant you are trying to repair. There is no CLI verb for HTTP tenants on either binary by design: `memory_mcp_http` exposes no `reembed` command, and operator automation is expected to use `POST /api/v1/operator/tenants/{id}/reembed`. Until an operator does, a degraded tenant stays degraded — the status line `embedding.reembed_required` in the tenant's logs is the signal, and backfill will not clear it.
