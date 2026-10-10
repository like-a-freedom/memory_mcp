# Perf Harness (Core) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Добавить в приватный crate `eval-harness` performance-поверхность: профиль-управляемые прогоны in-process сценариев, которые измеряют латентность и throughput ключевых операций (`ingest`, `extract`, `assemble_context`, `explain`, `resolve`, `invalidate`), время и классы каждого SQL-запроса, потребление RSS и дискового I/O процесса, и складывают результат в коммитимый артефакт `memory-mcp-perf/v1` с вердиктом `passed / regressed / invalid`.

**Architecture:** Новый модуль `crates/eval-harness/src/perf/` с чистым ядром (профиль → сценарии → редьюсер формул → артефакт → вердикт) и тонким драйвером in-process, который конструирует реальный `MemoryService` поверх embedded SurrealDB (`mem://`) по образцу `src/test_support.rs`. Наблюдение за SQL — prod-хук `QueryObserver` под фичей `eval-support` в единственном SQL-воронке `storage/client.rs`. Системные ресурсы (RSS, peak, disk read/write bytes, io ops) собирает `ResourceSampler` (`/proc` на Linux, `proc_pid_rusage`/`task_info` на macOS). Формулы производных метрик живут только в `perf/reducer.rs` (аналог ADR-0025 для perf).

**Tech Stack:** Rust 1.99.0, tokio, serde/serde_json, clap 4 (workspace), sha2 + hex (уже зависимости `eval-harness`), `libc` (единственная новая зависимость, target-gated на macOS — требует подтверждения мейнтейнером, см. Global Constraints).

**Spec:** `docs/superpowers/specs/2026-10-10-perf-evaluation-harness-design.md` — создаётся на Task 1 из дизайна, согласованного в переписке (разделы: Drivers / Sampling layer / SQL observer / Metric catalog / Profiles / Gates). ADR: `docs/adr/0082-perf-evaluation-surface-in-eval-harness.md`.

## Global Constraints

- `rust-version = 1.99.0` (workspace); все команды CI — только `cargo` (ADR-0064), Makefile — тонкий адаптер.
- Одиннаправленная зависимость: `eval-harness → memory_mcp` (ADR-0021); прод никогда не зависит от eval-harness. Новый MCP-tool не добавляется (8-tool surface заморожен).
- Единственная новая зависимость за весь план: `libc` в `crates/eval-harness/Cargo.toml` в секции `[target.'cfg(target_os = "macos")'.dependencies]` (workspace-версию не объявлять, `libc = "0.2"`). **Требует явного подтверждения мейнтейнером перед Task 5** (правило AGENTS.md «Ask before changing dependencies in Cargo.toml»). Если подтверждение не получено — macOS-сэмплер возвращает `None` для disk-полей, RSS через `ps -o rss= -p <pid>` (`std::process::Command`, без новой зависимости), и это фиксируется в ADR-0082 как деградация.
- Семантика вердиктов и exit-кодов как в quality-раннере: артефакт пишется всегда, а exit-код определяется вердиктом: passed→0, regressed→1, invalid→2. Invalid ≠ pass: пустые выборки, отсутствующий sampler там, где профиль его требует, mismatch хоста/схемы — всегда `invalid`, никогда оптимистичный default (truthful-eval §5).
- `perf/reducer.rs` — единственный дом арифметики и имён perf-метрик. Ни один scenario/artifact модуль не содержит own percentile/ratio math (перенос ADR-0025).
- Closed vocabulary везде: `ScenarioId`, `QueryClass`, gate-метрики — конечные перечисления; ничего производного от данных запроса/tenant/id не попадает в ключи артефакта без bounded-хэша (ADR-0048 принцип bounded labels).
- Perf-гейты абсолютных значений только на pinned host: сравнение артефакта с baseline при `host_tag != baseline.host_tag` → `invalid` (ADR-0034: PR-раннеры — только gross-таймаут и integrity).
- Никакого `unwrap()` в проде; `MemoryError` либо новый `PerfError` (thiserror) в eval-harness.
- Перед завершением каждого таска: `cargo clippy -p eval-harness --all-targets --locked -- -D warnings` (для Task 6 — `-p memory_mcp --features eval-support`) и `cargo fmt --all --check`. Компмит в конце каждого таска.
- Тесты по AGENTS.md: минимальный набор (`cargo test -p eval-harness <filter>`), поведение и контракты, не наличие файлов/документов.

## Review Focus

(Классы входов, которые дизайн подразумевает, но обычные happy-path тесты их не ловят; каждый закрывается тестом в указанном таске.)

1. **Неизвестная/будущая schema-версия** профиля или baseline-артефакта → явная ошибка и exit 2, без «попробуем как v1» (Task 2, Task 10).
2. **Пустой набор выборок** (все итерации ушли в warm-up / сценарий упал) → reducer возвращает `None`, вердикт `invalid`; ноль вместо данных пишется никогда (Task 3, Task 9).
3. **Аномальный SQL**: многострочный, длиннющие литералы, unicode, несколько точек расширения ключа → нормализация обязана остаться ограниченной: distinct-шаблонов ≤ 64, сверх — слияние в `(overflow)`; key-space артефакта не растёт с данными (Task 7).
4. **Sampler недоступен на платформе** → `ResourceSnapshot` полится `None`-полями, сценарий получает invalid-reason, фактические нули в артефакт не пишутся (Task 5, Task 9).
5. **Dedup-заражение замеров**: повторный ingest одинакового `source_id` уходит в dedup-путь, и «warm latency» меряет не steady-state → сценарии генерируют уникальные ids; тест проверяет рост числа эпизодов = числу вызовов (Task 8).

