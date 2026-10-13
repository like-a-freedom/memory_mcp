# Анализ памяти memory_mcp — 13 октября 2026

Корневая причина доминирующего расхода памяти найдена и воспроизведена. Чтение фактов с embeddings проходит через дорогую сериализацию SurrealDB Value в tagged JSON, затем через полное глубокое клонирование результата. После освобождения данных glibc сохраняет почти весь раздутый heap в RSS/swap.

Это составная причина: ненужные векторы в запросе → раздувание представления каждого числа → второй экземпляр всего ответа → удержание освобождённых страниц аллокатором. Исправление только последнего звена не устраняет опасный пик.

Этот отчёт описывает завершённый этап анализа; последующий этап планирования сохраняет документацию и обезличенные результаты в репозитории. Работа выполнялась как анализ. Я не редактировал исходники или конфигурацию приложения, не запускал рестарты и не менял настройки production. Экспериментальные программы, их результаты и этот отчёт находятся только в /private/tmp. Все прямые SQL-проверки были SELECT с агрегатами; содержимое фактов и credentials в отчёт не включены.

## Подтверждение на production

Исследованный процесс — memory_mcp_http, версия 1.34.0, Linux amd64, Debian glibc 2.41-12+deb13u4. Container image ID:

`sha256:cbdf13400635dc674301a24294e1f0fea7b789cbb62a22d518a77aa8f538d31b`

Repo digest:

`ghcr.io/like-a-freedom/memory_mcp@sha256:6f374a20668a3e2003727636c937c0448e90c86eb256dc9c0b5731c3c3ef243b`

SHA-256 фактически запущенного бинарника:

`234d11ba44e7834593b10aaabbac181c0d09117d5f813e0dd76f337dfa5ec017`

Контейнеру разрешено 1 ГиБ RAM и суммарно 2 ГиБ RAM + swap. MALLOC_ARENA_MAX уже равен 2. На момент снимка 08:45:59 UTC / 11:45:59 МСК:

| Показатель | Значение |
|---|---:|
| PID | 450909 |
| RSS | 737 980 КиБ, около 720,7 МиБ |
| VmSwap | 1 048 488 КиБ, около 1023,9 МиБ |
| Потоки | 3 |
| RestartCount | 7 |

RSS показывает только резидентные страницы. RSS + VmSwap здесь показывает масштаб удержанной памяти процесса, около 1,70 ГиБ; это не точная замена cgroup accounting. Нельзя оценивать ситуацию только по docker stats/RSS или складывать cgroup swapcache дважды.

В Grafana/VictoriaMetrics на 08:41:30 UTC:

| Показатель | Байты |
|---|---:|
| memory_alloc_arena_bytes | 1 797 877 760 |
| memory_alloc_free_bytes | 1 792 762 016 |
| memory_alloc_in_use_bytes | 5 115 744 |
| memory_alloc_mmap_bytes | 18 436 096 |
| memory_alloc_heap_bytes | 82 579 456 |

**99,715% пространства arenas — свободные блоки, а не живые объекты приложения.** Значение in_use относится к arena-учёту; отдельно учтённые mmap-блоки тоже нужно учитывать. Здесь нет основания считать 1,7 ГиБ живыми cache entries или моделью.

Значение [heap] отражает только соответствующий mapping, а не все arenas glibc. Маленький [heap] не исключает гигабайт anonymous mappings других heaps.

В этот же момент tenant runtime count, accounted context/query cache bytes, preflight reserved bytes и requests inflight равны нулю. Это исключает эти измеренные владельцы как объяснение основной удержанной памяти в снимке. Нулевой runtime pool не исключает фоновые операции: они связывают tenant storage отдельно.

Данные одного ready tenant: 306 фактов, 300 неинвалидированных фактов, 41 episode; суммарно 626 688 элементов embeddings = 306 × 2048. Суммарная длина content — всего 66 670 символов. Для доказательства не требовалось выгружать персональный контент.

## Когда возникает скачок

Логи старта 13 октября:

- 07:48:29.572 UTC: router_built, RSS 35 766 272 байта, swap 0.
- 08:19:37.786 UTC: router_built, RSS 35 487 744 байта, swap 0.

Следовательно, гигабайты появляются после запуска сервера/фоновых работ, а не при загрузке config, NER или построении router. В этой конфигурации embeddings обслуживает внешний OpenAI-compatible provider; SurrealDB — отдельный внешний процесс.

В первом окне после старта около 07:48:30–07:49:00 UTC метрики показывают рост arena с 1 495 040 до 1 718 726 656 байт. В том же окне завершился первый lifecycle_decay: duration sum 19,475127859 секунды, count 1. Первый embedding_backfill занял 0,146012759 секунды. На видимых HTTP series в этом окне есть /memory/metrics; tenant runtime count равен нулю.

