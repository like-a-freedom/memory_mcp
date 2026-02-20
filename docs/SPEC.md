---
**⚠️ DEPRECATED — This document is superseded by [/docs/MEMORY_SYSTEM_SPEC.md](/docs/MEMORY_SYSTEM_SPEC.md)**

**Date**: February 5, 2026  
**Reason**: Consolidated with architecture overview and Rust implementation plan into single source of truth.  
**Action**: Please refer to [MEMORY_SYSTEM_SPEC.md](../../docs/MEMORY_SYSTEM_SPEC.md) for current specification.

**Migration update (2026-02-19):** SurrealDB 2.x → 3.x migration validation was completed in `agent/rusty_memory_mcp` (schema/runtime fixes, deprecated SurrealQL replacements, full `fmt`/`clippy`/tests green). Current detailed status and changelog are maintained only in `MEMORY_SYSTEM_SPEC.md`.

**Embedded DB path fix (2026-02-20):**
- If `SURREALDB_DATA_DIR` is **not** set, embedded SurrealDB path defaults to `data/surrealdb` **relative to the running executable directory** (not process working directory).
- If `SURREALDB_DATA_DIR` **is** set, that path is used **as-is** (strict override, no implicit rewriting).
- Startup diagnostics: the service now emits a compact `startup.versions` Info log containing `client_version` (agent crate version) and `surrealdb_server_version` when available to aid compatibility/debugging.
- Regression coverage added in `agent/rusty_memory_mcp/src/config.rs` tests:
	- default path resolves under executable directory,
	- explicit absolute/relative custom paths are preserved unchanged.

---

Область и термины
Цель продукта — предоставить агентам и людям единый слой памяти/контекста, который собирается из многоканальных источников (почта, чаты, звонки, календарь, таски, финансы), преобразуется в граф фактов/отношений и подгружается в LLM по запросу с минимальным токен-бюджетом и низкой задержкой.
Документ следует общим принципам SRS (структура “введение → описание → требования → качества”) согласно ISO/IEC/IEEE 29148.
​

Определения (минимум):

Episode (Эпизод) — первичный “сырой” фрагмент источника (письмо, кусок транскрипта, сообщение) с ссылкой на источник и временем.
​

Entity (Сущность) — человек/компания/проект/сделка/объект, выделенный из эпизодов, с дедупликацией и алиасами.

Fact/Item (Факт/элемент памяти) — обещание, задача, метрика, решение, мнение и т. п., извлечённое из эпизода и связанное с сущностями.

Bi-temporal — хранение времени “когда было истинно” и времени “когда система узнала/записала”, для корректных исправлений/опровержений и аудита.
​

Community/Cluster (Комьюнити) — кластер плотносвязанных сущностей с агрегированным резюме, чтобы ускорять поиск и сборку контекста.
​

Пользователи и доступ
Роли пользователей:

Owner (личный контур): владелец памяти; полный доступ к личным источникам.

Org Admin: управляет орг-контуром, политиками, подключениями источников, ретеншном.

Team Member: доступ к командным областям (проекты/сделки/общие агенты).

HR/Finance: доступ к приватным доменам (зарплаты, счета) по строгим политикам.

Agent (служебная роль): доступ только через policy-bound токены/скоупы.

FR-AC (Access Control) — требования доступа:

FR-AC-01: Система MUST поддерживать уровни контекста: personal / team / org / private-domain (например HR/finance).

FR-AC-02: Каждому объекту памяти MUST быть назначен visibility_scope и policy_tags (например: hr.salary, deal.pipeline, personal.health).

FR-AC-03: Запросы на retrieval MUST фильтроваться политиками до выполнения LLM (никаких “пост-фильтров” ответов).

FR-AC-04: Доступ агентов MUST осуществляться через аутентификацию (JWT/внешний auth server) и ограничение области токенов (audience/claims), если используется SurrealDB Cloud/SurrealMCP.
​

FR-AC-05: Должны быть реализованы rate limits на слой MCP/шлюза (RPS/burst) для защиты от утечек/абьюза и несанкционированного “выкачивания” памяти.

FR-AC-06: Система MUST разделять personal и corporate контуры в разных namespaces одной базы данных.

FR-AC-07: Cross-scope references MUST разрешаться только через policy rules (явные allow/deny), с обязательным логированием.

FR-AC-08: Cross-scope retrieval MUST выполняться только после pre-check политик и scope-claims.

