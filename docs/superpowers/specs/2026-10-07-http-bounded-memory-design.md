# HTTP bounded memory — вариант A

**Статус:** проект дизайна для совместного review с планом; не разрешение на реализацию или deployment.
**Дата:** 2026-10-07.
**Исходный запрос:** снизить RAM memory_mcp на VPS 2 CPU / 2 GB; желаемые 10–20 MB idle и 100–200 MB active для одного tenant. Пользователь выбрал вариант A: исправить memory bounds и избыточные аллокации существующего SaaS-профиля. На этой стадии нужны проверки и план, не product code.
**Связанный план:** `../plans/2026-10-07-http-bounded-memory.md`.

## 1. Цель и границы

Сохранить полный `streamable-http` продукт, tenant isolation, dual-era MCP, bi-temporal validity, audit trail и canonical vector policy. Ограничить ресурсы до HTTP buffering, retained caches и detached embedding work; убрать ненужные full scans и повторное создание тяжёлых dependencies.

Не менять allocator, зависимости, migrations, MCP tools, public response schemas, authentication strength или качество NER посредством незаметного truncation/chunking. Не выделять worker process и не вводить remote-only build в вариант A.

Сначала воспроизвести footprint на точной Linux-сборке, затем менять независимо проверяемые участки. Состояние `/proc` не является доказательством allocator fragmentation или live heap ownership.

## 2. Проверенные наблюдения

Локальный HEAD: `98ac11961f34b119610ab2c9935ad382b302fffe`. На момент исследования уже изменены пользователем HTTP `config/types.rs`, `config/validate.rs` и observability assets; сохранить эти изменения. На VPS image ID `sha256:0fdc287b8a8843007535b88197517d3ab0256beaa7fb8895fed3dbb0ebcc7247`; OCI revision labels отсутствуют. Равенство deployed source и HEAD не установлено.

Read-only SSH 2026-10-07, UTC:

| Время | PID | VmRSS KiB | VmSwap KiB | VmHWM KiB |
|---|---:|---:|---:|---:|
| 11:38:12 | 3443295 | 371460 | 346908 | 713732 |
| 11:47:20 | 3443295 | 793588 | 639252 | 802052 |
| 11:47:53 | 3443295 | 751960 | 727256 | 1215228 |

Процесс запущен 11:04:33 UTC; 3 потока. При первом замере cgroup current=389083136 B, swap.current=355860480 B, peak=739004416 B. При втором `[heap]` Size=724504 KiB, RSS=723044 KiB; большие anon segments также содержат swap. Оба HTTP DB targets имеют `ws` scheme. Effective provider: `openai-compatible`, NER: `anno`, dimension: 2048, MALLOC_ARENA_MAX: 2.

Уже выставлены body=1048576 B, requests=8, subscriptions=2, pool=2, TTL=120 s, maintenance parallelism=1. Это не чистый A/B. Трафик между снимками не изолирован; в исследовательской сессии также выполнялся MCP recall, связь которого с этим deployment не установлена. Не обозначать рост как health-only leak. `docker logs` не удалось классифицировать JSON-парсером; отсутствие распознанных событий не является отсутствием нагрузки.

Установленные code facts:
- Pool capacity не создаёт заранее 32 tenant runtimes.
- Remote selected paths не вызывают embedded engine/local model constructors; compiled dependencies всё равно присутствуют.
- HTTP body collection/JSON parsing предшествуют ordinary admission.
- Context cache: 512 entries, deep copies, нет byte bound.
- Query cache: 128 entries, 300 s lazy per-key expiry.
- Triple extractor повторно компилирует 32 regexes при service construction.
- Startup count/sample скачивают fact table, sample=16 применяется после retrieval.
- Task artifact reconciliation materializes все committed artifacts.
- Maintenance создаёт полный MemoryService.
- Prometheus install_recorder не запускает upkeep; raw observations drain при scrape. 300 s × 5 buckets означает 25 минут, а не пять.

## 3. Memory acceptance contract

Для constrained profile: 1 ready tenant, remote SurrealDB в отдельном процессе, anno, remote embedding mock 2048 dimensions, без локальных моделей. Dataset: 129 facts, 24 episodes, 454 entities; факт <=4 KiB content, episode <=64 KiB; отдельный stress dataset указан в harness. Все новые limits должны быть видны в evidence. Реальные пользовательские документы не копировать в репозиторий.

