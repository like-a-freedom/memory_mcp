# Storage materialization memory remediation — design and acceptance contract

**Дата:** 2026-10-13. **Статус:** проект для review; реализация не начата.

## Основание и границы доказательства

Источник: [анализ production и причинный Linux A/B](../../performance/MEMORY_ROOT_CAUSE_2026-10-13.md), с сохранёнными JSON-результатами и SHA-256 в `docs/performance/memory-root-cause-2026-10-13/manifest.json`.

Подтверждённый механизм: `SELECT *` приносит ненужные embeddings; generic serde serialization создаёт два tagged object на каждый float; `extract_records` глубоко клонирует дерево; glibc удерживает освобождённые страницы. В Linux arm64-контроле 300×2048 дали 1533,93 MiB peak live allocations; удаление только clone — 800,03 MiB; прямая конвертация — 33,10 MiB. Это не измерения исправленного HTTP-приложения.

Production — `memory_mcp_http` 1.34.0, Linux amd64, glibc `2.41-12+deb13u4`, remote SurrealDB 3.3.2 и remote embedding provider с dimension 2048. Лимиты: RAM 1 GiB, RAM+swap 2 GiB, `MALLOC_ARENA_MAX=2`. Production baseline image:

`ghcr.io/like-a-freedom/memory_mcp@sha256:6f374a20668a3e2003727636c937c0448e90c86eb256dc9c0b5731c3c3ef243b`

Git revision этого image не доказан: OCI revision label отсутствует. Source release `bec27690df8578b807d76fee4551b2687acbc983` служит source baseline, а не доказательством состава image. Окончательный гейт требует нового полного Linux amd64 A/B.

## Цель и объём

Устранить подтверждённое раздувание materialization, прекратить чтение embeddings там, где они не нужны, и закрепить memory acceptance исполняемыми тестами **без регрессии производительности и метрик качества**. Проверяются разные размерности, отсутствие vectors, выключенный provider и переходы через reembed. Общий adapter обслуживает CLI/stdio и HTTP; исправление не должно зависеть от caller или allocator feature.

Обязательный объём: consuming conversion и extraction; отдельная decay projection; проекции active reads, nonsemantic full scans, ANN response и reembed batch reads; совместимость storage/use-case contracts; Linux memory regressions; гейты CI и production composition. P0 и проекции выпускаются одним проверенным изменением: исправление только clone недостаточно.

Отдельный следующий проект: SQL pushdown, pagination и общий runtime admission budget для произвольно больших tenant datasets. Текущий план задаёт проверяемый бюджет на фиксированных fixtures, **не обещает глобальную bounded-memory гарантию для неограниченного scan**. `rank_lexical_records` строит anchor profile по всему candidate set; простой `LIMIT` или независимый top-K каждой страницы меняет ранжирование. Это требует собственного дизайна и проверки recall, а не дополнения к срочному adapter fix.

Allocator switch, periodic `malloc_trim`, увеличение RAM/swap, изменение production concurrency и отключение lifecycle в эту реализацию не входят. Они не устраняют исходный amplification. После P0 остаточный профиль решает, нужны ли дополнительные меры.

## Неизменяемые ограничения реализации

- Rust `1.99.0`; SurrealDB/SDK `3.3.2`; `serde_json` `1.0.151`; использовать текущий `Cargo.lock`, команды с `--locked`.
- Новых зависимостей, миграций, generated-code изменений и MCP tools нет. `Cargo.toml` не меняется.
- Один Active Namespace, выбранный при binding. Не вводить namespace/table selector в application-facing read.
- В HTTP production build: `streamable-http,mcp-apps` плюс существующий default `fs-watch`; без `mimalloc` и `test-fixtures` в измеряемом application binary.
- Ошибки остаются `MemoryError`; никакой ошибки query/take/namespace нельзя превращать в пустой успешный ответ.
- Политика хранения неизменна: facts не удаляются; decay retracts через `ClaimStore`, атомарно закрывая fact и derived claims.
- Не менять temporal predicates, ordering, row limits, relevance thresholds, cache defaults, job cadences или алгоритмы ranking в рамках проекций. Единственное отдельно разрешённое исправление semantic admission: строка без валидного score и пригодного fallback vector не является семантическим совпадением даже при threshold 0 или -1; правка только после отдельного RED. ColdDecay acceptance сохраняет decay cadence 3600 s; повторный pass в том же процессе реально дожидается следующего cadence.
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
| Reembed batch | `fact_id, fact_type, content, quote` | stale-signature predicates, cursor, ORDER BY fact_id ASC, limit 100; subquery workaround сохраняется, старый embedding не читается |
| ANN | `id` + existing `NONSEMANTIC_FACT_PROJECTION` + computed `sem_score` | vector predicate и cosine вычисляются в DB; K/ef_search/filter/order/limit прежние |

