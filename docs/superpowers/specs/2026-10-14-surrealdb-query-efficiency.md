# SurrealDB query efficiency: design and acceptance contract

Статус: proposed design; реализация и production rollout не разрешены этим документом.

## Цель и evidence

Снять подтверждённую лишнюю auth/session/query работу и обеспечить применение уже существующих индексов без деградации ответов, recall, ранжирования, temporal/access semantics, durability, performance и памяти. [Аудит и evidence](../../performance/surrealdb-query-audit-2026-10-14/audit.md) — источник фактов. Потенциальные scaling improvements проходят экспериментальный gate перед включением в production code; не каждый TableScan является дефектом.

## Ограничения

- Rust `1.99.0`; SurrealDB server/SDK `3.3.2`; baseline `bb4e9208c3054811e4fb883f28ad2145c4be65e7`.
- Production только read-only до отдельного разрешения; development fixtures изолированы. Не менять deployment, креды, logging level, данные или cadence на production при исследовании.
- Не менять существующие immutable migration scripts/checksums. Новые migration files/dependency changes требуют согласования по AGENTS.md до реализации.
- One Active Namespace на bound client; клиент никогда не rebinding под параллельный query. Outer `Arc` clone вместо SDK `Surreal::clone` на каждом tick.
- Context-local business policy в `provisioning/api.rs`, `operations/api.rs`, `knowledge`, `memory`, `embedding`; infra/composition в storage/http/bootstrap. Никакой новой бизнес-логики `service/`, transport или shared MemoryService dependency.
- FTS terms, per-term limits, ANN K/EF (`K=max(limit,1)`, `EF=max(4*K,16)`), distance EUCLIDEAN/F32, cosine threshold, reranker pool/radius, admission, graph32 batches и hop budgets неизменны.
- Никакой отмены query logging, access heat, provenance, temporal/ACL фильтров, security expiry/revocation или recovery deadlines ради экономии.
- Все новые behavior changes: наблюдённый RED → минимальный GREEN → REFACTOR. Реальная DB/network — integration tests, unit — controlled ports/time/identity. Ordering через barriers/permits, bounded waits, без sleep-based tests.

## Архитектура и порядок

1. Общий evidence harness фиксирует exact SQL/RPC/session/resource baseline и quality oracle.
2. Сначала безопасная outbox repair/upgrade semantics: fixed record, monotonic counter, migration042 никогда не replay-ится поверх существующего counter.
3. Ready tenant получает controlled catalog upgrade, не только HNSW reconciliation. Compatible expand migrations053–055 применяются ровно через canonical leased runner; readiness проверяет fields/index definitions и plans. Dimension/model transition имеет прежний explicit reembed gate; уже записанные foreign vectors нельзя автоматически переписать или удалить.
4. Maintenance получает отдельный bounded lightweight client pool, не активируя тяжёлый TenantRuntime. Общий factory/lease ownership исключает session churn; политика размерности не хранится как stale runtime state.
5. Scheduler имеет named jobs, bounded fair tenant pages, независимые dispatch/recovery/cleanup cadences и bounded reconciliation. Outbox delivery polling остаётся независимым.
6. Query owners исправляют graph cast, расширенные projection/existence reads и выполняют сохраняющие семантику batching experiments. Domain ranking остаётся в домене.

## Resource и scheduler policy

- Maintenance client pool: максимум32 resident/in-flight sessions суммарно; capacity reserved **до** bind; idle TTL300s; pin in-flight leases, release on cancellation, single-flight по binding key. Key = namespace/database/engine generation/migration dimension. При отсутствии capacity backpressure, не unbounded spill. Внутренний byte budget1MiB на keys/entries; SDK retained memory отдельно измеряется, не считается нулём. Значения конфигурируемы внутри deployment policy с теми же default.
- Ready discovery: page100, cursor persists между passes, wrap at end; не unbounded tenant Vec. Deadline fairness измеряется по количеству страниц/длительности, не обещается1s для произвольного числа tenants. Security readiness/revocation подтверждается registry перед admission; discovery cache не источник auth.
- Task dispatch и expired lease recovery: существующий1s tick / lease60s / heartbeat20s сохраняются. Cleanup sessions и terminal tasks:60s; cleanup delay не продлевает admission expiry. Artifact repair budget64/pass/tenant, prompt pass1s; старые terminal artifacts не переписываются, recovery backlog progresses без starvation. Fresh/recoverable work имеет приоритет над историческим sweep; commit-to-repair lag измеряется отдельно и не может деградировать из-за round-robin cursor. Ни один полный pass не обязан ждать все history rows.
- Sequence invariant audit:60s плюс safe ensure при outbox write; listener polling/notification contract не меняется. Quota reconcile60s и local-admin buckets300s сохраняются. Backfill cadence60s сохраняется; embeddings-off не вызывает provider/ANN/index rebuild.
- Cadence принадлежит instance state и injected clock, не process-global LAST_RUN/lock; пропуск cadence не считается executed DB work. Metrics labels — pinned job categories/outcomes, без tenant/SQL/content label cardinality.

## Семантическая эквивалентность

Graph: empty seeds, both directions, self-loops, duplicates, IDs с `:`, пробелами, Unicode, `⟩`, out-of-order invalidation, inclusive/exclusive cutoff, missing/NULL timestamps; rows/path provenance/degrees/hop coverage идентичны.

