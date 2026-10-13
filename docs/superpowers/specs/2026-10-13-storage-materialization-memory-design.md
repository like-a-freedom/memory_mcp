# Storage materialization memory remediation — design and acceptance contract

**Дата:** 2026-10-13. **Статус:** проект для review; реализация не начата.

## Основание и границы доказательства

Источник: [анализ production и причинный Linux A/B](../../performance/MEMORY_ROOT_CAUSE_2026-10-13.md), с сохранёнными JSON-результатами и SHA-256 в `docs/performance/memory-root-cause-2026-10-13/manifest.json`.

Подтверждённый механизм: `SELECT *` приносит ненужные embeddings; generic serde serialization создаёт два tagged object на каждый float; `extract_records` глубоко клонирует дерево; glibc удерживает освобождённые страницы. В Linux arm64-контроле 300×2048 дали 1533,93 MiB peak live allocations; удаление только clone — 800,03 MiB; прямая конвертация — 33,10 MiB. Это не измерения исправленного HTTP-приложения.

Production — `memory_mcp_http` 1.34.0, Linux amd64, glibc `2.41-12+deb13u4`, remote SurrealDB 3.3.2 и remote embedding provider с dimension 2048. Лимиты: RAM 1 GiB, RAM+swap 2 GiB, `MALLOC_ARENA_MAX=2`. Production baseline image:

`ghcr.io/like-a-freedom/memory_mcp@sha256:6f374a20668a3e2003727636c937c0448e90c86eb256dc9c0b5731c3c3ef243b`

Git revision этого image не доказан: OCI revision label отсутствует. Source release `bec27690df8578b807d76fee4551b2687acbc983` служит source baseline, а не доказательством состава image. Окончательный гейт требует нового полного Linux amd64 A/B.

## Цель и объём

Устранить подтверждённое раздувание materialization, прекратить чтение embeddings там, где они не нужны, и закрепить memory acceptance исполняемыми тестами. Общий adapter обслуживает CLI/stdio и HTTP; исправление не должно зависеть от caller или allocator feature.

Обязательный объём: consuming conversion и extraction; отдельная decay projection; проекции active reads, nonsemantic full scans и ANN response; совместимость storage/use-case contracts; Linux memory regressions; гейты CI и production composition. P0 и проекции выпускаются одним проверенным изменением: исправление только clone недостаточно.

Отдельный следующий проект: SQL pushdown, pagination и общий runtime admission budget для произвольно больших tenant datasets. Текущий план задаёт проверяемый бюджет на фиксированных fixtures, **не обещает глобальную bounded-memory гарантию для неограниченного scan**. `rank_lexical_records` строит anchor profile по всему candidate set; простой `LIMIT` или независимый top-K каждой страницы меняет ранжирование. Это требует собственного дизайна и проверки recall, а не дополнения к срочному adapter fix.

Allocator switch, periodic `malloc_trim`, увеличение RAM/swap, изменение production concurrency и отключение lifecycle в эту реализацию не входят. Они не устраняют исходный amplification. После P0 остаточный профиль решает, нужны ли дополнительные меры.

## Неизменяемые ограничения реализации

- Rust `1.99.0`; SurrealDB/SDK `3.3.2`; `serde_json` `1.0.151`; использовать текущий `Cargo.lock`, команды с `--locked`.
- Новых зависимостей, миграций, generated-code изменений и MCP tools нет. `Cargo.toml` не меняется.
- Один Active Namespace, выбранный при binding. Не вводить namespace/table selector в application-facing read.
- В HTTP production build: `streamable-http,mcp-apps` плюс существующий default `fs-watch`; без `mimalloc` и `test-fixtures` в измеряемом application binary.
- Ошибки остаются `MemoryError`; никакой ошибки query/take/namespace нельзя превращать в пустой успешный ответ.
- Политика хранения неизменна: facts не удаляются; decay retracts через `ClaimStore`, атомарно закрывая fact и derived claims.
- Не менять temporal predicates, ordering, row limits, relevance thresholds, cache defaults, job cadences или алгоритмы ranking в рамках проекций. ColdDecay acceptance сохраняет decay cadence 3600 s; повторный pass в том же процессе реально дожидается следующего cadence.
- Контрактные проверки идут через публичные `DbClient`, knowledge stores и memory capabilities. Visibility private helpers не расширяется ради тестов.
- Production `82.165.149.36:2288` остаётся read-only. Настоящий rollout требует отдельной явной авторизации после всех гейтов.
- Секреты и реальные memory contents не попадают в fixtures, git, artifacts и логи.

