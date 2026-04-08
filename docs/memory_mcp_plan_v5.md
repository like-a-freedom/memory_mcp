# memory_mcp: Master Plan v5

> 2026-04-07 · Консолидация v2 + v3 + v4  
> Rust-native / embedded / KISS / intent-driven MCP / no external LLM / SurrealDB only

---

## Принципы (неизменны)

1. Zero new MCP tools — весь новый функционал через params/view_mode существующих 8 инструментов
2. Zero external LLM — детерминированная логика или local candle embeddings
3. Zero file exports — SurrealDB единственный store
4. Pure Rust deps only — каждая новая зависимость проверяется на C-free
5. Eval-first — написать/дополнить eval кейс → baseline → implement → measure → iterate

---

## Eval targets (цели)

| Метрика | Target |
|---|---|
| recall_at_5 (кастомный fixture) | ≥ 0.90 |
| recall_at_5 (LongMemEval) | ≥ 0.75 |
| pass_rate tier=direct | ≥ 0.95 |
| pass_rate tier=alias | ≥ 0.85 |
| pass_rate tier=temporal | ≥ 0.80 |
| pass_rate tier=graph | ≥ 0.70 |
| pass_rate tier=reasoning | ≥ 0.60 |
| fact_type_accuracy (extraction) | ≥ 0.85 |
| assemble p95 latency | ≤ 50ms |
| ingest p95 latency | ≤ 200ms |

---

## YAGNI — что НЕ делаем

| Идея | Причина |
|---|---|
| Новые MCP tools (list_projects, query_structured, wake_up, ingest_file, ingest_url) | Покрываются params/view_mode существующих |
| Файловый экспорт (wiki/, markdown export) | SurrealDB — единственный store |
| Внешние LLM (community summary, wiki synthesis, SummaryProvider) | Нет внешних LLM |
| leiden-rs community detection | C-биндинги; union-find на std::collections::HashMap достаточно |
| AAAK compression | Не bottleneck в Rust MCP |
| Night synthesis worker | Требует внешний LLM |
| Clipboard/IMAP/browser auto-ingest | YAGNI — только FS watcher |
| PPR graph retrieval | XL complexity |
| Session summarization (LLM) | Агент сам вызывает ingest с summary — паттерн, не код |
| Persona "soul" system (qwe-qwe) | view_mode="wake_up" достаточен |

---

## Новые зависимости (все pure Rust, 0 C-deps)

```toml
lopdf         = "0.40.0"           # PDF text extraction
roxmltree     = "0.21.1"           # OOXML XML walking
zip           = { version = "8.5.1", default-features = false, features = ["deflate"] }
notify        = { version = "8.2.0", optional = true }

[features]
default   = []
cli-watch = ["notify"]
```

> **Примечание:** first-pass document ingest реализован через `lopdf` + `zip`/`roxmltree` и lightweight local parsers для HTML/markdown/email. Follow-up с `DocumentParser`/`TextChunk` abstraction и word-based chunker уже доведён в `src/service/ingest/mod.rs` + `src/service/ingest/chunker.rs`.

---

## Спринт 0: Eval Foundation (2–3 дня, до любого кода)

**Цель:** baseline зафиксирован, tier distribution известна, ≥ 50 eval кейсов, инфраструктура повторяемых прогонов.

**Новая вводная (2026-04-07):**
- external retrieval evals ведём от source-oriented raw datasets, а не от legacy adapted fixtures
- canonical normalized schema + adapter registry держим в `tests/eval_support/external.rs`
- shortlist публичных dataset tracks и rationale зафиксированы в `docs/EVAL_DATASET_STRATEGY.md`