Retrieval: policy tags empty/NONE/NULL, inaccessible sources, fact types, browse, low-overlap lexical, multilingual substring, exact references, relative-date event вне t_valid window, persona wake-up ties, facets counts, source siblings с неодинаковой степенью. Сокращение scan допустимо только при доказательстве совпадения набора и global ranking statistics. Перенос LIMIT перед фильтром/ranking, общий LIMIT вместо per-term/per-seed LIMIT, FTS вместо substring — запрещены без полного oracle proof; в этом проекте не планируются.

Mutations: saturating signed access increments, duplicate item semantics, missing rows, vector writer race, field-level CAS, fenced tasks, cancellation after commit, partial batch faults, migration crash/retry/checksum, outbox unique sequence и replay. Нельзя вводить транзакционность batch, меняющую прежнюю partial-success семантику.

## Матрица acceptance

- Dimensions `17,384,768,1536,2048,3072`; provider disabled с legacy vectors, namespace вообще без vectors, missing/NULL/mixed old signature; transitions `384→2048→768`, смена model same-dimension, failed/cancelled/resumed reembed. Синтетические vectors проверяют contracts; реальные provider/corpus — отдельно качество.
- Registry tenants1/100/101/257, clients churn1000bindings, two replicas, idle и reconnect/auth failure; task artifacts0/1/64/65/10000; rows0/1/100/10000; pending vector density0/1/50/100%; graph degree skew/self-loop/32+ seeds; embedding-heavy payloads.
- Parent and candidate immutable image/commit hashes; same Surreal image digest, migrated vs legacy fixtures **раздельно**, same provider/model/corpus hashes/time/resources/concurrency and warm/cold cache mode. Schema upgrade может менять ANN physical graph; quality gate требуется даже без ANN SQL edits.
- Реальные quality cases: recall@K, precision@K, MRR/NDCG, answer/support accuracy, irrelevant/hallucinated admissions и contradiction handling. Ни одна case не теряет relevant/support IDs; ни один reported quality slice не снижается. Отсутствующий denominator/artifact — INVALID, не PASS. Нормализация исключает только clocks/request IDs с явно фиксированным списком, не content/ranking/confidence.
- Deterministic comparison использует frozen episodes/facts/vectors и fixed time, чтобы query optimization не смешивалась со stochastic provider drift. Live extraction/provider quality проверяется отдельно на paired frozen cases; изменчивый provider output нельзя выдавать за deterministic SQL regression или маскировать одной aggregate метрикой.

Deterministic outputs/provenance/order unchanged, кроме исправления documented pre-existing defect с independent oracle. Такие исключения документируются case-by-case, не общей маской.

## Строгие гейты

G0: baseline executable tests/clippy/fmt green или сначала исправить ошибки отдельным scoped commit; не списывать как pre-existing.

G1: наблюдённый behavior RED каждого production change; isolated remote SurrealDB3.3.2 regression GREEN. Structured EXPLAIN assertions вместе с rows/semantics, не substring-only assertion.

G2: warmed idle N stable tenants: bind/signin count не растёт от tick count; reconnect создаёт не более одного нового session per invalidated binding. Capacity/byte accounting bounded, cancellation/shutdown releases resources.

G3: ровно0 quality regressions по всем cases/slices; frozen real provider/corpus run полный, без fixtures-only claims.

G4: same-host A/A noise calibration до A/B;5 paired AB/BA runs, warmup60s, measured300s для idle и throughput; ≥10000 successful observations для p99 claim, иначе p99 evidence INVALID. Если A/A variation p95/p99/throughput >3% или CPU/memory >5%, benchmark INVALID: устранить noise, не расширять budget. Для A/B верхняя95% paired interval latency/CPU/memory ratios ≤1+measured AA noise, нижняя throughput ratio ≥1−noise. Это измерительная неопределённость, не разрешение подтверждённой деградации. Повторяемая directionally worse metric даже внутри band требует расследования. Idle DB CPU target upper95% ratio ≤0.60. Total SQL/RPC/signin, app+DB RSS/HWM/cgroup/swap и physical I/O измеряются вместе. По принятому решению пользователя новый native CI runner не требуется: локальные amd64 прогоны в эмуляции допустимы, architecture/emulation обязательно помечаются, результаты не выдаются за production forecast. Native production validation — последующий canary при отдельном разрешении rollout; до него численный production speedup не подтверждён.

G5: cold/warm retrieval, ingest/extract/invalidate/explain, task dispatch/recovery/reembed, concurrent tenants, saturated pool и auth/reconnect tests; нет роста errors/timeouts, starvation, deadlocks или просроченных fences/notifications.

G6: smallest suites сначала; перед release полный applicable feature matrix, strict workspace Clippy, fmt, hosted CI. Validator fixtures ≠ actual measured evidence. Existing CI jobs используются без нового отдельного timing stage; candidate merge/release запрещён, пока G0–G6 не подтверждены.

## Экспериментальные ветки

Backfill query hint/projection, claim_id index, entity prefix indexing, community offset→keyset/snapshot, batched heat updates и source/neighborhood/hub/archival joins — включаются только при semantic oracle equivalence и workload performance win без write/memory regression. Непрошедшие эксперименты заканчиваются evidence «не внедрять», а не отключением quality gate. Это предусмотренный результат плана, не пропуск обязательной задачи.