Это устанавливает lifecycle_decay как обоснованный инициатор первого скачка: он немедленно запускается после старта и читает весь набор активных фактов с embeddings. PromQL range timestamps — времена вычисления запроса; скрейп и обновление gauges имеют конечный шаг, поэтому они не являются allocation trace с точностью до миллисекунд.

Kernel journal подтверждает настоящие memcg OOM:

- 07:48:28.303586 UTC / 10:48:28 МСК: killed PID 349749.
- 08:19:36.631412 UTC / 11:19:36 МСК: killed PID 436883.

State.OOMKilled=false у последующего запущенного контейнера не опровергает эти исторические события.

Во время расследования был сделан один MCP assemble_context-запрос на стенд. Он начался примерно в 08:19:30 UTC, завершился закрытием соединения и совпал с вторым OOM. После этого MCP-запросы на production больше не выполнялись. Этот эпизод не использован как управляемый нагрузочный тест: OOM уже происходили ранее. Read-oriented MCP методы могут иметь внутренние access/query-log эффекты; я не могу гарантировать отсутствие частичных побочных записей этого прерванного вызова.

## Точный дефект в исходниках

1. [run_decay_pass](/Users/solovey/Documents/dev/memory_mcp/crates/memory-mcp/src/memory/lifecycle_workers/decay.rs:110) вызывает select_active_facts(1000).
2. [build_select_active_facts_query](/Users/solovey/Documents/dev/memory_mcp/crates/memory-mcp/src/knowledge/queries.rs:183) строит SELECT * FROM fact. Для decay нужны fact_id, t_valid, t_invalid, confidence, access_count и last_accessed, а embedding не используется.
3. [SurrealDbClient::query](/Users/solovey/Documents/dev/memory_mcp/crates/memory-mcp/src/storage/client.rs:1072) вызывает surreal_to_json, затем extract_records.
4. [surreal_to_json](/Users/solovey/Documents/dev/memory_mcp/crates/memory-mcp/src/storage/helpers.rs:129) использует serde_json::to_value для SurrealDB Value. Это сериализация enum-представления, а не прямая конвертация в обычный JSON. Один float превращается в {"Number":{"Float":число}}: два JSON object вместо одного scalar.
5. [extract_records](/Users/solovey/Documents/dev/memory_mcp/crates/memory-mcp/src/storage/helpers.rs:149) извлекает Array, затем делает as_array().cloned(). Это глубокая копия всего tagged дерева, включая сотни тысяч объектов чисел, при ещё живом оригинале.
6. [normalize_surreal_json](/Users/solovey/Documents/dev/memory_mcp/crates/memory-mcp/src/storage/helpers.rs:193) строит обычное дерево, а исходные tagged деревья освобождаются. Живой результат маленький; ранее затронутые страницы остаются у glibc.

SurrealDB Value::Number и Number::Float сериализуются как externally tagged enum. Размер SurrealDB Value в воспроизведении — 56 байт, serde_json::Value — 32. JSON objects используют BTreeMap. Поэтому ответ с миллионами маленьких объектов существенно дороже самого массива чисел. Это не догадка о JSON: ниже измерены реальные allocation sizes и RSS.

helpers.rs и knowledge/queries.rs совпали побайтово с tag v1.34.0 (bec27690df8578b807d76fee4551b2687acbc983). Изменение decay.rs в текущем рабочем дереве касается уровня логирования ошибки, а не читаемых данных или алгоритма decay. Наличие незакоммиченных правок не принято за доказательство deployment.

## Управляемое воспроизведение и A/B

Временная программа создаёт 300 синтетических SurrealDB записей с embeddings 2048 чисел, fact_id и confidence. В baseline скопированы действующие функции extract_records/unwrap_object_wrapper/normalize_surreal_json. Настоящие данные production не копировались.

Использованы surrealdb-types 3.3.2, serde_json 1.0.151 и serde_core 1.0.229. Production cargo tree для serde_json: alloc/default/std; preserve_order/arbitrary_precision не включены. Linux container — Rust 1.99.0-trixie, glibc **той же версии 2.41-12+deb13u4**, MALLOC_ARENA_MAX=2. Операция выполняется в отдельном worker thread, который ещё жив при последнем снимке, как долгоживущий Tokio worker. Лимит локального контейнера 3 ГиБ; production не профилировался инструментом, останавливающим процесс.

| Вариант | Пик живых Rust allocations | RSS после drop |
|---|---:|---:|
| Действующий tagged JSON + clone | 1533,93 МиБ | 1614,27 МиБ |
| Удалён только clone, tagged JSON сохранён | 800,03 МиБ | 839,96 МиБ |
| Прямая Value::into_json_value, те же vectors | 33,10 МиБ | 35,06 МиБ |
| Embeddings не читаются, старый converter сохранён | 1,86 МиБ | 3,92 МиБ |
| Baseline, затем malloc_trim(0) | 1533,93 МиБ | 2,30 МиБ после trim |

