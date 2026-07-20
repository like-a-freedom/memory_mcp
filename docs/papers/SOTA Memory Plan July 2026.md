## Обзор состояния memory_mcp

`memory_mcp` (like-a-freedom) — Rust/SurrealDB MCP-сервер, реализующий bi-temporal knowledge model, episode ingestion, entity resolution, fact extraction, graph relationships, опциональные embedding-провайдеры (local-candle/openai-compatible/ollama), lifecycle decay/archival и GLiNER NER . Проект уже явно ориентируется на LongMemEval-style acceptance-тесты и адаптивные фичи "aligned with SOTA research" (fact-augmented index keys, heat-aware lifecycle, timeline retrieval) . Открытых issues в репозитории нет — недоработки нужно выводить из сопоставления с академической литературой и практикой продакшн-систем .

Ниже — синтез из ключевых survey 2026 года (Rethinking Memory Mechanisms of Foundation Agents, Memory in the Age of AI Agents, From Storage to Experience, Memory-Aware Software Engineering Agents ), профильных ICLR 2026 MemAgents workshop-статей, бенчмарков LongMemEval/LongMemEval-V2/MemoryAgentBench, и практики production-систем (Mastra/Zep/Graphiti, agentmemory).[^1][^2][^3][^4][^5][^6][^7][^8][^9][^10][^11][^12][^13][^14][^15][^16][^17][^18]

## Ключевые академические принципы SOTA агентной памяти на 2026 год

### Таксономия форм, функций и субъектов памяти

Свежий обзор "Memory in the Age of AI Agents" разграничивает три формы памяти — token-level, parametric, latent — и три функции: factual, experiential, working. Второй крупный survey добавляет измерение "субъекта" памяти (agent-centric vs user-centric) и substrate (внутренний/внешний), а также подчёркивает, что "разные типы памяти инстанцируются по-разному в зависимости от топологии агентов". Для memory_mcp это означает, что текущая архитектура (episodes → entities/facts → graph) покрывает **factual** и частично **working** память (через `assemble_context`), но **experiential** и **procedural** память практически не представлены — а именно эти типы survey по SE-агентам называет наиболее дефицитными в существующих системах.[^9][^14][^18]

### Трёхстадийная эволюция: Storage → Reflection → Experience

Свежий survey "From Storage to Experience" формализует эволюцию памяти агентов в три стадии: Storage (сохранение траекторий), Reflection (их доработка/уточнение) и Experience (абстрагирование в переносимый опыт), выделяя два трансформирующих механизма продвинутой стадии — proactive exploration и cross-trajectory abstraction. memory_mcp сейчас полностью находится на стадии Storage/Reflection (ingest → extract → invalidate); стадия Experience — то есть превращение эпизодов в переиспользуемые процедурные знания — отсутствует.[^11]

### Episodic-temporal дефицит и MCP-экосистема

Специализированный анализ 10 production SE-harness'ов и 40+ MCP memory add-ons показывает системный "episodic-temporal deficit": ни один production-харнесс не реализует episodic memory или temporal versioning нативно, а среди community MCP-серверов лишь единицы (Graphiti/Zep) обладают настоящей bi-temporal валидностью. memory_mcp здесь выделяется на общем фоне — bi-temporal модель (valid time + transaction time) у него уже реализована нативно , что ставит его в топ по этому измерению относительно почти всей экосистемы. Однако тот же survey подчёркивает, что **процедурная память остаётся самым редким типом** и в исследованиях, и в MCP-экосистеме  — это прямой сигнал для приоритизации.[^9]

### Три архитектурных парадигмы, достигшие production-зрелости

LoCoMo-Plus сравнение показывает, что ни одна парадигма не доминирует по всем компетенциям одновременно:[^9]

