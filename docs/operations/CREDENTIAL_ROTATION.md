# Credential Rotation Runbook

## Environment Variables and Their Keys

| Variable | Purpose | Rotation Impact |
|----------|---------|-----------------|
| `MEMORY_MCP_API_KEY_PEPPER` | HMAC pepper for API key verification | Invalidates all existing API keys |
| `MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY` | Blind index key for OIDC subject verifiers | Requires OIDC identity relinking |
| `MEMORY_MCP_HTTP_SESSION_KEY` | HMAC key for browser-session cookie verifiers | Invalidates all browser sessions |
| `MEMORY_MCP_HTTP_OIDC_STATE_KEY` | OIDC state nonce encryption key | Invalidates in-flight login flows |
| `MEMORY_MCP_HTTP_OIDC_NONCE_KEY` | OIDC nonce encryption key | Invalidates in-flight login flows |
| `MEMORY_MCP_HTTP_CSRF_KEY` | CSRF token HMAC key | Invalidates all CSRF tokens |

## Rotation Order

Rotate in this order to minimize disruption:

1. **API-key pepper first** — invalidates restored/compromised keys. Users must generate new keys.
2. **OIDC identity-index key second** — requires restored OIDC identities to relink via the control plane.
3. **Browser/OIDC session keys** — users must re-login.
4. **CSRF keys** — invalidated alongside session rotation.

## Notes

- Rotation alone does not erase restored data. Data persists in SurrealDB regardless of key rotation.
- After rotation, verify with the conformance suite and isolation tests.
- Document the rotation in your change log with timestamp and reason.

## Verification

After rotating any key above, run the focused conformance and isolation checks
against the intended deployment:

```bash
# Re-validate the relevant control-plane and isolation behavior after rotation.
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_control_plane -- --test-threads=1
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_isolation -- --test-threads=1
```

The control-plane test seeds a session via its test fixture and exercises the
cookie/key path; the isolation test re-provisions tenants and exercises the
rotated API-key pepper. Record deployment and commit manually when this
runbook is used for a remote deployment.
