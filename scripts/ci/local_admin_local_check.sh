#!/bin/sh
# End-to-end local-admin check against the real binaries and a real
# RocksDB-backed control registry. The server and the CLI cannot hold the
# same embedded RocksDB path at once, so the server is stopped while the
# CLI issues a code.
set -e

REPO=/Users/solovey/Documents/dev/memory_mcp
ROOT=/tmp/lmcp_local_check
CLI="$REPO/target/debug/memory_mcp"
HTTP="$REPO/target/debug/memory_mcp_http"
BASE=http://127.0.0.1:18443
HOST_HEADER="Host: localhost"
ORIGIN_HEADER="Origin: https://localhost:8443"

# The check requires a registry with no administrator yet, so it resets the
# embedded RocksDB directories by default. Set LMCP_CHECK_RESET=0 to inspect
# the failure state of a previous run in place.
RESET=${LMCP_CHECK_RESET:-1}

# The check drives the real binaries, so build them with the production
# feature set. `test-fixtures` is deliberately excluded: it switches the HTTP
# binary into a bootstrap mode that extra environment variables must unlock.
echo "=== 0. build the binaries under test ==="
( cd "$REPO" && cargo build --locked \
    --features fs-watch,mcp-apps,streamable-http,control-plane )

. "$ROOT/env"

start_server() {
    # Each instance appends to its own log; the shared `out.log` is
    # truncated on every start, which would destroy the evidence from the
    # instance that actually handled a request.
    ( set -a; . "$ROOT/env"; set +a; "$HTTP" >> "$ROOT/instance.log" 2>&1 &
      echo $! > "$ROOT/pid" )
    i=0
    while [ "$i" -lt 40 ]; do
        if curl -sS -m 2 -o /dev/null -H "$HOST_HEADER" "$BASE/health/ready" 2>/dev/null; then
            return 0
        fi
        sleep 1
        i=$((i + 1))
    done
    echo "server did not become ready" >&2
    tail -20 "$ROOT/instance.log" >&2
    return 1
}

stop_server() {
    if [ -f "$ROOT/pid" ]; then
        kill "$(cat "$ROOT/pid")" 2>/dev/null || true
        wait "$(cat "$ROOT/pid")" 2>/dev/null || true
        rm -f "$ROOT/pid"
    fi
    sleep 2
}

echo "=== 1. issue an activation code (CLI, server stopped) ==="
stop_server
if [ "$RESET" = "1" ]; then
    rm -rf "$ROOT/control" "$ROOT/tenant"
fi
CREATE_JSON=$(set -a; . "$ROOT/env"; set +a; "$CLI" admin create --username ops.one)
echo "$CREATE_JSON"
CODE=$(printf '%s' "$CREATE_JSON" | sed -n 's/.*"code": "\([0-9a-f]*\)".*/\1/p')
echo "code length: ${#CODE}"

echo "=== 2. start the server on the same registry ==="
start_server
curl -sS -H "$HOST_HEADER" "$BASE/api/v1/auth/config"; echo

echo "=== 3. activate through HTTP ==="
PREAUTH=$(curl -sS -D "$ROOT/p.h" -c "$ROOT/cj" -H "$HOST_HEADER" "$BASE/api/v1/auth/local/csrf")
TOKEN=$(printf '%s' "$PREAUTH" | sed -n 's/.*"csrf_token":"\([0-9a-f]*\)".*/\1/p')
COOKIE=$(grep -i '^set-cookie: __Host-memory_mcp_admin_preauth' "$ROOT/p.h" | sed 's/^[Ss]et-[Cc]ookie: //' | cut -d';' -f1)
curl -sS -o /dev/null -w 'activate HTTP %{http_code}\n' -X POST \
  -H "$HOST_HEADER" -H "$ORIGIN_HEADER" -H 'Content-Type: application/json' \
  -H "X-CSRF-Token: $TOKEN" -H "Cookie: $COOKIE" \
  --data "{\"code\":\"$CODE\",\"password\":\"a sufficiently long passphrase\"}" \
  "$BASE/api/v1/auth/local/activate"

