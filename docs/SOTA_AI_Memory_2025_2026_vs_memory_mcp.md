# SOTA AI Memory 2025–2026 vs memory_mcp: Анализ и план действий

## Введение: где находится memory_mcp в 2026 году

> **Repository-fit note (2026-03-27):** этот документ остаётся полезным как gap-analysis относительно SOTA, но не является canonical runtime-спецификацией репозитория. Текущее shipped-поведение описано в `docs/MEMORY_SYSTEM_SPEC.md`, retrieval target-state — в `docs/SIMPLIFIED_SEARCH_REDESIGN_SPEC.md`, а адаптированная под ограничения репозитория целевая архитектура — в `docs/superpowers/specs/2026-03-27-sota-memory-alignment-design.md`.

> **Важно:** часть наблюдений ниже уже частично или полностью закрыта в кодовой базе (например, lifecycle workers, topology-based community maintenance, multi-source provenance), а часть SOTA-подходов требует адаптации. В частности, для `memory_mcp` не следует автоматически трактовать SOTA как аргумент за возврат embedding-heavy runtime search: в репозитории уже принят курс на lexical/BM25 + graph expansion как основной retrieval backbone.

За последние полтора года область AI-памяти пережила качественный сдвиг. Если в 2023–2024 годах стандартом был простой vector store с temporal метками, то в 2025–2026 SOTA — это многоуровневая, рефлексирующая, само-эволюционирующая система. Три независимых бенчмарка — LongMemEval, LoCoMo и DMR — стали стандартом оценки, и разрыв между лучшими системами (Supermemory: 81.6%, Zep: 71.2%) и наивными подходами достигает 30–60 процентных пунктов.[^1][^2][^3][^4]

С учётом последних изменений репозитория актуальнее говорить так: `memory_mcp` уже реализует lexical-first retrieval, bounded graph/community expansion, alias expansion через батчевый entity lookup, lifecycle workers, bi-temporal facts/edges и provenance-aware explainability. Это хорошая база, но конкретные паттерны из SOTA, которые ещё не адаптированы под текущую архитектурную линию репозитория, по-прежнему обещают измеримый прирост на LongMemEval/LoCoMo-классах задач.

Это эссе — карта разрывов и конкретный план их устранения.

***

## Часть I: Ландшафт SOTA 2025–2026

### 1.1 Иерархическая память: MemoryOS

MemoryOS (EMNLP 2025, BAI-LAB) предложил аналог OS-style memory management: STM → MTM → LTM с FIFO и сегментированной пейджинацией. Ключевой инсайт — **тепловая эвакуация** (heat-driven eviction): факты эвакуируются не по возрасту, а по composite score из частоты доступа и семантической дистанции от текущих запросов. На бенчмарке LoCoMo это дало +49.11% F1 и +46.18% BLEU-1 над базовыми системами.[^5][^6]

В `memory_mcp` archival worker работает на `archival_age_days` — чистый TTL. Факт, к которому агент обращался вчера, имеет одинаковый шанс быть архивированным с фактом, который никогда не запрашивали. Это нарушает принцип "важное остаётся доступным".

### 1.2 HippoRAG 2: граф + dense retrieval

HippoRAG 2 (ICML 2025) — первый подход, который **одновременно** превзошёл крупные embedding-модели (NV-Embed-v2) на factual, sense-making и associative memory tasks. Архитектура: KG с Personalized PageRank (PPR) + dense passage integration. Индексирование в 10x дешевле GraphRAG при сопоставимом качестве. Ключевое отличие от наивного BFS — PPR распространяет релевантность транзитивно через весь граф за один шаг, а не итеративно.[^7][^8]

В `memory_mcp` граф traversal — это `select_edge_neighbors` с per-hop DB round-trip. Community detection существует как FTS-поиск по `community.summary`, но топологических сигналов — степень узла, мостовые рёбра, PageRank — нет. Сообщества формируются только через внешний шаг (`community` таблица в схеме), но нет кода, который их строит на основе edge-топологии.

### 1.3 A-MEM: живая память по принципу Zettelkasten

