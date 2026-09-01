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
