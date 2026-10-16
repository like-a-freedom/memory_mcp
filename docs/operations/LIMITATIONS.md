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

11. **OIDC availability and trust limits** — A cold process without verification keys cannot validate OIDC credentials. Fresh JWKS remain usable during provider outages for at most 300 seconds from the last successful fetch; failed refreshes never extend this deadline. Expired keys are refused, and a successful replacement removes omitted keys immediately. Discovery metadata stays fixed for the process lifetime after validation: restart after issuer, endpoints or signing-policy changes. There is no persistent cache or stale-key grace period.

OIDC discovery/JWKS failures do not terminate HTTP or gate overall readiness. `/health/ready` includes `oidc.enabled`, `oidc.metadata` (`disabled`, `initializing`, `ready`, `backoff`, `invalid_response`) and `oidc.jwks` (`disabled`, `empty`, `fresh`, `stale`), without making provider requests. Health availability does not promise OIDC sign-in availability.

Provider-dependent authentication returns `503 temporarily_unavailable`, `Cache-Control: no-store` and a positive `Retry-After` when verification cannot proceed. Invalid credentials return `401` (bearer requests include the authentication challenge). API keys and valid established sessions remain independent; their normal expiry, revocation and policy checks still apply. A token exchange is sent once: after timeout or provider failure, restart sign-in with a new authorization flow rather than replaying the consumed code.

Requests use a two-second connect timeout, five-second total timeout, no redirects and a 256 KiB streaming body limit. Discovery/JWKS retries use exponential equal jitter (1–60 seconds); `429`/`503` Retry-After can extend this to 300 seconds. Invalid metadata and other non-transient responses retry after 60 seconds. JWKS refreshes share one gate and a minimum one-second cooldown; proactive refresh runs at 240–270 seconds. Provider response bodies, tokens and credentials are never diagnostics.
