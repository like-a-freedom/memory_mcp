---
name: memory-mcp
description: 'Manages persistent agent memory via the Memory MCP server: storing episodes, extracting entities and facts, resolving entity aliases, assembling ranked context for a query, explaining fact provenance, and invalidating outdated facts. Use when the user asks to "ingest content", "extract entities", "resolve entity", "assemble context", "invalidate fact", "explain a fact", or mentions "memory operations", "knowledge graph", or "bi-temporal queries". All tool arguments are flat snake_case; payload wrappers and camelCase keys are rejected.'
argument-hint: 'Describe the memory operation (ingest, extract, resolve, assemble_context, explain, invalidate)'
user-invocable: true
disable-model-invocation: false
---

# Memory MCP Operations

All tools return `ToolResponse<T>` with fields: `status`, `result`, `guidance` (read this — it tells you what to do next), `has_more`, `total_count`, `next_offset`. All parameter keys are flat snake_case. CamelCase and `payload` wrappers are rejected.

**Rejected:**
```
// WRONG — nested payload, camelCase
mcp_memory-mcp_ingest({ payload: { sourceType: "email", sourceId: "x" } })
```

**Correct:**
```
mcp_memory-mcp_ingest({ source_type: "email", source_id: "x", content: "...", t_ref: "2026-03-27T10:00:00Z" })
```

## Common workflow

For a full capture-to-retrieval cycle, follow in order:

```
Memory Progress:

Step 1: ingest raw content → get episode_id
Step 2: extract entities/facts from episode_id
Step 3 (optional): resolve any ambiguous entity names
Step 4: assemble_context for the user's query
Step 5 (optional): explain context items for citations
```

Skip steps 1-2 if the caller already has an `episode_id` or wants to query existing memory directly with `assemble_context`.

## Tools

### ingest

Store raw source material as an episode.

```
mcp_memory-mcp_ingest({
   source_type: "email",
   source_id: "message-id",
   content: "raw text",
   t_ref: "2026-03-27T10:00:00Z",
   scope: "team"
})
```

| Field | Required | Default | Notes |
|-------|----------|---------|-------|
| `source_type` | yes | — | e.g. `"email"`, `"document"`, `"tfs_work_item"` |
| `source_id` | yes | — | unique within source_type |
| `content` | yes | — | raw text |
| `t_ref` | yes | — | ISO 8601 |
| `scope` | no | `"org"` | see Scope section |
| `project` | no | — | project tag for scoped retrieval |
| `t_ingested` | no | now | override ingestion timestamp |
| `visibility_scope` | no | same as `scope` | |
| `policy_tags` | no | `[]` | |

Returns `episode_id`. Guidance: call `extract` next.

### extract

Transform episodes into structured knowledge. Two modes:

- **By episode_id** — searches all configured namespaces to find the episode. Scope is not used.
- **By inline content** — ingests first, then extracts. Scope determines namespace (defaults to `"org"`).

```
mcp_memory-mcp_extract({ episode_id: "episode:abc123" })
```

| Field | Required | Default | Notes |
|-------|----------|---------|-------|
| `episode_id` | one of these | — | |
| `content` | or this | — | inline text |
| `text` | or this | — | alias for `content` |
| `source_type` | no | `"ad-hoc"` | inline mode only |
| `source_id` | no | content hash | inline mode only |
| `t_ref` | no | now | inline mode only |
| `scope` | no | `"org"` | inline mode only |
| `zero_shot_labels` | no | — | custom GLiNER entity labels |

Validation: exactly one input source. Both `content`+`text`, both `episode_id`+content, or neither → error.

Returns `ExtractResult`: `episode_id`, `entities` (entity_id, type, canonical_name), `facts` (fact_id, type), `links` (entity_id, episode_id), `warnings` (contradiction alerts).

Task-optional: task-capable clients can use `tasks/call` for async execution, then `tasks/get` + `tasks/result`.

### resolve

Get canonical entity ID for a name with variants.

```
mcp_memory-mcp_resolve({
   entity_type: "person",
   canonical_name: "John Smith",
   aliases: ["John", "J. Smith"]
})
```

| Field | Required | Default |
|-------|----------|---------|
| `entity_type` | yes | — |
| `canonical_name` | yes | — |
| `aliases` | no | `[]` |

