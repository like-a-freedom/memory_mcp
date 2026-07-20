Прежде чем писать итоговый анализ, мне нужно верифицировать несколько ключевых утверждений из загруженных документов. Не все источники в них одинаково надёжны, и я хочу проверить, что реально существует, а что могло быть преувеличено или устареть.


# Критический анализ SOTA агентной памяти и план доработок `memory_mcp`

## 0. Контекст и методология

Я верифицировал все ключевые источники из трёх загруженных документов через прямой поиск и извлечение контента. Текущее состояние репозитория: **v1.7.0** (14 июля 2026), 196 коммитов, 827 тестов, активная разработка с рефакторингом архитектуры (InvalidateCapability, Graph BFS в service layer, models split на подмодули) [[github]].

---

## 1. Критическое ревью загруженных документов

### 1.1. «SOTA Memory Plan July 2026.md» (исходный обзор)

**Что верно:**
- Корректная идентификация bi-temporal модели как конкурентного преимущества — среди 40+ MCP memory add-ons лишь Graphiti/Zep имеют настоящую bi-temporal валидность.
- Точная диагностика «episodic-temporal deficit» в MCP-экосистеме.
- Правильная таксономия: Storage → Reflection → Experience (Luo et al. 2026, ICLR MemAgents).

**Что оспорено:**

| Утверждение | Контраргумент |
|---|---|
| Приоритет 2 смешивает GCC и Lore | GCC (intra-session checkpoint/rollback) и Lore (inter-session decision records) — **разные задачи** с разным lifecycle. Смешение размывает scope и создаёт ложную зависимость. |
| Приоритет 3 (мультиграфовая декомпозиция) выше retrieval fusion | Multi-signal fusion даёт ~80% эффекта за ~20% усилий. MAGMA впечатляет (39 цит., ACL 2026 Main ), но для SurrealDB это 3–4 недели миграции с breaking changes. **Сначала fusion, потом графы.** |
| Приоритет 4 (governance) занижен | Для single-user personal scope — да, занижен. Для org/team scope с multi-agent — **критичен**. MINJA: 74 цитирования, 95% success rate injection . Prompt injection attacks выросли на 340% в 2026. |
| RL-политики — «emerging direction» | Memory-R1 на ACL 2026 Main с **147 цитированиями**  — это уже мейнстрим. Формулировка устарела, хотя production-зрелость для Rust-сервера действительно отсутствует. |
| Не упомянуты OM, STATE-Bench, StructMemEval, A-MEM | Три из четырёх — SOTA или near-SOTA. OM: 94.87% LongMemEval **без retrieval** . STATE-Bench: v0.8.1, 4 дня назад обновлён . Пропуск искажает картину. |

### 1.2. «Proposal SOTA Alignment Roadmap.md»

**Что верно:**
- Конкретные schema extensions с Rust-кодом.
- Acceptance criteria для каждого приоритета.
- Правильная идентификация `distill` как ключевого инструмента.

**Что оспорено:**

| Утверждение | Контраргумент |
|---|---|
| P1: `success_rate: f32` как curation metric | MACLA использует **Bayesian reliability** (Beta distribution: α/(α+β)) , а не простой success_rate. Beta distribution даёт естественный uncertainty при малых выборках — критично, когда процедур мало. |
| P2: `checkpoint`/`rollback` в одном пункте с Lore | GCC требует отдельного дизайна (working memory versioning, undo/redo semantics). Это **не зависит** от Lore и не должно блокироваться им. |
| P3: 4 графа сразу | MAGMA использует **policy-guided traversal** для выбора графа . Без policy это brute force по 4 графам. Для SurrealDB без бенчмарка performance — риск. |
| P4: `trust_level: TrustLevel` enum | Lin et al. показывают: trust scoring без ground truth — эвристика . Для educational проекта — over-engineering. Достаточно `contributed_by` + heuristic injection detection. |
| P5: Test-Time Learning через auto-generation corrective facts | Это создаёт **feedback loop**: retrieval генерирует facts, которые влияют на следующий retrieval. Без human-in-the-loop — риск галлюцинационного загрязнения. |

