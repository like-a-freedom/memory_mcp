# Memory System — Unified Specification

**Version:** 2.0  
**Date:** February 5, 2026  
**Status:** Consolidated (supersedes all previous SPEC.md versions)

---

## Document Change History

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

Memory System provides **agents and humans** with a unified memory/context layer that:
- Aggregates multi-channel sources (email, chat, calendar, tasks, calls, files)
- Transforms into a **bi-temporal knowledge graph** (facts + relationships)
- Delivers to LLMs on-demand with minimal token budget and low latency
- Supports personal, team, and organizational scopes with strict access control

### 1.2 Key Design Principles

1. **Separation of Concerns**: Specialized Memory Agent handles all memory operations; Product Manager Agent delegates via `runSubagent`
2. **Bi-temporal Modeling**: Track both "when was it true" (validity time) and "when did we learn" (transaction time) for correct historical queries and audit
3. **Determinism**: All operations produce stable, reproducible results (no randomness, stable sort orders)
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
- Skills like `context-assembly`, `entity-tracking`, `stakeholder-analysis`, `decision-tracking`, `ingest-episode` are **embedded** in Memory Agent prompt (not standalone skills)
- PDM delegates via `runSubagent(agentName: "memory", ...)` instead of calling MCP tools directly

---

## 2. System Architecture

### 2.1 Component Responsibilities

| Component | Responsibilities |
|-----------|------------------|
| **PDM Agent** | Strategy, roadmapping, stakeholder management, requirements engineering. Delegates memory operations. |
| **Memory Agent** | Episode ingestion, entity extraction, fact extraction, context assembly, stakeholder analysis, decision tracking. Encapsulates memory skills. |
| **Memory MCP Server** | Exposes MCP tools (`ingest`, `extract`, `resolve`, `assemble_context`, etc.), manages SurrealDB lifecycle, migrations, rate limiting. |
| **SurrealDB** | Stores all memory objects (Episode/Entity/Fact/Edge/Community), executes graph traversals, provides full-text/vector search. |

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

**FR-IN-03**: For each incoming object, MUST save "raw episode" (preserving text/metadata) and link to original (URI/ID/audio timeframe).  
**Status**: ✅ Done

**FR-IN-04**: For each episode, MUST record `t_ref` (reference time of event) and `t_ingested` (when added to system) for bi-temporal logic.  
**Status**: ✅ Done

**FR-IN-05**: Idempotent ingest MUST use deterministic `episode_id` based on `source_type`, `source_id`, `t_ref`, and `scope`.  
**Status**: ✅ Done

**FR-IN-06**: Normalization rules for sources and identifiers MUST be documented and applied before computing deterministic IDs (trim/unicode normalization, timezone normalization, email/case canonicalization) to avoid collisions and ensure stability across re-ingests.  
**Status**: ✅ Done

### 5.2 SurrealDB Transports and Protocols

**FR-IN-07**: System MUST support SurrealDB transports: **RPC** (preferred for production, typed RPC + CBOR), **HTTP** (stateless endpoints: `/sql`, import/export), **CBOR** (binary encoding with SurrealDB custom tags) for efficient and type-safe data exchange.  
**Status**: ✅ Done

**FR-IN-08**: All RPC/HTTP interactions MUST be logged in execution/event log (who/what/when/args/result) with transport type and content-type (application/cbor or application/json).  
**Status**: ✅ Done

**FR-IN-09**: Use of session variables (`vars`) in RPC MUST be explicit and included in operation log; session-dependent behavior must be controllable and reproducible.  
**Status**: ✅ Done

**FR-IN-10**: Serialization using CBOR MUST use agreed SurrealDB CBOR tags for dates/IDs/decimal/uuid/geometry to ensure correct round-trip and determinism.  
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
**Status**: ✅ Done

**FR-DB-06**: Execution/event log MUST be stored in SurrealDB (append-only) or synchronized there for audit.  
**Status**: ✅ Done

### 5.4 Entity and Fact Extraction

**FR-EX-01**: System MUST extract entities: `Person`, `Company`, `Project`, `Deal`, `Product`, `Asset`, `Location` (extensible).  
**Status**: ✅ Done

