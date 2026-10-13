# Storage materialization memory remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Устранить подтверждённое раздувание памяти storage adapter и ненужное чтение embeddings, доказав кратное снижение памяти без регрессии производительности и качества для HTTP/CLI, разных размерностей, reembed и работы без embeddings.

**Architecture:** Consuming conversion остаётся в storage infrastructure и применяется одинаково ко всем операциям `SurrealDbClient`. Knowledge-owned query builders задают минимальные проекции; memory workers и retrieval сохраняют текущую бизнес-семантику. Отдельные integration/process fixtures измеряют реальный adapter и production binary; CI и staging закрывают разные гейты.

**Tech Stack:** Rust 1.99.0; SurrealDB/SDK 3.3.2; serde_json 1.0.151; Tokio; существующие Dockerfile, xtask sampler, production composition, HTTP/MCP fixtures; текущий Cargo.lock.

**Spec:** [2026-10-13-storage-materialization-memory-design.md](../specs/2026-10-13-storage-materialization-memory-design.md). Доказательства: [production/root-cause report](../../performance/MEMORY_ROOT_CAUSE_2026-10-13.md) и `docs/performance/memory-root-cause-2026-10-13/manifest.json`.

## Global Constraints

- Rust `1.99.0`; SurrealDB/SDK `3.3.2`; `serde_json` `1.0.151`; текущий `Cargo.lock`, команды с `--locked`.
- Новых зависимостей, миграций, generated-code изменений и MCP tools нет. `Cargo.toml` не меняется.
- Один Active Namespace, выбранный при binding. Не вводить namespace/table selector в application-facing read.
- HTTP application features: `streamable-http,mcp-apps` плюс default `fs-watch`; без `mimalloc` и `test-fixtures` в измеряемом binary.
- Facts не удаляются; decay retracts через `ClaimStore`, атомарно закрывая fact и derived claims.
- Не менять temporal predicates, ordering, row limits, relevance thresholds, cache defaults, job cadences и ranking algorithms. Отдельный RED-разрешённый repair: semantic candidate без score/пригодного fallback vector не допускается при threshold 0/-1 (Task6).
- Public `DbClient`/stores/capabilities — test boundary; private helper visibility не расширяется ради тестов.
- Synthetic fixture v2: dimensions `17,384,768,1536,2048,3072` и absent vectors; isolated cases 300/1000 rows; stable HTTP306 facts /300 active /41 episodes; transition205 facts. Исторические v1 artifacts не менять.
- G2 D≤2048: `< 67_108_864` bytes для300, `< 201_326_592` для1000; D3072: `< 100_663_296` / `< 301_989_888`; no-vector: `< 16_777_216` / `< 50_331_648`; `peak_1000 <= 4 * peak_300` в каждой mode/dimension.
- G4 native Linux amd64, glibc `2.41-12+deb13u4`, `MALLOC_ARENA_MAX=2`: peak RSS `< 268_435_456`, steady RSS+swap `< 134_217_728`, swap 0, cgroup peak `< 335_544_320`, OOM deltas 0.
- G4: 20 warmup + 100 cache-miss requests; plateau growth `<= 8_388_608` bytes; 3 fresh-process repetitions; concurrent 8 foreground requests (4×2 tenants) + maintenance parallelism 1.
- G4 absolute targets относятся к controlled remote/disabled; local native model total peak/steady≤baseline, plateau≤8MiB, model memory не вычитать. Для comparable2048 HTTP peak candidate≤0.25×legacy на одинаковом RAM3GiB плюс candidate absolute G4 при RAM1GiB.
- G7 matrix из spec полностью обязательна, включая disabled-with-stored-vectors, backfill, width increase/decrease, same-width signature change, interrupted reembed/resume и CLI/HTTP composition.
- G8:5 paired A/B runs, A/A calibration,30 warmup +1000 requests на subcase, fixed-seed10_000 paired run-block bootstrap; point latency/CPU≤1.00 и throughput≥1.00;95% uncertainty bounds1.03/0.97. Missing/inconclusive=OPEN.
- G9: native release profile,37 retrieval-quality cases,6 метрик=1.0; fresh source-parent quality A/B budget0; literal Recall@5/MRR/nDCG@5 и policy/abstention per mode, reranker on/off. Native model manifests immutable.
- Production `82.165.149.36:2288` read-only; rollout только по отдельной явной авторизации после гейтов.
- Секреты и реальные memory contents не попадают в fixtures, git, artifacts и логи.

## Review Focus

1. Tagged-to-plain conversion теряет RecordId identity, rare typed values или fractional datetime: Tasks 1–2 проверяют public storage round trips и literal fixtures.
2. Projection возвращает None/Null так, что decay пропускает активные facts или неправильно закрывает claims: Task 3 проверяет отсутствие/explicit null и durable atomic retraction.
3. Уменьшение payload незаметно меняет historical visibility, policy filtering или ranking: Tasks 4–5 проверяют IDs/order на прежнем полном наборе кандидатов.
4. ANN projection теряет fallback; reembed ломает width/signature/index/cursor; disabled читает старые vectors: Tasks6–7 проверяют переходы и invalid candidates при threshold0/-1.
5. Memory PASS скрывает cache hits, platform mismatch или ухудшение latency/quality: Tasks1,8–9 проверяют настоящий workload, fail-closed evidence и независимые A/B gates.

---

## Объём, зависимости и критерий завершения

Tasks1–3 — P0; Tasks4–7 — проекции и embedding lifecycle; Task8 — полный memory gate; Task9 — performance/quality и итоговая приёмка. Dependencies:1→2→3→4→5→6→7→8→9. Выпускать частичный результат нельзя: удаление одного clone оставляло800MiB, а decay projection не защищает остальные callers. Все G0–G9 блокируют merge/release; GRAY/skip/нет стенда означает открытый гейт.

| Gate | Owning tasks и проверяемый результат |
|---|---|
| G0 | Tasks1–9: preceding RED assertions, commands/exits/source SHA; passing characterization не RED |
| G1 | Tasks1–7: public types/identity/errors/temporal/use-case contracts |
| G2 | Tasks1–2: native remote adapter300/1000 ×dimension/no-vector, allocation/scaling budgets |
| G3 | Tasks3–7: real projection results для всех read paths и widths; IDs/order/visibility |
| G4 | Task8: full production binary memory/plateau/4×factor, all modes/scenarios/repetitions |
| G5 | Task9: complete project suites, fmt/clippy/toolchain/observability, final review безP0/P1 |
| G6 | Tasks8–9: comparable immutable provenance, complete sample/outcome/case-ID inventory |
| G7 | Task7: stable/disabled/backfill/reembed/restart/resume matrix; public CLI/HTTP |
| G8 | Task9: separate controlled speed/CPU/throughput paired A/B, calibration/uncertainty |
| G9 | Task9: native release и mode quality; fresh baseline budget0; no per-case evidence loss |

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
| `crates/memory-mcp/src/memory/retrieval/semantic.rs` | Modify только admission repair после RED и tests, Task6 | Отличать missing evidence от valid score0/-1; сохранить fallback/ranking |
| `crates/memory-mcp/src/embedding/reembed_store.rs` | Modify, Task7 | Четыре поля reembed batch, обе cursor ветки |
| `crates/memory-mcp/tests/embedding_memory_modes.rs` | Create, Task7 | Public use-case contracts по dimensions/modes/transitions |
| `crates/memory-mcp/tests/common/embedding_memory_modes.rs` | Create, Task7 | Content-dependent provider, durable seed и CLI/HTTP task fixture |
| `crates/memory-mcp/tests/http_embedding_reembed.rs` | Modify только если отсутствует нужный dispatch case, Task7 | Full dispatch/index/state/resume protections |
| `crates/memory-mcp/tests/http_embedding_backfill.rs` | Existing protections, Task7 | Реальная backfill/recovery policy |
| `crates/memory-mcp/tests/common/http_storage_memory.rs` | Create, Task8 | Owned DB/provider/app containers, durable seed, sampling, artifacts; perf measurement mode Task9 |
| `crates/memory-mcp/tests/http_storage_memory.rs` | Create, Task8 | Production-composition matrix memory gate |
| `crates/memory-mcp/tests/fixtures/memory_exec_barrier.rs` | Create, Task8 | Standalone std-only Linux executable: file gate → same-PID exec, без shell в distroless |
| `crates/memory-mcp/tests/http_storage_performance.rs` | Create, Task9 | Paired full HTTP/CLI production-binary timing/CPU/provider evidence |
| `crates/eval-harness/tests/common/storage_release_gates.rs` | Create, Task9 | Comparable artifact validation, performance/quality acceptance; test-only |
| `crates/eval-harness/tests/storage_release_gates.rs` | Create, Task9 | Literal RED tests и настоящие A/B artifact gates |
| `crates/memory-mcp/tests/fixtures/storage_quality/modes-v2.json` | Create, Task7/9 | Frozen synthetic texts, queries, independently reviewed goldens |
| `.github/workflows/ci.yml` | Modify, Tasks8–9 | Required memory/performance/native-quality checks, cargo entry points, artifacts |
| `docs/performance/MEMORY_PROFILE.md` | Modify, Tasks8–9 | Реальные measurements и ограничения |

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

