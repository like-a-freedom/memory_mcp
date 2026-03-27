# Memory System — Unified Specification

**Version:** 2.3  
**Date:** February 5, 2026  
**Status:** Consolidated (supersedes all previous SPEC.md versions)

---

## Document Change History

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
| **Fact/Item** | Promise, task, metric, decision, opinion extracted from episodes and linked to entities |
| **Bi-temporal** | Storing both "when was it true" (validity time, `t_valid`) and "when did we learn" (transaction time, `t_ingested`) for correct historical queries and audit |
| **Community/Cluster** | Cluster of densely connected entities with aggregated summary for faster context assembly |
| **Scope** | Isolation level: `personal`, `team`, `org`, or `private-domain` (e.g., `hr.salary`, `deal.pipeline`) |
| **Provenance** | Complete lineage: which episode generated which fact, plus invalidation/update history |

### 3.2 Data Model Conventions

For consistency, all schemas/APIs/skills MUST use these field names:

- `entity_links[]` — list of canonical entity IDs (equivalent to `actors_involved`)
- `source_episode` — pointer to the episode ID
- `source_position` — position within the episode (char offset, line number, or timeframe)
- `content` — normalized fact statement
- `quote` — verbatim quote from source
- `t_valid` — validity time (when the fact became true)
- `t_invalid` — invalidation time (when the fact became false/superseded, if applicable)
- `t_ingested` — transaction time (when the system recorded this object)

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

**FR-DB-02**: All memory objects (Episode/Entity/Fact/Edge/Community) MUST be saved and read from SurrealDB, including graph relationships.  
**Status**: ✅ Done

**FR-DB-03**: System MUST support SurrealDB schemas/migrations as code (DDL/versions) and reproducible deployment.  
**Status**: ✅ Done

**FR-DB-04**: Namespace/database MUST be mandatory at service startup; values set via environment configuration.  
**Status**: ✅ Done

**FR-DB-05**: System MUST provide indexes in SurrealDB for retrieval: full-text, graph traversal, and (if available) vector indexes.  
**Status**: ⚠️ Partial — full-text, edge traversal, and vector-index scaffolding exist, but embedding-backed ranking is not enabled by default.

**FR-DB-06**: Execution/event log MUST be stored in SurrealDB (append-only) or synchronized there for audit.  
**Status**: ✅ Done

### 5.4 Entity and Fact Extraction

**FR-EX-01**: System MUST extract entities: `Person`, `Company`, `Project`, `Deal`, `Product`, `Asset`, `Location` (extensible).
**Status**: ✅ Implemented (2026-03-26) — Unicode-aware regex extractor using `[\p{Lu}][\p{Ll}]+` pattern for Cyrillic/Latin support. Classifies multi-word names as `person`, CamelCase single tokens as `technology`, and recognizes `company`/`event`/`location` via suffix indicators and gazetteer.

**FR-EX-02**: System MUST extract facts/items: `Promise`, `Task`, `Metric`, `Decision`, `Opinion`/`Preference`, `Relationship` (extensible).
**Status**: ⚠️ Partial — current extraction covers only simple `promise` and `metric` heuristics.

**FR-EX-03**: Each fact MUST contain: `content` (normalized statement), `quote` (verbatim quote), `source_pointer` (to episode and position), `actors_involved`, `t_valid` (when stated/true).  
**Status**: ⚠️ Partial — facts persist `content`, `quote`, `source_episode`, and `t_valid`, but `source_position` / actor linkage are not consistently populated.

**FR-EX-04**: To improve extraction quality, the system SHOULD use a two-step flow—initial extraction followed by self-validation—to reduce hallucinations and omissions.  
**Status**: ❌ Not done — current extraction is single-pass and heuristic.

### 5.5 Entity Resolution (Deduplication)

**FR-ER-01**: System MUST support aliases and entity merging (for example, "Mitya/Dima/Dmitry Ivanov").  
**Status**: ⚠️ Partial — aliases can be stored, but merge workflows are not implemented.

**FR-ER-02**: System MUST provide hybrid deduplication: (a) embedding similarity + (b) text features + (c) LLM verification based on episode context.  
**Status**: ⚠️ Partial — embedding fields and provider scaffolding now exist, but a real embedder and LLM-assisted verification pipeline are still pending.

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
**Status**: ⚠️ Partial — temporal fields exist and provenance is persisted, but temporal columns still use string-backed schema definitions and `explain()` does not yet trace provenance interactively.

