# ADR-0077: HTTP embedding maintenance

- Status: accepted — 2026-10-05
- Amends: [ADR-0042](0042-embedding-recovery-and-backfill.md) — which gives
  `reembed` ownership of HNSW index replacement ("belongs to `reembed`, which
  owns HNSW index replacement and durable progress") but was written for the
  stdio profile and did not say how an HTTP tenant reaches it
- Relates to: `docs/superpowers/specs/2026-08-27-streamable-http-saas.md` §2.1,
  §13, §16

The Streamable HTTP profile had no way to bring a tenant's stored vectors to
the deployment's current provider. Three operator situations all ended in the
same place: degraded, permanently, with a reason string naming `reembed` — a
command that exists only on the stdio binary.

1. **Provider/model/dimension change.** Stored vectors carry a foreign
   `embedding_signature`; the namespace serves lexical retrieval indefinitely.
2. **Provider outage.** Facts ingested while the provider was unreachable have
   `embedding IS NONE` and nothing in the HTTP profile ever fills them.
3. **Enable/disable cycles.** A namespace written while embeddings were off,
   then re-enabled, has facts with no vectors and no stored state — and was
   classified as "legacy embeddings require reembed", naming `reembed` as the
   exit for a namespace with nothing to re-embed *from*.

## The distinction

Two operations that look alike and are not:

| | **Class A — backfill** | **Class B — reembed** |
|---|---|---|
| Selects | `embedding IS NONE` | `embedding_signature != target` |
| Touches existing vectors | never | all of them |
| HNSW index | re-declared **only** when the namespace stores no vector; never under vectors | dropped, recreated at the target dimension |
| Reversible | yes | **no** |

**Class A is automatic. Class B requires an explicit operator decision.**

This follows from cost asymmetry rather than taste. Backfill's worst outcome is
wasted provider spend. A reembed pointed at a mistyped `EMBEDDINGS_MODEL`
overwrites every tenant's vectors and cannot be undone, at N tenants × M facts
with no cap, no opt-out and no rollback — so nothing may trigger it
automatically.

## Decisions

**Backfill is a scheduler job.** One process-level job walks
`list_ready_tenants(None, 100)`, works each tenant, logs failures without
aborting the pass, and records a single job metric. Gated on
`EMBEDDINGS_AUTO_RECOVERY`, the same variable the stdio profile reads, so one
mental model covers both.

*Rejected — a per-tenant recovery runtime as stdio has.* The runtime pool is
bounded and evicts on idle TTL, so a worker dies with the runtime it is attached
to while the need outlives it. A tenant nobody has called has no runtime, so
its accumulated unembedded facts would never be scanned — the common case in
multi-tenant, not an edge case. And it creates N×M workers over time. Backfill
also reuses `run_backfill` rather than reimplementing its batch loop, because
that loop already writes through `VectorWritePolicy::FillMissing` and routes
generation through `ProviderGeneration`, which is where the 8,000-character
input limit and the disabled-provider check live.

**Reembed is a control-plane route.** `POST /api/v1/operator/tenants/{id}/reembed`
enqueues a durable `reembed` task, registered inside the existing `operator`
router block so it inherits session → operator allowlist → CSRF, with
`require_recent_auth` on top. This is not a concession to the tool freeze; it is
what the specification already prescribes:

> `memory_mcp_http` has no ingest/extract/reembed/admin CLI commands. Operator
> automation uses the protected control-plane API.
> — `docs/superpowers/specs/2026-08-27-streamable-http-saas.md` §2.1

That sentence withdraws *CLI verbs*, not the operation. A control-plane API was
the intended mechanism from the start.

*Rejected — an MCP tool.* ADR-0016 freezes the surface at eight tools and
`AGENTS.md` requires an ADR plus an evidence gate for a ninth. Independently of
the freeze, the authorization model is wrong: tenant credentials must never be
able to trigger a fleet-wide irreversible rewrite.

*Rejected — automatic reembed on signature change.* See the cost asymmetry
above.

**The durable substrate is a `kind` column on `tenant_task`, not a second
mechanism.** Existing rows lack the value and read back as `extract`, so
in-flight extraction is unaffected by a deploy. `AGENTS.md` flags dual
substrates as a trap; adding a discriminator keeps one task table, one claim
path, one lease.

*Rejected — a second task table.* Two durable-task substrates in one codebase
diverge silently and double the number of places a recovery bug can hide.

**The task lease is heartbeated.** `claim_next_due` sets a 60-second lease with
no renewal, and a reembed over a large namespace outlives it: the row becomes
claimable again, a second replica rewrites the same vectors, and both fail their
fenced completion. The renewal reuses the store's existing `fenced_update`.

**The reembed executor builds a force-enabled service.** `prepare_reembed_pass`
refuses a service carrying a disabled provider or a `None` signature — correct
for serving, fatal for rewriting. Reusing the tenant's runtime provider would
hand the rewrite to the tenant least able to perform it, because the namespaces
that need a reembed most are exactly those the activation path degraded. The
`ForceEnabledForReembed` resolution was extracted into one shared function
called by both profiles rather than copied.

## Index ownership

**Reembed owns HNSW index replacement (ADR-0042). The activation reconcile
re-declares an index only when the namespace stores no vector.**

The two cases look identical from the mismatch alone and are not:

- **Index wrong, no vectors.** The reconcile is required: without it the first
  vector the provider writes is rejected by the stale index and backfill fails
  forever.
- **Index wrong, vectors present.** Re-declaring would strand vectors the new
  index cannot accept — precisely the state reembed exists to repair — and
  reembed could then never complete.

Both the activation path and the backfill tick ask this through one function,
`reconcile_tenant_index_dimension`, which returns an `IndexWriteGate`. They
initially answered independently and disagreed: activation declined to
re-declare while backfill went ahead, paid the provider, and had the write
rejected — every tick, for every affected tenant, with the job reported
degraded on each. The gate logs a decline at `Info` rather than counting it as
a failure, because counting it would page an operator for every tenant nobody
has reembedded yet, which after a provider change is the whole fleet.

## One dispatch

The durable scheduler has exactly one `kind` dispatch, `execute_one_task`. The
test seam passes the extraction step as an argument
(`Option<&ExtractorFn>`; production passes `None`) instead of copying the
dispatch body.

This matters because the first version did copy it: a second claim/match/policy
chain existed solely so a stub extractor could be injected, which meant every
test drove an imitation of production dispatch. A change that stopped threading
the deployment policy into the real `match` would have kept every test green
while breaking the operator's reembed route. Measured by dropping the policy:
no test before the seam was unified, one while dispatch still lived in a copy,
all four reembed tests after routing them through the real dispatch.

An unrecognised `kind` fails closed and names itself. A payload is never decoded
on a guess — the two kinds that exist are `extract` and `reembed`, and guessing
would run a maintenance task as an extraction, or an extraction as a destructive
whole-namespace rewrite.

## Out of scope

- **No per-tenant provider opt-out or credentials.** §13 makes provider policy
  deployment-level; per-tenant opt-out is not a v1 feature. Backfill finishes
  what ingest started and is not a separately metered feature.
- **No ninth MCP tool.**
- **No readiness change.** §16 keeps `/health/ready` true under a degraded
  provider. Backfill improves data in the background and must not make a
  deployment unhealthy because one tenant has many facts.
- **No lifecycle background workers per tenant.** The `LIFECYCLE_*` config is
  read and copied onto every tenant service; the workers are not spawned. Only
  the stdio profile calls `spawn_workers_from_config`. Resource growth across N
  tenants is a separate question and remains open.
- **No `embedding_job` reimplementation.** The existing cursor keyed by
  `last_completed_fact_id` already makes an interrupted pass resumable.

## Consequences

- An operator changing `EMBEDDINGS_MODEL` runs one reembed per affected tenant
  and accepts that it cannot be undone.
- A provider outage self-heals on the next scheduler tick once the provider is
  back, with no operator action and no readiness impact.
- A namespace whose vectors disagree with the deployment dimension is
  deliberately left alone by every automatic path. It stays on lexical retrieval
  until an operator asks for the rewrite.