**FR-EX-02**: System MUST extract facts/items: `Promise`, `Task`, `Metric`, `Decision`, `Opinion`/`Preference`, `Relationship` (extensible).  
**Status**: ✅ Done

**FR-EX-03**: Each fact MUST contain: `content` (normalized statement), `quote` (verbatim quote), `source_pointer` (to episode and position), `actors_involved`, `t_valid` (when stated/true).  
**Status**: ✅ Done

**FR-EX-04**: To improve extraction quality, SHOULD apply "two-pass" scheme (extract → self-check/reflection) to reduce hallucinations and omissions.  
**Status**: ✅ Done

### 5.5 Entity Resolution (Deduplication)

**FR-ER-01**: System MUST support aliases and entity merging (e.g., "Митя/Дима/Dmitry Ivanov").  
**Status**: ✅ Done

**FR-ER-02**: System MUST provide hybrid deduplication: (a) embedding similarity + (b) text features + (c) LLM verification based on episode context.  
**Status**: ✅ Done

**FR-ER-03**: System MUST preserve merge history (merge log): who/what/when/why merged, with rollback capability (split).  
**Status**: ✅ Done

**FR-ER-04**: After merge, all facts/links MUST reference canonical entity, preserving provenance.  
**Status**: ✅ Done

**FR-ER-05**: Alias resolution MUST be deterministic (exact match → canonical entity, then stable tie-break rules).  
**Status**: ✅ Done

### 5.6 Relationship Graph (Context Graph)

**FR-GR-01**: System MUST store graph: Nodes (Entities, Episodes, Facts, Communities) and Edges (`mentions`, `promised_by`, `assigned_to`, `related_to`, `same_as`, `derived_from`, etc.).  
**Status**: ✅ Done

**FR-GR-02**: Each edge/fact MUST have temporal attributes and provenance (source) to ensure explainability ("why did the agent decide this").  
**Status**: ✅ Done

**FR-GR-03**: System MUST support "communities/clusters" of entities and store their summaries for faster retrieval and organizational context overview.  
**Status**: ✅ Done

**FR-GR-04**: Each edge MUST contain metadata: `strength`, `confidence`, `provenance`, `t_valid`, `t_invalid`, and optional `weight`/`temporal_weight` for ranking.  
**Status**: ✅ Done

**FR-GR-05**: Edges MUST support bi-temporal attributes and invalidation: when adding a new conflicting edge, old edges should be marked `t_invalid` (see Edge Invalidation rules in FR-TM).  
**Status**: ✅ Done

### 5.7 Temporality: Decay and Invalidation

**FR-TM-01**: System MUST support decay (confidence degradation over time) by default with configurable half-life per fact type (e.g., one year for metrics/promises).  
**Status**: ✅ Done

**FR-TM-02**: System MUST support fact invalidation (supersede) when new contradictory fact appears, not just "graceful forgetting".  
**Status**: ✅ Done

**FR-TM-03**: System MUST implement bi-temporal model: store validity time of fact (T) and transaction/ingest time (T′) for audit, retroactive corrections, and correct "as-of" answers.  
**Status**: ✅ Done

**FR-TM-04**: Retrieval MUST support "as-of" queries (snapshot at date): show context as it was at meeting/email time.  
**Status**: ✅ Done

**FR-TM-05**: When new fact/metric conflicts with existing ones, system MUST perform contradiction check (LLM-assisted or rule-based) and upon confirmation set `t_invalid` on old facts (explicit invalidation), preserving provenance.  
**Status**: ✅ Done

### 5.8 Context Assembly

**FR-CA-01**: System MUST assemble context dynamically for task/question: return top-K facts/nodes with quotes and source links.  
**Status**: ✅ Done

**FR-CA-02**: System MUST support hybrid retrieval: vector (semantic), full-text, and graph traversal (BFS/limited hops) for "social" queries and connection chains.  
**Status**: ✅ Done

**FR-CA-03**: System MUST enforce token budgeting: limits on fact count, quote length, detail levels (brief/standard/deep).  
**Status**: ✅ Done

**FR-CA-04**: Assembly result MUST include: (a) facts, (b) confidence score, (c) rationale (why included), (d) provenance.  
**Status**: ✅ Done

