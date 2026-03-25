# Lifecycle background jobs follow-up

The current remediation wave keeps lifecycle correctness in the request path and deliberately avoids introducing asynchronous consolidation work.

## Deferred background jobs

The following work should happen in a later operational wave, not inside the correctness-focused remediation:

- **confidence decay refresh**
  - periodically recompute long-lived ranking caches that depend on temporal decay
  - keep request-time confidence logic authoritative until cache invalidation rules are proven
- **community consolidation**
  - compact duplicate or stale community records left behind by older implementations or interrupted writes
  - optionally rebuild connected components from the persisted graph on a schedule
- **embedding backfill**
  - populate missing embeddings after a real provider is enabled
  - reindex vector fields after dimension or provider changes

## Why this is deferred

Mixing background mutation with the current correctness wave would make it harder to prove:

- which request-path writes are authoritative
- whether invalidation and community updates are deterministic
- whether migration startup checks are rejecting drift correctly

For now, request-path correctness wins. Background jobs can come later once the steady-state rules are fully validated.