**FR-GR-03**: System MUST support "communities/clusters" of entities and store their summaries for faster retrieval and organizational context overview.  
**Status**: ✅ Done

**FR-GR-04**: Each edge MUST contain metadata: `strength`, `confidence`, `provenance`, `t_valid`, `t_invalid`, and optional `weight`/`temporal_weight` for ranking.  
**Status**: ✅ Done

**FR-GR-05**: Edges MUST support bi-temporal attributes and invalidation: when adding a new conflicting edge, old edges should be marked `t_invalid` (see Edge Invalidation rules in FR-TM).  
**Status**: ⚠️ Partial — conflicting active versions of the same logical triple are invalidated, but broader semantic contradiction handling remains future work.

### 5.7 Temporality: Decay and Invalidation

**FR-TM-01**: System MUST support decay (confidence degradation over time) by default with configurable half-life per fact type (e.g., one year for metrics/promises).  
**Status**: ✅ Done

**FR-TM-02**: System MUST support explicit fact invalidation (supersession) when a new contradictory fact appears, rather than relying only on gradual confidence decay.  
**Status**: ✅ Done

**FR-TM-03**: System MUST implement bi-temporal model: store validity time of fact (T) and transaction/ingest time (T′) for audit, retroactive corrections, and correct "as-of" answers.  
**Status**: ✅ Done

**FR-TM-04**: Retrieval MUST support "as-of" queries (snapshot at date): show context as it was at meeting/email time.  
**Status**: ✅ Done

**FR-TM-05**: When new fact/metric conflicts with existing ones, system MUST perform contradiction check (LLM-assisted or rule-based) and upon confirmation set `t_invalid` on old facts (explicit invalidation), preserving provenance.  
**Status**: ⚠️ Partial — explicit fact invalidation exists and repeated edge writes invalidate prior active versions, but higher-level contradiction detection is still heuristic/manual.

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

**FR-CA-06**: System MUST support definition and management of analyzers and indexes for full-text search and vector indexes; this includes ability to specify tokenizers, filters, and analyzer functions for domain texts.  
**Status**: ⚠️ Partial — analyzer support exists for fact and community FTS, and vector index definitions are configurable by dimension, but production embedding retrieval is still disabled by default.

**FR-CA-07**: To reduce query variability, agents MUST be provided with canonical query templates and typed memory operations (e.g., `Q_ACTOR_BY_ALIAS`, `Q_PROMISES`, `add_fact`, `invalidate_fact`, `get_briefing`). These operations should validate input using JSON Schema.  
**Status**: ✅ Done

**FR-CA-08**: `assemble_context` MUST support multi-word queries where query terms appear non-adjacently in fact content. Implementation uses SurrealDB `@@` full-text search operator (primary) with per-word `CONTAINS` fallback. Query preprocessing strips `episode:xxx` references, boolean operators, quoted phrases, and tokens < 2 characters.
**Status**: ✅ Done — `select_facts_filtered()` uses DB-side `content @1@ $query` with `search::score(1) AS ft_score`; query preprocessing in `preprocess_search_query()` strips noise.

**FR-CA-09**: `assemble_context` MUST support optional timeline retrieval mode via `view_mode` parameter. When `view_mode=timeline`, results are sorted chronologically by `t_valid` (oldest first) instead of relevance ranking. Optional `window_start` and `window_end` parameters filter facts to a time window.
**Status**: ✅ Done — implemented 2026-03-27 as part of adaptive memory alignment. Timeline sorting and window filtering applied after fusion ranking, before budget truncation. Backwards-compatible: default `view_mode=None` preserves standard relevance ordering.

**FR-CA-10**: FTS retrieval MUST match facts via both `content` and `index_keys` fields. `index_keys` populated at ingest with canonical entity names, aliases, and temporal markers (month-year, ISO date components) extracted from fact content.
**Status**: ✅ Done — implemented 2026-03-27. SurrealDB FTS index `fact_index_keys_search` on `index_keys` with `memory_fts` analyzer. Query searches `content @1@ $query OR index_keys @1@ $query` with merged scores.

