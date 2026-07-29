# Adding a New MCP Tool

1. **Design first** — use `mcp-design` skill to evaluate the tool schema before writing code.
2. **Add parameter types** in `crates/memory-mcp/src/mcp/params.rs`
3. **Add parser** in `crates/memory-mcp/src/mcp/parsers.rs` if validation is needed
4. **Implement handler** in `crates/memory-mcp/src/mcp/handlers.rs` (or `crates/memory-mcp/src/mcp/handlers/` for complex handlers)
5. **Register tool** in the MCP server configuration (see `crates/memory-mcp/src/mcp.rs`)
6. **Add tests** in `crates/memory-mcp/tests/` (e.g. `tools_e2e.rs`)
7. **Update `memory-mcp` skill** if the tool changes the API surface
