# Architecture audit follow-up design

**Status:** Accepted direction

**Implementation:** planned, not implemented. The production changes below have
not been made; the linked plan carries the task-level gates. Design decisions
were made autonomously under the user's instruction to decide and act.

**Baseline:** `e506d99` (`v1.23.0`). This is a follow-up to the September 30
remediation, not a replacement or a claim that its checked steps never landed.

**Plan:** [implementation tasks and verification](../plans/2026-10-03-architecture-audit-follow-up.md).
**Decision:** [ADR-0072](../../adr/0072-single-owner-for-tenant-runtime-activation.md).
**Vocabulary:** [GLOSSARY.md](../../../GLOSSARY.md).

## Outcomes and priority

| Order | Finding | Required outcome |
|---|---|---|
| 1 — P1 | Cancelled cold activation retains senders without a producer | One cancellation-safe activation owner; retries and capacity recover |
| 2 — P2 | Canonical vector replacement writes an entire stale fact snapshot | Embedding-only conditional updates; actual write outcome returned |
| 3 — P2 | Preflight rejects unsupported initialization proposals | rmcp negotiates proposals; mirror and later-request checks remain |
| 4 — P2 | Source and public-surface guards accept counterexamples | Rooted traversal and syntax-aware bounded guards reject fixtures |
| 5 — P2 | No-default process tests exist but CI never executes them | Explicit no-default behavioral test row runs both tests |

An independent source traversal reached all 391 current production source files
across the union of feature declarations. No present undeclared file was found.
That result is not semantic function-call reachability. The trait-caller guard
matches method text and must not be described as proof of production use.

## Resolved design questions

### Tenant activation

Choose request-owned cancellation with synchronous RAII cleanup over supervised
background activation. The runtime pool owns technical resource lifecycle;
Tenancy owns binding and resolution policy. Keep the factory seam because it has
a production adapter and a useful blocked/failing test adapter.

Cancellation, timeout and factory error are different terminal causes:
cancellation permits immediate retry; timeout/error preserve bounded backoff.
No follower waits on a sender without a live producer. Cleanup is generation
fenced. Abandoned empty slots cannot exhaust capacity. Pins protect a selected
runtime before an asynchronous concurrency wait begins.

Storage identity changes fail closed. Mutable plan/schema/concurrency revisions
do not redefine storage identity: a runtime needing replacement is drained under
existing bounds rather than returned stale or rejected as a binding conflict.
Lifecycle status is not reuse equality; neither a warm runtime nor a completed
activation bypasses trusted resolution. Shutdown terminates pending acquisition
without starting detached cleanup and prevents late runtime publication.

### Canonical vectors

Keep the existing canonical-vector module and port; deepen the adapter rather
than adding another abstraction. Both FillMissing and ReplaceStale use an atomic
conditional mutation of embedding fields only, targeting the record through the
parameterized `type::record('fact', $id)` form rather than interpolating an ID
into a quoted identifier. A preliminary read may save work but must not authorize
an unconditional write.

FillMissing preserves every existing vector, including an unsigned legacy vector.
ReplaceStale writes when no vector exists or its signature differs; it preserves
an existing vector with the requested signature. Missing records produce
NotFound, conditional no-ops produce AlreadyCurrent, writes produce Applied, and
storage failures remain failures. Disabled generation remains Skipped.

Return the durable outcome instead of deriving Applied from `Ok(())`. Keep
model/dimension metadata consistent with the validated vector identity. Audit
the access-count writer on this same record: it must not write a full snapshot
back and undo a newly stored vector. This reciprocal fix is necessary to close
the concurrency invariant, not a general storage refactor.

Do not overclaim the invalidation consequence: fields absent from the stale
snapshot are not SET by the current query builder. The concrete regression is a
concurrent change to a field present in the snapshot, such as access_count.

### Protocol proposals and negotiated revisions

An `initialize` body's `protocolVersion` is a proposal. Its membership in the
known-legacy list is not a prerequisite to reaching rmcp negotiation. Era
detection remains body-derived; modern metadata, mirrored method/name headers,
host/origin checks, authentication and Admission remain unchanged.

Do not relax protocol-header validation or contradiction checks as a side effect.
Test unsupported proposals without a protocol-version header and both known and
modern-looking proposal values. Malformed initialization is still rejected.
Subsequent legacy requests cannot claim a modern selected revision.

### Evidence guards

Guard the union of declared feature paths from Cargo source roots, not incoming
edges from every file on disk. Discover targets with `cargo metadata --no-deps`;
use the existing serde_json dependency. No new parser dependency is authorized.

A bounded, test-only Rust token reader handles only the syntax needed by current
guards, including comments, strings, attributes, module items and use trees.
Keep the existing prohibition on production path attributes, including hidden
cfg_attr path overrides: none is used today. Do not implement hypothetical path
resolution. Unsupported constructs affecting traversal or guarded exports
produce explicit diagnostics rather than silently passing. This is not a new
Rust compiler or a claim of type-resolved call reachability.

Share lexical mechanics only where two guard callers need them. Keep source,
public-surface and documentation assertions separate; ADR-0065 rejects combining
unrelated guard failures into one test. Mark the trait-caller check as lexical,
exclude comments/strings, and document its receiver-type limitation.

### CI behavior

Add a direct Cargo test command for the existing filesystem-disabled process
tests. Make those tests' subprocess lifetime bounded, with kill-and-reap on
expiry or assertion failure, before the row is enabled: both current helpers can
block indefinitely on a child. Keep compile-only feature checks distinct. Do not
change product defaults, add a profile framework, duplicate the platform matrix,
or make real model downloads prerequisites for this test row.

## Non-goals and constraints

- No new MCP tools, migrations, dependency changes, deployment action or release.
- No changes to public environment-variable names/defaults or stateless transport.
- No shared container injected into a bounded context and no new business logic
  placed in `src/service/`.
- No detached activation cleanup, permanent binding cache or generic task manager.
- No full rewrite of storage, parsing, auth, admission or observability.
- No claim that a source guard proves every function is reachable.
- No test removal justified solely by cleaner internals: relocate behavioral
  assertions to the new owner and add the missing failure scenarios.

## Acceptance evidence

The plan maps each finding to production callers, tests, commands and deletion
checks. Every defect needs a regression observed failing before its fix.
Whole-suite and lint success supplement, not replace, the targeted evidence.
Real-client interoperability remains unverified until actual clients are driven;
synthetic protocol tests must not change that documentation status.
