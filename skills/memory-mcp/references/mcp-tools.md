# MCP Tool Reference

Load this reference when constructing a Memory MCP call, interpreting its
response, or choosing an app. The connected server schema is authoritative.

## Tool names

Use the exact qualified name advertised by the connected host. Hosts may expose
`mcp__memory__ingest`, `memory-mcp:ingest`, or another server prefix; do not copy a
prefix from another environment or invent a bare tool. The table identifies
operations, not literal callable names. Tool schemas remain authoritative.

## Canonical tools

| Tool | Required caller input | Expected result |
|---|---|---|
| `ingest` | `source_type`, `source_id`, `content`, `t_ref`; optional `policy_tags`, paired `source_group`/`source_position` | created or existing `episode_id` |
| `extract` | exactly one source: `episode_id` or inline `content`/`text` | entities, facts, links, warnings |
| `resolve` | `entity_type`, `canonical_name`; optional aliases | canonical `entity_id` |
| `assemble_context` | `query`; optional fact types, `valid_at`, `known_at`/`known_revision`, legacy `as_of`, budget, view/window fields | ranked context items |
| `explain` | `context_items` encoded as required by the exposed schema | citation-ready provenance |
| `invalidate` | `fact_id`, `reason`, `t_invalid` | persisted invalidation confirmation |

`extract` with inline `content`/`text` is an ingest-then-extract: it creates a
new episode (default `source_type` `ad-hoc`) before extracting, so it is not a
way to re-extract stored content — pass `episode_id` for that. Free-form extraction uses source-type-dependent heuristics and can produce
facts, a fallback note or no durable facts. Inspect the actual result; do not
classify by input format alone. Check `status` and `guidance` first: partial or
pending extraction stays `pending`, preserving `episode_id` for retry. Only a
completed successful extraction with no facts is `episode-only`. Structured
summaries and `ingestion_review` are recovery options, not guaranteed outcomes.

Arguments use flat `snake_case` fields. Do not wrap them in `payload`. Inspect
the live schema for optional fields and enums rather than inventing them. No
canonical tool accepts a `scope`, `project`, or `namespace` argument: the Active
Namespace is server startup configuration and is never selected per request.

Record IDs are canonical `<table>:<id>` strings — `episode:474b2d8b…`,
`fact:52f9d92d…`, `entity:…`. Round-trip every ID exactly as a tool returned
it; never strip or re-add the table prefix. A bare hex ID without its prefix
is rejected with a validation error, not treated as "not found". Such an
error names the exact ID to send back, so retry with the string it quotes
rather than guessing a prefix from the parameter name — a wrong-kind prefix
(`fact:` passed to `extract`) is refused separately and also names the kind
the tool expects.

Successful tool responses use an envelope with `status`, `result`, and
`guidance`. List results also expose pagination metadata. MCP errors are
failures, not empty results; preserve known handles and report `pending`.

## Evidence and source boundaries

`ingest.content` is literal text. URLs and file paths are never fetched/opened.
Omit `t_ingested`: the server rejects caller transaction times. An ordered stream
may supply both `source_group` and positive one-based `source_position`; do not
invent stream identity/order for unrelated documents. Credential-bearing writes
can be refused; redact and retry only within the authorized source boundary.
External reads require the separate authorized operator CLI import workflow.

`extract.include_fact_evidence` defaults to true. Complete `statement` and
`source_quote` pairs share a 4 KiB serialized JSON budget. If any pair is missing
or the budget is exceeded, all pairs are omitted and `fact_evidence_omitted` is
true. Facts can still exist. Set `include_fact_evidence=false` for compact output;
use `explain` for the evidence you need. Claim rollout defaults to `evidence`,
but an operator can configure another stage; unknown role/coverage is not proof
that a claim is current, contradicted or absent.

Read the memory contract before using temporal selectors. Temporal selectors
belong to `assemble_context`, not `explain`; explain accepts only its advertised
item/window fields. Source `t_ref` is an event date, not the retrieval snapshot.
Historical metadata is under `context`: preserve snapshot fields and inspect
partial completeness and omissions. Assembly can persist fact-access heat; logical recall is not a
zero-write storage operation.

## Bounded explain

`context_items` is a JSON-array **string**. Forward each selected returned item
whole, including its top-level `source_episode`, `fact_id` and provenance; wrap
it in the array and JSON-encode once. A fact ID alone can omit the required
source episode. Canonical source-ID strings are also accepted. The envelope is capped at 32 KiB; inspect `status`,
`partial_reasons`, `next_offset` and item-level metadata even with `compact=false`.
Use `item_offset` to select one submitted item. Source/provenance continuations
require exactly one selected item. For an explicit source window pass
`source_offset` and optionally `source_limit_scalars` (maximum 8000); offsets count
Unicode scalars, not bytes. Follow `citation_context_next_offset` and
`provenance_next_offset`; provenance pages contain at most 20 sources.

Default citation context is a bounded window, not the entire source. A returned
quote is not automatically verified: inspect `quote_verified`,
`quote_contained_in_window`, `quote_complete` and
`citation_context_covers_entire_source`. Do not claim an exact citation from an
unverified or truncated excerpt. Preserve the source and fact IDs exactly.
Keep fact identity separate from quote verification: `quote_verified=false`
does not turn a `fact:…` into an episode fallback. Report fallback only when
the returned item identifies it. In a historical final answer include the
actual `context.snapshot` boundary; source `t_ref` cannot substitute for it.

`resolve` is not a lookup. It calls resolve-or-create and can persist a new
canonical entity or aliases. Keep it out of the recall SOP.

## App routing

`open_app` and `app_command` are always advertised in the tool list. Without
the `mcp-apps` feature they fail at call time with an error telling you to
enable it; the app discovery and session resources are what the feature gates
at compile time.

| App | Use for |
|---|---|
| `inspector` | one entity, fact, or episode |
| `diff` | comparison of two temporal snapshots |
| `ingestion_review` | review of a draft episode and extracted items |
| `lifecycle` | server-wide (Active Namespace) maintenance |
| `graph` | traversal between entities |

Call `open_app`, retain its `session_id`, and read the returned resource URI.
Then call `app_command` with only actions and fields advertised by the live tool
schema. Re-read the resource after actions that report a changed view.

App errors are configuration or routing states, not permission to substitute a
canonical tool that changes the user's intent.