**Текущее состояние:**
- `tests/eval_retrieval.rs` — есть, 60 кейсов (`direct=15`, `alias=10`, `temporal=10`, `graph=15`, `reasoning=10`) + per-case `min_recall_at_k` assert; ignored runner фиксирует `as_of` после max(`t_valid`, runtime `t_ingested`), чтобы suite не зависел от wall-clock, и теперь жёстко валидирует `recall_at_5 ≥ 0.90` + per-tier pass-rate targets
- `tests/eval_external_retrieval.rs` — есть normalization tests + ignored smoke runners для `longmemeval-cleaned`, `locomo`, `personamem`, `prefeval`
- `tests/eval_external_full_datasets.rs` — есть bundle/wrap coverage + ignored official full-source loader checks для `longmemeval-cleaned`, `locomo`, `personamem`, `prefeval`
- `tests/eval_external_provenance.rs` — есть ignored upstream provenance verifier для `longmemeval-cleaned`, `locomo`, `personamem`, `prefeval`
- `tests/eval_support/external.rs` — есть canonical external adapter registry + `LongMemEvalCleaned` / `LoCoMo` / `PersonaMem` / `PrefEval` normalizers
- `tests/eval_support/external_full.rs` — есть sample/full loader, official cache/bundle plumbing, `PrefEval` wrapping и `PersonaMem` bundling из `questions_32k.csv` + `shared_contexts_32k.jsonl`
- `tests/eval_extraction.rs` — есть fixture-driven suite (9 кейсов) с contradiction warnings, `experience` и document-style action-item coverage
- `tests/eval_latency.rs` — есть in-memory latency suite с `ingest_p50/p95` и `assemble_p50/p95`, плюс плановые threshold asserts (`ingest_p95 ≤ 200ms`, `assemble_p95 ≤ 50ms`)
- `Makefile` — есть `eval-baseline`, `eval-quick`, `eval-compare` (verified via `make eval-compare`)
- `tests/fixtures/evals/retrieval_cases.json` — 60 кейсов; coverage snapshot: `direct=15`, `alias=10`, `temporal=10`, `graph=15`, `reasoning=10`
- `tests/fixtures/evals/raw/longmemeval/sample_longmemeval_s_cleaned.json` — trimmed official excerpt из `xiaowu0162/longmemeval-cleaned`
- `tests/fixtures/evals/raw/locomo/sample_locomo10.json` — trimmed official excerpt из `snap-research/locomo`
- `tests/fixtures/evals/raw/personamem/sample_personamem_32k.json` — paired official excerpt: `questions_32k.csv` row + `shared_contexts_32k.jsonl` context slice
- `tests/fixtures/evals/raw/prefeval/sample_travel_hotel_implicit_persona.json` — trimmed official excerpt из `amazon-science/PrefEval` (`simcse_implicit_persona` track)
- `tests/fixtures/evals/full/` — ignored cache для full official datasets; sample fixtures остаются tiny smoke inputs, а full runners читают upstream-backed cache
- `tests/fixtures/evals/raw/README.md` — объясняет, что локальные raw fixtures intentionally tiny и как их перепроверить against upstream
- `tests/fixtures/evals/extraction_cases.json` — 9 кейсов: metric/promise baseline + contradiction warnings + `experience` + document-style action items
- `docs/EVAL_DATASET_STRATEGY.md` — есть research note по LongMemEval / LoCoMo / PersonaMem / PrefEval
- `docs/EVAL_BASELINE.md` — baseline синхронизирован с graph bridge coverage: кастомный retrieval now reports `actual_tier=direct total=31`, `actual_tier=temporal total=15`, `actual_tier=graph total=14`
- official `PersonaMem` full source now normalizes mixed `all_options` encodings (JSON arrays + Python-style quoted lists); current upstream split yields 589 normalized cases, so loader assertions target “hundreds of cases”, not a synthetic 1000+
- `retrieval_tier` + enriched `rationale` теперь протянуты для `direct` / `alias` / `temporal` / `graph` / `semantic` / `fallback`; temporal path promotes explicit temporal-marker intersections (month-year, weekdays, quarter/date phrases) to `TemporalExpanded`

| # | Шаг | Файл | Результат | Статус |
|---|---|---|---|---|
| 0.1 | Создать source-oriented adapter registry + canonical external retrieval schema; first slice = `LongMemEval-cleaned` normalizer и skeleton runner | `tests/eval_support/external.rs`, `tests/eval_external_retrieval.rs`, `tests/fixtures/evals/raw/longmemeval/` | Внешний raw dataset нормализуется в наш eval-формат; smoke-runner проходит | [done] |
| 0.2 | Добавить `LoCoMo` adapter в тот же registry | `tests/eval_support/external.rs`, `tests/fixtures/evals/raw/locomo/` | Второй публичный retrieval benchmark нормализуется тем же pipeline | [done] |
| 0.3 | Добавить secondary adapters/tracks для `PersonaMem` и `PrefEval`; `MemoryAgentBench` пересмотреть только после source-fit review | `tests/eval_support/external.rs`, `tests/fixtures/evals/raw/personamem/`, `tests/fixtures/evals/raw/prefeval/`, `tests/eval_support/external_full.rs`, `tests/eval_external_full_datasets.rs` | Персонализация/профиль покрыты source-backed adapters + sample/full loaders + ignored smoke/full runners | [done] |
| 0.4 | Создать отсутствующие suite `tests/eval_extraction.rs` и `tests/eval_latency.rs` | `tests/eval_extraction.rs`, `tests/eval_latency.rs` | Все 4 sprint-0 suite существуют в репо | [done] |
| 0.5 | Запустить доступные сейчас suite (`eval_retrieval`, `eval_external_retrieval`) и зафиксировать console baseline | — | Есть числа для кастомного retrieval и external skeleton | [done] |
| 0.6 | Записать результаты в `docs/EVAL_BASELINE.md` (шаблон ниже) | `docs/EVAL_BASELINE.md` | Baseline для retrieval / external / extraction / latency зафиксирован | [done] |
| 0.7 | Подсчитать кейсы в `retrieval_cases.json` по тирам (`jq '[.[] | .expected.tier] \| group_by(.) \| map({(.[0]): length}) \| add' ...`) | — | Таблица покрытия | [done] |
| 0.8 | Добавить `RetrievalTier` enum в `src/service/context.rs`; `retrieval_tier: Option<String>` в `AssembledContextItem` (models.rs) | `src/models.rs`, `src/service/context.rs` | Поле в API доступно | [done] |
| 0.9 | Пронести tier через текущий pipeline: `Direct/AliasExpanded/GraphExpanded/SemanticExpanded/EpisodeFallback` | `src/service/context.rs` | `retrieval_tier` заполняется на активных путях retrieval, включая term fallback | [done] |
| 0.10 | Довести tier wiring до `TemporalExpanded` при реализации этой ветки | `src/service/context.rs` | Explicit temporal marker expansion/intersection promotes temporal matches to `retrieval_tier="temporal"` | [done] |
| 0.11 | Обновить `rationale`: `"tier=direct fts=0.87 access_count=12 confidence=0.91"` | `src/service/context.rs` | Информативный rationale уже приходит в API | [done] |
| 0.12 | Добавить `actual_tiers: &[&str]` в `record_retrieval_case`; `actual_tier_totals` в `RetrievalSuiteSummary`; вывод в `print_retrieval_summary` | `tests/eval_support/` | Видно реальный путь каждого факта | [done] |
| 0.13 | Прогнать retrieval eval заново — зафиксировать tier distribution | — | Есть реальный actual-tier output для кастомного retrieval | [done] |
| 0.14 | Добавить ≥ 10 кейсов на каждый tier (direct/alias/temporal/graph/reasoning) в `retrieval_cases.json` | `tests/fixtures/evals/retrieval_cases.json` | ≥ 50 кейсов итого | [done] |
| 0.15 | Добавить assert по `min_recall_at_k` per-case в `eval_retrieval.rs` | `tests/eval_retrieval.rs` | Per-case контроль | [done] |
| 0.16 | Создать `Makefile` с целями `eval-baseline`, `eval-quick`, `eval-compare` | `Makefile` | Repeatability | [done] |

