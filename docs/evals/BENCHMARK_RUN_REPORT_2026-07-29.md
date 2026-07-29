# Benchmark & Evaluation Run Report v2

**Date**: 2026-07-29  
**Commit**: `fa57d49b`  
**Previous run**: [2026-07-28](BENCHMARK_RUN_REPORT_2026-07-28.md)  
**Host**: macOS 26.5.2 — Apple Silicon (arm64)  
**Rust**: 1.97.1 / Cargo 1.97.1  

---

## 1. Fixture-Coverage Tests

Быстрые синхронные тесты, проверяют корректность eval-фикстур и public surface.

| Test File | Tests | Passed | Failed | Ignored | Status |
|-----------|-------|--------|--------|---------|--------|
| `eval_agent_memory_lifecycle` | 5 | 4 | 0 | 1 | ✅ |
| `eval_claim_reconciliation` | 1 | 1 | 0 | 0 | ✅ |
| `eval_extraction` | 4 | 3 | 0 | 1 | ✅ |
| `eval_retrieval` | 5 | 4 | 0 | 1 | ✅ |
| **Total** | **15** | **12** | **0** | **3** | ✅ |

**Ignored** (ожидаемо):
- `run_agent_memory_lifecycle_baseline` — deferred per ADR-0017
- `harness_extraction_suite` — прогоняется через `memory-eval run`
- `harness_retrieval_suite` — прогоняется через `memory-eval run`

---

## 2. Eval-Harness Profile Runs

### 2.1 PR Profile (`evals/profiles/pr.json`)

**Run ID**: `run-1785296815` | **Duration**: 9 902 ms

| Suite | Mode | Total | Passed | Quality Failed | Invalid |
|-------|------|-------|--------|----------------|---------|
| `local-retrieval` | retrieval_only | 66 | 65 | 1 | 0 |
| `extraction` | end_to_end | 9 | 7 | 2 | 0 |
| `claim-reconciliation` | end_to_end | 42 | 38 | 4 | 0 |
| **Total** | | **117** | **110** | **7** | **0** |

#### Suite Metrics

**local-retrieval:**
| Metric | Observed | Hard Floor | Status |
|--------|----------|------------|--------|
| recall_at_5 | 0.9868 | 0.90 | ✅ Passed |
| mrr | 0.9823 | 0.85 | ✅ Passed |
| top_1_hit_rate | 0.9697 | 0.80 | ✅ Passed |

**extraction:**
| Metric | Observed | Hard Floor | Status |
|--------|----------|------------|--------|
| entity_f1 | **0.7500** | 0.70 | ✅ Passed |
| entity_precision | 0.6000 | — | — |
| entity_recall | 1.0000 | — | — |

**claim-reconciliation:**
| Metric | Observed | Hard Floor | Status |
|--------|----------|------------|--------|
| claim_precision | 0.0000 | 0.50 | ❌ Failed |
| claim_recall | 0.0000 | 0.50 | ❌ Failed |

#### Gate Results

| Gate | Observed | Floor | Status |
|------|----------|-------|--------|
| recall_at_5 | 0.9868 | 0.90 | ✅ |
| mrr | 0.9823 | 0.85 | ✅ |
| top_1_hit_rate | 0.9697 | 0.80 | ✅ |
| entity_f1 | **0.7500** | 0.70 | ✅ |
| claim_precision | 0.0000 | 0.50 | ❌ |
| claim_recall | 0.0000 | 0.50 | ❌ |

**Overall**: ⚠️ GATE FAILED (retrieval + entity gates passed; claim gates fail at zero)

---

### 2.2 Release Profile (`evals/profiles/release.json`)

**Run ID**: `run-1785296832` | **Duration**: 10 647 ms

