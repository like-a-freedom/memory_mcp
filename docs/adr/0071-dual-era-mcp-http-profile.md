# ADR-0071: Serve the Streamable HTTP profile as a dual-era MCP server

## Status

Accepted; supersedes the "Modern-only MCP transport" section of ADR-0052.

## Context

ADR-0052 built the HTTP profile on MCP `2026-07-28` alone, and explicitly
declined `rmcp`'s compatibility paths because dual-era behavior "is optional,
not required by the deprecation policy, and would add session ownership,
storage, lifecycle, and security complexity to a new service."

Two of those premises did not survive contact with the evidence.

The first is the deprecation policy. MCP does have one, and it guarantees a
deprecated feature at least twelve months before removal — but it covers Roots,
Sampling, Logging, Dynamic Client Registration, and the legacy HTTP+SSE
transport. An entire protocol revision is not on that clock. A `2025-11-25`
client is not a feature awaiting removal; it is a peer revision that the
specification's own versioning page says a server should interoperate with.

The second is the session cost. The legacy revisions say a server **MAY** assign
an `Mcp-Session-Id`; the client obligation to echo it is conditional on the
server having returned one. `rmcp` is stricter than the spec — it routes any
`initialize` through the session path when `legacy_session_mode` is on — but the
alternative it offers, `legacy_session_mode(false)`, serves `initialize` through
the same stateless path as modern requests. So the complexity ADR-0052 avoided
was never required in the first place.

The decision was also made without a single measurement. `HTTP_INTEROP_MATRIX.md`
records eight clients and four steps, every cell `Not executed`, every version
`Not pinned`, no evidence, and no test or CI job that reads it.

What the measurements show, as of 2026-10-02:

- Of 72 responding endpoints in a public-registry probe, 7 (9.7%) spoke
  `2026-07-28`. All five prominent npm MCP servers sat on `2025-06-18` and
  answered `server/discover` with method-not-found.
- The two most-installed packages, `@modelcontextprotocol/sdk` 1.31.0 and `mcp`
  1.30.0, are still on the `2025-11-25` line. `2026-07-28` support lives in the
  v2 lines (`@modelcontextprotocol/client` 2.2.0, `mcp` 2.2.0, `go-sdk` 1.8.0).
- Zed v1.22.0 carries no `2026-07-28` string in `crates/context_server/src` and
  has no `rmcp` dependency at all.
- Every production precedent we surveyed serves both on one listener: MCPG,
  mcp-hub, Kuadrant mcp-gateway, MCP-Nest (dual by default, modern-only as an
  opt-in), Cloudflare.

The cost of getting this wrong was paid in a real incident. A Zed client
speaking `2025-11-25` was refused at preflight with `400 HeaderMismatch:
protocol version` before authentication, so the API key was never looked up and
`last_used_at` stayed null. Zed routes the error body to a debug-only channel and
leaves the pending request unresolved, so the user saw a 60-second timeout and
reasonably concluded the network or the key was at fault.

## Decision

### One endpoint, both eras, no configuration

`POST /mcp` serves both eras unconditionally. There is no environment variable,
no feature flag, and no second endpoint. A client selects its era by how it
opens, which is what the specification's compatibility matrix prescribes: a
request carrying per-request `_meta` is served statelessly per `2026-07-28`, and
an `initialize` request is answered under a negotiated legacy revision.

A "legacy profile" or "compat mode" is deliberately not a concept here. A
profile is something an operator turns on; a dual-era server has nothing to
turn on. Any user who had to configure anything to reach the server would be
evidence the contract is wrong.

### The era is read from the body, never from a header

Preflight classifies a request by whether the body carries
`params._meta["io.modelcontextprotocol/protocolVersion"]`. `ValidatedMcpRequest`
— the extension that drives the admission class — is built from the same body on
both paths.

This is what keeps ADR-0052's security property intact. A client cannot present a
header that disagrees with the request `rmcp` dispatches on, so a forged
`Mcp-Method: subscriptions/listen` still cannot buy the subscription admission
class, its exempt deadline, or its released runtime pin.