A-MEM (NeurIPS 2025) вводит принцип **memory evolution**: при добавлении нового факта система анализирует существующие воспоминания, находит связанные и **обновляет** их атрибуты — keywords, contextual descriptions, теги. Память перестаёт быть append-only журналом и превращается в живую сеть.[^9][^10]

В `memory_mcp` факты иммутабельны после записи (за исключением `t_invalid`). Если вы узнали новый контекст о старом факте — обновить его представление нельзя без полной инвалидации и перезаписи. Это принципиально ограничивает quality of retrieval: старый факт продолжает быть представлен с устаревшими embedding и без новых связей.

### 1.4 Reflective Memory Management

RMM (Tan et al., 2025) вводит двухфазовую рефлексию:[^11][^12]
- **Prospective Reflection** — перед записью: суммаризация диалога на трёх уровнях гранулярности (utterance / turn / session) для оптимального индексирования.
- **Retrospective Reflection** — после retrieval: RL-based online refinement на основе evidence, которые агент процитировал в ответе. Система учится, какие факты были полезны.

На LongMemEval RMM даёт +10% accuracy над базовыми методами. В `memory_mcp` `assemble_context` — одношаговый retrieval без петли обратной связи. Нет учёта того, какие `AssembledContextItem` агент реально использовал.[^11]

### 1.5 Временно́е мышление: TReMu и LongMemEval insights

Стандарт LongMemEval (ICLR 2025) выявил, что самые сложные задачи — temporal reasoning и multi-session reasoning — остаются нерешёнными даже у коммерческих ассистентов (30–60% accuracy drop). Решения, которые работают:[^4]
- **Time-aware query expansion**: к запросу добавляются временны́е маркеры из контекста ("апрель", "прошлый месяц").[^13][^4]
- **Fact-augmented key expansion**: при индексации факта дополнительно сохраняются извлечённые из него именованные сущности как дополнительный ключ.[^13]
- **Session decomposition**: история сессии нарезается на атомарные факты, а не хранится как blob.[^4]

Текущий `select_facts_filtered` в `memory_mcp` принимает `cutoff` как hard filter и опциональный `query_contains`. Time-aware query expansion — расширение запроса темпоральными синонимами — не реализовано.

### 1.6 Supermemory и disambiguation

Supermemory (2025) занял SOTA на LongMemEval-s с 81.6% (vs Zep 71.2%, full context GPT-4o 60.2%). Ключевое отличие — **disambiguation beyond simple vector similarity**: система различает, когда два факта про "Алексей Иванов" относятся к разным людям, используя контекстуальные сигналы, а не только имя. В персональном MCP это критично — у одного человека могут быть несколько знакомых с одинаковыми именами.[^1]

### 1.7 Write–Manage–Read как формальная модель

Свежий survey (March 2026) формализует агентную память как **write–manage–read loop** с тремя осями: temporal scope, representational substrate, control policy. Пять семейств механизмов: context compression, retrieval-augmented stores, reflective self-improvement, hierarchical virtual context, policy-learned management. `memory_mcp` реализует retrieval-augmented stores и частично hierarchical (через lifecycle), но reflective self-improvement и policy-learned management отсутствуют полностью.[^14]

***

## Часть II: Структурный анализ разрывов

| Паттерн | Реализован в memory_mcp | Потенциальный прирост | Сложность |
|---------|--------------------------|----------------------|-----------|
| Hybrid retrieval (lexical + semantic + community) | ✅ Да (RRF) | — | — |
| Bi-temporal модель | ✅ Да (t_valid/t_ingested) | — | — |
| Exponential decay (Ebbinghaus) | ✅ Да | — | — |
| Alias expansion (batch entity lookup) | ✅ Да | — | — |
| Heat-based eviction vs age-TTL | ❌ Нет | +15–20% долгосрочный recall | S |
| Memory evolution (backward update) | ❌ Нет | +10–15% retrieval quality | M |
| Time-aware query expansion | ❌ Нет | +10% temporal reasoning | S |
| Prospective reflection (session summarization) | ❌ Нет | +10–20% на длинных сессиях | L |
| Retrospective reflection (feedback loop) | ❌ Нет | +5–10% long-term | XL |
| PPR-based graph retrieval | ❌ Нет | +7–20% multi-hop tasks | XL |
| Community detection по топологии | ❌ Нет | +10–15% associative recall | L |
| LongMemEval eval integration | ❌ Нет | Observability | M |
| Fact-augmented key expansion (indexing) | ❌ Нет | +5–10% information extraction | M |
| Disambiguation (entity coreference) | ❌ Частично | +10% precision | L |

