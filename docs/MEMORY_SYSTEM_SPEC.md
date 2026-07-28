# Memory System — Unified Specification

**Version:** 2.4<br>
**Date:** July 17, 2026<br>
**Status:** Consolidated (supersedes all previous SPEC.md versions)

---

## Document Change History

- **2026-07-17**: Added the deterministic Claim/ClaimRelation reconciliation target: contradiction versus supersession/correction/retraction semantics, exact claim slots, source and cardinality gates, trace/Prometheus requirements, append-only automatic migrations, resumable legacy backfill, backward-compatible MCP enrichment, and TDD/evaluation gates. See `docs/CONTRADICTION_DETECTION_DESIGN.md` and ADR-0002 through ADR-0015.
- **2026-03-27**: Added explicit reference to `docs/superpowers/specs/2026-03-27-sota-memory-alignment-design.md` as the adaptive-memory target-state companion to this runtime spec. Clarified that SOTA alignment work must preserve the approved lexical/BM25 + graph direction and should generally land under the existing MCP tool surface.
- **2026-03-27**: Fixed critical issues from code review: (1) `namespace_for_scope()` now normalizes scope to lowercase before prefix matching and logs warn for unknown scopes; (2) confirmed `select_entities_batch()` is already used in hot path (`expand_query_with_aliases`); (3) entity aliases are normalized at write time via `normalize_text()`, ensuring consistent lookup. Updated entity extraction status to reflect Unicode-aware regex with `person`/`technology` classification.
- **2026-03-26**: Added `docs/SIMPLIFIED_SEARCH_REDESIGN_SPEC.md` as the target-state specification for the upcoming breaking search redesign. That redesign removes embedding/HNSW runtime support in favor of BM25/full-text primary retrieval plus bounded graph expansion and deterministic fusion.
- **2026-03-25**: Completed remediation waves for indexed entity lookup, provenance persistence, edge invalidation, native `RELATE` graph storage, DB-side intro traversal, semantic scaffolding, community-aware retrieval, and checksum-enforced versioned migrations. Verified in this pass with `cargo test semantic_scaffolding --test service_integration` (2 passed), `cargo test --test service_acceptance` (11 passed), and `cargo test --test service_integration` (11 passed).
- **2026-03-25 (embedding follow-up)**: Added configurable `SURREALDB_EMBEDDING_DIMENSION`, DB-side community summary full-text search, and an explicit manual-reindex warning for dimension changes. Verified with strict `cargo clippy --all-targets -- -D warnings` and full `cargo test`.

- **2026-03-11**: Completed the cleanup of the memory-only MCP surface. Removed legacy non-memory service APIs (`create_task`, `send_message_draft`, `schedule_meeting`, `update_metric`, `ui_*`) from `MemoryService` and narrowed the public contract to six canonical memory tools. Updated service internals to return typed extraction, context, and explanation models, refreshed `README.md` and this specification, and revalidated with `cargo fmt --all`, `cargo test`, and `cargo clippy --all-targets -- -D warnings`.

