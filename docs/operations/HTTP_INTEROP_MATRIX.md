# HTTP Interoperability Matrix

This matrix is the record of which MCP clients and SDKs have been
exercised against the `memory_mcp_http` deployment. Rows are populated
only by running real clients against a real deployment and recording
the outcome; they are not populated from code inspection.

Rows are optional compatibility notes for the current single-user project.
`Not executed` is an honest coverage state, not a release-blocking gate.

## How a row becomes `Pass`

1. Pick a pinned version (see the table) and check out that client in a
   local workspace directory of your choice.
2. Launch the in-tree test proxy from the same workspace root so
   the streaming claim is validated alongside the client behavior.
3. Drive each step, using the exchange the row's protocol era actually uses:
   - **Discover**: on a `2026-07-28` row, `server/discover` must return a
     non-empty capabilities block. On a `2025-11-25` row, `client.initialize()`
     must return a negotiated handshake-era `protocolVersion` and a usable
     capabilities block, and the response must carry no `Mcp-Session-Id`.
   - **Tool call**: `client.tools/call` with `ingest` and
     `assemble_context` must return a 200 with a valid envelope.
   - **Notification**: `client.sendNotification` must return 202
     with an empty body.
   - **SSE final response**: the streamed `data:` line must echo
     the request id.
4. Record the version and evidence path on the row.

## Matrix

| Client/SDK | Exact version | Protocol | Discover | Tool call | Notification | SSE final response | Result | Evidence |
|---|---|---|---|---|---|---|---|---|
| `zed` | Not pinned | Streamable HTTP `2025-11-25` (legacy) | Not executed | Not executed | Not executed | Not executed | Not executed — informational | |
| `claude-code` | Not pinned | Streamable HTTP `2025-11-25` (legacy) | Not executed | Not executed | Not executed | Not executed | Not executed — informational | |
| `cursor` | Not pinned | Streamable HTTP `2025-11-25` (legacy) | Not executed | Not executed | Not executed | Not executed | Not executed — informational | |
| `@modelcontextprotocol/sdk-python` 1.30.0 | Not pinned | Streamable HTTP `2025-11-25` (legacy) | Not executed | Not executed | Not executed | Not executed | Not executed — informational | |
| `@modelcontextprotocol/sdk-typescript` 1.31.0 | Not pinned | Streamable HTTP `2025-11-25` (legacy) | Not executed | Not executed | Not executed | Not executed | Not executed — informational | |
| `mcp` (PyPI) 2.2.0 | Not pinned | Streamable HTTP `2026-07-28` (modern) | Not executed | Not executed | Not executed | Not executed | Not executed — informational | |
| `@modelcontextprotocol/client` 2.2.0 | Not pinned | Streamable HTTP `2026-07-28` (modern) | Not executed | Not executed | Not executed | Not executed | Not executed — informational | |
| `@modelcontextprotocol/sdk-go` 1.8.0 | Not pinned | Streamable HTTP `2026-07-28` (modern) | Not executed | Not executed | Not executed | Not executed | Not executed — informational | |
| `inspector` | Not pinned | Streamable HTTP `2026-07-28` (modern) | Not executed | Not executed | Not executed | Not executed | Not executed — informational | |

Rows are split by the protocol era the client actually speaks, because the
endpoint serves both ([ADR-0071](../adr/0071-dual-era-mcp-http-profile.md)).
A legacy client cannot be validated by a modern row: `Discover` for a legacy
client means `initialize` followed by a `tools/list` on the negotiated revision,
not the `server/discover` exchange. A modern-only client will not be discovered
by a legacy row either.

This split is not cosmetic. As of 2026-10-02, of 72 responding endpoints in a
public-registry probe, 7 (9.7%) spoke `2026-07-28`; the most-installed SDK
packages (`mcp` 1.30.0, `@modelcontextprotocol/sdk` 1.31.0) are still on the
`2025-11-25` line. Serving only the modern revision excludes most of the
installed base.

## Updating a row

When the interop runner executes a client against a deployed
`memory_mcp_http` instance, the runner writes a row with:

- the pinned version of the client (commit hash for source builds,
  semver for tagged releases);
- the protocol header used during the run (`2026-07-28` is the
  modern profile);
- per-step pass/fail and an evidence path under
  `docs/operations/interop-evidence/<client>/<date>/`.
- a `Pass` / `Fail` in the `Result` column. A `Fail` records a compatibility
  issue for follow-up; it does not block the single-user project release.

Keep the pinned client version and evidence path next to each manually tested row.
