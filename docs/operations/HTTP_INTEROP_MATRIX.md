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
3. Drive each step:
   - **Discover**: `client.initialize()` followed by the modern
     `server/discover` exchange must return a non-empty capabilities
     block.
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
| `@modelcontextprotocol/sdk-python` | Not pinned | Streamable HTTP 2026-07-28 | Not executed | Not executed | Not executed | Not executed | Not executed — informational | |
| `@modelcontextprotocol/sdk-typescript` | Not pinned | Streamable HTTP 2026-07-28 | Not executed | Not executed | Not executed | Not executed | Not executed — informational | |
| `@modelcontextprotocol/sdk-go` | Not pinned | Streamable HTTP 2026-07-28 | Not executed | Not executed | Not executed | Not executed | Not executed — informational | |
| `@modelcontextprotocol/sdk-rust` | Not pinned | Streamable HTTP 2026-07-28 | Not executed | Not executed | Not executed | Not executed | Not executed — informational | |
| `claude-code` | Not pinned | Streamable HTTP 2026-07-28 | Not executed | Not executed | Not executed | Not executed | Not executed — informational | |
| `cursor` | Not pinned | Streamable HTTP 2026-07-28 | Not executed | Not executed | Not executed | Not executed | Not executed — informational | |
| `zed` | Not pinned | Streamable HTTP 2026-07-28 | Not executed | Not executed | Not executed | Not executed | Not executed — informational | |
| `inspector` | Not pinned | Streamable HTTP 2026-07-28 | Not executed | Not executed | Not executed | Not executed | Not executed — informational | |

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
