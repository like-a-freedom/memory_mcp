# Storage materialization memory remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Устранить подтверждённое раздувание памяти storage adapter и ненужное чтение embeddings, доказав корректность и устойчивый memory profile полного HTTP-приложения до выпуска.

**Architecture:** Consuming conversion остаётся в storage infrastructure и применяется одинаково ко всем операциям `SurrealDbClient`. Knowledge-owned query builders задают минимальные проекции; memory workers и retrieval сохраняют текущую бизнес-семантику. Отдельные integration/process fixtures измеряют реальный adapter и production binary; CI и staging закрывают разные гейты.

**Tech Stack:** Rust 1.99.0; SurrealDB/SDK 3.3.2; serde_json 1.0.151; Tokio; существующие Dockerfile, xtask sampler, production composition, HTTP/MCP fixtures; текущий Cargo.lock.

**Spec:** [2026-10-13-storage-materialization-memory-design.md](../specs/2026-10-13-storage-materialization-memory-design.md). Доказательства: [production/root-cause report](../../performance/MEMORY_ROOT_CAUSE_2026-10-13.md) и `docs/performance/memory-root-cause-2026-10-13/manifest.json`.

## Global Constraints

- Rust `1.99.0`; SurrealDB/SDK `3.3.2`; `serde_json` `1.0.151`; текущий `Cargo.lock`, команды с `--locked`.
- Новых зависимостей, миграций, generated-code изменений и MCP tools нет. `Cargo.toml` не меняется.
- Один Active Namespace, выбранный при binding. Не вводить namespace/table selector в application-facing read.
- HTTP application features: `streamable-http,mcp-apps` плюс default `fs-watch`; без `mimalloc` и `test-fixtures` в измеряемом binary.
- Facts не удаляются; decay retracts через `ClaimStore`, атомарно закрывая fact и derived claims.
- Не менять temporal predicates, ordering, row limits, relevance thresholds, cache defaults, job cadences и ranking algorithms.
- Public `DbClient`/stores/capabilities — test boundary; private helper visibility не расширяется ради тестов.
- Synthetic fixtures: dimensions 2048; isolated cases 300 и 1000 rows; HTTP fixture 306 facts / 300 active / 41 episodes.
- G2 peak delta: `< 67_108_864` bytes для 300; `< 201_326_592` для 1000; `peak_1000 <= 4 * peak_300`.
- G4 native Linux amd64, glibc `2.41-12+deb13u4`, `MALLOC_ARENA_MAX=2`: peak RSS `< 268_435_456`, steady RSS+swap `< 134_217_728`, swap 0, cgroup peak `< 335_544_320`, OOM deltas 0.
- G4: 20 warmup + 100 cache-miss requests; plateau growth `<= 8_388_608` bytes; 3 fresh-process repetitions; concurrent 8 foreground requests (4×2 tenants) + maintenance parallelism 1.
- Production `82.165.149.36:2288` read-only; rollout только по отдельной явной авторизации после гейтов.
- Секреты и реальные memory contents не попадают в fixtures, git, artifacts и логи.

## Review Focus

1. Tagged-to-plain conversion теряет RecordId identity, rare typed values или fractional datetime: Tasks 1–2 проверяют public storage round trips и literal fixtures.
2. Projection возвращает None/Null так, что decay пропускает активные facts или неправильно закрывает claims: Task 3 проверяет отсутствие/explicit null и durable atomic retraction.
3. Уменьшение payload незаметно меняет historical visibility, policy filtering или ranking: Tasks 4–5 проверяют IDs/order на прежнем полном наборе кандидатов.
4. ANN projection лишает alternative adapter fallback vectors или принимает ложный similarity: Task 6 проверяет score-present и score-missing paths отдельно.
5. «Успех» memory gate достигается cache hits, embedded DB, thread exit, ARM/emulation или пропущенными samples: Tasks 1 и 7 проверяют настоящие misses, живой worker, native platform и fail-closed evidence.

---

## Объём, зависимости и критерий завершения

Tasks 1–3 — P0, Tasks 4–6 — проекции P1, Task 7 — обязательная приёмка всей ветки. Выпускать частичный результат Tasks 1 или 3 нельзя: удаление одного clone оставляло 800 MiB, а проекция только decay оставляет опасными остальные callers. Все G0–G6 из spec блокируют merge/release; GRAY/skip/нет стенда означает открытый гейт.

Pagination/SQL pushdown/runtime admission для произвольно больших datasets — самостоятельный следующий проект, с собственным spec и recall tests. Здесь `scan_facts` сохраняет полный набор строк и получает меньший payload; это не глобальный byte bound. Periodic trim/allocator switch и изменение concurrency не входят в scope.

### Карта файлов и интерфейсов