Цели candidate, требующие измерения:
- Active process footprint: VmRSS+VmSwap <=200 MiB на оговорённой mixed workload, с обязательным отдельным RSS peak.
- Warm idle после workload и quiescence: VmRSS+VmSwap <=100 MiB.
- Минимум 4x снижение post-work footprint против fresh-process exact-build baseline на идентичной workload; исходный неконтролируемый VPS snapshot не годится как denominator.
- Если baseline уже ниже target, требование 4x не принуждает искусственно уменьшать функциональность; сравнить targets и абсолютные значения.
- 10–20 MiB idle — stretch, не обещанный acceptance gate варианта A.
- No growing retained-resource counters после последовательных одинаковых циклов; OS footprint slope оценивается несколькими циклами, не 40-секундным плато.
- Latency: p95 <=1.15 baseline на локальном controlled provider для accepted requests; overload refusals считать отдельно. Внешняя NVIDIA latency не годится для causal regression gate.

Это не universal bound для любого document/graph/tenant count. Для огромных persisted rows, lexical rescue и community rebuild остаются отдельные ограничения/дизайн. Gate обязан выявить их, а не объявлять весь процесс bounded.

## 4. HTTP input protection

Добавить независимую process-wide preflight reservation до body collection после дешёвых header checks. Не менять preflight → authenticate → runtime admission порядок. Nonblocking admission, без waiter queue. Запрос со valid `Content-Length` резервирует объявленный размер до чтения; запрос без длины начинает с нулевого byte reservation. До копирования каждого data frame в preflight buffer accounting увеличивается до cumulative observed body length через checked atomic budget; understated `Content-Length` не обходит лимит. `Limited` сохраняет индивидуальный hard body limit и возвращает 413 сверх него. При исчерпании aggregate byte budget middleware возвращает 503 и не копирует отказанный frame. RAII освобождает slot и учтённые bytes на error/cancellation/downstream completion; reservation не удерживается до конца SSE stream. Parsed preflight Value уничтожается до downstream await.

Defaults: 20 preflight slots, 64 MiB accounted raw bytes; constrained: 2 slots, 2 MiB accounted raw bytes, body=1 MiB. Такой byte ledger учитывает preflight body bytes, но не transport-owned chunks, JSON expansion или allocator RSS. Saturated slot/declared-byte gate: 503 `preflight buffering capacity exhausted`; неизвестный/understated body может получить этот 503 после чтения chunk, но до его копирования. Cheap header refusal сохраняет существующие 415/406/413. При overload body-dependent malformed request может получить 503 раньше 400; это намеренный ресурсный контракт.

## 5. Caches

Context: существующие 512 entries, HTTP 4 MiB accounted bytes/runtime, local 16 MiB; constrained 4 MiB. Oversized entry bypass, не ошибка запроса. Учитывать keys, owned nested data, capacities и documented node allowance. Это оценка retained memory, не exact allocator bytes. Не клонировать giant candidate только ради проверки веса. Generation fence препятствует repopulation после invalidation. Не добавлять shared-result Arc redesign в первый slice: byte bound важнее и не требует изменения retrieval result interface.

Query embeddings: 128 entries, TTL=300 s, default 2 MiB; constrained 2 MiB. Purge всех expired entries перед get/insert; expiry не продлевается на hit. На idle expired entries могут сохраняться до следующей операции, но byte bound действует; не обещать таймерное освобождение.

Knobs: `MEMORY_CONTEXT_CACHE_BYTES`, `MEMORY_QUERY_EMBEDDING_CACHE_BYTES`, `MEMORY_MCP_HTTP_PREFLIGHT_REQUEST_LIMIT`, `MEMORY_MCP_HTTP_PREFLIGHT_BYTES`. HTTP default context budget 4 MiB, stdio 16 MiB. Положительные значения; preflight bytes >= body limit; slots <= Semaphore::MAX_PERMITS. Новые serde поля имеют defaults. Env parse только в composition roots.

## 6. Extraction

Whole-input semantics для принятого текста, без silent chunks/truncation. `ANNO_MAX_INPUT_BYTES`: default 1048576 B; constrained 65536 B; allowed 1..=1048576. Изменение поведения для ранее принимавшихся oversized persisted episodes явно описать. UTF-8 bytes, включая whitespace. Для других selectors явный override отклоняется как irrelevant.