echo "=== 4. login ==="
PREAUTH=$(curl -sS -D "$ROOT/p2.h" -c "$ROOT/cj2" -H "$HOST_HEADER" "$BASE/api/v1/auth/local/csrf")
TOKEN=$(printf '%s' "$PREAUTH" | sed -n 's/.*"csrf_token":"\([0-9a-f]*\)".*/\1/p')
COOKIE=$(grep -i '^set-cookie: __Host-memory_mcp_admin_preauth' "$ROOT/p2.h" | sed 's/^[Ss]et-[Cc]ookie: //' | cut -d';' -f1)
curl -sS -o /dev/null -D "$ROOT/login.h" -w 'login HTTP %{http_code}\n' -X POST \
  -H "$HOST_HEADER" -H "$ORIGIN_HEADER" -H 'Content-Type: application/json' \
  -H "X-CSRF-Token: $TOKEN" -H "Cookie: $COOKIE" \
  --data '{"username":"ops.one","password":"a sufficiently long passphrase"}' \
  "$BASE/api/v1/auth/local/login"
SESSION=$(grep -i '^set-cookie: __Host-memory_mcp_admin=' "$ROOT/login.h" | sed 's/^[Ss]et-[Cc]ookie: //' | cut -d';' -f1)
echo "session cookie acquired: $(printf '%s' "$SESSION" | cut -c1-40)..."
echo "cleared preauth cookie present: $(grep -ci 'Max-Age=0' "$ROOT/login.h")"

echo "=== 5. session + CSRF ==="
curl -sS -H "$HOST_HEADER" -H "Cookie: $SESSION" "$BASE/api/v1/admin/session" > "$ROOT/session.json"
cat "$ROOT/session.json"; echo
SCSRF=$(sed -n 's/.*"csrf_token":"\([0-9a-f]*\)".*/\1/p' "$ROOT/session.json")

echo "=== 6. create a client ==="
curl -sS -D "$ROOT/create.h" -w '\ncreate HTTP %{http_code}\n' -X POST \
  -H "$HOST_HEADER" -H "$ORIGIN_HEADER" -H 'Content-Type: application/json' \
  -H "X-CSRF-Token: $SCSRF" -H "Cookie: $SESSION" \
  -H "Idempotency-Key: 11111111-1111-4111-8111-111111111111" \
  --data '{"display_name":"team-alpha"}' \
  "$BASE/api/v1/admin/clients"
grep -i '^location:' "$ROOT/create.h" || true

echo "=== 7. list clients ==="
curl -sS -H "$HOST_HEADER" -H "Cookie: $SESSION" "$BASE/api/v1/admin/clients"; echo

echo "=== 7b. provisioning reaches ready ==="
ACCOUNT_ID=$(sed -n 's/.*"account_id":"\([^"]*\)".*/\1/p' "$ROOT/session.json" 2>/dev/null || true)
ACCOUNT_ID=$(printf '%s' "${ACCOUNT_ID:-}")
if [ -z "$ACCOUNT_ID" ]; then
    ACCOUNT_ID=$(curl -sS -H "$HOST_HEADER" -H "Cookie: $SESSION" "$BASE/api/v1/admin/clients" \
        | sed -n 's/.*"account_id":"\([^"]*\)".*/\1/p')
fi
echo "account: $ACCOUNT_ID"
i=0
STATUS=""
while [ "$i" -lt 45 ]; do
    STATUS=$(curl -sS -H "$HOST_HEADER" -H "Cookie: $SESSION" \
        "$BASE/api/v1/admin/clients/$ACCOUNT_ID" \
        | sed -n 's/.*"tenant_status":"\([^"]*\)".*/\1/p')
    if [ "$STATUS" = "ready" ]; then
        break
    fi
    sleep 2
    i=$((i + 1))
 done
echo "tenant_status after ${i} polls: $STATUS (want ready)"

echo "=== 7c. an issued key authenticates the data plane ==="
# Issue a key through the admin API (the secret is captured in a shell
# variable and never printed), then use it against `/mcp` twice: once while
# it is live, and once after it has been revoked.
KEY_JSON=$(curl -sS -X POST \
  -H "$HOST_HEADER" -H "$ORIGIN_HEADER" -H 'Content-Type: application/json' \
  -H "X-CSRF-Token: $SCSRF" -H "Cookie: $SESSION" \
  -H 'Idempotency-Key: 22222222-2222-4222-8222-222222222222' \
  --data '{"name":"e2e-probe","expiry":{"kind":"never"}}' \
  "$BASE/api/v1/admin/clients/$ACCOUNT_ID/keys")