## Storage conversion

Переход должен потреблять `SurrealValue` по владению. Для `Array`/`Object` рекурсивно перемещать элементы; для обычных numbers/strings не создавать intermediate tagged JSON. Один consuming traversal; числовые vectors не проходят через `serde_json::to_value` над всем Surreal enum tree.

На стороне client возвращаются уже готовые rows через новый внутренний интерфейс:

```rust
pub(crate) fn surreal_records_to_json(value: surrealdb::types::Value)
    -> Vec<serde_json::Value>;
```

Его контракт: корневой `Array` — rows по порядку; `Object` — один row; `None` — пустые rows; явный scalar `Null` — один `serde_json::Value::Null`; другой scalar — один row. Вложенные `None`/`Null` — JSON null, Bool — JSON bool, Int/finite Float — JSON number, String/Datetime — строка. Datetime остаётся RFC3339, не теряет дробные секунды или timezone instant. Nonfinite float — JSON null, без panic.

Существующий `surreal_to_json(value: SurrealValue) -> Value` остаётся общим consuming value converter, в том числе для `server_version`. Это не механическая замена всего дерева на SDK `into_json_value`: SDK превращает RecordId в строку.

RecordId сохраняет structured representation, на которую опираются graph/read helpers: для `entity:trip-a` — `{"RecordId":{"table":"entity","key":"trip-a"}}`. String keys с дефисом, пробелом, Unicode и colon не обрезаются. Numeric/UUID/composite record keys сохраняют исходную идентичность и существующее представление; не сводить их к строке через `split(':')`.

Decimal не теряет точность и остаётся строкой (`"12345678901234567890.123456789"`). Для редких SDK variants `Uuid`, `Bytes`, `Duration`, `Geometry`, `Table`, `File`, `Range`, `Regex`, `Set` сохранить текущий adapter output: допустима legacy serialization только соответствующего leaf/subtree, исключая общую ветку numeric embeddings. Перед правкой снять через публичный storage interface маленькие baseline snapshots и вручную закрепить literal expected JSON в fixtures. Не вычислять expected новым converter или нормализатором. Любое намеренное изменение редкого типа требует отдельного решения и теста до production code.

`extract_records(Value) -> Vec<Value>` нужен и compatibility callers; он должен перемещать owned `Array`, а не `as_array().cloned()`. `BoundDbClient::query_rows` также должен перемещать готовые plain rows, а не клонировать их через `as_array().cloned()`. Borrowing `normalize_surreal_json(&Value) -> Value` остаётся для write builders; adapter read path не должен снова нормализовать полный plain JSON tree. Не совмещать write-path cleanup с данным исправлением.

Обязательны create/update/select_one/select_table/query и `server_version`, nested values, absence/explicit null, record identity, SQL failure во втором statement и namespace refusal. JSON null у nullable projection fields не должен сделать активный fact «уже invalidated».

## Query projections

| Read | Возвращаемые поля | Неизменяемая семантика |
|---|---|---|
| Decay | `fact_id, t_valid, t_invalid, confidence, access_count, last_accessed` | existing full bi-temporal visibility, `ORDER BY t_valid ASC`, limit 1000, ClaimStore retraction |
| Active facts, оба knowledge stores | existing `NONSEMANTIC_FACT_PROJECTION` | existing visibility/order/caller limit; experience 500, dashboard 10000 |
| `KnowledgeStoreClient::scan_facts` | `id` + existing `NONSEMANTIC_FACT_PROJECTION` | весь прежний набор строк, без нового WHERE/LIMIT; сохраняет данные для исторических reads и identity fallback |
| ANN | `id` + existing `NONSEMANTIC_FACT_PROJECTION` + computed `sem_score` | vector predicate и cosine вычисляются в DB; K/ef_search/filter/order/limit прежние |

