# Context — Memory MCP

> Shared context document for the memory_mcp codebase. This file records the
> canonical vocabulary, architectural seams, and non-negotiable constraints
> that every contributor and agent must follow.

## Public surface (frozen)

Exactly eight MCP tools. The `public_surface_snapshot` test in
`crates/memory-mcp/tests/agent_memory_lifecycle_release_gate.rs` freezes this
surface.

```text
ingest, extract, resolve, assemble_context, explain, invalidate, open_app, app_command
```

Lifecycle integration adds no public MCP tool. The one output-only onboarding
exception, `memory_mcp init`, is authorized by ADR-0030; it prints host
configuration, does not build a service, and does not change the eight-tool MCP
surface. Operator-only lifecycle maintenance is exposed through the ordinary
CLI under ADR-0047 and is not part of the MCP tool surface. Any future
public-surface proposal requires a separate ADR and the evidence gate described
in ADR-0016.

## Module seams

- `src/models/` — domain values and typed records.
- `src/service/agent_memory/` — internal lifecycle orchestration (policy,
  recall, capture, projection, worker). Not registered in `tools/list`.
- `src/service/capabilities/` — protocol-agnostic capability modules
  (ingest, extract, resolve, assemble_context, explain, invalidate).
  Each takes `&ServiceContext` (the narrow seam) and delegates to
  domain services. This is the deepening that replaced the god-object
  `MemoryService`.
- `src/service/embedding_service.rs` — embedding generation, query
  embedding caching, and background retry logic. Holds the
  `EmbeddingService` struct that owns embedding-specific concerns.
- `src/service/` — core business logic, `ServiceContext` (narrow
  shared infrastructure), `FactService`, `EmbeddingService`, lifecycle
  workers, claim reconciliation.
- `src/storage/` — `DbClient` and narrow stores. Backward compatible.
- `src/storage/agent_memory.rs` — narrow store for lifecycle events and
  durable projection jobs.
- `src/storage/claims.rs` — narrow store for the claim reconciliation
  pipeline.
- `src/tools/` — protocol-agnostic tool implementations shared by MCP
  and CLI. Each tool delegates to its matching capability via
  `ServiceContext`.
- `src/mcp/` — MCP protocol handlers.
- `src/cli/` — clap-based CLI surface, including hidden internal
  `lifecycle-capture` and `lifecycle-recall` subcommands consumed by
  hook scripts.

## Entity extraction vocabulary

**Entity Extractor**:
The stable capability that turns source text into normalized entity candidates. It is the caller-facing seam for entity extraction and does not expose model architecture, checkpoint format, or runtime details.
_Avoid_: Model, NER implementation, provider-specific extractor

**NER Backend**:
A concrete implementation of the Entity Extractor capability. Backends may differ in model family and execution strategy while preserving the same candidate semantics.
_Avoid_: Model provider, public MCP tool

**Extractor Selection**:
The configured choice of an Entity Extractor. A selection may name a lightweight backend or one supported model checkpoint; it is not a second extraction capability.
_Avoid_: NER provider, generic model

**Model Checkpoint**:
A versioned set of model artifacts used by one NER Backend. A checkpoint is not assumed to be interchangeable with another checkpoint in the same model family.
_Avoid_: Generic model, provider

## Lifecycle vocabulary

**Lifecycle Event**:
A normalized host boundary occurrence (session start, pre-tool boundary, post-compaction resume, significant post-tool result, stop) that the lifecycle bridge classifies as recall-eligible or capture-eligible.
_Avoid_: Hook, trigger, raw host signal

**Lifecycle Bridge**:
The set of standard transports (MCP stdio, hooks, AGENTS.md + skill) that deliver lifecycle events to internal capabilities. No custom socket listener or separate bridge binary.
_Avoid_: Adapter process, transport server

