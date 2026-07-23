# Codex Integration

> Pinned host contract for Codex lifecycle integration.

## Supported hook events

| Codex hook | Internal action |
|---|---|
| `session_start` | Recall once for the resolved task; wake-up view only when the task is empty |
| `user_turn` | Recall when the normalized task changes; capture only an explicit preference, constraint, decision, commitment, or correction |
| `tool_call` | Recall only when no fresh trace exists for the same task/scope/project/policy key |
| `tool_result` | Capture a bounded verified success/failure summary and artifact references |
| `compaction` | Capture one idempotent checkpoint summary |
| `turn_complete` | Capture one idempotent outcome; overlapping stop events converge on the same identity |

**Codex has no `post_compaction` event.** This is unsupported, not silently
substituted from Claude Code's contract.

## Pinned version

Adapter ID: `codex`
Adapter version: `1.0`

## Hook configuration

See `integrations/codex/hooks.example.toml` for the command-hook configuration
that forwards events to the `memory-mcp-host-bridge` executable.