Для projection fixtures использовать `SurrealDbClient::connect_in_memory_with_dimension(&str,&str,&str,usize)` с выбранной positive D и feature `eval-support`; seed through public `DbClient`. Existing consumer helpers с default dimension не подходят для произвольной D. Remote cases: raw SDK connect/signin/bind один раз, public `from_prebound_remote_with_dimension(Surreal<Client>, &str, &str, usize)` и реальные migrations с D. `connect_bound` сам хранит default1536. No-vector — absent field при valid positive schema width, не DIMENSION0. Test inventory проверять `cargo test ... -- --list`: mandatory cases не должны исчезать из-за cfg/feature, commands явно включают eval-support.

### Обязательная дисциплина исполнения

- До кода прочитать spec, AGENTS.md, TDD и `writing-good-tests.md`. Experimental `/private/tmp/.../src/main.rs` не читать, не копировать, не адаптировать как implementation reference. Использовать доказательства и pinned SDK source.
- Перед исполнением снять актуальный `git status`: прежний snapshot25 dirty files устарел, состояние меняется другими задачами. Применить `using-git-worktrees`; execution-parent SHA/dirty snapshot/lock hash зафиксировать **до** первого fix и использовать для обеих source A/B builds. Не reset/stash/commit чужие изменения, не `git add .`.
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
- [ ] **2.7a Dimension/no-vector protection:** независимо именованные 300/1000 cases для D17/384/768/1536/2048/3072 и absent embedding. D0 в `run_remote_allocation_case(rows,0)` — только **test fixture sentinel** «поле отсутствует»; никогда не передавать0 schema/provider constructor. `AllocationReport.dimension=0` только для этого case. Assert row count, actual vector.len=D и literal samples на first/middle/last positions; для absent `assert!(row.get("embedding").is_none())`. G2 thresholds из Global Constraints; scaling ratio сопоставляет одну D/mode. Добавлять до dimension-specific repair; baseline-passing cases — protection, не выдуманный RED. Команда всего resource target с `--ignored --test-threads=1`; обязательная inventory:14 row/dimension cases плюс clone/bound-copy regressions.
- [ ] **2.8 Refactor и verify:** повторить весь storage contract target и G2; `rtk cargo fmt --all --check`. Counter и profiler остаются test-only. `rtk git add crates/memory-mcp/src/storage.rs crates/memory-mcp/src/storage/json_conversion.rs crates/memory-mcp/src/storage/helpers.rs crates/memory-mcp/src/storage/client.rs crates/memory-mcp/tests/common/storage_memory.rs crates/memory-mcp/tests/storage_materialization_memory.rs crates/memory-mcp/tests/storage_json_contract.rs`; `rtk git commit -m "perf(storage): consume typed values without tagged amplification"`.

## Task 3: Минимальный knowledge read для decay

**Files:** queries.rs, graph_store.rs, decay.rs; tests/lifecycle_decay.rs и knowledge_fact_projections.rs.

**Interfaces:** Produces `pub fn build_select_decay_facts_query(cutoff: &str, limit: i32) -> (String, Value)` в knowledge/queries.rs и `pub async fn select_decay_candidates(&self, limit: i32) -> Result<Vec<Value>, MemoryError>` в `KnowledgeGraphStore`. Consumes existing visibility builder и BoundDbClient::query_rows. `run_decay_pass(&LifecycleHandles<'_>, f64, f64) -> Result<usize, MemoryError>` signature не меняется.

- [x] **3.1 Write regression через существующий public `platform::lifecycle_runtime::decay_pass`:** named `decay_reads_only_lifecycle_fields`. Настоящая DB + QueryObserver записывает выполненный SELECT; test проверяет read contract на boundary, не исходный текст функции. Observer fixture local to integration target, implements existing QueryObserver; gate feature `eval-support`.

```rust
assert_eq!(selected_columns, ["fact_id", "t_valid", "t_invalid",
    "confidence", "access_count", "last_accessed"]);
assert_eq!(invalidated, 1); // old fact 0.4, age 400 days, threshold .3, half-life 100
assert!(stored_fact["t_invalid"].as_str().is_some());
assert!(stored_fact["t_invalid_ingested"].as_str().is_some());
```

`selected_columns` извлекается из фактически выполненного query observer event; literals не берутся из нового projection constant. Дополнительный store-result test проверит field absence, чтобы одного SQL-текста не хватало.
- [x] **3.2 Verify RED:** `rtk cargo test -p memory_mcp --test lifecycle_decay --features eval-support --locked decay_reads_only_lifecycle_fields -- --exact --test-threads=1`. FAIL зафиксирован на реальном SQL observer: actual projection была `[*]` вместо шести lifecycle fields; после реализации тот же тест прошёл.
- [x] **3.3 Minimal GREEN:** добавить builder с шестью полями, прежним full visibility clause/ordering/limit; метод graph store; заменить один call в run_decay_pass на `select_decay_candidates(DECAY_BATCH_LIMIT)`. Лимит 1000 и existing ClaimStore request/reason `confidence_decay` прежние.
- [x] **3.4 Verify real result:** named `decay_projection_omits_content_and_embedding`; вызвать новый public store method для seeded fact с embedding/content:

```rust
assert!(row.get("embedding").is_none());
assert!(row.get("content").is_none());
assert_eq!(row["fact_id"].as_str(), Some(fact_id.as_str()));
assert_eq!(row["confidence"], 0.4);
```

Run `rtk cargo test -p memory_mcp --test knowledge_fact_projections --features eval-support --locked decay_projection -- --test-threads=1` и 3.2; expected PASS. Проверяются исчезновение embedding/content и сохранение fact_id/confidence.
- [x] **3.5 Protect lifecycle outcomes отдельными tests:** переиспользованы `decay_pass_invalidates_active_fact_with_absent_t_invalid_field`, `decay_projection_keeps_explicit_null_invalid_date_active`, `decay_pass_skips_already_invalidated_facts`, `decay_pass_when_fact_was_recently_accessed_then_skips_invalidation`, `decay_projection_closes_derived_claims_after_successful_retraction`, `decay_pass_processes_only_active_namespace`. Проверяются counts и durable readback fact/claim; причина `confidence_decay` сохраняется у fact. Claim close выполняется после fact close отдельным запросом, поэтому проверен успешный lifecycle outcome, а DB transaction atomicity/failure rollback здесь не утверждается. Для каждого необходимого исправления наблюдать новый RED до кода. Existing relevant tests переиспользовать вместо дублирования.
- [x] **3.6 Verify suite и commit:** `rtk cargo test -p memory_mcp --test lifecycle_decay --test lifecycle_archival --test knowledge_fact_projections --features eval-support --locked -- --test-threads=1`; `rtk cargo test -p memory_mcp --lib --features streamable-http,test-fixtures --locked http::lifecycle::tests`. Explicit git add четырёх source/test paths + нового target; `rtk git commit -m "perf(knowledge): project only decay candidate fields"`.

