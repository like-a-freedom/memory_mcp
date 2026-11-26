# Automatic Embedding Input Type Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Автоматически поддерживать обычные embedding API и `input_type=passage/query`, не добавляя настройки пользователю и не смешивая несовместимые векторы.

**Architecture:** Существующие document/query пути сохраняются. Embedding context определяет immutable input contract через проверенный профиль модели или ограниченную проверку API; провайдер, signature и все composition adapters используют один resolved target. Старые symmetric signature остаются стабильными.

**Tech Stack:** Rust workspace, async-trait, reqwest, serde_json, Tokio, существующие storage/reembed/recovery ports. Новые зависимости не нужны.

**Spec:** [Дизайн](../specs/2026-11-26-automatic-embedding-input-type-design.md). Код реализован и основные проверки пройдены; чекбоксы ниже фиксируют исходный план, а не актуальный статус каждого предложенного теста. Live smoke NVIDIA на синтетическом тексте подтвердил ответы passage/query размерностью 2048; сравнительная оценка качества не выполнялась.

## Global Constraints

- Пользователь не настраивает input_type: без новой env-переменной, config field, CLI option или MCP tool.
- Workspace MSRV `1.100.0`; `rust-toolchain.toml` остаётся rolling `stable`.
- 8-tool surface frozen. Не менять зависимости в Cargo.toml, generated code и migration files.
- Документ → passage; query → query. Локальные E5 prefixes, Ollama и disabled сохраняют текущий контракт.
- Не включать mode/source/time реестра в identity иначе, чем стабильный идентификатор input contract. Старые symmetric и local E5 signature сохраняются побайтно.
- Preflight: общий deadline 10 секунд, максимум 3 запроса, фиксированный технический текст, без runtime backoff.
- Пробные запросы и сетевые проверки — integration tests; unit tests используют управляемые adapters. Никаких sleep для синхронизации тестов.
- Новая бизнес-логика — в embedding context; существующие service/startup/recovery adapters только передают resolved result и выполняют текущую orchestration.
- Перед shipping обязательны форматирование и workspace clippy с feature set из AGENTS.md. Этот документ сам по себе не является shipping продукта.

## Review Focus

1. Неизвестная модель + dimension override: override не определяет input contract; не активировать guessed target при неудачном probe. Task 2.
2. API возвращает 200 и игнорирует input_type: профиль обязателен для известных асимметричных моделей; не объявлять arbitrary API автоматически распознанным. Tasks 1–2.
3. 400/422 из-за другого поля, текст ошибки с input_type или 401/429: не менять режим, не скрывать первоначальный отказ. Task 2.
4. У модели та же размерность, но изменился input contract: не использовать старую signature/query cache и не считать индекс ready. Task 3.
5. Recovery/reembed/HTTP/eval создаёт новый provider без уже определённого контракта: parity проверяется через фактически принятые endpoint запросы. Tasks 2–3.

## Карта файлов

Все пути ниже относительно корня репозитория. Перечислены целевые файлы; текущие незакоммиченные изменения в checkout не входят в реализацию этого плана.

| Файлы | Ответственность |
|---|---|
| Create `crates/memory-mcp/src/embedding/input_contract.rs` | Тип контракта, точные проверенные model profiles, чистый выбор профиля |
| Create `crates/memory-mcp/src/embedding/providers/remote/capabilities.rs` | Bounded negotiation и классификация missing-input_type, отделённые от retries |
| Modify `crates/memory-mcp/src/embedding.rs`, `embedding/providers.rs`, `embedding/providers/remote.rs` | Регистрация модуля, target, общий request builder, override embed_query, фабрика |
| Modify `crates/memory-mcp/src/config/embedding.rs`, `config.rs` | Contract-aware signature без изменения прежних hashes |
| Modify `crates/memory-mcp/src/bootstrap/stdio.rs`, `service/startup.rs`, `service/embedding_recovery.rs` | Передача target от resolution до активации и recovery |
| Modify `crates/memory-mcp/src/http/runtime/bootstrap.rs`, `http/runtime/storage.rs` | Deployment policy и tenant compatibility из одной identity |
| Modify `crates/memory-mcp/src/eval_support/retrieval.rs` | Тот же resolver/provider для quality runs |
| Modify `crates/memory-mcp/src/embedding/service.rs`, `embedding/api.rs` | Только необходимые identity/cache regression tests и комментарии; маршрутизация уже существует |
| Create `crates/memory-mcp/tests/remote_embedding_input_type.rs`, `tests/common/remote_embedding_endpoint.rs` | Записывающий HTTP fixture и фактические запросы через production factory |
| Modify `crates/memory-mcp/tests/cli_embedding_reembed.rs`, `http_embedding_reembed.rs`, `http_embedding_backfill.rs` | Passage при обслуживании и parity HTTP |
| Modify `README.md` | Автоопределение, offline boundary и reembed workflow |

