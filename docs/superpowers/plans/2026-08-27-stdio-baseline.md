# stdio baseline snapshot (Phase 1.1)

Captured 2026-08-28 on the `streamable-http-mcp` branch, before any HTTP
work, per `docs/superpowers/plans/2026-08-27-streamable-http-saas.md` Task 1.1.

## Test results

This is a narrative companion to `2026-08-27-stdio-baseline.log` (the single
source of truth for the captured test counts).

- `cargo test -p memory_mcp --test service_acceptance` — 27 passed, 0 failed
- `cargo test -p memory_mcp --test tools_e2e` — 11 passed, 0 failed
- `cargo test -p memory_mcp --test tools_shared` — 8 passed, 0 failed
- `cargo build -p memory_mcp` — clean, no warnings

Total: **46 stdio tests passing** on the frozen baseline.

## Tool surface (8 tools, frozen per spec §4.1, ADR-0038)

1. `ingest`
2. `extract`
3. `resolve`
4. `invalidate`
5. `assemble_context`
6. `explain`
7. `open_app`
8. `app_command`

All eight names are confirmed in `crates/memory-mcp/src/mcp/handlers.rs` at
the `#[tool]` macro sites (lines 257, 283, 295, 308, 321, 391, 475, plus
`extract` at 283 and `ingest` at 257).

## Reproduce

```bash
cargo test -p memory_mcp --test service_acceptance --test tools_e2e --test tools_shared -- --nocapture
cargo build -p memory_mcp
```
