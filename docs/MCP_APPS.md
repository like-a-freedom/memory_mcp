# SRS v0.2 — MCP Apps для memory_mcp

> **Статус документа:** исторический draft раннего UI-first дизайна.
> Актуальный shipped public MCP-контракт описан в `README.md` и `docs/superpowers/plans/2026-03-30-mcp-apps.md`: публичная поверхность упрощена до `open_app` + `app_command` + `resources/read` / `resources/templates/list`.

**Версия:** 0.2
**Дата:** 2026-03-30
**Статус:** Draft
**Область:** MCP Apps UI layer поверх memory_mcp (ветка memory-sota-improvements)

***

## 1. Введение

### 1.1 Назначение документа

Документ описывает функциональные и нефункциональные требования к пяти интерактивным MCP Apps для `memory_mcp`. Каждый App — это интерактивный UI-компонент, возвращаемый tool call и рендеримый клиентом (VS Code Copilot Chat, Claude Desktop) прямо в диалоге без открытия внешнего браузера.

### 1.2 Область действия

В scope входят:

| ID | Компонент | Назначение | Приоритет |
|---|---|---|---|
| APP-01 | Memory Inspector | Просмотр entity / fact / episode с provenance и temporal state | P0 |
| APP-02 | Temporal Diff | Сравнение состояния памяти между двумя `as_of` | P0 |
| APP-03 | Ingestion Review | Human-in-the-loop проверка кандидатов перед commit | P1 |
| APP-04 | Lifecycle Console | Decay, archival candidates, stale memory hygiene | P1 |
| APP-05 | Graph Path Explorer | Просмотр и объяснение пути между сущностями | P2 |

Вне scope: внешние dashboards, экспорт в третьи системы, real-time multi-user коллаборация.

### 1.3 Определения

- **App** — MCP App, интерактивный UI-компонент, возвращаемый tool call.
- **AppSession** — серверная сессия, открытая вызовом `open_*` tool. Хранит контекст и draft state.
- **Draft** — временная область хранения для APP-03, не входящая в основной store до `commit`.
- **Scope** — namespace изоляции данных в memory_mcp (`AccessContext.allowed_scopes`).
- **Valid-time** — время, когда факт был истинным в реальном мире (`t_valid`, `t_invalid`).
- **Transaction-time** — время, когда факт был записан в систему (`t_ingested`, `t_invalid_ingested`).
- **Decayed confidence** — вычисляемый на лету показатель релевантности факта с учётом времени.
- **AccessContext** — структура авторизации: `caller_id`, `allowed_scopes`, `allowed_tags`, `cross_scope_allow`.

### 1.4 Ограничения и предположения

- **Статический бинарь:** UI-бандл каждого App должен быть встроен в бинарь или поставляться как локальный ресурс. CDN, внешние JS-зависимости и рантайм-загрузка запрещены.
- **Без внешних интеграций:** сервер не делает исходящих HTTP-запросов, кроме embedding provider (опционально).
- **Degradation:** если клиент не поддерживает MCP Apps capability, все `open_*` tools должны возвращать корректный текстовый fallback.
- **Схема БД:** все новые сущности (`draft_ingestion`, `draft_item`, `app_session`) добавляются отдельными миграциями поверх существующей схемы.

***

## 2. Общие требования

### 2.1 Архитектурные принципы

- **Разделение open/apply:** каждый App открывается `open_*` tool (read-only, возвращает UI-модель), а мутирующие действия выполняются отдельными `apply_*` / named tools. UI не содержит скрытой бизнес-логики.
- **Server-side state:** состояние App живёт на сервере как AppSession. UI — thin client, не хранит canonically значимых данных.
- **AccessContext-транзитивность:** access context из вызова `open_*` транслируется во все вложенные service calls в рамках сессии.

### 2.2 Функциональные требования

