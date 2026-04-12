---
name: memory-mcp
description: 'Work with Memory MCP server for persistent agent memory. Use when storing episodes, extracting entities/facts, resolving aliases, assembling context, or managing knowledge provenance. Triggers: "ingest content", "extract entities", "resolve entity", "assemble context", "invalidate fact", "memory operations", "knowledge graph", "bi-temporal queries". All tool arguments are flat snake_case; do not use payload wrappers.'
argument-hint: 'Describe the memory operation (ingest, extract, resolve, assemble_context, explain, invalidate)'
user-invocable: true
disable-model-invocation: false
---

# Memory MCP Operations

## When to Use

- Store source material (emails, documents, conversations) as episodes
- Extract structured entities and facts from content
- Resolve ambiguous entity names to canonical identifiers
- Retrieve relevant context for queries with temporal filtering
- Explain provenance of retrieved facts
- Invalidate outdated facts while preserving audit trail

## Core Workflow

### 1. Ingest Episodes

**Purpose:** Store raw source material before extraction.

```
mcp_memory-mcp_ingest({
   source_type: "email",
   source_id: "message-id",
   content: "raw text",
   t_ref: "2026-03-27T10:00:00Z",
   scope: "team"
   // optional: project, t_ingested, visibility_scope, policy_tags
})
```

**Required:** `source_type`, `source_id`, `content`, `t_ref`, `scope`.

**Optional:** `project`, `t_ingested`, `visibility_scope`, `policy_tags`.

**Returns:** `episode_id` — use for subsequent extraction.

**Guidance:** Call `extract` next to derive entities and facts.

### 2. Extract Entities and Facts

**Purpose:** Transform episodes into structured knowledge.

```
mcp_memory-mcp_extract({
   episode_id: "episode:abc123"
})
```

**Returns:** `ExtractResult` with:
- `entities`: extracted people, companies, technologies
- `facts`: promises, metrics, decisions
- `links`: relationships between entities

**Guidance:** Resolve canonical entities for ambiguous names before creating manual links. The tool also accepts inline `content` or `text` plus optional `source_type`, `source_id`, `t_ref`, `scope`, `project`, and `zero_shot_labels`.

### 3. Resolve Entity Aliases

**Purpose:** Get canonical ID for names with multiple variants.

```
mcp_memory-mcp_resolve({
   entity_type: "person",
   canonical_name: "John Smith",
   aliases: ["John", "J. Smith"]
})
```

**Returns:** `entity_id` — use for linking facts.

**Guidance:** Use this `entity_id` when linking facts or relationships.

### 4. Assemble Context

**Purpose:** Retrieve ranked, relevant facts for a query.

```
mcp_memory-mcp_assemble_context({
   query: "promises John made",
   scope: "org",
   as_of: "2026-01-20T00:00:00Z",
   budget: 10,
   fact_types: ["promise"]
})
```

**Returns:** Ranked `AssembledContextItem[]` with:
- `content`: fact text
- `quote`: source snippet
- `source_episode`: provenance reference
- `confidence`: relevance score
- `rationale`: why this was included

**Guidance:** Use these items directly in reasoning or pass to `explain` for citations. Optional filters include `fact_types`, `project`, `view_mode`, `window_start`, and `window_end`.

### 5. Explain Provenance

**Purpose:** Get citation-ready source snippets.

```
mcp_memory-mcp_explain({
   context_items: JSON.stringify([
      {
         fact_id: "fact:xyz...",
         source_episode: "episode:abc...",
         quote: "I'll have the MVP ready by this Friday"
      }
   ])
})
```

**Returns:** `ExplainItem[]` with:
- Full source quotes
- Episode metadata
- Ready-to-cite text

**Guidance:** `context_items` must be a JSON array string; items may be full context objects with snake_case keys or plain source ID strings. Use these citations directly in the final response.

### 6. Invalidate Facts

**Purpose:** Mark facts as outdated while preserving history.

```
mcp_memory-mcp_invalidate({
   fact_id: "fact:old_decision...",
   reason: "Decision reversed: new timeline approved",
   t_invalid: "2026-01-20T00:00:00Z"
})
```

**Returns:** Confirmation message.

**Guidance:** Re-run `assemble_context` with fresh `as_of` to confirm fact is no longer active.

## Common Patterns

### Pattern: New Document Ingestion

```
1. mcp_memory-mcp_ingest({ source_type: "document", source_id: "doc-123", content: "...", t_ref: "2026-03-27T10:00:00Z", scope: "team" })
   → Returns episode_id

2. mcp_memory-mcp_extract({ episode_id: "episode:..." })
   → Returns entities, facts, links

3. For each ambiguous entity:
   mcp_memory-mcp_resolve({ entity_type: "person", canonical_name: "Alice Smith", aliases: ["A. Smith", "Alice S."] })
   → Returns entity_id

4. Use entity_ids in subsequent fact linking
```

### Pattern: Query with Context Assembly