### 1.3. «SOTA Agent Memory → memory_mcp.md» (финальный план)

**Что верно:**
- Верифицированные источники (11+ первоисточников).
- Конкретные code snippets для каждого приоритета.
- Секция антипаттернов — редкая и ценная.
- Правильная последовательность: fusion → графы, P1 ∥ P3.

**Что оспорено:**

| Утверждение | Контраргумент |
|---|---|
| P1: 3–4 недели на procedural memory | Оптимистично. Новый тип в models + новый сервис + новый MCP-инструмент + LLM-зависимость distill + тесты + heuristic fallback. Реалистично: **4–6 недель** для одного разработчика. |
| P2: 1–2 недели на Lore | Реалистично для базовой интеграции. Но Lore имеет **2 цитирования**  — community adoption не гарантирован. Инвестиция в протокол с неопределённым будущим. |
| P4: зависит от P2 (causal graph ← decision provenance) | Спорно. Causal graph может строиться на `invalidate` chains **без** VCS context. Зависимость P4→P2 искусственная и блокирует параллельную работу. |
| P6: Memory evolution 2–3 недели | A-MEM evolution pass на каждый `extract` добавляет latency. Для watch-mode с high-volume ingestion — **критично**. Нужен async/background режим, что увеличивает оценку до 3–4 недель. |
| OM упомянут, но не интегрирован в план | OM показывает 94.87% **без retrieval** . Это не просто «ещё один подход» — это прямой вызов архитектуре. Для conversational use-cases observation log + reflection может быть **достаточен**. Нужен явный decision: добавлять Observer-слой или нет. |

---

## 2. Верифицированный SOTA-ландшафт (июль 2026)

### 2.1. Четыре production-зрелые парадигмы

| Парадигма | Пример | LongMemEval | Сильная сторона | Слабость |
|---|---|---|---|---|
| Extraction-based | Mem0 v3  | 94.4% | Multi-signal retrieval, 6.8K tokens/query | ADD-only → stale facts, потеря causal chains  |
| Self-managing | A-MEM  | — | Memory evolution, 890 цит. | Низкая прозрачность, мало production-систем |
| Graph-based temporal | Graphiti/Zep, **memory_mcp** | 71.2% (Zep) | Bi-temporal, causal reasoning | Latency, сложность графа |
| Observation-based | Mastra OM  | **94.87%** | Zero retrieval, prompt-cacheable, 3–6× compression | Теряет exact wording; multi-session ceiling 87.2% |

**Критический нюанс:** OM не использует retrieval вообще. Контекст — append-only observation log + Observer/Reflector агенты . Для большинства conversational use-cases это может быть достаточно. Но для coding-агентов **exact wording критичен** — OM теряет его без retrieval mode.

### 2.2. Ключевые бенчмарки

| Бенчмарк | Статус | Что измеряет | Релевантность для memory_mcp |
|---|---|---|---|
| LongMemEval | 428 цит. | 5 способностей QA | Уже частично покрыт |
| STATE-Bench  | v0.8.1, 16 июля 2026 | pass@1, **pass^5**, UX Score, Cost/Task | Новый стандарт. Agent Learning Track — для memory/skills |
| StructMemEval  | ICLR 2026, 3 цит. | Организация памяти: ledgers, trees, state tracking | Тестирует то, что retrieval **не решает** |
| LongMemEval-V2  | 6 цит., 451 вопрос | Agent-environment experience | Coding-agent: 72.5% vs RAG: 48.5% |
| BEAM  | ICLR 2026, 19 цит. | До 10M tokens, 10 способностей | Масштабный стресс-тест |
| MemoryAgentBench | 4 компетенции | Test-Time Learning, Conflict Resolution | Conflict Resolution ≈ `invalidate` |

**Ключевой вывод StructMemEval:** simple retrieval (EMem baseline) **обходит** сложные memory-системы на LongMemEval/LoCoMo . Разница проявляется **только** на задачах организации памяти. Если memory_mcp бенчмаркается только на LongMemEval, он может не увидеть преимущества своей графовой архитектуры.