### 5.9 Agent Scenarios (Skills/Flows)

**FR-AG-01**: System MUST expose six canonical memory operations: `ingest`, `extract`, `resolve`, `invalidate`, `assemble_context`, and `explain`.  
**Status**: ✅ Done

**FR-AG-02**: Canonical memory operations MUST be accessible via MCP interface (stdio/http/socket) so IDEs/assistants can call them uniformly.  
**Status**: ✅ Done

**FR-AG-03**: Entity resolution and fact invalidation MUST remain explainable and auditable: all merges and invalidations are logged, and callers can request citations and explanations via `explain`.  
**Status**: ⚠️ Partial — invalidations are logged and `explain` performs provenance tracing back to source episodes, but explicit entity-merge history is still missing.

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
| **Fact/Item** | `id`, `type`, `content`, `quote`, `entity_links[]`, `t_valid`, `t_invalid?`, `confidence`, `source_episode`, `index_keys[]`, `access_count`, `last_accessed?` | Every fact has quote and valid temporal attributes; correctly disappears/degrades when stale/invalidated. `index_keys` populated at ingest with entity names, aliases, and temporal markers for enriched BM25 retrieval. `access_count` and `last_accessed` updated on retrieval and explain for heat-aware lifecycle. |
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
| `invalidate` | Mark fact as superseded | `fact_id`, `reason`, `t_invalid` | `ToolResponse<String>` |
| `assemble_context` | Build recency-first context pack for query | `query`, `scope`, `as_of?`, `budget` | `ToolResponse<Vec<AssembledContextItem>>` |
| `explain` | Return citation-shaped context items | `context_items` | `ToolResponse<Vec<ExplainItem>>` |

### 7.2 Contract Design Notes

- Public MCP surface is intentionally limited to the six canonical memory tools above.
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
**Status**: ✅ Done

### 8.3 Security

**NFR-S-01 (Security)**: MUST enforce strict data segregation and token/authentication management at MCP level; MCP transport should support local and network modes (stdio/http/unix socket) depending on deployment model.  
**Status**: ⚠️ Partial — stdio/local-first operation is documented and embedded mode now uses `Capabilities::default()`, but remote RBAC/capability lockdown remains a follow-up item.

### 8.4 Auditability

**NFR-A-01 (Auditability)**: MUST store complete provenance: "which episode generated which fact", plus invalidation/update history (bi-temporal).  
**Status**: ✅ Done

### 8.5 Determinism

**NFR-D-01 (Determinism)**: All MCP responses MUST be deterministic (no randomness, stable ordering).  
**Status**: ✅ Done

**NFR-D-02 (Determinism)**: Object identifiers MUST be deterministic and collision-resistant; conflicts resolved predictably.  
**Status**: ✅ Done

**NFR-D-03 (Determinism)**: Any operation depending on RPC session state MUST include all relevant session vars in query parameters and execution log to ensure deterministic result on replay.  
**Status**: ✅ Done

### 8.6 Maintainability

**NFR-M-01 (Maintainability)**: All schemas, policies, and pipelines MUST be managed as code (Git) with migrations and versioning.  
**Status**: ✅ Done

### 8.7 Observability

**NFR-AO-01 (Observability)**: System MUST provide structured logging with levels (trace/debug/info/warn/error); human-readable text format with keys and brief values (arrays → `[a,b]`, objects → `{k=v,..}`).  
**Status**: ✅ Done

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

**Status**: ✅ Done

#### 9.2.7 Migrations

- [x] Strategy: apply `.surql` migrations on startup
- [x] Source selection: filesystem (`repo_root/migrations`) → embedded → none
- [x] Idempotent error handling: ignore benign errors (already exists/defined/index exists)
- [x] Expectations: `script_migration` schema or canonical initial migration (`__Initial.surql`)
- [x] Integration test: apply migrations to embedded SurrealDB, verify indexes/tables
- [x] Versioned multi-file migrations with checksum verification

**Status**: ✅ Done

#### 9.2.8 Configuration and Environment

- [x] Required env vars: `SURREALDB_DB_NAME`, `SURREALDB_URL`, `SURREALDB_NAMESPACES`, `SURREALDB_USERNAME`, `SURREALDB_PASSWORD`
- [x] Optional: `LOG_LEVEL` (unified variable, `RUST_LOG` removed from docs)
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