- **FR-COM-01:** Сервер объявляет UI resources через `ui://memory/{app_id}` в capability response.
- **FR-COM-02:** Каждый App имеет read-model, action-model и error-model.
- **FR-COM-03:** Все write-операции выполняются через явные server tools, не через неявные UI-side мутации.
- **FR-COM-04:** Каждый App передаёт в UI только данные, разрешённые `scope` и `policy_tags` текущего AccessContext.
- **FR-COM-05:** Каждое пользовательское действие, меняющее память, пишет запись в `event_log`.
- **FR-COM-06:** Максимальное число одновременных AppSession per scope — configurable, default 10. При превышении сервер возвращает ошибку `SESSION_LIMIT_EXCEEDED`.
- **FR-COM-07:** TTL idle-сессии — configurable, default 1h. По истечении TTL сессия переходит в статус `expired`; повторные запросы к ней возвращают `SESSION_EXPIRED`.
- **FR-COM-08:** Все Apps реализуют `close_session(session_id)` tool. Закрытие idle-сессии явно; закрытие draft-сессии (APP-03) без commit — это cancel без записи данных в store.
- **FR-COM-09:** `open_*` tools принимают опциональный параметр `access?: AccessContext`. Если не передан — используется дефолтный контекст из конфига сервера.
- **FR-COM-10:** Для destructive и необратимых actions требуется явное подтверждение: параметр `confirmed: true` или отдельный confirmation step в UI.

### 2.3 Нефункциональные требования

- **NFR-COM-01:** Сервер не требует внешнего backend, кроме самого `memory_mcp`.
- **NFR-COM-02:** UI-бандл встроен в бинарь или поставляется локально, без CDN.
- **NFR-COM-03:** Первый экран любого App открывается ≤ 2 секунд на памяти до 10k facts при embedded-режиме.
- **NFR-COM-04:** При недоступности Apps-capability сервер деградирует в текстовый fallback. Формат fallback определён отдельно для каждого App в §2.5.
- **NFR-COM-05:** Все destructive actions помечены визуально и требуют подтверждения.
- **NFR-COM-06:** Все bulk operations идемпотентны.

### 2.4 Модель данных AppSession

```
AppSession {
  session_id:    string (UUID)
  app_id:        "inspector" | "diff" | "ingestion" | "lifecycle" | "graph"
  scope:         string
  access:        object FLEXIBLE  // serialized AccessPayload (AccessContext в памяти, AccessPayload при сериализации)
  target:        object (app-specific)
  state:         "loading" | "ready" | "empty" | "error" | "stale" | "expired"
  created_at:    datetime
  last_active:   datetime
  ttl_seconds:   number
}

AppActionResult {
  ok:               bool
  message:          string
  refresh_required: bool
  updated_targets:  string[]
  task_id?:         string  // для async operations
}

AppError {
  code:         string
  user_message: string
  debug_hint:   string
}
```

### 2.5 Fallback output contracts

Текстовый fallback для клиентов без Apps-capability:

| App | Fallback format |
|---|---|
| APP-01 | JSON с полями: `fact_id/entity_id/episode_id`, `content`, `state`, `t_valid`, `confidence`, `provenance` |
| APP-02 | JSON с тремя ключами: `added[]`, `removed[]`, `changed[]`; каждый элемент содержит `id`, `type`, `summary` |
| APP-03 | JSON с `draft_id`, `candidates: {entities[], facts[], edges[]}` (кандидаты на commit, не финальные записи), `pending_actions: "call commit_ingestion_review to finalize"` |
| APP-04 | JSON с `low_confidence[]`, `archival_candidates[]`, `archived_episodes[]`, `stale_communities[]`, каждый с `id` и `reason` |
| APP-05 | JSON с `path[]` (ordered list of nodes/edges), `path_found: bool`, `reason_if_empty` |

***

## 3. APP-01 — Memory Inspector

### 3.1 Назначение

Быстрый read-oriented просмотр одной единицы памяти (entity, fact, episode) с provenance, temporal state и inline actions без чтения сырого JSON.

### 3.2 Tool contract

