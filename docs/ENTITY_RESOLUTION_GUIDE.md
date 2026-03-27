# Entity Resolution Guide

**Status:** ✅ Implemented (2026-03-26)
**Last Updated:** 2026-03-27

## Overview

Entity resolution in Memory MCP provides deterministic alias-based lookup with normalization at write time. This guide explains how entity names and aliases are handled throughout the ingestion and retrieval pipeline.

## Key Design Decisions

### 1. Normalization at Write Time

All entity names and aliases are normalized when written to the database:

```rust
// In src/service/core.rs::resolve()
let normalized = super::normalize_text(&candidate.canonical_name);
let aliases = candidate
    .aliases
    .into_iter()
    .filter(|alias| !alias.trim().is_empty())
    .map(|alias| super::normalize_text(&alias))  // ← Normalization here
    .collect::<Vec<_>>();
```

**Normalization rules:**
- Lowercase conversion
- Whitespace trimming and collapse
- Unicode-aware (supports Cyrillic, Latin, etc.)

### 2. Entity Type Classification

The `RegexEntityExtractor` classifies entities into types based on naming patterns:

| Pattern | Type | Examples |
| --- | --- | --- |
| Multi-word (≥2 words) | `person` | "Alice Smith", "Иван Петров", "Maria Garcia" |
| Single-word CamelCase | `technology` | "PostgreSQL", "OpenAI", "Kubernetes" |
| Company suffixes | `company` | "Acme Corp", "Globex Inc", "Initech Limited" |
| Event indicators | `event` | "Tech Summit", "Hackathon 2026" |
| Location gazetteer | `location` | "San Francisco", "Moscow", "Europe" |
| Unknown fallback | `unknown` | Other single-token names |

**Implementation:** `src/service/entity_extraction.rs::classify_entity_type()`

### 3. Unicode-Aware Regex Extraction

The entity extractor uses Unicode character classes:

```rust
Regex::new(r"[\p{Lu}][\p{Ll}]+(?:\s+[\p{Lu}][\p{Ll}]+)+|[\p{Lu}][\p{L}\p{N}]{2,}")
```

**Pattern breakdown:**
- `[\p{Lu}][\p{Ll}]+` — Capital letter followed by lowercase letters
- `(?:\s+[\p{Lu}][\p{Ll}]+)+` — Multi-word names (one or more additional words)
- `|` — OR
- `[\p{Lu}][\p{L}\p{N}]{2,}` — Single-word CamelCase (min 3 chars)

**Supported scripts:**
- Latin (English, European languages)
- Cyrillic (Russian, Ukrainian, Bulgarian, etc.)
- Any Unicode script with uppercase/lowercase distinction

## Retrieval Pipeline

### Single Entity Lookup

```rust
// In src/storage.rs::select_entity_lookup()
// Step 1: Canonical name match
SELECT * FROM entity WHERE canonical_name_normalized = $name LIMIT 1

// Step 2: Alias match
SELECT * FROM entity WHERE aliases CONTAINS $name LIMIT 1
```

### Batch Entity Lookup

```rust
// In src/storage.rs::select_entities_batch()
SELECT * FROM entity 
WHERE canonical_name_normalized IN $names 
   OR aliases CONTAINSANY $names
```

**Why this works:**
- `$names` contains normalized names from the query
- `canonical_name_normalized` is normalized at write time
- `aliases` array is normalized at write time
- Both sides of the comparison use the same normalization

## Example Flow

### Ingestion

```rust
// Input
EntityCandidate {
    entity_type: "person",
    canonical_name: "Dmitry Ivanov",
    aliases: vec!["Dima Ivanov".to_string(), "DI".to_string()],
}

// After normalization (stored in DB)
{
    "entity_id": "entity:abc123",
    "entity_type": "person",
    "canonical_name": "Dmitry Ivanov",
    "canonical_name_normalized": "dmitry ivanov",
    "aliases": ["dima ivanov", "di"]
}
```

### Resolution