| Файл | Действие | Ответственность |
|---|---|---|
| `crates/memory-mcp/src/storage/helpers.rs` | Modify, Tasks 1–2 | Consuming compatibility extraction; делегирование typed converter |
| `crates/memory-mcp/src/storage/json_conversion.rs` | Create, Task 2 | Surreal values/rows → JSON по владению, без numeric tagged tree |
| `crates/memory-mcp/src/storage.rs` | Modify, Task 2 | Только регистрация `pub(crate) mod json_conversion` |
| `crates/memory-mcp/src/storage/client.rs` | Modify, Task 2 | Подключение conversion к query/select/create/update/server_version |
| `crates/memory-mcp/src/knowledge/queries.rs` | Modify, Tasks 3–6 | Decay/active/scan/ANN projections; прежние predicates |
| `crates/memory-mcp/src/knowledge/graph_store.rs` | Modify, Task 3 | Owner-named `select_decay_candidates` |
| `crates/memory-mcp/src/knowledge/knowledge_store.rs` | Modify, Task 5 | Nonsemantic fact scan через projected query |
| `crates/memory-mcp/src/memory/lifecycle_workers/decay.rs` | Modify, Task 3 | Использовать специализированный knowledge read |
| `crates/memory-mcp/tests/common/storage_memory.rs` | Create, Task 1 | Isolated remote fixture, child allocator counters, bounded teardown |
| `crates/memory-mcp/tests/storage_materialization_memory.rs` | Create, Tasks 1–2 | Memory regressions через public DbClient; один test binary с counting allocator |
| `crates/memory-mcp/tests/storage_json_contract.rs` | Create, Tasks 1–2 | Literal typed-value и public CRUD/query contracts |
| `crates/memory-mcp/tests/fixtures/storage_json/typed-values-v1.json` | Create, Task 1 | Вручную проверенные маленькие legacy leaf expectations |
| `crates/memory-mcp/tests/lifecycle_decay.rs` | Modify, Task 3 | Public decay + DB readback + executed-query observer |
| `crates/memory-mcp/tests/knowledge_fact_projections.rs` | Create, Tasks 3–6 | Настоящие store results, fields/IDs/scores/visibility |
| `crates/memory-mcp/tests/service_integration.rs` | Modify только если нужен новый scenario, Tasks 4–5 | Experience/wake-up use-case regression |
| `crates/memory-mcp/tests/service_acceptance.rs` | Modify только если нужен новый scenario, Task 5 | Lexical/temporal results и policy filtering |
| `crates/memory-mcp/src/memory/retrieval/semantic.rs` | Existing tests; new test cases Task 6 | Защитить existing score-missing fallback; алгоритм не переписывать |
| `crates/memory-mcp/tests/common/http_storage_memory.rs` | Create, Task 7 | Owned DB/provider/app containers, durable seed, sampling, artifacts |
| `crates/memory-mcp/tests/http_storage_memory.rs` | Create, Task 7 | Полный production-composition memory gate |
| `.github/workflows/ci.yml` | Modify, Task 7 | Required resource job, cargo entry points, failure artifacts |
| `docs/performance/MEMORY_PROFILE.md` | Modify, Task 7 | Реальные результаты, commands, ограничения scale |

Test utility интерфейсы фиксируются здесь, чтобы задачи не изобретали несовместимые helpers:

```rust
// tests/common/storage_memory.rs; подключать по #[path = "common/storage_memory.rs"].
pub type TestResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct AllocationReport {
    pub rows: usize,
    pub dimension: usize,
    pub peak_delta_bytes: u64,
    pub after_drop_delta_bytes: u64,
}
pub async fn run_remote_allocation_case(rows: usize, dimension: usize)
    -> TestResult<AllocationReport>;
// Родитель создаёт remote fixture, child выполняет public DbClient::query;
// сериализуемый report возвращается после bounded join, worker ещё жив при sample.
```

`CountingSystem` реализует `GlobalAlloc` только в `storage_materialization_memory` test binary. Поля report — итог, не источник expected values. Docker endpoint/credentials генерируются fixture; child получает их только через env. Не переиспользовать production env. Runtime/query observer для decay доступен через existing `eval-support` feature и `SurrealDbClient::set_eval_query_observer`.

Для projection fixtures использовать `SurrealDbClient::connect_in_memory_with_dimension(database: &str, active_namespace: &str, log_level: &str, fact_embedding_dimension: usize)` с dimension 2048 и feature `eval-support`; seed through public `DbClient`. Для existing consumer scenarios использовать существующий `common::make_service_with_client()` без изменения его default dimension. Новые projection tests не должны молча исчезать из test inventory при запуске без eval-support: fixture setup должен сообщать требуемый feature, либо все команды target явно содержат его.

### Обязательная дисциплина исполнения

- До кода прочитать spec, AGENTS.md, TDD и `writing-good-tests.md`. Experimental `/private/tmp/.../src/main.rs` не читать, не копировать, не адаптировать как implementation reference. Использовать доказательства и pinned SDK source.
- В checkout уже есть чужие незакоммиченные изменения в 25 файлах. Применить `using-git-worktrees` при исполнении; согласованный execution baseline и source SHA записать в evidence. Не reset/stash/commit чужие изменения и не использовать `git add .`.
- До первого production edit сохранить baseline relevant tests и RED. Для каждого поведения: test → правильный assertion FAIL → минимальная правка → PASS → допустимый refactor → повторный PASS. Build error/Docker error не являются RED доказательством.
- Уже существующие или baseline-passing compatibility checks — protection/characterization, а не доказательство RED. Правку adapter разрешают именно failing memory/null regressions; новый контракт не считается проверенным одной зелёной characterization.
- На каждый цикл сохранять test name, failing assertion, command, exit code и SHA. RED logs и resource JSON — artifacts, не пометка «проверено» вручную.
- Любой failure, включая обнаруженный до изменения, перечислить по имени. До исправления или явно пересмотренного scope соответствующий гейт открыт. Не ослаблять assertions/thresholds, не удалять проблемный test, не маскировать skip.
- В этой среде команды prefix `rtk`; в CI `run:` остаётся cargo-only по AGENTS.md. Все git add ниже выполняются в isolated execution worktree и только для реально изменённых перечисленных файлов.

## Task 1: Убрать глубокий clone и закрепить измеряемую регрессию

**Files:** helpers.rs; новые common/storage_memory.rs, storage_materialization_memory.rs, storage_json_contract.rs и typed-values-v1.json из карты выше.

