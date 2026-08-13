# MCP Tool Reference

Load this reference when constructing a Memory MCP call, interpreting its
response, or choosing an app. The connected server schema is authoritative.

## Tool names

MCP skills call fully qualified tool names. Hosts render qualification
differently:

- this repository's Codex surface uses `mcp_memory-mcp_ingest`,
  `mcp_memory-mcp_extract`, `mcp_memory-mcp_resolve`,
  `mcp_memory-mcp_assemble_context`, `mcp_memory-mcp_explain`,
  `mcp_memory-mcp_invalidate`, `mcp_memory-mcp_open_app`, and
  `mcp_memory-mcp_app_command`;
- hosts using `ServerName:tool_name` convention may expose
  `memory-mcp:ingest`, and so on.

Use the exact qualified name listed by the connected host. The tables below use
raw names only to identify the operation.

## Canonical tools

| Tool | Required caller input | Expected result |
|---|---|---|
| `ingest` | `source_type`, `source_id`, `content`, `t_ref`; optional `policy_tags` | created or existing `episode_id` |
| `extract` | exactly one source: `episode_id` or inline `content`/`text` | entities, facts, links, warnings |
| `resolve` | `entity_type`, `canonical_name`; optional aliases | canonical `entity_id` |
| `assemble_context` | `query`; optional fact types, `as_of`, budget, view/window fields | ranked context items |
| `explain` | `context_items` encoded as required by the exposed schema | citation-ready provenance |
| `invalidate` | `fact_id`, `reason`, `t_invalid` | persisted invalidation confirmation |

Arguments use flat `snake_case` fields. Do not wrap them in `payload`. Inspect
the live schema for optional fields and enums rather than inventing them. No
canonical tool accepts a `scope`, `project`, or `namespace` argument: the Active
Namespace is server startup configuration and is never selected per request.

Successful tool responses use an envelope with `status`, `result`, and
`guidance`. List results also expose pagination metadata. MCP errors are
failures, not empty results; preserve known handles and report `pending`.

`resolve` is not a lookup. It calls resolve-or-create and can persist a new
canonical entity or aliases. Keep it out of the read-only recall SOP.

## App routing

App tools are available only when the server is built with MCP Apps support.

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