Пути `embedding/...`, `config.rs` и `service/...` в нескольких строках таблицы продолжают `crates/memory-mcp/src/`. Имена тестов в шагах ниже — предлагаемые новые regression tests.

### Task 1: Зафиксировать тип контракта и проверенные профили

**Files:** `embedding/input_contract.rs`, `embedding.rs`, `embedding/providers.rs`, `config/embedding.rs`, `config.rs`.

**Interfaces:**

- В новом модуле: `pub enum EmbeddingInputContract { Symmetric, QueryPassageV1 }`, `#[derive(Debug, Clone, Copy, PartialEq, Eq)]`.
- `pub(crate) fn known_input_contract(model: &str) -> Option<EmbeddingInputContract>` — точный match по проверенным ID из spec, без произвольного lowercasing или substring matching.
- В `config/embedding.rs`: `pub fn build_embedding_signature_for_contract(provider_label: &str, model: Option<&str>, base_url: Option<&str>, dimension: usize, contract: EmbeddingInputContract) -> String`. Re-export через `config.rs`.
- Существующий `build_embedding_signature(...) -> String` сохраняет поведение и используется как совместимый wrapper для Symmetric; local-candle сохраняет существующий material с E5 contract.
- Для QueryPassageV1 добавить в material ровно `"input_contract": "input-type-query-passage-v1"`; никакого общего bump для всех моделей.

- [ ] **1. Написать failing tests чистого публичного интерфейса.** Имена: `nemotron_hosted_profile_selects_query_passage`, `unlisted_nvidia_model_is_not_guessed`, `model_id_is_not_rewritten`, `symmetric_signature_preserves_legacy_hash`, `query_passage_signature_differs_at_equal_dimension`, `local_e5_signature_is_unchanged`. Зафиксировать literal прежних hashes до изменения helper, не вычислять expected тем же новым кодом.

  Фокальные assertions:

  ```rust
  assert_eq!(known_input_contract("nvidia/nemotron-3.5-embed-1b"), Some(EmbeddingInputContract::QueryPassageV1));
  assert_eq!(known_input_contract("nvidia/unlisted-embedding-model"), None);
  ```

- [ ] **2. Запустить** `rtk cargo test -p memory_mcp embedding::input_contract --locked` и `rtk cargo test -p memory_mcp config::embedding::tests --locked`. Новые проверки должны сначала падать по отсутствующему интерфейсу/изменившемуся hash.
- [ ] **3. Реализовать** тип, узкий реестр с источниками на каждую запись и signature helper. Проверить актуальную документацию OpenAI для трёх symmetric ID перед добавлением записей; не включать неподтверждённые family masks.
- [ ] **4. Повторить команды шага 2.** Ожидается PASS; существующие signature tests также проходят.
- [ ] **5. Создать отдельный commit** `feat(embedding): model automatic input contracts` только из файлов задачи.

### Task 2: Автоопределение и passage/query через единый resolved target

**Files:** `embedding/providers/remote.rs`, новый `embedding/providers/remote/capabilities.rs`, `embedding/providers.rs`, `bootstrap/stdio.rs`, `service/startup.rs`, `service/embedding_recovery.rs`, `http/runtime/bootstrap.rs`, `http/runtime/storage.rs`, `eval_support/retrieval.rs`, новый `tests/remote_embedding_input_type.rs`, новый `tests/common/remote_embedding_endpoint.rs`.

Это одна вертикальная задача: resolver, signature, фабрика и runtime role должны попасть в один компилируемый commit. Добавление поля target без обновления всех constructors не является завершённым промежуточным результатом.

**Resolution interfaces:**