```
open_memory_inspector(
  scope:        string,
  target_type:  "entity" | "fact" | "episode",
  target_id:    string,
  as_of?:       datetime,        // default: now()
  page_size?:   number,          // default: 20
  cursor?:      string,
  access?:      AccessContext
) → AppSession

refresh_memory_inspector(session_id: string) → AppSession

open_related_timeline(
  session_id:   string,
  target_type:  string,
  target_id:    string
) → AppSession  // APP-01 в timeline mode

invalidate_fact(
  session_id:   string,
  fact_id:      string,
  reason?:      string,
  confirmed:    bool
) → AppActionResult

archive_episode(
  session_id:   string,
  episode_id:   string,
  reason?:      string,
  confirmed:    bool
) → AppActionResult

copy_record_id(
  session_id:   string,
  target_id:    string
) → AppActionResult

close_session(session_id: string) → AppActionResult
```

### 3.3 Функциональные требования

- **FR-INS-01:** App поддерживает три режима: `entity`, `fact`, `episode`.
- **FR-INS-02:** Режим `entity` показывает: canonical name, aliases, entity_type, связанные active facts (пагинированно), связанные edges с temporal validity, communities.
- **FR-INS-03:** Режим `fact` показывает: content, quote, source_episode, confidence, decayed_confidence, provenance, bi-temporal поля (`t_valid`, `t_ingested`, `t_invalid`, `t_invalid_ingested`), active/inactive state.
- **FR-INS-04:** Режим `episode` показывает: source_type, source_id, `t_ref`, `t_ingested`, status, archived_at, список фактов эпизода (пагинированно).
- **FR-INS-05:** App показывает provenance trace: минимум `source_episode`, `ingested_at`, `invalidated_at`, `policy_tags`, `caller_id` (если есть в provenance).
- **FR-INS-06:** Доступные actions: `invalidate fact`, `archive episode`, `open timeline`, `copy record id`.
- **FR-INS-07:** App показывает state badge: `active`, `invalidated`, `archived`, `future-valid` (t_valid > now), `not-yet-ingested` (t_ingested > now).
- **FR-INS-08:** При числе связанных фактов > `page_size` App поддерживает cursor-based пагинацию.

### 3.4 Нефункциональные требования

- **NFR-INS-01:** Первичный экран требует ≤ 3 server round trips.
- **NFR-INS-02:** При частичной недоступности связанных данных основной объект отображается.
- **NFR-INS-03:** UI поддерживает компактный режим для inline chat rendering.

### 3.5 Acceptance criteria

| ID | Сценарий | Ожидаемый результат |
|---|---|---|
| AC-INS-01 | Открыть fact | Temporal поля и provenance видны без переходов |
| AC-INS-02 | Нажать `invalidate` | Появляется confirmation; после подтверждения badge меняется на `invalidated` |
| AC-INS-03 | Entity с 100+ facts | Отображается первая страница, доступна пагинация |
| AC-INS-04 | Клиент без Apps-capability | Возвращается JSON fallback согласно §2.5 |
| AC-INS-05 | TTL истёк | Повторный запрос возвращает `SESSION_EXPIRED`, клиент видит понятный empty state |

***

## 4. APP-02 — Temporal Diff

### 4.1 Назначение

Сравнение состояния памяти между двумя точками времени для демонстрации bi-temporal value prop.

### 4.2 Tool contract

```
open_temporal_diff(
  scope:          string,
  target_type:    "scope" | "entity" | "episode",
  target_id?:     string,
  as_of_left:     datetime,
  as_of_right:    datetime,
  time_axis:      "valid" | "transaction" | "both",  // default: "valid"
  view?:          "summary" | "detailed",             // default: "summary"
  filters?:       DiffFilters,
  access?:        AccessContext
) → AppSession

DiffFilters {
  only_facts?:            bool
  only_edges?:            bool
  only_active?:           bool
  only_policy_visible?:   bool
}

export_temporal_diff(
  session_id:   string,
  format:       "json" | "markdown"
) → AppActionResult

open_memory_inspector_from_diff(
  session_id:   string,
  target_id:    string,
  target_type:  string
) → AppSession  // APP-01

close_session(session_id: string) → AppActionResult
```

### 4.3 Функциональные требования