FR-AC-09: Система MUST вести неизменяемый execution/event log для всех MCP операций (who/what/when/args/result) с возможностью replay для отладки и аудита.
​

Функциональные требования
Интеграции и ingestion
FR-IN-01: Система MUST поддерживать подключение источников: email, chat (Telegram/Slack), календарь, tasks (Todo/Notion/Jira), файлы (PDF/Docs), звонки (аудио+транскрипт).

FR-IN-02: При появлении нового документа/события ingestion pipeline MUST запускаться автоматически (near-real-time) и повторно переиндексировать изменения по расписанию.

FR-IN-03: Для каждого входящего объекта MUST сохраняться “сырой эпизод” (не теряя текст/метаданные) и ссылка на оригинал (URI/ID/временной диапазон аудио).
​

FR-IN-04: Для каждого эпизода MUST фиксироваться t_ref (референс-время события) и t_ingested (когда добавлено в систему) для bi-temporal логики.

FR-IN-05: Идемпотентный ingest MUST использовать детерминированный episode_id на основе source_type, source_id, t_ref и scope.

FR-IN-06: Правила нормализации для источников и идентификаторов MUST быть задокументированы и применяться перед вычислением детерминированных ID (trim/normalize unicode, timezone normalization, canonicalization of email/case), чтобы избежать коллизий и обеспечить стабильность across re-ingests.
​

SurrealDB transports and protocols
FR-IN-07: Система MUST поддерживать SurrealDB transports: **RPC** (предпочтительный для production, typed RPC + CBOR), **HTTP** (stateless endpoints: /sql, import/export) и **CBOR** (binary encoding with SurrealDB custom tags) для эффективной и типобезопасной передачи данных.
FR-IN-08: Все взаимодействия через RPC/HTTP MUST логироваться в execution/event log (who/what/when/args/result) с указанием используемого транспорта и content-type (application/cbor или application/json).
FR-IN-09: Использование session variables (`vars`) в RPC MUST быть явным и включено в лог операции; поведение, зависящее от сессионного состояния, должно быть контролируемым и воспроизводимым.
FR-IN-10: Сериализация при использовании CBOR MUST использовать согласованные CBOR-теги SurrealDB для дат/IDs/decimal/uuid/geometry, чтобы обеспечить корректный round-trip и детерминизм.

SurrealDB storage backend (single source of truth)
FR-DB-01: Система MUST использовать **SurrealDB как единственный backend** хранения данных; для тестов допускается только in-memory режим SurrealDB без отдельного in-memory storage в MCP.
FR-DB-02: Все объекты памяти (Episode/Entity/Fact/Edge/Community) MUST сохраняться и читаться из SurrealDB, включая графовые связи.
FR-DB-03: Система MUST поддерживать схемы/миграции SurrealDB как код (DDL/версии) и воспроизводимое развёртывание.
FR-DB-04: Namespace/database MUST быть обязательными для выбора при старте сервиса; значения задаются через конфигурацию окружения.
FR-DB-05: Система MUST обеспечивать индексы в SurrealDB для retrieval: полнотекстовые, graph traversal и (при наличии) векторные.
FR-DB-06: Execution/event log MUST храниться в SurrealDB (append-only) или синхронизироваться туда для аудита.

FR-AC-10: Аутентификация/авторизация для HTTP/RPC MUST соответствовать требованиям FR-AC (JWT, scope/claims, ns/db headers при HTTP).

NFR-D-03: Любая операция, зависящая от RPC session state, MUST включать все релевантные session vars в параметры запроса и execution log, чтобы обеспечить детерминированный результат при повторении.

Извлечение сущностей и фактов
FR-EX-01: Система MUST извлекать сущности: Person, Company, Project, Deal, Product, Asset, Location (расширяемо).

FR-EX-02: Система MUST извлекать факты/элементы: Promise, Task, Metric, Decision, Opinion/Preference, Relationship (расширяемо).

FR-EX-03: Каждый факт MUST содержать: content (нормализованная формулировка), quote (дословная цитата), source_pointer (на эпизод и позицию), actors_involved, t_valid (когда было заявлено/истинно).

FR-EX-04: Для повышения качества извлечения SHOULD применяться “двухпроходная” схема (extract → self-check/reflection) для снижения галлюцинаций и пропусков.