**Selective Recall**:
The internal capability that evaluates recall eligibility for one lifecycle event, suppresses a duplicate within the freshness window, and delegates to the existing `assemble_context` pipeline exactly once. Output is wrapped in a fixed "memory is data" preamble.
_Avoid_: Auto-recall, always-recall, recall tool

**Selective Capture**:
The internal capability that classifies one lifecycle event via a deterministic salience policy (ignored, accepted, quarantined, rejected, degraded), persists accepted evidence once, and schedules durable projection. Reuses inline-extract preparation, extraction, embedding, and claim projection.
_Avoid_: Auto-ingest, background capture tool

**Exposure Trace**:
An ephemeral per-session record of a recall's selected fact and experience IDs, retrieval fingerprint, and policy fingerprint. Held in an LRU of at most 32 traces for 30 minutes. Persists only when a later significant capture links it.
_Avoid_: Recall receipt, durable recall log

**Session Trace Registry**:
The process-local store of exposure traces, keyed by session ID. Bounded to at most 256 active sessions. Expired traces are evicted on every record. Not persisted across process restarts.
_Avoid_: Trace cache, recall state store

**Action Grounding**:
The property that a recalled memory item influenced a consequential agent action. Proven only by evaluation replay, never by the existence of a trace.
_Avoid_: Recall hit, memory access

```text
invocation origin
projection job
procedure candidate
procedure version
```

"Do not use 'discipline' as a domain noun or public feature name."

## Embedding lifecycle vocabulary

**Embedding Recovery**:
The in-process re-enablement of a configured remote embedding provider after startup degraded because the provider was unreachable. Recovery re-resolves the embedding target identity and, on success, restores semantic retrieval without a process restart. A compatible recovery uses the durable `backfill_pending` state until deferred work completes; a signature-mismatch recovery keeps semantic retrieval degraded after restart while it fills only missing vectors.
_Avoid_: Failover, auto-restart, hot reload

**Embedding Backfill**:
The deferred background generation of embeddings for facts persisted without one during a degraded period, performed at an unchanged dimension and without vector-index rebuild. Backfill completes what degradation skipped; it is not a rewrite of existing embeddings. The selection predicate is always `embedding IS NONE`; stale existing vectors remain the `reembed` responsibility.
_Avoid_: Reembed, rebuild, reindex

**Reembed**:
The deliberate maintenance operation that rewrites fact embeddings for a new embedding target identity, possibly changing dimension and rebuilding the vector index. Reembed is operator-driven and is the recovery path for dimension or signature changes that Embedding Recovery cannot handle.
_Avoid_: Backfill, migration

**Embedding State**:
The durable per-namespace record of the active embedding target identity and readiness status. It is the crash-resume marker for Embedding Recovery and Reembed, and is written by exactly one owning store.
_Avoid_: Embedding config, provider state, index state

## App surface vocabulary

**App Session**:
A process-scoped interactive session opened through `open_app` for one app (inspector, diff, graph, ingestion review, lifecycle), carrying a typed payload, bounded by an optional TTL, and closed explicitly or by expiry.
_Avoid_: UI session, connection, workspace

## Trust model

Trust is derived from the invocation channel and configured server policy.
Public MCP and CLI arguments never set final trust.

- `InvocationOrigin::AgentSelected` — ordinary path, capped at agent inference.
- `InvocationOrigin::LifecycleAdapter` — configured bridge evidence.
- `InvocationOrigin::VerifiedConnector` — independent transport identity.
- `InvocationOrigin::Operator` — operator-approved through the app surface.

Heuristics may lower trust, ignore, quarantine, or reject. They **never**
elevate trust. External content cannot become privileged instruction,
preference, policy, retraction, or procedure.

## Memory is data, never instruction

Recall output carries a fixed preamble: memory items are source-labeled data,
not system or developer instructions. Verify high-risk actions against live
sources.

## Filesystem ingestion vocabulary