- **FR-DIFF-01:** App сравнивает `scope` целиком, конкретную `entity` или конкретный `episode`.
- **FR-DIFF-02:** Различия разложены по трём группам: `added`, `removed`, `changed`.
- **FR-DIFF-03:** Для `changed` показывается field-level diff по: `content`, `confidence`, `t_invalid`, `provenance`. Прочие поля — опционально.
- **FR-DIFF-04:** Поддерживаются два режима оси времени: `valid` — по `t_valid`/`t_invalid`; `transaction` — по `t_ingested`/`t_invalid_ingested`; `both` — обе оси с явной разметкой.
- **FR-DIFF-05:** Из любого элемента diff можно открыть карточку в Memory Inspector.
- **FR-DIFF-06:** Поддерживаются фильтры `DiffFilters`.
- **FR-DIFF-07:** Default view `summary` показывает счётчики per group; `detailed` — раскрывает каждый элемент.

### 4.4 Нефункциональные требования

- **NFR-DIFF-01:** При diff > 1000 записей App показывает summary первым, details — по lazy expand.
- **NFR-DIFF-02:** Сравнение детерминировано при одинаковых `scope`, `target_id`, `as_of_*`, `time_axis`.

### 4.5 Acceptance criteria

| ID | Сценарий | Ожидаемый результат |
|---|---|---|
| AC-DIFF-01 | Факт активен в left, инвалидирован в right | Попадает в `removed` |
| AC-DIFF-02 | Изменился `confidence`, content не изменился | Попадает в `changed` |
| AC-DIFF-03 | Нажать на строку diff | Открывается Memory Inspector для этого объекта |
| AC-DIFF-04 | Режим `transaction` vs `valid` | Разные результаты при restatement-сценариях |
| AC-DIFF-05 | Клиент без Apps-capability | JSON fallback согласно §2.5 |

***

## 5. APP-03 — Ingestion Review

### 5.1 Назначение

Human-in-the-loop проверка кандидатов (entities, facts, edges) перед записью в основной store.

### 5.2 Модель данных Draft

```sql
-- Миграция: 011_ingestion_draft.surql
DEFINE TABLE draft_ingestion SCHEMAFULL;
DEFINE FIELD draft_id         ON draft_ingestion TYPE string;
DEFINE FIELD scope            ON draft_ingestion TYPE string;
DEFINE FIELD status           ON draft_ingestion TYPE string;
  -- "open" | "committed" | "cancelled" | "expired"
DEFINE FIELD created_at       ON draft_ingestion TYPE datetime;
DEFINE FIELD expires_at       ON draft_ingestion TYPE datetime;
DEFINE FIELD access           ON draft_ingestion TYPE object FLEXIBLE;

DEFINE TABLE draft_item SCHEMAFULL;
DEFINE FIELD draft_id         ON draft_item TYPE string;
DEFINE FIELD item_id          ON draft_item TYPE string;
DEFINE FIELD item_type        ON draft_item TYPE string;
  -- "entity" | "fact" | "edge"
DEFINE FIELD status           ON draft_item TYPE string;
  -- "pending" | "approved" | "rejected" | "edited"
DEFINE FIELD payload          ON draft_item TYPE object FLEXIBLE;
DEFINE FIELD original_payload ON draft_item TYPE object FLEXIBLE;
DEFINE FIELD confidence       ON draft_item TYPE number;
DEFINE FIELD rationale        ON draft_item TYPE string;
DEFINE FIELD source_snippet   ON draft_item TYPE string;
```

### 5.3 Tool contract

```
open_ingestion_review(
  scope:              string,
  source_text?:       string,
  draft_episode_id?:  string,
  access?:            AccessContext,
  ttl_seconds?:       number       // default: 86400 (24h)
) → AppSession  // содержит draft_id

get_draft_summary(
  session_id: string
) → { draft_id, total, by_type, by_status, expires_at }

approve_ingestion_items(
  session_id:  string,
  item_ids:    string[]
) → AppActionResult

reject_ingestion_items(
  session_id:  string,
  item_ids:    string[],
  reason?:     string
) → AppActionResult

edit_ingestion_item(
  session_id:  string,
  item_id:     string,
  patch: {
    content?:         string,
    canonical_name?:  string,
    aliases?:         string[],
    relation?:        string,
    confidence?:      number,
    policy_tags?:     string[]
  }
) → AppActionResult

bulk_approve_by_type(
  session_id:  string,
  item_type:   "entity" | "fact" | "edge"
) → AppActionResult

bulk_reject_low_confidence(
  session_id:   string,
  threshold:    number
) → AppActionResult

commit_ingestion_review(
  session_id:  string,
  confirmed:   bool       // обязателен
) → AppActionResult

cancel_ingestion_review(
  session_id:  string
) → AppActionResult

close_session(session_id: string) → AppActionResult
```

