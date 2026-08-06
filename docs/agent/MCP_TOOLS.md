# MCP Tools Reference

The server exposes exactly eight tools via the MCP protocol. This is a frozen public surface — adding a tool requires a separate ADR.

## Tool Overview

| Tool | Purpose |
|------|---------|
| `mcp_memory-mcp_ingest` | Store raw source material as an episode |
| `mcp_memory-mcp_extract` | Extract entities and facts from an episode |
| `mcp_memory-mcp_resolve` | Resolve entity aliases to canonical IDs |
| `mcp_memory-mcp_assemble_context` | Retrieve ranked, relevant facts for a query |
| `mcp_memory-mcp_explain` | Get citation-ready source snippets from context items |
| `mcp_memory-mcp_invalidate` | Mark facts as outdated (preserves audit trail) |
| `mcp_memory-mcp_open_app` | Open the app surface for operator review |
| `mcp_memory-mcp_app_command` | Execute app commands for operator workflows |

## Common Patterns

- All tool arguments use flat snake_case.
- Responses include `status`, `guidance`, `has_more`, `total_count`.
- The eight-tool surface is tested by `public_surface_snapshot` in `tests/agent_memory_lifecycle_release_gate.rs`.

### Record id contract

Tools that accept a stored-record identifier (`extract.episode_id`, `invalidate.fact_id`, `explain.context_pack[*].source_episode`, etc.) require the canonical `<table>:<id>` form. The id returned by `ingest` (`episode:<hex>`) and by `invalidate` ack payloads (`fact:<hex>`) must be passed back unchanged.

**Do not strip the prefix.** Passing the bare hex — e.g. `{"episode_id": "474b2d8b81b3feabf832ef08"}` instead of `{"episode_id": "episode:474b2d8b81b3feabf832ef08"}` — is rejected with a `Validation` error whose message names the expected form, e.g.:

```
validation error: record_id '474b2d8b81b3feabf832ef08' is not a valid table name;
  expected '<table>:<id>'
```

Pre-fix, this misuse surfaced as `Episode not found: <hex>` because the query builder silently turned the malformed id into a no-op SELECT. The fix replaces that misleading signal with an explicit `INVALID_PARAMS` response, so callers see the input-shape problem at the call site rather than chasing a phantom missing record.

Round-trip ids verbatim. If you must transform an id (logging, persistence, comparison), keep the `<table>:<id>` shape intact.