**Filesystem Watch**:
An optional background capability of a running MCP server that detects supported files created or changed in its configured Ingestion Inbox and submits them to the memory ingestion pipeline. It is not a standalone operating mode.
_Avoid_: CLI watch, watch mode, watcher daemon

**Ingestion Inbox**:
The single filesystem directory explicitly assigned to one MCP server process as a source of files for Filesystem Watch. Configuring an inbox opts that process into filesystem ingestion; a process without an inbox does not watch the filesystem.
_Avoid_: Watch directory, shared folder, import database

**Inbox Source Lineage**:
The continuing identity of one inbox document across content changes. Its identity is derived from the normalized document path relative to the Ingestion Inbox; moving the document to another relative path begins a new lineage. The inbox root and absolute host path are not part of the identity.
_Avoid_: Current file, mutable episode, overwritten source

**Inbox Revision**:
One immutable content version within an Inbox Source Lineage, identified by the content bytes. A revision proceeds independently through discovery, processing, success, or terminal failure, and never overwrites an earlier revision. Its reference time comes from structured source metadata when available, otherwise filesystem modification time, otherwise observation time.
_Avoid_: File update, latest copy, mutable source

## SaaS tenancy vocabulary

**External Identity**:
A verified OAuth identity identified by the stable pair of issuer and subject. It authenticates an Account but never names or selects storage directly.
_Avoid_: User token, email identity, namespace identity

**Account**:
The stable Memory MCP identity to which one or more External Identities may authenticate. In the initial SaaS model each Account owns exactly one Tenant.
_Avoid_: OAuth subject, access token, namespace

**Tenant**:
The strict private-memory ownership and authorization boundary of the SaaS deployment. The initial model has one Account per Tenant and does not share memory between Tenants.
_Avoid_: User token, workspace, shared scope

**Tenant Namespace**:
The SurrealDB namespace exclusively bound to one Tenant by control-plane state. It is selected from authenticated server context and is never accepted as a data-plane request argument.
_Avoid_: Active Namespace, user-supplied namespace, scope

**Tenant Registry**:
The control-plane mapping from Accounts and External Identities to Tenant storage bindings and provisioning state. It is isolated from Tenant Namespaces and is not accessible through MCP domain tools.
_Avoid_: Namespace lookup table, request scope, tenant data

**Tenant Runtime**:
A bounded, process-local activation of services for exactly one Tenant Namespace. It is selected only after authentication and Tenant Registry resolution, is never rebound to another Tenant, and may be unloaded without changing Tenant data or identity.
_Avoid_: Active Namespace, transport session, user connection, mutable tenant context

**Account API Key**:
A named, independently revocable credential owned by an Account and presented as a Bearer credential. Its secret is shown only when issued; durable state contains an identifier and irreversible verifier, never the recoverable secret. In the initial SaaS model every valid key authorizes the full public MCP capability set of its Account.
_Avoid_: OAuth identity, Tenant selector, stored secret

**Authenticated Principal**:
The request-scoped server identity produced by successful credential verification and Account-to-Tenant resolution. API keys and future OAuth access tokens produce the same principal shape; neither credential contents nor MCP arguments select a Tenant Namespace.
_Avoid_: Access token, transport session, namespace parameter

**Active Namespace**:
The single immutable SurrealDB namespace selected at startup by a local stdio server process. It remains a local deployment concept and is not the SaaS Tenant Namespace selected for an authenticated HTTP request.
_Avoid_: Tenant Namespace, dynamic namespace, request namespace

## Constraints

- Production code uses `MemoryError` and `Result`; no production `unwrap`,
  `expect`, or `panic`.
- No lock guard lives across `.await`.
- Metrics labels use bounded enums only.
- Migration files are append-only.
- Preserve raw episodes and source facts. Contradiction, supersession,
  correction, source retraction, privacy erasure, procedure deprecation, and
  procedure revocation remain separate operations.
- Never let recall or a background worker manufacture a corrective fact as a
  retrieval side effect.
