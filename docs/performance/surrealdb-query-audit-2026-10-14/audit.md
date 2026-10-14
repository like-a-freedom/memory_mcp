# SurrealDB: расширенный аудит запросов, 2026-10-14

Базовый commit: `bb4e9208c3054811e4fb883f28ad2145c4be65e7`, production app 1.34.2, SurrealDB/SDK 3.3.2. Исследование read-only. Реализация, миграции, production configuration и данные не изменялись. Существующие незакоммиченные правки CI сохранены.

Этот раздел фиксирует исходное состояние на момент read-only аудита. Результаты последующей локальной реализации приведены отдельно ниже; они не меняют и не переинтерпретируют production captures.

## Вывод

Кроме доказанного session/password-auth churn обнаружены **неприменённые миграции ready-tenant**, **неиндексируемый batch graph query**, **дорогой пустой backfill probe**, повторная работа artifact reconciliation, N+1 и широкие чтения отдельных retrieval/lifecycle-путей. Их нельзя считать одинаковыми причинами постоянного CPU: первые фоновые запросы работают без клиентов; retrieval-ветки вызываются запросами, а artifacts сейчас отсутствуют.

Заявление «100% CPU объяснено» не обосновано. Есть read-only query profiles и причинный локальный auth A/B, но нет production symbolized CPU flamegraph или controlled full-service A/B. Остаток CPU и logical I/O не атрибутирован.

## Доказательства и воспроизводимость

- [Предыдущий idle CPU анализ](idle-cpu-investigation.md): passive WS capture, fixed Grafana windows, process/cgroup deltas, exact-image auth experiment.
- [Профили запросов](query-probes.json): 30 проб, включая повтор одного backfill baseline; 29 различных форм. Не все пробы — точные production SQL: упрощённые формы обозначены ниже.
- [SQL manifest](query-manifest.json) фиксирует владельца, caller, сценарий, plan, число samples и имеющиеся row counts для каждой пробы, а также явные `unmeasured`-строки для неохваченных owner paths. Его можно пересобрать командой `rtk run python3 docs/performance/surrealdb-query-audit-2026-10-14/build_query_manifest.py`. `sampleCount` — число диагностических повторов, не число SQL/RPC; `rowBytes`, RPC и signin остаются `null`, потому что эти данные не собирались.
- [Production schema](production-schema.json): безопасные schema/ledger/inventory aggregates.
- Скрипты `surreal_read_audit.py`, `surreal_read_audit_deep.py`, `surreal_read_audit_ann.py`, `surreal_read_audit_backfill.py`, `surreal_read_audit_invariants.py`: запускать **через stdin SSH**, не записывая ничего на сервер. Credentials/token остаются внутри процесса; печатаются schema, counts, plans и timings, но не content/vectors/passwords. Для воспроизведения first/deep/ANN script: `rtk proxy sh -c 'ssh -o BatchMode=yes -p 2288 root@82.165.149.36 python3 < docs/performance/surrealdb-query-audit-2026-10-14/surreal_read_audit_deep.py'`. Это диагностическая нагрузка, не нагрузочный production benchmark.
- Серии production probes: начало 12:26:45, 12:29:34, 12:31:04 и 12:35:35 UTC; короткие bearer HTTP SELECT/INFO/EXPLAIN, 5 или 7 повторов. Sequence/fields прочитаны отдельно. Exact capture end не записан; сохранены исходные transcripts рядом со скриптами. Файлы `*.jsonl` содержат смешанные diagnostic lines и JSON records: для machine-readable consumption использовать `query-probes.json` / `production-schema.json`. Скрипты привязаны к текущему container name/IP; перед повтором проверить read-only inventory. Wall time включает network/server scheduling/serialization. Для deep probes дополнительно сохранена медиана SQL time.
- Bound graph A/B имеет полный product temporal predicate, тот же cutoff, четыре реальных seed IDs, тот же порядок и projection. Все 14 результатов имеют один SHA-256. Это доказательство эквивалентности **этой пробы**, не всех возможных входов.
- ANN использует synthetic `e0` размерности 2048; K/EF соответствуют budget=5 без reranker / с reranker. Это проверка query plan и стоимости; не оценка recall реальных вопросов.
- Первые browse/substring/entity/claim probes местами сокращают projection/predicate. Их latency — диагностический ориентир, не latency capability. Упрощённый `tasks_due` использовал неправильное имя lease field; исключён из выводов о точном task claim. Code claim использует `lease_expiry`; его exact performance предстоит измерить на fixture.