Промежуточные стадии baseline, байты:

- SurrealDB rows: live 34 704 466.
- Tagged JSON: live 804 188 793, accumulated peak 838 891 593.
- Во время клонирования: peak **1 608 441 696**.
- После нормализации: live 19 871 756.
- После drop: live 1666; glibc arena 1 690 808 320, free 1 690 774 144; RSS около 1,58 ГиБ.

Это воспроизводит и размер скачка, и характер production: маленький живой heap при огромном свободном пространстве arenas и большом RSS. Наивная интерпретация «память не освободилась, значит объекты утекли» неверна для этого механизма.

Прямая конвертация снижает allocation peak примерно в **46 раз**. Удаление embeddings из запроса устраняет основную нагрузку даже при старом converter. Удаление одного clone оставляет почти 800 МиБ пика и не является достаточной итоговой митигацией.

Дополнительные контрольные эксперименты на macOS: компиляция всех 32 regex TripleExtractor дала peak около 0,75 МиБ; 10 000 duration observations с upkeep/render Prometheus exporter — около 0,77 МиБ. Они не являются Linux baseline приложения, но не воспроизвели раздувание, в отличие от storage conversion.

## Почему предыдущие ограничения не помогли

- Cache budgets/pool caps ограничивают долгоживущие объекты, а проблема возникает при materialization ответа базы.
- HTTP body/preflight quotas ограничивают входящие данные, но не SELECT-result и его внутреннее представление.
- Maintenance parallelism=1 не ограничивает объём одного ответа и не сериализует фоновую работу с обычными запросами.
- Ограничение decay 1000 строк не ограничивает байты: уже 300 строк достаточно для пика 1,5 ГиБ живых allocations.
- MALLOC_ARENA_MAX=2 ограничивает число arenas, а не количество байтов в них. Этот параметр уже действовал и в production, и в воспроизведении. [Описание glibc tunables](https://sourceware.org/glibc/manual/latest/html_node/Memory-Allocation-Tunables.html).
- Startup snapshots завершаются до тяжёлой первой фоновой операции. Periodic owner gauges не атрибутируют SDK response, tagged JSON и его clone.
- Текущие mallinfo2 gauges достаточно хорошо показывают последствия, но без чтения source и A/B не называют инициирующую allocation. [Семантика mallinfo2](https://sourceware.org/glibc/manual/latest/html_node/Statistics-of-Malloc.html).

Масштаб свободного arena не равен resident bytes. В trim-контроле RSS упал до 2,3 МиБ, а arena почти не изменился. Поэтому критерием успеха должна быть реальная память процесса/cgroup вместе с allocation peak, а не только снижение arena или free/arena.

## План митигации

**P0 — убрать amplification в storage adapter.** Заменить generic enum serialization на consuming conversion SurrealDB Value → обычные JSON значения, не создающую tagged дерево для каждого scalar. Извлекать массив по владению, без глубокого clone. Это касается общей точки, используемой query/select_one/select_table/create/update, а не только decay.

Value::into_json_value уже есть в закреплённой зависимости, поэтому новая зависимость не нужна. Однако A/B демонстрирует memory effect, а не готовый совместимый patch: RecordId в прямой конвертации становится строкой; текущие helpers умеют структурированный RecordId. Перед использованием метода проверить/адаптировать договорённости по RecordId, datetime, None/Null, decimal, UUID, bytes и geometry. Вариант безопасной реализации — consuming converter с явно сохранённым нужным представлением этих типов, без tagged numeric trees.

**P0 — отдельная минимальная projection для decay.** Читать только перечисленные lifecycle поля. Сохранить bi-temporal predicate и retraction через ClaimStore. Эта мера убирает инициатор первого большого скачка и лишний сетевой ответ. Она должна идти вместе с исправлением общего converter, потому что другие чтения остаются опасны.

**P1 — пересмотреть все nonsemantic reads.** scan_facts используется в lexical fallback, temporal retrieval и wake-up view; select_active_facts — в experience retrieval и lifecycle dashboard. Эти пути способны принести embeddings, даже если затем нужен только текст/metadata. ANN query тоже SELECT *, хотя embedding нужен для вычисления similarity в базе, а не обязательно в ответе. Добавить owner-specific projections, SQL-фильтры и ограниченную обработку страниц. Проверять ранжирование/recall и bi-temporal/access invariants; не заменять полный scan произвольным усечением первых N фактов.

**P1 — установить бюджет materialization.** Row limit дополнять ограничением по размеру ответа и количеству одновременно обрабатываемых тяжёлых страниц/запросов. Выбирать concurrency из измеренного peak per operation, учитывая совместное выполнение maintenance и MCP. При этом сетевой payload byte limit сам по себе тоже не заменяет исправление раздувающей конвертации.

**P2 — allocator и краткосрочная эксплуатационная митигация.** malloc_trim доказан как способ вернуть уже освобождённые страницы в локальном контроле, но не предотвращает исходный пик. Без замера latency и полной версии приложения не включать периодический trim в production. Смена аллокатора — вторичная оптимизация после P0/P1. Отдельный обнаруженный факт: global_allocator для mimalloc находится только в main.rs (stdio/CLI); в memory_mcp_http.rs его нет. Просто добавить feature mimalloc к Docker build недостаточно, чтобы HTTP binary начал его использовать.

Повышение RAM/swap или частые рестарты увеличивают запас/сбрасывают последствия, но не исправляют amplification. Эти действия в ходе анализа не выполнялись.

## Приёмка будущей реализации

1. Контрактные проверки через публичный storage interface: чтение/создание/обновление, идентификаторы с дефисами и нестандартными keys, typed values, None/Null и даты; значения embeddings и ошибки должны остаться корректны. Не тестировать только private normalizer.
2. Изолированный Linux profiling fixture 300 × 2048, затем 1000 × 2048; конкретные значения и метаданные фиксированы. Для 300 записей proposed gate: peak allocations <64 МиБ вместо 1534 МиБ. Для 1000 — подтвердить рост, соответствующий обычным rows/JSON, а не tagged amplification.
3. Полный memory_mcp_http в staging с production digest/settings baseline, затем исправленной сборкой и синтетическим tenant той же формы: первый decay, повторный decay, lexical/temporal/wake-up/experience/ANN и assemble_context; фоновые работы совместно с запросами.
4. Предлагаемый бюджет полного remote-embedding профиля на fixture: peak RSS <256 МиБ, steady RSS + VmSwap <128 МиБ, VmSwap=0. Эти пороги — acceptance targets; это не измеренные показатели ещё не написанного исправления.
5. Серия минимум 100 assemble_context, проверка возврата к стабильному плато и отсутствия роста RSS/swap после прогрева; отдельная повторяемая проверка одновременных операций в заданных production concurrency limits. Не считать один успешный запрос доказательством.
6. Проверить реальные cgroup peak/events и kernel OOM, dataset counts, отсутствие изменения данных от maintenance сверх ожидаемых lifecycle правил; корректность temporal/access/retrieval контрактов.
7. Узкие integration tests, cargo fmt --all --check, затем обязательный workspace clippy из AGENTS.md перед shipping. Production rollout и повторный production замер — отдельные будущие действия.

## Границы подтверждения

Дефект конвертации, глубокое клонирование, размер amplification и удержание glibc **подтверждены причинным A/B**; OOM и текущее удержание памяти подтверждены непосредственно на production. Первый lifecycle_decay — источник, подтверждённый code path и временной корреляцией. Live allocation stack внутри production не снимался, поэтому эксклюзивную атрибуцию каждой production allocation конкретному caller и происхождение каждого последующего OOM заявлять нельзя.

Linux воспроизведение выполнено на arm64; production — amd64. Совпадают версия glibc, критические версии библиотек и 64-битные размеры представлений. Это убедительно воспроизводит механизм и масштаб, но не заменяет полный amd64 staging A/B приложения. OCI revision label у production отсутствует: версия, digest и binary hash закреплены, но image→Git commit не доказан build provenance.

Исследование установило доминирующую причину наблюдаемого расхода; оно не доказывает отсутствие всех иных утечек. После устранения amplification потребуется проверить остаточный профиль. Эффект ещё не реализованного исправления на production не заявляется.

## Сохранённые материалы

- [Production observations, startup logs, kernel OOM, scalar SQL aggregates](/Users/solovey/Documents/dev/memory_mcp/docs/performance/memory-root-cause-2026-10-13/production-observations.json)
- [Grafana metrics at 08:41:30 UTC](/Users/solovey/Documents/dev/memory_mcp/docs/performance/memory-root-cause-2026-10-13/metrics-at-084130.json)
- [First background/startup window](/Users/solovey/Documents/dev/memory_mcp/docs/performance/memory-root-cause-2026-10-13/startup-window.json)
- [All five Linux A/B outputs and exact run commands](/Users/solovey/Documents/dev/memory_mcp/docs/performance/memory-root-cause-2026-10-13/results.json)
- [Probe source](/private/tmp/memory-rootcause-linux-probe/src/main.rs)
- [Probe dependency lock](/private/tmp/memory-rootcause-linux-probe/Cargo.lock)

Контейнеры эксперимента запускались с --rm; исходники проекта и production config не менялись. В baseline функции storage helpers использованы без изменения; варианты move/direct/projected/trim — только в экспериментальном коде.

