# Hook scripts for memory capture

These scripts persist lightweight session snapshots into `memory_mcp` by calling the existing `ingest` MCP tool over the server's stdio transport.

They are intentionally deterministic:

- no external LLMs
- no new MCP tools
- content comes from `MEMORY_HOOK_CONTENT`, a hook payload `transcript_path`, or the raw JSON payload itself

## Files

- `hooks/memory_stop_hook.sh` — capture a session snapshot when an agent run completes
- `hooks/memory_precompact_hook.sh` — capture an emergency snapshot before context compaction
- `hooks/memory_profile.sh` — internal RSS/footprint sampling helper (not part of the public lifecycle contract)

## Zero-config default

For the normal repository-local setup, you do **not** need to set any hook-specific environment variables.

If you simply:

1. make the scripts executable
2. point your editor hooks at `./hooks/memory_stop_hook.sh` and `./hooks/memory_precompact_hook.sh`

the scripts will work with these defaults:

- start the MCP server with `cargo run --quiet --bin memory_mcp`
- run that command from the repository root
- use the server's Active Namespace (default `main`)
- store the entry as `source_type="session_summary"`
- auto-generate a deterministic `source_id`
- apply sensible default `policy_tags` for stop vs precompact events
- fall back to a local embedded SurrealDB if no DB env vars are already configured

That means environment variables are mainly for **power users**: custom binary paths, server configuration, manual summaries, or debugging.

## What gets ingested

Both scripts call `ingest` with:

- `source_type = "session_summary"` by default

- deterministic `source_id` based on hook event + session id + content hash
- default `policy_tags`:
  - stop: `hook:stop,session_summary`
  - precompact: `hook:precompact,session_summary,emergency_save`

In other words: if you do nothing, both hooks save a session snapshot into the configured Active Namespace using the semantic source type `session_summary`.

Content is resolved in this order:

1. `MEMORY_HOOK_CONTENT` if you want to provide an explicit summary string (mainly useful for manual runs, tests, or CI)
2. the last `MEMORY_HOOK_MAX_TRANSCRIPT_LINES` lines from `transcript_path` / `transcriptPath`
3. raw stdin text if stdin is not JSON
4. pretty-printed hook payload JSON as a fallback

## Environment variables (optional)

Most users can skip this entire section.

No hook-specific environment variables are required for the default repo-local setup.

### Common optional overrides

These are the only variables most teams will ever need.

| Variable | Default | Expected values | Purpose / when to use |
| --- | --- | --- | --- |

| `MEMORY_MCP_SERVER_CMD` | `cargo run --quiet --bin memory_mcp` | Any shell command that starts this MCP server, for example `cargo run --quiet --bin memory_mcp` or `./target/release/memory_mcp` | Use this when you want the hook to start a prebuilt binary, a wrapper script, or a differently located command instead of `cargo run`. |
| `MEMORY_MCP_SERVER_CWD` | repo root | Directory path | The working directory used when launching `MEMORY_MCP_SERVER_CMD`. Usually keep the default. Change it only if your command depends on relative paths, a nearby `Cargo.toml`, local data files, or `.env` resolution from another directory. |

### Power-user overrides

These knobs are useful for debugging, manual runs, CI, or special classification rules. Most teams should leave them alone.

| Variable | Default | Expected values | Purpose / when to use |
| --- | --- | --- | --- |
| `MEMORY_HOOK_CONTENT` | unset | Any non-empty text string | Forces the exact content that will be ingested. If set, it wins over transcript extraction and stdin fallback. Best for manual invocations, tests, CI jobs, or “save this exact summary” flows. |
| `MEMORY_HOOK_MAX_TRANSCRIPT_LINES` | `80` | Positive integer such as `20`, `50`, `80`, `200` | Limits how many trailing lines are copied from `transcript_path` / `transcriptPath`. Lower values reduce noise and token volume; higher values preserve more context from long sessions. Only matters when the hook payload includes a transcript file path. |
| `MEMORY_HOOK_VERBOSE` | unset | `1` to enable, otherwise leave unset | Prints a short success message to stdout after ingest. Useful for manual testing or CI logs. It does **not** change what gets stored. |
| `MEMORY_HOOK_SOURCE_TYPE` | `session_summary` | Any non-empty semantic source label such as `session_summary`, `document`, `email`, `conversation` | Overrides the `source_type` sent to `ingest`. In this repository the value is just a semantic label, not an enum hardcoded by the hooks. For normal hook usage, keep `session_summary`. Change it only if you intentionally want these records classified differently. |
| `MEMORY_HOOK_POLICY_TAGS` | Event-specific defaults | Comma-separated tag list such as `hook:stop,session_summary` or `hook:precompact,session_summary,emergency_save,team:core` | Replaces the default policy tags with your own full list. Use this only if you have downstream filtering, compliance, or routing logic that depends on custom tags. |