## Task 4: Nonsemantic projection для общих active reads

**Files:** knowledge/queries.rs, knowledge_fact_projections.rs, service_integration.rs; existing wake-up/dashboard protection tests.

**Interfaces:** Existing `build_select_active_facts_query(&str, i32) -> (String, Value)`; both `KnowledgeGraphStore::select_active_facts(i32)` and the private-module `KnowledgeStoreClient::select_active_facts(i32)` delegate to it. Use existing `NONSEMANTIC_FACT_PROJECTION`, not the six-field decay projection. Do not widen the private store's visibility for tests: verify its real experience consumer through the public context-assembly use case.

- [x] **4.1 Write failing store-result test:** `active_fact_reads_omit_embeddings_and_keep_nonsemantic_fields` through the public `KnowledgeGraphStore` on the embedded SurrealDB fixture. Persist a full-size embedding first; assert it is omitted while content, source episode, policy tags, confidence, and index_keys remain. The private knowledge store is covered through `test_service_experience_read_keeps_source_and_confidence`.

```rust
assert!(row.get("embedding").is_none());
assert_eq!(row["content"], "synthetic active fact");
assert_eq!(row["source_episode"], "episode:active-fixture");
assert_eq!(row["policy_tags"], serde_json::json!(["persona"]));
```

- [x] **4.2 Verify RED:** `rtk proxy cargo test -p memory_mcp --test knowledge_fact_projections --features eval-support --locked active_fact_reads -- --test-threads=1`. Expected FAIL: the existing `SELECT *` result contains the stored embedding.
- [x] **4.3 Minimal GREEN:** заменить только projection в builder, оставить visibility и `ORDER BY t_valid ASC LIMIT $limit`. Caller limits 500/10000 не менять. Структура existing stores не меняется.
- [x] **4.4 Verify GREEN и consumer contracts:** run the whole `knowledge_fact_projections` target with `eval-support`; run `build_select_active_facts_query_uses_full_bitemporal_visibility` and `decay_candidate_query_preserves_active_visibility_order_and_bindings` to pin the complete temporal predicate, ordering, caller limit and bindings; run `test_service_experience_read_keeps_source_and_confidence`, `test_service_assemble_context_wake_up_prioritizes_persona_then_recent`, and `lifecycle_dashboard_returns_the_public_operation_envelope`. Query tests pin temporal/limit semantics; consumers pin public fields and ordering. Any behavior fix requires a new RED first.
- [x] **4.5 Commit:** explicitly add `knowledge/queries.rs`, `knowledge_fact_projections.rs`, and `service_integration.rs`; `rtk git commit -m "perf(knowledge): omit vectors from active fact reads"`.

## Task 5: Полные nonsemantic scans без vectors и без потери recall

**Files:** knowledge/queries.rs, knowledge/knowledge_store.rs, knowledge_fact_projections.rs; точечно existing service_acceptance/service_integration tests.

**Interfaces:** Produces `pub fn build_scan_nonsemantic_facts_query() -> String` в knowledge/queries.rs. Existing `KnowledgeStoreClient::scan_facts(&self) -> Result<Vec<Value>, MemoryError>` сохраняет signature и весь прежний row set; query через `BoundDbClient::query_rows` с `None` vars. Projection `id, {NONSEMANTIC_FACT_PROJECTION}`.

- [x] **5.1 Write failing integration test:** `nonsemantic_scan_omits_vectors_without_truncating_history` через публичный `AssembleContextCapability` в `wake_up` mode; private `KnowledgeStoreClient::scan_facts` не открывать для тестов. Seed три independently identified facts (active, closed after an earlier as-of, newest), each with a persisted vector. Install `QueryObserver` after seeding. At a cutoff before closure assert all three fixture fact IDs are returned; at a later cutoff assert the closed fact is absent. From the executed fact query assert `id` plus required nonsemantic fields are selected, `embedding` is absent, and no `WHERE`/`LIMIT` narrows the scan. The result assertions prove the projected temporal/policy/source fields remain usable; the observer proves which SQL actually served the public use case.

```rust
assert_eq!(returned_ids, ["fact:active", "fact:closed", "fact:newest"]); // sorted only for set check
assert!(active_row.get("embedding").is_none());
assert_eq!(closed_row["t_invalid"], "2026-02-01T00:00:00Z");
assert!(closed_row.get("id").is_some());
```

- [x] **5.2 Verify RED:** `rtk cargo test -p memory_mcp --test knowledge_fact_projections --features eval-support --locked nonsemantic_scan_omits_vectors_without_truncating_history -- --exact --test-threads=1`. Expected FAIL because executed scan SQL still selects `embedding` via `SELECT *`; fixture creation and both historical results must already pass.
- [x] **5.3 Minimal GREEN:** projected SQL полного fact scan в owning knowledge context. Не менять table-generic `DbClient::select_table`: другие owner reads требуют их полные записи. Не добавлять WHERE/LIMIT/paging или менять lexical/temporal/wake-up functions.
- [x] **5.4 Verify GREEN:** 5.2 плюс `rtk cargo test -p memory_mcp --test knowledge_read_scopes --test service_acceptance --test service_integration --test assemble_context_supersession_ranking --locked`. Add `lexical_fallback_runs_the_nonsemantic_fact_scan` to pin the public lexical caller and `temporal_scan_keeps_relative_event_date_match` for a fact whose `t_valid` is outside the requested day but whose relative event date is inside. Prove unbounded scan completeness independently with `full_scan_keeps_rows_after_the_first_1000_records`: seed more than 1,000 facts, place a deterministic record ID after row 1,000 in the actual table-scan order, then assert the wake-up view returns the complete expected ID set with a budget above the row count. This uses the scan-backed public view rather than relying on FTS candidate ordering: `QueryObserver` records executed SQL but does not expose result rows. Add `projected_scan_preserves_policy_restrictions` for access filtering. Reuse `test_service_assemble_context_wake_up_prioritizes_persona_then_recent` for ordering and the before/after cutoff assertions in 5.1 for the closed historical predecessor; do not duplicate those complete scenarios. Pin returned fixture IDs, not only nonempty results.
- [x] **5.5 Review:** подтвердить по executed-query/results, что rows не усекаются и no semantic embedding consumer зовёт изменённый full scan. Вернуть ошибку storage по прежнему contract; missing table degradation проверить existing knowledge scope tests.
- [x] **5.6 Commit:** explicit add queries.rs, knowledge_store.rs, projection target и реально добавленных use-case tests; `rtk git commit -m "perf(knowledge): project metadata for nonsemantic scans"`.

## Task 6: ANN response projection с сохранением semantic fallback

**Files:** knowledge/queries.rs; knowledge_fact_projections.rs; memory/retrieval/semantic.rs для узкого admission repair после6.4a и protections; новый embedding_memory_modes target/common fixture начинается здесь и продолжается Task7.

**Interfaces:** Existing `build_select_facts_ann_query(cutoff: &str, query_vec: &[f64], limit: i32) -> (String, Value)` и `KnowledgeStoreClient::select_facts_ann(&str, &[f64], i32) -> Result<Vec<Value>, MemoryError>`. Возвращает `id + NONSEMANTIC_FACT_PROJECTION + sem_score`; embedding участвует в DB expression/predicate, отсутствует в row payload.

- [x] **6.1 Write failing result test:** `ann_returns_score_without_vector_payload`. Real embedded Surreal dimension 2048, vectors `e0` и `e1`, query `e0`, limit 1.

```rust
assert_eq!(rows.len(), 1);
assert_eq!(rows[0]["fact_id"], "fact:semantic-e0");
assert_eq!(rows[0]["sem_score"].as_f64(), Some(1.0));
assert!(rows[0].get("embedding").is_none());
```