**Interfaces:** Existing public `DbClient::query(&self, sql: &str, vars: Option<Value>, namespace: &str) -> Result<Value, MemoryError>`; `SurrealDbClient::connect_bound(&SurrealTargetConfig, &str) -> Result<Self, MemoryError>`; produces `run_remote_allocation_case`/`AllocationReport` из карты. Existing `extract_records(Value) -> Vec<Value>` остаётся внутренним.

- [ ] **1.1 Write test first:** named `remote_300_rows_does_not_allocate_a_second_tagged_tree`. Fixture полностью по spec; отдельный child, real remote DB, no mocks. Создать test-only counter/seed/teardown вместе с этим сценарием, не изменяя application source.

```rust
let r = run_remote_allocation_case(300, 2048).await?;
assert_eq!(r.rows, 300);
assert_eq!(r.dimension, 2048);
assert!(r.peak_delta_bytes < 1_073_741_824, "{r:?}");
```

- [ ] **1.2 Verify RED на native Linux amd64:** `rtk cargo test -p memory_mcp --test storage_materialization_memory --locked remote_300_rows_does_not_allocate_a_second_tagged_tree -- --ignored --exact --test-threads=1 --nocapture`. Expected FAIL на peak `< 1_073_741_824`; child имеет 3 GiB для legacy baseline и 120 s deadline. Если original adapter не воспроизводит amplification, остановиться: проверить fixture/measurement window, не писать fix.
- [ ] **1.3 Capture public compatibility baseline:** создать маленькие typed queries и вручную закрепить rare leaf JSON в fixture. Cases: UUID `00000000-0000-4000-8000-000000000001`, bytes `[0,127,255]`, duration `1h30m`, point `(1.25,-2.5)`, table `entity`, regex `a.*b`, range `1..3`, set со строками `alpha`/`beta`, file literal `f"fixture:/sample.txt"`. Record keys `trip-a`, `key with space`, `ключ`, `a:b`, numeric 7 и composite `[1,"a"]`. Проверить baseline output глазами и сохранить literals; нельзя expected вычислять `normalize_surreal_json`. Это baseline protection, не RED claim.
- [ ] **1.4 Minimal GREEN:** в `helpers.rs::extract_records(value: Value) -> Vec<Value>` извлечь tagged `Array` через ownership match и переместить Vec. Не менять decoder/queries/normalizer semantics. Никакого clone массива или переименования private функций сверх необходимого.
- [ ] **1.5 Verify GREEN:** повторить 1.2, затем `rtk cargo test -p memory_mcp --test storage_json_contract --test record_id_integrity --test typed_record_accessors --locked`. Expected peak below 1 GiB и все contract cases PASS. Это промежуточный результат, **не финальный memory acceptance**.
- [ ] **1.6 Refactor only after GREEN:** устранить локальную дубликацию extraction, если она появилась; повторить 1.5. Проверить counter realloc failure/zeroed allocation accounting без allocations внутри allocator callback; unsafe обоснован safety comments, panic не используется в allocator callback.
- [ ] **1.7 Commit:** `rtk git add crates/memory-mcp/src/storage/helpers.rs crates/memory-mcp/tests/common/storage_memory.rs crates/memory-mcp/tests/storage_materialization_memory.rs crates/memory-mcp/tests/storage_json_contract.rs crates/memory-mcp/tests/fixtures/storage_json/typed-values-v1.json`; `rtk git commit -m "perf(storage): move owned result arrays without cloning"`.

## Task 2: Consuming typed conversion без numeric tagged tree

**Files:** json_conversion.rs (new), storage.rs, helpers.rs, client.rs; оба новых storage test targets.

**Interfaces:** Produces `pub(crate) fn surreal_to_json(value: surrealdb::types::Value) -> serde_json::Value` и `pub(crate) fn surreal_records_to_json(value: surrealdb::types::Value) -> Vec<serde_json::Value>` в json_conversion.rs. helpers.rs сохраняет existing `pub fn surreal_to_json(value: SurrealValue) -> Value` delegator. DbClient signatures не меняются. Consumes Task 1 report/fixtures.

- [ ] **2.1 Write failing resource test:** named `remote_300_rows_materializes_within_64_mib`.

```rust
let r = run_remote_allocation_case(300, 2048).await?;
assert!(r.peak_delta_bytes < 67_108_864, "{r:?}");
```

- [ ] **2.2 Verify RED:** `rtk cargo test -p memory_mcp --test storage_materialization_memory --locked remote_300_rows_materializes_within_64_mib -- --ignored --exact --test-threads=1 --nocapture`. Expected assertion FAIL: после Task 1 tagged serialization всё ещё значительно выше 64 MiB.
- [ ] **2.3 Write independently named scalar regressions** через `DbClient::query`, затем запустить их до production edit:

```rust
// query_none_has_no_rows: SQL RETURN NONE
assert_eq!(actual, serde_json::json!([]));
// query_explicit_null_has_one_null_row: SQL RETURN NULL
assert_eq!(actual, serde_json::json!([null]));
// nested_nullable_fields_are_json_null: RETURN {a: NONE, b: NULL}
assert_eq!(actual, serde_json::json!([{"a":null,"b":null}]));
// bool_scalar_is_json_bool: RETURN true
assert_eq!(actual, serde_json::json!([true]));
```