## Scope

Этот план — фаза 1+2 ядра (in-process драйвер). **Не входит (follow-up план 2):** stdio/HTTP subprocess-драйверы и scrape `/metrics`, расширение прод-словаря `DECLARED_STAGES`/`StageTimer`, contention-сценарии, детерминированный embedding-provider, RocksDB-внутренности (block cache/compaction).

## File Structure

Создаваемые:

| Файл | Ответственность |
|---|---|
| `crates/eval-harness/src/perf.rs` | корень модуля, re-export публичного API (`pub mod perf;` в `src/lib.rs`) |
| `crates/eval-harness/src/perf/domain.rs` | общие типы: `HostFingerprint`, `ResourceSnapshot`, `ScenarioEvidence`, `SqlTemplateRecord`, константы схем |
| `crates/eval-harness/src/perf/profile.rs` | загрузка/валидация `evals/perf/profiles/*.json`, closed-vocab `ScenarioSpec` |
| `crates/eval-harness/src/perf/reducer.rs` | единственные формулы: `LatencyStats`, percentiles, throughput, scaling exponent, ratio |
| `crates/eval-harness/src/perf/artifact.rs` | `PerfArtifact` `memory-mcp-perf/v1`: сериализация, чтение, digest |
| `crates/eval-harness/src/perf/verdict.rs` | сравнение с baseline, regression budgets, exit-коды |
| `crates/eval-harness/src/perf/sampler.rs` | `ResourceSampler` trait + Linux/macOS реализации, pure-парсеры |
| `crates/eval-harness/src/perf/db_observer.rs` | harness-side `QueryObserver`: нормализация SQL, `QueryClass`, `QueryLedger` |
| `crates/eval-harness/src/perf/scenarios.rs` | `Scenario` trait, закрытый реестр сценариев, in-process исполнение |
| `crates/eval-harness/src/perf/runner.rs` | `run_profile`: warmup/итерации/budget guard → артефакт + вердикт |
| `crates/eval-harness/src/perf_main.rs` | `[[bin]] memory-perf` (clap: `run`) |
| `crates/memory-mcp/tests/eval_query_observer.rs` | интеграционный тест прод-хука (feature `eval-support`) |
| `evals/perf/profiles/perf-pr-smoke.json`, `perf-weekly.json` | профили |
| `docs/adr/0082-perf-evaluation-surface-in-eval-harness.md`, `docs/superpowers/specs/2026-10-10-perf-evaluation-harness-design.md` | документация (Task 1), baseline-runbook (Task 12) |

Модифицируемые: `crates/eval-harness/Cargo.toml` (`[[bin]]`, `libc` mac-only), `crates/eval-harness/src/lib.rs` (`pub mod perf;`), `crates/memory-mcp/src/storage/client.rs` (observer-хук), `crates/memory-mcp/src/eval_support.rs` (re-export порта), `Makefile`, `.github/workflows/ci.yml`, `.github/workflows/evaluations.yml`, `docs/evals/README.md`.

---

### Task 1: Документация (ADR-0082 + spec) и скелет модуля

**Files:**
- Create: `docs/adr/0082-perf-evaluation-surface-in-eval-harness.md`
- Create: `docs/superpowers/specs/2026-10-10-perf-evaluation-harness-design.md`
- Create: `crates/eval-harness/src/perf.rs`, `perf/domain.rs`, `perf/profile.rs`, `perf/reducer.rs`, `perf/artifact.rs`, `perf/verdict.rs`, `perf/sampler.rs`, `perf/db_observer.rs`, `perf/scenarios.rs`, `perf/runner.rs`
- Modify: `crates/eval-harness/src/lib.rs` (добавить `pub mod perf;`)

**Interfaces:**
- Produces: пустые (только `// TODO(Task N)` + `pub use`) модули, компилируемые дерево; спецификация с зафиксированными константами схем.

- [ ] **Step 1:** Перенести согласованный дизайн в spec-файл (структура из переписки: Drivers / Sampling layer / SQL observer / Metric catalog / Profiles / CI / Risks). В ADR-0082 зафиксировать: отдельный бинарь `memory-perf`; schema-строки `memory-mcp-perf-profile/v1` и `memory-mcp-perf/v1`; право scenario-runner'а быть не-Criterion (Criterion остаётся для микро-бенчей моделей, ADR-0034 переживается целенаправленно); pinned-host политика гейтов; observer как eval-only seam, наблюдающий только успешные исполнения (`server_version` вне воронки); решение по `libc`.
- [ ] **Step 2:** Создать файлы модуля с одним содержимым: `perf.rs` объявляет `mod domain; mod profile; ...; ` c заглушками `// filled in Task N`, `domain.rs` содержит две константы:
```rust
pub const PROFILE_SCHEMA: &str = "memory-mcp-perf-profile/v1";
pub const ARTIFACT_SCHEMA: &str = "memory-mcp-perf/v1";
```
- [ ] **Step 3:** `cargo check -p eval-harness` — PASS.
- [ ] **Step 4:** Commit: `git add docs crates/eval-harness && git commit -m "docs+perf: ADR-0082, perf spec, module skeleton"`.

