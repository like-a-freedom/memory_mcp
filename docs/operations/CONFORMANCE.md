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
| `server_discover_advertises_only_2026_07_28` | §3.1 | Server discover endpoint advertises only 2026-07-28 protocol |
| `unsupported_legacy_version_returns_400` | §3.1 | Request with unsupported protocol version returns 400 |
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

## Notes

- Tests spawn the `memory_mcp_http` binary on an ephemeral port
- Each test is independent and cleans up after itself
- The bootstrap API key is `mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_conformancesuite0123456789abcdef`
