# Memory MCP (Rust) — QuickStart и примеры использования 🧠🔧

Краткое описание
- Этот репозиторий содержит Rust‑версию Memory MCP сервера (`memory_mcp`) с stdio-only RMCP транспортом и встроенной/удалённой поддержкой SurrealDB.
- Сервер намеренно **memory-only**: он хранит факты о сущностях и связях, извлекает структуру из эпизодов, резолвит сущности, инвалидирует устаревшие факты и собирает долгосрочный контекст для LLM.
- Публичная MCP-поверхность сведена к шести intent-driven инструментам: `ingest`, `extract`, `resolve`, `invalidate`, `assemble_context`, `explain`.

## Быстрый старт ✅
1. Установить (рекомендуется) — через `cargo install`:

```bash
cargo install --locked memory_mcp
```

Или собрать бинарник из исходников:

```bash
cargo build --release
```

2. Задать необходимые переменные окружения (пример):

```bash
# Example environment variables (can be stored in .env)
export SURREALDB_DB_NAME=memory              # e.g., memory, testdb, production_db
export SURREALDB_URL=ws://127.0.0.1:8000/rpc # ws://host:port/rpc (omit for embedded)
export SURREALDB_EMBEDDED=false             # true | false (recommended: leave unset to infer from SURREALDB_URL)
export SURREALDB_DATA_DIR=./data/surrealdb  # path used when embedded (default ./data/surrealdb)
export SURREALDB_NAMESPACES=org,personal,private  # comma-separated namespaces (at least one)
export SURREALDB_USERNAME=root
export SURREALDB_PASSWORD=root
export LOG_LEVEL=info                        # trace | debug | info | warn | error
```

### Описание переменных окружения

| Переменная | Описание | Формат / Примеры |
|---|---|---|
| `SURREALDB_DB_NAME` | Имя базы в SurrealDB | `memory`, `testdb`, `production_db` |
| `SURREALDB_URL` | URL SurrealDB (WebSocket RPC). Если пустой — используется embedded SurrealDB | `ws://127.0.0.1:8000/rpc` |
| `SURREALDB_EMBEDDED` | Принудительно включить embedded режим. Рекомендуется оставить unset и полагаться на `SURREALDB_URL` | `true` / `false` |
| `SURREALDB_DATA_DIR` | Путь к data dir, используется в embedded режиме | `./data/surrealdb` |
| `SURREALDB_NAMESPACES` | Спискок namespaces, разделённых запятой | `org,personal,private` |
| `SURREALDB_USERNAME` | Имя пользователя SurrealDB | `root` |
| `SURREALDB_PASSWORD` | Пароль SurrealDB | `root` |
| `LOG_LEVEL` | Уровень логирования | `trace`, `debug`, `info`, `warn`, `error` |

> Советы: храните секреты в CI/секретном хранилище, а не в репозитории. Для локальной разработки используйте `.env` и `set -a; source .env; set +a`.

3. Запустить сервер (stdio transport):

Если установлен через `cargo install`:

```bash
LOG_LEVEL=info memory_mcp
```

Если собран из исходников:

```bash
LOG_LEVEL=info ./target/release/memory_mcp
```

> Сервер ожидает входных сообщений по stdin и отвечает в stdout в формате RMCP/MCP envelope (см. примеры ниже).

---

## Пример `mcp.json` (интерфейс инструментов) 🗂️
Ниже — минимальный пример манифеста `mcp.json`, который описывает доступные инструменты и их контракт.

```json
{
  "name": "memory_mcp",
  "version": "0.1.0",
  "description": "Memory MCP — long-term memory tools for ingest, extraction, entity resolution, invalidation, context assembly, and explanation",
  "tools": [
    {
      "name": "ingest",
      "description": "Ingest a document or message into memory (creates an episode)",
      "input_example": {
        "source_type": "email",
        "source_id": "MSG-123",
        "content": "The email body or raw document",
        "t_ref": "2026-03-11T10:00:00Z",
        "scope": "org"
      }
    },
    {
      "name": "extract",
      "description": "Extract entities, facts, and links from an episode or inline content",
      "input_example": { "episode_id": "episode:..." }
    },
    {
      "name": "resolve",
      "description": "Resolve a canonical entity id from a name and aliases",
      "input_example": {
        "entity_type": "person",
        "canonical_name": "John Doe",
        "aliases": ["Johnny Doe", "J. Doe"]
      }
    },
    {
      "name": "assemble_context",
      "description": "Assemble active ranked context for a natural-language query",
      "input_example": {
        "query": "ARR commitments for Alice",
        "scope": "org",
        "as_of": "2026-03-11T10:00:00Z",
        "budget": 10
      }
    },
    {
      "name": "explain",
      "description": "Return provenance-ready citations for context items",
      "input_example": {
        "context_items": "[{\"content\":\"ARR $2M\",\"quote\":\"ARR $2M\",\"source_episode\":\"episode:abc123\"}]"
      }
    },
    {
      "name": "invalidate",
      "description": "Mark a fact as no longer valid while preserving history",
      "input_example": {
        "fact_id": "fact:...",
        "reason": "Superseded by a newer update",
        "t_invalid": "2026-03-11T10:00:00Z"
      }
    }
  ]
}
```