**Шаблон EVAL_BASELINE.md:**
```
# Eval Baseline — YYYY-MM-DD

## Retrieval (кастомный)
suite=eval_retrieval total=? passed=? recall_at_5=? precision_at_5=? mrr=?
tier=direct    total=? passed=? recall_at_5=? pass_rate=?
tier=alias     ...
tier=temporal  ...
tier=graph     ...
tier=reasoning ...

## LongMemEval
suite=longmemeval total=? passed=? recall_at_5=?

## Extraction
suite=eval_extraction entity_precision=? entity_recall=? entity_f1=? fact_type_accuracy=?

## Latency (in-memory)
ingest_p50_ms=? ingest_p95_ms=? assemble_p50_ms=? assemble_p95_ms=?
```

**Целевые команды прогона (после 0.4):**
```bash
cargo test --test eval_retrieval run_retrieval_evals -- --ignored --nocapture --test-threads=1
cargo test --test eval_external_retrieval run_longmemeval_retrieval -- --ignored --nocapture --test-threads=1
cargo test --test eval_extraction run_extraction_evals -- --ignored --nocapture --test-threads=1
cargo test --test eval_latency run_latency_evals -- --ignored --nocapture --test-threads=1
```

---

## Спринт 1: Document Ingest Pipeline (1.5 недели)

**Цель:** `ingest` работает для PDF/DOCX/XLSX/PPTX/MD/TXT/EML через существующий ingest tool.

> **Важно:** `source_type` уже используется семантически ("email", "conversation", "document", "tfs_work_item"). Для document ingest через файлы используем существующий `source_type="document"` + новый `content` = извлечённый текст. Файловый ввод — это transport-level dispatch в `MemoryService::ingest`, не новый source_type.

### Eval-шаги

| # | Шаг | Файл | Результат | Статус |
|---|---|---|---|---|
| 1.E1 | Создать `tests/fixtures/docs/`: `sample.pdf`, `sample.docx`, `sample.xlsx`, `sample.pptx`, `sample.md`, `sample.eml`, `sample.html` — фиксированные документы/HTML с известным контентом | `tests/fixtures/docs/` | Тестовые документы и loopback URL fixture | [done] |
| 1.E2 | Написать `tests/eval_document_ingest.rs` — кейсы: ingest file/url/dir → assemble_context → must_contain известную фразу | `tests/eval_document_ingest.rs` | `#[ignore]`, red/green suite для Sprint 1 | [done] |
| 1.E3 | Прогнать baseline → убедиться что suite fail до episode/ingest fallback wiring | — | Baseline captured before green phase | [done] |

### Имплементация

| # | Шаг | Файл | Детали | Статус |
|---|---|---|---|---|
| 1.1 | `lopdf`, `zip`, `roxmltree` → `Cargo.toml` | `Cargo.toml` | pure-Rust parser stack for PDF + OOXML | [done] |
| 1.2 | `DocumentParser` trait + `TextChunk` struct в `src/service/ingest/mod.rs` | `src/service/ingest/mod.rs` | `can_handle(ext) -> bool`, `parse(bytes) -> Result<Vec<TextChunk>>` | [done] |
| 1.3 | `PdfParser` (`lopdf`): text extraction from PDF bytes | `src/service/ingest/pdf.rs` | fixture phrase recovery | [done] |
| 1.4 | `OfficeParser` (`zip` + `roxmltree`): DOCX/XLSX/PPTX text extraction | `src/service/ingest/office.rs` | detect by extension + OOXML XML walking | [done] |
| 1.5 | `MarkdownParser` + `PlainTextParser` | `src/service/ingest/text.rs` | normalized text extraction | [done] |
| 1.6 | `EmailParser`: headers + body | `src/service/ingest/email.rs` | lightweight RFC822-style parsing | [done] |
| 1.7 | `detect_format(path)` по extension + magic bytes | `src/service/ingest/mod.rs` | shared ingest dispatch helper | [done] |
| 1.8 | `Chunker`: 400 слов, overlap 50 слов, `source_id = sha256(canonical_path)` для dedup | `src/service/ingest/chunker.rs` | | [done] |
| 1.9 | Dispatch в `MemoryService::ingest`: file/url/dir → parse/fetch/aggregate → persist через существующий ingest pipeline | `src/service/core.rs`, `src/service/ingest/mod.rs` | Существующий `ingest` tool без API-изменений | [done] |
| 1.10 | URL ingest → `reqwest::get` → strip HTML → ingest | `src/service/ingest/mod.rs`, `src/service/core.rs` | loopback-tested with local HTML fixture | [done] |
| 1.11 | Dir ingest → `std::fs::read_dir` recursive → skip non-supported ext → aggregate supported files into one ingested episode | `src/service/ingest/mod.rs`, `src/service/core.rs` | std only | [done] |
| 1.12 | Unit tests для parser/helpers: fixture phrases + recursive dir + HTML strip | `src/service/ingest/mod.rs`, `src/service/context.rs` | `#[cfg(test)]` | [done] |