`NONSEMANTIC_FACT_PROJECTION` — `fact_id, fact_type, content, quote, source_episode, t_valid, t_ingested, t_invalid, t_invalid_ingested, confidence, index_keys, access_count, last_accessed, entity_links, scope, policy_tags, provenance`. Для decay создаётся отдельный knowledge-owned query builder и метод graph store; не обеднять общий active read до шести полей.

ANN response без vectors допустим, когда DB возвращает finite numeric `sem_score`. Существующий semantic fallback для строк без score, но с embedding, сохраняется для альтернативного adapter/test port. При отсутствии обоих полей строка не семантический кандидат, включая threshold 0 и -1. Missing score + vector другой длины также не кандидат. Finite score остаётся authoritative; invalid numeric score даёт прежнюю validation error, malformed score не становится ложным совпадением. Fallback действителен только при пригодном vector той же длины, что query. Это отдельный RED → repair, а не скрытая правка ranking.

## Fixtures и методы измерения

Нормативный synthetic fixture v2 (v1 — историческое доказательство, его JSON/hashes не перезаписывать):

- Основной причинный fixture — 2048; обязательная матрица dimensions `17, 384, 768, 1536, 2048, 3072` и отсутствие embedding. 17 — защита от hardcoded width, не модель и не обещание поддержки всех возможных размеров. Числа vector[k] = `(k % 17) as f64 / 17.0`; fixed confidence 0.8. Никакого external embedding API при fixture seed.
- Isolated adapter cases: 300 и 1000 rows, ordered IDs `fact:mem-000000` и далее; `content="synthetic memory fixture"`; metadata timestamps `2026-01-01T00:00:00Z`; поле `t_invalid` отсутствует. Для scalar/identity contracts отдельные маленькие fixtures.
- Full HTTP: 306 facts, из них 300 active и 6 closed; 41 episodes; vectors выбранной dimension у всех 306 для enabled cases; в no-vector case поле отсутствует. Disabled-with-existing-vectors сохраняет старые vectors и выключает provider. Closed facts имеют обе temporal close dates. Основная performance fixture высокоуверенная и не должна случайно decay от текущей даты; в test env threshold 0.0 для стабильного workload. Отдельный correctness case — один старый fact confidence 0.4, возраст 400 дней, threshold 0.3, half-life 100 дней.
- Concurrency case: два ready tenants с тем же dataset каждый; 4 MCP-запроса на tenant = 8 суммарно; maintenance parallelism 1. Это проверка текущих production caps, не разрешение менять caps.
- Metadata/content полностью synthetic. Retrieval correctness использует отдельные literals: persona fact, recent fact, exact lexical match, in/out-of-window event, semantic vectors `e0` и `e1`, scoped/protected fact и predecessor/successor.