## Новые подтверждённые находки

| ID | Находка и root mechanism | Подтверждение | Приоритет / план |
|---|---|---|---|
| Q1 | Ready tenants не получают base catalog updates. Provisioning выбирает только due states и `provision_one` возвращает сразу для Ready; activation выполняет dimension reconciliation, но не общий catalog/readiness. | Production ready, schema=44; ledger заканчивается 039. 053/054/055 отсутствуют; отсутствуют episode ID/time/source-context, fact source/time, edge time indexes и source fields, task kind. Source: `http/leases/migration.rs:65,236,511`; `http/runtime/storage.rs:502`; `storage/migrations.rs:164`. | P0: upgrade lifecycle + readiness, задачи 2–3. Не добавлять заново уже существующие индексы. |
| Q2 | `entity_aliases`/`community_members` имеют старые whole-array definitions. Наличие имени не гарантирует membership access. | Production `FIELDS aliases` / `FIELDS member_entities`; `CONTAINSANY` — TableScan. 054 уже меняет на `.*`. `verify_schema_postconditions` проверяет только имена. | P0: проверить definition + реальные plans после upgrade, задача 3. |
| Q3 | Batch graph expansion остался на `<record>` cast, хотя одиночные neighbor builders уже используют two-part `type::record`. | `knowledge/queries.rs:307`; `retrieval/graph.rs:178`. Exact 4-seed probe: TableScan → UnionIndexScan edge_in/out; wall median 29.806 → 2.787 ms; SQL median 18.839 → 1.371 ms; одинаковые 9 rows/hash. | P1: задача 6; сохранить batch size 32, все edges, temporal filters/path provenance. |
| Q4 | Пустой backfill всё равно делает count, затем SELECT; planner выбирает fact_id index и late embedding filter, читая кандидатов до конца. | Все 411 vectors уже 2048, результат пуст. Product first-page SELECT*: ~155 ms median. Повтор baseline ~145 ms, `WITH NOINDEX` ~17.6 ms, тот же пустой результат. Это исходные production probes, а не контролируемый workload A/B. | P1: задача 9 с density/scale gates. Не объявлять IndexScan автоматически хорошим планом и не включать принудительный hint до G4. |
| Q5 | Artifact reconciliation повторно читает весь committed history и отправляет условный UPDATE для каждого artifact каждый pass. | `http/tasks/worker.rs:420`: upper bound + pages64 + A conditional UPDATE, каждый scheduler tick. Уже terminal artifacts остаются committed. CPU этой ветки в текущем production не измерен: artifacts=0. | P1 scaling: задача 5; pending-only bounded reconciliation с recovery/fencing regression. |
| Q6 | Lexical fallback делает последовательный SQL для каждой phrase/singleton, даже если initial не пуст; episode fallback повторяет substring scans после miss. | `retrieval/lexical.rs:32,171,738`. Full temporal FTS absent-term probe использует FullTextScan, ~2.8 ms wall. Количество fallback terms зависит от вопроса. | P2: задача 8, batching при сохранении per-term cap/order/score; не удалять terms/каналы. |
| Q7 | Lexical low-overlap, temporal window, wake-up читают все nonsemantic facts; facets читает все episodes. | `scan_facts` callers lexical:269, temporal:569, views:319; `scan_episodes` views:221. Probe fact projection 411 rows ~144 ms median; browse 50 всё ещё TableScan на legacy schema. | P2: задача 7. Полный scan иногда семантически необходим: relative event dates и global anchor mass исключают наивный WHERE t_valid/window или ранний LIMIT. |
| Q8 | Source siblings читаются по episode последовательно через SELECT* и возвращают embedding, хотя downstream нужен Fact без vector. Archival existence вызывает тот же широкий reader. | `knowledge/graph_store.rs:201`; pipeline:727,1095,1377; archival:148. Production missing provenance index усугубляет чтение. | P1: задача 7 — projection/existence; задача 8 — per-seed batching только после equivalence. |
| Q9 | Hubs загружают все entities, затем берут ограниченный prefix и делают два neighbor SQL на entity. Archival делает два existence query на каждый из ≤500 episodes. Community rebuild — offset pages + накопление всех active edges. | `retrieval/graph_reads.rs:102`; `lifecycle_workers/archival.rs:106`; `communities.rs:208`. Не постоянная idle причина; источник роста query count/latency. | P2: задача 10, только сохраняющий coverage batching. Community offset/snapshot — эксперимент, без самовольной смены алгоритма communities. |
| Q10 | Outbox repair создаёт случайный record вместо `:default`; повтор HTTP 042 reset-ит существующий counter. | subscriptions/scheduler: SELECT default, затем `CREATE tenant_change_sequence`; migration042: `UPSERT ...:default SET value=0`. Production здоров: 1 row, counter=latest event=1341. Это latent recovery/upgrade defect, не доказанная активная CPU причина. | P0: задача 2 **до** schema upgrade. Regression next event=1342, never reset to1. |