- [x] **6.2 Verify RED:** `rtk cargo test -p memory_mcp --test knowledge_fact_projections --features eval-support --locked ann_returns_score_without_vector_payload -- --exact --test-threads=1`. Expected FAIL по присутствующему embedding.
- [x] **6.3 Minimal GREEN:** изменить SELECT-list; сохранить cosine expression, K/ef_search, visibility, ordering и binding limits. Не удалять `embedding_from_value` fallback в semantic collector.
- [x] **6.4 Verify GREEN:** 6.2 и `rtk cargo test -p memory_mcp --lib --locked memory::retrieval::semantic::tests`. Через public assemble_context/use-case boundary проверены independently named cases: `semantic_score_without_embedding_keeps_candidate`, `semantic_missing_score_uses_embedding_fallback`, invalid numeric similarity → existing validation error. Fixtures — literals1.0/0.0 и e0/e1; expected не вычислять production cosine helper. Private collector/validator не расширять ради tests.
- [x] **6.4a Separate RED for unsupported candidate:** existing collector превращал absent score/vector в similarity0. При допустимом threshold0/-1 он проходил filter. Четыре independently named проверки обнаружили admission для rows без пригодного доказательства или с неверной шириной; после исправления collector они проходят. `semantic_candidate_missing_evidence_is_rejected_at_zero_threshold`, `semantic_candidate_missing_evidence_is_rejected_at_negative_threshold`, `semantic_candidate_wrong_width_fallback_is_rejected`, `semantic_candidate_malformed_score_without_vector_is_rejected`. Дополнительные protections сохраняют explicit score0 при threshold0 и score-1 при threshold-1.
- [x] **6.4b Minimal GREEN only after6.4a:** collector сохраняет distinction «нет usable evidence» и scalar0; неподдержанные rows пропускаются. Valid score/fallback, пороги0/-1, tie-break/reranking/order/budget сохранены. Explicit score0 при threshold0 и score-1 при threshold-1 допускаются; out-of-range numeric остаётся storage error. Новые protections и semantic/service suites прошли. Изменение ограничено admission validation.
- [x] **6.5 Protect end-to-end ranking:** `rtk cargo test -p memory_mcp --test service_acceptance --test service_integration --test assemble_context_semantic_quality --locked`. Набор кандидатов, source identity и optional reranking input прежние. Любой новый repair начинает свой RED цикл.
- [x] **6.6 Commit:** explicit add queries.rs, projection target, semantic.rs и действительно созданные embedding_memory_modes target/common fixture; production repair и разрешившие его RED tests входят в один commit; `rtk git commit -m "perf(knowledge): omit ANN vectors from scored responses"` — `c98be03`.

## Task 7: Reembed projection и контракты разных embedding modes

**Files:** embedding/reembed_store.rs; новые tests/embedding_memory_modes.rs, tests/common/embedding_memory_modes.rs, fixtures/storage_quality/modes-v2.json; existing http_embedding_reembed/http_embedding_backfill/startup_embedding_metadata protections. Новый target начат Task6; не создавать несовместимый второй harness.

**Interfaces:** Production signatures прежние: `ReembedStoreClient::select_facts_needing_reembed(&self, target_signature: &str, last_completed_fact_id: Option<&str>, limit: i32) -> Result<Vec<Value>, MemoryError>` остаётся внутренним owner store. Публичный focal action — `MemoryService::reembed_all_facts(&self, &ReembedOptions, &dyn ReembedProgressReporter, &tokio_util::sync::CancellationToken) -> Result<(ReembedSummary, ReembedOutcome), MemoryError>`, public CLI process или protected HTTP operator route. Test utility определяет `pub enum EmbeddingFixtureMode { Remote { dimension: usize, model_revision: String }, DisabledEmpty, DisabledStored { stored_dimension: usize }, LocalNative }` и `pub enum EmbeddingTransition { Dimension { from: usize, to: usize }, Signature { dimension: usize }, Backfill }`; используются Task8–9. Remote provider — внешний content-dependent endpoint в process tests; existing EmbeddingProvider port для narrow integration scenarios, не mock persistence. TestResult из Task1.

- [ ] **7.1 Write RED projection regression:** `reembed_batch_does_not_read_old_vectors` через public reembed use case, real DB и delegating QueryObserver, seed205 stale facts D3072, target384. Во время observer читать real delivered batch через delegating DbClient wrapper, без подмены rows. До vector rewrite assert каждая row имеет `fact_id/fact_type/content/quote`, не имеет `embedding` и ненужного large metadata; first100 IDs, second100 после cursor, last5. Assertions используют literal expected IDs и content, не новый projection constant. Existing raw DB readback vectors вне measurement не включать в batch assertion.
- [ ] **7.2 Verify RED:** `rtk cargo test -p memory_mcp --test embedding_memory_modes --features eval-support,streamable-http,test-fixtures --locked reembed_batch_does_not_read_old_vectors -- --exact --test-threads=1 --nocapture`. Expected FAIL по vector presence от SELECT*. Provider target/schema setup mismatch не RED. Зафиксировать SQL/result evidence **обеих cursor branches**; private store visibility не менять.
- [ ] **7.3 Minimal GREEN:** в обеих query ветках select только `fact_id, fact_type, content, quote`. Сохранить stale-signature OR predicate, subquery при cursor, stable ORDER BY fact_id ASC, `$limit`; batch100, writes/index/state/fencing/retry policies не менять. Inner query может фильтровать embedding_signature без возврата этого поля. Conditional identity read/write в embedding/backfill_store.rs не трогать — там чтение old identity необходимо для race-safe conditional update.
- [ ] **7.4 Verify GREEN:**7.2; `rtk cargo test -p memory_mcp --lib --features eval-support --locked service::reembed::tests`; `rtk cargo test -p memory_mcp --test http_embedding_reembed --test http_embedding_backfill --test startup_embedding_metadata --features streamable-http,test-fixtures,eval-support --locked -- --test-threads=1`. Каждая проверка факт/claims/episode metadata неизменны кроме embedding-owned columns; include access_count/temporal metadata изменённые concurrent writer. Existing owner integration tests переиспользовать.
- [ ] **7.5 Freeze independently reviewed mode corpus:** modes-v2.json содержит минимум20 answerable +5 unknown +5 protected/historical queries, graded relevance labels0/1/2, fixed as_of, literal ordered source keys и required evidence. Goldens проверены по synthetic sources, не сняты только с candidate. Существующие corpus/модельные manifests не редактировать для PASS. Полностью одинаковые fake vectors не различают wrong model/ranking — endpoint задаёт по content e0/e1 и signature-specific deterministic mapping.
- [ ] **7.6 Add independently named matrix protections BEFORE any mode repair:** stable width17/384/768/1536/2048/3072, disabled-empty, disabled-with-stored3072, wrong provider width, invalid configured0, mixed stored widths, absent/explicit-null schema behavior, enabled backfill. Width каждого fact и index читать через public DbClient/INFO; assert literal target D, signature и final state, не только `is_enabled`. Disabled cases: real model provider выключен; отдельный HTTP trap listener возвращает503 и регистрирует все requests, assert0calls; no ANN query по observer, ingest/extract/lexical/temporal/graph/wake-up/experience/lifecycle success; stored vectors checksum неизменён. Wrong width ошибки/degraded response сравнивать с existing documented contract, не писать новый policy ради теста. Baseline PASS — protection; найденный FAIL требует отдельного RED → minimal repair в owning context до продолжения.
- [ ] **7.6a Complete G3 per width/mode:** existing projection target параметризовать17/384/768/1536/2048/3072/absent; отдельные named cases для decay/active/scan/ANN/reembed results. Nonsemantic rows одинаковы при одинаковых facts независимо отD, embedding отсутствует; ANN score и ordered IDs совпадают с hand-derived e0/e1 expectations, query/index widths=D. Disabled case не вызывает ANN, но проверяет все остальные reads со stored3072 ибезvectors. Projection verification через actual owner store/use-case results, не equality SQL constant itself. Run `rtk cargo test -p memory_mcp --test knowledge_fact_projections --features eval-support --locked -- --test-threads=1`; mandatory per-dimension inventory приложить к G3/G7.
- [ ] **7.7 Transition cases:** отдельно `reembed_384_to_2048`, `reembed_2048_to_384`, `reembed_1536_to_3072`, `reembed_changes_signature_at_768`, `reembed_resumes_after_success_101`, `reembed_retries_failure_after_success_101`, `reembed_second_pass_is_noop`, `reembed_does_not_modify_other_tenant`. 205facts, provider barrier/reporter сигнализирует101 successes; cancel/fail/restart ordering не sleeps. Resume reads durable cursor, failed fact не пропущен; assert205 correct vectors/signatures, target index width, ready/completed state **только после завершения**. При interrupted pass не требовать готовый индекс; lexical context остаётся доступен. Same-width switch до explicit reembed не перезаписывает старые vectors автоматически. Каждая matrix transition проходит как CLI subprocess и HTTP real task dispatch; fixture не читает/пишет production.
- [ ] **7.8 Verify complete matrix:** `rtk cargo test -p memory_mcp --test embedding_memory_modes --features eval-support,streamable-http,test-fixtures --locked -- --test-threads=1 --nocapture`. Проверить inventory named cases; matrix table spec — обязательный coverage manifest с case IDs, не условный loop, который silently skips unsupported setup. SDK/setup failure оставляет G7 OPEN. Новая бизнес-логика не добавляется в service/; уже существующий reembed orchestration сохраняется, repair при необходимости — owner context.
- [ ] **7.9 Refactor/commit:** повторить7.4/7.8 и semantic candidate protections Task6. Explicit add reembed_store.rs, перечисленные new fixtures/tests и реально расширенные existing tests; `rtk git commit -m "perf(embedding): project reembed inputs and protect mode transitions"`.