### Task 2: Профиль — загрузка и валидация

**Files:**
- Modify: `crates/eval-harness/src/perf/profile.rs`
- Test: модульные `#[cfg(test)]` в `perf/profile.rs`

**Interfaces:**
- Consumes: `perf::domain::PROFILE_SCHEMA`.
- Produces:
```rust
pub struct PerfProfile { pub name: String, pub budget_seconds: u64,
    pub host_tag: String, pub env: BTreeMap<String, String>,
    pub scenarios: Vec<ScenarioSpec>, pub gates: Vec<GateSpec> }
pub struct ScenarioSpec { pub id: ScenarioId, pub driver: DriverKind,
    pub scales: Vec<u32>, pub iterations: u32, pub warmup: u32, pub queries: Option<u32> }
pub struct GateSpec { pub scenario: ScenarioId, pub metric: GateMetric,
    pub max_ratio: f64, pub min_samples: u32,
    #[serde(default)] pub direction: GateDirection } // default = LowerIsBetter
pub enum GateDirection { LowerIsBetter, HigherIsBetter } // serde: "lower_is_better"|"higher_is_better"
pub enum ScenarioId { IngestScale, AssembleScale, LatencyExtract,
    LatencyResolve, LatencyExplain, LatencyInvalidate } // as_str(): kebab-case
pub enum DriverKind { InProcess, Stdio, Http } // serde: "in_process"|"stdio"|"http";
// load() отклоняет любой != InProcess → PerfError::UnsupportedDriver(<имя>)
pub enum GateMetric { P50Us, P95Us, P99Us, MeanUs, ThroughputEps }
pub fn load(path: &std::path::Path) -> Result<PerfProfile, PerfError>;
```
`PerfError` — новый thiserror-enum в `crates/eval-harness/src/perf.rs` (`#[from] serde_json::Error`, `Schema`, `UnsupportedDriver(String)`, `Validation(String)`).

- [ ] **Step 1:** Написать падающие тесты: (a) happy-path inline-JSON проходит и поля прочитаны; (b) `"schema": "memory-mcp-perf-profile/v2"` → `PerfError::Schema`; (c) `"driver": "stdio"` → `UnsupportedDriver("stdio")`; (d) `budget_seconds: 0` и немонотонный/пустой `scales` → `Validation`; (e) неизвестный `scenario.id` → `Validation`; (f) gate без поля `direction` → `LowerIsBetter` (serde default).
- [ ] **Step 2:** `cargo test -p eval-harness profile::tests` — FAIL.
- [ ] **Step 3:** Реализовать serde-структуры + `load` (ручная валидация после десериализации).
- [ ] **Step 4:** Тесты PASS; clippy/fmt.
- [ ] **Step 5:** Commit `feat(perf): profile schema-v1 loader with closed vocabulary`.

### Task 3: Reducer — единственный дом формул

**Files:**
- Modify: `crates/eval-harness/src/perf/reducer.rs`
- Test: модульные тесты в том же файле

**Interfaces:**
- Produces:
```rust
pub struct LatencyStats { pub count: u32, pub p50_us: u64, pub p90_us: u64,
    pub p95_us: u64, pub p99_us: u64, pub mean_us: u64, pub total_us: u64 }
pub fn latency_stats(samples: &[u64]) -> Option<LatencyStats>; // пустой срез → None
pub fn throughput_eps(total_us: u64, episodes: u32) -> Option<f64>; // total_us==0 → None
pub fn scaling_exponent(points: &[(f64, f64)]) -> Option<f64>; // log2-log2 least-squares, <3 точек → None
pub fn ratio(current: f64, baseline: f64) -> Option<f64>; // baseline<=0 → None
```
Percentile — nearest-rank (`ceil(p*n)` индекс 1-based). Имена ключей: `pub fn latency_key(scenario: &str, stat: GateMetric) -> String` → `"{scenario}.{metric}"`, `pub fn sql_key(class: &str) -> String` → `"sql.{class}"`.

- [ ] **Step 1:** Падающие тесты: срез `1..=100` → p95==95 (nearest-rank), count/mean/total точны; пустой срез → `None`; один элемент → p50==p99==значение; `throughput_eps(0, 5)` → `None`; exponent `[(1.0,10.0),(2.0,40.0),(3.0,160.0)]` → `Some(2.0)` (±1e-9); `ratio(2.0, 0.0)` → `None`.
- [ ] **Step 2:** `cargo test -p eval-harness perf::reducer` — FAIL.
- [ ] **Step 3:** Реализовать.
- [ ] **Step 4:** PASS; clippy/fmt.
- [ ] **Step 5:** Commit `feat(perf): reducer as the single formula home`.

### Task 4: Артефакт `memory-mcp-perf/v1`

**Files:**
- Modify: `crates/eval-harness/src/perf/artifact.rs`, `perf/domain.rs`
- Test: модульные тесты