**FR-CA-05**: Retrieval results MUST be deterministically ordered (stable sort + tie-break by time and ID).  
**Status**: ✅ Done

**FR-CA-06**: System MUST support definition and management of analyzers and indexes for full-text search and vector indexes; this includes ability to specify tokenizers, filters, and analyzer functions for domain texts.  
**Status**: ✅ Done

**FR-CA-07**: To reduce query variability, agents MUST be provided canonical query templates and typed memory-skills (e.g., `Q_ACTOR_BY_ALIAS`, `Q_PROMISES`, `add_fact`, `invalidate_fact`, `get_briefing`). Skills should validate input using JSON Schema.  
**Status**: ✅ Done

**FR-CA-08**: `assemble_context` MUST support multi-word queries where query terms appear non-adjacently in fact content. Implementation uses SurrealDB `@@` full-text search operator (primary) with per-word `CONTAINS` OR fallback. Query preprocessing strips `episode:xxx` references, boolean operators, quoted phrases, and tokens < 2 characters.  
**Status**: ✅ Done

### 5.9 Agent Scenarios (Skills/Flows)

**FR-AG-01**: System MUST provide "skills" as memory operations: `ingest_document`, `extract_entities`, `resolve_entity`, `assemble_context`, `create_task`, `send_message_draft`, `schedule_meeting`, `update_metric`.  
**Status**: ✅ Done

**FR-AG-02**: Skills MUST be accessible via MCP interface (stdio/http/socket) so IDEs/assistants can call them uniformly.  
**Status**: ✅ Done

**FR-AG-03**: System MUST support human-in-the-loop mode: confirmation for sending emails, changing promise/task status. For entity merging (`resolve_entity`), system MUST support `require_confirmation` option (if `true` — request confirmation); by default, automatic merging without dry-run is allowed unless explicitly specified otherwise; all actions logged in merge log.  
**Status**: ✅ Done

**FR-AG-04**: System MUST support agent types: personal, team (2 owners), collective (group visibility) at minimum via scope/ACL.  
**Status**: ✅ Done

### 5.10 UI/UX (Minimum for "Context Graph")

**FR-UX-01**: UI MUST allow selecting counterparty/partner/project and get answers:
- "Who promised what to whom? Is it fulfilled?"
- "What metrics/deals were mentioned and how did they change?"
- "What tasks for me/team, priority, deadline?"  
**Status**: ✅ Done

**FR-UX-02**: Each answer MUST have quote and link to primary source (episode/document/timecode).  
**Status**: ✅ Done

**FR-UX-03**: UI MUST allow launching next flow ("find intro to OpenAI → generate email draft") from context screen.  
**Status**: ✅ Done

---

## 6. Data Model

### 6.1 Core Objects

| Object | Required Fields | Acceptance Criteria |
|--------|----------------|---------------------|
| **Episode** | `id`, `source_type`, `source_id`, `content`, `t_ref`, `t_ingested` | For any fact, can open source episode and see exact quote/fragment. |
| **Entity** | `id`, `type`, `canonical_name`, `aliases[]`, `embedding`, `merge_history[]` | Search by any alias returns canonical entity; merge/split reflected in history. |
| **Fact/Item** | `id`, `type`, `content`, `quote`, `entity_links[]`, `t_valid`, `t_invalid?`, `confidence`, `source_episode` | Every fact has quote and valid temporal attributes; correctly disappears/degrades when stale/invalidated. |
| **Edge** | `id`, `from_entity`, `to_entity`, `relation_type`, `strength`, `confidence`, `provenance`, `t_valid`, `t_invalid?` | Relationships tracked with provenance; invalidation supported. |
| **Community** | `id`, `member_entities[]`, `summary`, `updated_at` | Adding new entities updates cluster/summary without full graph recompute. |

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
| `ingest` | Store raw episode | `source_type`, `source_id`, `content`, `t_ref`, `scope` | `episode_id`, `status` |
| `extract` | Extract entities + facts from episode | `episode_id` (optional: `content`/`text` for soft-fallback) | `entities[]`, `facts[]` |
| `resolve` | Deduplicate/merge entities | `entity_candidate`, `scope`, `require_confirmation?` | `canonical_entity_id`, `merge_actions[]` |
| `invalidate` | Mark fact as superseded | `fact_id`, `reason`, `t_invalid` | `ok` |
| `assemble_context` | Build context pack for query | `query`, `scope`, `as_of?`, `token_budget` | `facts[]`, `confidence`, `rationale`, `provenance` |
| `explain` | Get citations for facts | `fact_ids[]` | `citations[]` (episode links, quotes) |