Data model conventions: Для однозначности все схемные поля должны использовать согласованные имена: `entity_links[]` (список canonical entity ids) — эквивалент `actors_involved`; `source_episode` и `source_position` — указатели на эпизод и позицию в нём; `content` и `quote` — нормализованная и дословная цитата соответственно. Эти имена обязаны использоваться в API и skills.
​

Entity Resolution (дедупликация)
FR-ER-01: Система MUST поддерживать алиасы и слияние сущностей (например, “Митя/Дима/Dmitry Ivanov”).

FR-ER-02: Система MUST обеспечивать гибридную дедупликацию: (a) embedding similarity + (b) текстовые признаки + (c) проверка LLM на основании контекста эпизода.

FR-ER-03: Система MUST сохранять историю слияний (merge log): кто/что/когда/почему объединено, с возможностью отката (split).

FR-ER-04: После merge все факты/связи MUST ссылаться на canonical entity, сохраняя provenance.

FR-ER-05: Разрешение алиасов MUST быть детерминистичным (точное совпадение → canonical entity, далее стабильные tie-break правила).

Граф отношений (контекстный граф)
FR-GR-01: Система MUST хранить граф: Nodes (Entities, Episodes, Facts, Communities) и Edges (mentions, promised_by, assigned_to, related_to, same_as, derived_from, etc.).
​

FR-GR-02: Каждое ребро/факт MUST иметь темпоральные атрибуты и provenance (источник), чтобы обеспечить объяснимость (“почему агент так решил”).
​

FR-GR-03: Система MUST поддерживать “комьюнити/кластеры” сущностей и хранить их summaries для ускорения retrieval и обзора организационного контекста.

FR-GR-04: Каждое ребро MUST содержать метаданные: `strength`, `confidence`, `provenance`, `t_valid`, `t_invalid` и опциональные `weight`/`temporal_weight` для ранжирования.

FR-GR-05: Edges MUST поддерживать bi-temporal атрибуты и инвалидацию: при добавлении нового противоречащего ребра старые ребра должны помечаться `t_invalid` (см. Edge Invalidation правила в FR-TM).
​

Темпоральность: устаревание и инвалидация
FR-TM-01: Система MUST поддерживать decay (устаревание доверия) по умолчанию с настраиваемым half-life по типам фактов (год для метрик/обещаний и т. п.).

FR-TM-02: Система MUST поддерживать инвалидацию фактов (supersede) при появлении нового противоречащего факта, а не только “плавное забывание”.
​

FR-TM-03: Система MUST реализовать bi-temporal модель: хранить время истинности факта (T) и время транзакции/ингеста (T′) для аудита, корректировок задним числом и правильных ответов “на тот момент”.
​

FR-TM-04: Retrieval MUST уметь отвечать “as-of” (срез на дату): показать контекст на дату встречи/письма.

FR-TM-05: При появлении нового факта/метрики, конфликтующего с существующими, система MUST выполнять проверку на contradiction (LLM-assisted или rule-based) и при подтверждении устанавливать `t_invalid` старых фактов (explicit invalidation), сохраняя provenance.

Сборка контекста (Context Assembly)
FR-CA-01: Система MUST собирать контекст под задачу/вопрос динамически: возвращать top-K фактов/узлов с цитатами и ссылками на источники.

FR-CA-02: Система MUST поддерживать гибридный retrieval: векторный (semantic), полнотекстовый и graph traversal (BFS/ограниченные hop’ы) для “социальных” запросов и цепочек знакомств.
​

FR-CA-03: Система MUST обеспечивать токен-бюджетирование: лимиты на количество фактов, длину цитат, уровни детализации (brief/standard/deep).

FR-CA-04: Результат сборки MUST включать: (a) факты, (b) confidence score, (c) rationale (почему включено), (d) provenance.

FR-CA-05: Результаты retrieval MUST быть детерминированно упорядочены (stable sort + tie-break по времени и id).

FR-CA-06: Система MUST поддерживать определение и управление analyzers и индексами для full-text search и vector indexes; это включает возможность задавать tokenizers, filters и analyzer functions для доменных текстов.

FR-CA-07: Для уменьшения вариативности запросов агентам MUST быть предоставлен набор канонических query templates и typed memory-skills (e.g., `Q_ACTOR_BY_ALIAS`, `Q_PROMISES`, `add_fact`, `invalidate_fact`, `get_briefing`). Skills должны валидировать вход с помощью JSON Schema.