| Парадигма | Пример | Сильная сторона | Слабость |
|---|---|---|---|
| Extraction-based | Mem0 | Фактический recall | Теряет причинные цепочки и temporal-контекст [^9] |
| Self-managing | Letta | Поведенческая непрерывность, эволюция состояния | Низкая прозрачность/аудируемость [^9] |
| Graph-based temporal | Graphiti/Zep | Причинное рассуждение, différenciation event time / ingestion time | Latency и сложность построения графа [^9] |

memory_mcp структурно ближе к третьей парадигме (graph relationships + bi-temporal), что хорошо, но отчёты подчёркивают: лучшие результаты дают **гибридные** архитектуры, объединяющие несколько парадигм одновременно.[^9]

### Мультиграфовая и иерархическая декомпозиция памяти

MAGMA представляет каждый элемент памяти в ортогональных графах — семантическом, temporal, каузальном и entity-графе, с retrieval как policy-guided traversal. Формальная теория иерархической памяти вводит три оператора — extraction, coarsening, traversal — а TiMem реализует это через temporal memory tree. memory_mcp сейчас использует единый граф "episodes-entities-facts" без разделения на orthogonal-слои — это ограничивает возможность независимо запрашивать, например, только каузальные связи decision provenance или только temporal-валидность API.[^9]

### Decision provenance и версионирование через git-подобные механизмы

Git Context Controller решает intra-session проблему памяти через git-инспирированные операции, а протокол Lore превращает git commit messages в структурированные decision records с constraints, rejected alternatives, verification metadata через git trailers. Это прямо релевантно для инструмента разработчика — memory_mcp имеет `explain()` с multi-source provenance (direct/linked sources, entity_path) , но не привязан к VCS-артефактам (commit hash, PR, branch), что ограничивает decision provenance до уровня "episode ↔ entity", а не "код ↔ решение".[^9]

### Экспериенциальная память требует масштаба модели и курации

WebCoach выявляет "capacity threshold": модели ≤7B не выигрывают от экспериенциальной памяти, а модели 32B+ показывают выраженный прирост. Отдельно показано, что самогенерируемый опыт превосходит по эффективности внешне посеянный, а SWE Context Bench формулирует критический вывод: "суммаризированный опыт улучшает resolution, неотфильтрованный — вредит". Procedural-memory системы типа Memp дистиллируют траектории в step-by-step инструкции и script-подобные абстракции, а ReasoningBank хранит не сырые траектории, а обобщённые reasoning-стратегии, извлечённые из самооценённых успехов/неудач.[^9]

### Reinforcement learning над политиками памяти

Memory-R1 и AgeMem заменяют эвристические политики управления памятью на обученные RL-политики, что открывает нетривиальные стратегии (например, превентивная суммаризация до заполнения контекстного окна). Это направление пока экспериментальное, но фиксируется как одна из точек роста "второй половины" исследований по памяти агентов.[^14][^9]

### Governance, безопасность и мультипользовательская память

Collaborative Memory вводит двухуровневую модель private/shared memory с provenance-атрибутами (contributing agents, accessed resources, timestamps). При этом MINJA демонстрирует атаки типа memory injection через query-only векторы на shared-memory агентов, что критично для инструментов с доступом к кодовой базе и деплойным пайплайнам. SSGM и MemArchitect предлагают governance middleware для эволюционирующей памяти, а отдельный survey адресует "mnemonic sovereignty" в контексте требований EU AI Act (удаление данных, аудируемость). У memory_mcp уже есть security-hardening roadmap документ , но нет явных защит от memory poisoning/injection через входящие эпизоды и нет multi-user access control модели.[^9]

### Бенчмаркинг: что валидно измерять

LongMemEval разделяет пять способностей: information extraction, multi-session reasoning, temporal reasoning, knowledge updates, abstention. Оригинальная статья предлагает три ключевых оптимизации индексации/поиска — session decomposition, fact-augmented key expansion, time-aware query expansion — которые улучшают recall и downstream QA. memory_mcp явно указывает, что реализует "fact-augmented index keys" и LongMemEval-style acceptance-тесты  — это прямое следование best practice. Однако важный нюанс из практики agentmemory: retrieval recall (R@K) — не то же самое, что official LongMemEval-метрика, которая требует end-to-end generation + GPT-4o judge. У memory_mcp пока нет данных о том, тестируется ли именно end-to-end QA accuracy, а не только recall метрика ассемблирования контекста.[^6][^15]

