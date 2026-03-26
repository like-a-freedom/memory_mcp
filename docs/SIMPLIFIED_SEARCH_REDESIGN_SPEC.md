# Simplified Search Redesign Specification

**Status:** Proposed target-state specification for the next breaking redesign  
**Date:** 2026-03-26  
**Scope:** Search and retrieval only  
**Precedence:** This document defines the intended target design for the next implementation wave. It does **not** claim that the current runtime already matches this design.

---

## 1. Why this redesign exists

The current repository contains a mixed retrieval story:

- lexical full-text retrieval is the active first-stage search path,
- graph traversal is used for relationship-aware expansion,
- community summaries add another retrieval leg,
- embedding fields, HNSW indexes, and semantic retrieval scaffolding remain in the runtime despite being disabled by default.

That shape is more complex than the product actually needs.

For this repository, the memory corpus is personal/team knowledge, not a web-scale document retrieval system. Under that workload, the design should optimize for:

1. **determinism**,
2. **explainability**,
3. **single-binary deployment**,
4. **minimal moving parts**,
5. **easy maintenance**, and
6. **strong performance on compositional queries involving names, projects, promises, and relationship chains**.

Recent retrieval literature supports this simplification:

- single-vector embedding retrieval has hard expressiveness limits on compositional queries,
- sparse retrieval remains strong for precise multi-constraint lookup,
- graph expansion is especially valuable once relevant entities are identified,
- temporal graph memory is useful when time and relationships are first-class.

Therefore, the target design shifts from **"lexical + graph + dormant semantic branch"** to **"lexical/BM25 primary retrieval + graph expansion + deterministic fusion"**, with embeddings removed from the runtime entirely.

---

## 2. Hard constraints

The redesign MUST satisfy all of the following constraints.

### 2.1 Deployment

- The MCP server ships as a **standalone binary**.
- The runtime must not require an external embedding model, vector service, or extra sidecar process.
- Search must work in embedded/local SurrealDB mode and in remote SurrealDB mode.

### 2.2 Schema bootstrap

- The binary owns a **single startup-applied schema/bootstrap path**.
- Schema changes may still be tracked internally, but the deployment model must remain: **start binary → binary ensures schema**.
- No separately distributed migration pack is allowed as an operational requirement.

### 2.3 Compatibility posture

- **Breaking changes are allowed.**
- Obsolete search code and schema fields should be removed rather than preserved behind compatibility shims.

### 2.4 Design principles

- **KISS**: one primary retrieval strategy, not several half-enabled ones.
- **YAGNI**: do not keep dormant vector infrastructure “just in case”.
- **DRY**: one canonical query pipeline for context assembly.
- **DDD**: retrieval operates on domain concepts first: episodes, entities, facts, edges, communities.

---

## 3. Core design decisions

### 3.1 Remove embeddings from the runtime

The redesigned system MUST remove all embedding-specific runtime behavior:

- no `EmbeddingProvider` in the production retrieval path,
- no `NullEmbedder`,
- no embedding fields on `episode`, `entity`, or `fact`,
- no HNSW indexes,
- no `SURREALDB_EMBEDDING_DIMENSION`,
- no semantic ANN lookup in context assembly,
- no embedding backfill jobs.

The redesign MUST also complete the graph model migration instead of preserving pseudo-graph string joins:

- edges MUST use native SurrealDB relation endpoints via `in` / `out` record links,
- edges MUST NOT rely on denormalized `from_id` / `to_id` string fields as the primary graph contract,
- graph traversal queries MUST operate on record links first, not on Rust-side string join logic.

Embeddings may be discussed in architecture notes as a possible future experiment, but they are **not** part of the target runtime design.

### 3.2 Lexical search becomes the primary retrieval primitive

The primary candidate generator MUST be SurrealDB full-text retrieval with BM25-style ranking.

The spec requires a real full-text analyzer rather than the current whitespace-only tokenizer.

Target analyzer shape:

```sql
DEFINE ANALYZER memory_fts
	TOKENIZERS class
	FILTERS lowercase, ascii, snowball(english);
```

Equivalent analyzer configurations are acceptable only if they preserve the same intent:

- punctuation-aware tokenization,
- case-insensitive matching,
- ASCII normalization,
- stemming suitable for English fact text.

Target behavior:

1. Normalize the query.
2. Run full-text retrieval against the canonical searchable text fields.
3. Return the top lexical candidates with deterministic ranking.

Searchable text fields:

- `fact.content` — primary field,
- `fact.quote` — optional supporting field,
- `community.summary` — optional low-cost thematic field,
- `entity.canonical_name` / `entity.aliases` — for anchor detection, not as the main context result set.

The design target is **BM25-first**, not substring fallback and not vector fallback.

### 3.3 Graph retrieval becomes entity-anchored expansion

Graph retrieval MUST no longer behave like a separate fuzzy retriever. It is a second-stage expansion strategy.

Pipeline:

1. Resolve explicit entity anchors from the query using canonical-name and alias lookup.
2. Extract additional anchors from the top lexical fact hits via their `entity_links`.
3. Expand from anchors with bounded graph traversal.
4. Score graph-derived candidates independently from lexical candidates.

`entity_links` MUST have an explicit storage contract in the redesigned schema. The preferred target contract is:

- `fact.entity_links: array<record<entity>>`

If SurrealDB constraints force a temporary fallback during migration, the only acceptable interim alternative is:

- `fact.entity_links: array<string>` containing canonical entity record IDs

Arbitrary mixed arrays or loosely typed payloads are not allowed in the target design.

Allowed graph evidence:

- directly linked facts,
- 1-hop neighbors by default,
- 2-hop neighbors only for relationship questions,
- active edges only,
- `as_of` filtering applied before traversal.

Graph traversal is intended to answer questions like:

- “who knows X?”
- “who can introduce me to Y?”
- “what else is connected to this person/project?”
- “what recent facts are attached to this entity cluster?”

### 3.4 Deterministic fusion

Final ranking MUST combine lexical and graph candidates deterministically.

The default fusion algorithm is **Reciprocal Rank Fusion (RRF)**:

$$
\mathrm{score}(d)=\sum_i \frac{1}{k + \mathrm{rank}_i(d)}
$$

with a fixed implementation constant, e.g. $k = 60$.

Why RRF:

- it is robust across heterogeneous scoring scales,
- it avoids fragile score normalization,
- it keeps the implementation simple,
- it is easy to test deterministically.

Tie-break order after fusion:

1. higher fused score,
2. more recent `t_valid`,
3. higher confidence,
4. stable ID ordering.

### 3.5 Communities remain optional graph summaries, not a third search engine

Communities MAY remain in the model, but only as a graph-derived optimization.

Rules:

- communities are derived from persisted graph structure,
- community summaries may help lexical recall,
- community retrieval must not become an independent fuzzy search subsystem,
- if communities add complexity without measurable value, they should be removed in a later cleanup wave.

### 3.6 Explanation stays first-class

Every assembled context item MUST remain explainable.

Explanation payload should identify:

- whether the item entered through lexical retrieval, graph expansion, or both,
- the matched terms or matched summary field,
- the anchor entity or traversal path used for graph inclusion,
- the source episode and quote.

The system should be able to answer:

- “matched query terms `alice` + `atlas` in fact content”,
- “expanded from entity `entity:alice` via edge `works_on`”,
- “included because community `community:atlas` supplied anchor entity `entity:alice`”.

---

## 4. Canonical retrieval pipeline

`assemble_context` MUST be redesigned around one canonical pipeline.

### 4.1 Step 1 — normalize the query

Normalize input by:

- trimming whitespace,
- lowercasing for lexical matching,
- removing reserved transport noise such as `episode:...` references from the free-text portion,
- stripping unsupported boolean syntax,
- preserving quoted phrases when possible,
- splitting into lexical terms for fallback and diagnostics.

### 4.2 Step 2 — detect query mode

The system SHOULD classify queries into a small deterministic set of flags rather than a single mutually exclusive enum.

Supported flags:

- **keyword/topic** — e.g. “atlas launch risk”
- **entity-centric** — e.g. “john smith promises”
- **relationship/path** — e.g. “who can introduce me to OpenAI”
- **time-scoped** — e.g. “what changed last month”

This classifier must remain rule-based and auditable.

Example:

- “what did alice promise last month” should classify as `entity-centric + time-scoped`

Implementation rule:

- downstream retrieval behavior MUST consume the full flag set deterministically,
- the classifier MUST NOT rely on ad-hoc first-match branching.

### 4.3 Step 3 — lexical candidate generation

Run Surreal full-text search over facts using BM25-enabled indexes.

Requirements:

- apply scope and policy filters before ranking,
- apply `as_of` validity filters in the query,
- return a bounded top-N candidate set,
- avoid Rust-side table scans.

### 4.4 Step 4 — anchor resolution