## Task 8: Полный HTTP memory gate, CI и evidence для выпуска

**Files:** tests/common/http_storage_memory.rs, tests/http_storage_memory.rs, .github/workflows/ci.yml, docs/performance/MEMORY_PROFILE.md. Existing `crates/xtask/src/memory.rs` sampler использовать без изменения, если нет воспроизведённого дефекта.

**Interfaces:** В новом test utility определить:

```rust
pub enum MemoryScenario {
    ColdDecay, RetrievalMisses, ConcurrentMaintenance, ErrorRelease,
    Backfill, Reembed, InterruptedResume,
}
pub enum RetrievalWorkload {
    LexicalFallback, Temporal, WakeUp, Experience, Ann, AssembleContext, Dashboard,
}
pub enum ProcessComposition { Http, Stdio, Cli }
pub struct MemoryCase {
    pub composition: ProcessComposition,
    pub mode: EmbeddingFixtureMode,
    pub scenario: MemoryScenario,
    pub transition: Option<EmbeddingTransition>,
    pub workload: Option<RetrievalWorkload>,
    pub repetition: u8,
}
pub struct HttpMemoryReport {
    pub peak_rss_bytes: u64,
    pub steady_rss_plus_swap_bytes: u64,
    pub max_swap_bytes: u64,
    pub cgroup_peak_bytes: u64,
    pub oom_delta: u64,
    pub oom_kill_delta: u64,
    pub plateau_growth_bytes: i64,
    pub unexpected_failed_requests: usize,
    pub expected_fault_outcomes: usize,
    pub db_read_delta: u64,
    pub samples: usize,
}
pub async fn run_http_memory_case(image: &str, case: &MemoryCase,
    output_dir: &std::path::Path) -> TestResult<HttpMemoryReport>;
pub fn validate_http_memory_evidence(output_dir: &std::path::Path)
    -> TestResult<HttpMemoryReport>;
```

Consumes Task 1 `TestResult`, existing `HttpProductionComposition::connect(&HttpConfig)`, durable owner stores и `common::http_server::mcp_call`. Test fixture seed создаёт ready tenants/account/API key bindings и source facts до app launch; никаких production-only debug hooks. Image argument — captured immutable local image ID или baseline digest, не mutable `latest`. Наличие app features `test-fixtures` запрещено гейтом. `db_read_delta` считать по existing `db.execute_query.done` DEBUG events, только foreground SELECT с request_id соответствующих cache-miss запросов; background queries и startup seed не включаются. Test env включает DEBUG для этих diagnostic events; SQL/content только synthetic, credentials/binds не печатаются. Сигнатуры новых production methods не добавляются.

`composition` — явная часть case ID/manifest и driver dispatch, не выводится из embedding mode. Полная tenant/concurrency/dashboard matrix относится к HTTP. Long-lived local memory/cache/retrieval cases используют stdio MCP `memory_mcp serve` с pinned models или remote/disabled mode; одноразовый CLI не изображает long-lived plateau. CLI subprocess cases измеряют public startup/reembed completion и durable outcomes; rate reembed считается по фактам, не по выдуманным MCP requests. Для CLI/noncache path не требовать100cache misses: manifest фиксирует фактический action и число завершённых work units. Existing function names `run_http_*` обозначают test utility entry point; он обязан dispatch по `composition`, отчёт всегда содержит настоящий binary/transport. Unsupported composition/scenario combination — заранее reviewed N/A, никогда silent skip. Public HTTP reembed, CLI reembed и stdio retrieval — разные обязательные case IDs.

- [ ] **8.1 Write full-app acceptance tests before harness implementation:** independently named scenarios ×spec mode matrix ×repetition1/2/3. Один process =один workload/mode. Assertions ниже для remote/disabled; LocalNative проверяет total peak/steady≤paired baseline, plateau8MiB и0 swap/OOM по spec, не remote absolute budget:

```rust
assert!(r.peak_rss_bytes < 268_435_456);
assert!(r.steady_rss_plus_swap_bytes < 134_217_728);
assert_eq!(r.max_swap_bytes, 0);
assert!(r.cgroup_peak_bytes < 335_544_320);
assert_eq!(r.oom_delta, 0);
assert_eq!(r.oom_kill_delta, 0);
assert!(r.plateau_growth_bytes <= 8_388_608);
assert_eq!(r.unexpected_failed_requests, 0);
assert!(r.samples > 0);
// RetrievalMisses scenario: assert!(r.db_read_delta >= 100);
```