- В `remote.rs`: `pub(super) enum RemoteEmbeddingPurpose { Document, Query }` и один builder `fn openai_embedding_body(model: &str, input: &str, purpose: RemoteEmbeddingPurpose, contract: EmbeddingInputContract) -> serde_json::Value`. Он используется и рабочим запросом, и one-shot probe.
- В `remote/capabilities.rs`: `pub(crate) struct ResolvedRemoteCapabilities { pub dimension: usize, pub input_contract: EmbeddingInputContract }`.
- `pub(crate) async fn resolve_openai_capabilities(client: &reqwest::Client, config: &EmbeddingConfig) -> Result<ResolvedRemoteCapabilities, MemoryError>` — algorithm/deadline из spec. Общий timeout охватывает все запросы вместе.
- `ResolvedEmbeddingTarget` получает `pub(crate) input_contract: EmbeddingInputContract`. `resolve_embedding_target_identity` принимает прежние аргументы, но получает dimension + contract совместно, затем строит contract-aware signature.
- HTTP fixture: endpoint `127.0.0.1:0`, записанные JSON, сценарий ответов, shutdown handle. Существующие Tokio/serde_json, channels/permits для порядка, bounded shutdown/join. Тесты вызывают production factory через `eval_support::retrieval::create_eval_embedding_provider`; private helpers не открываются ради тестов.

**Runtime interfaces:**

- Заменить внутреннюю фабрику, теряющую contract: `pub(crate) async fn create_embedding_provider_for_target(config: &EmbeddingConfig, data_dir: &str, target: &ResolvedEmbeddingTarget) -> Result<Arc<dyn EmbeddingProvider>, MemoryError>`.
- `OpenAiCompatibleEmbeddingProvider` хранит immutable contract. `embed` использует Document, собственный `embed_query` — Query. Оба сохраняют существующие normalization, validation и runtime retry policy.
- Recovery port: `async fn probe_target(&self) -> Result<ResolvedEmbeddingTarget, MemoryError>` и `async fn create_provider(&self, target: &ResolvedEmbeddingTarget) -> Result<Arc<dyn EmbeddingProvider>, MemoryError>`. Вместо `probe_dimension` возвращается цель целиком.
- `choose_recovery_decision(index_dimension: usize, stored_signature: Option<&str>, target: ResolvedEmbeddingTarget) -> RecoveryDecision`. Не пересчитывать signature из config.
- `http::runtime::storage::EmbeddingPolicy` получает `pub input_contract: EmbeddingInputContract`; tenant target копирует его из policy. Поле отражается в Debug без credentials. Обновить все Rust struct literals; не менять HTTP/MCP JSON schemas.
- Local/Ollama/disabled возвращают Symmetric как remote wire descriptor; локальная E5 input identity по-прежнему кодируется существующим E5 material. Никакого изменения E5 prefixes.

- [ ] **1. Написать failing integration tests** с feature `eval-support`:

  | Test | Observable assertion |
  |---|---|
  | `known_nemotron_uses_both_roles_during_preflight` | Только passage, затем query; model ID неизменён; один выбранный contract |
  | `unknown_required_input_type_is_detected` | Сначала без поля → structured missing input_type 422 → passage → query; всего 3 запроса |
  | `ordinary_strict_endpoint_keeps_legacy_body` | Тело содержит только model/input, endpoint отвергает любые дополнительные поля |
  | `known_nemotron_does_not_depend_on_missing_field_error` | Endpoint принял бы запрос без поля, но профиль всё равно отправил обе роли |
  | `unknown_permissive_endpoint_keeps_legacy_contract` | HTTP 200 без поля сохраняет symmetric wire contract; не делается ложный вывод об архитектуре |
  | `unrelated_validation_error_does_not_negotiate` | Неподходящие 400/422 не вызывают второй запрос |
  | `unstructured_input_type_message_does_not_negotiate` | Упоминание input_type в строке/HTML не запускает переключение |
  | `authentication_or_rate_limit_does_not_change_contract` | Независимые parameterized 401/403/429 cases: один preflight request, исходный error class |
  | `inconsistent_role_dimensions_refuse_activation` | Разные размерности passage/query не создают готовый provider |
  | `invalid_role_vector_refuses_activation` | Независимые missing/empty/nonfinite-vector cases отказывают до readiness |
  | `negotiation_has_one_total_deadline` | Управляемый незавершаемый response future отменяется в пределах общего бюджета; не 10 секунд на каждый шаг |
  | `known_profile_override_resolves_offline` | Известная модель + dimension override не отправляет HTTP |
  | `unknown_profile_override_still_resolves_input_contract` | Неизвестная strict-модель + override проходит negotiation до возврата target |

  Дополнительные runtime cases: `document_and_query_use_distinct_wire_roles`, `runtime_retries_keep_the_selected_role`, `known_query_passage_does_not_downgrade_on_rejection`, `recovery_reuses_resolved_target`, `http_tenants_share_one_contract_resolution`, `eval_factory_uses_runtime_input_contract`. После preflight очистить список принятых запросов: документ даёт ровно passage, query — ровно query. Для ретрая использовать управляемое время/Retry-After=0, без sleep. HTTP parity включает реальное разрешение deployment policy, а не только ручное создание policy в тесте.