Guard original episode перед sanitization/extraction writes; guard actual adapter input перед backend invocation. Validation error: `entity extraction input too large: provider=anno actual_bytes={actual} max_bytes={max}`. Inline source episode мог уже быть ingested: не обещать ingestion rollback; facts/entities/edges/extraction projection не создаются при refusal. Limits не покрывают уже materialized огромную запись из DB.

Shared immutable RuleBasedTripleExtractor на process lifetime; preserve injected test/custom extractors, normalization и pattern order. Не обещать нулевой рост regex scratch caches.

## 7. Storage и maintenance

Knowledge-owned count и scalar dimension sample (LIMIT 16), strict shape decoding, preserve startup decisions, NONE/NULL/empty-vector contract. Nonsemantic projections сохраняют ACL, temporal, provenance и scoring fields. ANN embeddings оставить: есть local similarity fallback.

Task artifacts: projected keyset pages 64 rows, stable id ceiling/pass, preserve audit and existing completion semantics. Cursor advances по последней scanned записи, включая orphan/terminal. Page row cap не является byte cap; giant fact_ids и отсутствие pagination index явно остаются рисками. No migration в этом плане.

HTTP lifecycle получает narrow handles, claim store для atomic retraction и deployment policy, не новый MemoryService. HTTP extraction/backfill создают только требуемые dependencies через owning use cases и shared deployed providers. CLI lifecycle redesign не входит автоматически: отдельный scope, если нужен.

Detached embedding work: один runner на HTTP deployment, не N limits на tenant. Default/constrained limits: admitted=8, running=1, retained accounted input+keys=262144 B, total timeout=60 s с момента admission. Dedicated stdio runner использует те же defaults. Nonblocking refusal, дедуп key содержит namespace+fact identity или provider signature+query identity. RAII cleanup, bounded shutdown ownership, no unbounded waiters; job-handle tracking also bounded by eight admitted jobs with completed-handle reaping, not append-only history. Retain provider prefix <=8000 chars; query key вычисляется по исходному тексту. Для query background admission оригинал >65536 UTF-8 B не ставится в retry; foreground behavior не меняется. Reject outcome bounded enum и guidance/log, без потери durable fact; backfill остаётся recovery route.

## 8. Metrics и lifetime

Periodic Prometheus upkeep every 5 s, owned cancellation/join, независимо от scrape. Summary bucket duration=60 s, count=5. Existing metric names/type/quantile families сохранить; не делать histogram migration в этом плане.

Subscriptions: сначала regression proving удержание old service generations. Если chain подтверждён, stream captures только subscription ports/identity/shutdown/config scalars, не весь MemoryMcp/MemoryService. Активная subscription корректно продолжает работать либо завершает работу по уже существующей policy; pool eviction не должен создавать скрытые retained service generations. Detached work может временно держать bounded narrow references; считать их отдельно.

## 9. Evidence и gates

Linux snapshot sampler через xtask, no product unsafe allocator instrumentation. Staging fresh-process baseline, isolated remote DB/mock provider, scenario markers и workload log. Allocation profiler на отдельном стенде с exact symbols; не inject в production. Проверять allocation stacks, live retained objects и startup/active peaks. Quiescence подтверждать observable owner/drain signals; если нужный сигнал отсутствует, idle gate inconclusive, а не успешен по таймеру. При необходимости allocator classification — отдельное согласование unsafe/platform dependency instrumentation, не встроенный malloc_trim timer.

Если первый профиль выявит один dominant startup owner вне перечисленных slices: остановить исполнение остальных как попытку root-cause fix, приложить evidence и вынести конкретный design patch на review. Независимые robustness slices можно выполнить отдельно с явным статусом, но это не доказательство снижения footprint.

## 10. Совместимость и безопасность

Rust channel 1.99.0. No dependency/migration/new MCP-tool changes без отдельного approval. No request-level namespace. No fact deletion. Business rules в owning context api.rs, transport и composition thin. No production unwrap. Preserve пользовательские worktree changes. Production restart/config/build/deploy требуют нового approval; SSH read-only разрешён только для evidence. Не сохранять credentials, tokens, document contents или публично связанные с пользователем workload samples.

Обязательные проверки после реализации: targeted tests → full production crate tests при cross-context изменениях → cargo fmt --all --check → cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings. Runtime RAM validation только Linux release build.
