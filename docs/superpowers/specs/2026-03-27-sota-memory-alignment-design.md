# SOTA Memory Alignment Design

> **Purpose:** align `memory_mcp` with 2025–2026 agent-memory research without conflating shipped behavior with target-state design and without undoing the approved lexical-first retrieval direction.

**Date:** 2026-03-27  
**Status:** Proposed target-state design  
**Scope:** memory architecture, retrieval refinement, lifecycle policy, evaluation, and MCP-surface implications

---

## 1. Why this document exists

The repository now has three different “truth layers,” and they should stay distinct:

1. `docs/MEMORY_SYSTEM_SPEC.md` describes the **current runtime contract**.
2. `docs/SIMPLIFIED_SEARCH_REDESIGN_SPEC.md` describes the approved **retrieval simplification target**.
3. `docs/SOTA_AI_Memory_2025_2026_vs_memory_mcp.md` identifies the **research gap** between the repository and 2025–2026 state of the art.

That gap analysis is directionally useful, but it must be adapted to repository reality:

- several items it names as gaps are already implemented or partially implemented;
- some SOTA systems assume embedding-heavy or training-heavy architectures that conflict with this repository’s current single-binary, local-first, deterministic direction;
- this MCP server intentionally exposes a small, curated tool surface, so many improvements should land as **internal behavior** rather than as new public tools.

This document defines the repository-fit target state.

---

## 2. Design options considered

### Option A — Keep only a current-state spec

Pros:
- minimal documentation churn;
- no ambiguity about shipped behavior.

Cons:
- SOTA-driven work remains scattered across essays and plans;
- roadmap decisions stay implicit;
- implementation work will repeatedly re-litigate architecture.

### Option B — Fold SOTA target state directly into `MEMORY_SYSTEM_SPEC.md`

Pros:
- one canonical document.

Cons:
- mixes “implemented now” with “target later”;
- makes status claims harder to trust;
- increases review friction because the spec stops being a faithful runtime contract.

### Option C — Dual-track specification model **(recommended)**

Keep:

- `docs/MEMORY_SYSTEM_SPEC.md` as the **current-state** source of truth,
- `docs/SIMPLIFIED_SEARCH_REDESIGN_SPEC.md` as the **retrieval-specific** target spec,
- this document as the **adaptive memory target-state** spec above retrieval.

Pros:
- preserves a trustworthy runtime contract;
- allows target-state design to be ambitious but explicit;
- makes implementation planning straightforward.

Cons:
- requires discipline when updating multiple documents.

**Decision:** adopt Option C.

---

## 3. Hard repository-fit constraints

The target state MUST satisfy all of the following:

### 3.1 Keep the simplified retrieval direction

The SOTA alignment work MUST NOT reintroduce embedding-heavy runtime search as a core dependency.

Allowed:
- BM25/full-text lexical retrieval,
- graph expansion,
- deterministic fusion,
- temporal and usage-aware ranking heuristics.

Not allowed as part of the primary roadmap:
- mandatory embedding providers,
- HNSW/vector-search revival as the default path,
- external search sidecars required for local use.

### 3.2 Preserve the intent-driven MCP shape

This server currently performs well architecturally because its public contract is small.

The target state SHOULD preserve the six canonical memory operations and prefer:

- internal orchestration inside `ingest`, `extract`, `assemble_context`, and `explain`,
- backwards-compatible parameter extensions,
- structured rationale fields,

over adding many new MCP tools.

### 3.3 Stay local-first and deterministic

New behavior should continue to work in:

- embedded SurrealDB mode,
- remote SurrealDB mode,
- single-binary local execution,
- predictable, testable retrieval paths.

### 3.4 Respect current implementation facts

The design must build on what already exists:

- bi-temporal facts and edges,
- native `RELATE` edges,
- lexical-first retrieval,
- connected-component community maintenance,
- background lifecycle workers,
- provenance-aware `explain()`.

---

## 4. Chosen target architecture

The recommended architecture is **adaptive graph memory under a stable MCP contract**.

Instead of copying Supermemory, Graphiti, HippoRAG, A-MEM, or Second Me literally, `memory_mcp` should absorb their strongest ideas into four layers:

1. **Write layer** — richer indexing keys (entity names, aliases, temporal markers in `index_keys` at ingest time).
2. **Manage layer** — heat-aware lifecycle using `access_count` and `last_accessed` fields on fact, topology-aware community refresh.
3. **Read layer** — deterministic lexical + graph fusion, timeline-oriented retrieval mode via `assemble_context` extension.
4. **Evaluate layer** — LongMemEval-style acceptance harness for benchmark-driven verification.

This maps cleanly onto the 2026 survey framing of memory as a **write–manage–read loop**, with a lightweight reflection channel.

---

## 5. Target capabilities

### 5.1 Write layer

### 5.1.1 Fact-augmented key expansion

Each stored fact should carry additional lexical search keys derived from existing extraction output.

Target fields:

- `fact.index_keys: array<string>`

Population sources:

- canonical entity names,
- aliases resolved at write time,
- extracted temporal markers when explicit,
- selected normalized noun phrases when extraction confidence is high.

This captures the LongMemEval insight that indexing quality matters as much as retrieval.

### 5.1.2 Lightweight memory evolution — DEFERRED

> **Deferred rationale (review 2026-03-27):** searching for related active facts and updating their metadata at write time adds significant complexity (new queries, merge logic, `related_fact_ids` field) for marginal retrieval improvement in a personal memory corpus. Related-ness is already captured by shared `entity_links` and community membership. Revisit only if benchmark results show a measurable gap.

### 5.1.3 Session-summary facts — DEFERRED

> **Deferred rationale (review 2026-03-27):** generating meaningful summary facts requires language understanding (LLM). The current extraction pipeline is regex-based and the binary ships without an LLM runtime. This contradicts the single-binary, no-external-dependency constraint. Revisit when/if the server gains an LLM integration path.

---

### 5.2 Manage layer

### 5.2.1 Heat-aware lifecycle policy

Pure age-based archival is too blunt.

Target fields:

- `fact.access_count: int`
- `fact.last_accessed: option<datetime>`

Archival and decay should use heat-aware rules:

- cold + old facts decay/archive faster,
- recently accessed facts decay slower,
- summary or persona-critical facts may have protected policy tags.

### 5.2.2 Usage-aware retention

The system should treat evidence that is actually reused as higher value than merely retrieved evidence.

Preferred repository-fit approach:

- increment `fact.access_count` and update `fact.last_accessed` inside `assemble_context` for returned facts,
- increment by a larger delta (e.g., `+= 3`) when a fact is passed through `explain`,
- lifecycle workers consume `access_count` and `last_accessed` directly from the fact record.

This uses SurrealDB atomic updates (`UPDATE fact SET access_count += 1, last_accessed = time::now()`) and avoids a separate `usage_event` table, extra queries, and extra schema.

A dedicated "feedback" tool or separate usage-event table should only be introduced if evidence proves the `access_count` signal is insufficient.

### 5.2.3 Community maintenance remains topology-driven

Community detection should continue to come from graph structure, not from free-text clustering.

Near-term target:
- strengthen connected-components baseline with incremental refresh triggers and member statistics.

Longer-term target:
- introduce a ranking signal such as node centrality or weighted expansion, without requiring a full Graphiti-style external stack.

---

### 5.3 Read layer

### 5.3.1 Temporal marker indexing at write time

Instead of expanding queries at read time (Rust-side query manipulation), temporal markers should be extracted from fact content at **ingest time** and stored in `index_keys`.

Examples of extracted markers:
- month name + year ("march 2026"),
- ISO date components ("2026-03"),
- explicit temporal phrases from the fact content.

BM25 FTS with the existing `memory_fts` analyzer then matches temporal terms naturally without client-side query variants. This follows KISS: index better at write time instead of adding complexity at read time.

### 5.3.2 Timeline-oriented retrieval mode

The repository needs a first-class way to answer temporal questions chronologically.

Preferred shape:
- extend `assemble_context` with an optional retrieval/view mode rather than adding a new public tool.

Target behavior:
- detect timeline intent,
- filter by entity anchor and time window,
- return results sorted by `t_valid` rather than fused relevance score when timeline mode is selected.

### 5.3.3 Deterministic lexical + graph fusion stays primary

The read path remains:

1. lexical retrieval,
2. anchor resolution,
3. bounded graph expansion,
4. deterministic fusion.

SOTA alignment should improve candidate quality and ranking policy, not replace this core shape.

### 5.3.4 PPR-class associative retrieval is deferred

HippoRAG-style Personalized PageRank is architecturally interesting, but it is not a Phase 1 requirement.