### Server / database defaults

If SurrealDB env vars are absent, both scripts fall back to local embedded defaults:

- `SURREALDB_DB_NAME=memory`
- `SURREALDB_EMBEDDED=true`
- `SURREALDB_NAMESPACE=main` (unless already set for the process)
- `SURREALDB_USERNAME=root`
- `SURREALDB_PASSWORD=root`
- `SURREALDB_DATA_DIR=<repo>/data/surrealdb`
- `RUST_LOG=error`

You usually do **not** need to set these in the hook configuration either.

They are normal server-level settings, not hook-level UX knobs. The hooks only provide these defaults so they can work out of the box on a local checkout.

## One-time setup

Make the scripts executable:

```bash
chmod +x hooks/memory_stop_hook.sh hooks/memory_precompact_hook.sh
```

## Claude Code

Claude Code has native `Stop` and `PreCompact` hooks and passes JSON input on stdin.
The public hooks docs currently document both events and confirm that stdin contains event-specific JSON payloads, including `transcript_path` for `PreCompact` and `stop_hook_active` / session metadata for stop-related hooks.

Project-local `.claude/settings.json` example:

```json
{
  "$schema": "https://json.schemastore.org/claude-code-settings.json",
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "$CLAUDE_PROJECT_DIR/hooks/memory_stop_hook.sh"
          }
        ]
      }
    ],
    "PreCompact": [
      {
        "matcher": "manual|auto",
        "hooks": [
          {
            "type": "command",
            "command": "$CLAUDE_PROJECT_DIR/hooks/memory_precompact_hook.sh"
          }
        ]
      }
    ]
  }
}
```



## Cursor

Recent Cursor builds expose beta lifecycle hooks through `.cursor/hooks.json`, including `stop` and `preCompact`. Hook commands receive JSON on stdin.

Project-local `.cursor/hooks.json` example:

```json
{
  "version": 1,
  "hooks": {
    "stop": [
      {
        "command": "./hooks/memory_stop_hook.sh",
        "timeout": 30
      }
    ],
    "preCompact": [
      {
        "command": "./hooks/memory_precompact_hook.sh",
        "timeout": 30
      }
    ]
  }
}
```

If your Cursor build only exposes `stop`, keep `stop` and trigger the precompact script from your own wrapper/task before clearing agent context.

## OpenCode

OpenCode's public docs do **not** currently describe a Claude-style shell-hook config with named `Stop` / `PreCompact` entries.

The closest documented integration points are:

- plugin event handlers such as `session.idle` for a stop-like save
- the documented `experimental.session.compacting` hook for precompact-style saves

OpenCode loads project-local plugins from `.opencode/plugins/`, so you can keep the hook scripts in this repository and call them from a small plugin.

Example `.opencode/plugins/memory-hooks.ts`:

```ts
import type { Plugin } from "@opencode-ai/plugin"

export const MemoryHooks: Plugin = async ({ $, worktree }) => {
  return {
    event: async ({ event }) => {
      if (event.type === "session.idle") {
        await $`MEMORY_HOOK_CONTENT="OpenCode session became idle" ${worktree}/hooks/memory_stop_hook.sh`
      }
    },
    "experimental.session.compacting": async () => {
      await $`MEMORY_HOOK_CONTENT="OpenCode session is compacting" ${worktree}/hooks/memory_precompact_hook.sh`
    },
  }
}
```

Notes:

- `session.idle` is the closest documented stop-like event, not a guaranteed 1:1 equivalent of Claude Code `Stop`.
- `experimental.session.compacting` is the documented pre-compaction hook and is the best fit for `memory_precompact_hook.sh`.
- If you want a fully manual flow instead of automation, you can also keep these scripts as standalone shell commands and run them from your terminal inside the OpenCode project root.

## VS Code

Plain VS Code does **not** provide native agent lifecycle hooks equivalent to Claude Code / Cursor in its core task system.

The simplest documented integration is to expose these scripts as workspace tasks and run them manually from:

- **Terminal → Run Task**
- **Command Palette → Tasks: Run Task**
- optional keyboard shortcuts bound to those tasks

Example `.vscode/tasks.json`:

```json
{
  "version": "2.0.0",
  "tasks": [
    {
      "label": "Memory: stop snapshot",
      "type": "shell",
      "command": "${workspaceFolder}/hooks/memory_stop_hook.sh",
      "options": {
        "cwd": "${workspaceFolder}",
        "env": {
          "MEMORY_HOOK_PROJECT": "memory_mcp",
          "MEMORY_HOOK_VERBOSE": "1"
        }
      },
      "problemMatcher": [],
      "presentation": {
        "reveal": "always",
        "panel": "new"
      }
    },
    {
      "label": "Memory: precompact snapshot",
      "type": "shell",
      "command": "${workspaceFolder}/hooks/memory_precompact_hook.sh",
      "options": {
        "cwd": "${workspaceFolder}",
        "env": {
          "MEMORY_HOOK_PROJECT": "memory_mcp",
          "MEMORY_HOOK_VERBOSE": "1"
        }
      },
      "problemMatcher": [],
      "presentation": {
        "reveal": "always",
        "panel": "new"
      }
    }
  ]
}
```

Optional `keybindings.json` example if you want one-keystroke manual saves:

```json
[
  {
    "key": "cmd+alt+m",
    "command": "workbench.action.tasks.runTask",
    "args": "Memory: stop snapshot"
  },
  {
    "key": "cmd+alt+shift+m",
    "command": "workbench.action.tasks.runTask",
    "args": "Memory: precompact snapshot"
  }
]
```

Notes:

- VS Code tasks are available when working in a workspace/folder, not when editing a single loose file.
- `options.cwd` is the documented way to ensure the task runs from the workspace root.
- `options.env` is a convenient place to set `MEMORY_HOOK_PROJECT` or `MEMORY_HOOK_VERBOSE` without exporting them globally in your shell.

## Continue

Continue's public `config.yaml` docs currently expose MCP server wiring and telemetry/event export, but they do **not** document native `Stop` / `PreCompact` lifecycle hooks equivalent to Claude Code or Cursor.

So for Continue, use these scripts as manual or external automation entry points instead of native lifecycle hooks. Two practical options:

1. run the stop script from a task/command after an agent session ends
2. run the precompact script before manually resetting context or rotating logs

Examples:

```bash
printf '%s\n' '{"hook_event_name":"manual-stop","summary":"Continue session finished"}' | ./hooks/memory_stop_hook.sh
printf '%s\n' '{"hook_event_name":"manual-precompact","reason":"manual context reset"}' | ./hooks/memory_precompact_hook.sh
```

Or inject a handcrafted summary directly:

```bash
MEMORY_HOOK_CONTENT="Implemented filesystem ingestion and validated fs-watch tests" ./hooks/memory_stop_hook.sh
```


## Internal lifecycle CLI subcommands

In addition to the ordinary `ingest` path used by these hook scripts,
the server exposes two hidden CLI subcommands for selective lifecycle
capture and recall with policy classification:

- `memory_mcp lifecycle-capture --event <json> --context <json>`
- `memory_mcp lifecycle-recall --event <json> --context <json>`

These are internal (hidden from `--help`) and consumed by hook scripts
that need selective capture/recall with trust derivation, salience
policy, and ephemeral trace management per ADR-0016 AD-4/AD-5. The full
hook contract, including all environment variables, payload schemas, and
editor-by-editor configuration examples, is documented inline in this
README.

The ordinary `ingest` path used by the scripts in this directory is
always available and works without lifecycle configuration.

## Notes

- The scripts speak newline-delimited JSON-RPC over stdio and perform the minimal MCP handshake: `initialize` → `notifications/initialized` → `tools/call`.
- They use `protocolVersion = "2025-06-18"`, which matches the protocol versions supported by the current `rmcp` dependency in this repository.
- If you prefer a prebuilt binary, set `MEMORY_MCP_SERVER_CMD="./target/release/memory_mcp"` after `cargo build --release`.
- `MEMORY_MCP_SERVER_CWD` is usually only relevant together with a custom `MEMORY_MCP_SERVER_CMD`; if you are using the scripts from this repository as-is, leave it alone.
- `PreCompact` hooks are intentionally non-blocking in Claude Code; these scripts only persist state and exit `0` on success.