Remote fixture с dimension D использует public `SurrealDbClient::from_prebound_remote_with_dimension(Surreal<Client>, &str, &str, usize)` после однократного raw SDK bind; миграции и HNSW имеют D до seed. `connect_bound` рендерит default 1536 и не подходит для миграции произвольной dimension. В no-vector fixture schema width остаётся валидным positive default; отсутствие vector не кодируется schema DIMENSION 0.

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
| G2: adapter 300 rows | D≤2048: peak delta `< 67_108_864`; D=3072: `< 100_663_296`; без vectors: `< 16_777_216` bytes. |
| G2: adapter 1000 rows | D≤2048: `< 201_326_592`; D=3072: `< 301_989_888`; без vectors: `< 50_331_648`; `peak_1000 <= 4 * peak_300` в одной dimension/mode. Отдельный child для каждой ячейки. |
| G3: projections | Decay, active, scans, reembed batch и ANN реально возвращают нужные поля без embedding; retrieval IDs/order/visibility прежние. |
| G4: full app | На cold start, repeat decay, distinct retrieval workloads и concurrency: VmHWM/peak RSS `< 268_435_456` bytes; steady RSS+VmSwap `< 134_217_728`; VmSwap = 0; application cgroup `memory.peak < 335_544_320` (320 MiB); `oom`/`oom_kill` delta = 0. |
| G4: plateau | 20 warmup + 100 **cache-miss** requests на lexical/temporal/semantic enabled subcase; plateau medians в post-request idle checkpoints, последний блок 20 против первого блока 20 после warmup: рост RSS+swap `<= 8_388_608` bytes. Ноль неожиданных failures. ErrorRelease фиксирует exact injected failures/cancellations отдельно, не вычитает их молча из total. |
| G4: repeated runs | Три fresh processes для каждого full-app scenario; должны пройти все три, не только среднее. |
| G5: release quality | Full default workspace test command и HTTP optional suite pass; fmt, обе clippy feature rows, toolchain pin, observability checks pass; final review без открытых P0/P1. |
| G6: evidence integrity | Raw results, platform/libc/digests/binary hash/Git SHA/lock hash/config/fixture digest/commands/exit codes присутствуют и совпадают. Missing samples/platform/fixture/error outcome = FAIL, не skip/pass. |
| G7: embedding lifecycle | Вся mode/dimension/transition matrix ниже; durable vector width/signature/index/state/cursor проверены; no-vector retrieval работает; explicit disabled не зовёт provider. |
| G8: performance | Comparable paired A/B по каждому workload/mode; point latency/CPU ratios ≤1.00, throughput ≥1.00; 95% uncertainty limits 1.03/0.97; ошибки 0; incomplete/inconclusive = OPEN. |
| G9: quality | Native release profile PASS, все шесть retrieval-quality метрик =1.0; paired quality regression budget 0; per-case golden IDs/visibility и режимные quality метрики не ниже baseline. |

Steady checkpoint: wait inflight=0 and required background job completion under a deadline, затем 30 s наблюдения; максимум RSS+swap этого окна должен уложиться в steady threshold. Polling сигналов и sampling могут иметь интервал; запрет sleeps относится к установлению причинного порядка unit/concurrency tests — там barrier/permit/explicit polling. Cold decay completion отслеживается по job metric delta/log completion и seed-fact readback, не по router ready.

Miss подтверждается existing cache-owner hit/miss counters и фактически выполненным foreground storage read для данного request_id. Новый request object сам по себе cache key не меняет. Для query-less wake-up/dashboard выбирать разные реальные cache-key inputs; если path не кешируется, документировать это по code и проверять DB-read, не называть его cache miss. Нельзя отключать cache ради gate. Context queries меняются literals так, чтобы сохранять intent; expected answers каждого query заранее зафиксированы. 100 cache hits не закрывают гейт. Отдельно проверить warm-cache path и release ресурсов при error/cancellation.

Full-app fixtures предварительно записываются в durable remote stores через существующие owner stores/production composition **до запуска измеряемого процесса**. HTTP application запускается без `MEMORY_MCP_HTTP_TEST_BOOTSTRAP` и без `test-fixtures` feature. Controlled external OpenAI-compatible provider параметризуется dimension/model/signature и возвращает ненулевые content-dependent vectors; identical constant vectors не подходят для quality/ranking tests. Dimension 2048 остаётся causal control. Измеряется приложение, provider process отдельно. Это тест application plumbing, не качества реальной модели; реальный provider проверяется при staging review с явной маркировкой измерений.