### 2.3. Безопасность

Memory Lifecycle Security Framework (Lin et al., 4 цит.) : 6 фаз lifecycle × 4 security objectives. VMG: 5 примитивов с зависимостью **VF ⪯ RB ⪯ PV ⪯ WA**. Без Write Authorization всё остальное не работает.

MINJA (NeurIPS 2025, **74 цитирования**) : query-only injection в shared memory с 95% success rate. Для single-user personal scope угроза **драматически ниже**, но для org/team — критична.

### 2.4. Procedural memory

MACLA (AAMAS 2026, 13 цит.) : hierarchical procedures + **Bayesian reliability** (Beta distribution) + contrastive refinement. 78.1% avg, 90.3% ALFWorld unseen, **2800× быстрее fine-tuning**. 2851 траектория → 187 процедур за 56 секунд.

---

## 3. План доработок: пересмотренные приоритеты

Я пересматриваю приоритеты из финального плана с учётом:
- Реального состояния v1.7.0 (827 тестов, чистая архитектура, SurrealDB 3.2.0)
- Того, что это **educational/research** проект, не production
- Соотношения effort/impact
- Реальных зависимостей (не искусственных)

### P1 — Multi-signal retrieval fusion ⚡ (1–2 недели)

**Почему первый, а не procedural memory:** 80% эффекта за 20% усилий. Не требует новых типов, новых MCP-инструментов, LLM-зависимости. Изменения только в `service/retrieval`.

**Что делать:**

```rust
// src/service/retrieval.rs
pub struct RetrievalSignals {
    pub bm25_score: f64,        // Существующий
    pub semantic_score: f64,    // Существующий (optional)
    pub entity_score: f64,      // НОВЫЙ: entity name/alias matching
}

pub fn fused_score(signals: &RetrievalSignals, weights: &SignalWeights) -> f64 {
    weights.bm25 * normalize(signals.bm25_score)
    + weights.semantic * normalize(signals.semantic_score)
    + weights.entity * normalize(signals.entity_score)
}
```

- Entity matching как третий сигнал: извлекать entities из query (уже есть `resolve`), матчить против entity-коллекции, boostить связанные facts.
- Веса через env vars: `RETRIEVAL_WEIGHT_BM25` (0.4), `RETRIEVAL_WEIGHT_SEMANTIC` (0.3), `RETRIEVAL_WEIGHT_ENTITY` (0.3).
- Нормализация: **RRF (Reciprocal Rank Fusion)** — проще и устойчивее min-max/z-score для разнородных scores.

**Обоснование:** Mem0 v3: semantic + BM25 + entity matching → **+29.6 на temporal, +23.1 на multi-hop** . Текущий `assemble_context` делает lexical/BM25-first с опциональным semantic — без fusion.

**Риски:**
- Entity matching без semantic retrieval (`EMBEDDINGS_ENABLED=false`) даёт только 2 сигнала. Fusion с 2 сигналами менее эффективен, но всё равно лучше одного.
- **Не повторять ошибку Mem0**: они заменили external graph store на built-in entity linking и **потеряли** queryable graph interface . Graph traversal остаётся.

**Definition of Done:**
- [ ] Entity matching как третий сигнал в `assemble_context`
- [ ] Fused score с конфигурируемыми весами (RRF)
- [ ] Acceptance test: query с entity anchor → entity-linked facts ранжируются выше
- [ ] Regression: 827 существующих тестов проходят без изменений

---

### P2 — Procedural memory с Bayesian reliability (4–6 недель)

**Почему второй:** Самый дефицитный тип в MCP-экосистеме и исследованиях. Но требует LLM-зависимости, нового типа, нового сервиса — поэтому не первый.

**Что делать:**

```rust
// src/models/procedure.rs (новый submodule в models/)
pub struct Procedure {
    pub id: ProcedureId,
    pub name: String,
    pub steps: Vec<ProcedureStep>,
    pub source_episodes: Vec<EpisodeId>,
    // Bayesian reliability (Beta distribution) — НЕ простой success_rate
    pub alpha: f64,  // α = success_count + 1
    pub beta: f64,   // β = failure_count + 1
    pub last_refined_at: DateTime<Utc>,
    pub scope: Scope,
    pub project: Option<String>,
}
```