### Eval-проверка после имплементации

| # | Шаг | Target | Статус |
|---|---|---|---|
| 1.E4 | Прогнать `eval_document_ingest` | ≥ 80% кейсов pass | [done] |
| 1.E5 | Прогнать основной retrieval eval | recall_at_5 ≥ baseline (нет регрессий) | [done] |

---

## Спринт 2: Project Filter + view_mode (3–4 дня)

**Цель:** project-filter изолирует факты; `view_mode="facets"|"wake_up"` работают.

> **Изменение:** `view_mode="map"` перенесён в спринт 3 — зависит от community detection.

### Eval-шаги

| # | Шаг | Файл | Результат | Статус |
|---|---|---|---|---|
| 2.E1 | Добавить ≥ 5 project-filter кейсов в `retrieval_cases.json`: два проекта с разными датами, query с `project` фильтром, `must_not_contain` факты из другого проекта | `tests/fixtures/evals/retrieval_cases.json` | Кейсы в fixture | [done] |
| 2.E2 | Зафиксировать red phase до green-имплементации | — | Sprint 2 red phase подтверждена integration tests/compile failures до завершения wiring | [done] |

### Имплементация

| # | Шаг | Файл | Детали | Статус |
|---|---|---|---|---|
| 2.1 | Миграция `016_project_tag.surql`: `DEFINE FIELD project ON episode TYPE option<string>; DEFINE FIELD project ON fact TYPE option<string>; DEFINE INDEX ...` | `src/migrations/016_project_tag.surql` | | [done] |
| 2.2 | `project: Option<String>` в `IngestRequest` + `EpisodeInput` | `src/models.rs` | | [done] |
| 2.3 | `project: Option<String>` в `IngestParams` | `src/mcp/params.rs` | MCP параметр для ingest tool | [done] |
| 2.4 | `project: Option<String>` + `fact_types: Vec<String>` в `AssembleContextRequest` | `src/models.rs` | | [done] |
| 2.5 | `project: Option<String>` + `fact_types: Vec<String>` в `AssembleContextParams` | `src/mcp/params.rs` | MCP параметры для assemble_context tool | [done] |
| 2.6 | `WHERE project = $project` (условный) в `select_facts_filtered` | `src/storage.rs` | | [done] |
| 2.7 | `WHERE project = $project` в `select_episodes_by_content` | `src/storage.rs` | | [done] |
| 2.8 | `WHERE fact_type IN $fact_types` (условный) в `select_facts_filtered` | `src/storage.rs` | | [done] |
| 2.9 | view_mode `"facets"`: `SELECT (project ?? first(policy_tags) ?? scope), count(*), max(t_ingested) FROM episode GROUP BY ... ORDER BY max(t_ingested) DESC` — generic агрегация без domain-specifics | `src/service/context.rs` | | [done] |
| 2.10 | view_mode `"wake_up"`: persona facts (`policy_tags CONTAINS "persona"`, низкий decay) + recent N facts по `t_ingested DESC` | `src/service/context.rs` | Аналог L0+L1 из mempalace | [done] |

### Eval-проверка

| # | Шаг | Target | Статус |
|---|---|---|---|
| 2.E3 | Прогнать project-filter кейсы | ≥ 95% pass | [done] |
| 2.E4 | Полный retrieval eval | recall_at_5 ≥ baseline | [done] |

---

## Спринт 3: Graph Quality (1 неделя)

**Цель:** EdgeOrigin в модели, community detection, hub entities, surprising connections в explain(), view_mode="map".

### Eval-шаги

| # | Шаг | Файл | Результат | Статус |
|---|---|---|---|---|
| 3.E1 | Добавить ≥ 5 кейсов tier=graph с явным multi-hop: episode A→entity→episode B, query → must_contain из B | fixtures | Community-bridge graph fixtures `ret-056..ret-060` added | [done] |
| 3.E2 | Прогнать → зафиксировать graph tier pass_rate baseline | — | `docs/EVAL_BASELINE.md`: `expected_tier=graph total=15`, `actual_tier=graph total=14`, `pass_rate=1.00` | [done] |

### Имплементация