## Покрытие остальных семейств

### Решения реализации после аудита (2026-10-14)

Локальная матрица Task 9 проверила плотность, размер таблицы, размерность и
проекцию, но не выполнила парный production-shaped G4 с одновременными app/DB
окнами. Поэтому `WITH NOINDEX` оставлен только в изолированных benchmark
кандидатах и удалён из production backfill SQL. Production сохраняет planner
default и узкую проекцию `fact_id,fact_type,content,quote`; count, eligibility
(`embedding IS NONE`), keyset cursor, повторное обнаружение поздней записи,
FillMissing CAS и reembed policy не меняются. Измеренные latency этой матрицы
не считаются доказательством снижения CPU или общей производительности сервиса.

Обзор code graph, signatures и AST inventory охватывает `query`, `query_rows`, registry `query_json`, typed select/create/update funnels. В AST выборках 131/47/58 call sites соответственно, **включая тесты**; это не число production SQL templates. SQL templates с dynamic predicates/transactions требуют отдельного manifest в задаче 1. Ниже — audit disposition; «проверено» не означает, что каждый возможный shape имеет production profile.

| Семейство / владелец | Проверено | Решение |
|---|---|---|
| Registry account/identity/API key/session/OIDC/local-admin/quota | Owner/query call inventory; ID-bound lookups, state/epoch/CAS/admission/retry; local-admin rate cleanup cadence300s. Due/ready tenant TableScan ранее измерен на 1 tenant. | Не кешировать auth/revocation/quota решения ради CPU. Scaling tenant discovery/paging в задаче5; остальные exact templates — baseline manifest, без доказанного нового idle culprit. |
| Provisioning/deletion migrations and leases | Ready no-op; catalog ledger; compatible schema range; due limit100; dimension gate; data deletion recovery owner `operations/api.rs`. | Upgrade должен быть fenced и отдельно сохранять deletion/suspended terminal decisions. schema44 совместима, но не доказывает catalog readiness. |
| App session persistence / tasks | Admission expiry, task lease/state/fencing, cleanup/reconcile code; production tables пустые. | Разделить security admission и физическую уборку; claim/heartbeat не замедлять. Не обосновывать новые task indexes пустой таблицей. |
| Subscriptions/outbox | Product next_batch WHERE sequence/resource filter, ASC LIMIT256; latest probe индексирован ранее; counter healthy. | Сохранить polling/replay deadlines. Repair/cadence можно отделить от delivery; counter upgrade fix обязателен. |
| ANN / embedding identity | Production HNSW2048, KnnScan; K20 EF80 ~85.8 ms wall, K50 EF200 ~140.6 ms wall. Adapter projection не возвращает vector, cosine считается DB. | Нет доказанного дефекта ANN index selection. Не менять metric, precision, K/EF, threshold, reranker/admission. Больше performance/recall доказательств в задаче11. |
| Entity resolve / aliases / triples / graph | Canonical name uses index; alias legacy scan; batch cast; exact endpoints builders already two-part; edge/community membership issue. | Исправить deployment schema и batch cast, не переделывать правильные одиночные builders. Unicode/quoted IDs должны проходить regression. |
| Claims / temporal relations / reconciliation | ID/source/slot/OR relation query shapes; source uses claim_source_projection_idx; relation bitmap indexes; claim_id lookup TableScan. | Claim-ID additional index — conditional scaling experiment, не обязательная migration. Сохранить null/legacy identity, validity, ordered cap и atomic reconciliation. |
| Ingest/extract, inbox/procedures, close/invalidate | Owner stores / transaction/outbox boundaries / conflict flow / inbox CAS and leases. | Exact mutation profiles только на isolated fixtures; не выполнять ingest/extract/invalidate на production. Текущие атомарные narrow vector/access writes не заменять whole-record updates. |
| Retrieval access/query log/explain | One atomic increment per trackable item, no vector read-modify-write; query-log persistence/retention; explain source lookup. | Оценить batch RPC с сохранением increment saturation, per-item errors и durability, задача10. Query logging не отключать. |
| Lifecycle decay/archival/communities | Decay projected1000, hourly; archival500/daily N+1; communities offset paging/all-edge algorithm. | Decay не показан постоянной CPU причиной. Не менять threshold/half-life/age/coverage/algorithm. Archival batching + exact existence оптимизация. |
| Reembed/backfill/startup | Four-field reembed projection; legacy subquery cursor workaround; count before/after batch; FillMissing CAS and dimension reconciliation. | Не убирать workaround без regression Surreal3.3.2; не считать устаревший vector missing; поддержать dims/off/transitions. Backfill density profiling задача9. |