**Почему Beta distribution, а не `success_rate: f32`:** MACLA показывает, что Bayesian reliability даёт P(reliability) = α/(α+β) с **естественным uncertainty** при малых выборках . Процедура с 1 успехом из 1 (rate=1.0) и процедура с 90 успехами из 100 (rate=0.9) — **разные** по надёжности. Beta distribution это различает.

- Новый сервис `src/service/distill.rs`: группировка эпизодов по entity overlap → contrastive pairing (success vs failure) → извлечение шагов → Bayesian update.
- Новый MCP-инструмент `distill`.
- **Heuristic fallback** для distill без LLM: entity overlap + temporal proximity + keyword extraction.
- `assemble_context` учитывает procedures при ранжировании (boost по reliability).

**Риски:**
- WebCoach: модели ≤7B **не выигрывают** от experiential memory. Для coding-агентов (32B+) менее критично, но конфигурируемость нужна.
- SWE Context Bench: «неотфильтрованный опыт **вредит**». Агрессивная фильтрация обязательна: `min_episodes` threshold, `alpha + beta >= 5` gate.
- Contrastive refinement требует явных success/failure labels — не всегда доступны.

**Definition of Done:**
- [ ] `procedure` тип в SurrealDB с Bayesian fields
- [ ] `distill` MCP-инструмент с heuristic fallback
- [ ] Acceptance test: 10 эпизодов → ≥1 procedure с α+β ≥ 5
- [ ] `assemble_context` boost по reliability

---

### P3 — VCS-linked decision provenance (1–2 недели, **без GCC**)

**Почему отдельно от GCC:** GCC (intra-session checkpoint/rollback) и Lore (inter-session decision records) — **разные задачи**. GCC требует working memory versioning, undo/redo semantics. Lore — расширение `explain()` и `ingest`. Смешение блокирует обе.

**Что делать:**

```rust
// Расширение models/provenance.rs
pub struct VcsContext {
    pub commit_hash: Option<String>,
    pub branch: Option<String>,
    pub pr_id: Option<String>,
    pub constraints: Vec<String>,
    pub rejected_alternatives: Vec<RejectedAlt>,
    pub confidence: Option<Confidence>,  // low/medium/high
}
```

- Новый optional параметр `vcs_context` в `IngestRequest`.
- `explain()` возвращает VCS context при наличии.
- Backward compatibility: ingest без VCS context работает как раньше.

**Критическая оговорка:** Lore protocol имеет **2 цитирования** . Community adoption не гарантирован. Инвестиция оправдана **только** если целевой use-case — coding-агенты. Для email/document ingestion VCS context нерелевантен.

**Решение:** Реализовать как **optional extension**, не как core dependency. Если Lore не получит adoption, код не мешает.

**GCC — отдельный пункт**, не в этом плане. Требует отдельного дизайна.

---

### P4 — Memory lifecycle security (2–3 недели, **только для org/team scope**)

**Почему не для personal:** Для single-user personal scope угроза memory poisoning **драматически ниже** . Не over-engineer.

**Что делать (в порядке VMG-зависимости):**

1. **Write Authorization (WA):** `contributed_by: AgentId` на каждой записи. Heuristic injection detection при ingest (аномально длинные/структурированные «инструкции»).
2. **Provenance Visibility (PV):** Расширить `explain()`: `contributed_by`, `trust_level`, `write_timestamp`.
3. **Principal-Scoped Retrieval (PS):** `contributed_by` filtering при retrieval для org scope.
4. **Rollbackability (RB):** Batch invalidation по `contributed_by`.
5. **Verified Forgetting (VF):** Новый MCP-инструмент `purge` с post-deletion verification.

**Критическая оговорка:** Trust scoring без ground truth — **эвристика**. False positives блокируют легитимный контент. Для educational проекта достаточно heuristic detection + `contributed_by`, без сложного composite trust scoring.