| # | Шаг | Файл | Детали | Статус |
|---|---|---|---|---|
| 3.1 | Миграция `017_edge_origin.surql`: `DEFINE FIELD origin ON edge TYPE string DEFAULT 'extracted'` | `src/migrations/017_edge_origin.surql` | | [done] |
| 3.2 | `EdgeOrigin` enum (`Extracted/Inferred/Ambiguous`) + `pub origin: EdgeOrigin` в `Edge` struct | `src/models.rs` | `#[serde(default)]` | [done] |
| 3.3 | В `build_ranked_context_facts`: weight для community/graph слоя умножать на `edge.origin` factor: Extracted=1.0, Inferred=edge.confidence, Ambiguous=0.5 | `src/service/context.rs` | Реализовано через origin-aware graph weighting: community layer uses active neighbor edges for matched entities and applies Extracted=1.0 / Inferred=`edge.confidence` / Ambiguous=0.5 before fusion ranking | [done] |
| 3.4 | ❌ Убрано — petgraph не нужен; connected_components заменяется union-find на std | — | YAGNI; removed from scope, no implementation required | [done] |
| 3.5 | `communities.rs`: загрузить все активные edges через `select_edges_filtered` → union-find на `std::collections::HashMap` → upsert в таблицу `community` | `src/service/lifecycle/communities.rs` | Периодический recompute pass добавлен; в коде зафиксирован комментарий про hard limit `select_edges_filtered` = 10K | [done] |
| 3.6 | Детерминированный summary для community: `"{top3_entity_names} (+N more)"` без LLM | `communities.rs` | Condensed summary format now reused by periodic rebuild and incremental community updates | [done] |
| 3.7 | Вызвать `build_communities()` из lifecycle worker по триггеру (run_lifecycle_jobs) | `src/service/lifecycle/mod.rs` | Реализовано через `spawn_community_worker` при lifecycle startup (на archival cadence) | [done] |
| 3.8 | `find_hub_entities(db, ns, limit) -> Vec<HubEntity>`: SurrealQL `SELECT id, canonical_name, count(<-e) + count(e->) AS degree FROM entity ORDER BY degree DESC LIMIT $limit` | `src/service/apps/graph.rs` | `find_hub_entities(...)` added in `src/service/apps/graph.rs`; current implementation ranks entities by active incoming/outgoing neighbor degree and feeds `view_mode="map"` | [done] |
| 3.9 | `find_surprising_connections(db, ns, source_entity, max_depth) -> Vec<SurprisingConnection>`: BFS глубиной 2–3, фильтр — target entity принадлежит другому community | `src/service/apps/graph.rs` | Реализовано bounded BFS по active neighbors c cross-community фильтром и кратчайшим path summary | [done] |
| 3.10 | `graph_insights: Option<GraphInsights>` в response `explain()` — не новый tool: hub_entities + surprising_connections для entity из context_pack | `src/mcp/handlers.rs` | Реализовано backward-compatible через optional `ExplainItem.graph_insights` без смены MCP envelope | [done] |
| 3.11 | view_mode `"map"`: top hub entities (degree query) + community list из таблицы community | `src/service/context.rs` | Реализовано через `build_map_view(...)`: hub entities serialизуются как `kind="hub_entity"`, communities как `kind="community"` | [done] |

### Eval-проверка

| # | Шаг | Target | Статус |
|---|---|---|---|
| 3.E3 | Прогнать graph tier eval | ≥ 70% pass_rate; latest run keeps `expected_tier=graph total=15`, `pass_rate=1.00` | [done] |
| 3.E4 | Полный retrieval eval | recall_at_5 ≥ baseline; latest run `suite=eval_retrieval total=60 passed=60 recall_at_5=1.00` | [done] |

---

## Спринт 4: Retrieval Quality + Temporal Expansion (3–5 дней)

**Цель:** temporal tier ≥ 0.80, contradiction detection при ingest, `experience` fact_type.

> **Примечание:** `expand_temporal_synonyms` уже существует в `src/service/context.rs` — нужно расширение, не создание с нуля.

### Eval-шаги

| # | Шаг | Файл | Результат | Статус |
|---|---|---|---|---|
| 4.E1 | Прогнать temporal tier eval отдельно: зафиксировать текущий pass_rate | — | `run_retrieval_evals`: `expected_tier=temporal total=10`, suite stays `passed=60/60`, `actual_tier=temporal total=15` | [done] |
| 4.E2 | Добавить ≥ 5 кейсов с contradiction: ingest A, ingest B (противоречит A), `extract()` → `warnings` в ответе | `tests/fixtures/evals/extraction_cases.json`, `tests/eval_extraction.rs` | Fixture-driven extraction eval now seeds prior episodes, expects exact `warnings`, and covers 5 contradiction cases across metric/promise flows | [done] |

### Имплементация

| # | Шаг | Файл | Детали | Статус |
|---|---|---|---|---|
| 4.1 | Расширить `expand_temporal_synonyms`: `"this week"` → Mon-Sun текущей недели; `"yesterday"` → конкретная дата; `"last quarter"/"Q1..Q4"` → диапазон месяцев | `src/service/context.rs` | Existing helper extended to emit current-week date markers and quarter month ranges; relative-day concrete-date behavior remains covered via `day_group_queries(...)` | [done] |
| 4.2 | `"Monday".."Sunday"` → конкретная дата относительно `as_of` | `src/service/context.rs` | Реализовано через `weekday_group(cutoff, token)` + week anchoring from request `as_of` | [done] |
| 4.3 | Верифицировать `record_fact_access` вызывается для всех результатов (cache hit + fresh) — уже есть в коде, добавить тест | `src/service/context.rs` / тест | Integration coverage added for fresh retrieval + cache hit; repeated `assemble_context` now proves `access_count` increments from 1 to 2 on the same fact | [done] |
| 4.4 | Contradiction detection при ingest: после extraction найти факты с тем же subject+predicate и другим object → добавить `warnings: Vec<ContradictionWarning>` в `ExtractResult` | `src/service/episode.rs`, `src/models.rs`, `src/mcp/handlers.rs`, `tests/service_integration.rs` | Реализовано как deterministic potential-contradiction warning: same `fact_type` + meaningful `entity_links` overlap + different content; MCP schema и integration coverage добавлены, ingest не блокируется | [done] |
| 4.5 | `experience` fact_type: зарезервировать как стандартный тип (добавить в enum/константы fact_types); при `assemble_context` автоматически включать recent `experience` факты в результат (низкий приоритет, но всегда) | `src/models.rs`, `src/service/context.rs`, `tests/service_integration.rs` | Standard fact-type constants now reserve `experience`; default/timeline `assemble_context` appends recent active experience facts as low-priority supplements unless caller explicitly narrows `fact_types` away from `experience` | [done] |

