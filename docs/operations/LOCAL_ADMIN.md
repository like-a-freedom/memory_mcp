# Local Admin Authentication — Operations Runbook

> **SUPERSEDED — do not follow this document.**
>
> This file predates the implementation and contains instructions that do not
> match the shipped code. It is retained only for history. The authoritative
> runbook is [`LOCAL_ADMIN_AUTH.md`](LOCAL_ADMIN_AUTH.md).
>
> Known false or unsafe statements in the text below:
>
> - `memory_mcp admin create ops.one` (positional) — the real CLI requires
>   `--username`.
> - The route `/api/v1/local/admin/challenge` — the real route is
>   `POST /api/v1/auth/local/challenge`.
> - `SameSite=Lax` — the real cookies are `SameSite=Strict`.
> - "printable ASCII" passwords — the real rule is 15–128 Unicode scalar values
>   with no NUL.
> - `SURREALDB_URL` / `SURREALDB_DB_NAME` for the HTTP profile — it reads
>   `SURREALDB_CONTROL_*` and `SURREALDB_TENANT_*`.

## Overview

Local admin authentication provides a self-contained admin access mode for
single-tenant or air-gapped deployments. It uses Argon2id password hashing,
database-backed sessions, and one-time challenge codes for admin provisioning.

## Configuration

Set environment variables before starting the HTTP server:

```bash
# Required: enable local admin mode
MEMORY_MCP_HTTP_AUTH_MODE=local

# Required: 32-byte hex keys for session and CSRF cookies
MEMORY_MCP_HTTP_SESSION_KEY=<64-char-hex>
MEMORY_MCP_HTTP_CSRF_KEY=<64-char-hex>

# SurrealDB connection (existing)
SURREALDB_URL=mem://
SURREALDB_NAMESPACE=main
SURREALDB_DB_NAME=memory_mcp
```

### Key Generation

Generate secure keys:

```bash
# Session key
openssl rand -hex 32

# CSRF key
openssl rand -hex 32
```

## Admin Provisioning

### Create First Admin

```bash
memory_mcp admin create ops.one
```

Output: a one-time activation code. Share this securely with the admin.

### Admin Activation

The admin visits `/api/v1/local/admin/challenge` with the activation code
and sets their password. The code is valid for 900 seconds (15 minutes).

### Password Requirements

- Minimum 15 characters, maximum 128 characters
- Must contain only printable ASCII characters
- No null bytes

### Username Requirements

- 3-64 characters
- Must start with a lowercase letter or digit
- May contain lowercase letters, digits, `.`, `_`, `-`
- Leading/trailing whitespace is trimmed

## Recovery

If an admin loses their password:

```bash
memory_mcp admin recover ops.one
```

This revokes all active sessions and issues a reset code. The admin uses
the reset code to set a new password.

## Session Management

- Sessions are stored server-side in SurrealDB
- Idle timeout: 30 minutes
- Absolute timeout: 24 hours
- Sessions are invalidated on password change or recovery
- Cookie: `__Host-memory_mcp_admin=<hex>` (Secure, HttpOnly, SameSite=Lax)

## Rate Limiting

- Maximum 2 concurrent KDF operations (Argon2id)
- Maximum 8 queued KDF operations
- Admission timeout: 2 seconds
- Failed login attempts are logged for audit

## Migration

The `047_local_admin_auth.surql` migration creates 8 tables:

- `browser_auth_policy` — singleton policy fence
- `local_admin` — admin credentials and state
- `local_admin_challenge` — one-time activation/reset codes
- `local_admin_session` — server-side sessions
- `local_admin_rate_bucket` — rate limiting
- `local_admin_client` — local workflow clients
- `local_admin_operation` — idempotency tracking
- `local_admin_audit` — audit trail

## Troubleshooting

### "key_fingerprint_mismatch" on startup

The session/CSRF keys changed since the policy was created. Either:
1. Use the original keys (check deployment records)
2. Reset the `browser_auth_policy` table (requires offline maintenance)

### "challenge_expired" during activation

The one-time code expired (15-minute TTL). Generate a new one:

```bash
memory_mcp admin create <username>
```

### "unauthenticated" on valid session

Possible causes:
1. Session exceeded idle timeout (30 min)
2. Session exceeded absolute timeout (24 hr)
3. Admin password was changed (all sessions invalidated)
4. Admin was recovered (all sessions invalidated)

### KDF saturation (503 errors)

Too many concurrent login attempts. The KDF semaphore allows 2 running
and 8 queued operations. Wait a few seconds and retry.
