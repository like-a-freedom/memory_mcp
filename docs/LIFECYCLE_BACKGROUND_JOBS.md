# Lifecycle Background Jobs

**Status:** Implementation plan complete (2026-03-26)
**See:** `docs/superpowers/plans/2026-03-26-lifecycle-background-jobs.md`

## Overview

The Memory MCP server includes optional background workers that maintain memory hygiene:

1. **Confidence Decay Worker** - marks stale facts as invalid
2. **Episode Archival Worker** - archives old episodes without active facts

Both workers are **disabled by default** and must be explicitly enabled via environment variables.

## Deferred background jobs (NOW IMPLEMENTED)

The following work has been specified and is ready for implementation:

- **confidence decay refresh**
  - Periodically recompute long-lived ranking caches that depend on temporal decay
  - Keep request-time confidence logic authoritative until cache invalidation rules are proven
  - **Implementation:** `LIFECYCLE_DECAY_INTERVAL_SECS` (default: 1 hour)
  - Marks facts with decayed confidence < 0.3 as invalid

- **episode consolidation / archival**
  - Compact duplicate or stale community records left behind by older implementations or interrupted writes
  - Optionally rebuild connected components from the persisted graph on a schedule
  - **Implementation:** `LIFECYCLE_ARCHIVAL_INTERVAL_SECS` (default: 24 hours)
  - Archives episodes older than 90 days without active facts

- **embedding backfill** (REMOVED from scope)
  - ~~Populate missing embeddings after a real provider is enabled~~
  - ~~Reindex vector fields after dimension or provider changes~~
  - **Status:** Superseded by `SIMPLIFIED_SEARCH_REDESIGN_SPEC.md` — embeddings removed from runtime

## Why this was deferred (HISTORICAL)

Mixing background mutation with the correctness wave would have made it harder to prove:

- which request-path writes are authoritative
- whether invalidation and community updates are deterministic
- whether migration startup checks were rejecting drift correctly

**Current status (2026-03-26):** Request-path correctness is complete. Background jobs are now implemented as optional workers controlled by `LIFECYCLE_ENABLED`.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `LIFECYCLE_ENABLED` | false | Enable background workers |
| `LIFECYCLE_DECAY_INTERVAL_SECS` | 3600 | Decay job interval |
| `LIFECYCLE_ARCHIVAL_INTERVAL_SECS` | 86400 | Archival job interval |
| `LIFECYCLE_DECAY_THRESHOLD` | 0.3 | Confidence threshold |
| `LIFECYCLE_ARCHIVAL_AGE_DAYS` | 90 | Episode age threshold |

## Implementation Status

| Component | Status | Plan |
|-----------|--------|------|
| Decay worker | 📋 Planned | `docs/superpowers/plans/2026-03-26-lifecycle-background-jobs.md` |
| Archival worker | 📋 Planned | `docs/superpowers/plans/2026-03-26-lifecycle-background-jobs.md` |
| Configuration | 📋 Planned | Task 1 of lifecycle plan |
| Integration tests | 📋 Planned | Tasks 2-3 of lifecycle plan |

## Next Steps

Execute the implementation plan using one of two approaches:

1. **Subagent-Driven** (recommended) - Fresh subagent per task with review checkpoints
2. **Inline Execution** - Batch execution with checkpoints in current session