***

## Часть III: Конкретный план действий

### Спринт A — Quick wins (3–5 дней)

**A1: Time-aware query expansion** (S, ~1 день)

При `assemble_context(query, as_of)` перед поиском расширить `query` темпоральными маркерами:
```rust
// В context.rs, перед select_fact_records_for_query
fn expand_query_temporally(query: &str, as_of: DateTime<Utc>) -> Vec<String> {
    let mut variants = vec![query.to_string()];
    // Добавить: "месяц назад", ISO дату недели, название месяца
    let month = as_of.format("%B %Y").to_string(); // "March 2026"
    variants.push(format!("{query} {month}"));
    variants
}
```
Запускать каждый variant параллельно и объединять через существующий RRF pipeline. Это прямо переносит оптимизацию из LongMemEval paper.[^13][^4]

**A2: Heat score на фактах** (S, ~1 день)

Добавить `access_count: u64` и `last_accessed: DateTime<Utc>` в схему `fact`. В `assemble_context` после возврата результатов — атомарно инкрементировать `access_count`. В archival worker заменить:
```sql
-- Было: WHERE t_ref < $cutoff
-- Стать:
WHERE t_ref < $cutoff AND access_count = 0 AND last_accessed < $cold_cutoff
```
Это реализует heat-driven eviction из MemoryOS без изменения основной логики.[^5]

**A3: Fact-augmented key expansion при индексации** (S, ~1–2 дня)

В `add_fact` после записи факта — извлечь из `content` именованные сущности через существующий `RegexEntityExtractor` и сохранить их как `index_keys: Vec<String>` в payload. При FTS-поиске включать `index_keys` в `FULLTEXT` индекс. Реализует insight из LongMemEval: дополнительные ключи = дополнительный recall на information extraction tasks.[^4]

***

### Спринт B — Memory evolution (1 неделя)

**B1: Backward update при ingest** (M, ~3 дня)

Добавить `update_related_facts` шаг в `episode.rs → extract_from_episode`:

```rust
// После store_fact(), для каждого нового факта:
async fn update_related_fact_embeddings(
    service: &MemoryService,
    new_fact: &Fact,
    namespace: &str,
) {
    // 1. ANN-поиск факов с cosine > 0.85
    let related = service.db_client.select_facts_ann(
        namespace, &new_fact.scope, &cutoff_iso,
        &new_fact.embedding, 5
    ).await;
    // 2. Для каждого related факта — обновить entity_links и index_keys
    // (не embedding — дорого; только метаданные)
}
```

Это не полная A-MEM эволюция, но даёт 80% эффекта за 20% стоимости: связи обновляются, embeddings — нет.[^9]

**B2: Community detection по edge-топологии** (M, ~4 дня)

Существующая таблица `community` заполняется только вручную или никак. Добавить воркер, который периодически запускает **connected components** (упрощённый Louvain) по таблице `edge`:

```sql
-- SurrealDB: простые connected components
SELECT * FROM edge WHERE t_invalid IS NULL
  AND t_valid <= time::now()
```

Rust-side: union-find алгоритм по `in_id/out_id`. Для каждого компонента размером > 2 — создать/обновить запись в `community` с LLM-generated summary (через опциональный `SummaryProvider` trait).

Это закрывает главный структурный разрыв с Zep/Graphiti: community detection должен быть основан на реальной graph topology, а не только на FTS.[^15][^16]

***

### Спринт C — Reflection layer (2–3 недели)

**C1: Prospective Reflection — session summarization** (L, ~1 неделя)