**Interfaces:**
- Consumes: `LatencyStats`, `ScenarioId`, `PROFILE_SCHEMA`.
- Produces:
```rust
pub struct HostFingerprint { pub host_tag: String, pub os: String, pub arch: String }
pub struct ResourceSnapshot { pub rss_bytes: Option<u64>, pub peak_rss_bytes: Option<u64>,
    pub disk_read_bytes: Option<u64>, pub disk_write_bytes: Option<u64>,
    pub io_read_ops: Option<u64>, pub io_write_ops: Option<u64> }
pub struct SqlTemplateRecord { pub template_hash: String, pub op: String, pub class: String,
    pub count: u32, pub stats: LatencyStats }
pub struct ScenarioRecord { pub latencies: Option<LatencyStats>, pub throughput_eps: Option<f64>,
    pub samples_raw: Vec<u64>, pub sql: Vec<SqlTemplateRecord>,
    pub resources: Option<ResourceSnapshot>, pub invalid_reasons: Vec<String> }
pub struct PerfArtifact { pub schema: String /*ARTIFACT_SCHEMA*/, pub profile: String,
    pub profile_digest: String /*sha256 hex bytes of profile file*/, pub commit: String,
    pub host: HostFingerprint, pub started_at: String, pub duration_ms: u64,
    pub scenarios: BTreeMap<String, ScenarioRecord> }
pub fn write(artifact: &PerfArtifact, path: &Path) -> Result<(), PerfError>;
pub fn read(path: &Path) -> Result<PerfArtifact, PerfError>; // schema != CONSTANT → PerfError::Schema
```

- [ ] **Step 1:** Падающие тесты: round-trip через `tempfile` (все поля сохранены побайтово-стабильно: два `write` одного артефакта → одинаковые байты, ключи отсортированы через BTreeMap); `read` файла с `schema:"memory-mcp-perf/v2"` → `Schema`; digest = sha256 от фиксированных байт (ожидание зашито hex-константой).
- [ ] **Step 2:** FAIL-прогон.
- [ ] **Step 3:** Реализовать (serde `rename_all = "camelCase"`, индентация 2 пробела).
- [ ] **Step 4:** PASS; clippy/fmt.
- [ ] **Step 5:** Commit `feat(perf): artifact schema-v1 with digest and strict read`.

### Task 5: ResourceSampler (Linux /proc, macOS via libc)

**Files:**
- Modify: `crates/eval-harness/src/perf/sampler.rs`, `crates/eval-harness/Cargo.toml`
- Test: модульные (парсеры), интеграционный macos-only live-тест

**Interfaces:**
- Consumes: `domain::ResourceSnapshot`.
- Produces:
```rust
pub trait ResourceSampler: Send + Sync { fn sample(&self) -> Option<ResourceSnapshot>; }
pub fn self_sampler() -> Box<dyn ResourceSampler>; // linux|macos|unsupported(→всегда None)
pub fn parse_proc_status(input: &str) -> Result<(u64, u64, u64), String>; // (VmRSS_kB, VmSwap_kB, VmHWM_kB)
pub fn parse_proc_io(input: &str) -> Result<(u64, u64, u64, u64), String>; // read_bytes,write_bytes,syscr,syscw
```
Порт `parse_proc_status` по образцу `crates/xtask/src/memory.rs:20` (kB-юниты сохранить как есть, байты считает вызывающий). macOS: `libc::proc_pid_rusage(self_pid, RUSAGE_INFO_V2)` → `phys_footprint` (rss_bytes), `ri_resident_size` (peak), `ri_diskio_bytesread/written` (disk-поля); `io_*_ops` = `None` на macOS (ограничение — в ADR-0082). Linux: `/proc/self/status`, `/proc/self/io`.

- [ ] **Step 1:** Падающие тесты парсеров на fixture-строках (status из xtask-теста; io: `"read_bytes: 4096\nwrite_bytes: 8192\nsyscr: 7\nsyscw: 9\n..."`).
- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** Добавить в `crates/eval-harness/Cargo.toml`:
```toml
[target.'cfg(target_os = "macos")'.dependencies]
libc = "0.2"
```
(**Стоп-точка:** подтвердить у мейнтейнера до коммита; иначе — fallback-режим из Global Constraints.)
- [ ] **Step 4:** Реализовать парсеры и обе платформы; `self_sampler()` ветвится `#[cfg]`.
- [ ] **Step 5:** PASS парсеров.
- [ ] **Step 6:** Live-тест `#[cfg(target_os = "macos")] async fn mac_sampler_reports_disk_writes`: записать 1 MiB в tempfile c fsync, сэмпл до/после, `disk_write_bytes` delta `Some(>0)`; ассерт RSS `Some(>1MiB)`. На Linux аналог через `/proc/self/io` c аналогичными границами; при `None` сэмплере тест обязан fail, не skip.
- [ ] **Step 7:** PASS; clippy/fmt; commit `feat(perf): resource sampler for linux and macos`.

### Task 6: Прод-хук `QueryObserver` под `eval-support` (memory_mcp)

