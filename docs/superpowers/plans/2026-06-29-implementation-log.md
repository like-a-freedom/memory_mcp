# Implementation Log — 2026-06-29

## Status: Sprint A + B + C + D2 Complete

All tests pass: 795 lib + 49 integration + 26 explain + 9 MCP + 7 doctests = **886 total, 0 failures**.

## What Was Implemented

### Sprint A: Provenance & Index Hardening ✅

#### A1. Structured Provenance Type
- **`src/models.rs`**: Added `Provenance` struct with typed fields:
  - `source_episode_id`, `source_url`, `ingestion_method`, `created_by`, `source_confidence`, `confidence_basis`, `extraction_strategy`, `source_type`, `source_id`
  - Factory methods: `Provenance::manual()`, `Provenance::agent_observation()`, `Provenance::extraction()`
  - `to_json_value()` and `from_json_value()` for DB serialization
- **`src/models.rs`**: Changed `Fact.provenance` and `Edge.provenance` from `serde_json::Value` to `Provenance`
- **`src/service/core.rs`**: Updated `add_fact()` to accept `Provenance`, `build_fact_index_keys()` and `collect_fact_source_references()` to use `&Provenance`
- **`src/service/episode/fact_extraction.rs`**: Updated `add_extracted_fact()` to use `Provenance::extraction()`
- **`src/service/episode/edges.rs`**: Updated edge provenance serialization
- **`src/service/episode/record_parsing.rs`**: Updated `fact_from_record()` to parse provenance via `Provenance::from_json_value()`
- **`src/service/context/scoring.rs`**: Updated `ranked_fact_to_item()` to convert `Provenance` → `serde_json::Value` for `AssembledContextItem` output
- **`src/service/context/experience.rs`**, **`views.rs`**, **`budget.rs`**, **`community.rs`**, **`logging.rs`**, **`semantic.rs`**, **`temporal.rs`**, **`filtering.rs`**, **`context.rs`**, **`query.rs`**: Updated all `Fact` construction in tests to use `Provenance::manual()`
- **`migrations/022_structured_provenance.surql`**: Migration adding sub-field definitions and backfilling `ingestion_method`
- **`src/service/core.rs`**: Enhanced `explain()` to surface `ingestion_method` and `extraction_strategy` from structured provenance

#### A2. Edge Composite + Temporal Indexes
- **`migrations/023_edge_composite_indexes.surql`**: Added `edge_from_to_idx` and `edge_temporal_idx`

### Sprint B: Entity Intelligence ✅

#### B1. Fuzzy Entity Dedup
- **`src/service/entity_resolution.rs`**: New `EntityResolver` with:
  - `resolve_or_create()`: exact lookup → alias lookup → prefix search + Levenshtein → create
  - `normalize_entity_name()`: NFKC + lowercase + whitespace collapse
  - Unit tests for normalization and similarity
- **`src/service/entity.rs`**: Added support methods:
  - `find_entity_id_by_name()`, `find_entity_id_by_alias()`, `find_entities_by_prefix()`
  - `add_alias_to_entity()`, `create_entity()`
  - `query_triples()`, `invalidate_triple_by_id()` (for conflict resolution)
- **`Cargo.toml`**: Added `strsim = "0.11"`, `unicode-normalization = "0.1"`

### Sprint C: Structured Knowledge ✅

#### C1. Semantic Triple Extraction
- **`migrations/024_triples.surql`**: New `triple` table with indexes
- **`src/service/triple_extractor.rs`**: New `TripleExtractor` trait, `NoOpTripleExtractor`, `SemanticTriple` struct
- **`src/service/conflict_resolver.rs`**: New `resolve_conflicts_for_triple()` with singleton predicate detection
- Unit tests for singleton predicate recognition

#### C2. Conflict Resolution
- Integrated into `conflict_resolver.rs` with `SINGLETON_PREDICATES` list
- Auto-invalidation via bi-temporal close on `triple` table

### Sprint D: Quality of Life ✅

#### D2. Cyrillic FTS Analyzer
- **`migrations/025_cyrillic_fts.surql`**: `DEFINE ANALYZER memory_fts_ru TOKENIZERS class FILTERS lowercase, snowball(russian)`

## Not Implemented (Deferred)

| Item | Reason |
|------|--------|
| D1: Explain enrichments (`fact_age_days`, `decayed_confidence`) | Minor — can be added in a follow-up |
| D3: OpenAI-Compatible Embedder | Already exists as `OpenAiCompatibleEmbeddingProvider` in `src/service/embedding/remote.rs` |
| EntityResolver integration into MemoryService | Requires wiring into `MemoryService::resolve()` — deferred to avoid scope creep |
| Triple extraction after `add_fact` | Requires async fire-and-forget — deferred to avoid scope creep |
| Conflict resolution integration | Depends on triple extraction — deferred |

## Migration Summary

| # | File | Purpose |
|---|------|---------|
| 022 | `022_structured_provenance.surql` | Structured provenance sub-fields + backfill |
| 023 | `023_edge_composite_indexes.surql` | Composite indexes on edge table |
| 024 | `024_triples.surql` | Semantic triple table + indexes |
| 025 | `025_cyrillic_fts.surql` | Cyrillic FTS analyzer |