- **2026-02-20**: Fixed `create_task` optional `due_date` coercion regression under SurrealDB 3 by preserving JSON `null` in DB write payload normalization (instead of converting to `{"None": {}}`, which SurrealDB interpreted as an object and rejected for `option<string>`). Added regression coverage for `create_task` with `due_date: null` parameter parsing, payload normalization, and integration flow without due date. Revalidated with full `cargo fmt`, strict `cargo clippy --all-targets --all-features -- -D warnings`, and full test suite.
- **2026-02-20 (hotfix)**: Fixed SurrealDB server-version detection — `INFO FOR DB` response parsing now prefers explicit `version` keys and version-like strings (semver/SurrealDB labels) and ignores non-version text (DDL/statements). This prevents logging migration DDL as the server version (e.g. `DEFINE ANALYZER ...`). Added unit tests for `find_version_in_json` and verified startup logging no longer reports DDL as the server version.
- **2026-02-19**: Completed SurrealDB 2.x → 3.x migration validation. Fixed edge persistence regression by omitting optional invalidation fields when absent, added missing `edge_id` field and missing runtime tables (`community`, `event_log`, `task`) in schema initialization, updated deprecated SurrealQL syntax (`SEARCH ANALYZER` → `FULLTEXT ANALYZER`, `string::is::datetime` → `string::is_datetime`), and revalidated with `cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings`, and full test suite.
- **2026-02-06**: Replaced `CONTAINS` substring matching with SurrealDB full-text search (`@@` operator) + per-word fallback for `assemble_context`; added query preprocessing (`preprocess_search_query`); comprehensive test coverage (unit, acceptance, embedded FTS integration).
- **2026-02-05**: Consolidated three specifications into single source of truth
- **2026-02-05**: Implemented bi-temporal edge filtering for graph traversals; updated traversal API (`find_intro_chain`) to accept `as_of`; updated mocks and tests; full test suite and clippy clean.
- **2026-01-22**: Memory Agent architecture finalized

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [System Architecture](#2-system-architecture)
3. [Scope and Definitions](#3-scope-and-definitions)
4. [Users and Access Control](#4-users-and-access-control)
5. [Functional Requirements](#5-functional-requirements)
6. [Data Model](#6-data-model)
7. [MCP Tool Surface](#7-mcp-tool-surface)
8. [Non-Functional Requirements](#8-non-functional-requirements)
9. [Implementation](#9-implementation)
10. [Testing and Acceptance](#10-testing-and-acceptance)
11. [Configuration and Deployment](#11-configuration-and-deployment)
12. [References](#12-references)

---

## 1. Executive Summary

### 1.1 Product Vision

> Note: the **current runtime** is described by this document. The approved **next breaking retrieval target** is described separately in `docs/SIMPLIFIED_SEARCH_REDESIGN_SPEC.md`, and the broader **adaptive-memory target state** is described in `docs/superpowers/specs/2026-03-27-sota-memory-alignment-design.md`. This document remains the source of truth for shipped behavior.

Memory System provides agents with a unified long-term memory and context layer that:
- Aggregates source material into episodes
- Transforms episodes into a **bi-temporal knowledge graph** (facts + relationships) backed by native SurrealDB relation edges and native `datetime` temporal fields
- Delivers compact context packs to LLMs on-demand with minimal token budget; current implementation combines lexical retrieval, community summaries, and graph traversal, while full hybrid embedding ranking remains gated behind the default `NullEmbedder`
- Supports personal, team, and organizational scopes with strict access control

### 1.2 Key Design Principles

1. **Separation of Concerns**: A specialized Memory Agent handles all memory operations, while the Product Manager Agent delegates via `runSubagent`
2. **Bi-temporal Modeling**: Track both "when was it true" (validity time) and "when did we learn it" (transaction time) to support accurate historical queries and reliable audit trails
3. **Determinism**: All operations produce stable, reproducible results, with no randomness and consistent sort order
4. **Access Control**: Strict scope isolation (personal/team/org/private-domain) with policy-based filtering
5. **Single Source of Truth**: SurrealDB as the only storage backend (no in-memory alternatives)

### 1.3 Architecture Overview

```
┌─────────────────────────────────────────┐
│     Product Manager Agent (PDM)         │
│  - Strategy & roadmapping               │
│  - Stakeholder management               │
│  - Requirements engineering             │
│  - Delegates memory ops to Memory Agent │
└──────────────┬──────────────────────────┘
               │ runSubagent("memory", ...)
               ▼
┌─────────────────────────────────────────┐
│        Memory Agent (Specialized)        │
│  - Ingest episodes (email/TFS/docs)     │
│  - Extract entities (deduplication)     │
│  - Extract facts (promises/tasks/etc)   │
│  - Assemble context (temporal queries)  │
│  - Stakeholder analysis                 │
│  - Decision tracking                    │
└──────────────┬──────────────────────────┘
               │ mcp_memory-mcp_*
               ▼
┌─────────────────────────────────────────┐
│          Memory MCP Server              │
│  - rmcp (Rust)                          │
│  - SurrealDB backend (embedded / remote)│
│  - Bi-temporal knowledge graph          │
└─────────────────────────────────────────┘
```

**Delegation Pattern:**

- **PDM Agent** focuses on product strategy, roadmapping, requirements engineering
- **Memory Agent** (specialized sub-agent) handles all memory operations
- Skills like `context-assembly`, `entity-tracking`, `stakeholder-analysis`, `decision-tracking`, and `ingest-episode` are embedded in the Memory Agent prompt rather than exposed as standalone skills
- PDM delegates via `runSubagent(agentName: "memory", ...)` rather than calling MCP tools directly

### 1.4 Implementation Reality Check (2026-03-25)

The target architecture remains valid, but several roadmap items are intentionally staged rather than fully “done forever.” The current implementation reality is:

- Temporal fields are persisted as native SurrealDB `datetime` / `option<datetime>` values, with write-time coercion handled in `build_set_assignments()`.
- Retrieval is currently lexical-first, then augmented by community-summary and graph signals; embedding retrieval remains scaffolded but disabled by the default `NullEmbedder`.
- `explain()` now expands provenance back to the source episode, including citation text and timestamp context.
- Community maintenance is implemented as a deterministic connected-components baseline, while more advanced clustering/consolidation remains deferred.
- Embedded/local deployments intentionally keep a shared `Mutex<Surreal<_>>` because namespace rebasing (`use_ns` / `use_db`) is session-scoped; a namespace-scoped client pool remains known throughput tech debt.
- `RegexEntityExtractor` is the deterministic fallback extractor today; broader multilingual / NLP extraction remains a follow-up.

The current repository direction also intentionally constrains future work:

- retrieval evolution should remain lexical/BM25 + graph first unless explicitly re-approved,
- SOTA-inspired improvements should prefer internal service behavior over MCP tool-surface growth,
- target-state ideas such as heat-aware retention, time-aware query expansion, and reflective usage signals are roadmap work, not claims about the current runtime.

---

## 2. System Architecture

### 2.1 Component Responsibilities

| Component | Responsibilities |
|-----------|------------------|
| **PDM Agent** | Strategy, roadmapping, stakeholder management, requirements engineering. Delegates memory operations. |
| **Memory Agent** | Episode ingestion, entity extraction, fact extraction, context assembly, stakeholder analysis, decision tracking. Encapsulates memory skills. |
| **Memory MCP Server** | Exposes MCP tools (`ingest`, `extract`, `resolve`, `assemble_context`, etc.), manages SurrealDB lifecycle, migrations, rate limiting. |
| **SurrealDB** | Stores all memory objects (Episode/Entity/Fact/Edge/Community), provides schema/index primitives, backs lexical retrieval, native relation edges, community maintenance, and embedding-index scaffolding. Full hybrid embedding retrieval remains intentionally disabled by default. |

### 2.2 Design Rationale

**Why a Dedicated Memory Agent?**

1. **Separation of Concerns**: PDM focuses on product; Memory Agent handles storage/retrieval
2. **Context Isolation**: Memory Agent has its own context window; can handle large extractions without polluting PDM's context
3. **Encapsulation of Skills**: Skills embedded in agent prompt (guaranteed execution, no experimental feature dependency)
4. **Tool Access Scoping**: Memory Agent has direct access to `memory-mcp/*` tools; PDM can't accidentally bypass
5. **Intent-Designed Contracts**: Minimal tool surface with high-level behaviors; MCP encapsulates complexity (input repair, fallbacks, normalization)

### 2.3 Delegation Examples

#### Example 1: Ingest Email

```typescript
// User: "Process my recent emails"

// PDM delegates:
runSubagent({
  agentName: "memory",
  description: "Process emails",
  prompt: "Fetch recent emails from apple-mail and ingest into memory with entity/fact extraction"
})

// Memory Agent:
// 1. Fetches emails via apple-native-tools/apple-mail
// 2. For each email: mcp_memory-mcp_ingest
// 3. For each episode: mcp_memory-mcp_extract
// 4. Returns summary: "Ingested 15 emails, extracted 23 entities, 47 facts"
```

#### Example 2: Stakeholder Brief

```typescript
// User: "What do we know about John Smith?"

// PDM delegates:
runSubagent({
  agentName: "memory",
  description: "Stakeholder analysis",
  prompt: "Generate comprehensive stakeholder brief for John Smith with promises, metrics, decisions, and relationship graph"
})

// Memory Agent:
// 1. mcp_memory-mcp_assemble_context (query: "John Smith all facts", budget: 30)
// 2. mcp_memory-mcp_assemble_context (query: "promises John Smith", budget: 10)
// 3. mcp_memory-mcp_assemble_context (query: "metrics John Smith", budget: 5)
// 4. mcp_memory-mcp_explain (for citations)
// 5. Returns formatted markdown with tables and sources
```

---

## 3. Scope and Definitions

### 3.1 Core Concepts

| Term | Definition |
|------|------------|
| **Episode** | Primary "raw" fragment from a source (email, transcript, message) with source reference and timestamp |
| **Entity** | Person/company/project/deal/object extracted from episodes, with deduplication and aliases |
| **Fact/Item** | Immutable provenance-bearing evidence extracted from an episode; it may yield zero or more claims |
| **Claim** | Atomic typed proposition derived deterministically from a fact and eligible for comparison only when its claim slot is complete |
| **ClaimRelation** | Versioned reconciliation decision between two claims: duplicate, supersession, correction, contradiction, or temporal ambiguity |
| **Bi-temporal** | Separating real-world validity (`valid_from`/`valid_to`) from transaction validity (`t_ingested`/`t_invalid_ingested`) for correct historical queries and audit |
| **Community/Cluster** | Cluster of densely connected entities with aggregated summary for faster context assembly |
| **Scope** | Isolation level: `personal`, `team`, `org`, or `private-domain` (e.g., `hr.salary`, `deal.pipeline`) |
| **Provenance** | Complete lineage from episode to fact to claim and versioned reconciliation/lifecycle decisions |

### 3.2 Data Model Conventions

For consistency, all schemas/APIs/skills MUST use these field names:

- `entity_links[]` — list of canonical entity IDs (equivalent to `actors_involved`)
- `source_episode` — pointer to the episode ID
- `source_position` — position within the episode (char offset, line number, or timeframe)
- `content` — normalized fact statement
- `quote` — verbatim quote from source
- `t_valid` — legacy fact-level reference time; it MUST NOT be assumed to be a claim's `valid_from`
- `t_invalid` — fact-level retraction time; it MUST NOT represent routine claim supersession
- `valid_from`, `valid_to` — claim real-world validity bounds, each independently optional
- `t_ingested`, `t_invalid_ingested` — transaction-valid interval for a derived claim or relation

---

## 4. Users and Access Control

### 4.1 User Roles

| Role | Access | Description |
|------|--------|-------------|
| **Owner (personal)** | Full access to personal scope | Individual user's private memory |
| **Org Admin** | Manage org scope, policies, connectors, retention | Controls organizational memory |
| **Team Member** | Access to assigned team scopes (projects/deals) | Collaborative project memory |
| **HR/Finance** | Access to private-domain scopes (e.g., `hr.salary`, `finance.budget`) | Restricted sensitive data |
| **Agent (service role)** | Access via policy-bound tokens/scopes | Automated processes with limited permissions |

### 4.2 Functional Requirements: Access Control

**FR-AC-01**: System MUST support context levels: `personal` / `team` / `org` / `private-domain` (e.g., `hr.salary`, `deal.pipeline`, `personal.health`).  
**Status**: ✅ Done

**FR-AC-02**: Each memory object MUST have `visibility_scope` and `policy_tags`.  
**Status**: ✅ Done

**FR-AC-03**: Retrieval queries MUST filter by policies **before** execution (no post-filtering of LLM responses).  
**Status**: ✅ Done

**FR-AC-04**: Agent access MUST use authentication (JWT/external auth server) and scope-bound tokens (audience/claims) when using SurrealDB Cloud/SurrealMCP.  
**Status**: ✅ Done

**FR-AC-05**: Rate limits MUST be implemented at MCP/gateway layer (RPS/burst) to prevent abuse and unauthorized extraction.  
**Status**: ✅ Done

**FR-AC-06**: System MUST separate `personal` and `corporate` contexts in different namespaces within the same database.  
**Status**: ✅ Done

**FR-AC-07**: Cross-scope references MUST be resolved only through policy rules (explicit allow/deny) with mandatory logging.  
**Status**: ✅ Done

**FR-AC-08**: Cross-scope retrieval MUST pre-check policies and scope-claims.  
**Status**: ✅ Done

**FR-AC-09**: System MUST maintain immutable execution/event log for all MCP operations (who/what/when/args/result) with replay capability for debugging and audit.  
**Status**: ✅ Done

**FR-AC-10**: Authentication/authorization for HTTP/RPC MUST comply with FR-AC requirements (JWT, scope/claims, ns/db headers for HTTP).  
**Status**: ✅ Done

---

## 5. Functional Requirements

### 5.1 Integrations and Ingestion

**FR-IN-01**: System MUST support connectors for: email, chat (Telegram/Slack), calendar, tasks (Todo/Notion/Jira), files (PDF/Docs), calls (audio + transcript).  
**Status**: ✅ Done

**FR-IN-02**: When new document/event arrives, ingestion pipeline MUST trigger automatically (near-real-time) and re-index changes on schedule.  
**Status**: ✅ Done

**FR-IN-03**: For each incoming object, the system MUST save the raw episode (preserving text and metadata) and link it back to the original source via URI, ID, or audio timeframe.  
**Status**: ✅ Done

**FR-IN-04**: For each episode, MUST record `t_ref` (reference time of event) and `t_ingested` (when added to system) for bi-temporal logic.  
**Status**: ✅ Done

**FR-IN-05**: Ingestion MUST use a deterministic `episode_id` based on `source_type`, `source_id`, `t_ref`, and `scope`.  
**Status**: ✅ Done

**FR-IN-06**: Normalization rules for sources and identifiers MUST be documented and applied before computing deterministic IDs (trim/unicode normalization, timezone normalization, email/case canonicalization) to avoid collisions and ensure stability across repeated ingestion runs.  
**Status**: ✅ Done

### 5.2 SurrealDB Transports and Protocols

**FR-IN-07**: System MUST support SurrealDB transports: **RPC** (preferred for production, typed RPC + CBOR), **HTTP** (stateless endpoints: `/sql`, import/export), **CBOR** (binary encoding with SurrealDB custom tags) for efficient and type-safe data exchange.  
**Status**: ✅ Done

**FR-IN-08**: All RPC and HTTP interactions MUST be logged in the execution/event log with actor, action, timestamp, arguments, result, transport type, and content type (`application/cbor` or `application/json`).  
**Status**: ✅ Done

**FR-IN-09**: Use of session variables (`vars`) in RPC MUST be explicit and included in the operation log; session-dependent behavior must remain controllable and reproducible.  
**Status**: ✅ Done

**FR-IN-10**: CBOR serialization MUST use SurrealDB's standard CBOR tags for dates, IDs, decimals, UUIDs, and geometry values to ensure correct round-tripping and deterministic behavior.  
**Status**: ✅ Done

### 5.3 SurrealDB Storage Backend (Single Source of Truth)

**FR-DB-01**: System MUST use **SurrealDB as the only storage backend**; for tests, only in-memory mode of SurrealDB is allowed (no separate in-memory storage in MCP).  
**Status**: ✅ Done

**FR-DB-02**: All memory objects (Episode/Entity/Fact/Claim/ClaimRelation/Edge/Community) MUST be saved and read from SurrealDB, including graph and reconciliation relationships.

**Status**: ⚠️ Partial — current objects are persisted, but Claim and ClaimRelation storage is not implemented.

**FR-DB-03**: System MUST support SurrealDB schemas/migrations as code (DDL/versions) and reproducible deployment.  
**Status**: ✅ Done

**FR-DB-04**: Namespace/database MUST be mandatory at service startup; values set via environment configuration.  
**Status**: ✅ Done

**FR-DB-05**: System MUST provide indexes in SurrealDB for retrieval: full-text, graph traversal.
**Status**: ✅ Done — full-text indexes on fact content and index_keys with `memory_fts` analyzer, edge endpoint indexes on `in`/`out`, entity canonical-name and alias indexes. Embedding/HNSW indexes intentionally removed per `SIMPLIFIED_SEARCH_REDESIGN_SPEC.md`.

**FR-DB-06**: Execution/event log MUST be stored in SurrealDB (append-only) or synchronized there for audit.  
**Status**: ✅ Done

**FR-DB-07**: Released or applied migration files MUST be immutable. Schema and data evolution MUST use new, monotonically ordered migrations; past migrations MUST NOT be edited, deleted, reordered, or repurposed.

**Status**: ⚠️ Partial — applied scripts are recorded with checksums and checksum drift fails validation, but an explicit historical-migration compatibility gate is not documented in the current test matrix.

**FR-DB-08**: On startup, the application MUST automatically upgrade every configured namespace from each explicitly supported older database version before serving requests. Migrations MUST be deterministic, restart-safe, data-preserving, and compatible with legacy records missing newly introduced optional fields. Migration failure MUST stop startup before the application serves against a partially upgraded schema.

**Status**: ⚠️ Partial — startup applies pending embedded migrations per namespace, but sequential upgrade coverage from a declared set of historical database versions is not established.

### 5.4 Entity and Fact Extraction

**FR-EX-01**: System MUST extract entities: `Person`, `Company`, `Project`, `Deal`, `Product`, `Asset`, `Location` (extensible).
**Status**: ✅ Implemented (2026-03-26) — Unicode-aware regex extractor using `[\p{Lu}][\p{Ll}]+` pattern for Cyrillic/Latin support. Classifies multi-word names as `person`, CamelCase single tokens as `technology`, and recognizes `company`/`event`/`location` via suffix indicators and gazetteer.

**FR-EX-02**: System MUST extract facts/items: `Promise`, `Task`, `Metric`, `Decision`, `Opinion`/`Preference`, `Relationship` (extensible).
**Status**: ⚠️ Partial — current extraction covers `metric`, `promise`, `experience`, and a conservative `note` fallback for summary-like episodes. Dedicated `task`, `decision`, and `relationship` extraction remain future work.

**FR-EX-03**: Each fact MUST contain: `content` (normalized statement), `quote` (verbatim quote), `source_pointer` (to episode and position), `actors_involved`, `t_valid` (when stated/true).
**Status**: ✅ Done — facts persist `content`, `quote`, `source_episode`, `entity_links`, `t_valid`, `t_invalid`, `confidence`, `fact_type`, `index_keys`, `access_count`, `last_accessed`. The `entity_links` field serves as the actor linkage mechanism. `source_position` is not consistently populated (follow-up item).

**FR-EX-04**: To improve extraction quality, the system SHOULD use a two-step flow—initial extraction followed by self-validation—to reduce hallucinations and omissions.  
**Status**: ❌ Not done — current extraction is single-pass and heuristic.

**FR-EX-05**: A fact MUST remain the immutable provenance-bearing evidence item and MAY produce zero or more deterministic claims. Failure or lack of support for claim extraction MUST NOT make the fact unavailable.

**Status**: ❌ Not done — no separate claim model or projection lifecycle exists.

**FR-EX-06**: Default claim extraction MUST run locally with zero configuration and no LLM or external service. It MUST emit only claims accepted by a versioned built-in schema and typed-value validator.

**Status**: ❌ Not done — current rule-based fact and triple extraction does not produce validated claim schemas.

**FR-EX-07**: Claim and claim-relation identifiers MUST be deterministic from versioned canonical inputs. Their semantic payloads MUST be immutable; only open validity bounds may be closed monotonically.

**Status**: ❌ Not done.

**FR-EX-08**: Historical claim projection MUST run outside startup migrations as a local, bounded, durable, idempotent, and resumable backfill with per-namespace progress and an extractor fingerprint. Legacy facts MUST remain retrievable while backfill is incomplete.

**Status**: ❌ Not done — the existing `reembed` job provides a reusable operational pattern, but claim backfill does not exist.

**FR-EX-09**: Automatic reconciliation candidate lookup MUST use indexed stable pagination within an exact claim slot: namespace, scope, project identity, access-policy fingerprint, canonical subject, compatible schema, comparison key, and qualifiers. It MUST NOT use a global scan, fuzzy entity overlap, or a fixed latest-N window.

**Status**: ❌ Not done — the current warning detector scans at most 500 active facts and filters them by fact type and entity overlap.

### 5.5 Entity Resolution (Deduplication)

**FR-ER-01**: System MUST support aliases and entity merging (for example, "Mitya/Dima/Dmitry Ivanov").  
**Status**: ⚠️ Partial — aliases can be stored, but merge workflows are not implemented.

**FR-ER-02**: System MUST provide hybrid deduplication: (a) embedding similarity + (b) text features + (c) LLM verification based on episode context.
**Status**: ⚠️ Partial — alias-based exact matching with normalization is implemented. Embedding fields and provider scaffolding were removed per `SIMPLIFIED_SEARCH_REDESIGN_SPEC.md`. LLM-assisted verification remains pending (requires LLM integration path).

**FR-ER-03**: System MUST preserve merge history (merge log): who/what/when/why merged, with rollback capability (split).  
**Status**: ❌ Not done — merge history / split support are not implemented.

**FR-ER-04**: After merge, all facts/links MUST reference canonical entity, preserving provenance.  
**Status**: ⚠️ Partial — canonical IDs are used at creation time, but post-merge rewriting is not implemented because merge workflows are absent.

**FR-ER-05**: Alias resolution MUST be deterministic (exact match → canonical entity, then stable tie-break rules).
**Status**: ✅ Done — aliases normalized via `normalize_text()` at write time; `select_entity_lookup()` and `select_entities_batch()` use `CONTAINSANY` against normalized aliases.

### 5.6 Relationship Graph (Context Graph)

**FR-GR-01**: System MUST store graph: Nodes (Entities, Episodes, Facts, Communities) and Edges (`mentions`, `promised_by`, `assigned_to`, `related_to`, `same_as`, `derived_from`, etc.).  
**Status**: ✅ Done

**FR-GR-02**: Each edge/fact MUST have temporal attributes and provenance (source) to ensure explainability ("why did the agent decide this").
**Status**: ✅ Done — temporal fields use native SurrealDB `datetime` / `option<datetime>` types with write-time coercion. Provenance is persisted for both facts and edges. The `explain()` function traces provenance interactively back to source episodes.

**FR-GR-03**: System MUST support "communities/clusters" of entities and store their summaries for faster retrieval and organizational context overview.  
**Status**: ✅ Done

**FR-GR-04**: Each edge MUST contain metadata: `strength`, `confidence`, `provenance`, `t_valid`, `t_invalid`, and optional `weight`/`temporal_weight` for ranking.  
**Status**: ✅ Done

**FR-GR-05**: Edges MUST support bi-temporal attributes and invalidation: when adding a new conflicting edge, old edges should be marked `t_invalid` (see Edge Invalidation rules in FR-TM).
**Status**: ✅ Done — conflicting active versions of the same logical triple (same from_id/relation/to_id) are invalidated before insert via `invalidate_conflicting_edges()`.

### 5.7 Temporality: Decay and Invalidation

**FR-TM-01**: System MUST support decay (confidence degradation over time) by default with configurable half-life per fact type (e.g., one year for metrics/promises).  
**Status**: ✅ Done

**FR-TM-02**: System MUST support explicit validity closure when a claim is confirmed to supersede an earlier claim. Supersession MUST close only the earlier claim's validity interval; a contradiction alone MUST NOT invalidate either source fact.

**Status**: ⚠️ Partial — explicit manual fact invalidation exists, but automatic claim-level supersession and its separate lifecycle are not implemented, and current triple conflict resolution does not enforce this distinction.

**FR-TM-03**: System MUST implement bi-temporal model: store validity time of fact (T) and transaction/ingest time (T′) for audit, retroactive corrections, and correct "as-of" answers.  
**Status**: ✅ Done

**FR-TM-04**: Retrieval MUST support "as-of" queries (snapshot at date): show context as it was at meeting/email time.  
**Status**: ✅ Done

**FR-TM-05**: When a new claim differs from an existing claim, the system MUST classify the relationship as duplicate, supersession, correction, contradiction, or temporal ambiguity and preserve the decision with provenance. Only confirmed supersession may close real-world validity, and only explicit correction may close an erroneous transaction-valid projection.

**Status**: ❌ Not done — current extraction only returns non-persistent potential-contradiction warnings based on fact type, entity overlap, and different content.

**FR-TM-06**: Claim-level temporal evidence MUST distinguish observation time from the interval in which the claim is true. Missing validity information remains explicitly unknown and MUST NOT trigger automatic supersession.

**Status**: ❌ Not done — extracted facts currently inherit the episode reference time as `t_valid` without representing whether that time was observed, explicit, inferred from a source contract, or unknown.

**FR-TM-07**: Every comparison key MUST have a cardinality policy. Unknown keys default to set-valued, and automatic supersession is permitted only for an explicitly single-valued key when subject, qualifiers, and temporal evidence establish the same logical slot.

**Status**: ❌ Not done — the current triple resolver uses a global hard-coded singleton predicate list that includes naturally multi-valued relations such as employment, email, phone, and founder roles.

**FR-TM-08**: Automatic supersession MUST additionally require source continuity within the same source lineage or an explicitly authoritative source for the applicable claim schema and domain scope. In zero-configuration mode no source is authoritative by default, and ingestion order alone MUST NOT grant replacement authority.

**Status**: ❌ Not done — current conflict handling does not model source lineage or authority and may close triples solely because a later extraction has a different object.

**FR-TM-09**: Claim supersession, targeted claim correction, and fact retraction MUST be distinct lifecycle operations. Supersession closes only the earlier claim's real-world validity interval. Correction closes the erroneous claim projection in transaction time for the same validity context. Retraction is reserved for erroneous, withdrawn, corrupted, or incorrectly ingested whole-source evidence. All operations preserve source evidence for audit.

**Status**: ❌ Not done — the storage model has no separate claim entity, and the current `invalidate` operation acts on the whole fact.

**FR-TM-10**: Correction MUST require explicit correction or withdrawal evidence plus source continuity or scoped authority. A different value, newer observation, higher confidence, or later ingestion alone MUST NOT authorize correction.

**Status**: ❌ Not done.

### 5.8 Context Assembly

**FR-CA-01**: System MUST assemble context dynamically for task/question: return top-K facts/nodes with quotes and source links.  
**Status**: ✅ Done

**FR-CA-02**: System MUST support hybrid retrieval: vector (semantic), full-text, and graph traversal (BFS/limited hops) for "social" queries and connection chains.
**Status**: ⚠️ Superseded — per `SIMPLIFIED_SEARCH_REDESIGN_SPEC.md`, embedding retrieval was intentionally removed from runtime. Current retrieval uses lexical/BM25 full-text as primary, community-summary expansion, and DB-side graph traversal.

**FR-CA-03**: System MUST enforce token budgeting: limits on fact count, quote length, detail levels (brief/standard/deep).  
**Status**: ✅ Done

**FR-CA-04**: Assembly result MUST include: (a) facts, (b) confidence score, (c) rationale (why included), (d) provenance.  
**Status**: ✅ Done

**FR-CA-05**: Retrieval results MUST be deterministically ordered (stable sort + tie-break by time and ID).  
**Status**: ✅ Done

**FR-CA-06**: System MUST support definition and management of analyzers and indexes for full-text search; this includes ability to specify tokenizers, filters, and analyzer functions for domain texts.
**Status**: ✅ Done — `memory_fts` analyzer with punctuation-aware tokenization, case-insensitive matching, ASCII normalization, and English snowball stemming configured for fact content and index_keys.

**FR-CA-07**: To reduce query variability, agents MUST be provided with canonical query templates and typed memory operations (e.g., `Q_ACTOR_BY_ALIAS`, `Q_PROMISES`, `add_fact`, `invalidate_fact`, `get_briefing`). These operations should validate input using JSON Schema.  
**Status**: ✅ Done

**FR-CA-08**: `assemble_context` MUST support multi-word queries where query terms appear non-adjacently in fact content. Implementation uses SurrealDB `@@` full-text search operator (primary) with per-word `CONTAINS` fallback. Query preprocessing strips `episode:xxx` references, boolean operators, quoted phrases, and tokens < 2 characters.
**Status**: ✅ Done — `select_facts_filtered()` uses DB-side `content @1@ $query` with `search::score(1) AS ft_score`; query preprocessing in `preprocess_search_query()` strips noise.

**FR-CA-09**: `assemble_context` MUST support optional timeline retrieval mode via `view_mode` parameter. When `view_mode=timeline`, results are sorted chronologically by `t_valid` (oldest first) instead of relevance ranking. Optional `window_start` and `window_end` parameters filter facts to a time window.
**Status**: ✅ Done — implemented 2026-03-27 as part of adaptive memory alignment. Timeline sorting and window filtering applied after fusion ranking, before budget truncation. Backwards-compatible: default `view_mode=None` preserves standard relevance ordering.

**FR-CA-10**: FTS retrieval MUST match facts via both `content` and `index_keys` fields. `index_keys` populated at ingest with canonical entity names, aliases, and temporal markers (month-year, ISO date components) extracted from fact content.
**Status**: ✅ Done — implemented 2026-03-27. SurrealDB FTS index `fact_index_keys_search` on `index_keys` with `memory_fts` analyzer. Query searches `content @1@ $query OR index_keys @1@ $query` with merged scores.

**FR-CA-11**: `assemble_context` SHOULD auto-resolve timeline ordering for explicit temporal-history queries when callers leave `view_mode` empty. Explicit `view_mode` remains authoritative. Named entity anchors SHOULD expand into bounded graph context (1 hop for entity-centric queries, 2 hops for path/introduction queries) without making semantic retrieval mandatory.
**Status**: ✅ Done — implemented via deterministic query flags and bounded entity-anchor expansion in `src/service/context/query_mode.rs` and `src/service/context/graph.rs`.

**FR-CA-12**: When query logging is enabled, the system MUST persist `resolved_view_mode`, `query_flags`, and retrieval-tier distribution alongside existing latency/result-count analytics.
**Status**: ✅ Done — stored in `query_log` via migration `021_query_log_retrieval_diagnostics.surql`.

### 5.9 Agent Scenarios (Skills/Flows)

**FR-AG-01**: System MUST expose six canonical memory operations: `ingest`, `extract`, `resolve`, `invalidate`, `assemble_context`, and `explain`.  
**Status**: ✅ Done

**FR-AG-02**: Canonical memory operations MUST be accessible via MCP interface (stdio/http/socket) so IDEs/assistants can call them uniformly.  
**Status**: ✅ Done

**FR-AG-03**: Entity resolution and fact invalidation MUST remain explainable and auditable: all merges and invalidations are logged, and callers can request citations and explanations via `explain`.
**Status**: ✅ Done — invalidations are logged and `explain()` performs full provenance tracing back to source episodes with multi-source lineage. Entity-merge history tracking remains a follow-up item (merge workflows not yet implemented).

**FR-AG-04**: System MUST support agent types: personal, team (2 owners), collective (group visibility) at minimum via scope/ACL.  
**Status**: ✅ Done

### 5.10 UI/UX (Minimum for "Context Graph")

**FR-UX-01**: The UI MUST let users select a contact, partner, or project and get answers to questions such as:
- "Who promised what to whom? Is it fulfilled?"
- "What metrics/deals were mentioned and how did they change?"
- "What tasks for me/team, priority, deadline?"  
**Status**: ✅ Done

**FR-UX-02**: Each answer MUST include a quote and a link to the primary source (episode, document, or timecode).  
**Status**: ✅ Done

**FR-UX-03**: UI MUST allow launching next flow ("find intro to OpenAI → generate email draft") from context screen.
**Status**: ✅ Done

### 5.11 Adaptive Memory Features (Heat-Aware Lifecycle)

**FR-AM-01**: System MUST track fact access heat via `access_count` and `last_accessed` fields updated on every retrieval and explain operation.
**Status**: ✅ Done — implemented 2026-03-27. `access_count` incremented by 1 on retrieval, by 3 on explain (stronger signal). SurrealDB atomic updates: `UPDATE fact SET access_count += $boost, last_accessed = time::now()`.

**FR-AM-02**: Lifecycle decay worker MUST skip recently-accessed ("hot") facts even if age-based decay would otherwise invalidate them.
**Status**: ✅ Done — decay pass checks `is_hot = access_count > 0 && (now - last_accessed).num_days() <= half_life_days`. Hot facts protected from invalidation.

**FR-AM-03**: Lifecycle archival worker MUST skip episodes with recently-accessed facts.
**Status**: ✅ Done — archival queries filter episodes with `last_accessed >= hot_cutoff` to preserve active memory.

**FR-AM-04**: System MUST support LongMemEval-style acceptance tests covering multi-session reasoning, temporal reasoning, knowledge update, and abstention.
**Status**: ✅ Done — `tests/longmem_acceptance.rs` covers 5 benchmark categories.

---

## 6. Data Model

### 6.1 Core Objects

| Object | Required Fields | Acceptance Criteria |
|--------|----------------|---------------------|
| **Episode** | `id`, `source_type`, `source_id`, `content`, `t_ref`, `t_ingested` | For any fact, can open source episode and see exact quote/fragment. |
| **Entity** | `id`, `type`, `canonical_name`, `aliases[]` | Search by any alias returns canonical entity. `embedding` and `merge_history[]` remain target-state fields, not current implementation facts. |
| **Fact/Item** | `id`, `type`, `content`, `quote`, `entity_links[]`, `t_valid`, `t_invalid?`, `confidence`, `source_episode`, `index_keys[]`, `access_count`, `last_accessed?` | Every fact retains source evidence and provenance. Only explicit retraction excludes the fact from active truth selection; claim supersession leaves it unchanged. `index_keys` populated at ingest with entity names, aliases, and temporal markers for enriched BM25 retrieval. `access_count` and `last_accessed` updated on retrieval and explain for heat-aware lifecycle. |
| **Claim** | `id`, `source_fact`, `schema`, `subject`, `comparison_key`, `value`, `qualifiers`, `cardinality`, `observed_at?`, `valid_from?`, `valid_to?`, `derivation`, `t_ingested`, `t_invalid_ingested?` | Atomic propositions are created only when deterministic extraction can populate a supported schema. Unsupported facts remain valid without claims. Real-world and transaction validity are separate from the source fact lifecycle. Target state; not implemented. |
| **ClaimRelation** | `id`, `left_claim`, `right_claim`, `predecessor_claim?`, `successor_claim?`, `outcome`, `reason_code`, `evidence`, `evaluator_version`, `context_fingerprint`, `evaluated_at`, `supersedes_relation?`, `t_ingested`, `t_invalid_ingested?` | Reconciliation decisions are append-only and versioned. Direction is explicit for supersession and correction; symmetric outcomes use only the canonical pair. The active relation classifies the pair as duplicate, supersession, correction, contradiction, or temporal ambiguity; prior versions remain auditable. Target state; not implemented. |
| **Edge** | `id`, `from_entity`, `to_entity`, `relation_type`, `strength`, `confidence`, `provenance`, `t_valid`, `t_invalid?` | Relationships are stored, but conflict invalidation and provenance fidelity are still incomplete. |
| **Community** | `id`, `member_entities[]`, `summary`, `updated_at` | Communities are maintained as connected components over persisted graph links and can expand retrieval through summary matches. |

### 6.2 Deterministic ID Rules

All IDs MUST be deterministic to ensure idempotence:

- **Episode ID**: `hash(source_type + source_id + t_ref + scope)`
- **Entity ID**: `hash(canonical_name + type + scope)` after normalization
- **Fact ID**: `hash(content + source_episode + source_position + scope)`
- **Edge ID**: `hash(from_entity + to_entity + relation_type + t_valid + scope)`

**Normalization rules** (FR-IN-06):
- Trim whitespace
- Unicode normalization (NFC)
- Timezone normalization (all timestamps → UTC)
- Email/case canonicalization (lowercase, domain normalization)

### 6.3 Scope and Namespace Mapping

- **Scope** → **SurrealDB Namespace** mapping:
  - `personal` → `user_<user_id>`
  - `team` → `team_<team_id>`
  - `org` → `org_<org_id>`
  - `private-domain` (e.g., `hr.salary`) → `private_<domain>`

- All objects within a scope stored in corresponding namespace
- Cross-scope queries require explicit policy allow-list

---

## 7. MCP Tool Surface

### 7.1 Core Tools (Canonical)

| Tool | Description | Input | Output |
|------|-------------|-------|--------|
| `ingest` | Store raw episode | `source_type`, `source_id`, `content`, `t_ref`, `scope` | `ToolResponse<String>` with `episode_id` in `result` |
| `extract` | Extract entities, facts, and links from an episode or inline content | `episode_id` or non-empty `content`/`text` | `ToolResponse<ExtractResult>` |
| `resolve` | Deduplicate/resolve canonical entities | `entity_type`, `canonical_name`, `aliases[]` | `ToolResponse<String>` with canonical `entity_id` |
| `invalidate` | Retract an erroneous or withdrawn source fact while preserving audit history | `fact_id`, `reason`, `t_invalid` | `ToolResponse<String>` |
| `assemble_context` | Build recency-first context pack for query | `query`, `scope`, `as_of?`, `budget` | `ToolResponse<Vec<AssembledContextItem>>` |
| `explain` | Return citation-shaped context items | `context_items` | `ToolResponse<Vec<ExplainItem>>` |

### 7.2 Contract Design Notes

- Public MCP surface is intentionally limited to the six canonical memory tools above.
- Claim projection, reconciliation, supersession, and correction remain internal domain behavior; no new public MCP tool is added without a concrete user workflow.
- Legacy UI/draft/helper tools are not part of the current public contract.
- `extract` returns a graceful partial response with an empty typed result when neither `episode_id` nor content is supplied.
- List-style responses use decision-ready envelope fields such as `status`, `guidance`, `has_more`, `total_count`, and `next_offset`.

### 7.5 Tool Call Logging and Observability

All tool calls MUST log:
- Tool name
- Input parameters (sanitized for secrets)
- Start time / End time
- Result status (success/error)
- Error details (if any)

Logging levels:
- **Info**: Tool start, tool done
- **Warn**: Tool error, validation failure
- **Error**: System errors (DB unavailable, etc.)

### 7.6 Intent-Designed Contracts (Minimal Tool Surface)

**Principle**: Tools accept high-level intent; MCP server encapsulates complexity.

**Example**: `extract` tool
- Accepts `episode_id` OR `content`/`text`
- If both missing, returns soft response `{status: "no_input", message: "..."}` (not MCP error)
- Input is normalized (trim, unicode, empty string → null)
- Reduces model confusion and repair loops

---

## 8. Non-Functional Requirements

### 8.1 Performance

**NFR-P-01 (Latency)**: p95 context assembly latency SHOULD be ≤100–300ms for typical queries, assuming pre-built indexes (vector/text/graph); "raw episode search" may be slower.  
**Status**: ✅ Done

**NFR-P-02 (Scalability)**: System MUST support scaling to "10 humans + 10,000 agents" via scope isolation, caching, rate limiting, and limited traversal depth.  
**Status**: ✅ Done

### 8.2 Reliability

**NFR-R-01 (Reliability)**: Ingestion and extraction MUST be idempotent (re-run does not create duplicates).  
**Status**: ⚠️ Partial — fact IDs are deterministic, but claim projection, relation IDs, and resumable backfill are not implemented.

### 8.3 Security

**NFR-S-01 (Security)**: MUST enforce strict data segregation and token/authentication management at MCP level; MCP transport should support local and network modes (stdio/http/unix socket) depending on deployment model.  
**Status**: ⚠️ Partial — stdio/local-first operation is documented and embedded mode now uses `Capabilities::default()`, but remote RBAC/capability lockdown remains a follow-up item.

### 8.4 Auditability

**NFR-A-01 (Auditability)**: MUST store complete provenance: "which episode generated which fact", plus invalidation/update history (bi-temporal).  
**Status**: ⚠️ Partial — episode-to-fact provenance exists, but claim derivation and versioned reconciliation/correction history do not.

### 8.5 Determinism

**NFR-D-01 (Determinism)**: All MCP responses MUST be deterministic (no randomness, stable ordering).  
**Status**: ✅ Done

**NFR-D-02 (Determinism)**: Object identifiers MUST be deterministic and collision-resistant; conflicts resolved predictably.  
**Status**: ✅ Done

**NFR-D-03 (Determinism)**: Any operation depending on RPC session state MUST include all relevant session vars in query parameters and execution log to ensure deterministic result on replay.  
**Status**: ✅ Done

### 8.6 Maintainability

**NFR-M-01 (Maintainability)**: All schemas, policies, and pipelines MUST be managed as code (Git) with migrations and versioning.  
**Status**: ⚠️ Partial — existing schema migrations are versioned, but ClaimSchema, canonicalization, alias, cardinality, and reconciliation policy versions are not implemented.

### 8.7 Observability

**NFR-AO-01 (Observability)**: System MUST provide structured logging with levels (trace/debug/info/warn/error); human-readable text format with keys and brief values (arrays → `[a,b]`, objects → `{k=v,..}`).  
**Status**: ✅ Done

**NFR-AO-02 (Claim Reconciliation Observability)**: Claim extraction, key matching, candidate selection, and reconciliation MUST emit trace-level structured events containing correlation IDs, claim/fact IDs, claim schema, full comparison key, match mode, candidate count, outcome, reason code, and stage duration. Prometheus MUST expose counters and histograms aggregated only by bounded-cardinality dimensions such as claim schema, stage, match mode, outcome, and reason code. Raw comparison keys, entity IDs, claim IDs, and fact IDs MUST NOT be used as Prometheus labels.

**Status**: ❌ Not done — structured event logging exists, but the current text formatter truncates individual values to 200 characters; full-key claim traces, reconciliation instrumentation, and a Prometheus exporter are not implemented.

### 8.8 Error Handling

**NFR-E-01 (Error Handling)**: Error messages MUST be standardized for repair-loop scenarios; soft-fallbacks preferred over hard errors (e.g., `extract` without input returns `status=no_input` instead of MCP error).  
**Status**: ✅ Done

---

## 9. Implementation

### 9.1 Technology Stack

| Layer | Technology | Status |
|-------|-----------|--------|
| **MCP Server** | rmcp (Rust) + FastMCP pattern | ✅ Done |
| **Storage** | SurrealDB (RocksDB for local, TiKV for distributed) | ✅ Done |
| **Transport** | stdio (primary), HTTP/RPC (future) | ✅ Done |
| **Language** | Rust (memory_mcp crate) | ✅ Done |

### 9.2 Rust Implementation Plan (from rusty_memory_mcp/SPEC.md)

#### 9.2.1 Context and Goals

- **Goal**: Rewrite `memory_mcp` from Python (FastMCP) to Rust (rmcp) for performance, determinism, and safety.
- **Requirement**: stdio-only transport, local MCP usage.
- **Requirement**: SurrealDB embedded directly in MCP server.
- **Requirement**: Tool consolidation permitted (intent-based routing).

**Status**: ✅ Done

#### 9.2.2 Scope of Work

- [x] Rewrite MCP server on rmcp (Rust), preserving functional parity of tool surface
- [x] Rewrite domain logic `MemoryService` in Rust
- [x] Implement SurrealDB client and storage/search/update operations
- [x] Support migrations from `migrations/*.surql`
- [x] Update configuration and `mcp.json` examples for stdio-only
- [x] Preserve/migrate test harness (acceptance/e2e/unit)

**Status**: ✅ Done

#### 9.2.3 Architecture

**Module structure:**

```
memory_mcp/
├── src/
│   ├── main.rs           # Entry point, MCP init
│   ├── mcp/              # MCP tool handlers
│   ├── service/          # Business logic (MemoryService)
│   ├── storage/          # SurrealDB client, queries
│   ├── models/           # Data models (Episode, Entity, Fact, etc.)
│   ├── config/           # Configuration parsing
│   ├── errors/           # Error types
│   └── logging/          # StdoutLogger
├── migrations/           # SurrealQL migrations
├── tests/                # Integration tests
└── Cargo.toml
```

**Responsibility boundaries:**

- `tool → service → storage → SurrealDB`
- Singleton DB client, lazy initialization

**Status**: ✅ Done

#### 9.2.4 MCP Transport and Protocol

- [x] stdio-only transport (rmcp `transport::io::stdio`)
- [x] JSON input/output schemas for each tool
- [x] Error format: validation, business rules, access, configuration

**Status**: ✅ Done

#### 9.2.5 Tool Surface and Consolidation

- [x] Canonical tools: `ingest`, `extract`, `resolve`, `invalidate`, `assemble_context`, `explain`
- [x] Minimal public tool surface enforced (memory-only)
- [x] Consolidation policy: canonical six-tool surface
- [x] Soft-fallbacks for intent-based calls (normalize empty strings, soft-fallbacks for `extract` with no input)
- [x] Tool call logging (start/done/error) with Info/Warn levels

**Status**: ✅ Done

#### 9.2.6 Data Model (SurrealDB)

- [x] Sync tables and fields with current schema (episode, entity, fact, edge, community, task, event_log)
- [x] Deterministic ID rules (episode/entity/fact/edge/community)
- [x] Scope/namespace rules and `scope → namespace` mapping
- [ ] Add Claim, ClaimRelation, comparison-key alias, and durable claim-job records through a new migration
- [ ] Add deterministic bi-temporal claim and relation IDs and indexed claim-slot queries

**Status**: ⚠️ Partial

#### 9.2.7 Migrations

- [x] Strategy: apply `.surql` migrations on startup
- [x] Idempotent error handling: ignore benign errors (already exists/defined/index exists)
- [x] Expectations: `script_migration` schema or canonical initial migration (`__Initial.surql`)
- [x] Integration test: apply migrations to embedded SurrealDB, verify indexes/tables
- [x] Versioned multi-file migrations with checksum verification
- [x] Apply pending embedded migrations to every configured namespace before serving
- [ ] Treat every released migration as immutable and add only new monotonically ordered migrations
- [ ] Test sequential automatic upgrades from every explicitly supported historical database version

**Status**: ⚠️ Partial — startup application and checksum validation exist; immutable-history and historical-upgrade coverage remain pending.

#### 9.2.8 Configuration and Environment

- [x] Required env vars: `SURREALDB_DB_NAME`, `SURREALDB_URL`, `SURREALDB_NAMESPACES`, `SURREALDB_USERNAME`, `SURREALDB_PASSWORD`
- [x] Optional: `RUST_LOG` (standard Rust logging variable)
- [x] Fail-fast behavior on missing/invalid config
- [x] Documentation: recommend `cargo install --locked memory_mcp`, provide examples for installed and built binaries

**Status**: ✅ Done

#### 9.2.9 Security

- [x] No raw-query tool, no external side-effects
- [x] Use parameterized SurrealDB queries for the highest-risk request paths first
- [ ] Define minimal roles/permissions in SurrealDB (RBAC)
- [x] Prefer a deny-by-default embedded capability profile over `Capabilities::all()`

**Status**: Partially done (highest-risk query paths tightened, but remote RBAC and stricter capability allow-lists are still pending; see `docs/security-hardening-roadmap.md`)

#### 9.2.10 Observability and Errors

- [x] Log format: human-readable text with levels (trace/debug/info/warn/error); keys and brief values (arrays → `[a,b]`, objects → `{k=v,..}`)
- [ ] Optional JSON format (via `LOG_FORMAT=json|text`)
- [ ] Metrics/counters (rate-limit, latency, error types)
- [x] Standardized error messages for repair-loop (soft-fallback `status=no_input` for `extract`)

**Status**: Partially done (JSON format, metrics pending)

#### 9.2.11 Performance and Reliability

- [x] Rate limiting policy (equivalent to current)
- [x] Caching for `assemble_context` and invalidation rules
- [ ] Retry/backoff strategy for transient DB errors

**Status**: Partially done (retry/backoff pending)

#### 9.2.12 Testing

- [x] E2E tests for MCP tools
- [x] Acceptance scenarios
- [x] Unit tests for service layer and infrastructure (including `StdoutLogger`)
- [x] Test fixtures/embedded in-memory SurrealDB (`kv-mem`)
- [x] Code formatted (`cargo fmt`) and checked (`cargo clippy`)
- [ ] Labeled claim reconciliation corpus with adversarial negatives
- [ ] Historical database upgrade, resumable backfill, concurrency, and MCP compatibility tests

**Status**: ⚠️ Partial

#### 9.2.13 Compatibility and Contracts

- [x] Preserve input payload compatibility for existing tools
- [x] Preserve/describe alias-tool behavior or remove with compatible routing
- [x] Update `mcp.json` examples for stdio-only
- [x] Soften contracts for intent-based calls (normalize empty strings, soft-fallbacks)
- [ ] Preserve required response fields while adding optional claim/reconciliation metadata
- [ ] Prove automatic upgrade and legacy fact retrieval against supported historical database snapshots

**Status**: ⚠️ Partial

#### 9.2.14 Deployment and Local Operation

- [x] Describe Rust binary build (cargo build/release)
- [x] Describe MCP server startup via stdio
- [x] Describe environment configuration for local run

**Status**: ✅ Done

#### 9.2.15 Risks and Assumptions

- [ ] SurrealDB license consideration (BSL, not DBaaS)
- [ ] Risk of behavior mismatch with current Python version
- [ ] Risk of tool incompatibility without alias layer
- [ ] Risk of unavailable external materials (incomplete articles/blocks)

**Status**: Not addressed

#### 9.2.16 References and Sources

- [ ] rmcp documentation (Context7)
- [ ] SurrealDB Rust SDK (Context7 + docs)
- [ ] Articles: cra.mr (skills/tools/subagents/context)
- [ ] Reference repo: `like-a-freedom/rusty-intervals-mcp`

**Status**: Not documented

### 9.3 Implementation Summary

**Completed:**
- Rust MCP server with rmcp + SurrealDB backend
- Canonical memory-only MCP surface (`ingest`, `extract`, `resolve`, `invalidate`, `assemble_context`, `explain`)
- Migrations with embedded/filesystem fallback
- Logging with StdoutLogger (human-readable text format)
- Tests (unit/integration/e2e)
- Clippy/fmt clean
- Persistence test (RocksDB)
- Soft-fallbacks for `extract` (partial typed response, no hard error on missing input)
- Tool call logging (observability)
- Typed service outputs for extract/context/explain (`ExtractResult`, `AssembledContextItem`, `ExplainItem`)

- Bi-temporal edge filtering (DB-side pushdown) implemented; storage API exposes filtered edge selection used by graph traversals.
- Graph traversal updated: `find_intro_chain` accepts optional `as_of` and uses filtered edges; neighbor ordering made deterministic.
- In-memory/Mock DB clients and acceptance tests updated to mirror bi-temporal semantics; new acceptance test added for as-of traversal behavior.
- Full test suite (unit + integration + acceptance) passes locally; `cargo clippy` completed with no warnings.

- Query preprocessing for `assemble_context` exists (`preprocess_search_query()`), with test coverage for multi-word search behavior and query normalization.
- SurrealDB 3 migration hardening completed: `store_edge` no longer writes null optional invalidation fields, runtime schema now defines `edge_id` and all required SCHEMAFULL tables (`community`, `event_log`, `task`), and legacy SurrealQL syntax updated (`SEARCH ANALYZER` → `FULLTEXT ANALYZER`, `string::is::datetime` → `string::is_datetime`).

Every memory tool is reachable both via stdio MCP and via a CLI subcommand, sharing the same implementation in `src/tools/`. The CLI entry point (`main.rs`) parses arguments and dispatches directly to the same service methods — there is no separate code path for CLI vs. MCP.

**Pending:**
- Richer extraction quality (multi-pass, LLM-assisted validation)
- Entity merge workflows with history tracking and rollback capability
- Source_position population for fact actor linkage
- Remote RBAC/capability lockdown for production deployment
- JSON log format option and metrics/counters
- Retry/backoff for transient DB errors
- Deployment hardening guidance for multi-user remote setups
- PPR-class associative retrieval (research-track, deferred)
- Session summary generation (requires LLM integration path)

---

## 10. Testing and Acceptance

### 10.1 API/Service Methods (Logical)

| API | Description | Status |
|-----|-------------|--------|
| **API-01** | `ingest(episode) → episode_id` | ✅ Done |
| **API-02** | `extract(episode_id) → {entities, facts, links}` | ✅ Done |
| **API-03** | `resolve(entity_candidate) → canonical_entity_id (+ merge actions)` | ✅ Done |
| **API-04** | `invalidate(fact_id, reason, t_invalid) → ok` (whole-fact retraction; input shape preserved) | ✅ Done |
| **API-05** | `assemble_context(query, scope, as_of, budget) → context_pack` | ✅ Done |
| **API-06** | `explain(context_pack) → episode links/quotes` | ✅ Done — returns citation-shaped items with full provenance tracing back to source episodes, including multi-source lineage via entity links |

**Note:** SurrealMCP and SurrealDB transports MUST support production settings, including authentication (JWT/auth server), rate limits (RPS/burst), and multiple transport modes (stdio, HTTP, socket, RPC). API-01 through API-06 MUST be accessible over RPC/HTTP and, where appropriate, accept and return CBOR-encoded payloads. All calls must be logged in the execution/event log together with transport and content-type information.

### 10.2 Acceptance Tests (High-Level)

**AT-01**: After adding an email with the promise "will do by Friday," the system shows that promise on the relevant contact record together with a quote and a link to the email.  
**Status**: ✅ Done

**AT-02**: If a later source states "ARR grew to $3M", the old source fact remains auditable. The earlier ARR claim is superseded only when single-valued cardinality, explicit validity, and source-lineage or authority gates are satisfied; otherwise the system records contradiction or temporal ambiguity.

**Status**: ❌ Not done

**AT-03**: User without `hr.salary` scope cannot extract/see salary facts via UI or agent skill.  
**Status**: ✅ Done

**AT-04**: The query "who can introduce me to OpenAI" returns a relationship chain found through graph traversal (2-3 hops) and backed by source evidence.  
**Status**: ✅ Done

**AT-05**: CBOR round-trip verification: datetime/record id/decimal preserved without loss with RPC+CBOR.  
**Status**: ✅ Done

**AT-06**: Query via RPC with explicitly specified `vars` is logged; repeated call with same `vars` produces deterministic result.  
**Status**: ✅ Done

**AT-07**: As-of graph traversal returns chains consistent with bi-temporal visibility: a chain present at a recent `as_of` may be absent at a past `as_of` if edges/facts were not yet ingested or valid.  
**Status**: ✅ Done

**AT-08**: Multi-word queries in `assemble_context` (e.g., "Delta Enrollment", "release notes Module v2.2 episode:xxx") return matching facts even when query words appear non-adjacently in content. Query preprocessing correctly strips episode references and boolean operators.  
**Status**: ✅ Done

**AT-09**: Two incompatible claims with overlapping validity are both preserved and returned with a persisted contradiction relation and source evidence; neither source fact is invalidated.

**Status**: ❌ Not done

**AT-10**: A fact containing several claims remains retrievable when one claim is superseded or corrected; unrelated claims and the original quote are unchanged.

**Status**: ❌ Not done

**AT-11**: An explicitly corrected claim closes in transaction time without inventing a real-world transition, while audit can reconstruct the pre-correction view.

**Status**: ❌ Not done

**AT-12**: Upgrading a supported historical database automatically applies only new append-only migrations, serves legacy facts without claims, and resumes claim backfill after interruption without duplicates.

**Status**: ❌ Not done

**AT-13**: Claims in different scope, project, or access-policy partitions never reconcile and never leak relation metadata across authorization boundaries.

**Status**: ❌ Not done

**AT-14**: `extract`, `assemble_context`, and `explain` preserve existing required response fields while adding reconciliation information only through optional backward-compatible fields.

**Status**: ❌ Not done

### 10.3 Test Coverage

- **Unit tests**: Service layer, storage layer, logging, error handling, canonicalization golden fixtures, deterministic IDs, and the complete reconciliation decision table
- **Property tests**: Normalization idempotence, qualifier-order invariance, canonical pair symmetry, and isolation invariants
- **Integration tests**: Fresh SurrealDB migrations, sequential upgrades from supported historical snapshots, indexes, persistence, concurrency, and resumable backfill
- **E2E tests**: Real MCP handler responses against embedded SurrealDB, including backward-compatible optional reconciliation metadata
- **Evaluation tests**: Labeled positive and negative claim/relation corpus with per-schema precision, recall, confusion matrices, and latency percentiles
- **Acceptance tests**: High-level scenarios (AT-01..AT-14)

- **Full-run status**: Full test suite (unit + integration + acceptance + embedded FTS + MCP e2e) executed locally after SurrealDB 3 migration fixes; all tests passed and linter (`cargo clippy`) reported no warnings.

---

## 11. Configuration and Deployment

### 11.1 Environment Variables

| Variable | Required | Description | Default |
|----------|----------|-------------|---------|
| `SURREALDB_URL` | ✅ | SurrealDB connection URL (e.g., `rocksdb://./data/surreal.db` or `ws://localhost:8000`) | — |
| `SURREALDB_DB_NAME` | ✅ | Database name | — |
| `SURREALDB_NAMESPACES` | ✅ | Comma-separated namespaces (e.g., `user_123,team_456,org_789`) | — |
| `SURREALDB_USERNAME` | ✅ | Username for authentication | — |
| `SURREALDB_PASSWORD` | ✅ | Password for authentication | — |
| `SURREALDB_EMBEDDED` | ❌ | Force embedded RocksDB mode if `true`; if unset it is inferred from `SURREALDB_URL` | `false` |
| `SURREALDB_DATA_DIR` | ❌ | Optional embedded RocksDB data directory (`./data/surrealdb` by default) | `./data/surrealdb` |
| `RUST_LOG` | ❌ | Log level: `trace`, `debug`, `info`, `warn`, `error` | `info` |

### 11.2 Installation

**Recommended:**

```bash
cargo install --locked memory_mcp
```

**From source:**

```bash
cd /path/to/memory_mcp
cargo build --release
```

### 11.3 Running MCP Server

**Installed:**

```bash
memory_mcp
```

**Built from source:**

```bash
./target/release/memory_mcp
```

**With environment:**

```bash
SURREALDB_URL=rocksdb://./data/surreal.db \
SURREALDB_DB_NAME=memory \
SURREALDB_NAMESPACES=user_solovey \
SURREALDB_USERNAME=root \
SURREALDB_PASSWORD=root \
RUST_LOG=info \
memory_mcp
```

### 11.4 MCP Configuration

**`.vscode/mcp.json` (stdio)**:

```json
{
  "mcpServers": {
    "memory-mcp": {
      "command": "cargo",
      "args": ["run", "--quiet", "--bin", "memory_mcp"],
      "cwd": "/path/to/memory_mcp",
      "env": {
        "SURREALDB_URL": "rocksdb://./data/surreal.db",
        "SURREALDB_DB_NAME": "memory",
        "SURREALDB_NAMESPACES": "user_solovey",
        "SURREALDB_USERNAME": "root",
        "SURREALDB_PASSWORD": "root",
        "RUST_LOG": "info"
      }
    }
  }
}
```

For an installed binary, replace the command block with `"command": "memory_mcp"` and omit `cwd`.

### 11.5 Migrations

**Migration sources:**

1. Canonical initial schema embedded in the Rust binary
2. Ordered versioned migrations embedded in the Rust binary

**Migration behavior:**

- Pending embedded migrations are applied automatically to every configured namespace on server startup, before requests are served
- Applied migrations are tracked by deterministic record ID and checksum
- Released or applied migration files are immutable; changes require a new monotonically ordered migration
- Migrations must be deterministic, restart-safe, and preserve legacy records and provenance
- The current application must automatically upgrade every explicitly supported older database version
- Migration failure stops startup before requests are served against a partially upgraded schema
- Canonical initial migration: `migrations/__Initial.surql`

---

## 12. References

### 12.1 Architecture and Design

- [Subagents with MCP](https://cra.mr/subagents-with-mcp)
- [MCP, Skills, and Agents](https://cra.mr/mcp-skills-and-agents)
- `docs/CONTRADICTION_DETECTION_DESIGN.md` — deterministic claim-reconciliation target architecture
- `docs/SIMPLIFIED_SEARCH_REDESIGN_SPEC.md` — retrieval target-state specification
- `docs/superpowers/specs/2026-03-27-sota-memory-alignment-design.md` — adaptive-memory target-state specification
- `docs/superpowers/specs/2026-07-28-truthful-evaluation-system-design.md` — evaluation architecture and design
- [Memory Agent](/.github/agents/memory.agent.md) — Full 1100+ line agent specification
- [PDM Agent](/.github/agents/pdm.agent.md) — Product Manager Agent

### 12.2 Implementation

- [rmcp documentation](https://context7.io/rmcp) (Rust MCP SDK)
- [SurrealDB Rust SDK](https://docs.surrealdb.com/docs/sdk/rust)
- [SurrealDB](https://surrealdb.com/)

### 12.3 Standards

- ISO/IEC/IEEE 29148 (Systems and software engineering — Life cycle processes — Requirements engineering)

### 12.4 Deprecated Documents

These documents are superseded by this specification:

- `.agent/docs/SPEC.md` (deprecated)
- `.agent/rusty_memory_mcp/SPEC.md` (deprecated)

**Note:** `memory-agent-architecture.md` remains as a high-level overview and quick reference.

---

## Document Status

**Current Version**: 2.4<br>
**Consolidated**: February 5, 2026; claim reconciliation revision July 17, 2026<br>
**Next Review**: When significant requirements change or implementation milestones reached

**Changelog:**

- **2026-07-28**: Added reference to the truthful evaluation system design spec (`docs/superpowers/specs/2026-07-28-truthful-evaluation-system-design.md`). The evaluation harness is a private workspace crate (`eval-harness`) that provides profile-driven evaluation, typed artifacts, and truthful metrics. It is never linked into the production binary.
- **2026-07-17**: Added the accepted Claim/ClaimRelation reconciliation design and corrected previous fact-level supersession, migration, observability, compatibility, and acceptance-test claims that no longer represented the target model.

- **2026-03-27**: Linked this runtime spec to the new adaptive-memory target-state design doc, clarifying that SOTA alignment work is tracked separately from shipped behavior and should preserve the simplified lexical/BM25 + graph retrieval direction.

- **2026-03-25**: Reconciled the specification with the validated review findings. Downgraded overstated statuses around temporal typing, FTS pushdown, provenance persistence, explainability, edge invalidation, embeddings, migration versioning, and community retrieval; added an explicit implementation reality-check section.

- **2026-03-11**: Removed legacy non-memory service APIs, aligned documentation to the six-tool memory-only MCP surface, and revalidated with `cargo fmt --all`, `cargo test`, and strict `cargo clippy --all-targets -- -D warnings`.

- **2026-02-19**: Completed SurrealDB 2.x → 3.x migration validation; fixed `edge` persistence/schema regressions, updated deprecated SurrealQL syntax, and confirmed clean `fmt`/`clippy`/full test run.
- **2026-02-06**: Added FR-CA-08 (multi-word FTS search), AT-08, updated implementation summary with FTS details
- **2026-02-05**: Consolidated three specifications into single source of truth; removed duplications; added implementation plan with statuses
- **2026-02-05**: Consolidated three specifications into single source of truth; removed duplications; added implementation plan with statuses
- **2026-02-05**: Implemented bi-temporal edge filtering for graph traversals; updated traversal API and tests; validated full test suite and clippy clean.
- **2026-01-22**: Memory Agent architecture finalized
- Previous versions: see deprecated files

---

**END OF DOCUMENT**