- [ ] **2. Запустить RED:** `rtk cargo test -p memory_mcp --features eval-support --test remote_embedding_input_type --locked`. Для HTTP cases добавить features `streamable-http,test-fixtures`. Убедиться, что baseline падает на отсутствующем input_type/неправильном числе запросов или роли, а не на поломанном fixture.
- [ ] **3. Реализовать** builder и resolver. Missing-field классификация — по точным structured param/code либо detail.loc/type, до redaction; body ограничен 4096 bytes, raw body не сохраняется. Не делать status-only fallback или substring search в MemoryError. Для known profile + override пропустить сеть; для unknown + override определить контракт до активации, сохранив override как expected runtime dimension.
- [ ] **4. Провести target через все constructors** и реализовать собственный `embed_query` удалённого провайдера. Resolver не вызывается внутри embed, runtime 4xx не меняет contract. Recovery не публикует частично определённый target. HTTP разрешает capability один раз на deployment, eval adapter использует ту же factory. Все старые прямые вызовы `build_embedding_signature` в production startup/recovery заменить contract-aware identity; тестовые legacy fixtures оставить там, где они намеренно моделируют старый индекс.
- [ ] **5. Выполнить GREEN:** повторить integration target, `rtk cargo test -p memory_mcp service::embedding_recovery --locked` и `rtk cargo test -p memory_mcp embedding::providers --locked`. Для HTTP cases — `--features eval-support,streamable-http,test-fixtures`. Затем `cargo check -p memory_mcp --features eval-support,streamable-http,test-fixtures --locked`. Обновить прежние offline tests: known symmetric/passage profiles остаются offline; unknown + override проверяется перед semantic activation. Новые network scenarios размещать в integration target. Не менять соседние пользовательские retrieval/eval правки.
- [ ] **6. Создать один commit** `feat(embedding): detect and apply remote input contracts` после прохождения всех checks задачи.

### Task 3: Проверить совместимость индекса, кэшей и обслуживания

**Files:** `embedding/service.rs`, `embedding/api.rs`, `tests/remote_embedding_input_type.rs`, `tests/cli_embedding_reembed.rs`, `tests/http_embedding_reembed.rs`, `tests/http_embedding_backfill.rs` и соответствующие существующие startup/recovery tests в owning modules.

**Interfaces:** используются target/factory из Tasks 1–2 и прежние canonical vector ports. Новые механизмы хранения, cache и rebuild API не вводятся.

- [ ] **1. Написать failing regression cases:**

  | Test | Observable assertion |
  |---|---|
  | `same_model_and_width_new_contract_requires_reembed` | Старый remote hash при той же dimension не даёт semantic readiness для нового контракта |
  | `unchanged_symmetric_contract_does_not_require_reembed` | Существующий ordinary index остаётся ready без переписывания векторов |
  | `query_cache_isolated_by_input_contract_signature` | Одинаковый query под разными contract signatures вызывает отдельную генерацию; повтор внутри одной signature — cache hit |
  | `cli_reembed_writes_passage_vectors` | Через CLI maintenance реально пришёл passage; сохранён target hash; не вызывается query для записи |
  | `http_reembed_restores_query_passage_readiness` | Существующий operator reembed записал passage; после ready retrieval отправил query |
  | `backfill_keeps_existing_vectors_during_contract_change` | Recovery/backfill заполняет только missing; старые векторы и active signature не переписаны до явного reembed |
  | `reembed_resume_keeps_input_contract` | После управляемого сбоя возобновление использует прежний target contract и не переопределяет его по dimension |

