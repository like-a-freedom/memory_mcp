# Claude Code Integration

> Pinned host contract for Claude Code lifecycle integration.

## Supported hook events

| Claude Code hook | Internal action |
|---|---|
| `session_start` | Recall once for the resolved task; wake-up view only when the task is empty |
| `user_prompt` | Recall when the normalized task changes; capture only an explicit preference, constraint, decision, commitment, or correction |
| `pre_tool` | Recall only when no fresh trace exists for the same task/scope/project/policy key |
| `post_tool` | Capture a bounded verified success/failure summary and artifact references |
| `pre_compaction` | Capture one idempotent checkpoint summary |
| `post_compaction` | Force one recall even if the previous key matches |
| `task_stop` | Capture one idempotent outcome; overlapping stop events converge on the same identity |

An event absent from this table is unsupported, not silently substituted.

## Pinned version

Adapter ID: `claude_code`
Adapter version: `1.0`

## Hook configuration

See `integrations/claude-code/hooks.example.json` for the command-hook
configuration that forwards events to the `memory-mcp-host-bridge` executable.
