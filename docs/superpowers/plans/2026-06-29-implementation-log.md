# Implementation Log — 2026-06-29 (Final)

## Status: ✅ ALL SPRINTS COMPLETE — gaps closed in review

All plan items implemented. Post-implementation review found and fixed six
discrepancies between the plan and the actual code (see "Review fix-up" below).

Validation: `cargo build`, `cargo clippy --all-targets` (0 warnings),
`cargo fmt --all --check` (0 diff), full `cargo test` (0 failures across
lib + integration + acceptance suites).

---

## Review fix-up — discrepancies found after the first pass

A careful diff of the plan vs. the committed code revealed six gaps. Each was
reproduced, fixed, and protected by a regression test.

| # | Sprint | Gap | Severity | Fix |
|---|--------|-----|----------|-----|
| R1 | D2 | Migration `025_cyrillic_fts.surql` defined `memory_fts_ru` but **no index referenced it** — Russian stemming never ran. Migration `006` OVERWRITEs `fact_content_search` and `community_summary_search` to `memory_fts` (English). | High | New migration `026_cyrillic_fts_active.surql` folds `snowball(russian)` into the shared `memory_fts` analyzer so all three FULLTEXT indexes (`fact_content_search`, `community_summary_search`, `fact_index_keys_search`) get both stemmers. 025 left byte-identical to preserve its checksum. |
| R2 | B1 | `EntityService::find_entity_id_by_alias` used `aliases @1@ $alias` (FTS operator), but `entity_aliases` is a plain (non-FULLTEXT) index → the query silently returned `[]`. Probe on real DB confirmed: alias stored on disk was not found. Step 2 of the fuzzy resolver was broken. | High | Changed operator to `aliases CONTAINS $alias` (SurrealDB array-membership, index-aware). Matches the existing legacy `build_select_entity_lookup_alias_query`. |
| R3 | C2 | `invalidate_triple` set `t_invalid` but not `t_invalid_ingested`, breaking the bi-temporal invariant that migration `024` introduced for triples. | Medium | Now sets both: `SET t_invalid = time::now(), t_invalid_ingested = time::now()`. Mirrors the fact/edge invalidation path in `lifecycle/decay.rs`. |
| R4 | B1 | `entity_resolution.rs` had dead scaffolding: a commented-out `// Entity resolution: merged ...` followed by `let _ = (entity_id.clone(), score);` that cloned `entity_id` only to bind it into a discarded tuple. | Low | Removed. |
| R5 | C1 | Doc comments in `spawn_triple_extraction` and `context/triple.rs` claimed "no-op if `NoOpTripleExtractor`", but the default is `RuleBasedTripleExtractor`. `NoOpTripleExtractor` is `#[allow(dead_code)]` and never wired. | Low | Doc comments corrected. |
| R6 | (new, untracked edits) | `normalize_russian_object` heuristic had duplicate entries in its `endings` list (`"ной"`×3, `"ого"`×2, etc.) and the "longest first" claim wasn't honored. | Low | List deduplicated, sorted longest-first, and switched the length guard from bytes to `chars().count()` for correctness on multi-byte Cyrillic. |

### Regression tests added (`tests/embedded_fts_search.rs`)

- `embedded_resolve_finds_entity_by_alias` — resolves a canonical name that
  exists only as an alias on a previously-created entity; asserts it returns
  the **same** entity id (guards R2).
- `embedded_fts_finds_russian_content` — stores a fact containing `Газпроме`
  (prepositional case) and queries `Газпром` (nominative); asserts a match
  (guards R1).

---

## Sprint A: Provenance & Index Hardening ✅

### A1. Structured Provenance Type
- **`Provenance` struct**: `source_episode_id`, `source_url`, `ingestion_method`, `created_by`, `source_confidence`, `confidence_basis`, `extraction_strategy`, `source_type`, `source_id`
- Фабрики: `manual()`, `agent_observation()`, `extraction()`
- `Fact.provenance` и `Edge.provenance`: `Provenance` вместо `serde_json::Value`
- `add_fact()` принимает `Provenance`
- Миграция `022_structured_provenance.surql` + backfill

