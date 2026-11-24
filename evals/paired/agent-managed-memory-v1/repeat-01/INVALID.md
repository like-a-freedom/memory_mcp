# Invalid capture: outer sandbox blocked Codex startup

The v2 runner completed all registered cases, but all formation and recall subprocesses exited before a model turn with `model_failure`, zero turns, zero calls, and zero tokens. A separate no-memory CLI preflight confirmed the cause: the outer restricted execution environment denied Codex's in-process app-server initialization with `Operation not permitted`. No memory tool was reached; these are host-launch failures, not memory or agent outcomes. The same no-memory preflight succeeded when launched on the host under the user's explicit authorization, with Codex's own read-only sandbox and shell disabled.

The full pilot is restarted under v3 with this host execution mode pinned. This directory is retained only as an invalid diagnostic capture and is excluded from estimates.
