# stdio baseline snapshot (Phase 1.1)

Captured 2026-08-28 on the `streamable-http-mcp` branch, before any HTTP
work, per `docs/superpowers/plans/2026-08-27-streamable-http-saas.md` Task 1.1.

## Test results

See `2026-08-27-stdio-baseline.log` for the captured test counts, command
outputs, and tool surface line citations (single source of truth).

## Tool surface (8 tools, frozen per spec §4.1, ADR-0038)

1. `ingest`
2. `extract`
3. `resolve`
4. `invalidate`
5. `assemble_context`
6. `explain`
7. `open_app`
8. `app_command`

All eight names are confirmed at the `#[tool(` macro sites in
`crates/memory-mcp/src/mcp/handlers.rs`:

- `ingest` — line 254
- `explain` — line 267
- `extract` — line 280
- `resolve` — line 292
- `invalidate` — line 305
- `open_app` — line 318
- `app_command` — line 388
- `assemble_context` — line 472

## Reproduce

```bash
cargo test -p memory_mcp --test service_acceptance --test tools_e2e --test tools_shared -- --nocapture
cargo build -p memory_mcp
```