> Поместите `mcp.json` рядом с бинарником или используйте его в CI для документирования интерфейса инструментов.

> Важно: legacy-инструменты вроде `create_task`, `send_message_draft`, `schedule_meeting`, `update_metric`, `ui_*`, а также alias-обёртки вроде `ingest_document` больше не являются частью публичной MCP-поверхности этого сервера.

---

### Пример развернутого `mcp.json` (stdio servers) — пример для локальной разработки

Ниже — пример более полного `mcp.json` с несколькими stdio‑серверами. Внимание: секции `env` содержат placeholders вместо реальных секретов — **не** храните секреты в репозитории.

```json
{
  "servers": {
    "memory-mcp": {
      "type": "stdio",
      "command": "./target/release/memory_mcp",
      "env": {
        "SURREALDB_DB_NAME": "memory",
        "SURREALDB_URL": "ws://127.0.0.1:8000/rpc",
        "SURREALDB_NAMESPACES": "org,personal,private",
        "SURREALDB_USERNAME": "root",
        "SURREALDB_PASSWORD": "root",
        "LOG_LEVEL": "info"
      }
    },
    "memory-mcp-embedded": {
      "type": "stdio",
      "command": "./target/release/memory_mcp",
      "env": {
        "SURREALDB_DB_NAME": "memory",
        "SURREALDB_EMBEDDED": "true",
        "SURREALDB_DATA_DIR": "./data/surrealdb",
        "SURREALDB_NAMESPACES": "org,personal,private",
        "SURREALDB_USERNAME": "root",
        "SURREALDB_PASSWORD": "root",
        "LOG_LEVEL": "info"
      }
    }
  }
}
```

> Примечание: значения env в примере совпадают с файлом `.env` в корне репозитория. Для запуска локально можно выполнить:

```bash
# загрузить переменные из .env (bash/zsh)
set -a; source .env; set +a
# затем запустить бинарник
./target/release/memory_mcp
```

> Миграции схемы должны лежать в единственной ожидаемой относительной папке `./migrations` (от корня репозитория). При первом старте MCP файлы `*.surql` из `./migrations` будут автоматически применены к базе. Убедитесь, что `./migrations/__Initial.surql` присутствует.

> В CI используйте защищённые переменные/секреты вместо хранения токенов в файлах.

> Подсказка: для CI используйте безопасные переменные/секреты в самом CI и не вставляйте токены в публичные файлы.


---

## Примеры реальных сценариев использования (stdin/stdout) 📡
Ниже — упрощённые примеры последовательного взаимодействия с сервером через stdio (RMCP envelope). Формат сообщения — JSON; конкретная envelope-форма может варьироваться по реализации клиента, но идея следующая:

1) Ингестируем документ:

```bash
printf '%s\n' '{"type":"call","id":"1","tool":"ingest","args": {"source_type":"email","source_id":"MSG-202","content":"Hello from Alice"}}' | ./target/debug/memory_mcp
```

Ожидаемый ответ (пример):

```json
{"type":"response","id":"1","ok":{"status":"success","result":"episode:abc123","guidance":"Call extract next to derive entities and facts."}}
```

2) Извлекаем сущности/факты из эпизода:

```bash
printf '%s\n' '{"type":"call","id":"2","tool":"extract","args": {"episode_id":"episode:abc123"}}' | ./target/release/memory_mcp
```

Ответ (пример):

```json
{"type":"response","id":"2","ok":{"status":"success","result":{"episode_id":"episode:abc123","entities":[{"entity_id":"entity:john","type":"person","canonical_name":"John Doe"}],"facts":[{"fact_id":"fact:arr","type":"metric"}],"links":[{"entity_id":"entity:john","episode_id":"episode:abc123"}]},"guidance":"Resolve canonical entities for any ambiguous names before creating manual links."}}
```