Resolve anchors from:

- exact canonical name matches,
- alias matches,
- entity links from the lexical top-N result set,
- optionally community members from top matched community summaries.

### 4.5 Step 5 — graph expansion

Expand from anchors using bounded BFS.

Defaults:

- 1 hop for entity-centric queries,
- 2 hops for relationship/path queries,
- deterministic neighbor ordering,
- no unbounded traversal.

### 4.6 Step 6 — merge and rank

Fuse lexical and graph result lists with RRF and deterministic tie-breakers.

### 4.7 Step 7 — shape result set

Return only budgeted results with:

- `fact_id`,
- `content`,
- `quote`,
- `source_episode`,
- `confidence`,
- `provenance`,
- explicit `rationale`.

---

## 5. Schema implications

### 5.1 Tables retained

The redesign continues to use these core tables:

- `episode`
- `entity`
- `fact`
- `edge`
- `community` (optional optimization)
- `event_log`
- `script_migration` or equivalent internal bookkeeping

### 5.2 Fields removed

The redesign removes the following runtime fields unless another non-embedding use is explicitly approved:

- `episode.embedding`
- `entity.embedding`
- `fact.embedding`

### 5.3 Indexes retained or added

Retain:

- fact full-text index,
- community summary full-text index if communities remain,
- entity canonical-name and alias indexes,
- edge indexes on relation and endpoint fields.

Target adjustment:

- full-text indexes SHOULD be configured for BM25 ranking support where SurrealDB supports it.
- endpoint indexes SHOULD migrate from `from_id` / `to_id` to native `in` / `out` relation endpoints as part of the graph cleanup.

### 5.4 Indexes removed

Remove:

- `episode_embedding_hnsw`
- `entity_embedding_hnsw`
- `fact_embedding_hnsw`

---

## 6. Breaking changes explicitly approved by this spec

The following breaking changes are part of the target redesign:

1. Remove all embedding-related config, fields, indexes, and retrieval code.
2. Remove semantic retrieval tests and fixtures.
3. Remove docs that describe dormant hybrid embedding ranking as an intended runtime path.
4. Narrow retrieval language across the codebase to: **lexical/BM25 + graph expansion**.
5. Rework context assembly tests around lexical ranking, graph expansion, and fusion.

---

## 7. Non-goals for this redesign

This redesign explicitly does **not** attempt to add:

- learned rerankers,
- external search engines,
- external vector databases,
- cross-encoder reranking,
- entity extraction quality upgrades beyond what is needed to preserve the current ingestion contract,
- multilingual NLP extraction upgrades,
- complex agentic retrieval loops.

Those may be considered later, but they are outside the scope of this simplification wave.

For this redesign, anchor quality is expected to come from existing canonical entity records and aliases. Search should improve retrieval over known entities, not redesign entity extraction itself.

---

## 8. Acceptance criteria for the implementation phase

The later implementation plan MUST verify at least the following target behaviors.

### 8.1 Retrieval correctness

- Multi-word lexical queries retrieve facts when terms appear non-adjacently.
- Punctuation and separator variants such as `atlas_launch` vs `atlas launch` tokenize into compatible lexical matches.
- Person/project queries use alias resolution to find relevant facts.
- Relationship queries return graph-expanded evidence with bounded traversal.
- Final ranking is deterministic across repeated runs.

### 8.2 Simplicity

- No embedding-related environment variables remain.
- No embedding-related schema fields remain.
- No HNSW indexes remain.
- No semantic retrieval branch remains inside `assemble_context`.
- Edges use native SurrealDB record links rather than string endpoint IDs.
- `entity_links` has one explicit, typed storage contract.

### 8.3 Explainability

- Each returned result says whether it came from lexical retrieval, graph expansion, or both.
- Relationship/path results include the anchor and traversal basis.

### 8.4 Operations

- The standalone binary bootstraps the required schema at startup.
- Embedded mode works without any external runtime components beyond SurrealDB itself.

---

## 9. Decision summary

The approved target direction is:

- **remove embeddings completely now**,
- make **BM25/full-text retrieval the default primary search path**,
- make **graph traversal a second-stage expansion mechanism**,
- use **deterministic RRF fusion** for final ranking,
- keep deployment **single-binary and startup-bootstrapped**,
- aggressively delete obsolete search complexity.

This document is the specification input for the next step: writing the implementation plan. Until code changes land, the current implementation-status documents remain descriptive of the existing runtime, not of the target redesign.