## Результаты локальной реализации Task 10

Проверки ниже проведены на embedded SurrealDB test fixtures после локальных изменений на `master`. Они подтверждают эквивалентность выбранных чтений и форму RPC на этих fixtures; это не remote workload, не CPU/RSS/latency A/B и не доказательство full-service экономии.

| Семейство | Решение после проверки | Локальное evidence | Ограничение |
|---|---|---|---|
| Hub candidates и degree | Ограничить чтение прежним prefix и пакетно читать active edges порциями до 32 IDs, сохраняя edge identity перед подсчётом degree. | Старые per-entity incoming/outgoing edges совпали с batched edge identities; map-view сохранил порядок/degree. Для 12 кандидатов edge RPC сократились 24→1; 33 ID разбиты на 2 запроса, duplicate на границе чанков дедуплицирован. | RPC-контракт embedded fixture; row bytes, DB/app CPU, latency и production degree distribution не измерялись. |
| Archival activity и heat | Группировать только чтения сигналов в двух owner stores, батч до 64 episode IDs; оставить прежние предикаты и per-episode status writes. | Активные, late-ingested invalidation, expired invalidation, hot-only и archiveable решения совпали с одиночными owner checks. На 5 кандидатах путь составил 5 RPC против 13 старых; 65 IDs дали 4 signal-read RPC. | Нет production workload/latency/CPU измерения; общий cycle и status update стоимость не профилировались. |
| Fact access writes | Оставить по одному атомарному increment на каждый результат. | Regression сохранил duplicate increments и saturation у `i64::MAX`; unit fault case проверяет, что ошибка среднего вызова не пропускает последующие попытки. | Batch API не вводился: он должен был бы сохранить порядок, дубликаты, per-item ошибку и насыщение. |
| Claim ID lookup/index | Не добавлять индекс и migration. | Существующий probe помечен как пустой TableScan lookup; изолированная выборка не даёт write/index-memory trade-off. | Новая миграция требует отдельного решения и реальных размеров/частоты lookup. |
| Community offset paging | Не подменять offset на cursor без snapshot/closure semantics. | Fixture воспроизводит повтор ребра после ранней вставки и пропуск ребра после invalidation между страницами. | Алгоритм полного community rebuild и consistency snapshot не менялись; улучшение требует отдельной спецификации. |
| Lexical fallback visibility | Исправить временную видимость по полной bi-temporal clause; оставить полноту fallback и исходные ranking/caps. | Fixture исключает future-ingested факт и сохраняет fact с invalidation, поступившей после as-of cutoff; integration retrieval использует ровно один as-of scan. | Это correction семантики, не снижение scan объёма; rare-case provider/corpus quality comparison остаётся открытым. |

