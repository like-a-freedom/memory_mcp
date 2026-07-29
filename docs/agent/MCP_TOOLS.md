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
