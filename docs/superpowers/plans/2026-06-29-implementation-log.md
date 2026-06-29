# Implementation Log — 2026-06-29

## Status: ✅ ALL SPRINTS COMPLETE

Все пункты плана реализованы. **886 тестов, 0 failures.**

## Что реализовано

### Sprint A: Provenance & Index Hardening ✅

#### A1. Structured Provenance Type
- **`src/models.rs`**: Добавлена структура `Provenance` с типизированными полями:
  `source_episode_id`, `source_url`, `ingestion_method`, `created_by`,
  `source_confidence`, `confidence_basis`, `extraction_strategy`, `source_type`, `source_id`
- Фабричные методы: `Provenance::manual()`, `Provenance::agent_observation()`,
  `Provenance::extraction()`
- Сериализация: `to_json_value()` / `from_json_value()` для совместимости с БД
- `Fact.provenance` и `Edge.provenance` изменены с `serde_json::Value` на `Provenance`
- **`src/service/core.rs`**: `add_fact()` принимает `Provenance`, `build_fact_index_keys()`
  и `collect_fact_source_references()` используют `&Provenance`
- **`src/service/episode/fact_extraction.rs`**: `add_extracted_fact()` → `Provenance::extraction()`
- **`src/service/episode/edges.rs`**: сериализация провенанса ребра
- **`src/service/episode/record_parsing.rs`**: `fact_from_record()` парсит `Provenance::from_json_value()`
- **`src/service/context/scoring.rs`**: `ranked_fact_to_item()` конвертирует `Provenance` → `serde_json::Value`
- **Все тестовые файлы** обновлены на новый тип провенанса
- **`migrations/022_structured_provenance.surql`**: определение подполей + backfill
- **`explain()`**: поверх `ingestion_method` и `extraction_strategy` из структурированного провенанса

#### A2. Edge Composite + Temporal Indexes
- **`migrations/023_edge_composite_indexes.surql`**: `edge_from_to_idx` + `edge_temporal_idx`

### Sprint B: Entity Intelligence ✅

#### B1. Fuzzy Entity Dedup
- **`src/service/entity_resolution.rs`**: `EntityResolver` с цепочкой:
  `exact lookup → alias lookup → prefix search + Levenshtein → create entity`
- `normalize_entity_name()`: NFKC + lowercase + whitespace collapse
- Юнит-тесты нормализации и similarity
- **`src/service/entity.rs`**: добавлены методы:
  `find_entity_id_by_name()`, `find_entity_id_by_alias()`, `find_entities_by_prefix()`,
  `add_alias_to_entity()`, `create_entity()`, `query_triples()`, `invalidate_triple_by_id()`,
  `execute_query()`
- **`Cargo.toml`**: `strsim = "0.11"`, `unicode-normalization = "0.1"`
- **Интеграция в `MemoryService::resolve()`**: `entity_resolver.resolve_or_create()` вызывается
  при каждом resolve, автоматически находя fuzzy-дубликаты и записывая алиасы

#### B2. Entity Resolution Test Suite
- Тесты: case normalization, whitespace, Cyrillic, NFKC, Levenshtein above/below threshold

### Sprint C: Structured Knowledge ✅

#### C1. Semantic Triple Extraction
- **`migrations/024_triples.surql`**: таблица `triple` + индексы
- **`src/service/triple_extractor.rs`**: трейт `TripleExtractor`, `NoOpTripleExtractor`,
  структура `SemanticTriple`, константы `SINGLETON_PREDICATES`
- **Fire-and-forget**: после `add_fact()` вызывается `spawn_triple_extraction()`,
  которая через `tokio::spawn` асинхронно извлекает тройки (no-op для NoOpExtractor)

#### C2. Conflict Resolution
- **`src/service/conflict_resolver.rs`**: `resolve_conflicts_for_triple()` —
  находит активные тройки с тем же (subject, predicate) но другим object
  и инвалидирует их через bi-temporal close
- **Интеграция**: вызывается из `spawn_triple_extraction()` для singleton-предикатов

### Sprint D: Quality of Life ✅

#### D1. Explain Enrichments
- **`src/models.rs`**: добавлены поля `fact_age_days`, `decayed_confidence`, `ingestion_method`
  в `ExplainItem`
- **`src/service/core.rs`**: `explain()` вычисляет:
  - `fact_age_days`: возраст факта от `t_valid` до `Utc::now()`
  - `decayed_confidence`: `confidence * 2^(-age / half_life)` с разделением
    на `METRIC_HALF_LIFE_DAYS` (365) и `DEFAULT_HALF_LIFE_DAYS` (180)
  - `ingestion_method`: из `Provenance.ingestion_method`

#### D2. Cyrillic FTS Analyzer
- **`migrations/025_cyrillic_fts.surql`**: `DEFINE ANALYZER memory_fts_ru`

#### D3. OpenAI-Compatible Embedder
- **Уже существовал** как `OpenAiCompatibleEmbeddingProvider` и `OllamaEmbeddingProvider`
  в `src/service/embedding/remote.rs`
- Поддержка OpenAI API, Azure OpenAI, Ollama, локальных vLLM
- Rate limiting с exponential backoff для 429

## Инфраструктурные изменения

### Builder (MemoryService)
- Добавлены поля: `entity_resolver`, `triple_extractor`
- Инициализируются в `build()` с дефолтными значениями
  (EntityResolver с порогом 0.85, NoOpTripleExtractor)

### Тесты исправлены
- `resolve_entity_by_type_delegates_to_resolve`: добавлен `apply_migrations()`
- `relate_creates_edge_between_entities`: добавлен `apply_migrations()`
- `open_app_inspector`: исправлен `string::startsWith` → `string::starts_with`
- Все `ExplainItem` конструкции обновлены новыми полями

## Миграции

| # | Файл | Назначение |
|---|------|-----------|
| 022 | `022_structured_provenance.surql` | Структурированный провенанс |
| 023 | `023_edge_composite_indexes.surql` | Составные индексы edge |
| 024 | `024_triples.surql` | Таблица семантических троек |
| 025 | `025_cyrillic_fts.surql` | Кириллический FTS анализатор |