Expected current legacy encoding failures подтвердить фактическим выводом. Если какой-то случай уже PASS, считать characterization; RED memory из 2.2 остаётся обязательным. Не менять expectations под legacy tags.
- [ ] **2.4 Minimal GREEN:** consuming Array/Object traversal, plain Number/String/Datetime/Bool/null handling; structured RecordId и legacy rare leaves согласно spec/fixture. Использовать pinned SDK `into_json_value` только там, где representation совпадает. Не сериализовать enum целиком для numeric Array/Object; не обходить повторно готовые rows normalizer-ом. Подключить row converter к `select_one`, `select_table`, `create`, `update`, `query`; value converter к `server_version`. Новое module registration — только `pub(crate)`.
- [ ] **2.4a Write a separate RED for bound rows:** existing `BoundDbClient::query_rows` тоже делает `value.as_array().cloned()` — это второе, меньшее копирование уже plain rows. В resource target добавить `owner_scoped_fact_scan_does_not_copy_plain_rows` через настоящий `KnowledgeStoreClient::scan_facts`. Fixture: 300 rows, content длиной 65_536 bytes каждый, без embedding; так будущая projection не скроет регрессию. Test utility `run_remote_bound_scan_case(rows: usize, content_bytes: usize) -> TestResult<AllocationReport>` в common/storage_memory.rs использует delegating `DbClient`: настоящий remote adapter выполняет query; wrapper начинает отдельное counter window непосредственно перед передачей готового JSON owning store. Counter live accounting не сбрасывается. Assertion: `assert!(r.peak_delta_bytes < 8_388_608);`. Deep copy 19,7 MB content нарушает allowance. Это measurement только handoff stage; основной G2 по-прежнему охватывает SDK decode и всю query, исключать из него decode запрещено. Wrapper не подменяет данные/SQL/errors и не имеет собственного fake response.
- [ ] **2.4b Verify RED then minimal GREEN:** `rtk cargo test -p memory_mcp --test storage_materialization_memory --locked owner_scoped_fact_scan_does_not_copy_plain_rows -- --ignored --exact --test-threads=1 --nocapture`; expected allocation-delta assertion FAIL. Изменить только success branch `BoundDbClient::query_rows(&self, sql: &str, vars: Option<Value>) -> Result<Vec<Value>, MemoryError>` в client.rs на consuming match `Value::Array`; прежние non-array→empty/missing-table/error outcomes сохраняются. Повторить команду до PASS и existing `storage::client::tests`. Не менять owner scan query до Task 5, иначе projection скроет именно этот RED.
- [ ] **2.5 Verify GREEN:** 2.2 и `rtk cargo test -p memory_mcp --test storage_json_contract --test record_id_integrity --test typed_record_accessors --locked`. Separate named tests покрывают каждый public CRUD/query method, exact float samples `0.0`, `1.0/17.0`, `16.0/17.0`, decimal literal `12345678901234567890.123456789`, timestamp `2026-01-01T00:00:00.123456Z`, nested record IDs и server version. Rare fixtures сравнивать как literal JSON. Для typed values, которые нельзя выразить JSON bind, fixture seed использует raw SDK `SurrealValue` bind в собственную schemaless `conversion_fixture` table; focal read идёт через public `SurrealDbClient::select_one`. Nonfinite cases `f64::NAN`, `f64::INFINITY`, `f64::NEG_INFINITY` — independently named cases с expected JSON null, без обращения к private converter. Если upstream отвергает seed, это setup error, не PASS теста conversion: найти supported SDK-return route или оставить этот contract gate OPEN с точной upstream ошибкой.
- [ ] **2.6 Add scale test before any scale-specific fix:** named `remote_1000_rows_materializes_within_192_mib`. Отдельные fresh child measurements 300/1000:

```rust
assert!(large.peak_delta_bytes < 201_326_592, "{large:?}");
assert!(large.peak_delta_bytes <= 4 * small.peak_delta_bytes);
```

Run `rtk cargo test -p memory_mcp --test storage_materialization_memory --locked remote_1000_rows_materializes_within_192_mib -- --ignored --exact --test-threads=1 --nocapture`. Если FAIL — новый RED цикл; расследовать allocation stages до правки, не добавлять allocator mitigation. Если сразу PASS — scale protection, а не новый code change.
- [ ] **2.7 Protect errors:** `query_propagates_error_in_second_statement`, `namespace_mismatch_does_not_execute_query`, `missing_record_is_none` отдельно. Существующий `storage::client::tests::query_rejects_error_in_later_statement` должен остаться PASS. Run `rtk cargo test -p memory_mcp --lib storage::client::tests --locked` и `rtk cargo test -p memory_mcp --test storage_json_contract --locked`. Storage failure не равен success с пустыми rows.
- [ ] **2.8 Refactor и verify:** повторить весь storage contract target и G2; `rtk cargo fmt --all --check`. Counter и profiler остаются test-only. `rtk git add crates/memory-mcp/src/storage.rs crates/memory-mcp/src/storage/json_conversion.rs crates/memory-mcp/src/storage/helpers.rs crates/memory-mcp/src/storage/client.rs crates/memory-mcp/tests/common/storage_memory.rs crates/memory-mcp/tests/storage_materialization_memory.rs crates/memory-mcp/tests/storage_json_contract.rs`; `rtk git commit -m "perf(storage): consume typed values without tagged amplification"`.

## Task 3: Минимальный knowledge read для decay

**Files:** queries.rs, graph_store.rs, decay.rs; tests/lifecycle_decay.rs и knowledge_fact_projections.rs.

**Interfaces:** Produces `pub fn build_select_decay_facts_query(cutoff: &str, limit: i32) -> (String, Value)` в knowledge/queries.rs и `pub async fn select_decay_candidates(&self, limit: i32) -> Result<Vec<Value>, MemoryError>` в `KnowledgeGraphStore`. Consumes existing visibility builder и BoundDbClient::query_rows. `run_decay_pass(&LifecycleHandles<'_>, f64, f64) -> Result<usize, MemoryError>` signature не меняется.

- [ ] **3.1 Write regression через существующий public `platform::lifecycle_runtime::decay_pass`:** named `decay_reads_only_lifecycle_fields`. Настоящая DB + QueryObserver записывает выполненный SELECT; test проверяет read contract на boundary, не исходный текст функции. Observer fixture local to integration target, implements existing QueryObserver; gate feature `eval-support`.

