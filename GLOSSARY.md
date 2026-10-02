# Glossary

Domain vocabulary for the Memory MCP server. Terms are canonical: when a decision,
ADR, or conversation uses a different word for one of these concepts, this file wins.

## Protocol eras

Terms defined by the MCP specification's versioning page. They describe **what a
request speaks**, not what a deployment is configured to allow.

- **Modern era** — a protocol revision that carries version, identity, and
  capabilities as per-request metadata. Revision `2026-07-28` and later. Every
  request stands alone; there is no handshake and no protocol session.
- **Legacy era** — a protocol revision that establishes a session with an
  `initialize` handshake. `2025-11-25` and earlier.
- **Dual-era** — an implementation that serves both eras. "Era" is a property of
  an individual request, not of a server: a modern-shaped request and a legacy
  `initialize` arriving at the same endpoint are both served, each in its own era.
- **Era detection** — deriving the era from the request itself. The specification
  defines the discriminants (a per-request `_meta` envelope selects modern; an
  `initialize` selects legacy). Detection is never taken from a header a client
  can set to a different value than the body it sent.

### Rejected: "legacy profile" / "compat mode"

A *profile* is something an operator turns on. A dual-era server has no such
switch: it serves both eras on one endpoint unconditionally, because a client
cannot be asked to change its client to reach a server. Naming this a profile
invites re-introducing the configuration question the dual-era decision settled.

### Rejected: "modern-only" as a description of a capability

"Modern-only" describes a *limitation*, not a feature. It is retained only when
naming the prior decision being superseded.

## Sessions

- **Stateless server** — an MCP server that mints no `Mcp-Session-Id` and keeps no
  per-connection protocol state. Every request is served from the request itself.
  This is a legal and normal mode in the legacy era: those revisions say a server
  *MAY* assign a session id, and client obligations to echo it are conditional on
  the server having returned one.
- **Protocol session** — state bound to a connection by the MCP protocol, as
  opposed to application state that outlives any one request. Memory MCP's
  Application Session is not a protocol session; see below.

## Identity and tenancy

- **Application Session** — the durable, tenant-scoped unit of agent work that
  survives across requests and processes. Identified by `session_id` and minted by
  the server. Unaffected by the absence of a protocol session: the two are
  independent, and removing protocol sessions does not remove this.
- **Tenant** — the storage and isolation boundary, selected at startup as the one
  Active Namespace. A request's tenant derives from the verified API key, never
  from MCP arguments, URL paths, or client-supplied headers.

## Request validation

- **Preflight** — validation that runs before any authentication or admission
  decision. Its purpose is to prevent a routing or authorization decision from
  trusting a client-supplied header before the client is authenticated.
- **Mirrored headers** — `Mcp-Method` and `Mcp-Name`, which the modern era requires
  on Streamable HTTP so an intermediary can route without parsing the body. They
  are *mirrors* of the body: their correctness is defined by agreeing with it.
- **Admission** — the resource decision made for an accepted request: which
  concurrency budget it draws from, whether a deadline applies, whether runtime
  capacity is pinned. Distinct from authentication, which decides *who* the
  caller is.