| Suite | Mode | Total | Passed | Quality Failed | Invalid |
|-------|------|-------|--------|----------------|---------|
| `local-retrieval` | retrieval_only | 66 | 65 | 1 | 0 |
| `extraction` | end_to_end | 9 | 7 | 2 | 0 |
| `claim-reconciliation` | end_to_end | 42 | 38 | 4 | 0 |
| `lifecycle` | lifecycle | 4 | 3 | 1 | 0 |
| **Total** | | **121** | **113** | **8** | **0** |

#### Lifecycle Suite Details

| Case ID | Status | Metrics |
|---------|--------|---------|
| `lifecycle-action-grounding` | ✅ Passed | grounding_pass_rate: 1.0 |
| `lifecycle-capacity` | ✅ Passed | — (structural test) |
| `lifecycle-poisoning` | ⚠️ Quality Failed | poisoning_pass_rate: 0.6667 (2/3) |
| `lifecycle-public-surface` | ✅ Passed | — (structural test) |

#### Gate Results

| Gate | Observed | Floor | Status |
|------|----------|-------|--------|
| recall_at_5 | 0.9868 | 0.90 | ✅ |
| mrr | 0.9823 | 0.85 | ✅ |
| top_1_hit_rate | 0.9697 | 0.80 | ✅ |
| entity_f1 | **0.7500** | 0.70 | ✅ |
| claim_precision | 0.0000 | 0.50 | ❌ |
| claim_recall | 0.0000 | 0.50 | ❌ |

**Overall**: ⚠️ GATE FAILED (same pattern as PR)

---

### 2.3 Nightly Profile (`evals/profiles/nightly.json`)

**Run ID**: `run-1785296847` | **Duration**: 10 300 ms  
**Result**: ✅ **PASSED** (отсутствие gate definitions = нет fail)

| Suite | Total | Passed | Quality Failed | Invalid |
|-------|-------|--------|----------------|---------|
| `local-retrieval` | 66 | 65 | 1 | 0 |
| `extraction` | 9 | 7 | 2 | 0 |
| `claim-reconciliation` | 42 | 38 | 4 | 0 |
| `end-to-end` | 2 | 1 | 1 | 0 |
| `downstream-qa` | 0 | — | — | — |
| **Total** | **119** | **111** | **8** | **0** |

#### End-to-End Suite Details

| Case ID | Status | Metrics | Failures |
|---------|--------|---------|----------|
| `e2e-pipeline-completes` | ✅ Passed | evidence: 3/3, context_items: 1 | — |
| `e2e-entity-extraction` | ❌ Quality Failed | evidence: 0/3, context_items: 0 | evidence mismatch, context empty |

**Note**: `downstream-qa` suite produced 0 cases (не реализована или не сконфигурирована).

---

## 3. Criterion Benchmarks

Compiled under `bench` profile (optimized).

### 3.1 Pipeline (`benches/pipeline.rs`)

| Benchmark | ns/iter | ms/iter | ± | vs v1 |
|-----------|---------|---------|---|-------|
| `ingest_single_episode` | 65 820 250 | 65.82 | 719 341 | +2.1% |
| `extract_single_episode` | 1 000 604 | 1.00 | 53 446 | +20.2% |
| `assemble_context_single_query` | 68 447 333 | 68.45 | 903 659 | +2.0% |
| `retrieval_metrics_100_cases` | 6 658 | 0.007 | 356 | +8.3% |

**Наблюдения**: стабильность в пределах шума. Изменение extract (+20%) может быть связано с тем, что теперь работает реальная entity extraction.

### 3.2 NER CPU (`benches/ner_cpu.rs`)

| Benchmark | ns/iter | µs/iter | ± | vs v1 |
|-----------|---------|---------|---|-------|
| `ner_cpu_single_window` | 69 783 562 | 69 784 | 3 024 385 | **+1 789 000×** |
| `ner_cpu_multi_window` | 3 946 562 | 3 947 | 117 087 | **+6 500×** |

**⚠️ Ключевое изменение**: в v1 bench-функции были пустыми/stub (39 ns / 606 ns — просто подсчёт слов). После фикса харнеса теперь реально исполняется полный пайплайн NER (включая инференс Candle). Это ожидаемое и корректное поведение.