```rust
assert_eq!(selected_columns, ["fact_id", "t_valid", "t_invalid",
    "confidence", "access_count", "last_accessed"]);
assert_eq!(invalidated, 1); // old fact 0.4, age 400 days, threshold .3, half-life 100
assert!(stored_fact["t_invalid"].as_str().is_some());
assert!(stored_fact["t_invalid_ingested"].as_str().is_some());
```

`selected_columns` извлекается из фактически выполненного query observer event; literals не берутся из нового projection constant. Дополнительный store-result test проверит field absence, чтобы одного SQL-текста не хватало.
- [ ] **3.2 Verify RED:** `rtk cargo test -p memory_mcp --test lifecycle_decay --features eval-support --locked decay_reads_only_lifecycle_fields -- --exact --test-threads=1`. Expected FAIL: старый executed SELECT возвращает `*`/избыточные поля.
- [ ] **3.3 Minimal GREEN:** добавить builder с шестью полями, прежним full visibility clause/ordering/limit; метод graph store; заменить один call в run_decay_pass на `select_decay_candidates(DECAY_BATCH_LIMIT)`. Лимит 1000 и existing ClaimStore request/reason `confidence_decay` прежние.
- [ ] **3.4 Verify real result:** named `decay_projection_omits_content_and_embedding`; вызвать новый public store method для seeded fact с embedding/content:

```rust
assert!(row.get("embedding").is_none());
assert!(row.get("content").is_none());
assert_eq!(row["fact_id"], "fact:decay-fixture");
assert_eq!(row["confidence"], 0.4);
```

Run `rtk cargo test -p memory_mcp --test knowledge_fact_projections --features eval-support --locked decay_projection -- --test-threads=1` и 3.2; expected PASS.
- [ ] **3.5 Protect lifecycle outcomes отдельными tests:** `decay_projection_keeps_absent_invalid_date_active`, `decay_projection_keeps_explicit_null_invalid_date_active`, `decay_projection_skips_already_closed_fact`, `decay_projection_keeps_recently_accessed_fact`, `decay_projection_closes_derived_claims_atomically`, `decay_projection_is_namespace_bound`. Проверять counts, actual durable close/reason и claims readback, не mock calls. Для каждого необходимого исправления наблюдать новый RED до кода. Existing relevant tests переиспользовать вместо дублирования.
- [ ] **3.6 Verify suite и commit:** `rtk cargo test -p memory_mcp --test lifecycle_decay --test lifecycle_archival --test knowledge_fact_projections --features eval-support --locked -- --test-threads=1`; `rtk cargo test -p memory_mcp --lib --features streamable-http,test-fixtures --locked http::lifecycle::tests`. Explicit git add четырёх source/test paths + нового target; `rtk git commit -m "perf(knowledge): project only decay candidate fields"`.

## Task 4: Nonsemantic projection для общих active reads

**Files:** knowledge/queries.rs; knowledge_fact_projections.rs; existing experience/wake-up/dashboard protection tests.

**Interfaces:** Existing `build_select_active_facts_query(&str, i32) -> (String, Value)`; `KnowledgeGraphStore::select_active_facts(i32)` и `KnowledgeStoreClient::select_active_facts(i32)` возвращают `Result<Vec<Value>, MemoryError>`. Использовать existing `NONSEMANTIC_FACT_PROJECTION`, не шесть decay fields.

- [ ] **4.1 Write failing store-result test:** `active_fact_reads_omit_embeddings`, отдельно для обоих stores через independently named cases.

```rust
assert!(row.get("embedding").is_none());
assert_eq!(row["content"], "synthetic active fact");
assert_eq!(row["source_episode"], "episode:active-fixture");
assert_eq!(row["policy_tags"], serde_json::json!(["persona"]));
```

- [ ] **4.2 Verify RED:** `rtk cargo test -p memory_mcp --test knowledge_fact_projections --features eval-support --locked active_fact_reads -- --test-threads=1`. Expected FAIL: existing SELECT * возвращает embedding.
- [ ] **4.3 Minimal GREEN:** заменить только projection в builder, оставить visibility и `ORDER BY t_valid ASC LIMIT $limit`. Caller limits 500/10000 не менять. Структура existing stores не меняется.
- [ ] **4.4 Verify GREEN и consumer contracts:** команда 4.2; `rtk cargo test -p memory_mcp --test service_integration --locked test_service_assemble_context_wake_up_prioritizes_persona_then_recent`; `rtk cargo test -p memory_mcp --test lifecycle_cli_subprocess --features mcp-apps --locked lifecycle_dashboard_returns_the_public_operation_envelope`. Добавить только отсутствующие named cases `experience_read_keeps_source_and_confidence`, `active_projection_keeps_historical_visibility`, `active_projection_keeps_requested_limit` с literals IDs/counts; новый behavior fix — только после RED.
- [ ] **4.5 Commit:** explicit add queries.rs, knowledge_fact_projections.rs и реально расширенных existing targets; `rtk git commit -m "perf(knowledge): omit vectors from active fact reads"`.

## Task 5: Полные nonsemantic scans без vectors и без потери recall

**Files:** knowledge/queries.rs, knowledge/knowledge_store.rs, knowledge_fact_projections.rs; точечно existing service_acceptance/service_integration tests.

**Interfaces:** Produces `pub fn build_scan_nonsemantic_facts_query() -> String` в knowledge/queries.rs. Existing `KnowledgeStoreClient::scan_facts(&self) -> Result<Vec<Value>, MemoryError>` сохраняет signature и весь прежний row set; query через `BoundDbClient::query_rows` с `None` vars. Projection `id, {NONSEMANTIC_FACT_PROJECTION}`.