**Definition of Done:**
- [ ] `contributed_by` на каждой записи
- [ ] Injection detection при ingest (heuristic, configurable threshold)
- [ ] `purge` MCP-инструмент с post-deletion verification
- [ ] Acceptance test: inject → detect → purge → verify empty

---

### P5 — Мультиграфовая декомпозиция (3–4 недели, **после P1**)

**Почему после fusion:** Multi-signal fusion даёт 80% эффекта за 20% усилий. MAGMA впечатляет, но для SurrealDB это migration + breaking changes + performance risk.

**Что делать:**

```sql
-- 4 ортогональных слоя в SurrealDB
RELATE fact:f1->semantic_rel->entity:alice;     -- semantic graph
RELATE fact:f1->temporal_next->fact:f2;          -- temporal graph
RELATE fact:f1->caused_by->fact:f0;              -- causal graph
RELATE entity:alice->alias_of->entity:alicia;    -- entity graph
```

- Расширение `assemble_context` параметром `graph_layers`.
- **Lightweight query classifier** (heuristic, не LLM): `"why"` → causal, `"timeline"` → temporal, `"what is"` → semantic + entity.
- Migration script для существующих данных.

**Критическая оговорка:** MAGMA использует **policy-guided traversal** . Без policy это brute force по 4 графам. SurrealDB graph performance на 4-слойной модели **не бенчмаркался**. Нужен performance gate перед merge.

**Зависимость от P3 (Lore) — снята.** Causal graph строится на `invalidate` chains, не на VCS context.

---

### P6 — End-to-end бенчмаркинг (2–3 недели + ongoing)

**Что делать:**

1. **LongMemEval end-to-end:** retrieve → generate → judge pipeline. Публиковать **обе** цифры: retrieval recall и QA accuracy.
2. **STATE-Bench Agent Learning Track** : хотя бы 1 домен (travel, 150 tasks). Метрики: pass@1, **pass^5** (reliability), UX Score, Cost/Task.
3. **StructMemEval** : хотя бы 1 категория (ledgers). Критично для валидации, что графовая архитектура даёт преимущество над simple retrieval.

**Критическая оговорка:** STATE-Bench и StructMemEval — **Python**. Интеграция с Rust MCP-сервером требует Python wrapper или subprocess. Это **нетривиально** и не должно блокировать основные доработки.

**Definition of Done:**
- [ ] LongMemEval end-to-end pipeline
- [ ] STATE-Bench: 1 домен (travel)
- [ ] StructMemEval: 1 категория (ledgers)
- [ ] Публичный `COMPARISON.md` с честными цифрами

---

### P7 — Memory evolution (A-MEM паттерн, 3–4 недели)

**Почему не выше:** A-MEM имеет 890 цитирований , но **production-систем на его основе мало**. Академический интерес ≠ engineering зрелость. Evolution pass добавляет latency на каждый `extract` — для watch-mode с high-volume ingestion **критично**.

**Что делать:**
- Evolution pass после extract: contradictions (уже есть через `invalidate`) + extensions + new links.
- **Async/background режим** для watch-mode, чтобы не блокировать ingestion.
- Bi-temporal preservation при UPDATE: новая версия с `t_transaction = now()`.

---

### P8 — RL-политики (исследовательский прототип, 4+ недель, **не в основном таймлайне**)

Memory-R1 (147 цит., ACL 2026 Main) : RL-trained MemoryManager + MemoryUtilizer. Для Rust/MCP-сервера единственный реалистичный путь — **ONNX export** inference-only policy. Не в production. Исследовательский прототип в отдельной ветке.

---

## 4. Антипаттерны: что НЕ делать