### Extraction eval

| # | Шаг | Файл | Детали | Статус |
|---|---|---|---|---|
| 4.6 | Добавить extraction кейсы для `experience` fact_type в `extraction_cases.json` | `tests/fixtures/evals/extraction_cases.json`, `src/service/episode.rs`, `tests/eval_extraction.rs` | Added fixture coverage plus deterministic `prefer/prefers/...` heuristic so preference statements now extract `experience` facts | [done] |
| 4.7 | Добавить extraction кейсы для document-style контента (action items из email) | `tests/fixtures/evals/extraction_cases.json`, `src/service/episode.rs`, `tests/eval_extraction.rs` | Added email-style action-item fixture plus deterministic header+bullet heuristic so structured action lists extract as `promise` facts | [done] |

### Ужесточить assertion targets

| # | Шаг | Файл | Детали | Статус |
|---|---|---|---|---|
| 4.8 | Ужесточить global assert в `eval_retrieval.rs`: recall_at_5 ≥ 0.90 + per-tier targets | `tests/eval_retrieval.rs`, `tests/eval_support/metrics.rs`, `tests/eval_support/report.rs` | `run_retrieval_evals` now asserts global recall target plus per-tier pass-rate thresholds using expected-tier pass accounting/reporting | [done] |
| 4.9 | Добавить latency assert в `eval_latency.rs`: assemble_p95 ≤ 50ms, ingest_p95 ≤ 200ms | `tests/eval_latency.rs` | `run_latency_evals` now enforces plan latency thresholds directly after percentile calculation | [done] |

### Eval-проверка

| # | Шаг | Target | Статус |
|---|---|---|---|
| 4.E3 | Temporal tier eval | ≥ 80% pass_rate; latest retrieval eval remains `passed=60/60`, so temporal cases stay green | [done] |
| 4.E4 | LongMemEval | `run_longmemeval_retrieval`: `total=1 passed=1 recall_at_5=1.00 pass_rate=1.00` on current sample track | [done] |
| 4.E5 | Latency eval | `run_latency_evals`: `ingest_p50=0.41ms ingest_p95=2.90ms assemble_p50=4.14ms assemble_p95=12.72ms` | [done] |
| 4.E6 | Extraction eval | `run_extraction_evals`: `total=9 passed=9 entity_precision=0.57 entity_recall=1.00 entity_f1=0.73 fact_type_accuracy=1.00 warning_recall=1.00` | [done] |

---

## Спринт 5: FS Watcher + Hooks (2–3 дня)

**Цель:** `memory-mcp watch <dir>` auto-ingest при изменении файлов; Claude Code hooks в репо.

> **Примечание:** `src/main.rs` уже существует как stdio entry point — нужно расширить его subcommand-ом `watch`, а не создавать заново.

| # | Шаг | Файл | Детали | Статус |
|---|---|---|---|---|
| 5.1 | `notify = { version = "8.2.0", optional = true }` + feature `cli-watch` в `Cargo.toml` | `Cargo.toml`, `Cargo.lock` | Optional watcher dependency wired behind `cli-watch`, so the default library/embedded build stays unchanged | [done] |
| 5.2 | `src/service/ingest/watcher.rs`: `FsWatcher::run(dir, project, scope, service)` — notify event loop → `Create/Modify` → detect format → ingest (использует парсеры из спринта 1) | `src/service/ingest/watcher.rs`, `src/service/ingest/mod.rs`, `src/service/mod.rs` | Feature-gated watcher now filters supported create/modify events, maps `.eml` to `source_type=email`, and rate-limits repeated ingests per file via `--interval` | [done] |
| 5.3 | Subcommand `watch` в `src/main.rs`: расширить существующий CLI — добавить `memory-mcp watch <dir> [--project <name>] [--scope <scope>] [--interval <secs>]` как альтернативу stdio serve | `src/main.rs` | CLI parsing + tests now dispatch `serve` vs `watch`; `watch` starts `FsWatcher` when built with `--features cli-watch` and returns a clear feature error otherwise | [done] |
| 5.4 | `hooks/README.md`: инструкция для Claude Code / Cursor / Continue — как настроить Stop и PreCompact hooks | `hooks/README.md` | README now covers Claude Code native hooks, Cursor beta hooks, Continue fallback automation, env overrides, and local embedded defaults | [done] |
| 5.5 | `hooks/memory_stop_hook.sh`: curl через stdio MCP или HTTP endpoint → `ingest(source_type="session_summary", ...)` | `hooks/memory_stop_hook.sh` | Implemented as a deterministic shell+Python stdio MCP client: `initialize` → `notifications/initialized` → `tools/call(ingest)` using explicit content, transcript excerpts, or raw hook payload JSON | [done] |
| 5.6 | `hooks/memory_precompact_hook.sh`: аналогично для PreCompact — emergency save | `hooks/memory_precompact_hook.sh` | Mirrors the stop hook path with precompact-specific policy tags and transcript/custom-instruction capture for emergency saves before compaction | [done] |
| 5.E1 | Ручной тест: `memory-mcp watch /tmp/test-dir --project test`, скопировать sample.pdf → убедиться что episode появился через `assemble_context` | — | Verified end-to-end with `cargo run --features cli-watch -- watch /tmp/memory-mcp-watch-smoke --project watch-smoke --scope org --interval 1`, copying `tests/fixtures/docs/sample.md`, observing `op=ingest`, then confirming `assemble_context` returned the watched `Maple markdown action item` episode from the embedded DB | [done] |