- [ ] **5.1 Write failing integration test:** `nonsemantic_scan_omits_vectors_without_truncating_history`. Seed три independently identified facts: active, invalidated after historical cutoff, newest; у каждого vector.

```rust
assert_eq!(returned_ids, ["fact:active", "fact:closed", "fact:newest"]); // sorted only for set check
assert!(active_row.get("embedding").is_none());
assert_eq!(closed_row["t_invalid"], "2026-02-01T00:00:00Z");
assert!(closed_row.get("id").is_some());
```

- [ ] **5.2 Verify RED:** `rtk cargo test -p memory_mcp --test knowledge_fact_projections --features eval-support --locked nonsemantic_scan_omits_vectors_without_truncating_history -- --exact --test-threads=1`. Expected FAIL по embedding presence, не по dataset preparation.
- [ ] **5.3 Minimal GREEN:** projected SQL полного fact scan в owning knowledge context. Не менять table-generic `DbClient::select_table`: другие owner reads требуют их полные записи. Не добавлять WHERE/LIMIT/paging или менять lexical/temporal/wake-up functions.
- [ ] **5.4 Verify GREEN:** 5.2 плюс `rtk cargo test -p memory_mcp --test knowledge_read_scopes --test service_acceptance --test service_integration --test assemble_context_supersession_ranking --locked`. Дать exact IDs/order assertion в отдельных отсутствующих tests: `lexical_fallback_keeps_best_match_after_the_first_1000_rows` (match за 1000 distractors), `temporal_scan_keeps_relative_event_date_match`, `wake_up_keeps_persona_before_recent_fact`, `historical_scan_preserves_closed_predecessor`, `projected_scan_preserves_policy_restrictions`. Existing complete scenarios не дублировать. Пример independently pinned outcome: `assert_eq!(selected_ids, ["fact:late-best-match"]);`, `assert!(!selected_ids.contains(&"fact:protected"));`.
- [ ] **5.5 Review:** подтвердить по executed-query/results, что rows не усекаются и no semantic embedding consumer зовёт изменённый full scan. Вернуть ошибку storage по прежнему contract; missing table degradation проверить existing knowledge scope tests.
- [ ] **5.6 Commit:** explicit add queries.rs, knowledge_store.rs, projection target и реально добавленных use-case tests; `rtk git commit -m "perf(knowledge): project metadata for nonsemantic scans"`.

## Task 6: ANN response projection с сохранением semantic fallback

**Files:** knowledge/queries.rs; knowledge_fact_projections.rs; tests в memory/retrieval/semantic.rs только если нужного случая ещё нет.

**Interfaces:** Existing `build_select_facts_ann_query(cutoff: &str, query_vec: &[f64], limit: i32) -> (String, Value)` и `KnowledgeStoreClient::select_facts_ann(&str, &[f64], i32) -> Result<Vec<Value>, MemoryError>`. Возвращает `id + NONSEMANTIC_FACT_PROJECTION + sem_score`; embedding участвует в DB expression/predicate, отсутствует в row payload.

- [ ] **6.1 Write failing result test:** `ann_returns_score_without_vector_payload`. Real embedded Surreal dimension 2048, vectors `e0` и `e1`, query `e0`, limit 1.

```rust
assert_eq!(rows.len(), 1);
assert_eq!(rows[0]["fact_id"], "fact:semantic-e0");
assert_eq!(rows[0]["sem_score"].as_f64(), Some(1.0));
assert!(rows[0].get("embedding").is_none());
```

- [ ] **6.2 Verify RED:** `rtk cargo test -p memory_mcp --test knowledge_fact_projections --features eval-support --locked ann_returns_score_without_vector_payload -- --exact --test-threads=1`. Expected FAIL по присутствующему embedding.
- [ ] **6.3 Minimal GREEN:** изменить SELECT-list; сохранить cosine expression, K/ef_search, visibility, ordering и binding limits. Не удалять `embedding_from_value` fallback в semantic collector.
- [ ] **6.4 Verify GREEN:** 6.2 и `rtk cargo test -p memory_mcp --lib --locked memory::retrieval::semantic::tests`. Защитить independently named cases: `semantic_score_without_embedding_keeps_candidate` → selected ID `fact:semantic-e0`; `semantic_missing_score_uses_embedding_fallback` → тот же ID через controlled existing DbClient port; `semantic_missing_score_and_embedding_has_no_positive_match` → empty candidates; invalid similarity → existing validation error. Fixtures — literals 1.0/0.0 и e0/e1; expected не вычислять production cosine helper.
- [ ] **6.5 Protect end-to-end ranking:** `rtk cargo test -p memory_mcp --test service_acceptance --test service_integration --test assemble_context_semantic_quality --locked`. Набор кандидатов, source identity и optional reranking input прежние. Любой новый repair начинает свой RED цикл.
- [ ] **6.6 Commit:** explicit add queries.rs, projection target и semantic test cases; `rtk git commit -m "perf(knowledge): omit ANN vectors from scored responses"`.

## Task 7: Полный HTTP memory gate, CI и evidence для выпуска

**Files:** tests/common/http_storage_memory.rs, tests/http_storage_memory.rs, .github/workflows/ci.yml, docs/performance/MEMORY_PROFILE.md. Existing `crates/xtask/src/memory.rs` sampler использовать без изменения, если нет воспроизведённого дефекта.

**Interfaces:** В новом test utility определить:

```rust
pub enum MemoryScenario { ColdDecay, RetrievalMisses, ConcurrentMaintenance, ErrorRelease }
pub struct HttpMemoryReport {
    pub peak_rss_bytes: u64,
    pub steady_rss_plus_swap_bytes: u64,
    pub max_swap_bytes: u64,
    pub cgroup_peak_bytes: u64,
    pub oom_delta: u64,
    pub oom_kill_delta: u64,
    pub plateau_growth_bytes: i64,
    pub failed_requests: usize,
    pub db_read_delta: u64,
    pub samples: usize,
}
pub async fn run_http_memory_case(image: &str, scenario: MemoryScenario,
    output_dir: &std::path::Path) -> TestResult<HttpMemoryReport>;
pub fn validate_http_memory_evidence(output_dir: &std::path::Path)
    -> TestResult<HttpMemoryReport>;
```

Consumes Task 1 `TestResult`, existing `HttpProductionComposition::connect(&HttpConfig)`, durable owner stores и `common::http_server::mcp_call`. Test fixture seed создаёт ready tenants/account/API key bindings и source facts до app launch; никаких production-only debug hooks. Image argument — captured immutable local image ID или baseline digest, не mutable `latest`. Наличие app features `test-fixtures` запрещено гейтом. `db_read_delta` считать по existing `db.execute_query.done` DEBUG events, только foreground SELECT с request_id соответствующих cache-miss запросов; background queries и startup seed не включаются. Test env включает DEBUG для этих diagnostic events; SQL/content только synthetic, credentials/binds не печатаются. Сигнатуры новых production methods не добавляются.

- [ ] **7.1 Write full-app acceptance tests before harness implementation:** четыре independently named сценария соответствуют enum; по три fresh runs. Один process = один workload. Assertions для каждого report:

```rust
assert!(r.peak_rss_bytes < 268_435_456);
assert!(r.steady_rss_plus_swap_bytes < 134_217_728);
assert_eq!(r.max_swap_bytes, 0);
assert!(r.cgroup_peak_bytes < 335_544_320);
assert_eq!(r.oom_delta, 0);
assert_eq!(r.oom_kill_delta, 0);
assert!(r.plateau_growth_bytes <= 8_388_608);
assert_eq!(r.failed_requests, 0);
assert!(r.samples > 0);
// RetrievalMisses scenario: assert!(r.db_read_delta >= 100);
```

`samples>0` недостаточно для полноты: fixture обязан отвергнуть missing cold-start coverage, early sampler death, PID replacement, отсутствие steady window/cgroup counters/platform manifest. Проверка полноты — до возврата report; returned report никогда не может скрыть invalid run.
- [ ] **7.2 Establish full-app RED control:** harness запускает pinned production image из spec на собственном synthetic remote DB, с 3 GiB для capture контролируемой assertion failure. Expected violation memory limits на startup/scan path. Сохранить legacy result отдельно; для legacy допускается expected FAIL, он не является candidate acceptance. Если old image недоступен или не воспроизведён — G4 evidence открыт; не подменять его ARM microprobe.
- [ ] **7.3 Implement minimal harness:** managed containers/process guards с kill+wait+remove; external SurrealDB 3.3.2 и separate deterministic OpenAI-compatible provider dimension 2048; candidate из existing Dockerfile. Seed durable registry/tenant stores, app RAM 1 GiB/RAM+swap 2 GiB, untouched System allocator. Native amd64 check host+image; glibc внутри app ровно 2.41-12+deb13u4. Provider failure используется только ErrorRelease scenario. Teardown bounded 10 s, startup deadline 60 s, ColdDecay deadline 4000 s (реальный repeat при cadence 3600 s), остальные scenarios 600 s; каждый CI job максимум 120 min. External launch barrier держит container wrapper до подключения sampler, затем exec app с тем же PID; app source/debug hooks не меняются.
- [ ] **7.4 Exercise workloads:** cold process измеряется до scheduler start, потом дождаться первого и повторного decay по completion signals; separate correctness fixture должен закрыть один old fact и его claims. `RetrievalMisses` — lexical fallback/temporal/wake-up/experience/ANN/assemble_context, 20 warmup +100 distinct cache-miss queries, then explicit warm-cache control. `ConcurrentMaintenance` — два tenants, четыре requests каждый, semaphore/barrier подтверждает overlap с background work. `ErrorRelease` — controlled query/provider failure/cancellation, затем successful request и return to plateau. Не устанавливать порядок sleeps-ами; sampler intervals/idle observation допустимы по spec.
- [ ] **7.5 Verify candidate GREEN:** `rtk cargo test -p memory_mcp --test http_storage_memory --features streamable-http,mcp-apps,eval-support,test-fixtures --locked -- --ignored --test-threads=1 --nocapture`. Harness автоматически строит candidate image из execution worktree через existing Dockerfile и фиксирует image ID/build provenance; baseline reference задаётся spec. Три fresh runs каждого сценария должны пройти все thresholds. Test driver может иметь eval-support/test-fixtures для existing config constructors и seed utilities; измеряемый app image — только production features, без fixture startup hooks. G2 также запускать отдельной полной командой из Task 1 target; на macOS этот gate не закрывается.
- [ ] **7.6 Fail-closed validation tests:** controlled resource fixture cases `missing_samples_fail_the_run`, `arm_image_is_not_accepted_as_amd64_evidence`, `cache_hits_do_not_close_miss_gate`, `sampler_exit_before_decay_is_failure`, `application_error_is_not_a_successful_request`, `cgroup_without_peak_or_events_is_incomplete`. Вызывают test utility public `validate_http_memory_evidence(&Path)` на самостоятельных literal synthetic artifact inputs; `run_http_memory_case` обязан использовать тот же validator до возврата report. Для каждого case `assert!(validate_http_memory_evidence(dir.path()).is_err());`, а error category проверяется отдельно по конкретной нарушенной причине. Это валидация настоящих JSON outputs harness, не tests for markdown/file presence. Сначала наблюдать FAIL, потом минимальный guard; run `rtk cargo test -p memory_mcp --test http_storage_memory --features streamable-http,mcp-apps,eval-support,test-fixtures --locked` (guard tests не ignored).
- [ ] **7.7 Integrate CI after local GREEN:** required native `x86_64` resource job в existing ci.yml, matrix `repetition: [1,2,3]`; named test suffixes `repetition_1`/`repetition_2`/`repetition_3`, cargo test filter запускает четыре scenarios одной repetition. Так настоящее часовое ожидание repeat decay не нарушает timeout 120 min на job. G2 — отдельный cargo test step; Docker fixture lifecycle выполняет сам integration harness, CI run steps cargo-only. Без continue-on-error. Artifacts upload always: raw JSON, sampler JSONL, sanitized app logs, baseline/candidate manifests, RED/GREEN log, fixture digest. CI используют controlled provider; полный production-profile staging A/B обязателен отдельно. Не менять branch protection через API: на review явно перечислить новый required check, пока его обязательность не закреплена владельцем repository — manual merge gate открыт.
- [ ] **7.8 Run complete quality gates** из следующего раздела. Doc MEMORY_PROFILE обновить только фактическими measurements; failed/missing cases и platform limitations назвать явно. Финальное review всех изменённых source/test/query boundaries; открытые P0/P1 блокируют передачу на rollout.
- [ ] **7.9 Commit:** explicit add two new test files, ci.yml и MEMORY_PROFILE; `rtk git commit -m "test(memory): gate storage amplification and HTTP memory plateau"`. Review whole branch с учетом Tasks 1–6; не выполнять production deployment.