Полный regression target: `cargo test -p memory_mcp --test query_scaling_regressions --features eval-support,test-fixtures --locked -- --test-threads=1` — 10 passed. Все новые методы подключены к map-view или archival use case; single-item access API остался на прежнем пути. Изменения разбиты на коммиты `9b8fffe1`, `15980d4b`, `22dcd6fc`, `dee69ee8`.

## Что остаётся неизвестным

Не получен symbolized production CPU profile: `perf` отсутствует, бинарник stripped. Production остаётся read-only, поэтому load/mutation benchmarks там не выполнялись. Для retrieval-quality теперь есть сопоставление frozen provider/corpus на parent и candidate (см. ниже), но нет full remote application pipeline. Отдельного runnable remote fixture orchestrator для `RemoteEvidence` пока нет: код валидирует готовую пару артефактов, а не собирает её. Короткие SQL medians не являются p95/p99/SLO; absent-term probes не измеряют стоимость популярных совпадений. Numeric full-service savings forecast отсутствует.

Не нужно добавлять новую CI job: validators/fast contract tests идут в существующие cargo jobs. Native remote/full-service evidence — отдельный acceptance artifact, не абсолютный timing gate на GitHub shared runner.

## Локальные acceptance результаты Task 11, 2026-10-14

### G3: парное сравнение frozen provider/corpus

Сравнены полные release-profile отчёты до оптимизаций и после них: parent `target/evals/release.json` (run `run-1791927202`, commit `02427f4f62201cdcd14cd30f13c01dc5508df201`, SHA-256 `0c243be801099b30c718f4837b15dd777a737f010e96947ab24bb2621884bd7`) и candidate `target/evals/query-efficiency-acceptance-20261014.json` (run `run-1792008529`, commit `e16ea221ede4b52e287494e75e0df91d49243af2`, SHA-256 `1144a98a83fa7c259a75b85540b440d783102b45dcf37603968107e1cf09837`). Оба артефакта имеют одинаковые configuration и retrieval-quality suite fingerprints: corpus `retrieval-quality-v1/r27-upstream-session-order`, manifest SHA-256 `ec720673f9dc0ea2f77f278ca3224a4672fea10f2af812ef8a5b9ca945363079`, provider `local-candle`, model `intfloat/multilingual-e5-small`, dimension384, одинаковые embedding/index/reranking/model-file signatures.

На candidate прошло 154/154 cases, `quality_failed=0`, `invalid=0`. При сравнении по case ID и полям `status`, `metrics`, `evidence`, `failures` точное совпадение получено для retrieval-quality 37/37, local-retrieval 61/61, extraction 9/9, claim-reconciliation 41/41 и lifecycle 4/4. В external-retrieval совпали оба answer-presence результата; различается только диагностическое `query_ms` одного случая: 6ms против 5ms. Так как corpus/model/configuration fingerprint и результаты каждого retrieval-quality case совпадают, G3 **PASS для зафиксированного корпуса и этого provider/model профиля**. Это не доказывает качество на иных corpus/model конфигурациях или native production данных.

### Размерности, режим без embedding и remote allocation

`http_embedding_backfill` прошёл 5/5, `http_embedding_reembed` — 10/10, `embedding_memory_modes` — 14/14. Покрыты переходы `384→2048`, `2048→384`, `1536→3072`, смена signature при `768→768`, отказ для чужой размерности без вызова provider, same-width чужая signature с явным reembed, resume после частичного сбоя, embeddings-off с сохранёнными legacy-векторами и пустой namespace без ANN.