- [ ] **2. Запустить** `rtk cargo test -p memory_mcp --features eval-support --test remote_embedding_input_type --test cli_embedding_reembed --locked`. Для HTTP: `rtk cargo test -p memory_mcp --features streamable-http,test-fixtures --test http_embedding_reembed --test http_embedding_backfill --locked`.
- [ ] **3. Исправить только выявленные пробелы передачи identity.** Существующие guards, cache keys и reembed должны быть достаточны при правильной signature; не добавлять параллельный migration engine. Комментарий VectorIdentity должен явно упоминать input contract.
- [ ] **4. Повторить команды шага 2** и `rtk cargo test -p memory_mcp embedding::service --locked`. Для E5 сохранить существующие purpose regressions; model-backed ignored tests запускать только при наличии проверенного локального cache.
- [ ] **5. Создать commit** `test(embedding): verify input contract migration and maintenance` либо `fix(embedding): preserve input contracts during maintenance`, если потребовался production fix.

### Task 4: Диагностика, документация и итоговая проверка

**Files:** `embedding/providers/remote/capabilities.rs`, `README.md`, `tests/remote_embedding_input_type.rs`; при необходимости existing logging tests.

**Interfaces:** событие `embedding.input_contract_resolved` из spec; публичные настройки и wire schemas Memory MCP не меняются.

- [ ] **1. Добавить проверку диагностики:** успешное определение даёт contract/source/dimension и sanitized endpoint; validation response с input/API key не раскрывает их в новых событиях. Tests проверяют observable captured events, не markdown inventory. Источник определения не влияет на hash.
- [ ] **2. Реализовать событие** через текущую logging facade и политику уровней. Успешный resolve — INFO; error levels из `MemoryError::log_level()`. Не включать raw response или memory content и не добавлять unbounded metric labels.
- [ ] **3. Обновить README:** автоматический выбор без input_type config; JSON examples passage/query; канонический model ID с точкой; отличие URL страницы; известный профиль + dimension override остаётся offline, unknown + override требует contract preflight; обычные модели не требуют reembed, а смена model/contract требует существующий явный rebuild. Не обещать поддержку произвольных молча игнорирующих параметр API.
- [ ] **4. Выполнить итоговые проверки** после узких tests:

  ```bash
  cargo fmt --all --check
  rtk cargo test -p memory_mcp --locked
  rtk cargo test -p memory_mcp --features eval-support --test remote_embedding_input_type --test cli_embedding_reembed --locked
  rtk cargo test -p memory_mcp --features streamable-http,test-fixtures --test http_embedding_reembed --test http_embedding_backfill --locked
  cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http,test-fixtures --locked -- -D warnings
  ```

  Ожидается PASS/zero warnings. Не запускать весь workspace test suite без новой причины. В грязном checkout отдельно сообщить о мешающих чужих failures; не исправлять их незаметно.

- [ ] **5. Выполнить ограниченный live smoke** при наличии пригодного API access: на синтетических текстах получить passage/query от NVIDIA и проверить нормальные vectors/dimension; затем тот же smoke для действующей symmetric модели. У smoke одна цель — wire compatibility, а не доказательство retrieval quality. Не читать/печатать секреты и не запускать reembed пользовательской базы. Если доступа нет, явно отметить live smoke как невыполненный.
- [ ] **6. Оценить retrieval quality отдельно** на фиксированном небольшом наборе синтетических RU/EN документов и вопросов: сохранённые relevance labels, одинаковые corpus/top-k/thresholds, Recall@k и MRR с fingerprints моделей/контрактов. Это отчёт, а не flaky CI assertion «новая модель обязательно лучше». Никакой передачи реальной памяти для этой проверки.
- [ ] **7. Создать commit** `docs(embedding): explain automatic input type support` и предоставить результаты checks, список поддержанных профилей и остаточную границу autodetect.

## Критерий готовности

Пользователь использует прежние model/provider/base URL settings. NVIDIA получает passage для сохранения и query для поиска; обычные API получают прежний запрос. Определение не повторяется на каждом вызове, signature соответствует фактическому контракту во всех paths, старые векторы переходят через существующий безопасный workflow обслуживания. Новых обязательных действий по настройке input_type нет.