For modern-shaped requests the mirrored headers must agree with the body
(`SEP-2243`); for legacy-shaped requests they are optional — a legacy client has
never heard of `Mcp-Method` or `Mcp-Name`, which are `2026-07-28` headers — and
one that is present but contradicting is rejected. Making this asymmetry
concrete matters: a first implementation made `Mcp-Method` era-aware and left
`Mcp-Name` unconditional, which refused every legacy `tools/call` with the very
`HeaderMismatch` this ADR exists to remove. The mirrored headers are modern-era
constructs; neither is required from a legacy client.

### Sessions stay stateless

`with_legacy_session_mode` remains `false` and `NeverSessionManager` remains in
place, so no session store, sticky routing, or session lifecycle is introduced.
GET and DELETE continue to return `405`, which is what the specification
requires of a server that offers no SSE stream — not a limitation we chose.

The flag is easy to misread, so it is worth recording: `legacy_session_mode`
selects **sessions**, not legacy support. Setting it to `true` would route every
legacy `initialize` into a session path that `NeverSessionManager` cannot serve.

### `supported_protocol_versions` keeps advertising every known revision

This is the one place where the obvious change is actively wrong.

`rmcp` uses the returned list as a hard membership check against every
per-request `_meta.protocolVersion`, before dispatch, with no fallback. Narrowing
it to the legacy revisions would therefore reject **modern** requests with
`-32022`, including `server/discover` — a server that serves neither era. The
narrowing needed for the handshake happens inside `rmcp`'s
`negotiate_protocol_version`, which already refuses to answer `initialize` with
a revision that has no handshake.

### Considered options

**Keep modern-only and return a clear error.** Cheapest, and closer to ADR-0052.
Rejected: for a public service, refusing the era that 9.7% of measured endpoints
and the majority of installed clients use is refusing most of the audience. The
clarity improvement is real but does not make the service usable.

**A separate `/memory/mcp/legacy` endpoint.** Preserves the modern contract
exactly. Rejected: the specification expects one endpoint to serve both eras, and
a user who must know which URL to use is a user we have not served.

**An environment switch, defaulting to modern.** Rejected: with a public
deployment, one switch means every user's reachability depends on configuration
we do not control, and the modern-only default reproduces the original incident
for anyone who forgets it.

**A gateway in front of the server** (MCPG, mcp-hub, Kuadrant). The market
preference, and it works with servers that cannot be changed. Rejected here: it
adds a second deployment that must hold the API key and the client session, and
it is the wrong place to pay for a capability the server can supply itself in
one flag.

**Per-era capability advertisement.** rmcp already gates era-specific methods
(`ping` and `resources/subscribe` legacy-only, `subscriptions/listen`
modern-only, `initialize` ungated). We declare capabilities accurately and let
that gating stand rather than advertising a capability a revision cannot deliver.

## Consequences

- `modern_protocol_only` no longer describes version policy. It is kept because it
  also selects the HTTP capability set over the stdio one, which differ — the
  stdio builder enables `resources` unconditionally. A future rename should not
  treat it as a version switch.
- Removing the pinned `get_info` negotiation fallback would be inert (rmcp
  discards a preferred fallback that has no handshake), so it stays.
- A legacy `initialize` is served, so the diagnostic gap ADR-0052 promised to
  close is closed for real clients. One residue remains and is pinned by a test:
  an `initialize` carrying a *modern* envelope still gets `-32601` with no
  supported list, because `rmcp` gates the method before version negotiation.
- Application Sessions are unaffected. `modern_protocol_only` is read only inside
  `get_info` and `supported_protocol_versions`; no tool handler reads it, and the
  single construction path in `http/runtime/storage.rs` is unchanged. A second
  construction path would have silently dropped durable sessions to the
  in-memory manager with no error, which is why none was added.
- The interop matrix stays `Not executed` until real clients are driven against
  a live deployment. Synthetic conformance proves the server answers both request
  shapes; it does not prove any client connects. Recording that honestly is the
  point — the previous silence is what let an unmeasured decision stand for a
  year.
