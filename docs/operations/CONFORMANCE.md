# Protocol Conformance Coverage Map

This document lists every test in `http_proto_conformance.rs` with the
spec section it covers.

## Test Coverage

| Test | Spec Section | Description |
|------|--------------|-------------|
| `get_on_mcp_returns_405` | §3.1 | GET on /mcp returns 405 Method Not Allowed |
| `delete_on_mcp_returns_405` | §3.1 | DELETE on /mcp returns 405 Method Not Allowed |
| `disallowed_host_returns_403` | §3.1 | Request with disallowed Host header returns 403 |
| `disallowed_origin_returns_403` | §3.1 | Request with disallowed Origin header returns 403 |
| `health_live_returns_ok` | §17 | GET /health/live returns 200 OK |
| `health_ready_returns_json` | §17 | GET /health/ready returns JSON with status field |
| `no_mcp_session_id_header_is_set` | §3.1 | Response does not include an Mcp-Session-Id header |
| `server_discover_advertises_every_known_revision` | §3.1 | Discovery advertises both the modern and legacy revisions the endpoint serves |
| `legacy_initialize_negotiates_a_legacy_revision_without_a_session` | §3.1 | A legacy `initialize` is served, negotiates a handshake-era revision, and mints no session |
| `legacy_ping_is_served_on_the_legacy_era` | §3.1 | A legacy-shaped request carrying no per-request `_meta` is served |
| `both_eras_reach_the_same_tools` | §3.1 | Legacy and modern clients receive the same eight-tool surface |
| `legacy_tools_call_succeeds_without_the_modern_mirrored_headers` | §3.1 | A legacy `tools/call` executes without `Mcp-Method` or `Mcp-Name`, which are 2026-07-28 headers |
| `legacy_era_never_mints_a_session` | §3.1 | No legacy request sets a session or resume header |
| `legacy_forged_mcp_method_header_is_still_rejected` | §3.1 | A legacy body with a contradicting `Mcp-Method` is rejected |
| `legacy_request_cannot_claim_a_modern_revision` | §3.1 | A legacy-shaped body may not claim a revision that has no handshake |
| `unknown_modern_version_is_refused_with_the_supported_list` | §3.1 | An unknown `_meta` revision returns -32022 listing supported revisions |
| `modern_envelope_initialize_is_refused_without_naming_supported_versions` | §3.1 | Known diagnostic gap: a modern-envelope `initialize` returns -32601 with no supported list |
| `body_over_limit_returns_413` | §3.1 | Request body exceeding limit returns 413 |
| `missing_accept_returns_406` | §3.1 | Request without Accept header returns 406 |
| `header_body_mismatch_returns_header_mismatch_error` | §3.1 | Content-Type header/body mismatch returns error |
| `tools_call_requires_matching_mcp_name` | §3.1 | tools/call requires matching `Mcp-Name` tool header |
| `missing_mcp_method_returns_400_before_authentication` | §3.1 | Missing `Mcp-Method` is rejected before auth |
| `missing_mcp_name_returns_400_before_authentication` | §3.1 | Missing `Mcp-Name` is rejected before auth |
| `mismatched_mcp_name_returns_400` | §3.1 | Body and `Mcp-Name` mismatch returns 400 |
| `missing_protocol_version_returns_400` | §3.1 | Missing protocol version is rejected |
| `notification_returns_202_with_empty_body` | §3.1 | Accepted notification returns 202 with no body |
| `forged_subscription_header_cannot_bypass_preflight` | §3.1 | Subscription classification cannot bypass header/body validation |
| `removed_ping_method_is_not_available` | §3.1 | Removed `ping` method returns method-not-found |

## Running Conformance Tests

```bash
cargo test -p memory_mcp --features streamable-http,test-fixtures \
  --test http_proto_conformance -- --nocapture
```

## Control-plane UI asset packaging

The `streamable-http` profile embeds the compiled Dioxus 0.7 SPA from
inside the backend binary. Build the bundle with the matching Dioxus CLI and
provide its absolute output directory when compiling `memory_mcp`:

```bash
cd crates/ui
dx bundle --platform web --release --out-dir "$PWD/../../target/ui-dist"
cd ../..
MEMORY_MCP_UI_DIST="$PWD/target/ui-dist/public" \
  cargo build --release --features streamable-http
```

The directory must contain a non-empty `index.html`. Asset paths are sorted and
embedded at compile time; the backend does not read the directory at runtime or
fetch missing assets. Providing the bundle is optional: when
`MEMORY_MCP_UI_DIST` is absent the binary still compiles with an
empty UI catalog and does not serve a UI. If the variable is set but the bundle
is malformed (no `index.html`, a symlink, an invalid entry), the build fails
fast rather than silently shipping a UI-less image. The Dioxus CLI is not part
of the Rust workspace dependencies and must be installed separately.

## Notes

- Tests spawn the `memory_mcp_http` binary on an ephemeral port
- Each test is independent and cleans up after itself
- The bootstrap API key is `mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_conformancesuite0123456789abcdef`

## Test-fixtures builds

Binaries compiled with `--features test-fixtures` require `MEMORY_MCP_HTTP_TEST_BOOTSTRAP` to be set to a non-empty value, otherwise `HttpConfig::validate` returns `ConfigInvalid` at startup. Production builds (without `test-fixtures`) reject the same variable with the same error so the fixture cannot leak into a release image. The fault-injection variables `MEMORY_MCP_HTTP_TEST_FAULT_POINT` and `MEMORY_MCP_HTTP_TEST_FAULT_AT` are also feature-gated to `test-fixtures` and rejected in production.
