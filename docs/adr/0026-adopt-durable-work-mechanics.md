# ADR-0026: Adopt Durable Work Mechanics as the Worker Timing Home

> Status: Accepted (2026-08-01)
> Completes the loose end left by the lifecycle integration (ADR-0016): the
> shared worker-timing module was added but never wired, and workers
> re-declared its constants locally.

## Context

`service/durable_work.rs` defines `DEFAULT_EMPTY_POLL_SECS`,
`DEFAULT_TRANSIENT_BACKOFF_SECS`, `DEFAULT_LEASE_SECS`,
`DEFAULT_MAX_ATTEMPTS`, and the matching duration helpers plus
`is_transient`. Every item carries `#[allow(dead_code)]` and the module has
zero consumers: the only occurrence of the string `durable_work` outside the
file is the `pub(crate) mod durable_work;` declaration in
`crates/memory-mcp/src/service.rs`.

Meanwhile the same constants are re-declared in at least two workers:

- `service/agent_memory/worker.rs:16-17` — `EMPTY_POLL_INTERVAL_SECS = 10`,
  `TRANSIENT_BACKOFF_SECS = 5`
- `service/agent_memory/projection.rs:18-24` — `DEFAULT_LEASE_SECS = 120`,
  `DEFAULT_MAX_ATTEMPTS = 5`

Three definitions of the same four values; the shared home is the dangling
one. This is the worker-layer instance of the leak ADR-0025 removes at the
eval layer — the "single formula home" instinct failed at the seam between
`agent_memory` and `claims` bounded contexts, exactly the seam
`durable_work.rs` was written to cover.

Friction today: changing lease or backoff policy means editing N sites and
remembering which one is authoritative (none is). New workers copy the
nearest existing worker, propagating stale values.

## Decision

Make `durable_work` the single timing/policy home for all durable workers
(lifecycle event worker, projection worker, community rebuild, decay,
archival, claim backfill and reconcile workers):

1. Workers import `durable_work::{empty_poll_backoff, transient_error_backoff,
   lease_duration, DEFAULT_MAX_ATTEMPTS, is_transient}` and delete their local
   constant definitions.
2. Where a worker intentionally uses a different value, it declares an
   override *at its own call site* (`Duration::from_secs(20)`), visibly, not
   by shadowing the shared default.
3. Remove every `#[allow(dead_code)]` from `durable_work.rs`; items still
   unused after step 1 are deleted rather than annotated.
4. Keep the existing per-worker tests; add a single test in
   `durable_work.rs` asserting defaults only if not already present.

The public tool surface, claim schemas, and durability semantics are
unchanged — this is a constant-and-helper relocation, not a behavior change.

## Consequences

- One edit changes a default everywhere; overrides are explicit at call
  sites — locality for policy, leverage over N workers.
- The delete test passes on `durable_work`: removing the module would now
  force every worker to redefine shared timing, so the module earns its
  seam.
- Zero benchmark impact expected — no hot path touched; verified via the
  stage gate on the card (PR profile).

## Alternatives Considered

### Delete `durable_work.rs` and keep per-worker constants

Rejected — constants have already drifted (three homes); the module
documents the intended end-state and the values match today's workers.
Deletion consolidates nothing; adoption does.

### Make every worker configurable via config

Rejected — scope creep; CONTEXT.md has no config surface for worker timing
and no caller asked for one (YAGNI).

## Verification

- `grep -rn 'DEFAULT_EMPTY_POLL\|EMPTY_POLL_INTERVAL_SECS\|TRANSIENT_BACKOFF\|DEFAULT_LEASE\|DEFAULT_MAX_ATTEMPTS' crates/memory-mcp/src` shows exactly one definition per constant (in `durable_work.rs`); all other matches are uses.
- `grep -c 'allow(dead_code)' crates/memory-mcp/src/service/durable_work.rs` = 0.
- `cargo test --workspace --all-targets --features cli-watch,mcp-apps` passes.
- PR eval profile gates hold at v5 observed values (no behavioral change expected; this is a smoke check for accidental worker loop changes).