### 7.2 UI Helper Tools

| Tool | Description |
|------|-------------|
| `ui_promises` | List all promise facts (filtered by scope/actor) |
| `ui_metrics` | List all metric facts with history |
| `ui_tasks` | List all task drafts |

### 7.3 Draft Tools (Human-in-the-Loop)

| Tool | Description |
|------|-------------|
| `create_task` | Draft new task (requires confirmation) |
| `send_message_draft` | Draft email/message (requires confirmation) |
| `schedule_meeting` | Draft calendar event (requires confirmation) |
| `update_metric` | Record new metric value (triggers invalidation of old) |

### 7.4 Semantic/Alias Tools (Intent-Based Routing)

For backward compatibility and ease of use, these tools route to canonical tools:

- `ingest_document` → `ingest`
- `extract_entities` → `extract`
- `resolve_entity` → `resolve`

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
**Status**: ✅ Done

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
- [x] UI tools: `ui_promises`, `ui_metrics`, `ui_tasks`
- [x] Draft tools: `create_task`, `send_message_draft`, `schedule_meeting`, `update_metric`
- [x] Alias/semantic tools: `ingest_document`, `extract_entities`, `resolve_entity`
- [x] Consolidation policy: intent-router or minimal set
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

**Status**: ✅ Done

#### 9.2.8 Configuration and Environment

- [x] Required env vars: `SURREALDB_DB_NAME`, `SURREALDB_URL`, `SURREALDB_NAMESPACES`, `SURREALDB_USERNAME`, `SURREALDB_PASSWORD`
- [x] Optional: `LOG_LEVEL` (unified variable, `RUST_LOG` removed from docs)
- [x] Fail-fast behavior on missing/invalid config
- [x] Documentation: recommend `cargo install --locked memory_mcp`, provide examples for installed and built binaries

**Status**: ✅ Done

#### 9.2.9 Security

- [x] No raw-query tool, no external side-effects
- [ ] Use parameterized SurrealDB queries
- [ ] Define minimal roles/permissions in SurrealDB (RBAC)
- [ ] Capabilities lockdown (deny-by-default)

**Status**: Partially done (parameterization, RBAC, capabilities pending)

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
- [x] Test fixtures/embedded SurrealDB (RocksDB used in tests)
- [x] Code formatted (`cargo fmt`) and checked (`cargo clippy`)

**Status**: ✅ Done

#### 9.2.13 Compatibility and Contracts

- [x] Preserve input payload compatibility for existing tools
- [x] Preserve/describe alias-tool behavior or remove with compatible routing
- [ ] Update `mcp.json` examples for stdio-only
- [x] Soften contracts for intent-based calls (normalize empty strings, soft-fallbacks)

**Status**: Partially done (mcp.json examples need review/harmonization)

#### 9.2.14 Deployment and Local Operation

- [ ] Describe Rust binary build (cargo build/release)
- [ ] Describe MCP server startup via stdio
- [ ] Describe environment configuration for local run

**Status**: Not done

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
- All canonical tools, UI tools, draft tools
- Migrations with embedded/filesystem fallback
- Logging with StdoutLogger (human-readable text format)
- Tests (unit/integration/e2e)
- Clippy/fmt clean
- Persistence test (RocksDB)
- Soft-fallbacks for `extract` (no hard errors on missing input)
- Tool call logging (observability)

- Bi-temporal edge filtering (DB-side pushdown) implemented; storage API exposes filtered edge selection used by graph traversals.
- Graph traversal updated: `find_intro_chain` accepts optional `as_of` and uses filtered edges; neighbor ordering made deterministic.
- In-memory/Mock DB clients and acceptance tests updated to mirror bi-temporal semantics; new acceptance test added for as-of traversal behavior.
- Full test suite (unit + integration + acceptance) passes locally; `cargo clippy` completed with no warnings.

