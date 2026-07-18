## Цель и постановка проблемы

`memory_mcp` — bi-temporal графовая память на SurrealDB с MCP-интерфейсом, включающая facts, edges, scoped namespaces, decay confidence и bi-temporal cutoff запросы. Текущая проблема — не качество хранения, а **дисциплина использования**: агент должен не просто иметь доступ к memory tools, а обязательно и фоново обращаться к ним на чтение и запись, не забывая об этом в длинных сессиях.

Аналогичные проекты (`HamzaFarhan/memory`, `g0t4/mcp-server-memory-file`, `t3ta/memory-bank-mcp-server`) решают эту проблему по-разному. Показательно, что `HamzaFarhan/memory` явно документирует ожидаемый lifecycle агента: «загрузить память в начале разговора → ссылаться на неё во время работы → обновлять память в конце разговора». Это协议, зафиксированный в промпте/инструкциях агента, а не полагание на память самого LLM о том, что у него есть memory tools.[^1][^2][^3]

`g0t4/mcp-server-memory-file` идёт дальше и явно проектирует двусторонний контракт: «когда начинается новый чат, Claude автоматически получает недавние memories (подмножество или все) ИЛИ может запросить memories… и затем использует их, чтобы влиять на ответы/tools». Это подтверждает тезис из предыдущего анализа: retrieval должен быть либо автоматическим при старте сессии, либо принудительно предписан в системном промпте, а не оставлен на решение модели.[^3]

## Что показывает research и community опыт

### Научная база

Работы по агентной памяти (Zep/Graphiti, A-Mem, Mem0) сходятся в том, что память — это управляемый жизненный цикл (observe → extract → retrieve → reflect), а не единичная операция поиска. Zep демонстрирует, что многоступенчатый retrieval (recall → rerank → context assembly) даёт выигрыш и в latency (до 90% сокращения), и в accuracy на temporal reasoning задачах по сравнению с full-context подходом.

### Опыт community

Обсуждения вокруг Mem0 показывают конкретные провалы, когда retrieval не дисциплинирован:
- ADD-only extraction в v3 может «поднимать» устаревшие или противоречивые факты для time-sensitive атрибутов — сообщество прямо просит supersession механизм.[^4]
- Запросы к памяти возвращают релевантные по семантике, но не самые свежие записи — старые сообщения перекрывают новые по важности.[^5]
- Обсуждение дедупликации и разрешения противоречий в масштабе — открытый вопрос даже для зрелого продукта.[^6]

Из простых файловых MCP-серверов community вынесла практический паттерн: **explicit contract в тексте инструкций агента** — когда именно вызывать `memory_add`, `memory_search`, когда обновлять память. Без этого текста агент либо не вызывает tools вообще, либо делает это хаотично.[^1][^3]

## План внедрения для memory_mcp (без кода)

### Этап 0. Зафиксировать memory-контракт в MCP tool descriptions

Каждый MCP tool должен иметь description, который не просто объясняет, что делает функция, а явно предписывает **когда** её вызывать. Это самый дешёвый и самый недооценённый рычаг — LLM-агенты в первую очередь ориентируются на текст tool description при решении, вызывать ли инструмент.

Конкретно для существующих операций `memory_mcp` (add_fact, assemble_context, find_intro_chain, explain, invalidate) нужно переписать descriptions так, чтобы они содержали императивные триггеры: «Call this BEFORE responding to any question about prior context», «Call this AFTER any decision, correction, or new fact is established in conversation», «Always call assemble_context at the start of a new task before planning».

### Этап 1. Ввести обязательный pre-action recall

Определить конкретные точки в жизненном цикле агента, где `assemble_context` (или новый lightweight recall tool) должен вызываться без вопросов:
- начало новой сессии/задачи;
- перед изменением кода в конкретном файле/модуле (repo-scoped recall);
- перед ответом на вопрос, который похож на уже обсуждавшуюся тему;
- при возврате к теме после переключения контекста.

Практический механизм — не полагаться только на решение модели, а зафиксировать это правило в системном промпте/инструкциях агента (`AGENTS.md`, `CLAUDE.md` или аналог), как это делают референсные проекты.[^3][^1]

### Этап 2. Ввести обязательный post-action write

Определить события, после которых `add_fact`/`add_observation` должны вызываться автоматически:
- принятое архитектурное решение;
- явное предпочтение или ограничение, высказанное пользователем;
- найденная и исправленная ошибка (с причиной);
- завершение значимого этапа задачи.

Ключевое отличие от текущей практики большинства простых MCP memory-серверов — запись не должна быть «дампом всего чата», а точечной операцией с явным поводом, иначе память быстро зарастёт шумом, как отмечено в анализе Mem0's ADD-only модели.[^4]