Опциональный target `storage_materialization_memory` был явно запущен: все 16/16 тестов прошли на Docker SurrealDB `3.3.2`, linux/amd64 под локальной эмуляцией. Первый запуск остановился до создания БД: Docker Desktop уже связывал multi-arch index digest с локальным arm64 образом и отказал в overwrite при amd64 pull. `docker buildx imagetools inspect` подтвердил amd64 child manifest `sha256:58944c284296507fd3cb343e7568a08dd38f397f7e763f9f54a71c5562c5839f`; fixture теперь закреплён на нём и передаёт `--platform linux/amd64`. Test worker работает на macOS/aarch64, сервер — в linux/amd64 контейнере. Проверены размерности17/384/768/1536/2048/3072 для 300 и 1000 строк, отсутствие embedding для 300/1000 строк и owner-scoped read 300 plain rows. Самый большой 1000-row пик при D=3072 — 260,153,244 bytes при лимите 301,989,888; для no-vector режима — 4,931,499 при лимите 50,331,648. Все сценарии прошли заданные budget и linear-scaling assertions. После смены platform-specific image pin повторно прошёл `storage_json_contract` 18/18. Это allocation/schema evidence; оно не измеряет полносервисные CPU/latency и не закрывает G4.

### Q1–Q10: итог решения и предел доказательства

| ID | Реализация | Локальное подтверждение | Что не доказано |
|---|---|---|---|
| Q1 | Готовые tenant проходят fenced schema/catalog upgrade; outbox sequence repair монотонен. | Миграционные и outbox regressions Tasks 2–3; 042 остаётся immutable. | Production upgrade, network/CPU cost. |
| Q2 | Membership indexes заданы по элементам массива и readiness проверяет definitions/postconditions. | Schema upgrade/readiness regressions Task 3. | Plan всех membership запросов на upgraded production data. |
| Q3 | Batch graph endpoint lookup использует constructed two-part record endpoints и bounded edge groups. | Task 6/10: прежние edge identities, порядок, degree и temporal visibility совпадают; `query_scaling_regressions` 10/10. | Production degree distribution и CPU/latency. |
| Q4 | Backfill first-page чтение сужено; cursor workaround SurrealDB 3.3.2 сохранён. | Density/mixed-signature, recovery и multi-page regressions Task 9. | Production query density/profile и performance ratio. |
| Q5 | Artifact repair ограничен pending/recoverable backlog и bounded page; terminal history не переписывается. | Task 5 paging, cancellation, fencing/recovery tests. | Сейчас в production artifacts=0, потому workload impact не измерен. |
| Q6 | FTS/substring fallback запросы сгруппированы до 8 terms, сохранены независимые group caps/order. | Task 8 compares grouped rows to sequential results; frozen G3 outputs are identical. | Популярные production terms, row bytes, CPU/latency. |
| Q7 | Nonsemantic fact/episode reads используют consumed-field projections; обязательные full scans/ranking сохраняются. | Task 7 temporal/policy/anchor/facet regressions; G3 case-level equality. | Full-index-present/absent application A/B. |
| Q8 | Source facts reuse owner projection; archival existence читает bounded ID; sibling reads batched только при сохранении per-source cap. | Projection and Task 8 group equivalence tests. | Production provenance skew and per-query resource cost. |
| Q9 | Hub candidates retain old prefix; active edge reads batched до 32, archival activity/heat до 64. Community offset algorithm оставлен. | Task 10: 12 hub candidates 24→1 edge RPC; 5 archival candidates 13→5 RPC; 65 IDs use 4 reads. | DB CPU, row bytes, latency; community rebuild remains unoptimized. |
| Q10 | Outbox uses fixed default record and preserves the next sequence across repair/upgrade. | RED/GREEN migration and subscription-replay regressions Task 2. | Production recovery path is latent and was not induced. |

### Перечень всех 42 manifest entries

Таблица учитывает каждую строку `query-manifest.json`; повтор `backfill_first_full` сохранён как два отдельных capture. «Fixture-measured» означает semantic/query-shape test на локальном fixture, но не production/full-service performance.