- A local stdio server process uses one Active Namespace. The SaaS HTTP profile
  instead resolves an authenticated Tenant to an immutable Tenant Runtime;
  neither profile accepts namespace selection as a data-plane request field.
- Scope, project, collection, basket, and vault are not Memory MCP partitioning
  concepts. Tenant is the private ownership boundary of the SaaS HTTP profile.
- Zero-configuration startup uses embedded SurrealDB, Active Namespace `main`,
  and database `memory`; it requires no `SURREALDB_*` variables, external
  service, or credentials.
- `SURREALDB_NAMESPACE` may select one Active Namespace for the whole process.
  Changing it takes effect only after restart and never moves, merges, copies,
  or deletes data in another namespace.
- Zero-configuration embedded storage uses the existing platform-conventional
  user-data resolution and compatibility rule. `SURREALDB_DATA_DIR` is the sole
  explicit override; storage location never depends on the executable.

## Evaluation domain language

This section defines the language used by evaluation ADRs, profiles, artifacts,
and implementation plans.

**Eval Profile**:
A named execution and gating policy for a particular feedback horizon:
`pr`, `release`, or `nightly`. It declares suites, modes, corpus coverage,
resources, and a time budget.
_Avoid_: Test group, ad hoc Make target

**Eval Mode**:
The system path whose behavior is being measured: `retrieval-only`,
`end-to-end`, `lifecycle`, or `performance`. Results from different modes are
reported separately.
_Avoid_: Benchmark type, hidden setup variant

**Eval Case Outcome**:
Exactly one of `passed`, `quality_failed`, or `invalid`. Invalid means that the
intended measurement could not be made; it never means skipped or passed.
_Avoid_: Skipped failure, infrastructure pass

**Evaluation Corpus**:
An immutable dataset revision identified by a manifest containing its source,
revision, digest, license, size, case count, and adapter version.
_Avoid_: Latest dataset, downloaded fixture

**Label Trust**:
The provenance class of expected evidence: `official`, `reviewed`, or `weak`.
Weak labels are diagnostic and cannot contribute to a release gate.
_Avoid_: Confidence score, implicit oracle

**Evaluation Artifact**:
A versioned machine-readable report containing all case outcomes, metrics,
coverage, thresholds, fingerprints, retries, and durations for one run.
_Avoid_: Console log, stdout baseline

**Quality Gate**:
A declared release decision combining use-case-derived hard floors with an
allowed regression budget against an approved baseline.
_Avoid_: Printed target, best-effort threshold

**Baseline**:
An approved Evaluation Artifact used for typed regression comparison. Replacing
it requires review, before/after evidence, and a reason.
_Avoid_: Previous stdout, latest local run

## Memory domain language

This section defines the language used to represent durable knowledge,
temporal change, and disagreement between sources.

### Vocabulary

**Active Namespace**:
The single native SurrealDB namespace selected for one Memory MCP server process. All memory operations in that process use it implicitly; `main` is selected when no override is configured. This is the implemented runtime language from ADR-0038; compatibility work for historical records and the remaining release gates is tracked in the one-active-namespace plan.
_Avoid_: Scope, project, collection, basket, tenant, vault, default namespace

**Fact**:
A durable, provenance-backed evidence item derived from an episode. A fact may contain zero or more claims. Claim supersession does not modify or invalidate the source fact.
_Avoid_: Claim, assertion

**Claim**:
An atomic, machine-comparable proposition derived from a fact, with a canonical subject, comparison key, typed value, and validity context. Claims are created only when that structure can be determined reliably.
_Avoid_: Fact, raw statement

**Claim Relation**:
A persisted, versioned reconciliation decision connecting two claims. Its outcome is duplicate, supersession, correction, contradiction, or temporal ambiguity, and it retains the decision reason, evidence, evaluator version, and evaluation time. A later evaluation supersedes the earlier relation version rather than rewriting it.
_Avoid_: Claim, unversioned warning