### 5.4 Функциональные требования

- **FR-ING-01:** App показывает кандидатов трёх типов: `entities`, `facts`, `edges`.
- **FR-ING-02:** Для каждого кандидата отображаются: confidence, extraction rationale, source snippet, editable fields.
- **FR-ING-03:** Пользователь может approve / reject / edit каждый item. Edit обратим до commit (`original_payload` сохраняется в `draft_item`).
- **FR-ING-04:** До вызова `commit_ingestion_review` с `confirmed: true` никакие данные не попадают в основной store.
- **FR-ING-05:** Порядок commit внутри транзакции: entities → facts → edges. Edge-кандидат считается валидным, если оба его endpoint (`from/to`) существуют в основном store **или** присутствуют среди approved entity-кандидатов в том же draft.
- **FR-ING-06:** App дедуплицирует кандидатов по детерминированным ID и явно показывает merge conflict при совпадении с существующей записью в store.
- **FR-ING-07:** Поддерживаются bulk actions: `approve by type`, `reject low confidence`.
- **FR-ING-08:** Перед commit сервер формирует `commit_summary` (число entities/facts/edges к записи) и требует `confirmed: true`.
- **FR-ING-09:** Draft TTL — configurable, default 24h. По истечении draft переходит в `expired`, commit невозможен, возвращается `DRAFT_EXPIRED`.
- **FR-ING-10:** При старте сервера и по расписанию выполняется cleanup expired drafts (удаление из `draft_ingestion` и `draft_item`).

### 5.5 Нефункциональные требования

- **NFR-ING-01:** Draft хранится в таблицах `draft_ingestion` / `draft_item`, изолированных от основного store.
- **NFR-ING-02:** Потеря UI-сессии не инициирует автоматический commit.
- **NFR-ING-03:** Все edit-операции обратимы до commit.
- **NFR-ING-04:** `cancel_ingestion_review` полностью удаляет draft без записи в основной store.

### 5.6 Acceptance criteria

| ID | Сценарий | Ожидаемый результат |
|---|---|---|
| AC-ING-01 | Reject edge, commit | Edge отсутствует в store после commit |
| AC-ING-02 | Edit fact content, commit | В store лежит отредактированная версия |
| AC-ING-03 | Два кандидата с одинаковым deterministic ID | App предлагает merge вместо двойной записи |
| AC-ING-04 | Edge с from_id = новая entity из того же draft | Edge проходит валидацию FR-ING-05 |
| AC-ING-05 | commit без `confirmed: true` | Ошибка `CONFIRMATION_REQUIRED`, commit не выполняется |
| AC-ING-06 | TTL истёк | `commit_ingestion_review` возвращает `DRAFT_EXPIRED` |
| AC-ING-07 | Cancel без commit | Данные в основном store не изменились |

***

## 6. APP-04 — Lifecycle Console

### 6.1 Назначение

Операционное управление lifecycle памяти: decay, archival candidates, community rebuild.

### 6.2 Tool contract

```
open_lifecycle_console(
  scope:      string,
  filters?:   LifecycleFilters,
  access?:    AccessContext
) → AppSession

LifecycleFilters {
  min_confidence?:      number
  max_confidence?:      number
  inactive_days?:       number
  include_archived?:    bool
}

archive_candidates(
  session_id:     string,
  candidate_ids:  string[],
  dry_run?:       bool,     // default: false
  confirmed:      bool
) → AppActionResult  // содержит task_id если объём > threshold

restore_archived(
  session_id:    string,
  episode_ids:   string[],
  confirmed:     bool
) → AppActionResult

recompute_decay(
  session_id:    string,
  target_ids?:   string[],  // если пусто — весь scope
  dry_run?:      bool
) → AppActionResult  // task_id для async

rebuild_communities(
  session_id:    string,
  dry_run?:      bool,
  confirmed:     bool
) → AppActionResult  // task_id для async

get_lifecycle_task_status(
  task_id: string
) → {
  task_id:    string,
  status:     "pending" | "running" | "done" | "failed",
  progress?:  { processed: number, total: number },
  result?:    AppActionResult,
  error?:     AppError
}

close_session(session_id: string) → AppActionResult
```