It should be documented as:
- a research-track enhancement,
- gated behind evidence from benchmark gaps,
- only worth pursuing after baseline observability exists.

---

### 5.4 Reflect and evaluate layer

### 5.4.1 LongMemEval-style benchmark harness

The repository needs a repeatable evaluation harness that covers:

- information extraction,
- multi-session reasoning,
- temporal reasoning,
- knowledge update,
- abstention.

This need not reproduce the full public benchmark immediately. A repository-local acceptance harness is sufficient for the first wave.

### 5.4.2 Reflection without tool sprawl

Retrospective reflection:
- `access_count` and `last_accessed` incremented during retrieval and explanation flows,
- lifecycle policies consume these signals directly from fact records.

Prospective reflection (session summaries) is deferred — see §5.1.3.

This keeps reflection internal and avoids adding a separate `usage_event` table.

### 5.4.3 Persona memory remains a future track

The Second Me idea is valuable, but model-training-centric L2 memory is out of scope.

Repository-fit adaptation:
- a protected, low-decay `persona` or equivalent namespace/profile tier,
- represented as ordinary facts with stricter retention policy,
- no fine-tuned personal model requirement.

---

## 6. MCP-surface implications

This section is intentionally strict.

### 6.1 Default rule

Prefer **no new public MCP tools** for Phase 1.

Implement via:
- `assemble_context` enhancements,
- `explain`-driven feedback capture,
- background jobs,
- ingestion-side enrichment.

### 6.2 Acceptable backwards-compatible extensions

Potential optional additions to existing tool contracts:

- `assemble_context.view_mode = standard | timeline`
- `assemble_context.window_start`
- `assemble_context.window_end`
- richer rationale / trace metadata in assembled items

### 6.3 Deferred additions

Only consider new tools after the benchmark harness proves a clear usability gap.

Candidates that remain explicitly deferred:
- dedicated `record_usage`,
- dedicated `summarize_session`,
- dedicated `temporal_timeline`.

These are conceptually valid intents, but they would expand the public surface before the repository has evidence that the extra surface is worth the complexity.

---

## 7. Data-model additions recommended by this design

Near-term additions:

- `fact.index_keys: array<string>` — entity names, aliases, temporal markers at ingest time
- `fact.access_count: int` — incremented on retrieval and explain
- `fact.last_accessed: option<datetime>` — set on retrieval and explain

All three fields live on the `fact` table directly. SurrealDB atomic updates (`UPDATE fact SET access_count += 1, last_accessed = time::now()`) handle heat tracking without a separate table.

> **Removed from this design (review 2026-03-27):** `fact.related_fact_ids` and `usage_event` table were removed as YAGNI violations. Related-ness is already captured by shared `entity_links` and community membership. Usage signals are adequately captured by `access_count` + `last_accessed` on the fact itself.

Deferred additions:

- `fact_type = "summary"` — deferred until LLM integration path exists
- `policy_tags` or retention hints for persona/profile facts

---

## 8. Acceptance criteria for the implementation plan

The implementation plan derived from this design must verify at least:

1. facts store `index_keys` derived from entities, aliases, and temporal markers at ingest time;
2. BM25 retrieval can match facts via both `content` and `index_keys`;
3. `access_count` and `last_accessed` are updated on retrieval and explain;
4. lifecycle workers skip recently-accessed ("hot") facts;
5. `assemble_context` supports a timeline-ordered view mode via optional `view_mode` parameter;
6. a LongMemEval-style acceptance harness exists in-repo covering 5 benchmark categories;
7. the public MCP surface remains intentionally small — no new tools;
8. no embedding fields, HNSW indexes, or external dependencies are reintroduced.

---

## 9. Decision summary

The repository should evolve toward **adaptive memory**, not toward a general-purpose memory research platform.

That means:

- keep current-state and target-state specs separate;
- preserve lexical/BM25 + graph retrieval as the runtime backbone;
- add write-time index enrichment and heat-aware lifecycle internally;
- defer features requiring LLM (summaries) or complex graph algorithms (PPR, memory evolution);
- delegate to SurrealDB wherever possible (atomic updates, FTS, temporal filtering);
- use benchmark-driven planning rather than paper-by-paper cargo culting;
- preserve a curated MCP surface unless evidence proves expansion is necessary.

This document is the design input for the implementation plan created on the same date.
