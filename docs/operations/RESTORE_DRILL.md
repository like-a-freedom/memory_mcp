# SurrealDB Restore Drill

This document covers the procedure for restoring from a SurrealDB
backup into a fresh deployment.

## Procedure

1. **Snapshot** the chosen remote SurrealDB deployment using its standard mechanism.

2. **Restore** into a fresh namespace pair (`control_restore`, `tenant_restore`).

3. **Boot** `memory_mcp_http` against the restored pair:
   ```bash
   MEMORY_MCP_SURREALDB_URL=surreal://... \
   MEMORY_MCP_SURREALDB_NS=control_restore \
   MEMORY_MCP_SURREALDB_DB=tenant_restore \
   cargo run --features streamable-http,control-plane --bin memory_mcp_http
   ```

4. **Provisioning workers** detect missing tenants; no resurrection of "deleted" Tenants is auto-attempted.

5. **Before opening ingress**, rotate:
   - API-key pepper (`MEMORY_MCP_API_KEY_PEPPER`)
   - OIDC identity-index key (`MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY`); require users to relink restored OIDC identities
   - Control Plane Session cookie/verifier key (`MEMORY_MCP_HTTP_SESSION_SIGNING_KEY`)
   - OIDC state and nonce keys (`MEMORY_MCP_HTTP_OIDC_STATE_KEY`, `MEMORY_MCP_HTTP_OIDC_NONCE_KEY`)
   - CSRF keys (`MEMORY_MCP_HTTP_CSRF_KEY`)

6. **Limitation**: Historical backups are immutable, so restored data may include data marked deleted before the snapshot. State this explicitly when communicating to stakeholders.

## Verification

After restore and rotation:
- Run the conformance suite: `cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_proto_conformance`
- Verify tenant isolation: `cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_isolation`
- Check that old API keys are rejected (pepper rotated)
- Confirm OIDC login requires re-linking
