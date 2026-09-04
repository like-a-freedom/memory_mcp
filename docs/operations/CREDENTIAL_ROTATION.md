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

After rotating any key above, re-run the release-evidence script
so the new key material is exercised against the full HTTP gate
matrix:

```bash
# Re-validate the entire gate matrix after the rotation.
# Set MEMORY_MCP_HTTP_CREDENTIAL_ROTATION_TARGET so the external
# gate picks up evidence instead of `not_executed`.
MEMORY_MCP_HTTP_CREDENTIAL_ROTATION_TARGET=<deployment> \
    scripts/http_release_evidence.sh release
```

The control-plane test (`http_control_plane`) seeds a session via
the `MEMORY_MCP_HTTP_TEST_SEED_SESSION` env var, so it covers
the cookie/key rotation path. The conformance and isolation
tests re-provision tenants through `MEMORY_MCP_HTTP_TEST_BOOTSTRAP`
and exercise the rotated `MEMORY_MCP_API_KEY_PEPPER`. A successful
run produces a `target/http-release-evidence/<ts>/gates.tsv` row
with `result=pass` for every row, including `credential_rotation`.