`samples>0` недостаточно для полноты: fixture обязан отвергнуть missing cold-start coverage, early sampler death, PID replacement, отсутствие steady window/cgroup counters/platform manifest. Проверка полноты — до возврата report; returned report никогда не может скрыть invalid run.
- [ ] **8.2 Create minimal test-only measurement fixture to execute8.1:** managed containers/process guards с kill+wait+remove; separate remote Surreal3.3.2/provider/sampler; content-dependent provider выбранной D/signature из Task7; в disabled mode real model provider выключен, отдельный trap listener регистрирует попытки вызовов без генерации embeddings. Test plumbing разрешено до acceptance RED, application source не меняется. Candidate existing Dockerfile, immutable runtime base; app remote/disabled RAM1GiB/RAM+swap2GiB, System allocator. LocalNative long-lived stdio MCP process RAM3GiB/no-swap, реальные pinned models; CLI startup/reembed измеряются отдельными action cases. Host/image nativeamd64, glibc2.41-12+deb13u4 для remote retention. Teardown10s, startup60s; ColdDecay4000s, другие memory scenarios600s. External wrapper barrier до sampler, затем exec app samePID. Guard behavior реализовать по guard RED cycles8.6 до признания evidence valid; compilation/setup error не acceptance RED.
- [ ] **8.2a Resolve shell-free/reproducible launch explicitly:** Dockerfile runtime — distroless/nonroot, нет `/bin/sh`, `getconf`/`ldd` нельзя предполагать. Standalone `tests/fixtures/memory_exec_barrier.rs` compiled pinned rustc1.99 GNUamd64, std-only: CLI `--gate /fixture/release --deadline-secs 60 -- /usr/local/bin/memory_mcp_http <args>`, ждёт explicit gate-file signal и `std::os::unix::process::CommandExt::exec` заменяет себя. Wrapper bind-mount read-only, gate dir readable nonroot; compiler/runtime ABI и wrapper hash проверены. Integration RED перед реализацией gate/exec: app не стартует до gate, PID/starttime сохраняется после release; unexpected process replacement/timeout FAIL. Новый helper не входит в application image/release и не добавляет зависимости. Glibc version проверять снаружи по extracted distroless libc package metadata/ELF+hash, не shell command внутри image. Docker FROM tags всех stages resolve до immutable digests и одинаково зафиксировать для parent/candidate builds; временная build recipe меняет только FROM references на captured digests, original Dockerfile в repo не править. Hash original/effective recipe и каждую stage identity сохранить. Не получать другой runtime security update только одной стороне; если exact production libc/rootfs недоступны — retention G4 OPEN.
- [ ] **8.3 Establish full-app RED control after fixture exists:** pinned shipped image на isolated synthetic2048 dataset, RAM3GiB для assertion failure. Отдельно source-parent image для causal attribution и comparable performance. Expected memory-limit assertion FAIL, сохранить каждый baseline report отдельно. Если baseline недоступен/не воспроизведён — G4 evidence OPEN, не ARM substitute. Запустить corrected candidate на том же RAM3GiB для4×comparison, затем на RAM1GiB для абсолютного gate. Legacy expected FAIL — evidence control, не permission считать его PASS. Все оба fullapp binaries production features, без test-fixtures/eval-support.
- [ ] **8.4 Exercise matrix workloads:** cold process до scheduler start, первый и повторный decay по completion signals; correctness fixture закрывает old fact и claims. RetrievalWorkload — отдельные cases,20warmup+100verified misses для cacheable query paths; no-cache dashboard/wake-up обозначить как repeated DB reads, новый request object не evidence miss. Каждому response — pinned IDs/temporal/policy assertions;100ANN samples без quality assertions недостаточны. ConcurrentMaintenance:2tenants×4requests с доказанным overlap. ErrorRelease: ожидаемые injected faults/cancellations имеют exact count/category, затем success и return to plateau; `unexpected_failed_requests=0`. Backfill/Reembed/InterruptedResume: реальные public operator flows Task7,205rows/3batches, ресурсные peaks во время rewrite и после resume/restart. No semantic workload для disabled — явно N/A; все nonsemantic workloads обязательны. LocalNative sampler измеряет весь process с model pages. Ordering barriers/signals, не sleeps.
- [ ] **8.5 Verify candidate GREEN:** `rtk cargo test -p memory_mcp --test http_storage_memory --features streamable-http,mcp-apps,eval-support,test-fixtures --locked -- --ignored --skip build_storage_memory_images --test-threads=1 --nocapture`. На local run harness может собрать candidate из execution worktree через pinned effective Dockerfile, затем фиксирует image ID/provenance. В CI отдельный prerequisite image-build job один раз строит parent/candidate/wrapper и публикует immutable OCI artifact с SHA256; resource jobs загружают и валидируют его, не повторяют длинный UI/Rust/C++ build перед часовым ColdDecay. Build entry point — `cargo test -p memory_mcp --test http_storage_memory --features streamable-http,mcp-apps,eval-support,test-fixtures --locked build_storage_memory_images -- --ignored --exact --nocapture` через cargo-only step, output dir из `STORAGE_MEMORY_IMAGE_BUNDLE`; общий measurement run пропускает build test через `--skip build_storage_memory_images`, а CI measurement jobs применяют exact case filter; measurement tests используют captured image manifest того же bundle. Три fresh runs каждого сценария проходят thresholds. Driver features eval-support/test-fixtures допустимы, app — только production features без fixture startup hooks. G2 отдельной полной командой Task1; macOS не закрывает gate.
- [ ] **8.6 Fail-closed validation tests:** controlled resource fixture cases `missing_samples_fail_the_run`, `arm_image_is_not_accepted_as_amd64_evidence`, `cache_hits_do_not_close_miss_gate`, `sampler_exit_before_decay_is_failure`, `application_error_is_not_a_successful_request`, `cgroup_without_peak_or_events_is_incomplete`. Вызывают test utility public `validate_http_memory_evidence(&Path)` на самостоятельных literal synthetic artifact inputs; `run_http_memory_case` обязан использовать тот же validator до возврата report. Для каждого case `assert!(validate_http_memory_evidence(dir.path()).is_err());`, а error category проверяется отдельно по конкретной нарушенной причине. Это валидация настоящих JSON outputs harness, не tests for markdown/file presence. Сначала наблюдать FAIL, потом минимальный guard; run `rtk cargo test -p memory_mcp --test http_storage_memory --features streamable-http,mcp-apps,eval-support,test-fixtures --locked` (guard tests не ignored).
- [ ] **8.6a Matrix/fault validation:** отдельно RED→GREEN для `disabled_mode_cannot_run_ann_case`, `wrong_resolved_width_invalidates_seed`, `unlisted_mode_does_not_silently_skip`, `unexpected_error_cannot_be_marked_as_injected`, `missing_required_case_is_incomplete`. Validator сравнивает actual executed case IDs с normative manifest, exact expected faults и provider/migration/index widths. `expected_fault_outcomes` в ordinary scenarios=0; ErrorRelease ждёт literals категорий и количества, не blanket allow all errors. LocalNative не использует remote RSS threshold и не может подставить fake provider.
- [ ] **8.6b Image/provider evidence guards:** RED→GREEN для `wrong_source_image_bundle_is_invalid`, `oci_hash_mismatch_is_invalid`, `instrumented_app_features_are_refused`. Disabled mode не требует работающей embedding модели: отдельный trap HTTP listener отвечает503 на любой вызов и ведёт request ledger; assert0requests и no ANN query. Это detection fixture, не рабочий provider и не fake retrieval response. Нельзя доказывать0calls только тем, что отсутствующий endpoint не записал запрос.
- [ ] **8.7 Integrate CI after local GREEN:** required nativeamd64 memory job, matrix `mode_dimension ×scenario_workload ×repetition[1,2,3]`, stableRemote384/768/1536/2048/3072 +disabledEmpty/disabledStored3072; transition cases отдельные rows. Один ColdDecay на job,120min; остальные cases не сериализовать всю matrix в один120min job. LocalNative отдельный required fixture-equipped job. Cargo filter должен исполнять ровно manifest expected case IDs; zero tests=FAIL через harness inventory. G2 отдельный cargo step. Не объединять perf с часовым cadence в одном job. Artifact upload always; no continue-on-error. Новые checks владелец repository делает required; до этого manual gate OPEN, branch protection через API этим task не менять.
- [ ] **8.8 Verify full memory matrix:** run8.5 +G2/G7; MEMORY_PROFILE содержит только actual measurements, missing cases явно OPEN. Это закрывает G4, но не G8/G9; final shipping review после Task9.
- [ ] **8.9 Commit:** explicit add two new test files, memory_exec_barrier.rs fixture, ci.yml и MEMORY_PROFILE; `rtk git commit -m "test(memory): gate storage amplification and HTTP memory plateau"`. Не выполнять production deployment.

## Task 9: Независимые performance/quality A/B gates и финальная приёмка

**Files:** новые tests/http_storage_performance.rs и расширение common/http_storage_memory.rs; eval-harness/tests/storage_release_gates.rs и common/storage_release_gates.rs; mode corpus из Task7; ci.yml, MEMORY_PROFILE. Existing `memory-perf` и `memory-eval` использовать без изменения их production schemas/algorithms. Новых dependencies/Cargo.toml changes нет; production crate не получает dependency на eval-harness.

**Interfaces:** Task8 fixture добавляет отдельный measurement entry, сохраняя memory API:

```rust
// memory-mcp/tests/common/http_storage_memory.rs
pub async fn run_http_performance_case(image: &str, case: &MemoryCase,
    output_dir: &std::path::Path) -> TestResult<()>;
// writes raw request timings/outcomes, cgroup CPU and comparable manifest;
// no counting allocator/DEBUG SQL logs. Release candidate/parent RAM3GiB/no-swap.

// eval-harness/tests/common/storage_release_gates.rs, test-only:
pub enum ReleaseGateVerdict {
    Passed, Regressed(Vec<String>), Invalid(Vec<String>),
}
pub fn compare_performance_pair_set(pair_set_dir: &std::path::Path)
    -> Result<ReleaseGateVerdict, Box<dyn std::error::Error + Send + Sync>>;
pub fn compare_quality_artifacts(baseline: &std::path::Path,
    candidate: &std::path::Path, release_profile: &std::path::Path)
    -> Result<ReleaseGateVerdict, Box<dyn std::error::Error + Send + Sync>>;
```

