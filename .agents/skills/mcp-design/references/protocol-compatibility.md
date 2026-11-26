# Dual-era protocol compatibility

Use when a server or client must support MCP `2026-07-28` and selected earlier revisions. Dual-era is a protocol and transport pattern; Tasks are optional and have additional [compatibility rules](tasks.md#dual-era-tasks-compatibility). Check the [versioning specification](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning), [2026 changes](https://modelcontextprotocol.io/specification/2026-07-28/changelog), each supported legacy revision, and the actual SDK and hosts. Older revisions differ: list the exact versions and transports supported, not simply “legacy”.

## Server admission and routing

- In `2026-07-28`, protocol interactions are stateless: requests carry version and client capabilities in per-request `_meta`, with no initialization handshake or protocol-level session. Application state can still live in storage or behind explicit handles; authenticate and authorize each request. Implement `server/discover`, but accept valid direct requests without requiring clients to call it first. Reject unsupported modern versions using `UnsupportedProtocolVersionError` and its supported-version list.
- For a legacy client, process `initialize`, negotiate an actual supported legacy revision, return that revision's capabilities, and handle `notifications/initialized` as specified. Associate later requests with the negotiated legacy context: stdio process or HTTP session as applicable. Do not respond to `initialize` with a modern no-handshake revision.
- Route by request metadata or the negotiated legacy context before enforcing era-specific capabilities, method availability, validation, and response shapes. A modern extension name in a legacy capability object does not enable modern behavior. Do not apply modern metadata requirements blindly to a valid legacy handshake or legacy rules to a modern request.
- Keep business operations shared where possible and isolate protocol-specific adapters at the boundary, including raw message handling when an SDK drops fields or methods. Verify what the SDK actually decodes, emits, and rejects. Never advertise a revision or feature solely because an SDK exposes a similarly named type.

## Transport and interaction differences

- **stdio:** verify newline-delimited JSON-RPC, legacy initialization, and modern per-request metadata independently.
- **Streamable HTTP:** apply the requested revision's header, validation, session, and streaming rules. For `2026-07-28`, check `MCP-Protocol-Version` against body metadata and enforce the applicable `Mcp-Method`/`Mcp-Name` headers; reject mismatches as specified. Do not require these modern-only headers from older clients. Check each legacy revision's version-header fallback, session and GET/SSE behavior against its own specification.
- In `2026-07-28`, ordinary results include `resultType: "complete"`; clients treating older responses without `resultType` as complete preserve legacy compatibility. Check the revision-specific availability of methods such as `ping` and `logging/setLevel`, and do not advertise unsupported ones.
- Modern `2026-07-28` uses request-scoped responses and multi-round-trip input instead of independent server-to-client JSON-RPC requests; subscriptions use `subscriptions/listen`. Earlier versions use their own server-to-client interaction, notification, cancellation, and streaming rules. Adapt supported workflows to the negotiated era rather than exposing modern messages to legacy clients.
- Scope authorization, state, and any stored operation identity to the authenticated caller across both paths. A compatibility adapter must not bypass validation, duplicate side effects, leak data across sessions, or make one era inherit another client's capabilities.

## Dual-era clients

- **stdio:** probe with `server/discover` before an era-ambiguous request. A discover result or recognized modern error selects modern behavior; on an unsupported version, select a supported one. An unrecognized error or timeout permits fallback to the legacy `initialize` handshake.
- **HTTP:** a client may try a modern request first. On HTTP 400 inspect the JSON-RPC body: recognized modern errors mean retry a supported version or fix the request; an empty or unrecognized body permits fallback to legacy initialization. Handle the deprecated `2024-11-05` HTTP+SSE binding separately only when support is required.
- Keep the selected legacy session context scoped to its connection or session. Do not silently interpret a modern protocol error as proof that the server is legacy. Test against modern-only, legacy-only, and dual-era servers.

## End-to-end evidence

For every claimed revision and transport, use representative real clients or wire-level requests through the deployed server boundary. For client implementations, also exercise discovery and fallback against servers of each supported era:

1. Complete the correct entry flow, list tools and run the same representative operation. Check the version, capability and result shapes; test a direct modern call without discovery.
2. Exercise missing, unsupported, and conflicting versions or metadata; ensure the correct error and no side effect. Test a mismatched capability and, for HTTP, mismatched headers and body.
3. Exercise applicable client interaction and notification/cancellation flows, auth isolation, and reconnect or session behavior. Test Tasks separately only when advertised.
4. Test concurrent legacy and modern HTTP clients on the same endpoint if supported; test stdio using the process/connection modes the implementation claims. Record exact versions, transports, SDKs, and gaps; do not infer support for an untested older revision from one passing legacy client.