KEY_ID=$(printf '%s' "$KEY_JSON" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
SECRET=$(printf '%s' "$KEY_JSON" | sed -n 's/.*"secret":"\([^"]*\)".*/\1/p')
echo "key issued: id=$KEY_ID secret_length=${#SECRET}"

MCP_BODY='{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}'
mcp_call() {
    curl -sS -o /dev/null -w '%{http_code}' -X POST \
      -H "$HOST_HEADER" -H 'Content-Type: application/json' \
      -H 'Accept: application/json, text/event-stream' \
      -H 'MCP-Protocol-Version: 2026-07-28' -H 'Mcp-Method: tools/list' \
      ${1:+-H "Authorization: Bearer $1"} \
      --data "$MCP_BODY" "$BASE/mcp"
}
echo "mcp without a credential: $(mcp_call '')"
mcp_detail() {
    curl -sS -X POST \
      -H "$HOST_HEADER" -H 'Content-Type: application/json' \
      -H 'Accept: application/json, text/event-stream' \
      -H 'MCP-Protocol-Version: 2026-07-28' -H 'Mcp-Method: tools/list' \
      ${1:+-H "Authorization: Bearer $1"} \
      --data "$MCP_BODY" "$BASE/mcp" | head -c 120
}
echo "mcp with the issued key: $(mcp_call "$SECRET") body=$(mcp_detail "$SECRET")"
curl -sS -o /dev/null -w 'revoke HTTP %{http_code}\n' -X DELETE \
  -H "$HOST_HEADER" -H "$ORIGIN_HEADER" \
  -H "X-CSRF-Token: $SCSRF" -H "Cookie: $SESSION" \
  "$BASE/api/v1/admin/clients/$ACCOUNT_ID/keys/$KEY_ID"
echo "mcp after revocation: $(mcp_call "$SECRET")"

echo "=== 8. negative checks ==="
curl -sS -o /dev/null -w 'no-CSRF client create HTTP %{http_code} (want 403)\n' -X POST \
  -H "$HOST_HEADER" -H "$ORIGIN_HEADER" -H 'Content-Type: application/json' \
  -H "Cookie: $SESSION" --data '{"display_name":"nope"}' "$BASE/api/v1/admin/clients"
curl -sS -o /dev/null -w 'wrong-origin login HTTP %{http_code} (want 403)\n' -X POST \
  -H "$HOST_HEADER" -H 'Origin: https://evil.example' -H 'Content-Type: application/json' \
  -H "Cookie: $COOKIE" --data '{"username":"ops.one","password":"a sufficiently long passphrase"}' \
  "$BASE/api/v1/auth/local/login"
curl -sS -o /dev/null -w 'oidc route HTTP %{http_code} (want 404)\n' -H "$HOST_HEADER" "$BASE/auth/oidc/authorize"
curl -sS -o /dev/null -w 'operator route HTTP %{http_code} (want 404)\n' -H "$HOST_HEADER" "$BASE/api/v1/operator/recovery/status"
curl -sS -o /dev/null -w 'unmatched api HTTP %{http_code} (want 404)\n' -H "$HOST_HEADER" "$BASE/api/v1/nope"

echo "=== 9. persistence across restart ==="
stop_server
start_server
curl -sS -H "$HOST_HEADER" -H "Cookie: $SESSION" -w '\nsession after restart HTTP %{http_code}\n' \
  "$BASE/api/v1/admin/session"
curl -sS -H "$HOST_HEADER" -H "Cookie: $SESSION" "$BASE/api/v1/admin/clients"; echo

echo "=== 10. plan mismatch is rejected at startup ==="
stop_server
( set -a; . "$ROOT/env"; set +a; MEMORY_MCP_HTTP_MAX_ACTIVE_API_KEYS=99 "$HTTP" > "$ROOT/mismatch.log" 2>&1 ) || true
grep -ci 'plan_limit_mismatch' "$ROOT/mismatch.log" || true
tail -3 "$ROOT/mismatch.log"

echo "=== 11. recovery invalidates the session ==="
stop_server
RECOVER_JSON=$(set -a; . "$ROOT/env"; set +a; "$CLI" admin recover --username ops.one)
echo "$RECOVER_JSON"
start_server
curl -sS -o /dev/null -w 'old session after recovery HTTP %{http_code} (want 401)\n' \
  -H "$HOST_HEADER" -H "Cookie: $SESSION" "$BASE/api/v1/admin/session"

echo "=== 12. provisioning evidence ==="
# The provisioning worker logs namespace creation as `op=schema.init`; there is
# no line containing the literal word "provision" on the tenant path.
grep -icE "schema\.init|provisioning" "$ROOT/instance.log" || true
grep -iE "schema\.init|provisioning" "$ROOT/instance.log" | head -20 || true

stop_server
echo "=== done ==="