CI выполняет resource cases native Linux amd64 и публикует raw artifacts даже при FAIL; шаг не имеет `continue-on-error`. Missing Docker/glibc/fixture не является success. Parent/candidate images и same-PID launch wrapper строятся один раз в prerequisite image-build job через Cargo test entry point; immutable OCI bundle передаётся измерительным jobs вместе с manifest и hashes. Измерительные jobs проверяют source SHA, features, binary/runtime hashes и identity wrapper, импортируют bundle и не повторяют Rust/Docker build внутри бюджета ColdDecay. ColdDecay deadline — 4000 s, остальные memory scenarios — 600 s. CI matrix разделяется по mode/dimension/scenario/repetition; один ColdDecay на job, timeout 120 min. Perf и native quality — отдельные jobs и бюджеты. Перед rollout требуется полный staging A/B; CI process harness не заменяет runtime composition check.

## Обязательная mode/dimension/transition matrix (G7)

| Ячейка | Обязательная проверка |
|---|---|
| Stable remote enabled D=384/768/1536/2048/3072 | Schema index width=D, provider width=D, stored width/signature/state; G2 для 300/1000 и G3 реальные results; полный HTTP G4/G8 для каждого D; literal retrieval goldens G9 |
| D=17 | G2/G3 и CRUD round trips проверяют всю длину/значения; маленький HNSW public-store retrieval test. Полный provider/model quality не подменяется этой синтетикой |
| Explicit disabled, нет vectors | `EMBEDDINGS_ENABLED=false`; schema dimension positive default; ingest/extract/assemble lexical/temporal/graph/wake-up/experience/lifecycle успешны. Real model provider не запускается; отдельный HTTP trap listener возвращает503 и записывает каждый request. Его журнал подтверждает 0 calls (connection refused без listener не доказывает отсутствие попыток); embedding отсутствует, ANN не выполняется |
| Explicit disabled, vectors сохранены | Старые D=3072 vectors/metadata неизменны, provider calls=0, nonsemantic paths не читают vector payload; G4/G8 и режимное качество |
| Enabled после отсутствия vectors | Real backfill закрывает missing-vector set, write metadata/index/state совместимы; scoped и invalidated fact semantics прежние; качество до/после сравнивается с baseline **в той же фазе** |
| Смена размерности | `384→2048`, `2048→384`, `1536→3072`: перед reembed documented degraded semantic readiness, lexical доступен; public CLI reembed и HTTP protected operator route, durable readback каждого fact, HNSW target width, complete state, рестарт, semantic retrieval после restart |
| Смена модели при той же D | D=768, разные signatures: старые vectors не перезаписывает ordinary startup/backfill; явный reembed меняет signature; no-op second pass не вызывает новых writes/provider generations |
| Reembed interrupt/failure/resume | 205 facts, 3 batches при limit100; controlled stop/failure после 101-го успешного fact; cursor только на последнем success; mixed old/new vectors до resume не выдаются за ready quality; resume не пропускает failed fact, завершает target metadata/index; соседний tenant не изменён |
| Invalid/mismatch | Effective enabled provider/schema D=0 не становится ready; malformed/empty/wrong-length vector, mixed stored dimensions и recovery refusal сохраняют существующий contract. Не приписывать `EmbeddingConfig::from_env` раннюю zero validation: он парсит override без этой проверки; focal test полного startup/constructor. Disabled override0 не использовать как способ убрать embedding, behavior pin по прежнему full startup contract |

Schema width, configured override, resolved provider dimension, stored dimensions и persisted signature — разные величины; manifest хранит каждую, а не один `dimension`. Positive schema width разрешён без vectors. Current HNSW index во время interrupted reembed может отсутствовать: expected state определяется existing public reembed contracts; не требовать преждевременного target-ready. Private stores проверять через публичные owner use cases и реальный executed query/results, без visibility widening. Fallback vector считается пригодным, когда existing typed parser возвращает vector той же ненулевой длины, что query; valid explicit score0/-1 остаётся допустимым при соответствующем threshold. Zero norm policy существующего cosine helper не менять этим repair.

HTTP и CLI используют один adapter, но разные composition: общий G2 и projection tests недостаточны без CLI process reembed и HTTP task dispatch. Новая автоматическая миграция/provider switch policy этой оптимизацией не вводится. Только select-list reembed сокращается до четырёх фактически потребляемых fields; nested cursor workaround в Surreal3 сохраняется. Conditional vector write продолжает читать нужную текущую identity и сохранять concurrent metadata, его read не обеднять механически.