```rust
// Query 1: Exact canonical match
resolve("Dmitry Ivanov") 
→ normalized: "dmitry ivanov"
→ matches: canonical_name_normalized = "dmitry ivanov"
→ returns: "entity:abc123"

// Query 2: Alias match
resolve("Dima Ivanov")
→ normalized: "dima ivanov"
→ matches: aliases CONTAINS "dima ivanov"
→ returns: "entity:abc123"

// Query 3: Case-insensitive
resolve("DMITRY IVANOV")
→ normalized: "dmitry ivanov"
→ matches: canonical_name_normalized = "dmitry ivanov"
→ returns: "entity:abc123"

// Query 4: Batch lookup
select_entities_batch(&["dmitry ivanov", "alice smith"])
→ matches: canonical_name_normalized IN [...] OR aliases CONTAINSANY [...]
→ returns: [entity:abc123, entity:def456]
```

## Testing

### Unit Tests

```bash
cargo test regex_entity_extractor
```

**Coverage:**
- `regex_entity_extractor_returns_deterministic_candidates`
- `regex_entity_extractor_includes_single_token_camel_case_names`
- `regex_entity_extractor_filters_out_short_words`
- `regex_entity_extractor_supports_unicode_names`
- `regex_entity_extractor_classifies_company_types`
- `regex_entity_extractor_classifies_event_types`
- `regex_entity_extractor_classifies_person_types`
- `regex_entity_extractor_classifies_technology_types`

### Integration Tests

```bash
cargo test embedded_resolve_alias
```

**Coverage:**
- `embedded_resolve_idempotent_for_canonical_name`
- `embedded_resolve_matches_existing_alias`
- `embedded_batch_lookup_finds_entity_by_alias`

## Troubleshooting

### Alias lookup fails

**Symptoms:** `select_entity_lookup()` returns `None` for known alias

**Check:**
1. Was the alias provided at ingestion time?
2. Is the alias non-empty after trimming?
3. Are you querying with the exact alias text (before normalization)?

**Debug query:**
```sql
SELECT canonical_name, aliases FROM entity WHERE aliases CONTAINS "dima ivanov"
```

### Wrong entity type classified

**Symptoms:** "OpenAI" classified as `unknown` instead of `technology`

**Check:**
1. Does the name match CamelCase pattern? (starts uppercase, has uppercase inside, no spaces)
2. Is it a single word? (multi-word names → `person`)
3. Does it contain a company suffix? (would override to `company`)

**Override:** Provide explicit `entity_type` in `EntityCandidate`

### Cyrillic names not extracted

**Symptoms:** "Иван Петров" not detected in text

**Check:**
1. Is the regex compiled with Unicode support? (default in Rust)
2. Are both words capitalized? (pattern requires `[\p{Lu}][\p{Ll}]+`)
3. Is the text properly encoded as UTF-8?

**Debug:**
```rust
let text = "Иван Петров встретился с Alice Smith";
let extractor = RegexEntityExtractor::new()?;
let candidates = extractor.extract_candidates(text).await?;
println!("{:?}", candidates);
```

## Performance Considerations

### Index Usage

- `canonical_name_normalized` — indexed via default SurrealDB primary key
- `aliases` — uses `CONTAINS`/`CONTAINSANY` operators (array index if available)

### Batch vs Sequential

**Always prefer batch lookup for N > 1 entities:**

```rust
// ✅ Good: O(1) DB round-trip
let entities = db.select_entities_batch(namespace, &names).await?;

// ❌ Bad: O(N) DB round-trips
for name in &names {
    let entity = db.select_entity_lookup(namespace, &name).await?;
}
```

## Future Enhancements

Potential improvements for later iterations:

- **Fuzzy matching** — Levenshtein distance for typo tolerance
- **LLM-assisted resolution** — Context-aware disambiguation for "John Smith" variants
- **Merge workflows** — Explicit entity merge with history tracking
- **Split/rollback** — Undo accidental merges
- **Confidence scoring** — Alias match quality metrics

## References

- `src/service/entity_extraction.rs` — Entity extraction logic
- `src/service/core.rs::resolve()` — Entity resolution entry point
- `src/storage.rs::select_entity_lookup()` — Single entity DB lookup
- `src/storage.rs::select_entities_batch()` — Batch entity DB lookup
- `docs/MEMORY_SYSTEM_SPEC.md` — Full system specification (FR-ER section)