Агентские сценарии (Skills/Flows)
FR-AG-01: Система MUST предоставлять “skills” как операции над памятью: ingest_document, extract_entities, resolve_entity, assemble_context, create_task, send_message_draft, schedule_meeting, update_metric.

FR-AG-02: Skills MUST быть доступны через MCP-интерфейс (stdio/http/socket), чтобы IDE/ассистенты могли вызывать их унифицированно.
​

FR-AG-03: Система MUST поддерживать human-in-the-loop режим: подтверждение отправки писем и подтверждение изменения статуса обещаний/задач. Для слияния сущностей (`resolve_entity`) система MUST поддерживать опцию `require_confirmation` (если `true` — запрос подтверждения); по умолчанию разрешается автоматическое слияние без dry‑run, если явно не задано иное, при этом все действия логируются в merge log.

FR-AG-04: Система MUST поддерживать типы агентов: личные, командные (2 владельца), коллективные (видимость результата группе), как минимум на уровне scope/ACL.

UI/UX (минимум для “графа контекста”)
FR-UX-01: UI MUST позволять выбрать контрагента/партнёра/проект и получить ответы:

“Кто кому что обещал? выполнено ли?”

“Какие метрики/сделки озвучивали и как менялись?”

“Какие задачи для меня/команды, приоритет, дедлайн?”

FR-UX-02: Каждый ответ MUST иметь цитату и ссылку на первоисточник (эпизод/документ/таймкод).

FR-UX-03: UI MUST позволять запускать следующий flow (“найди интро к OpenAI → сгенерируй драфт письма”) из контекстного экрана.

Нефункциональные требования (качество)
NFR-P-01 (Latency): p95 сборки контекста SHOULD быть ≤ 100–300 ms для типовых запросов, при условии заранее построенных индексов (вектор/текст/граф), а “raw search по эпизодам” допускается медленнее.

NFR-P-02 (Scalability): система MUST поддерживать рост до “10 людей + 10,000 агентов” через изоляцию скоупов, кэширование, rate limiting и ограничение глубины traversal.

NFR-R-01 (Reliability): ingestion и extraction MUST быть идемпотентными (повторный прогон не создаёт дубликаты).

NFR-S-01 (Security): MUST быть строгая сегрегация данных и управление токенами/аутентификацией на MCP-уровне; транспорт MCP должен поддерживать локальные и сетевые режимы (stdio/http/unix socket) в зависимости от модели деплоя.
​

NFR-A-01 (Auditability): MUST храниться полный provenance: “какой эпизод породил какой факт”, плюс история инвалидаций/обновлений (bi-temporal).
​


NFR-D-01 (Determinism): Любые MCP ответы MUST быть детерминированными (без случайности, со стабильным порядком).

NFR-D-02 (Determinism): Идентификаторы объектов MUST быть детерминированными и коллизионно-устойчивыми; конфликт разрешается предсказуемо.
NFR-M-01 (Maintainability): все схемы, политики и пайплайны MUST быть управляемы как код (Git), с миграциями и версионированием.

Данные, интерфейсы и критерии приёмки
Объекты данных (минимальная модель)
Объект	Обязательные поля	Критерий приёмки
Episode	id, source_type, source_id, content, t_ref, t_ingested 
​	По любому факту можно открыть исходный эпизод и увидеть точную цитату/фрагмент.
Entity	id, type, canonical_name, aliases[], embedding, merge_history[]	Поиск по любому алиасу возвращает canonical entity, merge/split отражается в истории.
Fact/Item	id, type, content, quote, entity_links[], t_valid, t_invalid?, confidence, source_episode 
​	Любой факт имеет цитату и валидные временные атрибуты, корректно исчезает/понижается при устаревании/инвалидации.
Community	id, member_entities[], summary, updated_at 
​	Добавление новых сущностей обновляет кластер/summary без полного пересчёта всего графа.
MCP/Service API (логические методы)
API-01 ingest(episode) → episode_id

API-02 extract(episode_id) → {entities, facts, links}

API-03 resolve(entity_candidate) → canonical_entity_id (+ merge actions)

API-04 invalidate(fact_id, reason, t_invalid) → ok

API-05 assemble_context(query, scope, as_of, budget) → context_pack

API-06 explain(context_pack) → ссылки на эпизоды/цитаты