| # | Антипаттерн | Почему |
|---|---|---|
| 1 | Реализовывать OM вместо текущего pipeline | OM оптимизирован для conversational memory. memory_mcp — для structured knowledge (emails, docs, code). OM **теряет exact wording** без retrieval mode . Для coding-агентов exact wording критичен. |
| 2 | Заменять graph store на built-in entity linking (Mem0 путь) | Mem0 заменил external graph на entity collection и **потерял** queryable graph interface . Graph traversal — ключевое преимущество memory_mcp. |
| 3 | Смешивать GCC и Lore в один пункт | Разные задачи, разные lifecycle, разные пользователи. GCC = intra-session, Lore = inter-session. |
| 4 | Over-engineer security для personal scope | MINJA: в реалистичных условиях с pre-existing memories атаки **драматически менее эффективны**. Для personal — базовый `contributed_by`. Для org/team — полный VMG. |
| 5 | Бенчмаркаться только на LongMemEval | StructMemEval: simple retrieval **обходит** complex memory на LongMemEval . Преимущество графовой архитектуры видно только на задачах организации. |
| 6 | Хранить все процедуры без курации | SWE Context Bench: «неотфильтрованный опыт **вредит**». MACLA: 2851 → 187 процедур . Агрессивная фильтрация обязательна. |
| 7 | Делать RL до P1–P7 | RL заменяет эвристики. Если эвристик ещё нет, RL нечего оптимизировать. |
| 8 | Test-Time Learning через auto-generation corrective facts | Feedback loop без human-in-the-loop → галлюцинационное загрязнение. |

---

## 5. Таймлайн

```
Август 2026:
  ├── P1: Multi-signal retrieval fusion          [1-2 нед]
  └── P2: Procedural memory + Bayesian           [4-6 нед]
      (параллельно, не зависят)

Сентябрь 2026:
  ├── P3: VCS-linked provenance (без GCC)        [1-2 нед]
  └── P6: Бенчмаркинг инфраструктура             [2-3 нед]

Октябрь 2026:
  ├── P4: Memory lifecycle security              [2-3 нед]
  └── P5: Мультиграфовая декомпозиция            [3-4 нед]
      (P5 зависит от P1: fusion уже в retrieval)

Ноябрь 2026:
  ├── P7: Memory evolution (A-MEM)               [3-4 нед]
  └── P6: STATE-Bench + StructMemEval            [ongoing]

Декабрь 2026+:
  └── P8: RL-политики (исследовательский)        [4+ нед]
```

**Зависимости (реальные, не искусственные):**
- P1 ∥ P2 — параллельно
- P5 → P1 (fusion уже в retrieval перед добавлением graph layers)
- P7 → P2 (evolution работает с procedures)
- P4 — в любой момент
- P6 — параллельно с любым
- P8 — после всех

---

## 6. Метрики успеха

| Метрика | Текущее | Целевое | Как измерять |
|---|---|---|---|
| LongMemEval retrieval recall | ~85% (est.) | ≥90% | Acceptance tests |
| LongMemEval QA accuracy (e2e) | Не измеряется | ≥80% | P6 pipeline |
| STATE-Bench pass^5 (travel) | Не измеряется | ≥40% | STATE-Bench  |
| StructMemEval (ledgers) | Не измеряется | ≥60% | StructMemEval  |
| Procedures distilled | 0 | ≥10 из 100 эпизодов | `distill` test |
| Retrieval latency p50 | ~50ms (est.) | ≤100ms (с fusion) | `query_log` |
| Entity matching coverage | 0% | ≥70% queries | `query_log` |
| Injection detection rate | 0% | ≥90% (heuristic) | Synthetic test suite |

---

## 7. Итоговая оценка

`memory_mcp` v1.7.0 — **крепкий educational проект** с архитектурой, которая уже превышает большинство MCP memory add-ons по bi-temporal модели, provenance и lifecycle. 827 тестов, чистый layered design, SurrealDB 3.2.0.

**Главный стратегический вопрос**, на который нужно ответить **до** начала доработок: для какого use-case оптимизируем?

- **Coding-агенты** → P1 (fusion) + P2 (procedural) + P3 (VCS) — максимальный ROI.
- **Conversational memory** → OM-подход может быть лучше. Но это **другой проект**.
- **Multi-agent org/team** → P4 (security) становится критичным и поднимается в приоритете.

Все бенчмарк-цифры в этом документе — **self-reported** авторами соответствующих работ. Cross-system сравнение условно из-за разных моделей, датасетов и judge-промптов.