Process composition фиксируется явно в case ID/manifest: HTTP, stdio MCP или одноразовый CLI. HTTP покрывает tenant pool, dashboard и весь concurrent workload. Long-lived локальные/remote/disabled retrieval и plateau cases выполняются через stdio MCP; CLI subprocess покрывает startup/reembed completion и durable state, не выдуманный request loop внутри завершившегося процесса. Неприменимые пары composition/scenario перечисляются заранее с причиной; обязательные CLI reembed, stdio retrieval и HTTP task cases имеют разные IDs. Binary и transport не выводятся только из embedding mode.

## Совместная приёмка memory × performance × quality

**Три baseline:** (1) pinned shipped image — causal legacy control; source SHA image не доказан. (2) immutable execution-parent SHA до первого исправления — основной performance/quality A/B, обе версии собраны одинаковыми Rust/lock/features/release flags и runtime base digest. (3) committed eval baseline — дополнительный existing release gate, не замена свежего A/B. Чужие commits должны быть одинаковыми в обеих source builds; нельзя сравнить release1.34.0 с текущей веткой и приписать всю разницу этому fix. Для distroless runtime launch barrier — отдельный std-only executable с same-PID exec, не shell/debug app hook. Все FROM stages pinned в temporary effective build recipe, original Dockerfile не меняется; libc/rootfs identity из extracted package metadata/ELF/hash. Missing exact production runtime оставляет G4 OPEN.

**G4 для controlled remote и disabled:** прежние абсолютные targets одинаковы для всех stable D≤3072 и no-vector modes; не растягивать RSS target пропорционально D, проекции убирают vectors. Cold/repeat decay, retrieval, concurrent maintenance, error recovery, Backfill, Reembed, InterruptedResume измеряются отдельными fresh app processes, все 3 repetitions обязательны. После reembed повторно выполнить steady retrieval/cache misses в том же процессе и после restart. G4 относится к 306-row fixture и 205-row transition fixture, а не к любому размеру dataset.

**Кратное снижение:** для сравнимого 2048 full-app causal control на одинаковом лимите3GiB и config peak candidate≤0.25×legacy peak (не менее4×), плюс абсолютный G4 candidate при RAM1GiB. Baseline steady может иметь swap; сравнивать RSS+swap, нельзя только RSS. Нельзя выдавать ARM allocation reduction за измеренный amd64 full-app factor. Для no-vector/малой D требуются абсолютные budgets и отсутствие regression, не произвольное обещание4× относительно уже небольшого baseline.

**Local-candle/локальный reranker:** native pinned model bundles обязательны для G9 и local performance A/B. Их resident model pages не входят в remote-only RSS target: total app memory измеряется и публикуется, отсутствие универсального абсолютного budget на все локальные модели обозначается явно. Проверяются isolated adapter G2, payload projections, total peak/steady candidate≤baseline и plateau≤8MiB после warmup (3 repeats). Отрезать model memory из report или отключить реальный provider для quality запрещено. Локальный CLI/stdio process с pinned e5/mmarco проверяется отдельно от native quality in-process suite. RAM limit local A/B3GiB обе стороны, без swap/OOM; это test constraint, не production config change.

**G8:** отдельный run без counting allocator и DEBUG SQL logging; sampler50ms одинаков на обеих версиях. Native same host/cgroup CPU limits/RAM3GiB/no-swap, отдельные DB/provider, fixed provider responses/delay, свежая DB copy per run. Memory candidate1GiB остаётся отдельным measurement. 5 пар fresh processes, чередование AB/BA с fixed schedule; calibration A/A5 пар до B. На workload:30 warmup +1000 completed requests для latency p50/p95/p99 (nearest rank); warm-cache отдельно от misses, concurrency1 и8 отдельно; fixed offered-load и closed-loop throughput отдельные subcases. Один процесс — один workload/mode; error-rate0, ordered IDs/quality literals на каждом ответе. Измерять wall time, successful requests/s, app и DB cgroup cpu.usage_usec на успешный request раздельно, provider time; startup ready time и reembed facts/s отдельно. CPU gate применяется к app и DB, перенос нагрузки не free improvement. Log/NER/fs-watch/cache/background settings одинаковы A/B, компоненты нельзя выключить для PASS. Provider latency не вычитать из end-to-end. Ошибки/timeout не исключаются и не ускоряют throughput.