### 6.3 Функциональные требования

- **FR-LIFE-01:** App показывает четыре списка: `low_confidence facts`, `archival candidates`, `archived episodes`, `stale communities`.
- **FR-LIFE-02:** Для каждого кандидата показывается причина попадания в список (confidence threshold, inactive duration, last_accessed).
- **FR-LIFE-03:** Поддерживаются bulk actions: archive, restore, recompute, rebuild.
- **FR-LIFE-04:** App визуально различает `derived decay` (автоматический) и `manual status` (явно выставленный через invalidate/archive).
- **FR-LIFE-05:** App показывает impact preview: сколько facts исчезнет из active retrieval после archive.
- **FR-LIFE-06:** Все bulk actions поддерживают `dry_run: true`. В dry_run данные не изменяются, возвращается preview результата.
- **FR-LIFE-07:** Async tasks записывают прогресс в таблицу `task` и немедленно возвращают `task_id`. Статус доступен через `get_lifecycle_task_status`.
- **FR-LIFE-08:** `rebuild_communities` запускается только явно через Lifecycle Console, не фоновым процессом без контроля.

### 6.4 Нефункциональные требования

- **NFR-LIFE-01:** Все bulk actions идемпотентны. Повторный вызов с теми же `candidate_ids` не создаёт дублей.
- **NFR-LIFE-02:** Для операций с > 100 записями используется async task с polling. Sync-ответ запрещён.
- **NFR-LIFE-03:** Необратимые actions (archive, rebuild) визуально помечены и требуют confirmation.

### 6.5 Acceptance criteria

| ID | Сценарий | Ожидаемый результат |
|---|---|---|
| AC-LIFE-01 | dry_run archive | Возвращает список episodes и число затронутых facts, данные не изменились |
| AC-LIFE-02 | rebuild_communities | После rebuild stale communities исчезают из соответствующего списка |
| AC-LIFE-03 | Restore archived episode | Episode переходит в active, факты становятся доступны для retrieval |
| AC-LIFE-04 | recompute_decay > 100 targets | Возвращает task_id, статус доступен через get_lifecycle_task_status |
| AC-LIFE-05 | Повторный archive с теми же ID | Идемпотентный результат, нет дублей в event_log |

***

## 7. APP-05 — Graph Path Explorer

### 7.1 Назначение

Визуализация и объяснение структурных связей между сущностями в графе памяти.

### 7.2 Tool contract

```
open_graph_path(
  scope:            string,
  from_entity_id:   string,
  to_entity_id:     string,
  as_of?:           datetime,    // default: now()
  max_depth?:       number,      // default: 4
  access?:          AccessContext
) → AppSession

expand_graph_neighbors(
  session_id:   string,
  entity_id:    string,
  direction:    "in" | "out" | "both",
  depth:        number           // max: 2, default: 1
) → AppActionResult

open_edge_details(
  session_id:   string,
  edge_id:      string
) → AppSession  // APP-01

use_path_as_context(
  session_id:   string,
  path_id:      string
) → AppActionResult
// Если клиент поддерживает sampling capability →
//   инициирует sampling/createMessage с path как structured input.
// Иначе → возвращает serialized path как text.

close_session(session_id: string) → AppActionResult
```

### 7.3 Функциональные требования