Returns `entity_id` (format: `entity:{type}:{snake_case_name}`). Resolution: exact normalized match → alias match → fuzzy match (Levenshtein ≥ 0.85) → create new.

### assemble_context

Retrieve ranked facts for a query.

```
mcp_memory-mcp_assemble_context({
   query: "promises John made",
   scope: "org",
   budget: 10,
   fact_types: ["promise"]
})
```

| Field | Required | Default | Notes |
|-------|----------|---------|-------|
| `query` | yes | — | |
| `scope` | yes | — | |
| `project` | no | — | restrict to project |
| `fact_types` | no | `[]` | filter: `"note"`, `"decision"`, `"metric"`, `"promise"`, `"experience"` |
| `as_of` | no | now | point-in-time snapshot |
| `budget` | no | `5` | max facts |
| `view_mode` | no | — | `"timeline"` for chronological sort |
| `window_start` | no | — | valid time lower bound |
| `window_end` | no | — | valid time upper bound |

Returns `AssembledContextItem[]`: `fact_id`, `content`, `quote`, `source_episode`, `confidence`, `rationale`, `retrieval_tier`, `provenance`.

### explain

Get citation-ready source snippets for context items.

```
mcp_memory-mcp_explain({
   context_items: JSON.stringify([
      { fact_id: "fact:xyz", source_episode: "episode:abc", quote: "I'll have the MVP ready by Friday" }
   ])
})
```

`context_items` is a JSON array string. Items may be snake_case objects or plain source ID strings (`"episode:abc"`). Legacy camelCase keys are rejected.

Returns `ExplainItem[]`: `fact_id`, `content`, `quote`, `source_episode`, `scope`, `t_ref`, `t_ingested`, `all_sources` (provenance lineage), `graph_insights`, `fact_age_days`, `decayed_confidence`.

### invalidate

Mark a fact as outdated while preserving audit trail. Facts are immutable — no delete exists.

```
mcp_memory-mcp_invalidate({
   fact_id: "fact:old_decision",
   reason: "Decision reversed",
   t_invalid: "2026-01-20T00:00:00Z"
})
```

All fields required. Returns confirmation.

## Scope and Namespace

Scopes map to SurrealDB namespaces. The mapping is fixed:

| Scope | Resolves to namespace | Notes |
|-------|----------------------|-------|
| `personal` | `personal` | |
| `team` | `team` → `org` | falls back to `org` |
| `org` | `org` | default for ingest/extract |
| `private-domain` | `private-domain` → `private` | |

Case-insensitive. `SURREALDB_NAMESPACES` must contain at least one namespace from the resolution chain for each scope you use. If no match → validation error.

**Rule:** use the most restrictive scope that fits.

## Entity Classification

The classifier runs in priority order:

1. **company** — name contains: Corp, Inc, Ltd, LLC, GmbH, AG, SA, PLC, Company, Group, Systems, Technologies, Solutions, Labs, etc.
2. **event** — contains: Conference, Summit, Meetup, Hackathon, Workshop, Festival, etc.
3. **location** — contains: City, County, State, Province, Country, District; OR is in gazetteer (~130 known locations)
4. **person** — multi-word (2+ tokens), not matching above
5. **technology** — single-word CamelCase (starts uppercase, contains another uppercase)
6. **unknown** — fallback

**Normalization:** NFKC → lowercase → whitespace collapse. Lookups are case-insensitive.

**Resolution:** exact normalized name → alias match → fuzzy (Levenshtein ≥ 0.85) → create new entity.

## Bi-Temporal Model

- **Valid time (`t_ref`)**: when the fact was true in reality
- **Transaction time (`t_ingested`)**: when recorded in memory

`as_of` queries both dimensions. Invalidated facts remain queryable for history. Fact types: `note`, `decision`, `metric`, `promise`, `experience`.

## Error Recovery

| Error | Cause | Fix |
|-------|-------|-----|
| `no namespace configured for scope X` | `SURREALDB_NAMESPACES` missing required namespace | Add namespace to config (see Scope section) |
| `Invalid t_ref format` | Non-ISO 8601 | Use `2026-03-27T10:00:00Z` |
| `episode_id not found` | Episode doesn't exist or wrong namespace | Check scope, ensure episode was ingested |
| `No input` | Missing both `episode_id` and content | Provide exactly one |
| `Invalid params` | Missing required fields or camelCase keys | Use snake_case, check required params |