**Status**: ✅ Done

#### 9.2.13 Compatibility and Contracts

- [x] Preserve input payload compatibility for existing tools
- [x] Preserve/describe alias-tool behavior or remove with compatible routing
- [x] Update `mcp.json` examples for stdio-only
- [x] Soften contracts for intent-based calls (normalize empty strings, soft-fallbacks)

**Status**: ✅ Done

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

**Pending:**
- Native datetime fields for temporal columns
- DB-side FTS + bi-temporal pushdown in a single query path
- Real `explain()` provenance expansion
- Embeddings and hybrid semantic retrieval
- Lifecycle consolidation / decay background jobs
- JSON log format option
- Parameterized queries (security hardening)
- RBAC setup (SurrealDB roles/permissions)
- Retry/backoff for transient errors
- Deployment hardening guidance
- Risk assessment documentation

---

## 10. Testing and Acceptance

### 10.1 API/Service Methods (Logical)

| API | Description | Status |
|-----|-------------|--------|
| **API-01** | `ingest(episode) → episode_id` | ✅ Done |
| **API-02** | `extract(episode_id) → {entities, facts, links}` | ✅ Done |
| **API-03** | `resolve(entity_candidate) → canonical_entity_id (+ merge actions)` | ✅ Done |
| **API-04** | `invalidate(fact_id, reason, t_invalid) → ok` | ✅ Done |
| **API-05** | `assemble_context(query, scope, as_of, budget) → context_pack` | ✅ Done |
| **API-06** | `explain(context_pack) → episode links/quotes` | ⚠️ Partial — currently returns citation-shaped items without episode lookup or provenance tracing |

**Note:** SurrealMCP and SurrealDB transports MUST support production settings, including authentication (JWT/auth server), rate limits (RPS/burst), and multiple transport modes (stdio, HTTP, socket, RPC). API-01 through API-06 MUST be accessible over RPC/HTTP and, where appropriate, accept and return CBOR-encoded payloads. All calls must be logged in the execution/event log together with transport and content-type information.

### 10.2 Acceptance Tests (High-Level)

**AT-01**: After adding an email with the promise "will do by Friday," the system shows that promise on the relevant contact record together with a quote and a link to the email.  
**Status**: ✅ Done

**AT-02**: If 6 months later new email states "ARR grew to $3M", old fact "$1M ARR" becomes invalidated (or confidence drops sharply), UI shows metric dynamics.  
**Status**: ✅ Done

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

### 10.3 Test Coverage

- **Unit tests**: Service layer, storage layer, logging, error handling
- **Integration tests**: SurrealDB migrations, indexes, persistence
- **E2E tests**: MCP tool calls with real SurrealDB (embedded)
- **Acceptance tests**: High-level scenarios (AT-01..AT-08)

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
| `LOG_LEVEL` | ❌ | Log level: `trace`, `debug`, `info`, `warn`, `error` | `warn` |

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
LOG_LEVEL=info \
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
        "LOG_LEVEL": "info"
      }
    }
  }
}
```

For an installed binary, replace the command block with `"command": "memory_mcp"` and omit `cwd`.

### 11.5 Migrations

**Migration sources (priority order):**

1. Filesystem: `<repo_root>/migrations/*.surql`
2. Embedded: Rust binary includes migrations at compile time
3. None: Skip migrations (for testing only)

**Migration behavior:**

- Applied on server startup
- Idempotent: benign errors ignored (already exists/defined)
- Canonical initial migration: `migrations/__Initial.surql`

---

## 12. References

### 12.1 Architecture and Design

- [Subagents with MCP](https://cra.mr/subagents-with-mcp)
- [MCP, Skills, and Agents](https://cra.mr/mcp-skills-and-agents)
- `docs/SIMPLIFIED_SEARCH_REDESIGN_SPEC.md` — retrieval target-state specification
- `docs/superpowers/specs/2026-03-27-sota-memory-alignment-design.md` — adaptive-memory target-state specification
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

**Current Version**: 2.3  
**Consolidated**: February 5, 2026  
**Next Review**: When significant requirements change or implementation milestones reached

**Changelog:**

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