- **FR-GRAPH-01:** App показывает найденный путь между двумя entities как последовательность nodes и edges.
- **FR-GRAPH-02:** Для каждого edge показываются: relation, confidence, strength, provenance, temporal validity.
- **FR-GRAPH-03:** Два режима: `shortest path` (default) и `explain why connected` (human-readable rationale по каждому ребру).
- **FR-GRAPH-04:** Пользователь может раскрыть соседей выбранного узла на 1–2 шага через `expand_graph_neighbors`.
- **FR-GRAPH-05:** App явно показывает причину отсутствия пути: `no_path`, `policy_hidden`, `inactive_at_as_of`, `depth_limit_exceeded`.
- **FR-GRAPH-06:** Кнопка `use as context` вызывает `use_path_as_context`. Сервер возвращает serialized path как text для ручного использования клиентом. При будущей поддержке `sampling` capability сервер сможет инициировать `sampling/createMessage` с path как structured input; до тех пор fallback — всегда serialized text.
- **FR-GRAPH-07:** UI не пытается рисовать весь граф. Отображаются только найденный path и local neighborhood (1 hop от каждого узла пути).

### 7.4 Нефункциональные требования

- **NFR-GRAPH-01:** Поиск пути ограничен `max_depth` и бюджетом traverse (configurable, default: 500 nodes).
- **NFR-GRAPH-02:** `expand_graph_neighbors` ограничен depth ≤ 2 и возвращает не более 50 соседей на вызов.
- **NFR-GRAPH-03:** При больших графах сервер возвращает сэмплированные или paginated neighbor sets.

### 7.5 Acceptance criteria

| ID | Сценарий | Ожидаемый результат |
|---|---|---|
| AC-GRAPH-01 | Связанные entities | Показывается путь, детали каждого ребра доступны |
| AC-GRAPH-02 | Несвязанные entities | Empty state с причиной `no_path` |
| AC-GRAPH-03 | Ребро неактивно при заданном `as_of` | Путь через это ребро не строится, причина `inactive_at_as_of` |
| AC-GRAPH-04 | Клиент без sampling capability | `use_path_as_context` возвращает serialized text, не падает |
| AC-GRAPH-05 | Клиент без Apps-capability | JSON fallback согласно §2.5 |

***

## 8. Релизный план

| Release | Apps | Итоговый workflow |
|---|---|---|
| R1 | APP-01 (read-only) | Просмотр любой единицы памяти |
| R2 | APP-01 (write actions) + APP-02 | Bi-temporal review workflow |
| R3 | APP-03 | Human-in-the-loop ingestion |
| R4 | APP-04 | Operational memory hygiene |
| R5 | APP-05 | Graph-native exploratory UX |

### 8.1 Definition of Done для каждого App

- **DoD-01:** Реализован `open_*` tool, UI resource, минимум один интеграционный сценарий.
- **DoD-02:** Реализован текстовый fallback согласно §2.5.
- **DoD-03:** Все write-actions логируются в `event_log`.
- **DoD-04:** Есть permission check по `scope` и `policy_tags`.
- **DoD-05:** Реализованы happy path, empty state, error state, stale state.
- **DoD-06:** Реализован `close_session`.
- **DoD-07:** Для Apps с async tasks реализован `get_lifecycle_task_status`.

### 8.2 Схемные миграции

| Миграция | Описание |
|---|---|
| `011_ingestion_draft.surql` | Таблицы `draft_ingestion`, `draft_item` (APP-03) |
| `012_app_sessions.surql` | Таблица `app_session` для server-side session tracking (все Apps) |

***

## 9. Открытые вопросы

| ID | Вопрос | Когда решать |
|---|---|---|
| OQ-01 | Какой конкретно формат возвращает `open_*` tool для Apps-capable клиента — полный UI bundle или delta? Нужна финализация по MCP Apps spec. | До R1 |
| OQ-02 | Cleanup expired drafts: комбинированный подход — lazy cleanup при открытии новой сессии (удаляет expired draft если `expires_at < now`) + periodic batch cleanup при старте сервера (FR-ING-10). Реализовать как tokio::spawn с интервалом 300 сек. | До R3 |
| OQ-03 | Вынести `get_lifecycle_task_status` в общий `get_task_status` для всех Apps? | До R4 |
| OQ-04 | Должен ли `rebuild_communities` запускаться автоматически по триггеру (n новых edges) или только вручную? | До R4 |