## Финальные гейты: точные команды и PASS evidence

Запускать после всех production edits; после последнего изменения rerun affected gates. Не начинать с полной suite на каждом RED цикле.

```bash
rtk cargo fmt --all --check
rtk cargo test --workspace --lib --bins --tests --locked
rtk cargo test -p memory_mcp --lib --bins --tests --features fs-watch,mcp-apps,streamable-http,test-fixtures,eval-support --locked
rtk cargo test -p memory_mcp --no-default-features --test fs_watch_process_disabled --locked
rtk cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
rtk cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http,test-fixtures,eval-support --locked -- -D warnings
rtk cargo run -p xtask --locked -- check-toolchain-pin
rtk cargo run -p xtask --locked -- check-observability
rtk cargo test -p memory_mcp --test storage_materialization_memory --locked -- --ignored --test-threads=1 --nocapture
rtk cargo test -p memory_mcp --test http_storage_memory --features streamable-http,mcp-apps,eval-support,test-fixtures --locked -- --ignored --test-threads=1 --nocapture
rtk git diff --check
```

PASS: каждый command exit 0; expected tests реально executed, named resource cases не только ignored summary; ноль fmt diff и clippy warnings; все G0–G6 evidence целостны. Existing ignored tests из общей suite перечислить с причиной; обязательные resource cases запускаются `--ignored` отдельно. Если какой-либо mandatory test/tool/runner недоступен, report помечает gate OPEN и ветка не «готова к shipping».

### Evidence package на каждый A/B

Сохранить под отдельным `work/memory-materialization/<run-id>/` (не credentials), загрузить CI artifacts; итоговые малые таблицы и immutable artifact links — MEMORY_PROFILE. Manifest содержит Git SHA+dirty state/build input identity, Cargo.lock SHA-256, rustc/version/features, app image ID/digest/binary hash, SurrealDB image ID/version, libc/kernel/arch, limits и effective config, fixture version+digest, provider identity, exact commands/exit codes, UTC start/end, scenarios/repetitions. Mutable tags разрешены только для первоначального pull/build, затем все runs используют captured IDs. Не сравнивать разные fixtures/platforms/allocators как causal A/B.

Перед rollout review предъявить: baseline FAIL и candidate PASS; peak live allocations и настоящие RSS/swap/cgroup; complete-request correctness и DB-query evidence; durable count/close/namespace checks; full-suite and CI result IDs; final review findings. Rollout после отдельной авторизации фиксирует target digest, observation windows и rollback criteria из spec; current read-only authorization этого не разрешает.

## Self-review плана

- Spec coverage: conversion/extraction Tasks 1–2; projections Tasks 3–6; type/error/identity protections Tasks 1–2; lifecycle/access/retrieval Tasks 3–6; cold/steady/concurrency/cancellation/platform/evidence Tasks 1 и 7; release gates Task 7. Pagination/admission/allocator явно самостоятельный scope.
- Step scan: каждый production edit имеет preceding RED и narrow GREEN command; setup привязан к измеряемому deliverable, нет самостоятельного scaffolding task. Длинные acceptance tasks делятся на named scenarios, а не один test с conditional assertions.
- Type consistency: DbClient signatures прежние; new row converter Vec<Value>; decay builder/store limit i32; scan builder без параметров; AllocationReport/HttpMemoryReport поля и units совпадают с assertions.
- Review Focus: все пять рисков имеют owning tests; обязательный memory FAIL нельзя заменить зелёными leaf characterizations.
- Proportion: plan фиксирует интерфейсы, literals, команды и gate decisions; implementation bodies оставлены исполнителю. Никакого experimental converter implementation в документе.

## Handoff

План и spec требуют review до implementation. Рекомендация — Subagent-driven: общая conversion граница имеет широкий compatibility impact, а новые Linux gates требуют независимого review измерений. Native тоже допустим с whole-branch fresh review после всех гейтов. Пользователь пока запросил только план; ни один task implementation не начат.