3) Собираем контекст по запросу (assemble_context):

```bash
printf '%s\n' '{"type":"call","id":"3","tool":"assemble_context","args": {"query":"ARR for John","scope":"org","as_of":"2026-03-11T10:00:00Z","budget":25}}' | ./target/debug/memory_mcp
```

Ответ (пример):

```json
{"type":"response","id":"3","ok":{"status":"success","result":[{"fact_id":"fact:arr","content":"ARR $2M","quote":"ARR $2M","source_episode":"episode:abc123","confidence":0.93,"provenance":{"source_episode":"episode:abc123"},"rationale":"matched scope=org and active at 2026-03-11"}],"guidance":"Call explain if you need provenance-ready citations for selected items.","has_more":false,"total_count":1}}
```

4) Поясняем происхождение (explain):

```bash
printf '%s\n' '{"type":"call","id":"4","tool":"explain","args":{"context_items":"[{\"content\":\"ARR $2M\",\"quote\":\"ARR $2M\",\"source_episode\":\"episode:abc123\"}]"}}' | ./target/debug/memory_mcp
```

Ответ (пример):

```json
{"type":"response","id":"4","ok":{"status":"success","result":[{"content":"ARR $2M","quote":"ARR $2M","source_episode":"episode:abc123"}],"guidance":"Use these citations directly in the final response.","has_more":false,"total_count":1}}
```

---

## Пример последовательности в CI (тестовое встраивание) 🧪
В CI можно запускать бинарник в фоновом процессе и взаимодействовать через stdio (или через тестовый клиент). Пример псевдо‑скрипта:

```bash
# запустить сервер (в background) и перенаправить stdin/stdout во временные FIFO
mkfifo /tmp/mcp_in /tmp/mcp_out
./target/debug/memory_mcp < /tmp/mcp_in > /tmp/mcp_out &
PID=$!
# отправить вызов
echo '{"type":"call","id":"1","tool":"ingest","args":{"source_type":"email","source_id":"MSG-1","content":"CI test"}}' > /tmp/mcp_in
# прочитать ответ
head -n 1 /tmp/mcp_out
# kill $PID после завершения
kill $PID
```

---

## Полезные переменные окружения и конфиг
- SURREALDB_DATA_DIR — путь к директории данных при embedded SurrealDB (по умолчанию `./data/surrealdb`).
- SURREALDB_DB_NAME — имя базы (пример: `testdb`).
- SURREALDB_USERNAME / SURREALDB_PASSWORD — учётные данные для SurrealDB.
- `LOG_LEVEL` — уровень логирования (рекомендуемый).

---

Если нужно, могу добавить конкретные примеры клиента (Python/Node) для работы с stdio RMCP, или подготовить `mcp.json` с полными JSON‑schema для каждого инструмента. Хотите, чтобы я добавил пример клиентской библиотеки (Python) для интеграции по stdin/stdout? 💬

---

## Ускорение инкрементальных сборок и покрытия 📦⚡
Чтобы `cargo build`, `cargo test`, `cargo tarpaulin` и `cargo llvm-cov` повторно использовали один и тот же кеш и не пересобирали всё с нуля — рекомендую использовать sccache + включить инкрементальную компиляцию для тестового профиля.

Что сделано в репозитории:
- Включена инкрементальная компиляция для `dev` и `test` (см. `Cargo.toml`).
- Добавлен локальный cargo‑wrapper `sccache` через `.cargo/config.toml` (необходим `sccache` в PATH).

Быстрый набор команд (macOS):

1) Установить sccache:

```bash
brew install sccache
```

2) Включить в текущей сессии (или добавить в `~/.zshrc`/`~/.bashrc`):

```bash
export RUSTC_WRAPPER=$(which sccache)
export SCCACHE_CACHE_SIZE=20G   # опционально
sccache --show-stats            # проверить статистику кеша
```

3) Запускать как обычно — артефакты будут кешироваться и переиспользоваться:

- `cargo build` — инкрементально
- `cargo test` — инкрементально (profile.test теперь с incremental = true)
- `cargo tarpaulin --no-clean` — быстрее повторные прогоны покрытия
- `cargo llvm-cov` — переиспользует кеш sccache между запусками

Советы и ограничения:
- Tarpaulin/llvm-cov меняют RUSTFLAGS (инструментирование), поэтому будут использовать отдельные кэш‑ключи, но sccache всё равно уменьшит время повторных сборок для одинаковых флагов.
- Не запускайте `cargo clean` если хотите сохранять кеш.