### Этап 3. Добавить supersession/contradiction workflow поверх bi-temporal модели

`memory_mcp` уже имеет техническую основу (t_valid/t_invalid) для этого, но она не задействована в operational contract. План:
- при `add_fact` для потенциально изменчивого факта — обязательная проверка существующих фактов той же entity/scope на конфликт;
- явные статусы: supports / refines / supersedes / contradicts / needs-review;
- отдельный MCP tool (или расширение `explain`) для явного возврата «этот факт устарел/оспорен, вот текущая версия».

Это прямой ответ на самую частую жалобу community на Mem0 — stale retrieval и contradiction handling.[^6][^5][^4]

### Этап 4. Ввести фоновые (background) операции

Три типа фоновых процессов, не завязанных на latency основного диалога:
- **Freshness scan** — периодическая проверка фактов на устаревание по decay threshold, без ожидания запроса от агента.
- **Consolidation** — синтез повторяющихся эпизодов/фактов в более устойчивые summary-факты или community-кластеры.
- **Reflection** — извлечение процедурных lessons («как в этом проекте принято делать X») из истории фактов и эпизодов.

Это должно работать как отдельный процесс/cron внутри `memory_mcp`, а не как MCP tool, вызываемый агентом — агент не должен «помнить» о необходимости консолидации.

### Этап 5. Явный memory audit / explain-режим

Расширить существующий `explain` (сейчас — заглушка) до реального provenance-трейсинга: почему конкретный факт был выбран в `assemble_context`, откуда он взялся, когда был создан/обновлён. Это даёт разработчику возможность быстро увидеть, действительно ли агент обращался к памяти и на основании чего принял решение — критично для дебага «забывчивости» агента.

### Этап 6. Session bootstrap protocol

По аналогии с `HamzaFarhan/memory`: зафиксировать явный протокол начала сессии — при инициализации MCP-соединения агент обязан выполнить один вызов, возвращающий сжатый context pack (последние релевантные факты + открытые контрадикции + repo-specific constraints), прежде чем переходить к выполнению задачи пользователя. Это снижает риск того, что агент вообще забудет, что у него есть долговременная память.[^1]

### Этап 7. Evaluation harness для проверки дисциплины

Ввести метрики, которые можно замерять на реальных сессиях:
- доля turns, где ожидался recall, но он не был вызван;
- доля значимых событий, не записанных в память;
- частота возврата stale/superseded фактов в `assemble_context`;
- задержка, добавляемая обязательным pre/post-action циклом.

Без этого харнесса невозможно отличить «агент реально использует память дисциплинированно» от «работает только в демо-сценарии».

## Итоговая последовательность приоритетов

| Приоритет | Этап | Цель |
|---|---|---|
| P0 | Переписать tool descriptions с императивными триггерами | Дешёвый рычаг для дисциплины вызовов |
| P0 | Session bootstrap protocol | Гарантия, что агент не забывает о памяти в начале сессии |
| P1 | Pre-action recall + post-action write контракт | Систематизация вызовов вместо хаотичных |
| P1 | Supersession/contradiction workflow | Решение проблемы stale/contradictory facts |
| P2 | Фоновые freshness/consolidation/reflection процессы | Снижение нагрузки на hot path |
| P2 | Реальный explain/provenance | Возможность дебага и доверия |
| P3 | Evaluation harness | Долгосрочный контроль качества дисциплины |

---

## References

1. [GitHub - HamzaFarhan/memory: MEMORY MCP](https://github.com/HamzaFarhan/memory) - MEMORY MCP. Contribute to HamzaFarhan/memory development by creating an account on GitHub.

2. [memory-bank-mcp-server/README.md at develop · t3ta/memory-bank-mcp-server](https://github.com/t3ta/memory-bank-mcp-server/blob/develop/README.md) - Contribute to t3ta/memory-bank-mcp-server development by creating an account on GitHub.

3. [mcp-server-memory-file/README.md at master · g0t4/mcp-server-memory-file](https://github.com/g0t4/mcp-server-memory-file/blob/master/README.md) - Attempt to replicate ChatGPT like memory (text file) for Claude (and other MCP clients) - g0t4/mcp-s...

4. [ADD-only extraction in v3 may surface stale/contradictory ...](https://github.com/mem0ai/mem0/issues/4956) - ADD-only extraction in v3 may surface stale/contradictory facts for time-sensitive attributes #4956....

5. [Searching Query · Issue #3236 · mem0ai/mem0](https://github.com/mem0ai/mem0/issues/3236) - When retrieving memories from Mem0, the results returned are relevant based on semantic similarity t...

6. [How does Mem0 handle memory deduplication and ...](https://github.com/mem0ai/mem0/discussions/4787) - How does Mem0 handle memory deduplication and contradiction resolution at scale?