Новый LongMemEval-V2 (LME-V2) специально фокусируется на agent-environment experience (workflow knowledge, environment gotchas, premise awareness) — сфере, для которой оптимальным методом оказался coding-agent-driven evidence gathering (72.5% accuracy) против RAG-baseline (48.5%). Это подсказывает, что чисто ретривальные подходы недостаточны для "gotchas"-типа знаний — нужна reflective-стадия, а не только retrieval.[^13]

MemoryAgentBench выделяет четыре компетенции: Accurate Retrieval, Test-Time Learning, Long-Range Understanding, Conflict Resolution. "Conflict Resolution" прямо соответствует bi-temporal invalidation в memory_mcp, но "Test-Time Learning" (обучение в моменте использования, а не только на этапе ingest) пока не покрыто явным механизмом.[^16]

## Практический вывод по позиционированию memory_mcp

memory_mcp опережает большинство MCP-конкурентов по bi-temporal модели, provenance и lifecycle decay, но недоинвестирован в: (1) procedural/experiential memory, (2) мультиграфовую декомпозицию, (3) governance/security против memory poisoning, (4) VCS-нативный decision provenance, (5) end-to-end QA-бенчмаркинг вместо чисто retrieval-метрик.[^9]

## Конкретный план доработок

### Приоритет 1 — процедурная и экспериенциальная память (закрывает самый острый дефицит категории)

- Добавить новый тип сущности `procedure`/`lesson`, отдельный от `fact`: хранить не факты, а дистиллированные шаги/скрипты по образцу Memp — "step-by-step instructions" и "script-like abstractions" из истории `extract`/`invalidate` вызовов.[^9]
- Реализовать reflective pass после серии эпизодов: агрегировать успешные/неуспешные траектории в обобщённые reasoning-стратегии (ReasoningBank-подход) вместо хранения сырых эпизодов один-в-один.[^9]
- Обязательно **курировать**, а не хранить всё подряд: добавить порог качества/суммаризации перед записью в experiential-память, так как SWE Context Bench показывает вред от неотфильтрованного опыта.[^9]
- Учесть capacity-threshold находку: если целевые агенты используют модели <7B, экспериенциальная память может не окупаться — сделать эту фичу опциональной/конфигурируемой по размеру модели клиента.[^9]

### Приоритет 2 — decision provenance, привязанный к VCS

- Расширить модель `episode`/`fact` полями commit hash, branch, PR id — по аналогии с протоколом Lore, который переиспользует git trailers для constraints, rejected alternatives, verification metadata.[^9]
- Добавить MCP-инструмент `link_decision` или расширить `explain()` так, чтобы provenance-путь мог включать не только `episode_id`, но и git-объекты — это прямое дифференцирующее преимущество для coding-агентов.
- Ввести intra-session versioned working memory по типу Git Context Controller — checkpoint/rollback операций внутри одной задачи, отдельно от inter-session `ingest`.[^9]

### Приоритет 3 — мультиграфовая декомпозиция вместо единого графа

- Разделить единый knowledge graph на как минимум два независимо индексируемых слоя: semantic-граф (сущности/факты, как сейчас) и causal/temporal-граф (decision provenance, invalidation chains) — по аналогии с MAGMA (orthogonal semantic/temporal/causal/entity graphs).[^9]
- Позволить `assemble_context` явно указывать, какой граф(ы) traversed — это даёт точный контроль для запросов типа "почему был принят этот API" (causal) vs "что известно про X" (semantic).

### Приоритет 4 — governance и защита от memory poisoning