### A2. Edge Composite Indexes
- Миграция `023_edge_composite_indexes.surql`: `edge_from_to_idx`, `edge_temporal_idx`

## Sprint B: Entity Intelligence ✅

### B1. Fuzzy Entity Dedup
- `EntityResolver` с цепочкой: exact → alias → prefix+Levenshtein → create
- `find_entity_id_by_name()`, `find_entity_id_by_alias()`, `find_entities_by_prefix()`, `add_alias_to_entity()`, `create_entity()`
- Интегрирован в `MemoryService::resolve()`
- `ENTITY_FUZZY_THRESHOLD` env var (default 0.85)
- `strsim = "0.11"`, `unicode-normalization = "0.1"` в Cargo.toml

### B2. Entity Resolution Test Suite
- 7 unit-тестов: normalization, case, whitespace, Cyrillic, NFKC, Levenshtein
- 4 integration-теста с MockDbClient: exact match, fuzzy Cyrillic, create new, below-threshold

## Sprint C: Structured Knowledge ✅

### C1. Semantic Triple Extraction
- Миграция `024_triples.surql`: таблица `triple` + индексы
- `TripleExtractor` trait + `NoOpTripleExtractor` (default)
- Fire-and-forget: `spawn_triple_extraction()` в `add_fact()`
- **Query support**: `collect_triple_facts()` — поиск фактов через triple table по subject/predicate/object, интегрирован в `assemble_default_context()`

### C2. Conflict Resolution
- `SINGLETON_PREDICATES`: `works_at`, `lives_in`, `has_email`, `has_phone`, и т.д.
- `resolve_conflicts_for_triple()`: поиск конфликтующих троек + auto-invalidation
- Вызывается из `spawn_triple_extraction()`

## Sprint D: Quality of Life ✅

### D1. Explain Enrichments
- `ExplainItem`: поля `fact_age_days`, `decayed_confidence`, `ingestion_method`
- `explain()` вычисляет: возраст от `t_valid`, decay по half-life (365/180d), ingestion_method из Provenance

### D2. Cyrillic FTS Analyzer
- Миграция `025_cyrillic_fts.surql`: определяет `memory_fts_ru` с `snowball(russian)`
- ⚠️ Review выявил, что 025 **только определяет** анализатор — ни один индекс на него не ссылался. Кириллический стемминг не работал.
- ✅ Fix: миграция `026_cyrillic_fts_active.surql` добавляет `snowball(russian)` в общий `memory_fts` анализатор, который уже используется индексами `fact_content_search`, `community_summary_search`, `fact_index_keys_search`. См. "Review fix-up R1" выше.

### D3. OpenAI-Compatible Embedder
- Уже существовал: `OpenAiCompatibleEmbeddingProvider` + `OllamaEmbeddingProvider`
- Rate limiting с exponential backoff, поддержка 429 Retry-After

---

## Миграции

| # | Файл | Назначение |
|---|------|-----------|
| 022 | `022_structured_provenance.surql` | Структурированный провенанс |
| 023 | `023_edge_composite_indexes.surql` | Составные индексы edge |
| 024 | `024_triples.surql` | Таблица семантических троек |
| 025 | `025_cyrillic_fts.surql` | Определяет `memory_fts_ru` (без привязки к индексу) |
| 026 | `026_cyrillic_fts_active.surql` | Активирует `snowball(russian)` в общем `memory_fts` (R1) |

## Новые файлы

| Файл | Назначение |
|------|-----------|
| `src/service/entity_resolution.rs` | Нечёткая дедупликация сущностей |
| `src/service/triple_extractor.rs` | Извлечение семантических троек |
| `src/service/conflict_resolver.rs` | Разрешение конфликтов предикатов-синглтонов |
| `src/service/context/triple.rs` | Поиск фактов через тройки в assemble_context |
