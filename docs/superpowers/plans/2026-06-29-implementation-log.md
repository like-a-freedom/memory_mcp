# Implementation Log — 2026-06-29 (Final)

## Status: ✅ ALL SPRINTS COMPLETE — 0 GAPS

**886 тестов, 0 failures.** Все пункты плана реализованы.

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
- Миграция `025_cyrillic_fts.surql`: `memory_fts_ru` с `snowball(russian)`

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
| 025 | `025_cyrillic_fts.surql` | Кириллический FTS анализатор |

## Новые файлы

| Файл | Назначение |
|------|-----------|
| `src/service/entity_resolution.rs` | Нечёткая дедупликация сущностей |
| `src/service/triple_extractor.rs` | Извлечение семантических троек |
| `src/service/conflict_resolver.rs` | Разрешение конфликтов предикатов-синглтонов |
| `src/service/context/triple.rs` | Поиск фактов через тройки в assemble_context |