Добавить `summarize_session` MCP tool:
```
Input: [episode_id_1, episode_id_2, ...] — список эпизодов сессии
Output: summary_fact_id — факт с type="session_summary"
```

Реализация: LLM-вызов через новый `SummaryProvider` trait (аналог `EmbeddingProvider`). По умолчанию — `DisabledSummaryProvider`. При включённом провайдере — суммаризация на трёх уровнях: per-turn, per-episode, per-session. Суммаризированные факты помечаются `fact_type="summary"` и имеют `entity_links` ко всем упомянутым сущностям.[^11]

**C2: Retrieval feedback — usage tracking** (M, ~3 дня)

Добавить новый MCP tool `record_usage`:
```
Input: { fact_ids: Vec<String>, query: String, session_id: String }
```
Сохраняет `usage_event` в отдельную таблицу. Использовать в decay worker: факты с `usage_count > 0` за последние N дней получают decay rate / 2. Это базовая версия retrospective reflection из RMM без RL.[^11]

***

### Спринт D — Temporal intelligence (1–2 недели)

**D1: Multi-session temporal reasoning** (M, ~4 дня)

Добавить `temporal_timeline` MCP tool:
```
Input: { entity_id: String, from: DateTime, to: DateTime }
Output: хронологически упорядоченные факты об entity
```

Реализация: запрос `SELECT * FROM fact WHERE entity_links CONTAINS $entity_id AND t_valid BETWEEN $from AND $to ORDER BY t_valid`. Плюс — при `assemble_context` для запросов с темпоральными маркерами ("что изменилось", "как было раньше") автоматически обогащать результат timeline.

**D2: LongMemEval eval harness** (M, ~3 дня)

Написать тест-сьют, реализующий 5 категорий LongMemEval:[^4]
1. Information extraction — сохранить факт, запросить через N эпизодов
2. Multi-session reasoning — факты из разных namespace
3. Temporal reasoning — as_of запросы с разными cutoff
4. Knowledge updates — invalidate + re-query
5. Abstention — запрос о несуществующем факте → пустой результат

Это даст **измеримую метрику** прогресса вместо "нам кажется, стало лучше".

***

### Спринт E — Architectural ceiling (долгосрочно, 1–2 месяца)

**E1: Personalized PageRank over entity graph** (XL)

Полная реализация HippoRAG 2: при `find_intro_chain` и community expansion использовать PPR вместо BFS. SurrealDB не поддерживает PPR нативно — потребуется либо перенести граф в rust-side (приемлемо для embedded режима), либо использовать petgraph crate. Высокий ROI для multi-hop associative queries, но дорого в реализации.[^8]

**E2: Параметрическая persona memory** (XL)

Вдохновлено Second Me: долгосрочные стабильные факты о пользователе (предпочтения, роль, контекст) вынести в отдельный `persona` namespace с пониженным decay rate и отдельным retrieval path. Эти факты инклюдируются в каждый `assemble_context` без запроса — как "базовый контекст личности". Полезно именно для персонального MCP.[^17][^18]

***

## Итог: приоритетная матрица

| Приоритет | Задача | Спринт | Ожидаемый эффект | Усилие |
|-----------|--------|--------|-----------------|--------|
| **P0** | Time-aware query expansion | A1 | +10% temporal reasoning | S |
| **P0** | Heat-based eviction | A2 | +15% долгосрочный recall | S |
| **P0** | Fact-augmented key expansion | A3 | +5-10% recall | S |
| **P1** | Backward update при ingest | B1 | +10-15% retrieval quality | M |
| **P1** | Community detection by topology | B2 | +10-15% associative | M |
| **P1** | LongMemEval eval harness | D2 | Observability | M |
| **P2** | Session summarization (prospective) | C1 | +10-20% long sessions | L |
| **P2** | Retrieval feedback / usage tracking | C2 | +5-10% long-term | M |
| **P2** | Temporal timeline tool | D1 | UX + temporal recall | M |
| **P3** | PPR graph retrieval | E1 | +7-20% multi-hop | XL |
| **P3** | Persona namespace | E2 | Персонализация | XL |