Pair-set schema/version, mode/workload/case inventory, manifests и raw evidence описаны spec и фиксируются literals в test fixtures; absolute file paths приходят через `STORAGE_RELEASE_PAIR_SET`, `STORAGE_RELEASE_BASELINE_QUALITY`, `STORAGE_RELEASE_CANDIDATE_QUALITY`. These env names — новые test-only inputs, не product config. Source parent ref/digest фиксирует Task1; image SHA нельзя объявить parent SHA по предположению. Test driver строит baseline/candidate **одинаково**, parent tree остаётся immutable; fixture/harness code подключается как внешний driver и не меняет parent application source. Validate TestResult errors не считаются Passed.

- [ ] **9.1 Write failing artifact guards before comparison code:** самостоятельно проверенные literal artifacts: `missing_baseline_is_invalid`, `wrong_model_or_dimension_is_invalid`, `different_host_or_cache_mode_is_invalid`, `insufficient_pairs_or_requests_is_invalid`, `latency_point_regression_is_rejected` (5ratios1.02), `throughput_point_regression_is_rejected` (5ratios0.99), `cpu_point_regression_is_rejected` (1.01), `wide_uncertainty_is_inconclusive`, `a_a_noise_above_three_percent_is_invalid`, `perfect_quality_cannot_hide_missing_cases`, `one_lost_required_fact_rejects_quality`, `quality_floor_pass_does_not_allow_baseline_regression` (baseline.95,candidate.94 при floor.9). Каждый guard — отдельный test и focal comparison. В тестах явно assert `Regressed` либо `Invalid` и именованную cause; просто любой Err недостаточен. Первоначальная minimal compiled fixture/parser даёт acceptance without relevant guard, так negative assertion должен FAIL; затем guard добавить минимально и проверить GREEN. Compilation/missing artifact setup не RED.
- [ ] **9.2 Run RED/GREEN per guard:** `rtk cargo test -p eval-harness --test storage_release_gates --locked <exact_case_name> -- --exact --nocapture` (подставляется реальное имя9.1). Исполнитель не пропускает запись каждой RED assertion. Minimal comparison body считает nearest-rank/latency по raw samples; использовать existing reducers только если они доступны через реальный public interface. Иначе test-only reducer в новом utility получает отдельные literal RED cases на percentile/units/directions; private visibility не расширять. Качественные ratio evidences читать из существующего eval artifact, не handwritten copy metric name into passing result. 10_000 run-block resamples, fixed seed20261013, units/metric directions и confidence limits из spec. Локальный deterministic resampling не требует новой dependency. Positive fixture со строго одинаковыми metrics проверяет Passed; error-rate/sample-count/coverage guards обязательны. Повторить `rtk cargo test -p eval-harness --test storage_release_gates --locked` после каждого guard/refactor.
- [ ] **9.3 Capture controlled PERFORMANCE A/A then A/B:** release optimized parent/candidate, same native host/kernel/libc/RAM3GiB/no-swap/CPU quota/base image/model/provider config, new DB fixture copy для каждого run. Передавать fixture composition/mode/phase из Tasks7–8, не подменять его native quality profile. 5пар A/Acalibration и5пар AB/BA; scheduleAB,BA,AB,BA,AB;30warmup+1000samples для long-lived HTTP/stdio retrieval workload, concurrency1/8 иwarm/miss отдельно. Startup/backfill/CLI reembed — отдельные5paired actions без искусственного1000request requirement; каждый action завершён и durably verified. Обязательные pathways: public storage conversion, lexical/temporal/ANN/context, wake-up/experience/dashboard, cold readiness, backfill/reembed facts/s, error recovery successful latency; HTTP tenant pool, local/remote/disabled stdio retrieval и public CLI startup/reembed composition. Disabled semantic workload и HTTP-only dashboard в stdio/CLI явно N/A, остальные applicable cases обязательны. ColdDecay часовой repeat не часть latency loop. Fixed offered rate —0.6×baseline calibration sustainable success rate, зафиксировать до candidate; closed-loop throughput отдельно. Не добавлять sampler/DEBUG verbosity/allocator overhead только одной стороне. App CPU/work unit, DB CPU/provider latency и очередь/inflight помогают отличить ускорение от внешнего bottleneck; ошибки нельзя удалить из denominator. Команда driver: `rtk cargo test -p memory_mcp --test http_storage_performance --features streamable-http,mcp-apps,test-fixtures,eval-support --locked -- --ignored --test-threads=1 --nocapture`. Baseline/candidate app images без test-fixtures/eval-support. Native local model total memory публиковать вместе с timings.
- [ ] **9.4 Gate real PERFORMANCE artifacts:** `rtk cargo test -p eval-harness --test storage_release_gates --locked performance_pair_set_meets_release_contract -- --ignored --exact --nocapture` с `STORAGE_RELEASE_PAIR_SET` pointing to captured artifact dir. Assertion `matches!(verdict, ReleaseGateVerdict::Passed)`; source ratio>1 lower-is-better или<1 higher-is-better FAIL, неопределённость/coverage OPEN. Existing memory-perf smoke без baseline с gates=[] не доказывает non-regression; weekly max_ratio1.25 также недостаточен для user requirement. Дополнительный existing smoke можно исполнить, но им нельзя закрыть G8. Native local in-process perf поддерживает только driver `in_process`; не выдумывать `memory-perf --driver http` или считать его HTTP acceptance.
- [ ] **9.5 Prepare verified NATIVE QUALITY models before A/B:** run exact pinned manifests, каждой cache hit проверить hash; source reviewed corpus frozen. На baseline иcandidate одинаковое env:

```bash
rtk cargo run -p eval-harness --bin memory-eval --release --locked -- prepare-model --manifest evals/models/multilingual-e5-small.json --output-root target/eval-models
rtk cargo run -p eval-harness --bin memory-eval --release --locked -- prepare-model --manifest evals/models/mmarco-minilm.json --output-root target/eval-models
```

Explicit process env: `EMBEDDINGS_ENABLED=true`, `EMBEDDINGS_PROVIDER=local-candle`, `EMBEDDINGS_MODEL=intfloat/multilingual-e5-small`, `EMBEDDINGS_MODEL_DIR=target/eval-models/intfloat--multilingual-e5-small/614241f622f53c4eeff9890bdc4f31cfecc418b3`, `RETRIEVAL_RERANKER_MODEL_DIR=target/eval-models/cross-encoder--mmarco-mMiniLMv2-L12-H384-v1/1427fd652930e4ba29e8149678df786c240d8825`, `RETRIEVAL_RERANKER_ADMISSION_FLOOR=0`, `MEMORY_CLAIM_ROLLOUT_STAGE=evidence`. Ровно resolved native dimension384; случайный `SURREALDB_EMBEDDING_DIMENSION=2048` из другого fixture env удаляется. Эти env передавать детям явно, process-global env тестов не менять параллельно. Pinned files/weights не сокращать для memory PASS.
- [ ] **9.6 Capture full NATIVE QUALITY baseline/candidate:** сначала parent run, затем candidate run, в их respective immutable worktrees и тех же model roots:

```bash
# parent worktree; artifact path resolves there; copy immutable artifact to A/B dir
rtk cargo run -p eval-harness --bin memory-eval --release --locked -- run --profile evals/profiles/release.json --artifact target/evals/storage-parent-release.json --baseline evals/baselines/one-active-namespace-release.json
# candidate worktree; existing committed gate additionally enforced
rtk cargo run -p eval-harness --bin memory-eval --release --locked -- run --profile evals/profiles/release.json --artifact target/evals/storage-candidate-release.json --baseline evals/baselines/one-active-namespace-release.json
rtk cargo test -p eval-harness --test storage_release_gates --locked native_quality_has_zero_regression -- --ignored --exact --nocapture
```

