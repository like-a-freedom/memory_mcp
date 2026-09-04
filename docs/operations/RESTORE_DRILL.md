# SurrealDB Restore Drill

This document covers the procedure for restoring from a SurrealDB
backup into a fresh deployment.

## Procedure

1. **Snapshot** the chosen remote SurrealDB deployment using its standard mechanism.

2. **Restore** into a fresh namespace pair (`control_restore`, `tenant_restore`).

3. **Boot** `memory_mcp_http` against the restored pair. The HTTP profile
   requires separate control and tenant bindings; use remote `ws://`/`wss://`
   targets or the documented non-production `rocksdb://` profile:
   ```bash
   SURREALDB_CONTROL_URL=wss://.../rpc \
   SURREALDB_CONTROL_USERNAME=... \
   SURREALDB_CONTROL_PASSWORD=... \
   SURREALDB_CONTROL_NAMESPACE=control_restore \
   SURREALDB_CONTROL_DB=registry_restore \
   SURREALDB_TENANT_URL=wss://.../rpc \
   SURREALDB_TENANT_USERNAME=... \
   SURREALDB_TENANT_PASSWORD=... \
   SURREALDB_TENANT_NAMESPACE=tenant_restore \
   SURREALDB_TENANT_DB=tenant_restore \
   MEMORY_MCP_HTTP_PUBLIC_BASE_URL=https://mcp.example.com \
   ALLOWED_HOSTS=mcp.example.com \
   ALLOWED_ORIGINS=https://mcp.example.com \
   MEMORY_MCP_API_KEY_PEPPER=... \
   MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY=... \
   MEMORY_MCP_HTTP_SESSION_KEY=... \
   MEMORY_MCP_HTTP_OIDC_STATE_KEY=... \
   MEMORY_MCP_HTTP_OIDC_NONCE_KEY=... \
   MEMORY_MCP_HTTP_CSRF_KEY=... \
   MEMORY_MCP_HTTP_SIGNUP_MODE=invite_only \
   cargo run --locked --features streamable-http,control-plane --bin memory_mcp_http
   ```

4. **Provisioning workers** detect missing tenants; no resurrection of "deleted" Tenants is auto-attempted.

5. **Before opening ingress**, rotate:
   - API-key pepper (`MEMORY_MCP_API_KEY_PEPPER`)
   - OIDC identity-index key (`MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY`); require users to relink restored OIDC identities
   - Control Plane Session cookie/verifier key (`MEMORY_MCP_HTTP_SESSION_KEY`)
   - OIDC state and nonce keys (`MEMORY_MCP_HTTP_OIDC_STATE_KEY`, `MEMORY_MCP_HTTP_OIDC_NONCE_KEY`)
   - CSRF keys (`MEMORY_MCP_HTTP_CSRF_KEY`)

6. **Limitation**: Historical backups are immutable, so restored data may include data marked deleted before the snapshot. State this explicitly when communicating to stakeholders.

## Verification

After restore and rotation:
- Run the conformance suite: `cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_proto_conformance`
- Verify tenant isolation: `cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_isolation`
- Check that old API keys are rejected (pepper rotated)
- Confirm OIDC login requires re-linking

## Verification with the release gate

The release-evidence script re-validates the restore path against
the full HTTP gate matrix. After restoring the database and
rotating the keys:

```bash
# Re-run the entire gate matrix against the restored deployment.
# Set MEMORY_MCP_HTTP_RESTORE_DRILL_DB to the restored target so
# the external gate picks up evidence instead of `not_executed`.
MEMORY_MCP_HTTP_RESTORE_DRILL_DB=<target-db> \
    scripts/http_release_evidence.sh release
```

The script also exercises the new control-plane test suite
(`http_control_plane`) which seeds an authenticated session
through the `MEMORY_MCP_HTTP_TEST_SEED_SESSION` env var and drives
`/api/v1/account/*` end to end. A successful run produces a
`target/http-release-evidence/<ts>/gates.tsv` row with
`result=pass` for every row, including `restore_drill`.