- Full-text search (FTS) implemented for `assemble_context`: primary `@@` operator uses existing `fact_content_search` index; per-word `CONTAINS` OR fallback when FTS returns empty results. Query preprocessing via `preprocess_search_query()` strips episode refs, boolean operators, quoted phrases, and short tokens. FakeDbClient and MockDbClient updated to mirror per-word OR semantics. Comprehensive test coverage: 6 unit tests for preprocessing, 2 unit tests for multi-word assemble, 1 acceptance test (3 scenarios), 1 embedded FTS integration test (3 scenarios).
- SurrealDB 3 migration hardening completed: `store_edge` no longer writes null optional invalidation fields, runtime schema now defines `edge_id` and all required SCHEMAFULL tables (`community`, `event_log`, `task`), and legacy SurrealQL syntax updated (`SEARCH ANALYZER` → `FULLTEXT ANALYZER`, `string::is::datetime` → `string::is_datetime`).

**Pending:**
- JSON log format option
- Parameterized queries (security hardening)
- RBAC setup (SurrealDB roles/permissions)
- Retry/backoff for transient errors
- Deployment documentation (build/run/config)
- `.vscode/mcp.json` example harmonization
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
| **API-06** | `explain(context_pack) → episode links/quotes` | ✅ Done |

**Note:** SurrealMCP and SurrealDB transports MUST support production settings: authentication (JWT/auth server), rate limits (RPS/burst), and different transports (stdio/http/socket/RPC/HTTP). API-01..API-06 MUST be accessible via RPC/HTTP and, where appropriate, accept/return CBOR-encoded payloads; all calls must be logged in execution/event log with transport and content-type info.

### 10.2 Acceptance Tests (High-Level)

**AT-01**: After adding email with promise "will do by Friday", system shows promise at counterparty with quote and link to email.  
**Status**: ✅ Done

**AT-02**: If 6 months later new email states "ARR grew to $3M", old fact "$1M ARR" becomes invalidated (or confidence drops sharply), UI shows metric dynamics.  
**Status**: ✅ Done

**AT-03**: User without `hr.salary` scope cannot extract/see salary facts via UI or agent skill.  
**Status**: ✅ Done

**AT-04**: Query "who can introduce to OpenAI" returns chain via graph traversal (2-3 hops) confirmed by sources.  
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
| `LOG_LEVEL` | ❌ | Log level: `trace`, `debug`, `info`, `warn`, `error` | `info` |

### 11.2 Installation

**Recommended:**

```bash
cargo install --locked memory_mcp
```

**From source:**

```bash
cd .agent/rusty_memory_mcp
cargo build --release
```

### 11.3 Running MCP Server

**Installed:**

```bash
memory_mcp
```

**Built from source:**

```bash
./.agent/rusty_memory_mcp/target/release/memory_mcp
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
      "command": "memory_mcp",
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

**Note:** Examples in `.vscode/mcp.json` need review and harmonization with installed binary path.

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

**Current Version**: 2.0  
**Consolidated**: February 5, 2026  
**Next Review**: When significant requirements change or implementation milestones reached

**Changelog:**

- **2026-02-19**: Completed SurrealDB 2.x → 3.x migration validation; fixed `edge` persistence/schema regressions, updated deprecated SurrealQL syntax, and confirmed clean `fmt`/`clippy`/full test run.
- **2026-02-06**: Added FR-CA-08 (multi-word FTS search), AT-08, updated implementation summary with FTS details
- **2026-02-05**: Consolidated three specifications into single source of truth; removed duplications; added implementation plan with statuses
- **2026-02-05**: Consolidated three specifications into single source of truth; removed duplications; added implementation plan with statuses
- **2026-02-05**: Implemented bi-temporal edge filtering for graph traversals; updated traversal API and tests; validated full test suite and clippy clean.
- **2026-01-22**: Memory Agent architecture finalized
- Previous versions: see deprecated files

---

**END OF DOCUMENT**