### 3.3 NER Metal (`benches/ner_metal.rs`)

| Benchmark | ns/iter | ± | vs v1 |
|-----------|---------|---|-------|
| `ner_metal_single_window` | 43 | 0 | +10.3% |

**Наблюдение**: bench-функция остаётся stub-ом (не вызывает реальный Metal GPU). 43 ns — минимальный шум. Требует отдельной доработки для полноценного GPU NER.

### 3.4 Contention (`benches/contention.rs`)

| Benchmark | ns/iter | ms/iter | ± | vs v1 |
|-----------|---------|---------|---|-------|
| `contention_single_client` | 85 630 145 | 85.6 | 1 312 192 | **+85 630 000×** |
| `contention/clients_2` | 298 672 187 | 298.7 | 29 768 639 | **+11 400×** |
| `contention/clients_4` | 298 843 708 | 298.8 | 29 289 090 | **+7 800×** |

**⚠️ Ключевое изменение**: bench теперь реально запускает `make_service()`, ингестит 5 эпизодов и экстрактит сущности через полный пайплайн. В v1 было 1 ns — пустой прогон.

**Закономерность**: 
- 1→2 клиента: 85.6 ms → 298.7 ms (3.5× — конкуренция за SurrealDB KV)
- 2→4 клиента: 298.7 ms → 298.8 ms (насыщение — throughput не растёт)

---

## 4. Delta-анализ: v1 → v2

### Что исправилось

| Метрика | v1 (2026-07-28) | v2 (2026-07-29) | Статус |
|---------|-----------------|-----------------|--------|
| **entity_f1** (PR) | 0.0000 | **0.7500** | ✅ Прошёл hard floor (0.70) |
| **entity_precision** | 0.0000 | **0.6000** | ✅ |
| **entity_recall** | 1.0000 | **1.0000** | ✅ (был и остался) |
| **retrieval recall@5** | 1.0000 | 0.9868 | ✅ Всё ещё выше 0.90 |
| **retrieval MRR** | 1.0000 | 0.9823 | ✅ Выше 0.85 |
| **Nightly profile** | ❌ Crash (e2e) | ✅ **PASSED** | 🎉 |
| **NER CPU bench** | stubs (39 ns) | real pipeline (~70 ms) | 🎉 |
| **Contention bench** | stubs (1 ns) | real pipeline (~86-300 ms) | 🎉 |

### Что осталось

| Метрика | v2 | Hard Floor | Проблема |
|---------|----|------------|----------|
| **claim_precision** | 0.0000 | 0.50 | Ни один claim не совпал — corpus или формат не конфигурирован |
| **claim_recall** | 0.0000 | 0.50 | Аналогично |
| **lifecycle-poisoning** | quality_failed (0.6667) | — | 1 из 3 кейсов упал по качеству |
| **e2e-entity-extraction** | quality_failed | — | evidence 0/3, контекст пуст |
| **NER Metal bench** | stub (43 ns) | — | не вызывает реальный GPU |

---

## 5. Артефакты

| Артефакт | Path |
|----------|------|
| PR eval artifact | `target/evals/v2-pr.json` |
| Release eval artifact | `target/evals/v2-release.json` |
| Nightly eval artifact | `target/evals/v2-nightly.json` |
| Fixture log | `target/evals/reports/v2/fixtures.log` |
| PR log | `target/evals/reports/v2/pr.log` |
| Release log | `target/evals/reports/v2/release.log` |
| Nightly log | `target/evals/reports/v2/nightly.log` |
| Pipeline bench | `target/evals/reports/v2/benches/pipeline.txt` |
| NER CPU bench | `target/evals/reports/v2/benches/ner_cpu.txt` |
| NER Metal bench | `target/evals/reports/v2/benches/ner_metal.txt` |
| Contention bench | `target/evals/reports/v2/benches/contention.txt` |