**Files:**
- Modify: `crates/memory-mcp/src/storage/client.rs` (поле OnceLock + setter + вызов в воронке), `crates/memory-mcp/src/eval_support.rs` (re-export)
- Test: `crates/memory-mcp/tests/eval_query_observer.rs`

**Interfaces:**
- Produces (все items `#[cfg(feature = "eval-support")]`):
```rust
pub struct SqlObservation { pub op: &'static str, pub sql: String, pub micros: u64 }
pub trait QueryObserver: Send + Sync + 'static { fn observe(&self, obs: &SqlObservation); }
impl SurrealDbClient { pub fn set_eval_query_observer(&self, obs: Arc<dyn QueryObserver>); }
```
Хук называется ровно в двух точках, где уже измеряется длительность: на `.done-событии приватных обёрток `execute_query` и `execute_raw_query` (весь SQL: `query`/`create`/`update`/`select_*`/миграции через `execute_migration_script` проходит только через них; `server_version` — вне воронки, осознанно). Источник микросекунд — существующий `timer.elapsed()`. В v1 наблюдаются только успешные исполнения (ошибки не попадают в ledger — фиксируется в ADR-0082); если observer не установлен — ровно один OnceLock-read, поведение не меняется. Поле `OnceLock<Arc<dyn QueryObserver>>` добавляется в `SurrealDbClient` под `#[cfg(feature = "eval-support")]` и инициализируется во всех конструкторах connect. В `eval_support.rs` — `pub use crate::storage::client::{QueryObserver, SqlObservation};`.

- [ ] **Step 1:** Интеграционный тест `crates/memory-mcp/tests/eval_query_observer.rs` (первая строка файла `#![cfg(feature = "eval-support")]`, чтобы обычный `cargo test -p memory_mcp` компилировал пустой файл): `SurrealDbClient::connect_in_memory("obs_test", "main", "warn")` (порядок аргументов: database, namespace, log_level) → `set_eval_query_observer(Arc<RecordingObserver>)` → `DbClient::query(&client, "SELECT 1 AS one", None, "main")` → ровно 1 запись, `micros > 0`, `op` непустая (`"db.execute_query"`), в записи нет `request_id`/данных (ADR-0005: наблюдение без unbounded-меток). Второй тест: setter вызван дважды → повторный вызов — no-op (OnceLock::set возвращает Err, setter его проглатывает), обе команды прошли.
- [ ] **Step 2:** `cargo test -p memory_mcp --features eval-support --test eval_query_observer` — FAIL (items не существуют).
- [ ] **Step 3:** Реализовать минимальный хук.
- [ ] **Step 4:** PASS; `cargo clippy -p memory_mcp --features eval-support --all-targets --locked -- -D warnings`.
- [ ] **Step 5:** Commit `feat(eval-support): sql query observer hook on the storage funnel`.

### Task 7: DB-обсертор harness'а — нормализация, классы, ledger

**Files:**
- Modify: `crates/eval-harness/src/perf/db_observer.rs`
- Test: модульные тесты

**Interfaces:**
- Consumes: `memory_mcp::eval_support::QueryObserver` (Task 6), `reducer::latency_stats`.
- Produces:
```rust
pub enum QueryClass { EpisodeWrite, FactWrite, EntityUpsert, EdgeProbe, RelateEdge,
    AnnSearch, AliasLookup, EdgeNeighbors, ActiveEdgeScan, CommunityScan,
    AccessLogWrite, ResolveLookup, ProvenanceRead, Migration, Other }
pub const MAX_TEMPLATES: usize = 64;
pub struct QueryLedger { /* MutexInner { templates: BTreeMap<hash,TplAcc>, overflow: TplAcc } */ }
impl QueryLedger { pub fn new() -> Arc<Self>; pub fn snapshot(&self) -> Vec<SqlTemplateRecord>; }
impl QueryObserver for Arc<QueryLedger> { /* observe */ }
pub fn normalize_sql(sql: &str) -> (String /*template*/, String /*sha256 hex[:16]*/);
pub fn classify(op: &str, sql: &str) -> QueryClass;
```
`normalize_sql`: string-литералы → `?`, hex-последовательности из 8+ символов (record id вида `episode:<sha256>`) → `#`, оставшиеся последовательности цифр → `#`, последовательности whitespace → один пробел, trim — порядок: литералы до hex (цифры внутри литералов не должны ломать template), hex до цифр (content-addressed id не должны фрагментировать template). Классификация — по op + ключевым маркерам SQL в `lowercased template` (ANN: `vector::similarity::cosine` + `embedding` — фактическая форма из `build_select_facts_ann_query`; alias: `"containsany"`; RELATE-маркер; `SELECT ... FROM edge` neighbors и т.п. — таблица правил константой `RULES: &[(class, markers)]` в начале файла).