Point estimate — median пяти run-level ratios: latency/CPU/ready duration≤1.00, throughput/reembed facts/s≥1.00. Fixed-seed20261013,10_000 paired block-bootstrap resamples **целых пар runs**, не отдельных коррелированных requests: односторонняя95% upper bound lower-is-better≤1.03, lower bound higher-is-better≥0.97. 3% — предел неопределённости измерения, не разрешение систематически замедлить сервис: point ratio>1 уже FAIL. Calibration A/A: каждая run-level metric ratio в[0.97,1.03], иначе invalid; calibration не применяет direction-only point gate. Missing baseline, wrong host/config/mode, incomplete sample count либо wide interval =OPEN/invalid. Никакого baseline update, усреднения workloads/modes или компенсации regression ускорением другого пути. Это проектные criteria, не доказанные показатели; конечная матрица не доказывает математическое отсутствие slowdown на всех будущих нагрузках. Pair-set на одном exclusive native host; steady perf run deadline1800s, full runner reservation фиксируется до A/B по pilot/coverage manifest. Hourly memory cases и native release profile1200s — отдельные jobs; timeout/нехватка runner оставляют gate OPEN.

30warmup+1000requests относится к long-lived HTTP/stdio retrieval measurements. Startup/backfill/CLI reembed вместо request loop имеют5paired complete actions с independently verified durable results; CPU нормируется на завершённый work unit (request/fact/action явно записан в artifact). Эти cases не могут закрыть retrieval latency gate и наоборот. HTTP-only dashboard/tenant cases и disabled semantic cases помечаются reviewed N/A; stdout MCP protocol не подменяется CLI запуском на каждый запрос.

**G9:** существующий `memory-eval run --profile evals/profiles/release.json` с настоящими pinned e5/mmarco models и source-reviewed corpus. Все37 retrieval-quality cases executed; все6 aggregate metrics =1.0: `precision_macro`, `required_group_recall`, `unknown_abstention_rate`, `answerable_non_abstention_rate`, `duplicate_free_rate`, `prohibited_free_rate`. Full release profile coverage также полная по pinned manifest (сейчас154 cases). Candidate проходит existing committed floors и свежий source-parent A/B с **regression budget0** для quality metrics во всех suites; existing budgets0.1 не разрешают ухудшение этим change. Нулевая coverage/invalid/skip/missing ratio population — OPEN, а не fabricated1.0.

Native extraction может порождать другие IDs/ingestion times: сравнение связывает facts с frozen source/content keys и temporal instant, не случайным UUID. Дополнительно режимная literal matrix через реальный HTTP и CLI фиксирует Recall@5, MRR, nDCG@5, unknown abstention, prohibited/duplicate-free rates и exact ordered IDs для deterministic inputs. Corpus минимум20 answerable queries +5 unknown +5 protected/historical negatives на mode; candidate не ниже source baseline ни по applicable aggregate, ни по per-case required evidence. Semantic quality no-vector modes — N/A с причиной; lexical/temporal/graph quality обязательны. Fake vectors проверяют plumbing/identity/ranking, не качество модели. При новой модели сравнивать A/B **с одинаковой новой моделью и фазой transition**, качество старого provider не oracle новой модели. Reranker on/off проверять раздельно.

## Выход и следующий проект

Результат данного объёма — branch/PR с доказанным устранением amplification, проекциями и автоматическими regression gates, плюс staging evidence. Production deployment и post-deploy readback выполняются только после отдельной авторизации. Rollback заранее фиксирует baseline digest и criteria: OOM, swap, неверные IDs/visibility, error-rate regression или выход за memory gate.

Следующий spec про pagination/admission должен охватить anchors across all pages, stable snapshot/cursor semantics, byte bounds до большого SDK materialization и общий budget maintenance+foreground. Его starting evidence — остаточный P0 профиль и fixtures 10K/100K с variable-size content. До такого проекта не называть `scan_facts` ограниченным по памяти.