`NONSEMANTIC_FACT_PROJECTION` — `fact_id, fact_type, content, quote, source_episode, t_valid, t_ingested, t_invalid, t_invalid_ingested, confidence, index_keys, access_count, last_accessed, entity_links, scope, policy_tags, provenance`. Для decay создаётся отдельный knowledge-owned query builder и метод graph store; не обеднять общий active read до шести полей.

ANN response без vectors допустим, когда DB возвращает finite numeric `sem_score`. Существующий semantic fallback для строк без score, но с embedding, сохраняется для альтернативного adapter/test port. При отсутствии обоих полей запрещено изобретать положительный similarity или успешно выдавать ложное совпадение.

## Fixtures и методы измерения

Нормативный synthetic fixture v1:

- Dimensions — 2048; числа vector[k] = `(k % 17) as f64 / 17.0`; fixed confidence 0.8. Никакого external embedding API при fixture seed.
- Isolated adapter cases: 300 и 1000 rows, ordered IDs `fact:mem-000000` и далее; `content="synthetic memory fixture"`; metadata timestamps `2026-01-01T00:00:00Z`; поле `t_invalid` отсутствует. Для scalar/identity contracts отдельные маленькие fixtures.
- Full HTTP: 306 facts, из них 300 active и 6 closed; 41 episodes; vectors 2048 у всех 306. Closed facts имеют обе temporal close dates. Основная performance fixture высокоуверенная и не должна случайно decay от текущей даты; в test env threshold 0.0 для стабильного workload. Отдельный correctness case — один старый fact confidence 0.4, возраст 400 дней, threshold 0.3, half-life 100 дней.
- Concurrency case: два ready tenants с тем же dataset каждый; 4 MCP-запроса на tenant = 8 суммарно; maintenance parallelism 1. Это проверка текущих production caps, не разрешение менять caps.
- Metadata/content полностью synthetic. Retrieval correctness использует отдельные literals: persona fact, recent fact, exact lexical match, in/out-of-window event, semantic vectors `e0` и `e1`, scoped/protected fact и predecessor/successor.

Isolated adapter замер — отдельный integration-test child с counting `System` allocator, настоящий `SurrealDbClient` и отдельный remote SurrealDB process. Counter учитывает alloc/alloc_zeroed/realloc/dealloc, включая SDK response decode; baseline снимается после connect и seed, окно начинается до запроса и заканчивается после drop результата. Не сбрасывать live counter при существующих allocations: peak delta = peak live in window − live at window start. Counting allocator не добавляется в application binary.

Worker thread жив до post-drop sample: exit thread не должен маскировать glibc retention. Child имеет deadline 120 s и гарантированный kill+wait в teardown. На один измеряемый процесс — один scenario, `--test-threads=1`. Не профилировать embedded DB внутри application process и не вычитать «неудобные» allocations.

Native Linux `x86_64-unknown-linux-gnu`, `MALLOC_ARENA_MAX=2`, default System allocator. Для G2 counting-allocation gate libc version записывается; для G4 retention/full-app gate обязательна glibc `2.41-12+deb13u4`, как на production. QEMU/emulation, macOS и Linux arm64 — вспомогательные результаты, не PASS release gate. Измеряемый app container отдельно от SurrealDB/provider/sampler, с RAM 1 GiB и RAM+swap 2 GiB. Контроль legacy amplification можно запускать с 3 GiB, чтобы получить assertion failure, а не OOM; это не лимит corrected candidate.

Для полного HTTP измерять `/proc/<pid>/status` (VmRSS, VmHWM, VmSwap), application cgroup `memory.peak`, `memory.current`, `memory.swap.current`, `memory.events` и existing allocator/owner gauges. Sampler — существующий `cargo run -p xtask -- sample-memory`, 50 ms interval; sampling стартует **до** первого maintenance и охватывает cold start. Peaks подтверждаются HWM/cgroup peak, а не максимумом редких Grafana scrape.

## Блокирующие quality gates

Все пороги ниже — **проектные acceptance targets**, а не уже измеренные показатели исправленного приложения. Изменять их для превращения FAIL в PASS запрещено; пересмотр — отдельное review дизайна с raw evidence.