**Claim Slot**:
The exact comparison boundary formed by access-policy fingerprint, canonical subject, compatible claim schema, and comparison key within the Active Namespace. Qualifiers do not define the slot; their differences are evaluated during reconciliation. Only claims in the same slot are candidates for automatic reconciliation.
_Avoid_: Fuzzy topic, entity overlap, qualifier barrier, scope, project

**Claim Projection**:
The versioned deterministic derivation of zero or more claims from an immutable source fact. Recomputing a projection may replace the current derived claims in transaction time but does not modify the fact.
_Avoid_: Source fact, destructive rewrite

**Reconciliation**:
The deterministic process that selects claims in the same claim slot and classifies their relationship using typed values, cardinality, validity, source continuity, and explicit correction evidence.
_Avoid_: Fuzzy similarity, latest-write-wins

**Claim Schema**:
A reusable structural form that defines a claim's slots and comparison semantics without enumerating real-world properties. A small schema set can represent an open-ended set of metrics, attributes, relations, commitments, and decisions.
_Avoid_: Property catalog, domain ontology entry

**Comparison Key**:
A stable, deterministic identifier for the specific dimension, attribute, or relation expressed by a claim. Claims participate in automatic comparison only when their comparison keys match directly or through a confirmed alias.
_Avoid_: Arbitrary predicate string, fuzzy topic

**Possible Alias**:
A non-authoritative suggestion that two comparison keys may express the same concept. A possible alias never authorizes automatic claim comparison until it is confirmed.
_Avoid_: Confirmed alias, automatic merge

**Observation Time**:
The time when a source recorded or reported a claim. Observation time is evidence about recency but does not imply when the claim became or stopped being true.
_Avoid_: Valid from, ingestion time

**Validity Interval**:
The period during which a claim is true in the world. Its start or end may be unknown when the source provides no reliable temporal evidence.
_Avoid_: Observation time, ingestion time

**Transaction Validity**:
The period during which a derived claim or claim relation is the system's current representation. It is separate from the claim's real-world validity interval and preserves what the system knew before re-extraction or correction.
_Avoid_: Validity interval, observation time

**Temporal Ambiguity**:
An outcome in which claims differ but the available temporal evidence cannot establish whether their validity intervals overlap or one supersedes the other.
_Avoid_: Contradiction, supersession

**Cardinality Policy**:
The rule that states whether a comparison key may have several simultaneous values or at most one value within matching qualifiers and validity. An unknown comparison key is set-valued by default.
_Avoid_: Global singleton predicate list

**Source Lineage**:
A sequence of observations that represent successive versions or snapshots of the same logical source record. Lineage establishes continuity, not authority over unrelated sources.
_Avoid_: Source type, ingestion order

**Authoritative Source**:
A source explicitly trusted to determine the current value for a defined claim schema or knowledge domain. No source is authoritative merely because it is newer.
_Avoid_: Latest source, preferred source

**Contradiction**:
A relationship between two claims that cannot both be true in the same context during an overlapping validity interval. A contradiction does not make either source fact invalid.
_Avoid_: Update, supersession

**Supersession**:
A temporal transition in which a claim with known validity replaces an earlier value for the same subject and comparison key from a specific validity time onward. Supersession closes the earlier claim's validity interval, not its source fact.
_Avoid_: Contradiction, correction

**Correction**:
An explicit replacement of a claim because the earlier assertion was wrong for the same validity context. Correction closes the earlier claim in transaction time rather than pretending that the world changed, and it leaves the source fact available as historical evidence.
_Avoid_: Supersession, whole-fact retraction

**Retraction**:
An explicit withdrawal of a source fact because the source assertion was erroneous, withdrawn, corrupted, or ingested incorrectly. A retracted fact and its derived claims are excluded from active truth selection but retained for provenance and audit.
_Avoid_: Supersession, contradiction