Последний command получает explicit env artifact paths и проверяет свежий parent↔candidate плюс pinned profile. Six retrieval-quality metrics=1.0,37executed cases; full profile currently154. Actual expected IDs сверять с pinned profile/corpus, не hardcode154 поверх будущего changed manifest. Quality metric .94 не PASS при baseline.95 даже если existing floor/budget разрешает. Unexpected extraction noise/unknown labels/provider failure — OPEN до объяснения/валидного comparable run; не менять gold/model threshold.
- [ ] **9.7 Gate MODE QUALITY through real transport:** Task7 corpus и actual response artifacts для stable remote384/768/1536/2048/3072, disabledEmpty/disabledStored, each transition phase before/during/after/restart; localNative; reranker on/off where supported. Recall@5/MRR/nDCG@5, required source keys/order, unknown/protected/duplicate rates не ниже same-mode source parent, per-case required fact не теряется даже при flat aggregate. D17 только structural plumbing, не model quality claim. Public CLI/HTTP outcomes закрывают G9 transport layer, native eval закрывает model/source corpus layer. Если реальный staging provider недоступен, synthetic matrix закрывает только plumbing; final real-provider staging gate OPEN. Ускорение/меньшая память не компенсируют качество.
- [ ] **9.8 CI и release evidence:** отдельные required checks `storage-memory`, `storage-performance`, `storage-native-quality`, mode coverage published; `if: always()` artifacts с exact request samples/case outcomes/digests/env-sanitized manifests/command exits; no skip/continue-on-error. Fixture-equipped native runner обязателен для local models, Linuxamd64 для retention. Strict full matrix можно shard, но manual/required aggregate check проверяет **все** case IDs/repetitions; частичный PR smoke не release PASS. Existing check lists сохранить; new run steps cargo-only. Existing Make targets не объявлять заменой resource/quality gates. Complete suite/clippy/fmt/toolchain/observability commands ниже; independent whole-branch review без openP0/P1 и source diff показывает только intended changes. MEMORY_PROFILE таблицы memory×latency/CPU/throughput×quality по mode/phase, без цифр ещё не выполненного harness.
- [ ] **9.8a Runner/time budget:** perf pair-set целиком на одном exclusive native host; нельзя собрать5pairs с разных ephemeral hosts под одинаковым host-tag. До запуска capture host fingerprint/CPU quota, fixture pilot time; deadline1800s на steady perf run, full pair-set deadline и runner reservation задаются заранее по expected case inventory, не после увиденного FAIL. Если mode/workload shard превышает CI max job duration, разделить запуск на jobs на том же reserved physical runner с теми же conditions и immutable artifact import в required aggregate check; не сокращать repetitions/1000samples. Native release budget1200s прежний; memory hourly jobs120min отдельно. Превышение deadline — OPEN/invalid, не пропуск samples. Cleanup10s; no orphan containers/processes/model-provider tasks. CPU/request гейт применяется к app и DB раздельно; перенос нагрузки в DB не засчитывается как free improvement. Переменные log verbosity/NER/backend/fs-watch/cache/background cadence совпадают в A/B; отключать production components для PASS запрещено.
- [ ] **9.9 Commit:** explicit add new performance/acceptance test targets и test utilities, touched CI/docs, fixtures только если действительно changed; `rtk git commit -m "test(perf): require memory gains without speed or quality regressions"`. Final rollout handoff предъявляет G0–G9 results и staging A/B; deployment не входит в текущую авторизацию.

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
rtk cargo test -p memory_mcp --test http_storage_memory --features streamable-http,mcp-apps,eval-support,test-fixtures --locked -- --ignored --skip build_storage_memory_images --test-threads=1 --nocapture
rtk cargo test -p memory_mcp --test embedding_memory_modes --features streamable-http,test-fixtures,eval-support --locked -- --test-threads=1
rtk cargo test -p memory_mcp --test http_storage_performance --features streamable-http,mcp-apps,eval-support,test-fixtures --locked -- --ignored --test-threads=1 --nocapture
rtk cargo test -p eval-harness --test storage_release_gates --locked
# Real artifact tests require captured immutable paths in env per Task9:
rtk cargo test -p eval-harness --test storage_release_gates --locked -- --ignored --test-threads=1 --nocapture
rtk git diff --check
```

PASS: каждый command exit0; expected tests executed, mandatory case-ID matrix полностью закрыта, не только ignored summary; ноль fmt diff/clippy warnings; все G0–G9 evidence целостны. Native model prepare/release eval команды Task9 тоже обязательны. Existing ignored tests общей suite перечислить с причиной; resource/real-artifact cases запускать `--ignored` отдельно. Zero matching tests, unsupported runner/provider либо отсутствующие artifacts оставляют gate OPEN, ветка не готова к shipping.

### Evidence package на каждый A/B

Сохранить под отдельным `work/memory-materialization/<run-id>/` (не credentials), CI artifacts и малые таблицы/immutable links в MEMORY_PROFILE. Manifest: source-parent SHA и candidate SHA/dirty/build inputs, separate shipped baseline digest, lock SHA256, rustc/features/release flags, app image ID/runtime-base digest/binary hash, Surreal image/version, measured kernel/arch/libc, CPU/RAM/swap/cache/log config, fixture version/digest, provider/model revision/hash/signature/configured-resolved-stored dimensions/index width, scenario/workload/phase/repetition/case inventory, samples/outcomes/UTC/commands/exits. Immutable snapshots для resume phases; no credentials/raw prod content. Mutable tags только для первоначального pull, затем captured IDs. Native models/hash verification и real staging provider evidence labels обязательны. Разные fixtures/platforms/configs не causal A/B.

Перед rollout review: legacy memory FAIL/candidate PASS и4×factor; source-parent/candidate performance pairs с interval evidence; raw complete-request/DB-query/outcome counts; durable count/close/namespace/dimension/index/cursor checks; native/mode quality results с0regression; full suite/CI IDs и final review. Missing mandatory case оставляет соответствующий gate OPEN. Rollback criteria включают memory/OOM/swap, latency/throughput/CPU, error rate и quality/visibility/index-state regression. Rollout только по отдельной авторизации.

## Self-review плана

- Spec coverage: conversion/extraction Tasks1–2; projections Tasks3–7; type/error/identity Tasks1–2; lifecycle/access/retrieval Tasks3–6; modes/dimensions/backfill/reembed Tasks6–7; full memory/CLI/model/cold/concurrency/error matrix Task8; performance/quality/evidence/release Task9. Pagination/admission/allocator — отдельный scope.
- Step scan: каждый production edit имеет preceding RED и narrow GREEN command; setup привязан к измеряемому deliverable, нет самостоятельного scaffolding task. Длинные acceptance tasks делятся на named scenarios, а не один test с conditional assertions.
- Type consistency: DbClient signatures прежние; converter Vec<Value>; decay limiti32; reembed public API включает CancellationToken; MemoryCase передаёт composition/mode/transition/workload/repetition, manifest — phase; unexpected failures отделены от exact injected outcomes; reports/units/guards совпадают.
- Review Focus: все пять рисков имеют owning tests; обязательный memory FAIL нельзя заменить зелёными leaf characterizations.
- Proportion: plan фиксирует интерфейсы, literals, команды и gate decisions; implementation bodies оставлены исполнителю. Никакого experimental converter implementation в документе.
- Повторное review2026-10-13: исправлены default1536 в remote fixture, harness RED ordering, ErrorRelease zero-error contradiction, cache-object assumption, отсутствие quality/performance gates, hardcoded2048, reembed read amplification и semantic default0 при threshold0/-1. Финальная уверенность ограничена ещё не исполненными tests/native/staging measurements; plan не означает G0–G9 PASS.

## Handoff

План и spec требуют review до implementation. Рекомендация — Subagent-driven: общая conversion граница имеет широкий compatibility impact, а новые Linux gates требуют независимого review измерений. Native тоже допустим с whole-branch fresh review после всех гейтов. Пользователь пока запросил только план; ни один task implementation не начат.
