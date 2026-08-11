# Adding a New MCP Tool

Adding a tool is a **frozen-surface change**. Do not start coding until the
governance gate passes.

## Governance gate (required, first)

The MCP surface is frozen at exactly eight tools (ADR-0016). Adding a ninth
tool requires:

1. A dedicated ADR that records the decision and amends the frozen surface.
2. The evidence gate described in ADR-0016: the ADR plus a `public_surface_snapshot`
   test update that proves the new surface is intentional and covered.
3. An entry in `CONTEXT.md` for any new domain concept the tool introduces.

`AGENTS.md` also requires asking before adding a new MCP tool. Open the ADR
first; only proceed after it is accepted.

## Architecture

Tool logic is **protocol-agnostic** and lives in layers that do not depend on
MCP:

1. **Capability** — `crates/memory-mcp/src/service/capabilities/*` (via
   `&ServiceContext`). This is where business logic and DB access live.
2. **Tools wrapper** — `crates/memory-mcp/src/tools/<name>.rs`. A thin,
   protocol-agnostic facade that the MCP handler calls; shared request
   plumbing lives in `src/tools/params.rs`, `parsers.rs`, `response.rs`.
3. **MCP handler** — `crates/memory-mcp/src/mcp/handlers.rs`. A thin adapter
   that parses MCP input, calls the tools wrapper, and formats the response.

## Steps

1. **Design first** — use `mcp-design` skill to evaluate the tool schema before
   writing code. Confirm the new tool passes the frozen-surface gate above.
2. **Implement the capability** in `crates/memory-mcp/src/service/capabilities/`
   (or the matching `src/service/` submodule), exposing it through
   `&ServiceContext`. Add unit tests here — the seam is the test surface.
3. **Add the tools wrapper** in `crates/memory-mcp/src/tools/<name>.rs`
   (register it in `src/tools/mod.rs`). Keep it protocol-agnostic; add
   parameter types and parsers in `src/tools/params.rs` / `parsers.rs` if
   validation is needed.
4. **Implement the MCP handler** in `crates/memory-mcp/src/mcp/handlers.rs`
   (or `crates/memory-mcp/src/mcp/handlers/` for complex handlers) — a thin
   adapter over the tools wrapper.
5. **Register the tool** on `MemoryMcp` via the `#[tool_router]` block in
   `crates/memory-mcp/src/mcp/handlers.rs`. (`src/mcp.rs` contains only module
   declarations; it is not where tools are registered.)
6. **Add tests** — capability unit tests in `src/service/`, plus end-to-end
   coverage in `crates/memory-mcp/tests/` (e.g. `tools_e2e.rs`).
7. **Update the frozen-surface contract** — extend the
   `public_surface_snapshot` test in
   `crates/memory-mcp/tests/agent_memory_lifecycle_release_gate.rs`, update
   `docs/agent/MCP_TOOLS.md`, and update the `memory-mcp` skill.