---

## Итоговая карта задач по файлам

### Новые файлы

```
src/service/ingest/
├── mod.rs           (DocumentParser trait, TextChunk, parser registry, detect_format, ingest dispatch)
├── pdf.rs           (`lopdf` extraction)
├── office.rs        (`zip` + `roxmltree` extraction)
├── text.rs          (lightweight markdown/plain-text extraction)
├── email.rs         (lightweight email extraction)
├── chunker.rs       (400-word chunks, overlap 50, stable transport dedup)
└── watcher.rs       (FsWatcher / notify-backed optional watch loop)

src/service/lifecycle/
├── mod.rs           (lifecycle worker dispatcher — уже есть)
├── decay.rs         (decay computation — уже есть)
├── archival.rs      (archival logic — уже есть)
└── communities.rs   (build_communities, union-find на std — новый)

src/migrations/
├── 016_project_tag.surql
└── 017_edge_origin.surql

src/
└── main.rs          (CLI entry point — existing stdio server + watch subcommand)

.env                (local embedded defaults for manual `cargo run` / `watch` / hook flows)

tests/
├── eval_external_retrieval.rs
├── eval_document_ingest.rs
├── eval_support/
│   └── external.rs
└── fixtures/
    ├── docs/
    │   ├── sample.pdf
    │   ├── sample.docx
    │   ├── sample.xlsx
    │   ├── sample.pptx
    │   ├── sample.md
    │   ├── sample.eml
    │   └── sample.html
    └── evals/
        ├── raw/
        │   ├── longmemeval/
        │   │   └── sample_longmemeval_s_cleaned.json
        │   ├── locomo/
        │   │   └── sample_locomo10.json
        │   ├── prefeval/
        │   └── personamem/
        ├── extraction_cases.json
        └── retrieval_document_ingest.json

docs/
├── EVAL_BASELINE.md
└── EVAL_DATASET_STRATEGY.md

hooks/
├── README.md
├── memory_stop_hook.sh
└── memory_precompact_hook.sh

Makefile
```

### Изменённые существующие файлы

```
src/models.rs
  + RetrievalTier enum (или в context.rs)
  + AssembledContextItem.retrieval_tier: Option<String>
  + IngestRequest.project: Option<String>
  + AssembleContextRequest.project: Option<String>
  + AssembleContextRequest.fact_types: Vec<String>
  + Edge.origin: EdgeOrigin
  + ExtractResult.warnings: Vec<ContradictionWarning>  [спринт 4]
  + ContradictionWarning struct                         [спринт 4]

src/mcp/params.rs
  + IngestParams.project: Option<String>               [спринт 2]
  + AssembleContextParams.project: Option<String>      [спринт 2]
  + AssembleContextParams.fact_types: Vec<String>      [спринт 2]

src/service/context.rs
  + RetrievalTier enum
  + tier tagging через весь pipeline (direct/alias/temporal/graph/semantic/fallback)
  + build_rationale() с tier + fts_score + access_count
  + view_mode "facets" / "wake_up" / "map"
  + project/fact_types WHERE условия (через storage)
  + expand_temporal_synonyms: week/yesterday/quarter/weekdays (расширение существующей)
  + experience facts автовключение в результат

src/storage.rs
  + select_facts_filtered: условный WHERE project, WHERE fact_type IN
  + select_episodes_by_content: условный WHERE project
  + select_active_edges() для communities.rs

src/service/core.rs
  + document ingest dispatch: file/url/dir → detect/fetch → parse/aggregate → ingest

src/service/mod.rs
  + `FsWatcher` re-export behind `cli-watch`

src/service/ingest/mod.rs
  + feature-gated `watcher` module wiring

src/service/context.rs
  + episode-content fallback in assemble_context for raw ingested documents

src/service/apps/graph.rs
  + find_hub_entities()
  + find_surprising_connections()

src/service/lifecycle/mod.rs
  + вызов build_communities() в lifecycle job

src/service/entity_extraction.rs
  + contradiction detection после extraction  [спринт 4]

src/mcp/handlers.rs
  + graph_insights в explain() response

Cargo.toml
  + pdf-extract, undoc, mailparse, pulldown-cmark
  + notify (optional, feature cli-watch)

src/main.rs
  + `serve` vs `watch` CLI parsing and dispatch
  + watch-argument validation tests

.env
  + local embedded SurrealDB defaults for manual server/watch usage

tests/common/mod.rs
  + seed_fact_with_links()
  + seed_entity()
  + seed_community()

tests/eval_support/metrics.rs
  + actual_tier_totals в RetrievalSuiteSummary
  + actual_tiers параметр в record_retrieval_case
  + expected-tier passed counts + pass-rate helpers for plan thresholds

tests/eval_support/external.rs
  + canonical external retrieval schema
  + source-oriented dataset registry
  + LongMemEval-cleaned / LoCoMo normalizers

tests/eval_support/report.rs
  + actual tier distribution в print_retrieval_summary
  + tier promotion analysis
  + expected-tier pass-rate rendering (`total/passed/pass_rate`)

tests/eval_retrieval.rs
  + actual_tiers передаются из AssembledContextItem.retrieval_tier
  + fixture seeding для `entity_links`, `entities`, `communities`
  + per-case min_recall_at_k assert
  + global recall_at_5 + per-tier threshold asserts

tests/eval_extraction.rs
  + fixture-driven extraction metrics (`entity_precision`, `entity_recall`, `entity_f1`, `fact_type_accuracy`, `warning_recall`)
  + multi-episode setup + exact contradiction warning expectations

tests/eval_latency.rs
  + in-memory latency measurement (`ingest_p50/p95`, `assemble_p50/p95`)
  + plan threshold asserts for ingest/assemble p95

tests/fixtures/evals/retrieval_cases.json
  + ≥ 10 кейсов per tier (direct/alias/temporal/graph/reasoning)
  + ≥ 5 project-filter кейсов

tests/fixtures/evals/extraction_cases.json
  + contradiction warning кейсы
  + experience fact_type кейсы
  + document-style контент кейсы
```