| Manifest IDs | Статус | Граница доказательства |
|---|---|---|
| `edge_cast`, `edge_constructed`, `graph_batch_cast`, `graph_batch_constructed` | Исправлено/fixture-measured | Графовые строки, identity и temporal predicates эквивалентны; production CPU и p95/p99 не измерены. |
| `facts_scan`, `facts_browse`, `episode_scan`, `facts_fts`, `episode_substring`, `fts_full_visibility` | Исправлено или сохранено по семантике | Bitemporal fallback, projection и grouped reads покрыты; полнота scan и ranking сохранены. Production hit-heavy workload не измерен. |
| `backfill_first`, `backfill_cursor`, `missing_vector_count`, `backfill_first_full` (capture 1), `backfill_first_full` (capture 2), `backfill_noindex` | Изучено/fixture-measured | Density, cursor/recovery и размерности проверены; hint не стал безусловным, count остался для обычного пути. Production density/performance unmeasured. |
| `artifacts_upper` | Реализовано/fixture-measured | Ограниченная pending/recovery reconciliation покрыта Task 5; production artifact table была пустой. |
| `tasks_due` | Отклонено как evidence | Исходная проба использовала неверное поле lease; product claim использует `lease_expiry`. Exact production performance не выводится. |
| `alias_lookup`, `community_members` | Schema/readiness исправлены; fixture-measured | Миграция задаёт field-level index и upgrade readiness; production post-upgrade query plans ещё не сняты. |
| `entity_batch`, `episode_id_lookup`, `source_fact_lookup`, `canonical_lookup`, `claim_by_id`, `claim_by_source`, `claim_relations`, `fact_entity_links` | Сохранено; диагностические lookup не репрезентативны | Пустые/малые production результаты не обосновали индексы или изменения алгоритма; новые расходы на запись/индексы не добавлялись. |
| `ann_k20_ef80`, `ann_k50_ef200` | Сохранено | K/EF, metric и thresholds не изменялись; G3 retrieval cases совпали. Synthetic production probe сама по себе не измеряет recall. |
| `unmeasured:http.registry:tenant discovery and pagination`, `unmeasured:http.registry:account/auth/OIDC/API-key/quota query variants`, `unmeasured:provisioning:create/upgrade/delete/suspend recovery`, `unmeasured:http.app_sessions:admission and expiry cleanup`, `unmeasured:http.tasks:claim/lease/heartbeat/requeue/reconciliation`, `unmeasured:http.subscriptions:poll/replay/outbox repair and mutation`, `unmeasured:knowledge.ingestion:ingest/extract/resolve/invalidate transactions`, `unmeasured:memory.retrieval:non-empty lexical, temporal, facets and explain`, `unmeasured:knowledge.graph:source siblings, hub, archival, community rebuild`, `unmeasured:embedding:reembed/backfill dimension and model transitions`, `unmeasured:surrealdb.session:RPC attach/use/signin/detach counters`, `unmeasured:full-pipeline:application and database CPU/RSS/I/O/latency/quality A/A` | Unmeasured or partially covered | Related behavior has focused fixture tests where listed above; no full remote pipeline, representative production rows, paired resource windows, or sign-in/CPU/I/O acceptance artifact. |

Выполненные локальные quality gates: `cargo test --workspace --locked` — 3,242 passed, 0 failed, 64 ignored; all-feature matrix `cargo test -p memory_mcp --features fs-watch,streamable-http,mcp-apps,eval-support,test-fixtures --locked` — 4,089 passed, 0 failed, 61 ignored; strict workspace Clippy и `cargo fmt --all --check` прошли. `xtask check-toolchain-pin` и `xtask check-observability` тоже прошли; для последнего потребовались Python 3.11 и временный PyYAML path. Эти числа относятся к указанным запускам по отдельности и не складываются.

После последних исправлений test-fixture/метрики полный all-feature matrix был запущен повторно и завершился с кодом 0; `http_isolation` прошёл отдельно (1/1 для ранее упавшего сценария) и всем target (3/3). Строгий workspace Clippy, fmt, `check-toolchain-pin` и `check-observability` также повторно прошли. Перед зелёным повторным прогоном один all-feature запуск вернул HTTP 503 в `http_isolation`; дефект не воспроизвёлся в изолированном повторе, полном повторе target или новом полном feature matrix. Это фиксирует наблюдавшуюся чувствительность интеграционных тестов к общей нагрузке, но не доказывает отсутствие редкой нестабильности.