SurrealMCP и SurrealDB transports MUST поддерживать прод-настройки: аутентификацию (JWT/auth server), лимиты RPS/burst и разные транспорты (stdio/http/socket/RPC/HTTP). API-01..API-06 MUST быть доступны через RPC/HTTP и, где уместно, принимать/возвращать CBOR-encoded payloads; все вызовы должны логироваться в execution/event log с информацией о транспорте и content-type.
​

Acceptance tests (high-level)
AT-01: После добавления письма с обещанием “сделаю до пятницы”, система показывает обещание у контрагента, с цитатой и ссылкой на письмо.

AT-02: Если через 6 месяцев добавлено новое письмо “ARR вырос до $3M”, старый факт “$1M ARR” становится invalidated (или резко теряет confidence), а UI показывает динамику метрик.

AT-03: Пользователь без hr.salary не может извлечь/увидеть зарплатные факты ни через UI, ни через агентский skill.

AT-04: Запрос “кто может познакомить с OpenAI” возвращает цепочку через граф traversal (2–3 hop) и подтверждается источниками.

AT-05: Проверка CBOR round-trip: datetime/record id/decimal сохраняются и восстанавливаются без потерь при RPC+CBOR.

AT-06: Запрос через RPC с явно указанными `vars` должен логироваться; повторный вызов с теми же `vars` даёт детерминированный результат.

Статусы требований
| Требование | Статус |
| --- | --- |
| FR-AC-01 | сделано |
| FR-AC-02 | сделано |
| FR-AC-03 | сделано |
| FR-AC-04 | сделано |
| FR-AC-05 | сделано |
| FR-AC-06 | сделано |
| FR-AC-07 | сделано |
| FR-AC-08 | сделано |
| FR-IN-01 | сделано |
| FR-IN-02 | сделано |
| FR-IN-03 | сделано |
| FR-IN-04 | сделано |
| FR-IN-05 | сделано |
| FR-IN-06 | сделано |
| FR-IN-07 | сделано |
| FR-IN-08 | сделано |
| FR-IN-09 | сделано |
| FR-IN-10 | сделано |
| FR-DB-01 | сделано |
| FR-DB-02 | сделано |
| FR-DB-03 | сделано |
| FR-DB-04 | сделано |
| FR-DB-05 | сделано |
| FR-DB-06 | сделано |
| FR-AC-10 | сделано |
| FR-EX-01 | сделано |
| FR-EX-02 | сделано |
| FR-EX-03 | сделано |
| FR-EX-04 | сделано |
| FR-ER-01 | сделано |
| FR-ER-02 | сделано |
| FR-ER-03 | сделано |
| FR-ER-04 | сделано |
| FR-ER-05 | сделано |
| FR-GR-01 | сделано |
| FR-GR-02 | сделано |
| FR-GR-03 | сделано |
| FR-TM-01 | сделано |
| FR-TM-02 | сделано |
| FR-TM-03 | сделано |
| FR-TM-04 | сделано |
| FR-CA-01 | сделано |
| FR-CA-02 | сделано |
| FR-CA-03 | сделано |
| FR-CA-04 | сделано |
| FR-CA-05 | сделано |
| FR-AG-01 | сделано |
| FR-AG-02 | сделано |
| FR-AG-03 | сделано |
| FR-AG-04 | сделано |
| FR-UX-01 | сделано |
| FR-UX-02 | сделано |
| FR-UX-03 | сделано |
| NFR-P-01 | сделано |
| NFR-P-02 | сделано |
| NFR-R-01 | сделано |
| NFR-S-01 | сделано |
| NFR-A-01 | сделано |
| NFR-M-01 | сделано |
| NFR-D-01 | сделано |
| NFR-D-02 | сделано |
| API-01 | сделано |
| API-02 | сделано |
| API-03 | сделано |
| API-04 | сделано |
| API-05 | сделано |
| API-06 | сделано |
| AT-01 | сделано |
| AT-02 | сделано |
| AT-03 | сделано |
| AT-04 | сделано |
| AT-05 | сделано |
| AT-06 | сделано |
| FR-AC-09 | сделано |
| FR-GR-04 | сделано |
| FR-GR-05 | сделано |
| FR-TM-05 | сделано |
| FR-CA-06 | сделано |
| FR-CA-07 | сделано |
| NFR-AO-01 | сделано |
| NFR-E-01 | сделано |
| NFR-D-03 | сделано |