- Реализовать access-control модель private/shared по образцу Collaborative Memory: для scope `personal` vs `org`/`team` уже есть разграничение , но нет provenance-атрибутов "contributing agent" на уровне записи — добавить поле `contributed_by`/`trust_level`.
- Добавить валидацию входящих эпизодов на признаки injection-паттернов (аномально длинные/структурированные "инструкции" внутри контента файла) — MINJA демонстрирует, что query-only injection реален именно для shared-memory систем.[^9]
- Формализовать retention/deletion API (уже есть `LIFECYCLE_ARCHIVAL_*`, `invalidate`) под требования GDPR/EU AI Act-совместимого удаления — обзор mnemonic sovereignty напрямую называет это неадресованным пробелом в экосистеме.[^9]

### Приоритет 5 — приведение бенчмаркинга к end-to-end стандарту

- Дополнить текущие LongMemEval-style acceptance-тесты  полным пайплайном retrieve→generate→judge (GPT-4o-as-judge или аналог), а не только recall@K — иначе цифры несопоставимы с официальным лидербордом LongMemEval.[^15][^6]
- Явно документировать, что публикуемые метрики — retrieval recall, а не QA accuracy, по примеру честного disclaimer в agentmemory COMPARISON.md, чтобы не повторить методологическую критику, которую получил MemPalace за смешение метрик.[^15]
- Добавить оценку по MemoryAgentBench-компетенциям Test-Time Learning и Conflict Resolution — вторая уже частично покрыта через `invalidate`, первая пока нет.[^16]
- Рассмотреть LongMemEval-V2 категории (workflow knowledge, environment gotchas, premise awareness) как ориентир для coding-специфичного тест-сета, поскольку это ближе к реальному use-case Rust/MCP-агента, чем общий чат-бенчмарк.[^13]

### Приоритет 6 — RL-политики над операциями памяти (долгосрочно, экспериментально)

- Изучить замену текущих эвристик lifecycle (decay half-life, archival age) на обучаемую политику по образцу Memory-R1/AgeMem — потенциал в нетривиальных стратегиях типа preemptive-суммаризации до исчерпания контекстного бюджета.[^9]
- Приоритет ниже остальных: методы пока не production-зрелые, четыре survey описывают их как "emerging direction", а не готовую практику.[^14][^9]

## Итоговая матрица соответствия SOTA-практикам

| Практика из SOTA-литературы | Статус в memory_mcp | Приоритет доработки |
|---|---|---|
| Bi-temporal validity (valid vs ingestion time) [^9] | Реализовано  | — |
| Fact-augmented index keys, LongMemEval acceptance tests [^6] | Реализовано  | Дополнить end-to-end QA-метрикой |
| Episodic memory | Реализовано (`ingest`/episodes)  | — |
| Procedural memory [^9] | Отсутствует | Приоритет 1 |
| Experiential/reflective memory (стадия Experience) [^11] | Отсутствует | Приоритет 1 |
| Decision provenance / VCS-linked [^9] | Частично (`explain` provenance без VCS)  | Приоритет 2 |
| Мультиграфовая декомпозиция (MAGMA) [^9] | Единый граф | Приоритет 3 |
| Governance / access control / anti-poisoning [^9] | Частично (scopes, security roadmap)  | Приоритет 4 |
| Test-Time Learning компетенция [^16] | Отсутствует | Приоритет 5 |
| RL-политики управления памятью [^9] | Отсутствует | Приоритет 6 (низкий) |

---

## References