| Gate | Статус | Основание |
|---|---|---|
| G0 | PASS | Workspace/default и feature matrix, Clippy, fmt зелёные. |
| G1 | PARTIAL | RED/GREEN contracts и AMD64 remote allocation/JSON tests пройдены; remote SurrealDB EXPLAIN/row equivalence не собраны для всех изменённых query shapes. |
| G2 | PASS локального session harness | 100 maintenance ticks не увеличили bind/signin churn; same-key singleflight, 32-session cap и 1000-binding churn прошли в Task 4. |
| G3 | PASS указанного frozen corpus/model | Полное парное сравнение case-level retrieval-quality 37/37 без quality regressions. |
| G4 | OPEN | Нет controlled A/A noise calibration и пяти AB/BA пар по 60s warmup + 300s измерения для idle и throughput; нет app+DB CPU/RSS/HWM/swap/physical-I/O windows. |
| G5 | PARTIAL | Dimension/off/model/reembed, retrieval semantics, concurrency/pool/cancellation regressions зелёные; sustained cold/warm full-application stages и error/latency/resource matrix не прогнаны. |
| G6 | PARTIAL | Локальные test/lint/format/xtask gates зелёные; hosted GitHub pipeline не запускался, потому что локальные коммиты не отправлялись. |

G4 остаётся обязательным для численного performance acceptance, merge/release и вывода об уменьшении production CPU. Remote evidence schema/validator не являются измерением, а текущие production ограничения исключают проведение A/B там. Production native canary будет отдельным rollout шагом после разрешения пользователя.

## Повторная локальная проверка после финального review, 2026-10-15

`cargo test --workspace --locked` и полная матрица `cargo test -p memory_mcp --features fs-watch,streamable-http,mcp-apps,eval-support,test-fixtures --locked` завершились с кодом 0. Первый feature-rich запуск обнаружил старый вызов `record_completed` в `http_maintenance_cadence`; после миграции на attempt-start contract target прошёл 6/6, затем вся матрица была запущена повторно и прошла. Структурный поиск Rust-кода не нашёл оставшихся ссылок на `record_completed`.

Строгий workspace Clippy прошёл для штатного CI feature-набора и для расширенного набора с `eval-support,test-fixtures`; `cargo fmt --all --check`, `xtask check-toolchain-pin` и `xtask check-observability` тоже прошли. Для observability-check временно использовался Python 3.11 с PyYAML 6.0.3; репозиторные зависимости и окружение проекта не менялись. `git diff --check` чистый, активная ветка — `master`.

Эти локальные проверки не закрывают G1/G4/G5/G6. Шесть remote WebSocket capture tests остались ignored из-за отсутствия изолированного локального fixture; 16 amd64 allocation tests также skipped в обычной матрице и подтверждаются отдельным ранее выполненным emulated run. Production CPU/RSS/latency/throughput paired windows и hosted GitHub pipeline в этой проверке не запускались. Изменения не выкатывались и на production database не записывали.

### Дополнительная локальная arm64 backfill-матрица

После предыдущего AMD64-emulated fixture был отдельно выполнен нативный macOS arm64 прогон на embedded Mem: 40 сценариев, 240 кандидатов. Для 10,000 строк при размерности 2048, pending=0, first-page medians составили 623.046 ms (indexed wide), 621.693 ms (indexed narrow), 2.619 ms (NOINDEX wide), 2.110 ms (NOINDEX narrow). При pending=1 — 629.883 ms, 628.878 ms, 2.483 ms и 2.270 ms соответственно. Это изолированный first-page query-shape microbenchmark; он не измеряет прямой keyset-запрос через приложение, app+DB CPU/RSS, p95/p99 или throughput. Поэтому production SQL оставляет planner default, `WITH NOINDEX` остаётся кандидатом только для дальнейшего G4 A/B и G4 остаётся OPEN.
