# ADR-0072: One owner for Tenant runtime activation

- Status: implemented — 2026-10-03, in `fix(http): give Tenant runtime
  activation one cancellation-safe owner`
- Date: 2026-10-03
- Related: ADR-0046, ADR-0052, ADR-0058, ADR-0066
- Implementation: [architecture audit follow-up](../superpowers/plans/2026-10-03-architecture-audit-follow-up.md)

## Decision

The HTTP runtime pool is the sole owner of runtime residency, single-flight
activation, activation timeout, negative backoff, capacity and idle eviction.
Tenancy retains trusted Tenant resolution, request-versus-maintenance lifecycle
policy, binding comparison and the narrow runtime factory port. It does not keep
a second runtime cache or activation registry. This refines the runtime split
under ADR-0058; it does not move business policy into the HTTP adapter.

Activation remains owned by the request that starts it. Dropping that request,
including on deadline expiry, cancels the factory future and synchronously
finishes the attempt through a generation-fenced RAII guard. Followers receive a
terminal failure, and a later request can retry. Cancellation does not create a
negative-backoff entry; ordinary factory failure and activation timeout retain
the existing bounded backoff. Cancelling a follower never cancels the leader.

The pool has one short-held synchronous bookkeeping lock so cleanup cannot
depend on another asynchronous task being scheduled. No state lock crosses an
await; factory work, logging and runtime destruction happen outside it. A
generation can be completed only once; cleanup from an older generation cannot
clear or publish into a newer one. Empty cancelled/failed slots are reclaimable
under capacity pressure. Runtime pins are reserved under the same state lock
that authorizes reuse, before waiting for activation or a concurrency permit.

Tenant binding identity is `(tenant_id, database, namespace)`, not the whole
runtime specification. Mutable lifecycle status must still pass resolution on
every request; it is neither reuse equality nor cached permission. Plan, schema
and concurrency changes are handled as bounded runtime replacement after
existing pins drain, not as an immutable-binding conflict and not by silently
returning the old runtime. There is no permanent binding cache beyond the
resident slot; the durable registry remains authoritative.

## Why this decision needs a record

The existing pool and Tenancy both retain activation senders, runtimes and
timeouts. A request cancellation bypasses both normal-return cleanup paths,
leaving a sender alive without a producer. Two owners also require eviction
notifications to keep their caches synchronized.

There were two credible alternatives:

1. **Keep both owners and add cancellation cleanup to each.** This fixes the
   observed symptom but retains duplicate lifecycle state and the eviction
   synchronization contract that made the failure difficult to see.
2. **Make activation a supervised pool-owned background job.** A disconnected
   leader would not waste cold-start work, but shutdown would need to cancel and
   join those jobs, fence late publication, and dispose of abandoned runtimes.
   This cannot be justified as a best-effort derivation under ADR-0046. Current
   use cases require bounded recovery, not warming a runtime after every caller
   has gone away.

Request-owned cancellation is the smaller coherent choice. It can repeat cold
work after a disconnect, and current followers fail rather than automatically
electing another producer. Those costs are preferable to a second task lifecycle
or a hidden retry loop. Existing environment-configured deadlines, capacity and
activation timeouts remain independent; no timeout-ordering workaround or new
configuration knob is introduced.

## Implementation obligations

- [x] Exercise the same acquisition interface from the production factory
  adapter and a deterministic blocked/failing factory adapter.
- [x] Cover leader cancellation, follower cancellation, factory panic unwinding,
  generation fencing, capacity reclamation, pinning, binding mismatch and a
  runtime-revision change.
- [x] Remove the redundant Tenancy lifecycle implementation; its lifecycle tests
  moved to the pool rather than being kept against a deleted cache.
- [x] Keep factory cancellation safe for resources acquired before it returns:
  the attempt guard owns cleanup, so unwinding releases the slot.
- [ ] Verify the real HTTP deadline path end to end. The HTTP fixtures spawn the
  server as a subprocess and expose no factory seam, so this remains open and is
  recorded as such in the plan rather than claimed.
- [x] Do not claim this decision is implemented beyond the evidence above.
