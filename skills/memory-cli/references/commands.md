# Memory CLI Command Reference

Load this reference when constructing commands or handling operational modes.
Run `memory_mcp <subcommand> --help` before relying on an optional flag; live
help is authoritative.

## One-shot commands

| Command | Required input |
|---|---|
| `memory_mcp ingest` | `--source-type`, `--source-id`, `--content`, `--t-ref`; optional repeatable `--policy-tag` |
| `memory_mcp extract` | `--episode-id`, or inline content plus its source metadata |
| `memory_mcp resolve` | `--entity-type`, `--canonical-name`; aliases are repeatable |
| `memory_mcp assemble-context` | `--query`; optional fact type, `as_of`, budget, view mode, temporal windows |
| `memory_mcp explain` | `--context-items` containing the JSON array from assembly |
| `memory_mcp invalidate` | `--fact-id`, `--reason`, `--t-invalid` |

Flag names are kebab-case forms of the shared snake_case parameter names.
Repeatable values use repeated flags. Optional assembly controls include fact
type, `as_of`, budget, view mode, and temporal windows. No ordinary command
accepts a `--scope`, `--project`, or `--namespace` flag: the Active Namespace is
server startup configuration and is never selected per command.

One-shot commands print structured responses to stdout. Capture stderr and the
exit status independently. Validation and configuration failures are terminal
until their cause is corrected; do not parse diagnostic text as a successful
response.

`resolve` invokes resolve-or-create. It may persist a new entity or aliases and
therefore belongs to the canonicalize workflow, not read-only recall.

## Operational commands

- `memory_mcp serve` starts the stdio MCP server. With no subcommand, the binary
  also defaults to server mode.
- Filesystem ingestion runs inside `serve`: set `MEMORY_INGESTION_INBOX` to an
  existing absolute directory to activate it (`fs-watch` feature required;
  official release binaries include it).
- `memory_mcp reembed` rebuilds fact embeddings. `--max-failures 0` is
  fail-fast; `--retry-failed` limits work to facts recorded as failed by a prior
  run.

`reembed` is interactive in a TTY, continues within its configured failure
budget, and uses terminal states `running`, `completed`, `completed_with_errors`,
`failed`, and `interrupted`.

Hidden lifecycle commands are implementation interfaces for installed hooks,
not public commands for ad-hoc agent use.