---

## Сводная таблица: все задачи

| Сп | # | Задача | Приоритет | Статус |
|---|---|---|---|---|
| 0 | 0.1–0.16 | Eval foundation: source-oriented adapters, baseline, tier infrastructure, fixture расширение, Makefile | P0 | [done] |
| 1 | 1.E1–1.E5, 1.1–1.12 | Document ingest: PDF/DOCX/XLSX/PPTX/MD/EML/URL/dir через ingest tool | P0 | [done] |
| 2 | 2.E1–2.E4, 2.1–2.10 | Project filter + view_mode facets/wake_up | P0 | [done] |
| 3 | 3.E1–3.E4, 3.1–3.11 | Graph: EdgeOrigin, community detection (union-find std), hub entities, surprising connections, graph_insights в explain, view_mode="map" | P1 | [done] |
| 4 | 4.E1–4.E6, 4.1–4.9 | Retrieval quality: temporal expansion, contradiction detection, experience fact_type, assert targets | P1 | [done] |
| 5 | 5.1–5.6, 5.E1 | FS watcher (notify, optional feature) + Claude Code hooks + CLI entry point | P1 | [done] |

**Бэклог (P3, не в спринтах):**
- `source_type="file"` для исходного кода `.rs/.ts/.py` через tree-sitter AST extraction
- [done] Query analytics: `query_log` таблица с tier, latency, result_count (best-effort logging для fresh/cache-hit `assemble_context`)
- petgraph 0.7 — если в будущем понадобятся weighted shortest path, betweenness centrality, MST (сейчас union-find на std покрывает connected components)

---

## Ключевые изменения относительно исходного плана

| # | Что было | Что стало | Причина |
|---|---|---|---|
| 1 | `oxidize-pdf` | `lopdf = "0.40.0"` | pure-Rust PDF extraction без C-deps |
| 2 | `undoc = "..."` | `zip = "8.5.1"` + `roxmltree = "0.21.1"` | lightweight OOXML extraction без отдельного heavy parser layer |
| 3 | `mailparse` / `pulldown-cmark` shortlist | lightweight local email/markdown parsers | first pass держит dependency surface меньше |
| 4 | `notify = "6"` | `notify = "8.2.0"` | Стабильная версия 8.x; 9.0.0-rc.2 — RC |
| 5 | Жёсткая конвертация legacy adapted fixtures через один `scripts/convert_external_evals.py` | Source-oriented raw datasets + adapter registry в `tests/eval_support/external.rs` | Новая вводная: старые адаптированные датасеты не источник истины; нужен устойчивый слой нормализации |
| 6 | `view_mode="map"` в спринте 2 | Перенесён в спринт 3 | Зависит от community detection |
| 7 | `source_type="file"` | Document ingest dispatch без нового source_type | source_type уже семантический ("email", "conversation") |
| 8 | Нет `IngestParams.project` | Добавлено в params.rs | MCP параметр нужен для project filter |
| 9 | Нет `AssembleContextParams.project/fact_types` | Добавлено в params.rs | MCP параметры нужны для фильтрации |
| 10 | `src/main.rs` не существует | Расширить существующий | Файл есть (57 строк, stdio MCP server), нужен subcommand dispatch |
| 11 | `src/service/lifecycle/` не существует | Добавить только `communities.rs` | Директория есть (mod.rs, decay.rs, archival.rs) |
| 12 | `expand_temporal_synonyms` с нуля | Расширение существующей функции | Уже реализована базовая версия |
| 13 | `petgraph = "0.7"` | Убран, union-find на std | Единственный use case — connected_components; ~25 строк на std vs ~150K строк крейта |
| 14 | `tests/eval_external_retrieval.rs`, `tests/eval_extraction.rs`, `tests/eval_latency.rs` уже есть | В репо есть только `tests/eval_external_retrieval.rs`; extraction/latency suites стали явными шагами 0.4 | План должен отражать реальное состояние репозитория, а не желаемое |