1. [ENGRAM: EFFECTIVE, LIGHTWEIGHT MEMORY OR](https://openreview.net/pdf?id=qajz4UkgIw) - Table 3: Performance comparison on the LongMemEval benchmark. We report accuracy across question typ...

2. [Compatibility-First Design Is Critical for Progress in Agentic ...](https://openreview.net/pdf?id=6OpIEryEWm) - This position paper argues that neither uncoor- dinated fragmentation nor rigid standardisation is d...

3. [APEX-MEM: Agentic Semi-Structured Memory with ...](https://openreview.net/pdf?id=Rub55frimD) - by P Banerjee · 2026 · Cited by 2 — We present APEX-MEM, a conver- sational memory system that combi...

4. [GAM: HIERARCHICAL GRAPH MEMORY FOR LLM](https://openreview.net/pdf/1deb3e361467c10ad50c9f24d17bf2a1bb452988.pdf) - A robust agentic memory must rapidly capture real-time interactions while safeguarding established k...

5. [MEMORY TYPE MATTERS: ENHANCING LONG-TERM ...](https://openreview.net/pdf?id=WkYzCpZMOF) - 2026 Memory Multi-Classification Prompt Goal: You are an advanced AI tasked with classifying a dialo...

6. [LONGMEMEVAL: Benchmarking Chat Assist- ants on Long ...](https://openreview.net/pdf?id=wIonk5yTDq) - by D Wu · Cited by 428 — We introduce LONGMEMEVAL, a comprehensive, challenging, and scalable benchm...

7. [HETEROGENEOUS MULTI-AGENT LLM SYSTEMS ...](https://openreview.net/pdf?id=UbSUxAK3BI) - by S Yuen · Cited by 19 — We propose Intrinsic Memory Agents, a framework for multi-agent LLM system...

8. [A LIGHTWEIGHT, DOMAIN-ADAPTIVE MEMORY SYS](https://openreview.net/pdf?id=PLkhOUxkHQ) - Agentic memory: Learning unified long-term and short-term memory management for large language model...

9. [Memory-Aware Software Engineering Agents](https://openreview.net/pdf?id=WeXF1A3xY8) - We present a survey that synthesizes evidence from a feature-level analysis of ten production softwa...

10. [BELIEF ENGINE: BAYESIAN MEMORY FOR CONFIG](https://openreview.net/pdf/a37508bb551b24339d57ad79adbebeebe6840269.pdf) - ICLR 2026 Workshop on Memory for LLM-Based Agentic Systems. This architecture provides the reliabili...

11. [A Survey on the Evolution of LLM Agent Memory ...](https://arxiv.org/abs/2605.06716) - by J Luo · 2026 · Cited by 4 — this survey proposes a novel evolutionary framework for LLM agent mem...

12. [agentmemory/benchmark/COMPARISON.md at main - GitHub](https://github.com/rohitg00/agentmemory/blob/main/benchmark/COMPARISON.md) - #1 Persistent memory for AI coding agents based on real-world benchmarks - rohitg00/agentmemory

13. [LongMemEval-V2: Evaluating Long-Term Agent Memory ...](https://arxiv.org/abs/2605.12493) - Long-term memory is crucial for agents in specialized web environments, where success depends on rec...

14. [Rethinking Memory Mechanisms of Foundation Agents in the Second Half: A Survey](https://arxiv.org/abs/2602.06052) - The research of artificial intelligence is undergoing a paradigm shift from prioritizing model innov...

15. [agentmemory/benchmark/LONGMEMEVAL.md at main - GitHub](https://github.com/rohitg00/agentmemory/blob/main/benchmark/LONGMEMEVAL.md) - #1 Persistent memory for AI coding agents based on real-world benchmarks - rohitg00/agentmemory

16. [Evaluating Memory in LLM Agents via Incremental Multi-Turn ...](https://github.com/HUST-AI-HYZ/MemoryAgentBench) - Open source code for Paper: Evaluating Memory in LLM Agents via Incremental Multi-Turn Interactions ...

17. [Using LongMemEval to Improve Agent Memory](https://www.youtube.com/watch?v=FTokJt1ioeg&vl=de) - Sam Bhagwat, co-founder of Mastra and author of Principles of Building AI Agents, shares how they’ve...

18. [[2512.13564] Memory in the Age of AI Agents](https://arxiv.org/abs/2512.13564) - by Y Hu · 2025 · Cited by 75 — This work aims to provide an up-to-date landscape of current agent me...