- [ ] **Step 1:** Падающие тесты: (a) нормализация двух запросов с разными литералами даёт одинаковый template/hash; (b) литерал со знаком `#`/цифрами внутри не меняет template; (c) classify fixture'ами: ANN-запрос строится подстановкой маркеров из фактической формы `build_select_facts_ann_query`, alias — `CONTAINSANY [$alias]`, RELATE и т.д. — таблица из ≥6 строк; (d) 65 distinct template'ов → 64 записи + 1 `(overflow)` с `count=1` (Review Focus #3); (e) observe из двух потоков → ledger содержит оба наблюдения (count=2).
- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** Реализовать.
- [ ] **Step 4:** PASS; clippy/fmt.
- [ ] **Step 5:** Commit `feat(perf): sql template reducer with bounded class vocabulary`.

### Task 8: Сценарии и in-process исполнение

**Files:**
- Modify: `crates/eval-harness/src/perf/scenarios.rs`
- Test: интеграционный `crates/eval-harness/tests/perf_scenarios.rs`

**Interfaces:**
- Consumes: `test_support::TestMemory` (паттерн конструирования `MemoryService` + `connect_in_memory` + миграции), Task 6 observer setter, Task 7 ledger, `reducer`, `IngestCapability::ingest_from_service` (паттерн `test_support::ingest_probe`).
- Produces:
```rust
pub struct ScenarioEvidence { pub scenario: ScenarioId, pub scale: u32,
    pub latencies_us: Vec<u64>, pub invalid_reasons: Vec<String> }
pub trait ScenarioRun { fn id(&self) -> ScenarioId; fn scale(&self) -> u32;
    async fn run(&self, env: &PerfEnv, iteration: u32) -> ScenarioEvidence; }
// run() = ОДНА итерация. Раннер вызывает run(): warmup раз с discard, затем iterations раз
// с записью — поэтому уникальность ids требует `iteration` в сигнатуре.
pub struct PerfEnv { pub service: MemoryService, pub db_client: Arc<SurrealDbClient>,
    pub ledger: Arc<QueryLedger>, pub sampler: Box<dyn ResourceSampler> }
impl PerfEnv { pub async fn connect() -> Self; }
// connect() = test_support::TestMemory::new() (внутри: connect_in_memory_with_namespaces +
// apply_migrations + MemoryService::new, ns всегда ACTIVE_NAMESPACE="main") + установка
// eval-observer → ledger через db_client. db_client нужен тестам и assemble_scale для
// проверок через SELECT. Реализация честная: expect-стиль, как в test_support.
pub fn build(spec: &ScenarioSpec) -> Box<dyn ScenarioRun>;
```
Сценарии (масштабы из профиля): `ingest_scale` — N ingest с уникальными ids `perf-{scenario}-{scale}-{iteration}-{i}`; `assemble_scale` — сначала ingest N, затем `queries` вызовов assemble (budget 5, детерминированные вопросы `"perf query {q}"`); латентные сценарии `latency-extract|resolve|explain|invalidate` на общем corpus-fixture (N=10, фиксированный `t_ref`). Каждое измерение: `Instant::now()` вокруг capability-вызова; ошибки записи → `invalid_reasons`, замер не пишется. Замеры warm-up исключаются раннером, не сценарием.

- [ ] **Step 1:** Падающий интеграционный тест `perf_scenarios.rs`: `PerfEnv::connect()` → `build(ingest_scale scale=2, iterations=2)`.run() для iteration=0 и iteration=1: суммарно `latencies_us.len()==4`, все `>0`; count эпизодов через `env.db_client` (`SELECT count()` по таблице эпизодов из миграций) ровно 4 → ids уникальны (`perf-{scenario}-{scale}-{iteration}-{i}`), дедуп-заражения нет (**Review Focus #5**); ledger: `EpisodeWrite` count == 4.
- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** Реализовать 6 сценариев (общий helper corpus-fixture) + `PerfEnv::connect`.
- [ ] **Step 4:** `cargo test -p eval-harness --test perf_scenarios` PASS; модульный `build()` unknown-id нет (тип закрыт).
- [ ] **Step 5:** Commit `feat(perf): in-process scenarios (ingest/assemble scale + op latency)` с тестом.

### Task 9: Runner — бюджет, warmup, артефакт, когерентный вердикт

**Files:**
- Modify: `crates/eval-harness/src/perf/runner.rs`
- Test: модульные (чистые части) + интеграционный `crates/eval-harness/tests/perf_runner.rs`

**Interfaces:**
- Consumes: всё предыдущее.
- Produces:
```rust
pub struct RunOptions { pub artifact_path: PathBuf, pub baseline_path: Option<PathBuf>,
    pub host_tag: String, pub commit: String, pub now_iso: String }
pub struct RunOutcome { pub artifact: PerfArtifact, pub verdict: PerfVerdict } // enum PerfVerdict {Passed, Regressed, Invalid} из verdict.rs
pub async fn run_profile(profile: &PerfProfile, profile_digest: &str, opts: RunOptions) -> RunOutcome;
```
Семантика: раннер вызывает `run(env, iteration)`: warmup-итерации (discard), затем `iterations` с записью, на каждый `(scenario, scale)`; перед сценарием/после — sampler-снапшоты, delta → `ScenarioRecord::resources` (`None` если сэмплер unsupported → +`invalid_reasons`, Review Focus #4); общий budget: `tokio::time::timeout(Duration::from_secs(profile.budget_seconds), ...)` вокруг всех сценариев — превышение завершает прогон штатно, пишет неполный артефакт, вердикт `Invalid` (Review Focus #2: пустые latencies → `None` + reason). Reducer заполняет `latencies`; `throughput_eps = reducer::throughput_eps(stats.total_us, stats.count)` для scale-сценариев; SQL-записи снимаются из snapshot delta `QueryLedger` один раз на сценарий. Env конструируется через `PerfEnv::connect()`.

- [ ] **Step 1:** Интеграционный тест: профиль-литератур (tempdir JSON, `IngestScale scales=[2] iterations=1 warmup=1`, `budget_seconds=60`, `host_tag="test"`) → `run_profile` → артефакт записан, `scenarios` содержит запись, `latencies.count==1`, `duration_ms>0`, вердикт `Passed` (без baseline). Отдельный тест: `budget_seconds=0` → артефакт записан, вердикт `Invalid`, причину содержит `"budget"`.
- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** Реализовать runner (порядок сценариев = порядок профиля — детерминизм).
- [ ] **Step 4:** PASS; clippy/fmt.
- [ ] **Step 5:** Commit `feat(perf): profile runner with budget guard, sampling and artifact emission`.

### Task 10: Verdict vs baseline (regression budgets)

**Files:**
- Modify: `crates/eval-harness/src/perf/verdict.rs`
- Test: модульные тесты

**Interfaces:**
- Consumes: `PerfArtifact`, `GateSpec`, `reducer::ratio`.
- Produces:
```rust
pub enum PerfVerdict { Passed, Regressed(Vec<String>), Invalid(Vec<String>) }
pub fn evaluate(profile: &PerfProfile, artifact: &PerfArtifact,
    baseline: Option<&PerfArtifact>) -> PerfVerdict;
pub fn exit_code(verdict: &PerfVerdict) -> i32; // 0/1/2
```
Правила: baseline present → (a) `baseline.host.host_tag != artifact.host.host_tag` → Invalid `"host_tag mismatch"` (Review Focus / ADR-0034); (b) для каждого gate: выборка current `count < gate.min_samples` → Invalid; baseline-метрика отсутствует → Invalid `"gate baseline metric missing"`; `ratio > max_ratio` → Regressed-запись `"scenario.metric ratio X>Y"`; при `direction=HigherIsBetter` сравнение инвертировано: regressed если `ratio(baseline, current) > max_ratio` (оба вызова через `reducer::ratio`). Без baseline — gates пропускаются (первый прогон определяет baseline, фиксируется в артефакте `invalid_reasons` не загрязняется).

- [ ] **Step 1:** Падающие тесты: идентичные артефакты → Passed; p95 ×1.3 при gate `max_ratio 1.25` → Regressed c сообщением, содержащим ключ и значения; mismatch host_tag → Invalid; gate c metric которого нет в baseline → Invalid; нет baseline → Passed; `min_samples=5` при count 3 → Invalid; throughput 0.7× baseline при `max_ratio 1.25` и `direction=HigherIsBetter` → Regressed (инверсия).
- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** Реализовать (весь ratio-math через `reducer::ratio`, не инлайн — Global Constraints).
- [ ] **Step 4:** PASS; clippy/fmt.
- [ ] **Step 5:** Commit `feat(perf): regression verdict with host-tag policy`.

### Task 11: Бинарь `memory-perf`, профили, Makefile, CI

**Files:**
- Modify: `crates/eval-harness/Cargo.toml` (новый `[[bin]]`), Create `crates/eval-harness/src/perf_main.rs`
- Create: `evals/perf/profiles/perf-pr-smoke.json`, `evals/perf/profiles/perf-weekly.json`
- Modify: `Makefile`, `.github/workflows/ci.yml`, `.github/workflows/evaluations.yml`
- Test: `crates/eval-harness/tests/perf_cli.rs`

**Interfaces:**
- Consumes: `perf::runner::run_profile`, `verdict::exit_code`.
- Produces:
```
[[bin]] name = "memory-perf" path = "src/perf_main.rs"
CLI: memory-perf run --profile <p> --artifact <a> [--baseline <b>] [--update-baseline <b>]
     --host-tag <t> (required) ; exits 0/1/2
```
`profile_digest` = sha256 файла профиля; `commit` из `git rev-parse --short HEAD` (Command), при ошибке — `"unknown"` (артефакт валиден, ADR-0082). `--update-baseline`: копирует записанный артефакт в путь baseline (только Passed-прогон). Профили: `perf-pr-smoke` (IngestScale [10] ×3 iters, LatencyExtract/Resolve/Explain/Invalidate, AssembleScale [10] queries=5, budget 300, `gates: []` — integrity-only, как response-size); `perf-weekly` (IngestScale [10,100], AssembleScale [10,100], gates: `assemble_scale.p95_us max_ratio 1.25 min_samples 5`, `ingest_scale.throughput_eps max_ratio 1.25` — где ratio direction: ниже = хуже; инверсия обрабатывается полем `direction` в `GateSpec` (см. Task 2), применяется в `verdict.rs`, не в reducer). ENV knobs: профиль передаёт `env` map (например `NER_EXTRACTOR=regex`) — runner применяет через `unsafe { std::env::set_var(...) }` ДО `PerfEnv::connect()`, по паттерну `with_ner_env` в `crates/memory-mcp/src/config/ner.rs` (edition 2024: set_var/remove_var — unsafe; общий `env_lock`-мьютекс не переиспользуется из memory-mcp cfg(test), в harness свой статический Mutex + SAFETY-комментарий: процесс memory-perf однопоточно применяет env до первого тред-пула), с WARN-логом почему.

- [ ] **Step 1:** Падающий CLI-тест `perf_cli.rs`: собрать tiny-профиль во временном каталоге, запустить `run_profile` через вспомогательную функцию бинаря (вынести `fn execute(cmd) -> i32` в `perf_main.rs`, `main` тонкий) → код 0, артефакт создан; tiny профиль в `tests/fixtures/` (уменьшенные итерации) → 0.
- [ ] **Step 2:** `cargo test -p eval-harness --test perf_cli` — FAIL.
- [ ] **Step 3:** Реализовать `src/perf_main.rs` (thin clap-парсинг + `fn execute(cmd) -> i32`) + профили (полный `perf-pr-smoke.json` и `perf-weekly.json` по схеме v1).
- [ ] **Step 4:** PASS: `cargo test -p eval-harness --test perf_cli`; `cargo run -p eval-harness --bin memory-perf --locked -- run --profile evals/perf/profiles/perf-pr-smoke.json --artifact target/perf/pr-smoke.json --host-tag local` на машине разработчика → exit 0, артефакт в `target/perf/`.
- [ ] **Step 5:** Makefile: `MEMORY_PERF_HOST_TAG ?= local` в шапке + цель `perf-smoke:` (команда из Step 4 с `--host-tag $(MEMORY_PERF_HOST_TAG)`); ci.yml: шаг после "Evaluation gate" (`make perf-smoke`, artifact upload `if: always()`); evaluations.yml: отдельная job `perf-weekly` (не шаг внутри eval-nightly — абсолютная латентность не измеряется на GitHub-hosted runner) с `runs-on: [self-hosted, perf-dev-m2]` и guard'ом `vars.MEMORY_PERF_PINNED_HOST == 'true'`, команда `cargo run -p eval-harness --bin memory-perf --locked -- run --profile evals/perf/profiles/perf-weekly.json --artifact target/perf/perf-weekly.json --baseline evals/perf/baselines/perf-weekly-dev-m2.json --host-tag dev-m2` + upload `target/perf/*.json` `if: always()` (новая repo-переменная — задокументировать в ADR-0082; значение `MEMORY_PERF_HOST_TAG` обязано совпадать с `host_tag` профиля — mismatch даёт exit 2).
- [ ] **Step 6:** Commit `feat(perf): memory-perf CLI, pr-smoke/weekly profiles, make + CI wiring`.

### Task 12: Первый baseline, results, runbook

**Files:**
- Create: `evals/perf/baselines/perf-weekly-<host_tag>.json`, `evals/perf/results/README.md`
- Modify: `docs/evals/README.md`, `docs/performance/` — создать `PERF_HARNESS.md` (короткий runbook)

- [ ] **Step 1:** На заявленной pinned-машине: зарегистрировать runner с label `perf-dev-m2` (`actions-runner --labels perf-dev-m2`) до выставления repo-переменной; затем `cargo run -p eval-harness --bin memory-perf --locked -- run --profile evals/perf/profiles/perf-weekly.json --artifact target/perf/weekly.json --host-tag dev-m2 --update-baseline evals/perf/baselines/perf-weekly-dev-m2.json`.
- [ ] **Step 2:** Проверить baseline-артефакт глазами (перцентили непустые, SQL-каталог содержит `EpisodeWrite`/`AnnSearch`/`FactWrite`, resources Some) — это human-gate, не автоматический тест (ADR-0073).
- [ ] **Step 3:** Закоммитить baseline + положить полный weekly.json в `evals/perf/results/2026-10-XX/` (полный размер выборок — в `target/`, в git — артефакт целиком: выборки cap не введён в v1).
- [ ] **Step 4:** Runbook `docs/performance/PERF_HARNESS.md`: команды, host_tag-политика, конвенция «обе цифры RSS (ps + footprint)», где читать SQL-каталог, как обновлять baseline (только с dated rationale, по образцу BASELINE_REVIEW). `docs/evals/README.md`: одна строка-секция Perf + ссылка.
- [ ] **Step 5:** Финальная валидация: `make perf-smoke` (exit 0), `cargo test -p eval-harness --test perf_cli --test perf_runner --test perf_scenarios`, `cargo clippy -p eval-harness --all-targets --locked -- -D warnings`, `cargo fmt --all --check`.
- [ ] **Step 6:** Commit `chore(perf): first weekly baseline, runbook, evals index entry`.

---

## Deferred (план 2, отдельный документ)

stdio/HTTP subprocess-драйверы + scrape `MEMORY_PROMETHEUS_LISTEN_ADDR` exposition (разблокирует гейты `HTTP_MEMORY_BASELINE.md`); прод-расширение `DECLARED_STAGES` (`extract.queue_wait`, `assemble.graph_expansion`, `assemble.budget_trim`, …); contention-сценарии (K клиентов, p95 + ops/s как типизированные метрики, замена `eprintln!` в `benches/contention.rs`); детерминированный embedding-provider под `eval-support`; spike RocksDB internals; spike remote-бэкенда (network RTT caveat в артефакте).