| Gate | Условие PASS |
|---|---|
| G0: RED provenance | Каждый production change имеет предшествующий failing regression; failure по ожидаемому contract/bytes, не typo/build error. Сохранены command, exit code, assertion и commit SHA. |
| G1: scalar/identity correctness | Все named storage и use-case contracts pass; нет потери RecordId/date/decimal/null/bi-temporal/access semantics. |
| G2: adapter 300 rows | peak live allocation delta `< 67_108_864` bytes (64 MiB). |
| G2: adapter 1000 rows | peak delta `< 201_326_592` bytes (192 MiB), и `peak_1000 <= 4 * peak_300`; каждый размер — отдельный тест/process. |
| G3: projections | Decay, active, scans и ANN реально возвращают нужные поля без embedding; retrieval IDs/order/visibility прежние. |
| G4: full app | На cold start, repeat decay, distinct retrieval workloads и concurrency: VmHWM/peak RSS `< 268_435_456` bytes; steady RSS+VmSwap `< 134_217_728`; VmSwap = 0; application cgroup `memory.peak < 335_544_320` (320 MiB); `oom`/`oom_kill` delta = 0. |
| G4: plateau | 20 warmup + 100 **cache-miss** requests; plateau medians в post-request idle checkpoints, последний блок 20 против первого блока 20 после warmup: рост RSS+swap `<= 8_388_608` bytes. Ни одного failed/error tool response. |
| G4: repeated runs | Три fresh processes для каждого full-app scenario; должны пройти все три, не только среднее. |
| G5: release quality | Full default workspace test command и HTTP optional suite pass; fmt, обе clippy feature rows, toolchain pin, observability checks pass; final review без открытых P0/P1. |
| G6: evidence integrity | Raw results, platform/libc/digests/binary hash/Git SHA/lock hash/config/fixture digest/commands/exit codes присутствуют и совпадают. Missing samples/platform/fixture/error outcome = FAIL, не skip/pass. |

Steady checkpoint: wait inflight=0 and required background job completion under a deadline, затем 30 s наблюдения; максимум RSS+swap этого окна должен уложиться в steady threshold. Polling сигналов и sampling могут иметь интервал; запрет sleeps относится к установлению причинного порядка unit/concurrency tests — там barrier/permit/explicit polling. Cold decay completion отслеживается по job metric delta/log completion и seed-fact readback, не по router ready.

100 запросов должны обходить context cache через уникальные query strings, которые сохраняют тот же проверяемый lexical/temporal intent; dashboard/wake-up cases получают новые request objects/cutoffs. В артефакте подтверждается delta реальных DB queries; 100 cache hits не закрывают гейт. Отдельно проверить warm-cache path и release ресурсов при error/cancellation.

Full-app fixtures предварительно записываются в durable remote stores через существующие owner stores/production composition **до запуска измеряемого процесса**. HTTP application запускается без `MEMORY_MCP_HTTP_TEST_BOOTSTRAP` и без `test-fixtures` feature. Controlled external OpenAI-compatible provider нужен для deterministic dimension 2048; измеряется приложение, provider process отдельно. Это тест application storage/memory, не оценка качества реальной модели. Возможность реального provider проверяется отдельно при staging review с явной маркировкой измерений.

CI выполняет resource cases native Linux amd64 и публикует raw artifacts даже при FAIL; шаг не имеет `continue-on-error`. Missing Docker/glibc/fixture не является success. ColdDecay deadline — 4000 s, остальные scenarios — 600 s; три repetitions разделены на matrix jobs, каждый timeout 120 min. Перед production rollout требуется отдельный полный staging A/B на pinned production image и corrected candidate; CI process harness не заменяет Dockerfile/runtime composition check.

## Выход и следующий проект

Результат данного объёма — branch/PR с доказанным устранением amplification, проекциями и автоматическими regression gates, плюс staging evidence. Production deployment и post-deploy readback выполняются только после отдельной авторизации. Rollback заранее фиксирует baseline digest и criteria: OOM, swap, неверные IDs/visibility, error-rate regression или выход за memory gate.

Следующий spec про pagination/admission должен охватить anchors across all pages, stable snapshot/cursor semantics, byte bounds до большого SDK materialization и общий budget maintenance+foreground. Его starting evidence — остаточный P0 профиль и fixtures 10K/100K с variable-size content. До такого проекта не называть `scan_facts` ограниченным по памяти.
