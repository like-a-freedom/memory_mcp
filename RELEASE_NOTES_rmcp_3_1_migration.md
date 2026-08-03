# Release Notes: rmcp 3.1 Migration

## Version

**memory_mcp 1.7.0** — MCP server migration to `rmcp` 3.1.0

## Summary

This release migrates the MCP server from `rmcp` 2.2.0 to 3.1.0, adopting the official [Tasks extension](https://github.com/modelcontextprotocol/rust-sdk/discussions/969) and removing all custom task infrastructure.

## Breaking Changes

- **Minimum Rust version**: MSRV is now **1.88** (declared in workspace metadata; CI continues to use stable for regular jobs)
- **Task API**: The server no longer exposes `tasks/list`, `tasks/result`, `TaskMetadata`, `TasksCapability`, `TaskSupport`, or `execution(task_support = "optional")`
- **Task identifiers**: Task IDs are now SDK-generated UUIDs; client-supplied IDs are not used

## New Capabilities

| Feature | Description |
|---------|-------------|
| `io.modelcontextprotocol/tasks` | Official extension; advertised in `initialize` capabilities |
| `extract` as task | Only `extract` is materialized as a task when the client advertises the extension |
| `tasks/get` | Poll for terminal state; completed payload under `result`, failed under `error` |
| `tasks/update` | Submit input responses (empty object for this server) |
| `tasks/cancel` | Request cooperative cancellation |
| `ReadResourceResponse` | Widened protocol boundary; internal `ReadResourceResult` helper unchanged |

## Behavior

- **Clients without the extension** → synchronous `extract` via `tools/call`
- **Clients with the extension** → `tools/call` returns `taskId`, `status`, `createdAt`, `pollIntervalMs`; poll `tasks/get` until `completed` / `failed` / `cancelled`
- **Cancellation** is cooperative; the task may still reach `completed` or `failed`
- **TTL, polling interval, retention** are managed by `rmcp::task_manager::TaskManager` (default 5 min TTL, 1 s poll)

## Dependencies

| Dependency | Version | Notes |
|------------|---------|-------|
| `rmcp` | `3.1.0` | features: `macros`, `transport-io`, `server` |
| `hf-hub` | `0.5.0` | avoids Rust-1.88-incompatible `hf-xet`/`konst`/`redb` chain |
| `surrealdb` | `3.0.0` | `kv-rocksdb`, `kv-mem`; avoids incompatible `ferntree` in 3.1+ |
| `candle-core` | git `21cca0b1` | locked revision preserved |

## Testing

All gates pass on stable:

```
cargo fmt --all --check
cargo check --workspace --all-targets --locked
cargo check --workspace --all-targets --all-features --locked
cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings
cargo test --workspace --all-targets --no-fail-fast
cargo test --workspace --all-targets --all-features --no-fail-fast
cargo metadata --locked --no-deps
```

CI jobs:

- `fmt`, `metadata`, `clippy`, `clippy_macos`, `test` (with `--locked`), `msrv` (Rust 1.88), `eval-pr`, `build_binaries` (on release)

## Documentation

- `README.md` → Tasks section rewritten for official extension
- `docs/performance/NER_PERFORMANCE.md` → task description updated to rmcp 3.1 lifecycle
- `docs/superpowers/plans/2026-08-03-rmcp-3-1-migration.md` → implementation plan

## Notes for Integrators

- If you already advertise `io.modelcontextprotocol/tasks`, no change needed
- If you don't, `extract` continues to work synchronously as before
- Task listing and the old `tasks/result` endpoint are not available