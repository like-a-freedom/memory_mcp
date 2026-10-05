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
- **Proposed revision** — the protocol revision a client requests during a
  legacy `initialize` handshake. A proposal is not evidence that the server
  supports that revision.
- **Negotiated revision** — the supported legacy protocol revision returned by
  the server's initialization response. It can differ from the proposed revision.

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
- **Tenant binding** — the association between a Tenant and its database and
  namespace. A change in plan or lifecycle status is not a change in binding.
- **Tenant runtime** — the active execution environment for a Tenant's bound
  storage. It is distinct from the Tenant's durable identity and lifecycle status.
- **Tenant activation** — preparing a Tenant runtime for use. Waiting for an
  activation does not itself authorize access to the Tenant's data.

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

## Memory domain

The vocabulary below governs what the system stores and reconciles. The terms
`retraction`, `supersession`, and `correction` are strictly distinct operations
and are the single most frequently confused group in this domain: conflating
them invalidates evidence the system is supposed to preserve.

- **Claim** — an attributed assertion extracted from an episode, carrying its
  subject, value, validity interval and lineage. The unit that reconciliation
  operates on; a fact may carry several claims.
_Avoid_: assertion, fact, statement

- **Fact** — a piece of remembered content, the unit retrieval ranks and returns.
  A fact is the reader-facing object; a claim is the attributed proposition
  beneath it.
_Avoid_: memory, record

- **Reconciliation** — the process of comparing claims about the same subject
  and recording a typed relation between them: `duplicate`, `supersession`,
  `correction`, `contradiction`, or `temporal_ambiguity`. Reconciliation
  produces relations; it does not by itself change what a reader is shown.
_Avoid_: dedup, conflict resolution, merge

- **Supersession** — a claim replaces an earlier claim of the same lineage
  because the later one is true over a later interval. The earlier claim's
  *real-world* validity interval closes; it is not erased, and the fact it
  supported remains retrievable.
_Avoid_: replacement, override, update

- **Correction** — a claim replaces an earlier one because the earlier was
  **wrong**, not merely outdated. Distinguished from supersession by *which*
  interval each closes.
_Avoid_: fix, patch, supersession

- **Retraction** — withdrawing a fact or claim from service because it should no
  longer be asserted at all, without asserting anything in its place. Closes
  transaction time only; the validity interval is untouched. This is what
  `invalidate` performs.
_Avoid_: invalidation (use retraction), deletion, forget

- **Duplicate** — a claim asserting the same proposition as another with a
  compatible validity interval. Redundancy, **not** staleness: a duplicate is
  never demoted by being superseded, because either copy may outlive the other.
_Avoid_: redundancy, clone

- **Contradiction** — two claims that cannot both be true. Recorded as a
  relation; a contradiction alone never invalidates anything.
_Avoid_: conflict, disagreement

- **Temporal ambiguity** — claims that cannot be compared because the validity
  information is insufficient. A recorded outcome, not a failure: it is how the
  system declines to guess.
_Avoid_: uncertainty, unknown

- **Successor** — the claim that supersedes or corrects another. Recorded once,
  in the relation's `successor_claim_id`; never recomputed by a reader.
_Avoid_: replacement claim, winner

- **Active claim** — a claim whose validity interval has not ended. Claims are
  never deleted: a claim leaves the active set by closing its interval, and its
  record persists.
_Avoid_: valid claim, current claim, live claim

- **Trust class** — how far a source may be trusted, distinct from extraction
  confidence. Trust describes *where a record came from*, not how sure the
  extractor was. Derived trust is the minimum over its bases and is never
  elevated by summarization or consolidation.
_Avoid_: confidence, reliability

- **Belief** — a current, materialized interpretation held to be true. **Not a
  type in this repository**: the reconciliation relation vocabulary already
  covers the concern, and adding a parallel belief layer would duplicate it.
  Named here only so external reviews using the term resolve to *reconciliation*.
_Avoid_: belief state, hypothesis (if it means the same thing)

### Rejected: "belief layer" as an addition to the claim model

External reviews propose a separate `Belief` entity with
`active | disputed | superseded | uncertain` statuses and its own
`supports`/`contradicts` vocabulary. The relation outcome set already expresses
all of it — a `disputed` belief is two claims in `contradiction`, an `uncertain`
one is a claim related by `temporal_ambiguity`. A parallel entity would make
every query answer two questions ("what does the claim say, and what does the
belief say?") and give two places for the answer to drift.