```
1. mcp_memory-mcp_assemble_context({
     query: "What did Alice promise about the API deadline?",
     scope: "team",
     as_of: "2026-03-27T12:00:00Z",
     budget: 10
   })
   → Returns ranked context items

2. mcp_memory-mcp_explain({ context_items: JSON.stringify([...context items...]) })
   → Returns citation-ready quotes

3. Use citations in final response
```

### Pattern: Historical Query

```
1. mcp_memory-mcp_assemble_context({
       query: "What was the status of Project X in January?",
       scope: "org",
       as_of: "2026-01-31T23:59:59Z",  // End of January
       window_start: "2026-01-01T00:00:00Z",
       window_end: "2026-01-31T23:59:59Z"
    })
   → Returns facts valid during that period
```

## Scope Management

| Scope | Use Case | Access Control |
|-------|----------|----------------|
| `personal` | Private notes, individual tasks | Only owner |
| `team` | Team decisions, shared projects | Team members |
| `org` | Company-wide policies, announcements | All org members |
| `private-domain` | Restricted content (e.g., HR) | Domain-specific ACL |

**Best Practice:** Always use the most restrictive scope that fits the content.

## Entity Resolution

### Automatic Classification

The system classifies entities by naming patterns:

| Pattern | Type | Examples |
|---------|------|----------|
| Multi-word names | `person` | "Alice Smith", "Иван Петров" |
| CamelCase single-word | `technology` | "PostgreSQL", "Kubernetes" |
| Company suffixes | `company` | "Acme Corp", "Globex Inc" |
| Event indicators | `event` | "Tech Summit", "Hackathon 2026" |
| Location gazetteer | `location` | "San Francisco", "Moscow" |

### Normalization

All entity names and aliases are normalized at write time:
- Lowercase conversion
- Whitespace trimming and collapse
- Unicode-aware (supports Cyrillic, Latin, etc.)

**Implication:** Lookups are case-insensitive and whitespace-tolerant.

## Anti-Patterns

### ❌ Don't: Retrieve before storing

```
// WRONG: Trying to assemble context without prior ingestion
assemble_context(query="What did we decide about X?")
→ Returns empty or irrelevant results
```

**Fix:** Ingest relevant documents first, then query.

### ❌ Don't: Use wrong scope

```
// WRONG: Storing team decisions in personal scope
ingest(source_type="meeting", scope="personal", content="Team decided to use Rust...")
→ Team members cannot retrieve this decision
```

**Fix:** Use `scope="team"` for shared decisions.

### ❌ Don't: Skip entity resolution

```
// WRONG: Creating facts with raw names
extract(episode_id=...) → entity: "A. Smith"
// Later query uses "Alice Smith" → no match
```

**Fix:** Resolve aliases to canonical IDs before linking.

### ❌ Don't: Delete facts directly

```
// WRONG: No delete operation exists
// Facts are immutable; use invalidation instead
```

**Fix:** Use `invalidate` with reason and timestamp to preserve audit trail.

## Bi-Temporal Model

Memory MCP tracks two time dimensions:

1. **Valid Time (`t_ref`)**: When the fact was true in the real world
2. **Transaction Time (`t_ingested`)**: When the fact was recorded in the system

### Query Implications

- `as_of`: Query point-in-time snapshot (both dimensions considered)
- `window_start/window_end`: Filter by valid time range
- Invalidated facts remain queryable for historical analysis

## Error Recovery

| Error | Cause | Recovery |
|-------|-------|----------|
| `Invalid t_ref format` | Non-ISO 8601 datetime | Use format: `2026-03-27T10:00:00Z` |
| `Invalid t_invalid format` | Non-ISO 8601 datetime | Same as above |
| `No input` | Missing both `episode_id` and content | Provide one of them |
| `Invalid params` | Missing required fields | Check required parameters per tool |

## Response Fields

### ToolResponse Structure

All tools return a consistent wrapper:

```json
{
  "status": "success" | "partial",
  "result": <tool-specific data>,
  "guidance": "Next step suggestion",
  "has_more": true | false,
  "total_count": 42,
  "next_offset": null
}
```

**Key fields:**
- `guidance`: Always read this — it tells you what to do next
- `has_more`: Pagination flag for large result sets
- `total_count`: Total records available

## Integration with Agents

### Memory Agent Pattern

```
Product Manager Agent (PDM)
├─ Strategy & roadmapping
├─ Stakeholder management
└─ Delegates memory ops via runSubagent("Memory Agent", ...)
   │
   ▼
Memory Agent (Specialized)
├─ Ingest episodes
├─ Extract entities/facts
├─ Resolve aliases
├─ Assemble context
└─ Manage invalidations
```

### Subagent Invocation

```
runSubagent(
  agentName: "Memory Agent",
  prompt: "Ingest the meeting notes from today's standup and extract action items"
)
```

**Important:** Always use the exact agent name `"Memory Agent"` (not `"memory"`) to avoid "requested agent not found" errors.