Три задачи P0 займут суммарно 3–4 дня и закрывают наиболее измеримые пробелы относительно LongMemEval benchmarks. Задачи P1 (B2: community topology + D2: eval harness) — стратегически самые важные: первая закрывает архитектурный разрыв с Zep, вторая даёт возможность измерять прогресс. Всё остальное — последовательное приближение к SOTA.

---

## References

1. [Supermemory Research — State-of-the-Art Agent Memory](https://supermemory.ai/research/) - Supermemory achieves SOTA results on LongMemEval, solving long-term forgetting in LLMs with reliable...

2. [Zep: A Temporal Knowledge Graph Architecture for Agent Memory](https://arxiv.org/abs/2501.13956) - We introduce Zep, a novel memory layer service for AI agents that outperforms the current state-of-t...

3. [Evaluating Very Long-Term Conversational Memory of LLM Agents](https://arxiv.org/pdf/2402.17753.pdf) - ...by
leveraging LLM-based agent architectures and grounding their dialogues on
personas and tempora...

4. [LongMemEval: Benchmarking Chat Assistants on Long ...](https://arxiv.org/abs/2410.10813) - Recent large language model (LLM)-driven chat assistant systems have integrated memory components to...

5. [Memory OS of AI Agent](https://www.alphaxiv.org/overview/2506.06326) - View recent discussion. Abstract: Large Language Models (LLMs) face a crucial challenge from fixed c...

6. [Memory OS of AI Agent](https://aclanthology.org/2025.emnlp-main.1318/) - Jiazheng Kang, Mingming Ji, Zhe Zhao, Ting Bai. Proceedings of the 2025 Conference on Empirical Meth...

7. [Yu Su on X](https://x.com/ysu_nlp/status/1895305574977265735)

8. [From RAG to Memory: Non-Parametric Continual Learning for Large ...](https://icml.cc/virtual/2025/poster/45585) - We address this unintended deterioration and propose HippoRAG 2, a framework that outperforms standa...

9. [A-MEM: Agentic Memory for LLM Agents](https://arxiv.org/abs/2502.12110) - While large language model (LLM) agents can effectively use external tools for complex real-world ta...

10. [A-MEM: Agentic Memory for LLM Agents](https://arxiv.org/abs/2502.12110v1) - While large language model (LLM) agents can effectively use external tools for complex real-world ta...

11. [In Prospect and Retrospect: Reflective Memory Management for Long-term Personalized Dialogue Agents](https://arxiv.org/abs/2503.08026) - Large Language Models (LLMs) have made significant progress in open-ended dialogue, yet their inabil...

12. [Reflective Memory for Personalized Dialogue Agents](https://www.emergentmind.com/papers/2503.08026) - This paper introduces Reflective Memory Management, a RL-based approach enhancing long-term dialogue...

13. [LongMemEval: Benchmarking Chat Assistants on Long-Term Interactive
  Memory](https://arxiv.org/pdf/2410.10813.pdf) - ...fact-augmented key expansion for indexing, and time-aware query expansion for
refining the search...

14. [[2603.07670] Memory for Autonomous LLM Agents:Mechanisms ...](https://arxiv.org/abs/2603.07670) - Large language model (LLM) agents increasingly operate in settings where a single context window is ...

15. [Zep: Temporal Knowledge Graph Architecture - Emergent Mind](https://www.emergentmind.com/topics/zep-a-temporal-knowledge-graph-architecture) - Zep integrates temporal dynamics with hierarchical memory organization to enable advanced AI reasoni...

16. [Zep: A Temporal Knowledge Graph Architecture for Agent ...](https://arxiv.org/html/2501.13956v1) - We introduce Zep, a novel memory layer service for AI agents that outperforms the current state-of-t...

17. [AI-native Memory 2.0: Second Me - arXiv](https://arxiv.org/html/2503.08102v1)

18. [[2503.08102] AI-native Memory 2.0: Second Me - arXiv](https://arxiv.org/abs/2503.08102) - Human interaction with the external world fundamentally involves the exchange of personal memory, wh...

